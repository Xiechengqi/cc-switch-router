use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use base64::Engine;
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify, RwLock, watch};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::ServerState;
use crate::config::Config;
use crate::metrics::MetricsRegistry;
use crate::metrics::models::LlmRequestMetric;
use crate::recent_traffic::RecentTraffic;
use crate::store::{
    AppStore, IMAGE_GENERATION_REQUEST_LOG_RETAIN_PER_SHARE, NewImageGenerationRequestLog,
    ShareForTest, image_result_path,
};

const MARKET_REQUEST_ID_HEADER: &str = "x-cc-switch-market-request-id";
const HEALTH_PROBE_FAILURE_CACHE_TTL: Duration = Duration::from_secs(2);
const CLIENT_WEB_USER_EMAIL_HEADER: &str = "x-cc-switch-web-user-email";
const CLIENT_WEB_ROLE_HEADER: &str = "x-cc-switch-web-role";
const CLIENT_WEB_INSTALLATION_ID_HEADER: &str = "x-cc-switch-installation-id";
const CLIENT_WEB_SUBDOMAIN_HEADER: &str = "x-cc-switch-client-tunnel-subdomain";
const SHARE_USER_COUNTRY_HEADER: &str = "X-CC-Switch-User-Country";
const SHARE_USER_COUNTRY_ISO3_HEADER: &str = "X-CC-Switch-User-Country-Iso3";
const SHARE_DATA_SOURCE_HEADER: &str = "X-CC-Switch-Data-Source";
const IMAGE_JOB_MAX_RUNNING_PER_SHARE: usize = 1;
const ROUTE_DRAIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RouteKind {
    Share,
    Market,
    ClientWeb,
}

#[derive(Debug, Clone)]
pub struct RouteShutdown {
    tx: watch::Sender<bool>,
}

impl RouteShutdown {
    pub(crate) fn new() -> (Self, watch::Receiver<bool>) {
        let (tx, rx) = watch::channel(false);
        (Self { tx }, rx)
    }

    pub(crate) fn shutdown(&self) {
        let _ = self.tx.send(true);
    }
}

/// Per-subdomain routing info.
#[derive(Debug, Clone)]
pub(crate) struct RouteEntry {
    backend: String,
    route_kind: RouteKind,
    share_id: Option<String>,
    share_name: Option<String>,
    subdomain: String,
    installation_id: Option<String>,
    connection_id: Option<String>,
    is_free_share: bool,
    parallel_limit: i64,
    shutdown: Option<RouteShutdown>,
    generation: u64,
    rotation_id: String,
    transport: Arc<RouteTransportState>,
}

#[derive(Debug, Default)]
struct RouteTransportState {
    inflight: AtomicUsize,
    idle: Notify,
}

#[derive(Debug)]
struct RouteInflightGuard {
    transport: Arc<RouteTransportState>,
}

impl Drop for RouteInflightGuard {
    fn drop(&mut self) {
        if self.transport.inflight.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.transport.idle.notify_waiters();
        }
    }
}

#[derive(Debug, Default)]
struct LogicalRoute {
    active: Option<RouteEntry>,
    candidates: BTreeMap<u64, RouteEntry>,
    draining: BTreeMap<u64, RouteEntry>,
}

#[derive(Debug)]
enum RouteLookup {
    Unknown,
    Reconnecting,
    Active(RouteEntry, RouteInflightGuard),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RouteGenerationError {
    #[error("route generation {generation} is stale; active generation is {active_generation}")]
    StaleGeneration {
        generation: u64,
        active_generation: u64,
    },
    #[error("route generation {generation} is already registered by another connection")]
    GenerationConflict { generation: u64 },
    #[error("route candidate generation {generation} is not ready")]
    CandidateNotReady { generation: u64 },
    #[error("route candidate identity does not match the activation request")]
    CandidateIdentityMismatch,
    #[error("route generation changed: expected {expected_generation}, active {active_generation}")]
    CompareAndSwapConflict {
        expected_generation: u64,
        active_generation: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProxyRouteState {
    pub active_generation: Option<u64>,
    pub active_connection_id: Option<String>,
    pub candidate_generations: Vec<u64>,
    pub draining_generations: Vec<u64>,
}

#[derive(Debug, Clone)]
struct PendingRouteEntry {
    expires_at: Instant,
}

impl RouteEntry {
    #[allow(clippy::too_many_arguments)]
    fn new(
        backend: String,
        route_kind: RouteKind,
        share_id: Option<String>,
        share_name: Option<String>,
        subdomain: String,
        installation_id: Option<String>,
        connection_id: Option<String>,
        is_free_share: bool,
        parallel_limit: i64,
        shutdown: Option<RouteShutdown>,
        generation: u64,
        rotation_id: String,
    ) -> Self {
        Self {
            backend,
            route_kind,
            share_id,
            share_name,
            subdomain,
            installation_id,
            connection_id,
            is_free_share,
            parallel_limit,
            shutdown,
            generation,
            rotation_id,
            transport: Arc::new(RouteTransportState::default()),
        }
    }

    fn acquire(&self) -> RouteInflightGuard {
        self.transport.inflight.fetch_add(1, Ordering::AcqRel);
        RouteInflightGuard {
            transport: self.transport.clone(),
        }
    }

    pub(crate) fn is_client_web(&self) -> bool {
        self.route_kind == RouteKind::ClientWeb
    }

    pub(crate) fn is_share(&self) -> bool {
        self.route_kind == RouteKind::Share
    }

    pub(crate) fn share_id(&self) -> Option<&str> {
        self.share_id.as_deref()
    }

    pub(crate) fn subdomain(&self) -> &str {
        &self.subdomain
    }

    pub(crate) fn connection_id(&self) -> Option<&str> {
        self.connection_id.as_deref()
    }

    pub(crate) fn installation_id(&self) -> Option<&str> {
        self.installation_id.as_deref()
    }

    /// Local `host:port` the server proxies into to reach this installation's
    /// tunnelled HTTP server. Used by the control-plane RPC client to call the
    /// client's `/_ctl/*` API over the same reverse SSH forward.
    pub(crate) fn route_target(&self) -> &str {
        &self.backend
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn rotation_id(&self) -> &str {
        &self.rotation_id
    }
}

#[derive(Debug, Default)]
struct KeyedConcurrencyLimiter {
    counters: Mutex<HashMap<String, usize>>,
}

#[derive(Debug)]
struct KeyedConcurrencyPermit {
    limiter: Arc<KeyedConcurrencyLimiter>,
    key: String,
}

#[derive(Debug)]
struct ShareConcurrencyPermit {
    _share: KeyedConcurrencyPermit,
    _app: Option<KeyedConcurrencyPermit>,
    _user: Option<KeyedConcurrencyPermit>,
}

impl Drop for KeyedConcurrencyPermit {
    fn drop(&mut self) {
        let limiter = self.limiter.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            let mut counters = limiter.counters.lock().await;
            let should_remove = match counters.get_mut(&key) {
                Some(inflight) if *inflight > 1 => {
                    *inflight -= 1;
                    false
                }
                Some(_) => true,
                None => false,
            };
            if should_remove {
                counters.remove(&key);
            }
        });
    }
}

/// Lifecycle guard that flips a recorded `RecentTraffic` event from
/// in-flight to completed when the proxy's response body stream ends. We
/// pair it with the same drop-then-spawn pattern as
/// [`KeyedConcurrencyPermit`] so the closure that owns the guard never has
/// to be `async`.
#[derive(Debug)]
struct RecentTrafficGuard {
    traffic: RecentTraffic,
    request_id: String,
}

impl Drop for RecentTrafficGuard {
    fn drop(&mut self) {
        let traffic = self.traffic.clone();
        let request_id = std::mem::take(&mut self.request_id);
        if request_id.is_empty() {
            return;
        }
        tokio::spawn(async move {
            traffic.complete(&request_id).await;
        });
    }
}

/// Records a lightweight LLM metric row when a share API proxy request ends.
/// Server-side log sync may later enrich the same `request_id` with token usage.
#[derive(Debug)]
struct ShareLlmProxyMetricsGuard {
    metrics: Arc<MetricsRegistry>,
    request_id: String,
    share_id: String,
    subdomain: String,
    app_type: Option<String>,
    status: u16,
    started: Instant,
}

impl Drop for ShareLlmProxyMetricsGuard {
    fn drop(&mut self) {
        let latency_ms = self.started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        let success = self.status < 400;
        self.metrics.record_llm_request(LlmRequestMetric {
            timestamp: chrono::Utc::now().timestamp(),
            request_id: Some(self.request_id.clone()),
            route_type: "direct".into(),
            market_email: None,
            share_id: Some(self.share_id.clone()),
            subdomain: Some(self.subdomain.clone()),
            app_type: self.app_type.clone(),
            provider: None,
            requested_model: None,
            actual_model: None,
            status: if success {
                "success".into()
            } else {
                "error".into()
            },
            error_kind: if self.status == 429 {
                Some("rate_limited".into())
            } else if success {
                None
            } else {
                Some("upstream_error".into())
            },
            http_status: Some(self.status),
            latency_ms: Some(latency_ms),
            ttft_ms: None,
            stream_started: false,
            stream_completed: success,
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
            estimated_cost_usd: None,
        });
    }
}

fn should_record_share_llm_proxy_metric(
    route: &RouteEntry,
    path: &str,
    is_share_router_probe: bool,
    skips_share_edge_auth: bool,
) -> bool {
    route.is_share()
        && !is_share_router_probe
        && !skips_share_edge_auth
        && is_allowed_direct_share_api_path(path)
}

fn share_llm_proxy_metrics_guard(
    state: &ServerState,
    route: &RouteEntry,
    path: &str,
    is_share_router_probe: bool,
    skips_share_edge_auth: bool,
    request_id: Option<&str>,
    status: u16,
    started: Instant,
    app_type: Option<String>,
) -> Option<ShareLlmProxyMetricsGuard> {
    if !should_record_share_llm_proxy_metric(
        route,
        path,
        is_share_router_probe,
        skips_share_edge_auth,
    ) {
        return None;
    }
    let share_id = route.share_id.clone()?;
    let request_id = request_id.filter(|value| !value.is_empty())?.to_string();
    Some(ShareLlmProxyMetricsGuard {
        metrics: state.metrics.clone(),
        request_id,
        share_id,
        subdomain: route.subdomain.clone(),
        app_type,
        status,
        started,
    })
}

impl KeyedConcurrencyLimiter {
    /// Increment the in-flight counter for this key. Returns `None` when a
    /// non-negative `parallel_limit` has been reached (caller should reject the
    /// request). A negative `parallel_limit` means unlimited — we still track
    /// the in-flight count so it can be surfaced in the dashboard.
    async fn try_acquire(
        self: &Arc<Self>,
        key: &str,
        parallel_limit: i64,
    ) -> Option<KeyedConcurrencyPermit> {
        let mut counters = self.counters.lock().await;
        let inflight = counters.entry(key.to_string()).or_insert(0);
        if parallel_limit >= 0 {
            let limit = parallel_limit as usize;
            if *inflight >= limit {
                return None;
            }
        }
        *inflight += 1;
        Some(KeyedConcurrencyPermit {
            limiter: self.clone(),
            key: key.to_string(),
        })
    }

    async fn snapshot(&self) -> HashMap<String, usize> {
        self.counters.lock().await.clone()
    }
}

#[derive(Debug, Default)]
pub struct ProxyRegistry {
    routes: Arc<RwLock<HashMap<String, LogicalRoute>>>,
    pending_routes: RwLock<HashMap<String, PendingRouteEntry>>,
    health_probe_failures: Mutex<HashMap<String, Instant>>,
    share_limiter: Arc<KeyedConcurrencyLimiter>,
    share_app_limiter: Arc<KeyedConcurrencyLimiter>,
    share_user_inflight: Arc<KeyedConcurrencyLimiter>,
    free_share_ip_limiter: Arc<KeyedConcurrencyLimiter>,
    image_limiter: Arc<KeyedConcurrencyLimiter>,
    /// Tracks requests that actually traversed the market proxy path, keyed by
    /// lowercased market email. Independent from `share_limiter` so a request
    /// that hits a share's own subdomain directly is not counted against the
    /// market it happens to be linked to.
    market_limiter: Arc<KeyedConcurrencyLimiter>,
}

#[derive(Debug, Clone, Copy)]
pub struct ProxyRegistryCounts {
    pub active_routes: usize,
    pub pending_routes: usize,
    pub health_probe_failure_cache: usize,
}

impl ProxyRegistry {
    pub async fn set_route(
        &self,
        subdomain: String,
        backend: String,
        connection_id: Option<String>,
        share_id: Option<String>,
        share_name: Option<String>,
        is_free_share: bool,
        parallel_limit: i64,
        shutdown: Option<RouteShutdown>,
    ) {
        let route_kind = if share_id.is_some() {
            RouteKind::Share
        } else {
            RouteKind::Market
        };
        self.set_route_with_kind(
            subdomain,
            backend,
            route_kind,
            None,
            connection_id,
            share_id,
            share_name,
            is_free_share,
            parallel_limit,
            shutdown,
        )
        .await;
    }

    pub(crate) async fn set_route_with_kind(
        &self,
        subdomain: String,
        backend: String,
        route_kind: RouteKind,
        installation_id: Option<String>,
        connection_id: Option<String>,
        share_id: Option<String>,
        share_name: Option<String>,
        is_free_share: bool,
        parallel_limit: i64,
        shutdown: Option<RouteShutdown>,
    ) {
        self.pending_routes.write().await.remove(&subdomain);
        let rotation_id = connection_id
            .clone()
            .unwrap_or_else(|| format!("legacy:{}", Uuid::new_v4()));
        let (subdomain, old_route) = {
            let mut routes = self.routes.write().await;
            let slot = routes.entry(subdomain.clone()).or_default();
            let generation = slot
                .active
                .iter()
                .map(|route| route.generation)
                .chain(slot.candidates.keys().copied())
                .chain(slot.draining.keys().copied())
                .max()
                .unwrap_or(0)
                .saturating_add(1);
            let route = RouteEntry::new(
                backend,
                route_kind,
                share_id,
                share_name,
                subdomain.clone(),
                installation_id,
                connection_id,
                is_free_share,
                parallel_limit,
                shutdown,
                generation,
                rotation_id,
            );
            let old_route = slot.active.replace(route);
            if let Some(old_route) = old_route.as_ref() {
                slot.draining
                    .insert(old_route.generation, old_route.clone());
            }
            (subdomain, old_route)
        };
        if let Some(old_route) = old_route {
            self.schedule_route_drain(subdomain, old_route);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn register_candidate_with_kind(
        &self,
        subdomain: String,
        backend: String,
        route_kind: RouteKind,
        installation_id: Option<String>,
        connection_id: Option<String>,
        share_id: Option<String>,
        share_name: Option<String>,
        is_free_share: bool,
        parallel_limit: i64,
        shutdown: Option<RouteShutdown>,
        generation: u64,
        rotation_id: String,
    ) -> Result<(), RouteGenerationError> {
        let mut routes = self.routes.write().await;
        let slot = routes.entry(subdomain.clone()).or_default();
        let active_generation = slot
            .active
            .as_ref()
            .map(|route| route.generation)
            .unwrap_or(0);
        if generation <= active_generation {
            return Err(RouteGenerationError::StaleGeneration {
                generation,
                active_generation,
            });
        }
        if slot.candidates.contains_key(&generation) {
            return Err(RouteGenerationError::GenerationConflict { generation });
        }
        slot.candidates.insert(
            generation,
            RouteEntry::new(
                backend,
                route_kind,
                share_id,
                share_name,
                subdomain,
                installation_id,
                connection_id,
                is_free_share,
                parallel_limit,
                shutdown,
                generation,
                rotation_id,
            ),
        );
        Ok(())
    }

    pub(crate) async fn promote_candidate(
        &self,
        subdomain: &str,
        connection_id: &str,
        rotation_id: &str,
        generation: u64,
        expected_generation: u64,
    ) -> Result<ProxyRouteState, RouteGenerationError> {
        let (old_route, stale_candidates, state) = {
            let mut routes = self.routes.write().await;
            let slot = routes.entry(subdomain.to_string()).or_default();
            let active_generation = slot
                .active
                .as_ref()
                .map(|route| route.generation)
                .unwrap_or(0);

            if let Some(active) = slot.active.as_ref() {
                if active.generation == generation
                    && active.connection_id() == Some(connection_id)
                    && active.rotation_id == rotation_id
                {
                    return Ok(proxy_route_state(slot));
                }
            }
            let recovering_persisted_head = active_generation == 0 && expected_generation > 0;
            if active_generation != expected_generation && !recovering_persisted_head {
                return Err(RouteGenerationError::CompareAndSwapConflict {
                    expected_generation,
                    active_generation,
                });
            }

            let candidate = slot
                .candidates
                .remove(&generation)
                .ok_or(RouteGenerationError::CandidateNotReady { generation })?;
            if candidate.connection_id() != Some(connection_id)
                || candidate.rotation_id != rotation_id
            {
                slot.candidates.insert(generation, candidate);
                return Err(RouteGenerationError::CandidateIdentityMismatch);
            }
            if generation <= active_generation {
                slot.candidates.insert(generation, candidate);
                return Err(RouteGenerationError::StaleGeneration {
                    generation,
                    active_generation,
                });
            }

            let old_route = slot.active.replace(candidate);
            if let Some(old_route) = old_route.as_ref() {
                slot.draining
                    .insert(old_route.generation, old_route.clone());
            }
            let stale_generations = slot
                .candidates
                .range(..=generation)
                .map(|(generation, _)| *generation)
                .collect::<Vec<_>>();
            let stale_candidates = stale_generations
                .into_iter()
                .filter_map(|generation| slot.candidates.remove(&generation))
                .collect::<Vec<_>>();
            (old_route, stale_candidates, proxy_route_state(slot))
        };
        self.pending_routes.write().await.remove(subdomain);
        for candidate in stale_candidates {
            if let Some(shutdown) = candidate.shutdown {
                shutdown.shutdown();
            }
        }
        if let Some(old_route) = old_route {
            self.schedule_route_drain(subdomain.to_string(), old_route);
        }
        Ok(state)
    }

    pub(crate) async fn rollback_candidate_promotion(
        &self,
        subdomain: &str,
        connection_id: &str,
        rotation_id: &str,
        generation: u64,
        expected_generation: u64,
    ) {
        let mut routes = self.routes.write().await;
        let Some(slot) = routes.get_mut(subdomain) else {
            return;
        };
        let promoted_matches = slot.active.as_ref().is_some_and(|route| {
            route.generation == generation
                && route.connection_id() == Some(connection_id)
                && route.rotation_id() == rotation_id
        });
        if !promoted_matches {
            return;
        }
        let Some(promoted) = slot.active.take() else {
            return;
        };
        if expected_generation > 0 {
            slot.active = slot.draining.remove(&expected_generation);
        }
        slot.candidates.insert(generation, promoted);
    }

    pub(crate) async fn route_state(&self, subdomain: &str) -> Option<ProxyRouteState> {
        self.routes
            .read()
            .await
            .get(subdomain)
            .map(proxy_route_state)
    }

    pub(crate) async fn candidate_for_activation(
        &self,
        subdomain: &str,
        connection_id: &str,
        rotation_id: &str,
        generation: u64,
    ) -> Option<RouteEntry> {
        self.routes
            .read()
            .await
            .get(subdomain)
            .and_then(|slot| slot.candidates.get(&generation))
            .filter(|route| {
                route.connection_id() == Some(connection_id) && route.rotation_id() == rotation_id
            })
            .cloned()
    }

    pub(crate) async fn next_generation(&self, subdomain: &str) -> u64 {
        self.routes
            .read()
            .await
            .get(subdomain)
            .map(|slot| {
                slot.active
                    .iter()
                    .map(|route| route.generation)
                    .chain(slot.candidates.keys().copied())
                    .chain(slot.draining.keys().copied())
                    .max()
                    .unwrap_or(0)
                    .saturating_add(1)
            })
            .unwrap_or(1)
    }

    pub(crate) async fn active_generation(&self, subdomain: &str) -> u64 {
        self.routes
            .read()
            .await
            .get(subdomain)
            .and_then(|slot| slot.active.as_ref())
            .map(|route| route.generation)
            .unwrap_or(0)
    }

    pub async fn mark_route_pending(&self, subdomain: String, ttl: Duration) {
        self.declare_known_route(subdomain.clone()).await;
        self.pending_routes.write().await.insert(
            subdomain,
            PendingRouteEntry {
                expires_at: Instant::now() + ttl,
            },
        );
    }

    pub(crate) async fn declare_known_route(&self, subdomain: String) {
        self.routes.write().await.entry(subdomain).or_default();
    }

    pub async fn has_pending_route(&self, subdomain: &str) -> bool {
        let now = Instant::now();
        let mut pending = self.pending_routes.write().await;
        pending.retain(|_, entry| entry.expires_at > now);
        pending.contains_key(subdomain)
    }

    pub async fn remove_route(&self, subdomain: &str) {
        let old_route = self.routes.write().await.remove(subdomain);
        self.pending_routes.write().await.remove(subdomain);
        shutdown_logical_route(old_route);
    }

    pub async fn remove_route_if_present(&self, subdomain: &str) -> bool {
        let old_route = self.routes.write().await.remove(subdomain);
        let removed = old_route.is_some();
        self.pending_routes.write().await.remove(subdomain);
        shutdown_logical_route(old_route);
        removed
    }

    pub async fn remove_route_if_connection(&self, subdomain: &str, connection_id: &str) -> bool {
        let mut routes = self.routes.write().await;
        let Some(slot) = routes.get_mut(subdomain) else {
            return false;
        };
        let mut removed = Vec::new();
        if slot.active.as_ref().and_then(RouteEntry::connection_id) == Some(connection_id) {
            if let Some(route) = slot.active.take() {
                removed.push(route);
            }
        }
        let candidate_generations = slot
            .candidates
            .iter()
            .filter_map(|(generation, route)| {
                (route.connection_id() == Some(connection_id)).then_some(*generation)
            })
            .collect::<Vec<_>>();
        for generation in candidate_generations {
            if let Some(route) = slot.candidates.remove(&generation) {
                removed.push(route);
            }
        }
        let draining_generations = slot
            .draining
            .iter()
            .filter_map(|(generation, route)| {
                (route.connection_id() == Some(connection_id)).then_some(*generation)
            })
            .collect::<Vec<_>>();
        for generation in draining_generations {
            if let Some(route) = slot.draining.remove(&generation) {
                removed.push(route);
            }
        }
        drop(routes);
        let should_remove = !removed.is_empty();
        for route in removed {
            if let Some(shutdown) = route.shutdown {
                shutdown.shutdown();
            }
        }
        should_remove
    }

    pub(crate) async fn remove_route_target_if_generation(
        &self,
        subdomain: &str,
        connection_id: &str,
        generation: u64,
    ) -> bool {
        let mut routes = self.routes.write().await;
        let Some(slot) = routes.get_mut(subdomain) else {
            return false;
        };
        let mut removed = None;
        if slot.active.as_ref().is_some_and(|route| {
            route.generation == generation && route.connection_id() == Some(connection_id)
        }) {
            removed = slot.active.take();
        } else if slot
            .candidates
            .get(&generation)
            .is_some_and(|route| route.connection_id() == Some(connection_id))
        {
            removed = slot.candidates.remove(&generation);
        } else if slot
            .draining
            .get(&generation)
            .is_some_and(|route| route.connection_id() == Some(connection_id))
        {
            removed = slot.draining.remove(&generation);
        }
        drop(routes);
        if let Some(route) = removed {
            if let Some(shutdown) = route.shutdown {
                shutdown.shutdown();
            }
            true
        } else {
            false
        }
    }

    pub async fn active_route_connections(&self) -> HashMap<String, Option<String>> {
        self.routes
            .read()
            .await
            .iter()
            .filter_map(|(subdomain, slot)| {
                slot.active
                    .as_ref()
                    .map(|route| (subdomain.clone(), route.connection_id.clone()))
            })
            .collect()
    }

    pub(crate) async fn backend_for_host(
        &self,
        host: &str,
        tunnel_domain: &str,
    ) -> Option<RouteEntry> {
        let subdomain = subdomain_for_host(host, tunnel_domain)?;
        self.routes
            .read()
            .await
            .get(&subdomain)
            .and_then(|slot| slot.active.clone())
    }

    async fn route_for_host_request(&self, host: &str, tunnel_domain: &str) -> RouteLookup {
        let Some(subdomain) = subdomain_for_host(host, tunnel_domain) else {
            return RouteLookup::Unknown;
        };
        let routes = self.routes.read().await;
        let Some(slot) = routes.get(&subdomain) else {
            return RouteLookup::Unknown;
        };
        match slot.active.as_ref() {
            Some(route) => RouteLookup::Active(route.clone(), route.acquire()),
            None => RouteLookup::Reconnecting,
        }
    }

    pub(crate) async fn route_by_share_id(&self, share_id: &str) -> Option<RouteEntry> {
        self.routes
            .read()
            .await
            .values()
            .filter_map(|slot| slot.active.as_ref())
            .find(|route| route.share_id.as_deref() == Some(share_id))
            .cloned()
    }

    async fn route_for_share_request(
        &self,
        share_id: &str,
    ) -> Option<(RouteEntry, RouteInflightGuard)> {
        let routes = self.routes.read().await;
        let route = routes
            .values()
            .filter_map(|slot| slot.active.as_ref())
            .find(|route| route.share_id.as_deref() == Some(share_id))?;
        Some((route.clone(), route.acquire()))
    }

    pub async fn active_subdomains(&self) -> Vec<String> {
        self.routes
            .read()
            .await
            .iter()
            .filter_map(|(subdomain, slot)| slot.active.as_ref().map(|_| subdomain.clone()))
            .collect()
    }

    pub async fn counts(&self) -> ProxyRegistryCounts {
        let now = Instant::now();
        let mut pending = self.pending_routes.write().await;
        pending.retain(|_, entry| entry.expires_at > now);
        let mut failures = self.health_probe_failures.lock().await;
        failures.retain(|_, expires_at| *expires_at > now);
        ProxyRegistryCounts {
            active_routes: self
                .routes
                .read()
                .await
                .values()
                .filter(|slot| slot.active.is_some())
                .count(),
            pending_routes: pending.len(),
            health_probe_failure_cache: failures.len(),
        }
    }

    fn schedule_route_drain(&self, subdomain: String, route: RouteEntry) {
        let routes = self.routes.clone();
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + ROUTE_DRAIN_TIMEOUT;
            loop {
                if route.transport.inflight.load(Ordering::Acquire) == 0 {
                    break;
                }
                if tokio::time::timeout_at(deadline, route.transport.idle.notified())
                    .await
                    .is_err()
                {
                    warn!(
                        subdomain = %subdomain,
                        generation = route.generation,
                        connection_id = route.connection_id().unwrap_or("-"),
                        inflight = route.transport.inflight.load(Ordering::Acquire),
                        "route drain deadline reached; closing old transport"
                    );
                    break;
                }
            }
            let mut routes = routes.write().await;
            if let Some(slot) = routes.get_mut(&subdomain) {
                let matches = slot.draining.get(&route.generation).is_some_and(|current| {
                    current.connection_id == route.connection_id
                        && current.rotation_id == route.rotation_id
                });
                if matches {
                    slot.draining.remove(&route.generation);
                    if let Some(shutdown) = route.shutdown.as_ref() {
                        shutdown.shutdown();
                    }
                }
            }
        });
    }

    /// Snapshot of in-flight request counts per share_id. Share IDs absent from
    /// the map have zero in-flight requests.
    pub async fn inflight_by_share(&self) -> HashMap<String, usize> {
        self.share_limiter.snapshot().await
    }

    /// Snapshot of in-flight request counts per share_id and app_type. Unknown
    /// app requests are intentionally omitted from this app-level view while
    /// still counted by `inflight_by_share`.
    pub async fn inflight_by_share_app(&self) -> HashMap<String, BTreeMap<String, usize>> {
        let snapshot = self.share_app_limiter.snapshot().await;
        let mut result = HashMap::<String, BTreeMap<String, usize>>::new();
        for (key, count) in snapshot {
            let Some((share_id, app)) = key.split_once(':') else {
                continue;
            };
            result
                .entry(share_id.to_string())
                .or_default()
                .insert(app.to_string(), count);
        }
        result
    }

    /// Snapshot of in-flight request counts per share, app, and user email.
    /// Keys are `{share_id}:{app}:{email}`; unknown app is stored as `_`.
    pub async fn inflight_by_share_user(
        &self,
    ) -> HashMap<String, BTreeMap<String, BTreeMap<String, usize>>> {
        let snapshot = self.share_user_inflight.snapshot().await;
        let mut result = HashMap::<String, BTreeMap<String, BTreeMap<String, usize>>>::new();
        for (key, count) in snapshot {
            let mut parts = key.splitn(3, ':');
            let Some(share_id) = parts.next() else {
                continue;
            };
            let Some(app) = parts.next() else {
                continue;
            };
            let Some(user) = parts.next() else {
                continue;
            };
            result
                .entry(share_id.to_string())
                .or_default()
                .entry(app.to_string())
                .or_default()
                .insert(user.to_string(), count);
        }
        result
    }

    /// Snapshot of in-flight request counts per market email (lowercased).
    /// Only requests that came through the market proxy handler are counted —
    /// direct share-subdomain traffic is not.
    pub async fn inflight_by_market_email(&self) -> HashMap<String, usize> {
        self.market_limiter.snapshot().await
    }

    pub(crate) async fn has_cached_health_probe_failure(&self, subdomain: &str) -> bool {
        let now = Instant::now();
        let mut failures = self.health_probe_failures.lock().await;
        failures.retain(|_, expires_at| *expires_at > now);
        failures.contains_key(subdomain)
    }

    pub(crate) async fn record_health_probe_failure(&self, subdomain: String) {
        self.health_probe_failures
            .lock()
            .await
            .insert(subdomain, Instant::now() + HEALTH_PROBE_FAILURE_CACHE_TTL);
    }

    pub(crate) async fn clear_health_probe_failure(&self, subdomain: &str) {
        self.health_probe_failures.lock().await.remove(subdomain);
    }

    #[cfg(test)]
    pub async fn set_share_inflight_for_test(&self, share_id: &str, count: usize) {
        for _ in 0..count {
            if let Some(permit) = self
                .try_acquire_share_permit(share_id, None, -1, None)
                .await
            {
                std::mem::forget(permit);
            }
        }
    }

    /// Acquire a tracking-only permit for a market-routed request. We pass an
    /// unlimited parallel cap (`-1`) because the rate gate is applied at the
    /// share level; this permit exists purely to drive the dashboard's
    /// PARALLEL aggregate.
    async fn acquire_market_permit(&self, market_email: &str) -> KeyedConcurrencyPermit {
        let key = market_email.to_ascii_lowercase();
        // Unlimited cap means try_acquire never returns None.
        self.market_limiter
            .try_acquire(&key, -1)
            .await
            .expect("unlimited market permit cannot be denied")
    }

    async fn try_acquire_share_permit(
        &self,
        share_id: &str,
        app_type: Option<&str>,
        parallel_limit: i64,
        user_email: Option<&str>,
    ) -> Option<ShareConcurrencyPermit> {
        let share = self
            .share_limiter
            .try_acquire(share_id, parallel_limit)
            .await?;
        let app = app_type
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .filter(|value| matches!(value.as_str(), "claude" | "codex" | "gemini"));
        let app_permit = match app.as_deref() {
            Some(app) => {
                let key = format!("{share_id}:{app}");
                self.share_app_limiter.try_acquire(&key, -1).await
            }
            None => None,
        };
        let user = match user_email.map(str::trim).filter(|value| !value.is_empty()) {
            Some(email) => {
                let app_key = app.clone().unwrap_or_else(|| "_".to_string());
                let key = format!("{share_id}:{app_key}:{}", email.to_ascii_lowercase());
                self.share_user_inflight.try_acquire(&key, -1).await
            }
            None => None,
        };
        Some(ShareConcurrencyPermit {
            _share: share,
            _app: app_permit,
            _user: user,
        })
    }

    async fn try_acquire_free_share_ip_permit(
        &self,
        user_ip: &str,
        parallel_limit: i64,
    ) -> Option<KeyedConcurrencyPermit> {
        self.free_share_ip_limiter
            .try_acquire(user_ip, parallel_limit)
            .await
    }

    async fn try_acquire_image_permit(
        &self,
        share_id: &str,
        parallel_limit: i64,
    ) -> Option<KeyedConcurrencyPermit> {
        self.image_limiter
            .try_acquire(share_id, parallel_limit)
            .await
    }
}

fn proxy_route_state(slot: &LogicalRoute) -> ProxyRouteState {
    ProxyRouteState {
        active_generation: slot.active.as_ref().map(|route| route.generation),
        active_connection_id: slot
            .active
            .as_ref()
            .and_then(|route| route.connection_id.clone()),
        candidate_generations: slot.candidates.keys().copied().collect(),
        draining_generations: slot.draining.keys().copied().collect(),
    }
}

fn shutdown_logical_route(route: Option<LogicalRoute>) {
    let Some(route) = route else {
        return;
    };
    for route in route
        .active
        .into_iter()
        .chain(route.candidates.into_values())
        .chain(route.draining.into_values())
    {
        if let Some(shutdown) = route.shutdown {
            shutdown.shutdown();
        }
    }
}

pub async fn market_proxy_handler(
    State(state): State<ServerState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    let (parts, body) = req.into_parts();
    let method = parts.method.clone();
    let host = parts
        .headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let path = parts.uri.path().to_string();
    if path.starts_with("/_ctl/") || path == "/_ctl" {
        return simple_response(StatusCode::NOT_FOUND, "not-found");
    }
    let query = parts
        .uri
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let client_metadata = crate::client_meta::extract_client_metadata(&parts.headers, peer);
    let user_ip = client_metadata
        .ip
        .clone()
        .unwrap_or_else(|| peer.ip().to_string());
    let user_country = client_metadata.country_code.as_deref().unwrap_or("-");
    let user_asn = trusted_asn_header(&parts.headers, peer);
    let user_agent = header_str(&parts.headers, "user-agent");

    let Some(token) = bearer_token(&parts.headers) else {
        return simple_response(StatusCode::UNAUTHORIZED, "missing-market-bearer-token");
    };
    let market = match state
        .store
        .authenticate_market_session(token, "market:proxy:use")
        .await
    {
        Ok(market) => market,
        Err(err) => {
            warn!(
                client_ip = %user_ip,
                client_country = %user_country,
                client_asn = %user_asn,
                user_agent = %user_agent,
                error = %err,
                "market proxy authentication failed"
            );
            return simple_response(StatusCode::UNAUTHORIZED, "invalid-market-session");
        }
    };
    let market_email = market.email.clone();
    let market_subdomain = market.subdomain.clone();

    if subdomain_for_host(&host, &state.config.tunnel_domain).as_deref()
        != Some(market_subdomain.as_str())
    {
        warn!(
            method = %method,
            host = %host,
            expected_subdomain = %market_subdomain,
            path = %path,
            client_ip = %user_ip,
            client_country = %user_country,
            client_asn = %user_asn,
            user_agent = %user_agent,
            "market proxy rejected: host does not match authenticated market"
        );
        return simple_response(StatusCode::FORBIDDEN, "market-host-mismatch");
    }

    let Some(rest) = path.strip_prefix("/_market/proxy/") else {
        return simple_response(StatusCode::NOT_FOUND, "invalid-market-proxy-path");
    };
    let (share_id, forwarded_path) = match rest.split_once('/') {
        Some((share_id, forwarded_path)) if !share_id.is_empty() => {
            (share_id.to_string(), format!("/{forwarded_path}"))
        }
        _ if !rest.is_empty() => (rest.to_string(), "/".to_string()),
        _ => return simple_response(StatusCode::NOT_FOUND, "missing-share-id"),
    };
    let path_and_query = format!("{forwarded_path}{query}");

    let active_subdomains = state.proxy.active_subdomains().await.into_iter().collect();
    let inflight_by_share = state.proxy.inflight_by_share().await;
    let authorized = match state
        .store
        .list_market_shares(
            &market_email,
            "main",
            &active_subdomains,
            &inflight_by_share,
            true,
        )
        .await
    {
        Ok(shares) => {
            let Some(share) = shares.into_iter().find(|share| share.share_id == share_id) else {
                return simple_response(StatusCode::FORBIDDEN, "share-not-authorized-for-market");
            };
            if share.disabled_by_market {
                return simple_response(StatusCode::FORBIDDEN, "share-disabled-by-market");
            }
            true
        }
        Err(err) => {
            warn!(error = %err, "market proxy share authorization lookup failed");
            return simple_response(StatusCode::SERVICE_UNAVAILABLE, "share-lookup-failed");
        }
    };
    if !authorized {
        return simple_response(StatusCode::FORBIDDEN, "share-not-authorized-for-market");
    }

    let Some((route, route_inflight_guard)) = state.proxy.route_for_share_request(&share_id).await
    else {
        return simple_response(StatusCode::NOT_FOUND, "share-offline");
    };
    let backend = route.backend.clone();
    let target = format!("http://{backend}{path_and_query}");

    let metrics_permit = state.metrics.proxy_request_started();
    let mut builder = state.proxy_http.request(method.clone(), target);
    for (name, value) in &parts.headers {
        let n = name.as_str();
        if n.eq_ignore_ascii_case("host")
            || n.eq_ignore_ascii_case("authorization")
            || n.eq_ignore_ascii_case(MARKET_REQUEST_ID_HEADER)
            || is_internal_share_context_header(n)
            || is_hop_by_hop_header(n)
        {
            continue;
        }
        builder = builder.header(name, value);
    }
    builder = builder.header("X-CC-Switch-Share-Id", share_id.as_str());
    builder = builder.header(SHARE_DATA_SOURCE_HEADER, "market");

    let log_share_id = mask_token(&share_id);
    let request_app = infer_share_request_app(&path_and_query, &parts.headers);
    let share_permit = match state
        .proxy
        .try_acquire_share_permit(
            &share_id,
            request_app.as_deref(),
            route.parallel_limit,
            Some(market_email.as_str()),
        )
        .await
    {
        Some(permit) => permit,
        None => {
            warn!(
                method = %method,
                host = %host,
                path = %path_and_query,
                share_id = %share_id,
                parallel_limit = route.parallel_limit,
                client_ip = %user_ip,
                client_country = %user_country,
                client_asn = %user_asn,
                user_agent = %user_agent,
                "market proxy rejected: share concurrency limit exceeded"
            );
            return simple_response(
                StatusCode::TOO_MANY_REQUESTS,
                "share-concurrency-limit-exceeded",
            );
        }
    };

    let free_share_ip_permit = if route.is_free_share && state.config.free_share_ip_limit_enabled()
    {
        match state
            .proxy
            .try_acquire_free_share_ip_permit(&user_ip, state.config.free_share_ip_parallel_limit)
            .await
        {
            Some(permit) => Some(permit),
            None => {
                return simple_response(
                    StatusCode::TOO_MANY_REQUESTS,
                    "free-share-ip-concurrency-limit-exceeded",
                );
            }
        }
    } else {
        None
    };

    // Tracking-only permit so the dashboard's market PARALLEL counter only
    // reflects requests that actually traversed the market proxy.
    let market_permit = state.proxy.acquire_market_permit(&market_email).await;

    let live_request_id = if let Some(request_id) =
        header_str(&parts.headers, MARKET_REQUEST_ID_HEADER)
            .split_whitespace()
            .next()
            .filter(|value| is_valid_market_request_id(value))
            .map(ToOwned::to_owned)
    {
        state
            .recent_traffic
            .record_with_id(
                request_id.clone(),
                share_id.clone(),
                route.share_name.clone(),
                Some(route.subdomain.clone()),
                client_metadata.country_code.clone(),
                None,
            )
            .await;
        Some(request_id)
    } else {
        Some(
            state
                .recent_traffic
                .record(
                    share_id.clone(),
                    route.share_name.clone(),
                    Some(route.subdomain.clone()),
                    client_metadata.country_code.clone(),
                    None,
                )
                .await,
        )
    };
    if let Some(ref request_id) = live_request_id {
        builder = builder.header("X-CC-Switch-Request-Id", request_id.as_str());
    }
    if route.is_share() || route.is_client_web() {
        let installation_id = route.installation_id().unwrap_or_default();
        let control_secret = match state
            .store
            .installation_control_secret(installation_id)
            .await
        {
            Ok(Some(secret)) if !secret.trim().is_empty() => secret,
            Ok(_) => {
                return simple_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "ingress-control-secret-missing",
                );
            }
            Err(error) => {
                warn!(
                    installation_id,
                    error = %error,
                    "proxy ingress context secret lookup failed"
                );
                return simple_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "ingress-control-secret-lookup-failed",
                );
            }
        };
        let request_id = live_request_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let route_id = route
            .share_id()
            .map(|share_id| format!("share:{share_id}"))
            .unwrap_or_else(|| format!("client:{installation_id}"));
        let signed = match crate::ingress_context::sign(
            crate::ingress_context::IngressContext {
                protocol_epoch: crate::namespace::PROTOCOL_EPOCH.to_string(),
                router_id: state
                    .config
                    .tunnel_domain
                    .trim_end_matches('.')
                    .to_ascii_lowercase(),
                route_id,
                installation_id: installation_id.to_string(),
                target_lane_id: installation_id.to_string(),
                public_host: format!("{}.{}", route.subdomain, state.config.tunnel_domain),
                share_id: route.share_id.clone(),
                request_id,
                user_email: None,
                user_role: None,
                user_country: client_metadata.country_code.clone(),
                issued_at_ms: chrono::Utc::now().timestamp_millis(),
            },
            &control_secret,
        ) {
            Ok(signed) => signed,
            Err(error) => {
                warn!(error, "proxy ingress context signing failed");
                return simple_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "ingress-context-signing-failed",
                );
            }
        };
        builder = builder
            .header(
                crate::ingress_context::INGRESS_CONTEXT_HEADER,
                signed.encoded_context,
            )
            .header(
                crate::ingress_context::INGRESS_SIGNATURE_HEADER,
                signed.signature,
            );
    }
    builder = with_share_user_country_headers(builder, client_metadata.country_code.as_deref());
    let recent_traffic_guard = live_request_id.as_ref().map(|id| RecentTrafficGuard {
        traffic: state.recent_traffic.clone(),
        request_id: id.clone(),
    });

    let upstream = match builder
        .body(reqwest::Body::wrap_stream(body.into_data_stream()))
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            state.metrics.record_proxy_upstream_error(false);
            warn!(
                method = %method,
                host = %host,
                path = %path_and_query,
                backend = %backend,
                share_id = %log_share_id,
                client_ip = %user_ip,
                client_country = %user_country,
                client_asn = %user_asn,
                user_agent = %user_agent,
                error = %err,
                "market proxy upstream request failed"
            );
            return simple_response(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!("connection-lost: {err}"),
            );
        }
    };

    let status = upstream.status();
    state.metrics.record_proxy_status(status);
    let response_headers = upstream.headers().clone();
    let is_event_stream = is_event_stream_response(&response_headers);
    let body_stream = upstream
        .bytes_stream()
        .scan(false, move |stream_ended, chunk| {
            let _route_inflight_guard = &route_inflight_guard;
            let _permit = &share_permit;
            let _free_share_ip_permit = &free_share_ip_permit;
            let _market_permit = &market_permit;
            let _recent_traffic_guard = &recent_traffic_guard;
            let _metrics_permit = &metrics_permit;
            let output = proxy_body_chunk(is_event_stream, stream_ended, chunk);
            futures_util::future::ready(output)
        });
    let body = Body::from_stream(body_stream);

    let mut response = Response::new(body);
    *response.status_mut() = status;
    response.headers_mut().clear();
    for (name, value) in &response_headers {
        if is_hop_by_hop_header(name.as_str()) {
            continue;
        }
        response.headers_mut().append(name.clone(), value.clone());
    }
    strip_connection_listed_headers(response.headers_mut());
    if is_event_stream {
        response.headers_mut().remove(header::CONTENT_LENGTH);
    }
    info!(
        method = %method,
        host = %host,
        path = %path_and_query,
        share_id = %share_id,
        backend = %backend,
        status = %status.as_u16(),
        share_id = %log_share_id,
        client_ip = %user_ip,
        client_country = %user_country,
        client_asn = %user_asn,
        user_agent = %user_agent,
        "market proxy request completed"
    );
    response
}

pub async fn gateway_proxy_handler(
    State(state): State<ServerState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    let (parts, body) = req.into_parts();
    let method = parts.method.clone();
    let host = parts
        .headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let path = parts.uri.path().to_string();
    if path.starts_with("/_ctl/") || path == "/_ctl" {
        return simple_response(StatusCode::NOT_FOUND, "not-found");
    }
    let query = parts
        .uri
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let client_metadata = crate::client_meta::extract_client_metadata(&parts.headers, peer);
    let user_ip = client_metadata
        .ip
        .clone()
        .unwrap_or_else(|| peer.ip().to_string());
    let user_country = client_metadata.country_code.as_deref().unwrap_or("-");
    let user_asn = trusted_asn_header(&parts.headers, peer);
    let user_agent = header_str(&parts.headers, "user-agent");

    let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(err) => {
            warn!(
                method = %method,
                host = %host,
                path = %path,
                client_ip = %user_ip,
                client_country = %user_country,
                client_asn = %user_asn,
                user_agent = %user_agent,
                error = %err,
                "gateway proxy request body read failed"
            );
            return simple_response(StatusCode::BAD_REQUEST, "failed-to-read-body");
        }
    };
    let body_hash = crate::api::sha256_hex(&body_bytes);
    let gateway = match authenticate_gateway_proxy(&state, &parts.headers, &body_hash).await {
        Ok(gateway) => gateway,
        Err(err) => {
            warn!(
                method = %method,
                host = %host,
                path = %path,
                client_ip = %user_ip,
                client_country = %user_country,
                client_asn = %user_asn,
                user_agent = %user_agent,
                error = %err,
                "gateway proxy authentication failed"
            );
            return simple_response(StatusCode::UNAUTHORIZED, "invalid-gateway-signature");
        }
    };

    let Some(rest) = path.strip_prefix("/_gateway/proxy/") else {
        return simple_response(StatusCode::NOT_FOUND, "invalid-gateway-proxy-path");
    };
    let (share_id, forwarded_path) = match rest.split_once('/') {
        Some((share_id, forwarded_path)) if !share_id.is_empty() => {
            (share_id.to_string(), format!("/{forwarded_path}"))
        }
        _ if !rest.is_empty() => (rest.to_string(), "/".to_string()),
        _ => return simple_response(StatusCode::NOT_FOUND, "missing-share-id"),
    };
    let path_and_query = format!("{forwarded_path}{query}");

    let active_subdomains = state.proxy.active_subdomains().await.into_iter().collect();
    let inflight_by_share = state.proxy.inflight_by_share().await;
    let authorized = match state
        .store
        .list_gateway_shares(&gateway, "main", &active_subdomains, &inflight_by_share)
        .await
    {
        Ok(shares) => shares.into_iter().any(|share| share.share_id == share_id),
        Err(err) => {
            warn!(error = %err, "gateway proxy share authorization lookup failed");
            return simple_response(StatusCode::SERVICE_UNAVAILABLE, "share-lookup-failed");
        }
    };
    if !authorized {
        return simple_response(StatusCode::FORBIDDEN, "share-not-authorized-for-gateway");
    }

    let Some((route, route_inflight_guard)) = state.proxy.route_for_share_request(&share_id).await
    else {
        return simple_response(StatusCode::NOT_FOUND, "share-offline");
    };
    let backend = route.backend.clone();
    let target = format!("http://{backend}{path_and_query}");

    let metrics_permit = state.metrics.proxy_request_started();
    let request_app = infer_share_request_app(&path_and_query, &parts.headers);
    let share_permit = match state
        .proxy
        .try_acquire_share_permit(
            &share_id,
            request_app.as_deref(),
            route.parallel_limit,
            None,
        )
        .await
    {
        Some(permit) => permit,
        None => {
            warn!(
                method = %method,
                host = %host,
                path = %path_and_query,
                share_id = %share_id,
                parallel_limit = route.parallel_limit,
                client_ip = %user_ip,
                client_country = %user_country,
                client_asn = %user_asn,
                user_agent = %user_agent,
                "gateway proxy rejected: share concurrency limit exceeded"
            );
            return simple_response(
                StatusCode::TOO_MANY_REQUESTS,
                "share-concurrency-limit-exceeded",
            );
        }
    };
    let free_share_ip_permit = if route.is_free_share && state.config.free_share_ip_limit_enabled()
    {
        match state
            .proxy
            .try_acquire_free_share_ip_permit(&user_ip, state.config.free_share_ip_parallel_limit)
            .await
        {
            Some(permit) => Some(permit),
            None => {
                return simple_response(
                    StatusCode::TOO_MANY_REQUESTS,
                    "free-share-ip-concurrency-limit-exceeded",
                );
            }
        }
    } else {
        None
    };

    let mut builder = state.proxy_http.request(method.clone(), target);
    for (name, value) in &parts.headers {
        let n = name.as_str();
        if n.eq_ignore_ascii_case("host")
            || n.eq_ignore_ascii_case("authorization")
            || n.eq_ignore_ascii_case("x-api-key")
            || n.eq_ignore_ascii_case("x-goog-api-key")
            || n.eq_ignore_ascii_case("api-key")
            || n.starts_with("x-cc-gateway-")
            || is_internal_share_context_header(n)
            || is_hop_by_hop_header(n)
        {
            continue;
        }
        builder = builder.header(name, value);
    }
    builder = builder.header("X-CC-Switch-Share-Id", share_id.as_str());
    builder = builder.header("X-CC-Switch-Share-Subdomain", route.subdomain.as_str());
    builder = builder.header(SHARE_DATA_SOURCE_HEADER, "gateway");
    builder = with_share_user_country_headers(builder, client_metadata.country_code.as_deref());

    let live_request_id = state
        .recent_traffic
        .record(
            share_id.clone(),
            route.share_name.clone(),
            Some(route.subdomain.clone()),
            client_metadata.country_code.clone(),
            None,
        )
        .await;
    builder = builder.header("X-CC-Switch-Request-Id", live_request_id.as_str());
    builder = match with_signed_ingress_context(
        &state,
        builder,
        &route,
        format!("{}.{}", route.subdomain, state.config.tunnel_domain),
        live_request_id.clone(),
        None,
        client_metadata.country_code.clone(),
    )
    .await
    {
        Ok(builder) => builder,
        Err(response) => return response,
    };
    let recent_traffic_guard = RecentTrafficGuard {
        traffic: state.recent_traffic.clone(),
        request_id: live_request_id,
    };

    let upstream = match builder.body(reqwest::Body::from(body_bytes)).send().await {
        Ok(response) => response,
        Err(err) => {
            state.metrics.record_proxy_upstream_error(false);
            warn!(
                method = %method,
                host = %host,
                path = %path_and_query,
                backend = %backend,
                share_id = %share_id,
                client_ip = %user_ip,
                client_country = %user_country,
                client_asn = %user_asn,
                user_agent = %user_agent,
                error = %err,
                "gateway proxy upstream request failed"
            );
            return simple_response(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!("connection-lost: {err}"),
            );
        }
    };

    let status = upstream.status();
    state.metrics.record_proxy_status(status);
    let response_headers = upstream.headers().clone();
    let body_stream = {
        use futures_util::StreamExt;

        upstream.bytes_stream().map(move |chunk| {
            let _route_inflight_guard = &route_inflight_guard;
            let _permit = &share_permit;
            let _free_share_ip_permit = &free_share_ip_permit;
            let _recent_traffic_guard = &recent_traffic_guard;
            let _metrics_permit = &metrics_permit;
            chunk
        })
    };
    let body = Body::from_stream(body_stream);

    let mut response = Response::new(body);
    *response.status_mut() = status;
    response.headers_mut().clear();
    for (name, value) in &response_headers {
        if is_hop_by_hop_header(name.as_str()) {
            continue;
        }
        response.headers_mut().append(name.clone(), value.clone());
    }
    strip_connection_listed_headers(response.headers_mut());
    info!(
        method = %method,
        host = %host,
        path = %path_and_query,
        gateway_id = %gateway.id,
        share_id = %share_id,
        backend = %backend,
        status = %status.as_u16(),
        client_ip = %user_ip,
        client_country = %user_country,
        client_asn = %user_asn,
        user_agent = %user_agent,
        "gateway proxy request completed"
    );
    response
}

async fn authenticate_gateway_proxy(
    state: &ServerState,
    headers: &HeaderMap,
    body_sha256_hex: &str,
) -> Result<crate::models::GatewayRegistryRecord, crate::error::AppError> {
    let gateway_id = gateway_header(headers, "x-cc-gateway-id")?;
    let timestamp_ms = gateway_header(headers, "x-cc-gateway-timestamp-ms")?
        .parse::<i64>()
        .map_err(|_| crate::error::AppError::Unauthorized("invalid gateway timestamp".into()))?;
    let nonce = gateway_header(headers, "x-cc-gateway-nonce")?;
    let signature = gateway_header(headers, "x-cc-gateway-signature")?;
    state
        .store
        .authenticate_gateway_signed_request(
            gateway_id,
            "gateway:proxy:use",
            "gateway:proxy",
            body_sha256_hex,
            timestamp_ms,
            nonce,
            signature,
        )
        .await
}

fn gateway_header<'a>(
    headers: &'a HeaderMap,
    name: &str,
) -> Result<&'a str, crate::error::AppError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| crate::error::AppError::Unauthorized(format!("missing {name} header")))
}

pub async fn proxy_handler(
    State(state): State<ServerState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    let (parts, mut body) = req.into_parts();
    let method = parts.method.clone();
    let host = parts
        .headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    let path = parts.uri.path().to_string();
    // The `/_ctl/*` namespace is reserved for the server→client control-plane
    // RPC, which the server reaches by connecting directly to the tunnel
    // backend (bypassing this handler). Inbound public traffic must never be
    // proxied into it, otherwise an external caller could try to drive the
    // client's control API. Reject before any routing happens.
    if path.starts_with("/_ctl/") || path == "/_ctl" {
        return simple_response(StatusCode::NOT_FOUND, "not-found");
    }
    let client_metadata = crate::client_meta::extract_client_metadata(&parts.headers, peer);
    let user_ip = client_metadata
        .ip
        .clone()
        .unwrap_or_else(|| peer.ip().to_string());
    let user_country = client_metadata.country_code.as_deref().unwrap_or("-");
    let user_asn = trusted_asn_header(&parts.headers, peer);
    let user_agent = header_str(&parts.headers, "user-agent");
    if let Some(remaining) = state.abuse.ban_remaining(&user_ip).await {
        warn!(
            method = %method,
            host = %host,
            path = %path_and_query,
            client_ip = %user_ip,
            client_country = %user_country,
            client_asn = %user_asn,
            user_agent = %user_agent,
            ban_remaining_secs = remaining.as_secs(),
            "proxy request rejected: client temporarily banned"
        );
        return simple_response(StatusCode::FORBIDDEN, "client-banned");
    }
    let is_internal_share_router_path = is_internal_share_router_path(&path);
    let is_share_router_probe = parts
        .headers
        .get("x-share-router-probe")
        .and_then(|value| value.to_str().ok())
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
        && path == "/_share-router/health";
    if !host_matches_tunnel_domain(&host, &state.config.tunnel_domain) {
        tracing::debug!(
            method = %method,
            host = %host,
            path = %path_and_query,
            tunnel_domain = %state.config.tunnel_domain,
            client_ip = %user_ip,
            client_country = %user_country,
            client_asn = %user_asn,
            user_agent = %user_agent,
            "proxy request ignored: host outside tunnel domain"
        );
        return simple_response(StatusCode::NOT_FOUND, "not-found");
    }

    let route_subdomain = subdomain_for_host(&host, &state.config.tunnel_domain);
    let (route, route_inflight_guard) = match state
        .proxy
        .route_for_host_request(&host, &state.config.tunnel_domain)
        .await
    {
        RouteLookup::Active(route, guard) => (route, guard),
        RouteLookup::Reconnecting => {
            if is_share_router_probe {
                if let Some(subdomain) = route_subdomain.as_deref() {
                    if state.proxy.has_pending_route(subdomain).await {
                        debug!(
                            method = %method,
                            host = %host,
                            path = %path_and_query,
                            client_ip = %user_ip,
                            client_country = %user_country,
                            client_asn = %user_asn,
                            user_agent = %user_agent,
                            "proxy health probe accepted while route registration is pending"
                        );
                        return empty_response(StatusCode::NO_CONTENT);
                    }
                }
            }
            warn!(
                method = %method,
                host = %host,
                path = %path_and_query,
                client_ip = %user_ip,
                client_country = %user_country,
                client_asn = %user_asn,
                user_agent = %user_agent,
                "proxy request deferred: registered tunnel is reconnecting"
            );
            return reconnecting_response();
        }
        RouteLookup::Unknown => {
            warn!(
                method = %method,
                host = %host,
                path = %path_and_query,
                client_ip = %user_ip,
                client_country = %user_country,
                client_asn = %user_asn,
                user_agent = %user_agent,
                "proxy request rejected: unregistered subdomain"
            );
            return simple_response(StatusCode::NOT_FOUND, "unregistered-subdomain");
        }
    };
    if is_internal_share_router_path
        && method == axum::http::Method::GET
        && !authorize_internal_share_router_get(&state, &route, &parts.headers, &path_and_query)
            .await
    {
        return simple_response(StatusCode::NOT_FOUND, "not-found");
    }
    let backend = route.backend.clone();
    let is_health_check_request = is_share_router_probe;
    let is_direct_share_web_request = route.is_share() && is_allowed_direct_share_web_path(&path);
    let skips_share_edge_auth =
        share_route_skips_edge_auth(is_internal_share_router_path, is_direct_share_web_request);
    if route.is_client_web() && is_share_router_probe {
        debug!(
            method = %method,
            host = %host,
            path = %path_and_query,
            backend = %backend,
            status = %StatusCode::NO_CONTENT.as_u16(),
            client_ip = %user_ip,
            client_country = %user_country,
            client_asn = %user_asn,
            user_agent = %user_agent,
            "proxy client web health probe completed"
        );
        return empty_response(StatusCode::NO_CONTENT);
    }
    if route.is_share() && !is_allowed_direct_share_proxy_path(&path) {
        debug!(
            method = %method,
            host = %host,
            path = %path_and_query,
            client_ip = %user_ip,
            client_country = %user_country,
            client_asn = %user_asn,
            user_agent = %user_agent,
            "proxy request ignored: non-api direct share path"
        );
        return simple_response(StatusCode::NOT_FOUND, "non-api-path");
    }
    let mut client_web_session: Option<(String, bool)> = None;
    if route.is_client_web() {
        if !is_allowed_client_web_path(&path) {
            debug!(
                method = %method,
                host = %host,
                path = %path_and_query,
                client_ip = %user_ip,
                client_country = %user_country,
                client_asn = %user_asn,
                user_agent = %user_agent,
                "proxy request ignored: disallowed client web path"
            );
            return simple_response(StatusCode::NOT_FOUND, "non-api-path");
        }
        if has_client_web_query_token(parts.uri.query()) {
            warn!(
                method = %method,
                host = %host,
                path = %path,
                client_ip = %user_ip,
                client_country = %user_country,
                client_asn = %user_asn,
                user_agent = %user_agent,
                "proxy request rejected: client web token in query string"
            );
            return simple_response(StatusCode::BAD_REQUEST, "query-token-not-allowed");
        }
        if is_client_web_auth_required_path(&path) {
            let owner_email = match state
                .store
                .resolve_client_tunnel_owner_email(
                    &route.subdomain,
                    route.installation_id().as_deref(),
                )
                .await
            {
                Ok(owner_email) => owner_email,
                Err(err) => {
                    warn!(
                        method = %method,
                        host = %host,
                        path = %path_and_query,
                        subdomain = %route.subdomain,
                        error = %err,
                        "proxy request rejected: client tunnel owner lookup failed"
                    );
                    return simple_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "client-tunnel-lookup-failed",
                    );
                }
            };
            if let Some(owner_email) = owner_email {
                let required_scope = client_web_required_api_token_scope(&path);
                let session = match resolve_client_web_bearer(
                    &state,
                    &parts.headers,
                    &owner_email,
                    required_scope,
                )
                .await
                {
                    Ok(Some(session)) => session,
                    Ok(None) if client_web_bearer_token(&parts.headers).is_some() => {
                        client_web_session = None;
                        ("".to_string(), false)
                    }
                    Ok(None) => {
                        return simple_response(StatusCode::UNAUTHORIZED, "login-required");
                    }
                    Err(err) => {
                        warn!(
                            method = %method,
                            host = %host,
                            path = %path_and_query,
                            client_ip = %user_ip,
                            client_country = %user_country,
                            client_asn = %user_asn,
                            user_agent = %user_agent,
                            error = %err,
                            "proxy request rejected: client web auth lookup failed"
                        );
                        return simple_response(StatusCode::UNAUTHORIZED, "login-required");
                    }
                };
                if !session.0.is_empty() && session.0 != owner_email && !session.1 {
                    return simple_response(StatusCode::FORBIDDEN, "client-web-forbidden");
                }
                if !session.0.is_empty() {
                    client_web_session = Some(session);
                }
            } else if client_web_bearer_token(&parts.headers).is_some() {
                debug!(
                    method = %method,
                    host = %host,
                    path = %path_and_query,
                    subdomain = %route.subdomain,
                    installation_id = route.installation_id().unwrap_or("-"),
                    "proxy client web auth passthrough: tunnel owner metadata missing, forwarding bearer to cc-switch-server"
                );
                client_web_session = None;
            } else {
                return simple_response(StatusCode::UNAUTHORIZED, "login-required");
            }
        }
    }
    if is_health_check_request
        && state
            .proxy
            .has_cached_health_probe_failure(&route.subdomain)
            .await
    {
        state.metrics.record_health_probe_cached_failure();
        debug!(
            method = %method,
            host = %host,
            path = %path_and_query,
            backend = %backend,
            client_ip = %user_ip,
            client_country = %user_country,
            client_asn = %user_asn,
            user_agent = %user_agent,
            "proxy health check short-circuited by recent upstream failure"
        );
        return simple_response(StatusCode::SERVICE_UNAVAILABLE, "connection-lost-cached");
    }

    // User-facing credentials terminate at the router. The client only sees
    // the internal share secret registered with this tunnel route.
    let mut api_user_email = None;
    if !skips_share_edge_auth {
        if let Some(share_id) = route.share_id.as_deref() {
            let Some(user_token) = crate::api::extract_router_api_token(&parts.headers) else {
                return simple_response(StatusCode::UNAUTHORIZED, "missing-router-api-token");
            };
            let principal = match state
                .store
                .resolve_user_api_token(user_token, "share:invoke")
                .await
            {
                Ok(Some(principal)) => principal,
                Ok(None) => {
                    return simple_response(StatusCode::UNAUTHORIZED, "invalid-router-api-token");
                }
                Err(err) => {
                    warn!(
                        method = %method,
                        host = %host,
                        path = %path_and_query,
                        share_id = %share_id,
                        client_ip = %user_ip,
                        client_country = %user_country,
                        client_asn = %user_asn,
                        user_agent = %user_agent,
                        error = %err,
                        "proxy request rejected: router api token authentication failed"
                    );
                    return simple_response(StatusCode::UNAUTHORIZED, "invalid-router-api-token");
                }
            };
            match state
                .store
                .user_can_invoke_share(
                    &principal.email,
                    share_id,
                    infer_share_request_app(&path, &parts.headers).as_deref(),
                )
                .await
            {
                Ok(true) => {
                    api_user_email = Some(principal.email.clone());
                }
                Ok(false) => {
                    return simple_response(StatusCode::FORBIDDEN, "share-not-authorized-for-user");
                }
                Err(err) => {
                    warn!(
                        method = %method,
                        host = %host,
                        path = %path_and_query,
                        share_id = %share_id,
                        user_email = %principal.email,
                        error = %err,
                        "proxy request rejected: share acl lookup failed"
                    );
                    return simple_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "share-acl-lookup-failed",
                    );
                }
            }
        }
    }
    if route.is_share()
        && method == axum::http::Method::POST
        && is_image_generation_submit_path(&path)
    {
        let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
            Ok(body) => body,
            Err(err) => {
                warn!(
                    method = %method,
                    host = %host,
                    path = %path_and_query,
                    backend = %backend,
                    client_ip = %user_ip,
                    client_country = %user_country,
                    client_asn = %user_asn,
                    user_agent = %user_agent,
                    error = %err,
                    "image generation request body read failed"
                );
                return json_error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("failed-to-read-body: {err}"),
                );
            }
        };
        if image_generation_request_wants_stream(&body_bytes) {
            return handle_image_generation_stream_submit(
                &state,
                &route,
                route_inflight_guard,
                body_bytes,
                api_user_email,
                user_ip,
                user_country.to_string(),
            )
            .await;
        }
        body = Body::from(body_bytes);
    }
    let target = format!("http://{backend}{path_and_query}");

    let metrics_permit = state.metrics.proxy_request_started();
    let mut builder = state.proxy_http.request(method.clone(), target);
    for (name, value) in &parts.headers {
        let n = name.as_str();
        if n.eq_ignore_ascii_case("host") || is_hop_by_hop_header(n) {
            continue;
        }
        // Strip client-supplied user/share credentials on share routes; router
        // authenticates the caller at the edge (user_api_token + email ACL)
        // and the cc-switch tunnel only needs the share id we inject below.
        if is_internal_share_context_header(n) {
            continue;
        }
        if route.is_share()
            && (n.eq_ignore_ascii_case("authorization")
                || n.eq_ignore_ascii_case("x-api-key")
                || n.eq_ignore_ascii_case("x-goog-api-key")
                || n.eq_ignore_ascii_case("api-key"))
        {
            continue;
        }
        if route.is_client_web()
            && (n.eq_ignore_ascii_case(CLIENT_WEB_USER_EMAIL_HEADER)
                || n.eq_ignore_ascii_case(CLIENT_WEB_ROLE_HEADER)
                || n.eq_ignore_ascii_case(CLIENT_WEB_INSTALLATION_ID_HEADER)
                || n.eq_ignore_ascii_case(CLIENT_WEB_SUBDOMAIN_HEADER)
                || (client_web_session.is_some() && n.eq_ignore_ascii_case("authorization"))
                || n.eq_ignore_ascii_case("cookie"))
        {
            continue;
        }
        builder = builder.header(name, value);
    }

    // Inject share id so cc-switch can identify the share on its tunnel side
    // and attribute usage. There is no longer a separate share_token credential —
    // tunnel transport itself is the only authority that we are speaking on
    // behalf of this share.
    if let Some(ref share_id) = route.share_id {
        builder = builder.header("X-CC-Switch-Share-Id", share_id.as_str());
    }
    builder = builder.header("X-CC-Switch-Share-Subdomain", route.subdomain.as_str());
    if route.is_share() {
        builder = builder.header(SHARE_DATA_SOURCE_HEADER, "direct");
    }
    if let Some(ref email) = api_user_email {
        builder = builder.header("X-CC-Switch-User-Email", email.as_str());
    }
    if route.is_share() {
        builder = with_share_user_country_headers(builder, client_metadata.country_code.as_deref());
    }
    if let Some((email, is_admin)) = client_web_session.as_ref() {
        builder = builder
            .header(CLIENT_WEB_USER_EMAIL_HEADER, email.as_str())
            .header(
                CLIENT_WEB_ROLE_HEADER,
                if *is_admin { "admin" } else { "owner" },
            )
            .header(
                CLIENT_WEB_INSTALLATION_ID_HEADER,
                route.installation_id().unwrap_or_default(),
            )
            .header(CLIENT_WEB_SUBDOMAIN_HEADER, route.subdomain.as_str());
    }

    let log_share_id = route
        .share_id
        .as_deref()
        .map(mask_token)
        .unwrap_or_else(|| "-".to_string());

    let share_permit = if skips_share_edge_auth {
        None
    } else if let Some(share_id) = route.share_id.as_deref() {
        let request_app = infer_share_request_app(&path, &parts.headers);
        match state
            .proxy
            .try_acquire_share_permit(
                share_id,
                request_app.as_deref(),
                route.parallel_limit,
                api_user_email.as_deref(),
            )
            .await
        {
            Some(permit) => Some(permit),
            None => {
                warn!(
                    method = %method,
                    host = %host,
                    path = %path_and_query,
                    share_id = %share_id,
                    parallel_limit = route.parallel_limit,
                    client_ip = %user_ip,
                    client_country = %user_country,
                    client_asn = %user_asn,
                    user_agent = %user_agent,
                    "proxy request rejected: share concurrency limit exceeded"
                );
                return simple_response(
                    StatusCode::TOO_MANY_REQUESTS,
                    "share-concurrency-limit-exceeded",
                );
            }
        }
    } else {
        None
    };

    let body = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(body) => body,
        Err(err) => {
            warn!(
                method = %method,
                host = %host,
                path = %path_and_query,
                backend = %backend,
                share_id = %log_share_id,
                client_ip = %user_ip,
                client_country = %user_country,
                client_asn = %user_asn,
                user_agent = %user_agent,
                error = %err,
                "proxy request body read failed"
            );
            return simple_response(
                StatusCode::BAD_REQUEST,
                &format!("failed-to-read-body: {err}"),
            );
        }
    };

    let free_share_ip_permit = if !skips_share_edge_auth
        && route.is_free_share
        && state.config.free_share_ip_limit_enabled()
    {
        match state
            .proxy
            .try_acquire_free_share_ip_permit(&user_ip, state.config.free_share_ip_parallel_limit)
            .await
        {
            Some(permit) => Some(permit),
            None => {
                warn!(
                    method = %method,
                    host = %host,
                    path = %path_and_query,
                    user_ip = %user_ip,
                    parallel_limit = state.config.free_share_ip_parallel_limit,
                    client_country = %user_country,
                    client_asn = %user_asn,
                    user_agent = %user_agent,
                    "proxy request rejected: free share ip concurrency limit exceeded"
                );
                return simple_response(
                    StatusCode::TOO_MANY_REQUESTS,
                    "free-share-ip-concurrency-limit-exceeded",
                );
            }
        }
    } else {
        None
    };

    // Record the request for the dashboard's demand/ticker stream and propagate the
    // generated identity downstream so share clients can write the same request id back
    // in their request logs.
    let live_request_id = if !skips_share_edge_auth && !is_share_router_probe {
        if let Some(share_id) = route.share_id.as_deref() {
            Some(
                state
                    .recent_traffic
                    .record(
                        share_id.to_string(),
                        route.share_name.clone(),
                        Some(route.subdomain.clone()),
                        client_metadata.country_code.clone(),
                        api_user_email.clone(),
                    )
                    .await,
            )
        } else {
            None
        }
    } else {
        None
    };
    if let Some(ref request_id) = live_request_id {
        builder = builder.header("X-CC-Switch-Request-Id", request_id.as_str());
    }
    if route.is_share() || route.is_client_web() {
        let installation_id = route.installation_id().unwrap_or_default();
        let control_secret = match state
            .store
            .installation_control_secret(installation_id)
            .await
        {
            Ok(Some(secret)) if !secret.trim().is_empty() => secret,
            Ok(_) => {
                return simple_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "ingress-control-secret-missing",
                );
            }
            Err(error) => {
                warn!(
                    installation_id,
                    error = %error,
                    "proxy ingress context secret lookup failed"
                );
                return simple_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "ingress-control-secret-lookup-failed",
                );
            }
        };
        let request_id = live_request_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let route_id = route
            .share_id()
            .map(|share_id| format!("share:{share_id}"))
            .unwrap_or_else(|| format!("client:{installation_id}"));
        let signed = match crate::ingress_context::sign(
            crate::ingress_context::IngressContext {
                protocol_epoch: crate::namespace::PROTOCOL_EPOCH.to_string(),
                router_id: state
                    .config
                    .tunnel_domain
                    .trim_end_matches('.')
                    .to_ascii_lowercase(),
                route_id,
                installation_id: installation_id.to_string(),
                target_lane_id: installation_id.to_string(),
                public_host: host
                    .split(':')
                    .next()
                    .unwrap_or_default()
                    .trim_end_matches('.')
                    .to_ascii_lowercase(),
                share_id: route.share_id.clone(),
                request_id,
                user_email: api_user_email
                    .clone()
                    .or_else(|| client_web_session.as_ref().map(|(email, _)| email.clone())),
                user_role: client_web_session
                    .as_ref()
                    .map(|(_, is_admin)| if *is_admin { "admin" } else { "owner" }.to_string()),
                user_country: client_metadata.country_code.clone(),
                issued_at_ms: chrono::Utc::now().timestamp_millis(),
            },
            &control_secret,
        ) {
            Ok(signed) => signed,
            Err(error) => {
                warn!(error, "proxy ingress context signing failed");
                return simple_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "ingress-context-signing-failed",
                );
            }
        };
        builder = builder
            .header(
                crate::ingress_context::INGRESS_CONTEXT_HEADER,
                signed.encoded_context,
            )
            .header(
                crate::ingress_context::INGRESS_SIGNATURE_HEADER,
                signed.signature,
            );
    }
    // Bind a completion guard to the recorded request id. While this binding
    // lives at function scope it covers the early-return-on-upstream-error
    // path; once the body stream is constructed we move it into the streaming
    // closure so completion fires when the upstream stream actually ends.
    let recent_traffic_guard = live_request_id.as_ref().map(|id| RecentTrafficGuard {
        traffic: state.recent_traffic.clone(),
        request_id: id.clone(),
    });
    let share_proxy_started = Instant::now();
    let share_request_app = infer_share_request_app(&path, &parts.headers);

    let upstream = match builder.body(body).send().await {
        Ok(response) => response,
        Err(err) => {
            if is_share_router_probe && state.proxy.has_pending_route(&route.subdomain).await {
                debug!(
                    method = %method,
                    host = %host,
                    path = %path_and_query,
                    backend = %backend,
                    share_id = %log_share_id,
                    client_ip = %user_ip,
                    client_country = %user_country,
                    client_asn = %user_asn,
                    user_agent = %user_agent,
                    error = %err,
                    "proxy health probe accepted while replacement route registration is pending"
                );
                return empty_response(StatusCode::NO_CONTENT);
            }
            if is_health_check_request {
                state
                    .proxy
                    .record_health_probe_failure(route.subdomain.clone())
                    .await;
            }
            state
                .metrics
                .record_proxy_upstream_error(is_health_check_request);
            warn!(
                method = %method,
                host = %host,
                path = %path_and_query,
                backend = %backend,
                share_id = %log_share_id,
                client_ip = %user_ip,
                client_country = %user_country,
                client_asn = %user_asn,
                user_agent = %user_agent,
                error = %err,
                "proxy upstream request failed"
            );
            let _share_llm_metrics_guard = share_llm_proxy_metrics_guard(
                &state,
                &route,
                &path,
                is_share_router_probe,
                skips_share_edge_auth,
                live_request_id.as_deref(),
                StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                share_proxy_started,
                share_request_app.clone(),
            );
            return simple_response(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!("connection-lost: {err}"),
            );
        }
    };

    let status = upstream.status();
    state.metrics.record_proxy_status(status);
    let share_llm_metrics_guard = share_llm_proxy_metrics_guard(
        &state,
        &route,
        &path,
        is_share_router_probe,
        skips_share_edge_auth,
        live_request_id.as_deref(),
        status.as_u16(),
        share_proxy_started,
        share_request_app,
    );
    if is_health_check_request {
        state
            .proxy
            .clear_health_probe_failure(&route.subdomain)
            .await;
    }
    let response_headers = upstream.headers().clone();
    let is_event_stream = is_event_stream_response(&response_headers);
    if is_invalid_auth_status(status) && is_abuse_tracked_api_path(&path) {
        if let Some(decision) = state.abuse.record_invalid_auth(&user_ip).await {
            warn!(
                method = %method,
                host = %host,
                path = %path_and_query,
                backend = %backend,
                status = %status.as_u16(),
                share_id = %log_share_id,
                client_ip = %user_ip,
                client_country = %user_country,
                client_asn = %user_asn,
                user_agent = %user_agent,
                failures_10m = decision.failures,
                ban_secs = decision.ban_duration.as_secs(),
                "proxy client temporarily banned: invalid auth threshold reached"
            );
        }
    }

    // Stream the response body instead of buffering it entirely.
    // This is critical for SSE (text/event-stream) responses so that
    // downstream clients receive chunks in real time.
    let body_stream = upstream
        .bytes_stream()
        .scan(false, move |stream_ended, chunk| {
            let _route_inflight_guard = &route_inflight_guard;
            let _permit = &share_permit;
            let _free_share_ip_permit = &free_share_ip_permit;
            // Hold the recent-traffic guard until the upstream stream ends so
            // the dashboard ticker keeps the row marked in-flight for the full
            // request lifecycle (success, client disconnect, or chunk error).
            let _recent_traffic_guard = &recent_traffic_guard;
            let _share_llm_metrics_guard = &share_llm_metrics_guard;
            let _metrics_permit = &metrics_permit;
            let output = proxy_body_chunk(is_event_stream, stream_ended, chunk);
            futures_util::future::ready(output)
        });
    let body = Body::from_stream(body_stream);

    let mut response = Response::new(body);
    *response.status_mut() = status;
    response.headers_mut().clear();
    for (name, value) in &response_headers {
        if is_hop_by_hop_header(name.as_str()) {
            continue;
        }
        response.headers_mut().append(name.clone(), value.clone());
    }
    strip_connection_listed_headers(response.headers_mut());
    if is_event_stream {
        response.headers_mut().remove(header::CONTENT_LENGTH);
    }
    if is_share_router_probe {
        debug!(
            method = %method,
            host = %host,
            path = %path_and_query,
            backend = %backend,
            status = %status.as_u16(),
            share_id = %log_share_id,
            client_ip = %user_ip,
            client_country = %user_country,
            client_asn = %user_asn,
            user_agent = %user_agent,
            "proxy health probe completed"
        );
    } else {
        info!(
            method = %method,
            host = %host,
            path = %path_and_query,
            backend = %backend,
            status = %status.as_u16(),
            share_id = %log_share_id,
            client_ip = %user_ip,
            client_country = %user_country,
            client_asn = %user_asn,
            user_agent = %user_agent,
            "proxy request completed"
        );
    }
    response
}

fn is_image_generation_submit_path(path: &str) -> bool {
    matches!(
        path.trim_start_matches('/'),
        "v1/images/generations" | "images/generations"
    )
}

fn image_generation_request_wants_stream(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value.get("stream").and_then(Value::as_bool))
        .unwrap_or(false)
}

async fn handle_image_generation_stream_submit(
    state: &ServerState,
    route: &RouteEntry,
    route_inflight_guard: RouteInflightGuard,
    body: axum::body::Bytes,
    api_user_email: Option<String>,
    user_ip: String,
    user_country: String,
) -> Response {
    let Some(share_id) = route.share_id.as_deref() else {
        return json_error_response(StatusCode::NOT_FOUND, "share-not-found");
    };
    let share = match state.store.get_share_for_test(share_id).await {
        Ok(Some(share)) => share,
        Ok(None) => return json_error_response(StatusCode::NOT_FOUND, "share-not-found"),
        Err(err) => {
            warn!(share_id = %share_id, error = %err, "image generation share lookup failed");
            return json_error_response(StatusCode::SERVICE_UNAVAILABLE, "share-lookup-failed");
        }
    };
    let Some((provider_id, provider_name)) = codex_image_generation_provider(&share) else {
        return json_error_response(
            StatusCode::BAD_REQUEST,
            "codex image generation is not enabled for the bound provider",
        );
    };
    let Some(image_permit) = state
        .proxy
        .try_acquire_image_permit(share_id, IMAGE_JOB_MAX_RUNNING_PER_SHARE as i64)
        .await
    else {
        return json_error_response(
            StatusCode::TOO_MANY_REQUESTS,
            "image-generation-stream-busy",
        );
    };

    let mut payload = match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(map)) => map,
        Ok(_) => {
            return json_error_response(
                StatusCode::BAD_REQUEST,
                "image request body must be a JSON object",
            );
        }
        Err(err) => {
            return json_error_response(
                StatusCode::BAD_REQUEST,
                &format!("invalid image request json: {err}"),
            );
        }
    };
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("gpt-5.5")
        .to_string();
    let prompt_preview = payload
        .get("prompt")
        .and_then(Value::as_str)
        .map(|value| compact_prompt_preview(value, 180));
    let output_format = payload
        .get("output_format")
        .or_else(|| payload.get("format"))
        .and_then(Value::as_str)
        .map(normalize_image_output_format)
        .unwrap_or_else(|| "png".to_string());
    payload.insert("stream".into(), Value::Bool(true));
    payload.insert("response_format".into(), Value::String("b64_json".into()));
    let upstream_body = match serde_json::to_vec(&Value::Object(payload)) {
        Ok(body) => body,
        Err(err) => {
            return json_error_response(
                StatusCode::BAD_REQUEST,
                &format!("serialize image request failed: {err}"),
            );
        }
    };

    let log_meta = ImageStreamLogMeta {
        request_id: format!("imgreq_{}", Uuid::new_v4().simple()),
        share_id: share_id.to_string(),
        installation_id: route.installation_id().unwrap_or_default().to_string(),
        share_name: route
            .share_name
            .clone()
            .unwrap_or_else(|| share_id.to_string()),
        provider_id,
        provider_name,
        app_type: "codex".into(),
        model,
        created_at: chrono::Utc::now().timestamp(),
        prompt_preview,
        created_by_email: api_user_email.clone(),
        client_ip: Some(user_ip.clone()),
        user_country: Some(user_country.clone()),
    };
    if let Err(err) = record_image_stream_log(
        &state.store,
        &state.config,
        &log_meta,
        ImageStreamLogOutcome {
            status: "running",
            status_code: None,
            latency_ms: 0,
            completed_at: None,
            error_message: None,
            result_mime_type: None,
            result_size_bytes: None,
            result_storage_key: None,
            result_access_token: None,
        },
    )
    .await
    {
        warn!(request_id = %log_meta.request_id, error = %err, "record image stream start log failed");
    }

    let target = format!("http://{}/v1/images/generations", route.backend);
    let mut builder = state
        .proxy_http
        .post(target)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "text/event-stream")
        .header("X-CC-Switch-Share-Subdomain", route.subdomain.as_str())
        .header(SHARE_DATA_SOURCE_HEADER, "direct");
    if let Some(share_id) = route.share_id.as_deref() {
        builder = builder.header("X-CC-Switch-Share-Id", share_id);
    }
    if let Some(email) = api_user_email.as_deref() {
        builder = builder.header("X-CC-Switch-User-Email", email);
    }
    builder = with_share_user_country_headers(builder, Some(user_country.as_str()));

    let metrics_permit = state.metrics.proxy_request_started();
    let request_id = state
        .recent_traffic
        .record(
            share_id.to_string(),
            route.share_name.clone(),
            Some(route.subdomain.clone()),
            Some(user_country.clone()),
            api_user_email.clone(),
        )
        .await;
    builder = match with_signed_ingress_context(
        state,
        builder,
        route,
        format!("{}.{}", route.subdomain, state.config.tunnel_domain),
        request_id.clone(),
        api_user_email.clone(),
        Some(user_country.clone()),
    )
    .await
    {
        Ok(builder) => builder,
        Err(response) => return response,
    };
    let recent_traffic_guard = Some(RecentTrafficGuard {
        traffic: state.recent_traffic.clone(),
        request_id,
    });

    let request_started = Instant::now();
    let upstream = match builder.body(upstream_body).send().await {
        Ok(response) => response,
        Err(err) => {
            state.metrics.record_proxy_upstream_error(false);
            warn!(
                share_id = %share_id,
                client_ip = %user_ip,
                client_country = %user_country,
                error = %err,
                "image generation stream upstream request failed"
            );
            if let Err(log_err) = record_image_stream_log(
                &state.store,
                &state.config,
                &log_meta,
                ImageStreamLogOutcome {
                    status: "failed",
                    status_code: None,
                    latency_ms: request_started.elapsed().as_millis() as u64,
                    completed_at: Some(chrono::Utc::now().timestamp()),
                    error_message: Some(format!("connection-lost: {err}")),
                    result_mime_type: None,
                    result_size_bytes: None,
                    result_storage_key: None,
                    result_access_token: None,
                },
            )
            .await
            {
                warn!(request_id = %log_meta.request_id, error = %log_err, "record image stream connection failure log failed");
            }
            return json_error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!("connection-lost: {err}"),
            );
        }
    };
    let status = upstream.status();
    state.metrics.record_proxy_status(status);
    if !status.is_success() {
        let status_code = status;
        let text = upstream
            .text()
            .await
            .unwrap_or_else(|err| format!("failed to read upstream error: {err}"));
        if let Err(err) = record_image_stream_log(
            &state.store,
            &state.config,
            &log_meta,
            ImageStreamLogOutcome {
                status: "failed",
                status_code: Some(status_code.as_u16()),
                latency_ms: request_started.elapsed().as_millis() as u64,
                completed_at: Some(chrono::Utc::now().timestamp()),
                error_message: Some(compact_prompt_preview(&text, 1000)),
                result_mime_type: None,
                result_size_bytes: None,
                result_storage_key: None,
                result_access_token: None,
            },
        )
        .await
        {
            warn!(request_id = %log_meta.request_id, error = %err, "record image stream upstream failure log failed");
        }
        return json_error_response(status_code, &compact_prompt_preview(&text, 1000));
    }

    let mut upstream_stream = upstream.bytes_stream();
    let log_store = state.store.clone();
    let result_config = state.config.clone();
    let stream = async_stream::stream! {
        use futures_util::StreamExt;

        let _route_inflight_guard = route_inflight_guard;
        let _image_permit = image_permit;
        let _metrics_permit = metrics_permit;
        let _recent_traffic_guard = recent_traffic_guard;
        let mut parser = ImageStreamSseParser::default();
        let mut terminal_logged = false;
        let completion_guard = ImageStreamCompletionGuard::new(
            log_store.clone(),
            result_config.clone(),
            log_meta.clone(),
            request_started,
            status.as_u16(),
        );
        let mut keepalive = tokio::time::interval(Duration::from_secs(15));
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                chunk = upstream_stream.next() => {
                    match chunk {
                        Some(Ok(bytes)) => {
                            if !terminal_logged {
                                if let Some(event) = parser.feed(&bytes, &output_format) {
                                    terminal_logged = true;
                                    let mut result_storage_key = None;
                                    let mut result_access_token = None;
                                    if event.status == "succeeded" {
                                        if let (Some(image_bytes), Some(ext)) =
                                            (event.image_bytes.as_deref(), event.result_ext)
                                        {
                                            match write_image_result(
                                                &result_config,
                                                &log_meta.share_id,
                                                &log_meta.request_id,
                                                ext,
                                                image_bytes,
                                            )
                                            .await
                                            {
                                                Ok(saved) => {
                                                    result_storage_key = Some(saved.storage_key);
                                                    result_access_token = Some(saved.access_token);
                                                }
                                                Err(err) => {
                                                    warn!(request_id = %log_meta.request_id, error = %err, "write image result file failed");
                                                }
                                            }
                                        }
                                    }
                                    if let Err(err) = record_image_stream_log(
                                        &log_store,
                                        &result_config,
                                        &log_meta,
                                        ImageStreamLogOutcome {
                                            status: event.status,
                                            status_code: Some(status.as_u16()),
                                            latency_ms: request_started.elapsed().as_millis() as u64,
                                            completed_at: Some(chrono::Utc::now().timestamp()),
                                            error_message: event.error_message,
                                            result_mime_type: event.result_mime_type,
                                            result_size_bytes: event.result_size_bytes,
                                            result_storage_key,
                                            result_access_token,
                                        },
                                    )
                                    .await
                                    {
                                        warn!(request_id = %log_meta.request_id, error = %err, "record image stream terminal log failed");
                                    }
                                    completion_guard.mark_terminal();
                                }
                            }
                            yield Ok::<Bytes, std::io::Error>(bytes)
                        }
                        Some(Err(err)) => {
                            if !terminal_logged {
                                if let Err(log_err) = record_image_stream_log(
                                    &log_store,
                                    &result_config,
                                    &log_meta,
                                    ImageStreamLogOutcome {
                                        status: "failed",
                                        status_code: Some(status.as_u16()),
                                        latency_ms: request_started.elapsed().as_millis() as u64,
                                        completed_at: Some(chrono::Utc::now().timestamp()),
                                        error_message: Some(format!("read upstream stream failed: {err}")),
                                        result_mime_type: None,
                                        result_size_bytes: None,
                                        result_storage_key: None,
                                        result_access_token: None,
                                    },
                                )
                                .await
                                {
                                    warn!(request_id = %log_meta.request_id, error = %log_err, "record image stream read failure log failed");
                                }
                                completion_guard.mark_terminal();
                            }
                            yield Err(std::io::Error::other(err.to_string()));
                            break;
                        }
                        None => {
                            if !terminal_logged {
                                if let Err(err) = record_image_stream_log(
                                    &log_store,
                                    &result_config,
                                    &log_meta,
                                    ImageStreamLogOutcome {
                                        status: "failed",
                                        status_code: Some(status.as_u16()),
                                        latency_ms: request_started.elapsed().as_millis() as u64,
                                        completed_at: Some(chrono::Utc::now().timestamp()),
                                        error_message: Some("stream ended before image_generation.completed".into()),
                                        result_mime_type: None,
                                        result_size_bytes: None,
                                        result_storage_key: None,
                                        result_access_token: None,
                                    },
                                )
                                .await
                                {
                                    warn!(request_id = %log_meta.request_id, error = %err, "record image stream incomplete log failed");
                                }
                                completion_guard.mark_terminal();
                            }
                            break;
                        }
                    }
                }
                _ = keepalive.tick() => {
                    yield Ok::<Bytes, std::io::Error>(Bytes::from_static(b": keepalive\n\n"));
                }
            }
        }
    };
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-transform"),
    );
    response
        .headers_mut()
        .insert("X-Accel-Buffering", HeaderValue::from_static("no"));
    response
}

#[derive(Debug, Clone)]
struct ImageStreamLogMeta {
    request_id: String,
    share_id: String,
    installation_id: String,
    share_name: String,
    provider_id: String,
    provider_name: String,
    app_type: String,
    model: String,
    created_at: i64,
    prompt_preview: Option<String>,
    created_by_email: Option<String>,
    client_ip: Option<String>,
    user_country: Option<String>,
}

struct ImageStreamLogOutcome {
    status: &'static str,
    status_code: Option<u16>,
    latency_ms: u64,
    completed_at: Option<i64>,
    error_message: Option<String>,
    result_mime_type: Option<String>,
    result_size_bytes: Option<u64>,
    result_storage_key: Option<String>,
    result_access_token: Option<String>,
}

struct ImageStreamCompletionGuard {
    store: AppStore,
    config: Config,
    meta: ImageStreamLogMeta,
    started: Instant,
    status_code: u16,
    terminal_logged: Arc<AtomicBool>,
}

impl ImageStreamCompletionGuard {
    fn new(
        store: AppStore,
        config: Config,
        meta: ImageStreamLogMeta,
        started: Instant,
        status_code: u16,
    ) -> Self {
        Self {
            store,
            config,
            meta,
            started,
            status_code,
            terminal_logged: Arc::new(AtomicBool::new(false)),
        }
    }

    fn mark_terminal(&self) {
        self.terminal_logged.store(true, Ordering::Relaxed);
    }
}

impl Drop for ImageStreamCompletionGuard {
    fn drop(&mut self) {
        if self.terminal_logged.load(Ordering::Relaxed) {
            return;
        }
        let store = self.store.clone();
        let config = self.config.clone();
        let meta = self.meta.clone();
        let status_code = self.status_code;
        let latency_ms = self.started.elapsed().as_millis() as u64;
        tokio::spawn(async move {
            if let Err(err) = record_image_stream_log(
                &store,
                &config,
                &meta,
                ImageStreamLogOutcome {
                    status: "failed",
                    status_code: Some(status_code),
                    latency_ms,
                    completed_at: Some(chrono::Utc::now().timestamp()),
                    error_message: Some(
                        "stream cancelled before image_generation.completed".into(),
                    ),
                    result_mime_type: None,
                    result_size_bytes: None,
                    result_storage_key: None,
                    result_access_token: None,
                },
            )
            .await
            {
                warn!(request_id = %meta.request_id, error = %err, "record image stream cancellation log failed");
            }
        });
    }
}

async fn record_image_stream_log(
    store: &AppStore,
    config: &Config,
    meta: &ImageStreamLogMeta,
    outcome: ImageStreamLogOutcome,
) -> Result<(), crate::error::AppError> {
    store
        .record_image_generation_request_log(NewImageGenerationRequestLog {
            request_id: meta.request_id.clone(),
            share_id: meta.share_id.clone(),
            installation_id: meta.installation_id.clone(),
            share_name: meta.share_name.clone(),
            provider_id: meta.provider_id.clone(),
            provider_name: meta.provider_name.clone(),
            app_type: meta.app_type.clone(),
            model: meta.model.clone(),
            status: outcome.status.into(),
            status_code: outcome.status_code,
            latency_ms: outcome.latency_ms,
            created_at: meta.created_at,
            completed_at: outcome.completed_at,
            prompt_preview: meta.prompt_preview.clone(),
            error_message: outcome.error_message,
            result_mime_type: outcome.result_mime_type,
            result_size_bytes: outcome.result_size_bytes,
            result_storage_key: outcome.result_storage_key,
            result_access_token: outcome.result_access_token,
            created_by_email: meta.created_by_email.clone(),
            client_ip: meta.client_ip.clone(),
            user_country: meta.user_country.clone(),
        })
        .await?;
    let stale_storage_keys = store
        .prune_image_generation_request_logs_for_share(
            &meta.share_id,
            IMAGE_GENERATION_REQUEST_LOG_RETAIN_PER_SHARE,
        )
        .await?;
    delete_image_result_files(config, stale_storage_keys).await;
    Ok(())
}

struct SavedImageResult {
    storage_key: String,
    access_token: String,
}

async fn write_image_result(
    config: &Config,
    share_id: &str,
    request_id: &str,
    ext: &str,
    bytes: &[u8],
) -> Result<SavedImageResult, std::io::Error> {
    let share_segment = storage_key_segment(share_id);
    let file_name = format!("{}.{}", storage_key_segment(request_id), ext);
    let storage_key = format!("{share_segment}/{file_name}");
    let Some(path) = image_result_path(config, &storage_key) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid image result storage key",
        ));
    };
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&path, bytes).await?;
    Ok(SavedImageResult {
        storage_key,
        access_token: image_result_access_token(),
    })
}

async fn delete_image_result_files(config: &Config, storage_keys: Vec<String>) {
    for storage_key in storage_keys {
        let Some(path) = image_result_path(config, &storage_key) else {
            continue;
        };
        if let Err(err) = tokio::fs::remove_file(&path).await {
            if err.kind() != std::io::ErrorKind::NotFound {
                warn!(
                    storage_key = %storage_key,
                    path = %path.display(),
                    error = %err,
                    "delete pruned image result file failed"
                );
            }
        }
    }
}

fn storage_key_segment(value: &str) -> String {
    let output = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if output.is_empty() {
        Uuid::new_v4().simple().to_string()
    } else {
        output
    }
}

fn image_result_access_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

#[derive(Debug)]
struct ImageStreamTerminalEvent {
    status: &'static str,
    error_message: Option<String>,
    result_mime_type: Option<String>,
    result_size_bytes: Option<u64>,
    result_ext: Option<&'static str>,
    image_bytes: Option<Vec<u8>>,
}

#[derive(Default)]
struct ImageStreamSseParser {
    buffer: Vec<u8>,
}

impl ImageStreamSseParser {
    fn feed(&mut self, bytes: &[u8], output_format: &str) -> Option<ImageStreamTerminalEvent> {
        self.buffer.extend_from_slice(bytes);
        let mut terminal = None;
        while let Some((index, separator_len)) = find_sse_separator(&self.buffer) {
            let block = self.buffer[..index].to_vec();
            self.buffer.drain(..index + separator_len);
            if let Some(event) = parse_image_stream_sse_block(&block, output_format) {
                terminal = Some(event);
                break;
            }
        }
        terminal
    }
}

fn find_sse_separator(buffer: &[u8]) -> Option<(usize, usize)> {
    for index in 0..buffer.len().saturating_sub(1) {
        if buffer[index] == b'\n' && buffer[index + 1] == b'\n' {
            return Some((index, 2));
        }
        if index + 3 < buffer.len()
            && buffer[index] == b'\r'
            && buffer[index + 1] == b'\n'
            && buffer[index + 2] == b'\r'
            && buffer[index + 3] == b'\n'
        {
            return Some((index, 4));
        }
    }
    None
}

fn parse_image_stream_sse_block(
    block: &[u8],
    output_format: &str,
) -> Option<ImageStreamTerminalEvent> {
    let text = std::str::from_utf8(block).ok()?;
    let mut event_name = "";
    let mut data_lines = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.starts_with(':') {
            continue;
        }
        if let Some(value) = line.strip_prefix("event:") {
            event_name = value.trim();
        } else if let Some(value) = line.strip_prefix("data:") {
            data_lines.push(value.trim_start());
        }
    }
    let data = data_lines.join("\n");
    let trimmed_data = data.trim();
    if trimmed_data.is_empty() || trimmed_data == "[DONE]" {
        return None;
    }
    let value = serde_json::from_str::<Value>(trimmed_data).ok();
    if event_name.contains("failed") || event_name.contains("error") {
        return Some(ImageStreamTerminalEvent {
            status: "failed",
            error_message: Some(
                value
                    .as_ref()
                    .and_then(extract_image_stream_error)
                    .unwrap_or_else(|| compact_prompt_preview(trimmed_data, 1000)),
            ),
            result_mime_type: None,
            result_size_bytes: None,
            result_ext: None,
            image_bytes: None,
        });
    }
    let Some(value) = value else {
        return None;
    };
    if let Some(error) = extract_image_stream_error(&value) {
        return Some(ImageStreamTerminalEvent {
            status: "failed",
            error_message: Some(error),
            result_mime_type: None,
            result_size_bytes: None,
            result_ext: None,
            image_bytes: None,
        });
    }
    let b64 = value
        .get("data")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("b64_json"))
        .and_then(Value::as_str);
    if let Some(b64) = b64 {
        return match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(image_bytes) => {
                let (mime, ext) = image_mime_and_ext(&image_bytes, output_format);
                let result_size = image_bytes.len() as u64;
                Some(ImageStreamTerminalEvent {
                    status: "succeeded",
                    error_message: None,
                    result_mime_type: Some(mime.into()),
                    result_size_bytes: Some(result_size),
                    result_ext: Some(ext),
                    image_bytes: Some(image_bytes),
                })
            }
            Err(err) => Some(ImageStreamTerminalEvent {
                status: "failed",
                error_message: Some(format!("decode upstream image failed: {err}")),
                result_mime_type: None,
                result_size_bytes: None,
                result_ext: None,
                image_bytes: None,
            }),
        };
    }
    if event_name == "image_generation.completed" {
        return Some(ImageStreamTerminalEvent {
            status: "failed",
            error_message: Some(
                "image_generation.completed did not contain data[0].b64_json".into(),
            ),
            result_mime_type: None,
            result_size_bytes: None,
            result_ext: None,
            image_bytes: None,
        });
    }
    None
}

fn extract_image_stream_error(value: &Value) -> Option<String> {
    value
        .get("error")
        .and_then(|error| {
            error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
        })
        .or_else(|| value.get("message").and_then(Value::as_str))
        .map(|message| compact_prompt_preview(message, 1000))
}

fn codex_image_generation_provider(share: &ShareForTest) -> Option<(String, String)> {
    let bound_provider_id = share
        .bindings
        .get("codex")
        .map(String::as_str)
        .filter(|value| !value.is_empty())?;
    share
        .app_providers
        .codex
        .iter()
        .find(|provider| {
            provider.id == bound_provider_id
                && provider.enabled
                && provider.codex_image_generation_enabled
        })
        .map(|provider| (provider.id.clone(), provider.name.clone()))
}

fn compact_prompt_preview(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        output.push_str("...");
    }
    output
}

fn normalize_image_output_format(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "jpg".into(),
        "webp" => "webp".into(),
        _ => "png".into(),
    }
}

fn image_mime_and_ext(bytes: &[u8], requested_format: &str) -> (&'static str, &'static str) {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return ("image/jpeg", "jpg");
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return ("image/webp", "webp");
    }
    match requested_format {
        "jpg" | "jpeg" => ("image/jpeg", "jpg"),
        "webp" => ("image/webp", "webp"),
        _ => ("image/png", "png"),
    }
}

fn json_response(status: StatusCode, value: Value) -> Response {
    let body = serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec());
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

fn json_error_response(status: StatusCode, message: &str) -> Response {
    let mut response = json_response(status, serde_json::json!({ "message": message }));
    response
        .headers_mut()
        .insert("x-share-router-error", HeaderValue::from_static("true"));
    if let Ok(value) = HeaderValue::from_str(message) {
        response
            .headers_mut()
            .insert("x-share-router-error-reason", value);
    }
    response
}

fn infer_share_request_app(path: &str, headers: &HeaderMap) -> Option<String> {
    for name in [
        "x-cc-switch-app",
        "x-cc-switch-app-type",
        "x-request-agent",
        "x-share-app",
    ] {
        if let Some(value) = headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| matches!(value.as_str(), "claude" | "codex" | "gemini"))
        {
            return Some(value);
        }
    }
    let path = path.trim_start_matches('/').to_ascii_lowercase();
    if path.starts_with("gemini/") || path.starts_with("v1beta/") {
        return Some("gemini".to_string());
    }
    if path.starts_with("anthropic/") || path.starts_with("v1/messages") {
        return Some("claude".to_string());
    }
    if path.starts_with("codex/")
        || path.starts_with("openai/")
        || path.starts_with("v1/chat/")
        || path.starts_with("v1/responses")
        || path.starts_with("v1/images/generations")
        || path.starts_with("images/generations")
        || path.starts_with("responses/")
    {
        return Some("codex".to_string());
    }
    None
}

fn simple_response(status: StatusCode, reason: &str) -> Response {
    let mut response = Response::new(Body::from(reason.to_string()));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert("x-share-router-error", HeaderValue::from_static("true"));
    if let Ok(value) = HeaderValue::from_str(reason) {
        response
            .headers_mut()
            .insert("x-share-router-error-reason", value.clone());
    }
    response
}

fn reconnecting_response() -> Response {
    let mut response = simple_response(StatusCode::SERVICE_UNAVAILABLE, "tunnel-reconnecting");
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    response
}

async fn with_signed_ingress_context(
    state: &ServerState,
    builder: reqwest::RequestBuilder,
    route: &RouteEntry,
    public_host: String,
    request_id: String,
    user_email: Option<String>,
    user_country: Option<String>,
) -> Result<reqwest::RequestBuilder, Response> {
    let installation_id = route.installation_id().unwrap_or_default();
    let control_secret = match state
        .store
        .installation_control_secret(installation_id)
        .await
    {
        Ok(Some(secret)) if !secret.trim().is_empty() => secret,
        Ok(_) => {
            return Err(simple_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "ingress-control-secret-missing",
            ));
        }
        Err(error) => {
            warn!(
                installation_id,
                error = %error,
                "proxy ingress context secret lookup failed"
            );
            return Err(simple_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "ingress-control-secret-lookup-failed",
            ));
        }
    };
    let route_id = route
        .share_id()
        .map(|share_id| format!("share:{share_id}"))
        .unwrap_or_else(|| format!("client:{installation_id}"));
    let signed = crate::ingress_context::sign(
        crate::ingress_context::IngressContext {
            protocol_epoch: crate::namespace::PROTOCOL_EPOCH.to_string(),
            router_id: state
                .config
                .tunnel_domain
                .trim_end_matches('.')
                .to_ascii_lowercase(),
            route_id,
            installation_id: installation_id.to_string(),
            target_lane_id: installation_id.to_string(),
            public_host,
            share_id: route.share_id.clone(),
            request_id,
            user_email,
            user_role: None,
            user_country,
            issued_at_ms: chrono::Utc::now().timestamp_millis(),
        },
        &control_secret,
    )
    .map_err(|error| {
        warn!(error, "proxy ingress context signing failed");
        simple_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "ingress-context-signing-failed",
        )
    })?;
    Ok(builder
        .header(
            crate::ingress_context::INGRESS_CONTEXT_HEADER,
            signed.encoded_context,
        )
        .header(
            crate::ingress_context::INGRESS_SIGNATURE_HEADER,
            signed.signature,
        ))
}

fn empty_response(status: StatusCode) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = status;
    response
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("-")
}

fn is_internal_share_context_header(name: &str) -> bool {
    matches!(
        name,
        "x-cc-switch-share-id"
            | "x-cc-switch-share-subdomain"
            | "x-cc-switch-request-id"
            | "x-cc-switch-user-email"
            | "x-cc-switch-user-country"
            | "x-cc-switch-user-country-iso3"
            | "x-cc-switch-data-source"
            | "x-cc-switch-source"
            | "x-cc-switch-health-check"
            | "x-cc-switch-web-user-email"
            | "x-cc-switch-web-role"
            | "x-cc-switch-installation-id"
            | "x-cc-switch-client-tunnel-subdomain"
            | "x-share-router-health-check"
            | "x-share-router-probe"
    )
}

fn is_internal_share_router_path(path: &str) -> bool {
    path.starts_with("/_share-router/")
}

fn share_route_skips_edge_auth(
    is_internal_share_router_path: bool,
    is_direct_share_web_request: bool,
) -> bool {
    is_internal_share_router_path || is_direct_share_web_request
}

async fn authorize_internal_share_router_get(
    state: &ServerState,
    route: &RouteEntry,
    headers: &HeaderMap,
    path_and_query: &str,
) -> bool {
    let Some(route_installation_id) = route.installation_id() else {
        return false;
    };
    let Some(installation_id) = single_header(headers, "x-ctl-installation-id") else {
        return false;
    };
    if installation_id != route_installation_id {
        return false;
    }
    let Some(timestamp_ms) =
        single_header(headers, "x-ctl-timestamp-ms").and_then(|value| value.parse::<i64>().ok())
    else {
        return false;
    };
    let Some(nonce) = single_header(headers, "x-ctl-nonce") else {
        return false;
    };
    let Some(signature) = single_header(headers, "x-ctl-signature") else {
        return false;
    };
    let Ok(Some(control_secret)) = state
        .store
        .installation_control_secret(route_installation_id)
        .await
    else {
        return false;
    };
    crate::ctl_client::verify_control_request_signature(
        "GET",
        path_and_query,
        &control_secret,
        &[],
        timestamp_ms,
        nonce,
        signature,
        chrono::Utc::now().timestamp_millis(),
    )
}

fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    value
        .to_str()
        .ok()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn with_share_user_country_headers(
    mut builder: reqwest::RequestBuilder,
    country_code: Option<&str>,
) -> reqwest::RequestBuilder {
    let Some(country) = country_code.map(str::trim).filter(|value| {
        value.len() == 2
            && value
                .as_bytes()
                .iter()
                .all(|byte| byte.is_ascii_uppercase())
    }) else {
        return builder;
    };

    builder = builder.header(SHARE_USER_COUNTRY_HEADER, country);
    if let Some(iso3) = crate::geo::iso2_to_iso3(country) {
        builder = builder.header(SHARE_USER_COUNTRY_ISO3_HEADER, iso3);
    }
    builder
}

fn trusted_asn_header(headers: &HeaderMap, peer: SocketAddr) -> &str {
    if !crate::cf::is_cloudflare_peer(peer.ip()) {
        return "-";
    }
    ["cf-asn", "cf-connecting-asn"]
        .into_iter()
        .map(|name| header_str(headers, name))
        .find(|value| *value != "-")
        .unwrap_or("-")
}

fn is_abuse_tracked_api_path(path: &str) -> bool {
    matches!(
        path,
        "/v1/chat/completions" | "/v1/responses" | "/v1/messages" | "/v1/completions"
    )
}

fn is_invalid_auth_status(status: StatusCode) -> bool {
    matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
}

fn is_allowed_direct_share_proxy_path(path: &str) -> bool {
    is_allowed_direct_share_api_path(path) || is_allowed_direct_share_web_path(path)
}

fn is_allowed_direct_share_api_path(path: &str) -> bool {
    path == "/v1"
        || path.starts_with("/v1/")
        || path == "/v1beta"
        || path.starts_with("/v1beta/")
        || path == "/gemini/v1beta"
        || path.starts_with("/gemini/v1beta/")
        || path.starts_with("/_share-router/")
}

fn is_allowed_direct_share_web_path(path: &str) -> bool {
    path == "/"
        || path == "/favicon.ico"
        || path == "/favicon.png"
        || path.starts_with("/assets/")
        || path == "/web-api/context"
        || path.starts_with("/web-api/invoke/")
}

fn is_allowed_client_web_path(path: &str) -> bool {
    (path == "/"
        || path == "/favicon.ico"
        || path == "/favicon.png"
        || path.starts_with("/assets/")
        || path == "/web-api"
        || path.starts_with("/web-api/"))
        && !path.starts_with("/_ctl/")
        && !path.starts_with("/_share-router/")
        && !is_allowed_direct_share_api_path(path)
}

fn is_client_web_auth_required_path(path: &str) -> bool {
    (path == "/web-api" || path.starts_with("/web-api/")) && !is_public_client_web_path(path)
}

fn has_client_web_query_token(query: Option<&str>) -> bool {
    query.is_some_and(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .any(|(name, _)| matches!(name.as_ref(), "token" | "accessToken"))
    })
}

fn is_public_client_web_path(path: &str) -> bool {
    matches!(
        path,
        "/web-api/auth/methods"
            | "/web-api/auth/email/request-code"
            | "/web-api/auth/email/verify-code"
            | "/web-api/auth/session/refresh"
            | "/web-api/auth/password/setup"
            | "/web-api/auth/password/login"
            | "/web-api/auth/password/refresh"
            | "/web-api/auth/initial-setup"
            | "/web-api/oauth/claude-cli/callback"
            | "/web-api/oauth/openai-cli/callback"
    ) || is_public_client_debug_path(path)
}

fn is_public_client_debug_path(path: &str) -> bool {
    if matches!(
        path,
        "/web-api/debug/runtime"
            | "/web-api/debug/diagnostics"
            | "/web-api/debug/logs/tail"
            | "/web-api/debug/restart"
            | "/web-api/debug/upgrade"
            | "/web-api/debug/upgrade/status"
            | "/web-api/debug/upgrade/stream"
    ) {
        return true;
    }
    path.strip_prefix("/web-api/debug/operations/")
        .is_some_and(|id| id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn client_web_required_api_token_scope(path: &str) -> &'static str {
    if path.starts_with("/web-api/invoke/") {
        "share:write"
    } else {
        "share:read"
    }
}

async fn resolve_client_web_bearer(
    state: &ServerState,
    headers: &HeaderMap,
    owner_email: &str,
    required_api_token_scope: &str,
) -> Result<Option<(String, bool)>, crate::error::AppError> {
    let Some(token) = client_web_bearer_token(headers) else {
        return Ok(None);
    };
    if let Some(session) = state.store.resolve_session_by_access_token(token).await? {
        let email = session.email;
        let is_admin = state.dynamic.read().await.is_admin(&email);
        return Ok(Some((email, is_admin)));
    }
    if let Some(principal) = state
        .store
        .resolve_user_api_token(token, required_api_token_scope)
        .await?
    {
        let email = principal.email;
        let is_admin = state.dynamic.read().await.is_admin(&email);
        if email == owner_email || is_admin {
            return Ok(Some((email, is_admin)));
        }
        return Ok(Some((email, false)));
    }
    if required_api_token_scope == "share:write" {
        if let Some(principal) = state
            .store
            .resolve_user_api_token(token, "share:read")
            .await?
        {
            let email = principal.email;
            let is_admin = state.dynamic.read().await.is_admin(&email);
            if email.eq_ignore_ascii_case(owner_email) || is_admin {
                return Ok(Some((email, is_admin)));
            }
        }
    }
    Ok(None)
}

fn client_web_bearer_token(headers: &HeaderMap) -> Option<&str> {
    bearer_token(headers).or_else(|| {
        ["x-api-key", "x-goog-api-key", "api-key"]
            .iter()
            .find_map(|name| headers.get(*name).and_then(|value| value.to_str().ok()))
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn is_hop_by_hop_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "proxy-connection"
    )
}

fn is_event_stream_response(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
        })
}

fn proxy_body_chunk<E: std::fmt::Display>(
    is_event_stream: bool,
    stream_ended: &mut bool,
    chunk: Result<Bytes, E>,
) -> Option<Result<Bytes, E>> {
    if *stream_ended {
        return None;
    }
    match chunk {
        Ok(bytes) => Some(Ok(bytes)),
        Err(error) if is_event_stream => {
            *stream_ended = true;
            warn!(error = %error, "proxy SSE upstream stream ended unexpectedly");
            None
        }
        Err(error) => Some(Err(error)),
    }
}

fn strip_connection_listed_headers(headers: &mut HeaderMap) {
    let connection_values = headers
        .get_all("connection")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|value| value.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();

    headers.remove("connection");
    for header in connection_values {
        headers.remove(header);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    use axum::routing::any;

    use super::*;

    #[tokio::test]
    async fn forged_health_check_header_cannot_reach_share_backend() {
        let backend_hits = Arc::new(AtomicUsize::new(0));
        let backend_listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let backend_addr = backend_listener.local_addr().unwrap();
        let backend = axum::Router::new()
            .fallback(any(|State(hits): State<Arc<AtomicUsize>>| async move {
                hits.fetch_add(1, AtomicOrdering::SeqCst);
                StatusCode::OK
            }))
            .with_state(backend_hits.clone());
        tokio::spawn(async move {
            axum::serve(backend_listener, backend).await.unwrap();
        });

        let config = proxy_test_config("forged-health");
        let proxy = Arc::new(ProxyRegistry::default());
        proxy
            .set_route(
                "share-a".into(),
                backend_addr.to_string(),
                None,
                Some("share-a".into()),
                Some("Share A".into()),
                false,
                -1,
                None,
            )
            .await;
        let state = proxy_test_state(&config, proxy);
        let request = Request::builder()
            .method("POST")
            .uri("/v1/messages")
            .header("host", "share-a.router.test")
            .header("content-type", "application/json")
            .header("x-share-router-health-check", "1")
            .body(Body::from(r#"{"model":"test","messages":[]}"#))
            .unwrap();

        let response = proxy_handler(
            State(state),
            ConnectInfo(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12345)),
            request,
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(backend_hits.load(AtomicOrdering::SeqCst), 0);
        let _ = std::fs::remove_file(&config.db_path);
    }

    #[tokio::test]
    async fn unsigned_internal_request_logs_cannot_reach_share_backend() {
        let backend_hits = Arc::new(AtomicUsize::new(0));
        let backend_listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let backend_addr = backend_listener.local_addr().unwrap();
        let backend = axum::Router::new()
            .fallback(any(|State(hits): State<Arc<AtomicUsize>>| async move {
                hits.fetch_add(1, AtomicOrdering::SeqCst);
                StatusCode::OK
            }))
            .with_state(backend_hits.clone());
        tokio::spawn(async move {
            axum::serve(backend_listener, backend).await.unwrap();
        });

        let config = proxy_test_config("unsigned-request-logs");
        let proxy = Arc::new(ProxyRegistry::default());
        proxy
            .set_route_with_kind(
                "share-a".into(),
                backend_addr.to_string(),
                RouteKind::Share,
                Some("inst-a".into()),
                None,
                Some("share-a".into()),
                Some("Share A".into()),
                false,
                -1,
                None,
            )
            .await;
        let state = proxy_test_state(&config, proxy);
        let request = Request::builder()
            .method("GET")
            .uri("/_share-router/request-logs?shareId=share-a")
            .header("host", "share-a.router.test")
            .body(Body::empty())
            .unwrap();

        let response = proxy_handler(
            State(state),
            ConnectInfo(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12345)),
            request,
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(backend_hits.load(AtomicOrdering::SeqCst), 0);
        let _ = std::fs::remove_file(&config.db_path);
    }

    #[tokio::test]
    async fn signed_internal_request_logs_reach_share_backend() {
        let backend_hits = Arc::new(AtomicUsize::new(0));
        let backend_listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let backend_addr = backend_listener.local_addr().unwrap();
        let backend = axum::Router::new()
            .fallback(any(|State(hits): State<Arc<AtomicUsize>>| async move {
                hits.fetch_add(1, AtomicOrdering::SeqCst);
                StatusCode::OK
            }))
            .with_state(backend_hits.clone());
        tokio::spawn(async move {
            axum::serve(backend_listener, backend).await.unwrap();
        });

        let config = proxy_test_config("signed-request-logs");
        let proxy = Arc::new(ProxyRegistry::default());
        let state = proxy_test_state(&config, proxy.clone());
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[7_u8; 32]);
        let registered = state
            .store
            .register_installation(
                crate::models::RegisterInstallationRequest {
                    protocol_epoch: crate::namespace::PROTOCOL_EPOCH.into(),
                    public_key: base64::engine::general_purpose::STANDARD
                        .encode(signing_key.verifying_key().to_bytes()),
                    platform: "test".into(),
                    app_version: "test".into(),
                    instance_nonce: "nonce-register-signed-route".into(),
                    timestamp_ms: None,
                    signature: None,
                    proof_version: None,
                },
                crate::models::ClientMetadata {
                    ip: None,
                    country_code: None,
                },
            )
            .await
            .unwrap();
        let control_secret = registered.control_secret.unwrap();
        proxy
            .set_route_with_kind(
                "share-a".into(),
                backend_addr.to_string(),
                RouteKind::Share,
                Some(registered.installation_id.clone()),
                None,
                Some("share-a".into()),
                Some("Share A".into()),
                false,
                -1,
                None,
            )
            .await;

        let path = "/_share-router/request-logs?shareId=share-a";
        let signed = crate::ctl_client::authorize_control_request(
            reqwest::Client::new().get(format!("http://share-a.router.test{path}")),
            "GET",
            path,
            &registered.installation_id,
            &control_secret,
            &[],
        )
        .build()
        .unwrap();
        let mut request = Request::builder()
            .method("GET")
            .uri(path)
            .header("host", "share-a.router.test");
        for (name, value) in signed.headers() {
            request = request.header(name, value);
        }

        let response = proxy_handler(
            State(state),
            ConnectInfo(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12345)),
            request.body(Body::empty()).unwrap(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(backend_hits.load(AtomicOrdering::SeqCst), 1);
        let _ = std::fs::remove_file(&config.db_path);
    }

    fn proxy_test_state(config: &Config, proxy: Arc<ProxyRegistry>) -> ServerState {
        let metrics = crate::metrics::MetricsRegistry::new(config.metrics.clone());
        let (share_edit_events, _) = tokio::sync::broadcast::channel(16);
        ServerState {
            config: config.clone(),
            server_geo: crate::ServerGeo {
                lat: None,
                lon: None,
            },
            store: AppStore::new(config).unwrap(),
            proxy,
            proxy_http: reqwest::Client::new(),
            resend: None,
            resend_usage_cache: Arc::new(Mutex::new(None)),
            dynamic: Arc::new(RwLock::new(
                crate::dynamic_settings::DynamicSettings::from_config(config),
            )),
            ssh_host_fingerprint: None,
            recent_traffic: RecentTraffic::new(),
            abuse: Arc::new(crate::abuse::AbuseTracker::new()),
            ip_blacklist_stats: Arc::new(crate::ip_blacklist_stats::IpBlacklistStats::new()),
            telegram: Arc::new(RwLock::new(None)),
            upgrade_registry: Arc::new(crate::admin::upgrade::UpgradeRegistry::new()),
            share_edit_events,
            env_path: std::env::temp_dir().join("cc-switch-router-proxy-test.env"),
            start_instant: Instant::now(),
            scheduling_overrides: crate::scheduling_signals::OverrideStore::new(),
            metrics,
            payout_profile_read_limiter: Arc::new(
                crate::server_state::PublicPayoutProfileReadLimiter::default(),
            ),
            registration_admission: Arc::new(
                crate::registration_admission::RegistrationAdmissionLimiter::new(
                    crate::registration_admission::RegistrationAdmissionPolicy::default(),
                ),
            ),
        }
    }

    fn proxy_test_config(name: &str) -> Config {
        Config {
            api_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            ssh_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            tunnel_domain: "router.test".into(),
            ssh_public_addr: String::new(),
            use_localhost: true,
            lease_ttl_secs: 60,
            db_path: std::env::temp_dir().join(format!(
                "cc-switch-router-proxy-{name}-{}.db",
                Uuid::new_v4()
            )),
            host_key_path: std::env::temp_dir().join(format!(
                "cc-switch-router-proxy-{name}-{}.key",
                Uuid::new_v4()
            )),
            cleanup_interval_secs: 300,
            lease_retention_secs: 24 * 60 * 60,
            client_stale_secs: 60 * 60,
            client_installation_retention_secs: 6 * 60 * 60,
            paused_share_stale_secs: 60 * 60,
            resend_api_key: None,
            resend_from: None,
            resend_from_name: None,
            resend_reply_to: None,
            client_notifications: crate::config::ClientNotificationSettings::default(),
            auth_code_ttl_secs: 600,
            auth_code_cooldown_secs: 60,
            auth_session_ttl_secs: 7 * 24 * 60 * 60,
            auth_refresh_ttl_secs: 30 * 24 * 60 * 60,
            auth_max_verify_attempts: 8,
            auth_email_hourly_limit: 10,
            auth_ip_hourly_limit: 30,
            auth_installation_hourly_limit: 15,
            ip_blacklist: String::new(),
            free_share_ip_parallel_limit: 1,
            verification_service_base_url: "https://tokenswitch.org".into(),
            verification_service_api_key: None,
            admin_emails: HashSet::new(),
            telegram_bot_token: None,
            telegram_chat_id: None,
            telegram_topic_id: None,
            telegram_notify_all: false,
            telegram_notify_admin: false,
            board_max_len: 1000,
            board_guest_per_hour: 5,
            board_user_per_hour: 30,
            board_pin_limit: 3,
            board_guest_self_delete_secs: 300,
            ux_telemetry_enabled: false,
            ux_telemetry_retention_days: 7,
            footer_telegram_url: crate::config::DEFAULT_FOOTER_TELEGRAM_URL.to_string(),
            metrics: crate::config::MetricsConfig {
                enabled: false,
                db_path: std::env::temp_dir().join(format!(
                    "cc-switch-router-proxy-{name}-{}-metrics.db",
                    Uuid::new_v4()
                )),
                retention_days: 7,
                sample_interval_secs: 5,
            },
        }
    }

    #[test]
    fn image_generation_paths_infer_codex_app() {
        let headers = HeaderMap::new();

        assert_eq!(
            infer_share_request_app("/v1/images/generations", &headers).as_deref(),
            Some("codex")
        );
        assert_eq!(
            infer_share_request_app("/images/generations", &headers).as_deref(),
            Some("codex")
        );
    }

    #[test]
    fn image_generation_stream_detection_is_strict() {
        assert!(is_image_generation_submit_path("/v1/images/generations"));
        assert!(is_image_generation_submit_path("/images/generations"));
        assert!(!is_image_generation_submit_path(
            "/v1/images/generations/async"
        ));
        assert!(image_generation_request_wants_stream(
            br#"{"stream":true,"prompt":"draw"}"#
        ));
        assert!(!image_generation_request_wants_stream(
            br#"{"stream":false,"prompt":"draw"}"#
        ));
        assert!(!image_generation_request_wants_stream(b"not json"));
    }

    #[test]
    fn image_generation_completed_event_keeps_result_bytes_for_preview_storage() {
        let event = parse_image_stream_sse_block(
            br#"event: image_generation.completed
data: {"data":[{"b64_json":"iVBORw0KGgo="}]}

"#,
            "png",
        )
        .expect("terminal event");

        assert_eq!(event.status, "succeeded");
        assert_eq!(event.result_mime_type.as_deref(), Some("image/png"));
        assert_eq!(event.result_ext, Some("png"));
        assert_eq!(event.result_size_bytes, Some(8));
        assert_eq!(
            event.image_bytes.as_deref(),
            Some(&b"\x89PNG\r\n\x1a\n"[..])
        );
    }

    #[tokio::test]
    async fn share_concurrency_limiter_enforces_limit_and_releases_on_drop() {
        let limiter = Arc::new(KeyedConcurrencyLimiter::default());

        let permit_1 = limiter
            .try_acquire("share-1", 3)
            .await
            .expect("first permit");
        let permit_2 = limiter
            .try_acquire("share-1", 3)
            .await
            .expect("second permit");
        let permit_3 = limiter
            .try_acquire("share-1", 3)
            .await
            .expect("third permit");

        assert!(limiter.try_acquire("share-1", 3).await.is_none());

        drop(permit_1);
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        let permit_4 = limiter
            .try_acquire("share-1", 3)
            .await
            .expect("permit after release");

        drop(permit_2);
        drop(permit_3);
        drop(permit_4);
    }

    #[tokio::test]
    async fn share_concurrency_limiter_tracks_unlimited_shares_in_snapshot() {
        let limiter = Arc::new(KeyedConcurrencyLimiter::default());

        let permit_a = limiter
            .try_acquire("unlimited-share", -1)
            .await
            .expect("unlimited grants permit");
        let permit_b = limiter
            .try_acquire("unlimited-share", -1)
            .await
            .expect("unlimited grants second permit");
        let _permit_c = limiter
            .try_acquire("limited-share", 5)
            .await
            .expect("limited grants permit");

        let snapshot = limiter.snapshot().await;
        assert_eq!(snapshot.get("unlimited-share").copied(), Some(2));
        assert_eq!(snapshot.get("limited-share").copied(), Some(1));

        drop(permit_a);
        drop(permit_b);
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        let snapshot = limiter.snapshot().await;
        assert!(snapshot.get("unlimited-share").is_none());
    }

    #[tokio::test]
    async fn backend_lookup_returns_share_metadata() {
        let registry = ProxyRegistry::default();
        registry
            .set_route(
                "demo".into(),
                "127.0.0.1:3000".into(),
                None,
                Some("share-1".into()),
                Some("Demo Share".into()),
                true,
                5,
                None,
            )
            .await;

        let route = registry
            .backend_for_host("demo.example.com", "example.com")
            .await
            .expect("route metadata");

        assert_eq!(route.backend, "127.0.0.1:3000");
        assert_eq!(route.share_id.as_deref(), Some("share-1"));
        assert!(route.is_free_share);
        assert_eq!(route.parallel_limit, 5);
    }

    #[tokio::test]
    async fn backend_lookup_handles_tunnel_domain_with_port() {
        let registry = ProxyRegistry::default();
        registry
            .set_route(
                "demo".into(),
                "127.0.0.1:3000".into(),
                None,
                Some("share-1".into()),
                None,
                false,
                5,
                None,
            )
            .await;

        assert!(
            registry
                .backend_for_host("demo.127.0.0.1:8787", "127.0.0.1:8787")
                .await
                .is_some()
        );
        assert!(
            registry
                .backend_for_host("demo.127.0.0.1:9999", "127.0.0.1:8787")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn stale_connection_cannot_remove_replaced_route() {
        let registry = ProxyRegistry::default();
        registry
            .set_route(
                "demo".into(),
                "127.0.0.1:3000".into(),
                Some("old-connection".into()),
                Some("share-1".into()),
                Some("Demo Share".into()),
                false,
                5,
                None,
            )
            .await;
        registry
            .set_route(
                "demo".into(),
                "127.0.0.1:3001".into(),
                Some("new-connection".into()),
                Some("share-1".into()),
                Some("Demo Share".into()),
                false,
                5,
                None,
            )
            .await;

        registry
            .remove_route_if_connection("demo", "old-connection")
            .await;

        let route = registry
            .backend_for_host("demo.example.com", "example.com")
            .await
            .expect("new route should remain");
        assert_eq!(route.backend, "127.0.0.1:3001");
        assert_eq!(route.connection_id(), Some("new-connection"));

        registry
            .remove_route_if_connection("demo", "new-connection")
            .await;
        assert!(
            registry
                .backend_for_host("demo.example.com", "example.com")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn replacing_or_removing_route_signals_forward_shutdown() {
        let registry = ProxyRegistry::default();
        let (old_shutdown, mut old_rx) = RouteShutdown::new();
        registry
            .set_route(
                "demo".into(),
                "127.0.0.1:3000".into(),
                Some("old-connection".into()),
                Some("share-1".into()),
                None,
                false,
                5,
                Some(old_shutdown),
            )
            .await;

        let (new_shutdown, mut new_rx) = RouteShutdown::new();
        registry
            .set_route(
                "demo".into(),
                "127.0.0.1:3001".into(),
                Some("new-connection".into()),
                Some("share-1".into()),
                None,
                false,
                5,
                Some(new_shutdown),
            )
            .await;

        old_rx
            .changed()
            .await
            .expect("old route should receive shutdown");
        assert!(*old_rx.borrow());
        assert!(!*new_rx.borrow());

        registry
            .remove_route_if_connection("demo", "new-connection")
            .await;
        new_rx
            .changed()
            .await
            .expect("removed route should receive shutdown");
        assert!(*new_rx.borrow());
    }

    #[tokio::test]
    async fn candidate_is_not_routable_until_generation_cas_promotes_it() {
        let registry = ProxyRegistry::default();
        registry
            .set_route(
                "demo".into(),
                "127.0.0.1:3000".into(),
                Some("old-connection".into()),
                Some("share-1".into()),
                None,
                false,
                5,
                None,
            )
            .await;
        registry
            .register_candidate_with_kind(
                "demo".into(),
                "127.0.0.1:3001".into(),
                RouteKind::Share,
                Some("installation-1".into()),
                Some("new-connection".into()),
                Some("share-1".into()),
                None,
                false,
                5,
                None,
                2,
                "rotation-2".into(),
            )
            .await
            .expect("register candidate");

        let before = registry
            .backend_for_host("demo.example.com", "example.com")
            .await
            .expect("old active route");
        assert_eq!(before.backend, "127.0.0.1:3000");

        let conflict = registry
            .promote_candidate("demo", "new-connection", "rotation-2", 2, 0)
            .await
            .expect_err("wrong expected generation must fail");
        assert!(matches!(
            conflict,
            RouteGenerationError::CompareAndSwapConflict { .. }
        ));
        let still_old = registry
            .backend_for_host("demo.example.com", "example.com")
            .await
            .expect("old active survives failed CAS");
        assert_eq!(still_old.backend, "127.0.0.1:3000");

        registry
            .promote_candidate("demo", "new-connection", "rotation-2", 2, 1)
            .await
            .expect("promote candidate");
        let promoted = registry
            .backend_for_host("demo.example.com", "example.com")
            .await
            .expect("promoted route");
        assert_eq!(promoted.backend, "127.0.0.1:3001");
        assert_eq!(promoted.generation(), 2);
    }

    #[tokio::test]
    async fn promoted_route_drains_old_inflight_before_listener_shutdown() {
        let registry = ProxyRegistry::default();
        let (old_shutdown, mut old_rx) = RouteShutdown::new();
        registry
            .set_route(
                "demo".into(),
                "127.0.0.1:3000".into(),
                Some("old-connection".into()),
                Some("share-1".into()),
                None,
                false,
                5,
                Some(old_shutdown),
            )
            .await;
        let inflight = match registry
            .route_for_host_request("demo.example.com", "example.com")
            .await
        {
            RouteLookup::Active(_, guard) => guard,
            other => panic!("expected active route, got {other:?}"),
        };
        registry
            .register_candidate_with_kind(
                "demo".into(),
                "127.0.0.1:3001".into(),
                RouteKind::Share,
                Some("installation-1".into()),
                Some("new-connection".into()),
                Some("share-1".into()),
                None,
                false,
                5,
                None,
                2,
                "rotation-2".into(),
            )
            .await
            .expect("register candidate");
        registry
            .promote_candidate("demo", "new-connection", "rotation-2", 2, 1)
            .await
            .expect("promote candidate");

        assert!(
            tokio::time::timeout(Duration::from_millis(50), old_rx.changed())
                .await
                .is_err(),
            "old listener must remain while its selected request is in flight"
        );
        assert_eq!(
            registry
                .route_state("demo")
                .await
                .expect("route state")
                .draining_generations,
            vec![1]
        );

        drop(inflight);
        tokio::time::timeout(Duration::from_secs(1), old_rx.changed())
            .await
            .expect("old listener shutdown after drain")
            .expect("shutdown sender remains alive");
        assert!(*old_rx.borrow());
    }

    #[tokio::test]
    async fn stale_generation_cleanup_cannot_remove_new_active_generation() {
        let registry = ProxyRegistry::default();
        registry
            .set_route(
                "demo".into(),
                "127.0.0.1:3000".into(),
                Some("old-connection".into()),
                Some("share-1".into()),
                None,
                false,
                5,
                None,
            )
            .await;
        registry
            .register_candidate_with_kind(
                "demo".into(),
                "127.0.0.1:3001".into(),
                RouteKind::Share,
                None,
                Some("new-connection".into()),
                Some("share-1".into()),
                None,
                false,
                5,
                None,
                2,
                "rotation-2".into(),
            )
            .await
            .expect("register candidate");
        registry
            .promote_candidate("demo", "new-connection", "rotation-2", 2, 1)
            .await
            .expect("promote candidate");

        registry
            .remove_route_target_if_generation("demo", "old-connection", 1)
            .await;
        let active = registry
            .backend_for_host("demo.example.com", "example.com")
            .await
            .expect("new active remains");
        assert_eq!(active.connection_id(), Some("new-connection"));
        assert_eq!(active.generation(), 2);
    }

    #[tokio::test]
    async fn known_route_without_target_is_reconnecting_not_unknown() {
        let registry = ProxyRegistry::default();
        registry.declare_known_route("known".into()).await;

        assert!(matches!(
            registry
                .route_for_host_request("known.example.com", "example.com")
                .await,
            RouteLookup::Reconnecting
        ));
        assert!(matches!(
            registry
                .route_for_host_request("unknown.example.com", "example.com")
                .await,
            RouteLookup::Unknown
        ));
        let response = reconnecting_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::RETRY_AFTER], "1");
    }

    #[tokio::test]
    async fn health_probe_failure_cache_is_scoped_by_subdomain() {
        let registry = ProxyRegistry::default();

        registry.record_health_probe_failure("demo".into()).await;

        assert!(registry.has_cached_health_probe_failure("demo").await);
        assert!(!registry.has_cached_health_probe_failure("other").await);

        registry.clear_health_probe_failure("demo").await;
        assert!(!registry.has_cached_health_probe_failure("demo").await);
    }

    #[test]
    fn host_matching_ignores_request_port_when_tunnel_has_no_port() {
        assert!(host_matches_tunnel_domain(
            "demo.example.com:443",
            "example.com"
        ));
        assert_eq!(
            subdomain_for_host("market-a.example.com:443", "example.com").as_deref(),
            Some("market-a")
        );
    }

    #[test]
    fn direct_share_proxy_path_allows_gemini_native_api() {
        assert!(is_allowed_direct_share_proxy_path(
            "/v1beta/models/gemini-2.5-flash:streamGenerateContent"
        ));
        assert!(is_allowed_direct_share_proxy_path(
            "/gemini/v1beta/models/gemini-2.5-flash:streamGenerateContent"
        ));
    }

    #[test]
    fn direct_share_proxy_path_allows_web_shell_paths() {
        assert!(is_allowed_direct_share_proxy_path("/"));
        assert!(is_allowed_direct_share_proxy_path("/favicon.ico"));
        assert!(is_allowed_direct_share_proxy_path("/favicon.png"));
        assert!(is_allowed_direct_share_proxy_path("/assets/index-abc.js"));
        assert!(is_allowed_direct_share_proxy_path("/web-api/context"));
        assert!(is_allowed_direct_share_proxy_path(
            "/web-api/invoke/list_shares"
        ));
    }

    #[test]
    fn router_owned_share_context_headers_are_never_forwarded_from_callers() {
        for name in [
            "x-cc-switch-share-id",
            "x-cc-switch-share-subdomain",
            "x-cc-switch-request-id",
            "x-cc-switch-user-email",
            "x-cc-switch-user-country",
            "x-cc-switch-user-country-iso3",
            "x-cc-switch-data-source",
            "x-cc-switch-source",
            "x-cc-switch-health-check",
            "x-cc-switch-web-user-email",
            "x-cc-switch-web-role",
            "x-cc-switch-installation-id",
            "x-cc-switch-client-tunnel-subdomain",
            "x-share-router-health-check",
            "x-share-router-probe",
        ] {
            assert!(is_internal_share_context_header(name), "{name}");
        }
        assert!(!is_internal_share_context_header("x-api-key"));
        assert!(!is_internal_share_context_header("anthropic-version"));
    }

    #[test]
    fn caller_health_header_does_not_bypass_share_edge_auth() {
        let mut headers = HeaderMap::new();
        headers.insert("x-share-router-health-check", HeaderValue::from_static("1"));

        assert!(headers.contains_key("x-share-router-health-check"));
        assert!(!share_route_skips_edge_auth(
            is_internal_share_router_path("/v1/messages"),
            is_allowed_direct_share_web_path("/v1/messages"),
        ));
        assert!(share_route_skips_edge_auth(
            is_internal_share_router_path("/_share-router/request-logs"),
            false,
        ));
        assert!(!is_internal_share_router_path("/_share-router-evil/logs"));
    }

    #[test]
    fn client_web_path_exposes_only_static_and_web_api_namespaces() {
        assert!(is_allowed_client_web_path("/"));
        assert!(is_allowed_client_web_path("/favicon.ico"));
        assert!(is_allowed_client_web_path("/favicon.png"));
        assert!(is_allowed_client_web_path("/assets/index-abc.js"));
        assert!(is_allowed_client_web_path("/web-api/context"));
        assert!(is_allowed_client_web_path("/web-api/auth/password/login"));
        assert!(is_allowed_client_web_path("/web-api/auth/password/set"));
        assert!(is_allowed_client_web_path("/web-api/auth/initial-setup"));
        assert!(is_allowed_client_web_path("/web-api/invoke/get_providers"));
        assert!(is_allowed_client_web_path(
            "/web-api/invoke/get_proxy_takeover_status"
        ));
        assert!(is_allowed_client_web_path("/web-api/events"));
        assert!(is_allowed_client_web_path("/web-api/admin/upgrade/stream"));
        assert!(is_allowed_client_web_path("/web-api/admin/upgrade/status"));
        assert!(is_allowed_client_web_path("/web-api/admin/logs/tail"));
        assert!(!is_allowed_client_web_path("/api/providers"));
        assert!(!is_allowed_client_web_path("/v1/messages"));
        assert!(!is_allowed_client_web_path("/_ctl/apply_share_settings"));
        assert!(!is_allowed_client_web_path("/_share-router/health"));
    }

    #[test]
    fn client_web_auth_policy_defaults_web_api_to_private() {
        assert!(!is_client_web_auth_required_path(
            "/web-api/auth/password/login"
        ));
        assert!(!is_client_web_auth_required_path(
            "/web-api/oauth/openai-cli/callback"
        ));
        assert!(is_client_web_auth_required_path("/web-api/context"));
        assert!(is_client_web_auth_required_path("/web-api/events"));
        assert!(is_client_web_auth_required_path(
            "/web-api/admin/upgrade/stream"
        ));
        assert!(is_client_web_auth_required_path(
            "/web-api/admin/upgrade/status"
        ));
        assert!(is_client_web_auth_required_path("/web-api/admin/logs/tail"));
        assert!(is_client_web_auth_required_path("/web-api/future-command"));
        assert!(!is_client_web_auth_required_path(
            "/web-api/debug/diagnostics"
        ));
        assert!(!is_client_web_auth_required_path(
            "/web-api/debug/operations/0123456789abcdef0123456789abcdef"
        ));
        assert!(is_client_web_auth_required_path(
            "/web-api/debug/operations/not-an-operation-id"
        ));
        assert!(is_client_web_auth_required_path(
            "/web-api/debug/future-capability"
        ));
    }

    #[test]
    fn debug_api_public_paths_are_explicit() {
        for path in [
            "/web-api/debug/runtime",
            "/web-api/debug/diagnostics",
            "/web-api/debug/logs/tail",
            "/web-api/debug/restart",
            "/web-api/debug/upgrade",
            "/web-api/debug/upgrade/status",
            "/web-api/debug/upgrade/stream",
            "/web-api/debug/operations/0123456789abcdef0123456789abcdef",
        ] {
            assert!(is_public_client_debug_path(path), "{path}");
        }
        for path in [
            "/web-api/debug",
            "/web-api/debug/",
            "/web-api/debug/restart/extra",
            "/web-api/debug/operations/",
            "/web-api/debug/operations/../../admin",
            "/web-api/debug/future",
            "/web-api/invoke/restart_server_service",
        ] {
            assert!(!is_public_client_debug_path(path), "{path}");
        }
    }

    #[test]
    fn client_web_rejects_query_string_tokens() {
        assert!(has_client_web_query_token(Some("token=secret")));
        assert!(has_client_web_query_token(Some(
            "taskId=task-1&accessToken=secret"
        )));
        assert!(has_client_web_query_token(Some("%74oken=secret")));
        assert!(!has_client_web_query_token(Some("taskId=task-1")));
        assert!(!has_client_web_query_token(Some("Token=secret")));
        assert!(!has_client_web_query_token(None));
    }

    #[test]
    fn event_stream_content_type_allows_parameters_and_mixed_case() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("Text/Event-Stream; charset=utf-8"),
        );
        assert!(is_event_stream_response(&headers));
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        assert!(!is_event_stream_response(&headers));
    }

    #[test]
    fn event_stream_chunk_errors_become_clean_eof_only_for_sse() {
        let mut ended = false;
        let sse = proxy_body_chunk(
            true,
            &mut ended,
            Err::<Bytes, _>(std::io::Error::other("connection lost")),
        );
        assert!(sse.is_none());
        assert!(ended);

        let mut ended = false;
        let regular = proxy_body_chunk(
            false,
            &mut ended,
            Err::<Bytes, _>(std::io::Error::other("connection lost")),
        );
        assert!(regular.is_some_and(|chunk| chunk.is_err()));
        assert!(!ended);
    }

    #[test]
    fn direct_share_proxy_path_still_rejects_unknown_paths() {
        assert!(!is_allowed_direct_share_proxy_path("/health"));
        assert!(!is_allowed_direct_share_proxy_path("/settings"));
    }
}

fn mask_token(token: &str) -> String {
    if token.len() <= 8 {
        return "*".repeat(token.len());
    }
    format!("{}...{}", &token[..4], &token[token.len() - 4..])
}

fn is_valid_market_request_id(value: &str) -> bool {
    (8..=80).contains(&value.len())
        && value.starts_with("req_")
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

pub(crate) fn subdomain_for_host(host: &str, tunnel_domain: &str) -> Option<String> {
    let host = parse_authority(host)?;
    let tunnel = parse_authority(tunnel_domain)?;
    if let Some(tunnel_port) = tunnel.port {
        if host.port != Some(tunnel_port) {
            return None;
        }
    }
    let suffix = format!(".{}", tunnel.host);
    if !host.host.ends_with(&suffix) {
        return None;
    }
    let subdomain = host.host.trim_end_matches(&suffix);
    if subdomain.is_empty() || subdomain.contains('.') {
        return None;
    }
    Some(subdomain.to_string())
}

fn host_matches_tunnel_domain(host: &str, tunnel_domain: &str) -> bool {
    let Some(host) = parse_authority(host) else {
        return false;
    };
    let Some(tunnel) = parse_authority(tunnel_domain) else {
        return false;
    };
    if let Some(tunnel_port) = tunnel.port {
        if host.port != Some(tunnel_port) {
            return false;
        }
    }
    host.host == tunnel.host || host.host.ends_with(&format!(".{}", tunnel.host))
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedAuthority {
    host: String,
    port: Option<u16>,
}

fn parse_authority(value: &str) -> Option<ParsedAuthority> {
    let value = value.trim().trim_end_matches('/').to_ascii_lowercase();
    if value.is_empty() {
        return None;
    }
    let authority = if value.starts_with("http://") || value.starts_with("https://") {
        let url = url::Url::parse(&value).ok()?;
        let host = url.host_str()?.trim_end_matches('.').to_string();
        return Some(ParsedAuthority {
            host,
            port: url.port(),
        });
    } else {
        value.split('/').next()?.to_string()
    };

    if authority.starts_with('[') {
        let end = authority.find(']')?;
        let host = authority[..=end].trim_end_matches('.').to_string();
        let port = authority[end + 1..]
            .strip_prefix(':')
            .and_then(|port| port.parse::<u16>().ok());
        return Some(ParsedAuthority { host, port });
    }

    match authority.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|ch| ch.is_ascii_digit()) => Some(ParsedAuthority {
            host: host.trim_end_matches('.').to_string(),
            port: port.parse::<u16>().ok(),
        }),
        _ => Some(ParsedAuthority {
            host: authority.trim_end_matches('.').to_string(),
            port: None,
        }),
    }
}
