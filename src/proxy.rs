use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use base64::Engine;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::{Stream, StreamExt};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::net::SocketAddr;
use std::sync::{
    Arc, Mutex as StdMutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify, RwLock, mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::ServerState;
use crate::config::Config;
use crate::metrics::models::LlmRequestMetric;
use crate::metrics::{MetricsPermit, MetricsRegistry};
use crate::proxy_stream::{
    MAX_PROXY_IMAGE_STREAM_EVENT_BYTES, ProxyStreamDetector, ProxyStreamParseError,
    ProxyStreamProtocol,
};
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
const DEFAULT_PROXY_REQUEST_BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;
const MEDIA_REQUEST_BODY_LIMIT_BYTES: usize = 32 * 1024 * 1024;
const CODEX_IMAGES_REQUEST_BODY_LIMIT_BYTES: usize = 48 * 1024 * 1024;
const MAX_PROXY_ERROR_RESPONSE_BODY_BYTES: usize = 1024 * 1024;
const ROUTE_DRAIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const ROUTE_RECONNECT_WAIT_TIMEOUT: Duration = Duration::from_secs(3);
const ROUTE_RECONNECT_MAX_WAITERS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RouteKind {
    Share,
    Market,
    ClientWeb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InferenceSurface {
    OpenAi,
    Anthropic,
    Gemini,
}

impl InferenceSurface {
    fn from_app(app: &str) -> Self {
        match app {
            "claude" => Self::Anthropic,
            "gemini" => Self::Gemini,
            _ => Self::OpenAi,
        }
    }
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

#[derive(Debug)]
struct LogicalRoute {
    active: Option<RouteEntry>,
    candidates: BTreeMap<u64, RouteEntry>,
    draining: BTreeMap<u64, RouteEntry>,
    state_since: DateTime<Utc>,
    reconnecting_since: Instant,
    transition: Arc<RouteTransition>,
}

impl Default for LogicalRoute {
    fn default() -> Self {
        Self {
            active: None,
            candidates: BTreeMap::new(),
            draining: BTreeMap::new(),
            state_since: Utc::now(),
            reconnecting_since: Instant::now(),
            transition: Arc::new(RouteTransition::default()),
        }
    }
}

impl LogicalRoute {
    fn mark_active(&mut self, was_active: bool) {
        if !was_active {
            self.state_since = Utc::now();
        }
        self.transition.notify();
    }

    fn mark_reconnecting(&mut self) {
        self.state_since = Utc::now();
        self.reconnecting_since = Instant::now();
        self.transition.notify();
    }

    fn availability(&self, reconnect_grace: Duration) -> RouteAvailabilitySnapshot {
        if self.active.is_some() {
            return RouteAvailabilitySnapshot {
                state: RouteAvailability::Active,
                since: self.state_since,
            };
        }
        if self.reconnecting_since.elapsed() < reconnect_grace {
            return RouteAvailabilitySnapshot {
                state: RouteAvailability::Reconnecting,
                since: self.state_since,
            };
        }
        let grace = chrono::Duration::from_std(reconnect_grace).unwrap_or(chrono::Duration::MAX);
        RouteAvailabilitySnapshot {
            state: RouteAvailability::Offline,
            since: self
                .state_since
                .checked_add_signed(grace)
                .unwrap_or(self.state_since),
        }
    }
}

#[derive(Debug)]
struct RouteTransition {
    revision: watch::Sender<u64>,
    waiters: AtomicUsize,
}

impl Default for RouteTransition {
    fn default() -> Self {
        let (revision, _) = watch::channel(0);
        Self {
            revision,
            waiters: AtomicUsize::new(0),
        }
    }
}

impl RouteTransition {
    fn notify(&self) {
        self.revision.send_modify(|revision| {
            *revision = revision.wrapping_add(1);
        });
    }

    fn try_acquire_waiter(self: &Arc<Self>) -> Option<RouteWaiterGuard> {
        let mut current = self.waiters.load(Ordering::Acquire);
        loop {
            if current >= ROUTE_RECONNECT_MAX_WAITERS {
                return None;
            }
            match self.waiters.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Some(RouteWaiterGuard {
                        transition: self.clone(),
                    });
                }
                Err(actual) => current = actual,
            }
        }
    }
}

#[derive(Debug)]
struct RouteWaiterGuard {
    transition: Arc<RouteTransition>,
}

impl Drop for RouteWaiterGuard {
    fn drop(&mut self) {
        self.transition.waiters.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug)]
enum RouteLookup {
    Unknown,
    Reconnecting,
    Active(RouteEntry, RouteInflightGuard),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RouteAvailability {
    Active,
    Reconnecting,
    Offline,
}

impl RouteAvailability {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Reconnecting => "reconnecting",
            Self::Offline => "offline",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RouteAvailabilitySnapshot {
    pub state: RouteAvailability,
    pub since: DateTime<Utc>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConcurrencyLimitExceeded {
    current: usize,
    limit: usize,
}

#[derive(Debug)]
struct ShareConcurrencyPermit {
    registry: Arc<ShareRequestRegistry>,
    lease_id: String,
    started_at: Instant,
    cancellation: CancellationToken,
}

impl Drop for ShareConcurrencyPermit {
    fn drop(&mut self) {
        self.release_registry_lease();
    }
}

impl ShareConcurrencyPermit {
    fn release_registry_lease(&self) -> bool {
        self.registry.release_lease(&self.lease_id).is_some()
    }

    fn register_keyed_permit(&self, permit: &KeyedConcurrencyPermit) {
        self.registry
            .attach_keyed_release(&self.lease_id, permit.release_handle());
    }

    fn mark_response_headers_received(&self) {
        self.registry
            .set_phase(&self.lease_id, ShareRequestPhase::AwaitingFirstEvent);
    }

    fn record_progress(&self) {
        self.registry.record_progress(&self.lease_id);
    }

    fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    fn hard_deadline(&self, max_lifetime: Duration) -> tokio::time::Instant {
        tokio::time::Instant::from_std(self.started_at + max_lifetime)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShareRequestPhase {
    AwaitingResponseHeaders,
    AwaitingFirstEvent,
    Streaming,
}

impl ShareRequestPhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingResponseHeaders => "awaiting_response_headers",
            Self::AwaitingFirstEvent => "awaiting_first_event",
            Self::Streaming => "streaming",
        }
    }
}

#[derive(Debug, Clone)]
struct ShareRequestEntry {
    request_id: String,
    share_id: String,
    app: Option<String>,
    user_email: Option<String>,
    started_at: Instant,
    phase: ShareRequestPhase,
    phase_started_at: Instant,
    last_progress_at: Instant,
    cancellation: CancellationToken,
    keyed_releases: Vec<KeyedConcurrencyRelease>,
}

#[derive(Debug, Default)]
struct ShareRequestRegistry {
    requests: StdMutex<HashMap<String, ShareRequestEntry>>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ShareRequestRegistrySnapshot {
    pub active_requests: usize,
    pub oldest_inflight_age_secs: u64,
    pub oldest_progress_age_secs: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleasedShareRequest {
    pub lease_id: String,
    pub request_id: String,
    pub share_id: String,
    pub app: Option<String>,
    pub user_email: Option<String>,
    pub phase: String,
    pub age_secs: u64,
    pub progress_age_secs: u64,
    pub reason: String,
}

impl ShareRequestRegistry {
    async fn try_acquire(
        self: &Arc<Self>,
        request_id: &str,
        share_id: &str,
        app: Option<&str>,
        parallel_limit: i64,
        user_email: Option<&str>,
    ) -> Result<ShareConcurrencyPermit, ConcurrencyLimitExceeded> {
        let app = app
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .filter(|value| matches!(value.as_str(), "claude" | "codex" | "gemini"));
        let user_email = user_email
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase);
        let now = Instant::now();
        let mut requests = self
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = requests
            .values()
            .filter(|entry| entry.share_id == share_id)
            .count();
        if parallel_limit >= 0 && current >= parallel_limit as usize {
            return Err(ConcurrencyLimitExceeded {
                current,
                limit: parallel_limit as usize,
            });
        }
        let lease_id = format!("lease_{}", Uuid::new_v4().simple());
        let cancellation = CancellationToken::new();
        requests.insert(
            lease_id.clone(),
            ShareRequestEntry {
                request_id: request_id.to_string(),
                share_id: share_id.to_string(),
                app,
                user_email,
                started_at: now,
                phase: ShareRequestPhase::AwaitingResponseHeaders,
                phase_started_at: now,
                last_progress_at: now,
                cancellation: cancellation.clone(),
                keyed_releases: Vec::new(),
            },
        );
        Ok(ShareConcurrencyPermit {
            registry: self.clone(),
            lease_id,
            started_at: now,
            cancellation,
        })
    }

    fn release_lease(&self, lease_id: &str) -> Option<ShareRequestEntry> {
        let entry = self
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(lease_id);
        if let Some(entry) = entry.as_ref() {
            for release in &entry.keyed_releases {
                release.release();
            }
        }
        entry
    }

    fn attach_keyed_release(&self, lease_id: &str, release: KeyedConcurrencyRelease) {
        let attached = {
            let mut requests = self
                .requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = requests.get_mut(lease_id) {
                entry.keyed_releases.push(release.clone());
                true
            } else {
                false
            }
        };
        if !attached {
            release.release();
        }
    }

    fn set_phase(&self, lease_id: &str, phase: ShareRequestPhase) {
        let now = Instant::now();
        if let Some(entry) = self
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(lease_id)
            && entry.phase != phase
        {
            entry.phase = phase;
            entry.phase_started_at = now;
        }
    }

    fn record_progress(&self, lease_id: &str) {
        let now = Instant::now();
        if let Some(entry) = self
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(lease_id)
        {
            if entry.phase != ShareRequestPhase::Streaming {
                entry.phase = ShareRequestPhase::Streaming;
                entry.phase_started_at = now;
            }
            entry.last_progress_at = now;
        }
    }

    fn force_release_matching(
        &self,
        request_id: Option<&str>,
        share_id: Option<&str>,
        reason: &str,
    ) -> Vec<ReleasedShareRequest> {
        let now = Instant::now();
        let removed = {
            let mut requests = self
                .requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let lease_ids = requests
                .iter()
                .filter(|(_, entry)| {
                    request_id.is_some_and(|value| entry.request_id == value)
                        || share_id.is_some_and(|value| entry.share_id == value)
                })
                .map(|(lease_id, _)| lease_id.clone())
                .collect::<Vec<_>>();
            lease_ids
                .into_iter()
                .filter_map(|lease_id| requests.remove(&lease_id).map(|entry| (lease_id, entry)))
                .collect::<Vec<_>>()
        };
        removed
            .into_iter()
            .map(|(lease_id, entry)| {
                for release in &entry.keyed_releases {
                    release.release();
                }
                entry.cancellation.cancel();
                released_share_request(lease_id, entry, now, reason)
            })
            .collect()
    }

    fn release_stale(
        &self,
        config: &crate::config::ProxyStreamConfig,
    ) -> Vec<ReleasedShareRequest> {
        let now = Instant::now();
        let removed = {
            let mut requests = self
                .requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let stale = requests
                .iter()
                .filter_map(|(lease_id, entry)| {
                    stale_share_request_reason(entry, config, now)
                        .map(|reason| (lease_id.clone(), reason))
                })
                .collect::<Vec<_>>();
            stale
                .into_iter()
                .filter_map(|(lease_id, reason)| {
                    requests
                        .remove(&lease_id)
                        .map(|entry| (lease_id, entry, reason))
                })
                .collect::<Vec<_>>()
        };
        removed
            .into_iter()
            .map(|(lease_id, entry, reason)| {
                for release in &entry.keyed_releases {
                    release.release();
                }
                entry.cancellation.cancel();
                released_share_request(lease_id, entry, now, reason)
            })
            .collect()
    }

    async fn inflight_by_share(&self) -> HashMap<String, usize> {
        let mut result = HashMap::new();
        let requests = self
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for entry in requests.values() {
            *result.entry(entry.share_id.clone()).or_default() += 1;
        }
        result
    }

    async fn inflight_by_share_app(&self) -> HashMap<String, BTreeMap<String, usize>> {
        let mut result = HashMap::<String, BTreeMap<String, usize>>::new();
        let requests = self
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for entry in requests.values() {
            if let Some(app) = entry.app.as_ref() {
                *result
                    .entry(entry.share_id.clone())
                    .or_default()
                    .entry(app.clone())
                    .or_default() += 1;
            }
        }
        result
    }

    async fn inflight_by_share_user(
        &self,
    ) -> HashMap<String, BTreeMap<String, BTreeMap<String, usize>>> {
        let mut result = HashMap::<String, BTreeMap<String, BTreeMap<String, usize>>>::new();
        let requests = self
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for entry in requests.values() {
            let Some(user_email) = entry.user_email.as_ref() else {
                continue;
            };
            *result
                .entry(entry.share_id.clone())
                .or_default()
                .entry(entry.app.clone().unwrap_or_else(|| "_".to_string()))
                .or_default()
                .entry(user_email.clone())
                .or_default() += 1;
        }
        result
    }

    async fn snapshot(&self) -> ShareRequestRegistrySnapshot {
        let now = Instant::now();
        let requests = self
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ShareRequestRegistrySnapshot {
            active_requests: requests.len(),
            oldest_inflight_age_secs: requests
                .values()
                .map(|entry| now.duration_since(entry.started_at).as_secs())
                .max()
                .unwrap_or_default(),
            oldest_progress_age_secs: requests
                .values()
                .map(|entry| now.duration_since(entry.last_progress_at).as_secs())
                .max()
                .unwrap_or_default(),
        }
    }
}

fn stale_share_request_reason(
    entry: &ShareRequestEntry,
    config: &crate::config::ProxyStreamConfig,
    now: Instant,
) -> Option<&'static str> {
    if now.duration_since(entry.started_at) >= Duration::from_secs(config.max_request_lifetime_secs)
    {
        return Some("hard_lifetime_timeout");
    }
    let phase_age = now.duration_since(entry.phase_started_at);
    match entry.phase {
        ShareRequestPhase::AwaitingResponseHeaders
            if phase_age >= Duration::from_secs(config.response_header_timeout_secs) =>
        {
            Some("response_header_timeout")
        }
        ShareRequestPhase::AwaitingFirstEvent
            if phase_age >= Duration::from_secs(config.first_event_timeout_secs) =>
        {
            Some("first_event_timeout")
        }
        ShareRequestPhase::Streaming
            if now.duration_since(entry.last_progress_at)
                >= Duration::from_secs(config.idle_timeout_secs) =>
        {
            Some("business_idle_timeout")
        }
        _ => None,
    }
}

fn released_share_request(
    lease_id: String,
    entry: ShareRequestEntry,
    now: Instant,
    reason: &str,
) -> ReleasedShareRequest {
    ReleasedShareRequest {
        lease_id,
        request_id: entry.request_id,
        share_id: entry.share_id,
        app: entry.app,
        user_email: entry.user_email,
        phase: entry.phase.as_str().to_string(),
        age_secs: now.duration_since(entry.started_at).as_secs(),
        progress_age_secs: now.duration_since(entry.last_progress_at).as_secs(),
        reason: reason.to_string(),
    }
}

#[derive(Debug, Default)]
struct KeyedConcurrencyLimiter {
    counters: StdMutex<HashMap<String, usize>>,
}

#[derive(Debug, Clone)]
struct KeyedConcurrencyRelease {
    limiter: Arc<KeyedConcurrencyLimiter>,
    key: String,
    released: Arc<AtomicBool>,
}

impl KeyedConcurrencyRelease {
    fn release(&self) {
        if self.released.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut counters = self
            .limiter
            .counters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let should_remove = match counters.get_mut(&self.key) {
            Some(inflight) if *inflight > 1 => {
                *inflight -= 1;
                false
            }
            Some(_) => true,
            None => false,
        };
        if should_remove {
            counters.remove(&self.key);
        }
    }
}

#[derive(Debug)]
struct KeyedConcurrencyPermit {
    release: KeyedConcurrencyRelease,
}

impl KeyedConcurrencyPermit {
    fn release_handle(&self) -> KeyedConcurrencyRelease {
        self.release.clone()
    }
}

impl Drop for KeyedConcurrencyPermit {
    fn drop(&mut self) {
        self.release.release();
    }
}

/// Lifecycle guard that flips a recorded `RecentTraffic` event from
/// in-flight to completed when the proxy's response body stream ends. We
/// The async completion is spawned from `Drop` so the closure that owns the
/// guard never has to be `async`.
#[derive(Debug)]
struct RecentTrafficGuard {
    traffic: RecentTraffic,
    request_id: String,
    started: Instant,
    status_code: Option<u16>,
}

impl RecentTrafficGuard {
    fn new(traffic: RecentTraffic, request_id: String) -> Self {
        Self {
            traffic,
            request_id,
            started: Instant::now(),
            status_code: None,
        }
    }

    fn set_status(&mut self, status: StatusCode) {
        self.status_code = Some(status.as_u16());
    }
}

impl Drop for RecentTrafficGuard {
    fn drop(&mut self) {
        let traffic = self.traffic.clone();
        let request_id = std::mem::take(&mut self.request_id);
        if request_id.is_empty() {
            return;
        }
        let status_code = self.status_code;
        let latency_ms = self.started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        tokio::spawn(async move {
            traffic
                .complete_with_result(&request_id, status_code, Some(latency_ms))
                .await;
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
    error_kind: Option<String>,
    started: Instant,
}

#[derive(Debug, Default)]
struct ProxyResponseLifecycle {
    _route: Option<RouteInflightGuard>,
    share: Option<ShareConcurrencyPermit>,
    _free_share_ip: Option<KeyedConcurrencyPermit>,
    _image: Option<KeyedConcurrencyPermit>,
    _market: Option<KeyedConcurrencyPermit>,
    _recent_traffic: Option<RecentTrafficGuard>,
    _share_llm_metrics: Option<ShareLlmProxyMetricsGuard>,
    _metrics: Option<MetricsPermit>,
}

impl ProxyResponseLifecycle {
    fn mark_response_headers_received(&self) {
        if let Some(permit) = self.share.as_ref() {
            permit.mark_response_headers_received();
        }
    }

    fn record_progress(&self) {
        if let Some(permit) = self.share.as_ref() {
            permit.record_progress();
        }
    }

    fn cancellation_token(&self) -> CancellationToken {
        self.share
            .as_ref()
            .map(ShareConcurrencyPermit::cancellation_token)
            .unwrap_or_default()
    }

    fn hard_deadline(&self, max_lifetime: Duration) -> tokio::time::Instant {
        self.share
            .as_ref()
            .map(|permit| permit.hard_deadline(max_lifetime))
            .unwrap_or_else(|| tokio::time::Instant::now() + max_lifetime)
    }
}

fn finish_proxy_response_lifecycle(lifecycle: &mut Option<ProxyResponseLifecycle>) -> bool {
    let owns_share_release = lifecycle.as_ref().is_some_and(|lifecycle| {
        lifecycle
            .share
            .as_ref()
            .is_none_or(ShareConcurrencyPermit::release_registry_lease)
    });
    drop(lifecycle.take());
    owns_share_release
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
            error_kind: self.error_kind.clone(),
            http_status: Some(self.status),
            latency_ms: Some(latency_ms),
            ttft_ms: None,
            stream_started: false,
            stream_completed: false,
            usage_state: None,
            stream_status: None,
            usage_revision: 0,
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
    error_kind: Option<String>,
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
        error_kind,
        started,
    })
}

fn llm_error_kind(status: StatusCode, headers: &HeaderMap) -> Option<String> {
    let error_code = headers
        .get("x-cc-switch-error-code")
        .and_then(|value| value.to_str().ok())
        .map(str::trim);
    if error_code.is_some_and(|code| {
        matches!(
            code,
            "cc_switch_user_concurrency_limit_exceeded"
                | "cc_switch_share_concurrency_limit_exceeded"
                | "cc_switch_provider_account_concurrency_limit_exceeded"
                | "cc_switch_free_share_ip_concurrency_limit_exceeded"
                | "cc_switch_image_concurrency_limit_exceeded"
        )
    }) {
        return Some("concurrency_limited".into());
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Some("rate_limited".into());
    }
    if status.is_success() {
        None
    } else {
        Some("upstream_error".into())
    }
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
    ) -> Result<KeyedConcurrencyPermit, ConcurrencyLimitExceeded> {
        let mut counters = self
            .counters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let inflight = counters.entry(key.to_string()).or_insert(0);
        if parallel_limit >= 0 {
            let limit = parallel_limit as usize;
            if *inflight >= limit {
                return Err(ConcurrencyLimitExceeded {
                    current: *inflight,
                    limit,
                });
            }
        }
        *inflight += 1;
        Ok(KeyedConcurrencyPermit {
            release: KeyedConcurrencyRelease {
                limiter: self.clone(),
                key: key.to_string(),
                released: Arc::new(AtomicBool::new(false)),
            },
        })
    }

    async fn snapshot(&self) -> HashMap<String, usize> {
        self.counters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[derive(Debug, Default)]
pub struct ProxyRegistry {
    routes: Arc<RwLock<HashMap<String, LogicalRoute>>>,
    pending_routes: RwLock<HashMap<String, PendingRouteEntry>>,
    health_probe_failures: Mutex<HashMap<String, Instant>>,
    share_requests: Arc<ShareRequestRegistry>,
    free_share_ip_limiter: Arc<KeyedConcurrencyLimiter>,
    image_limiter: Arc<KeyedConcurrencyLimiter>,
    /// Tracks requests that actually traversed the market proxy path, keyed by
    /// lowercased market email. A request that hits a Share subdomain directly
    /// is not counted against the linked market. This stays separate from the
    /// Share request registry, which owns Share admission and lifecycle state.
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
            let was_active = slot.active.is_some();
            let old_route = slot.active.replace(route);
            if let Some(old_route) = old_route.as_ref() {
                slot.draining
                    .insert(old_route.generation, old_route.clone());
            }
            slot.mark_active(was_active);
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

            let was_active = slot.active.is_some();
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
            slot.mark_active(was_active);
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
        if slot.active.is_some() {
            slot.transition.notify();
        } else {
            slot.mark_reconnecting();
        }
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

    pub(crate) async fn declare_known_routes<I>(&self, subdomains: I)
    where
        I: IntoIterator<Item = String>,
    {
        let mut routes = self.routes.write().await;
        for subdomain in subdomains {
            routes.entry(subdomain).or_default();
        }
    }

    pub(crate) async fn route_availability(
        &self,
        subdomain: &str,
        reconnect_grace: Duration,
    ) -> Option<RouteAvailabilitySnapshot> {
        self.routes
            .read()
            .await
            .get(subdomain)
            .map(|slot| slot.availability(reconnect_grace))
    }

    pub(crate) async fn route_availability_snapshots(
        &self,
        reconnect_grace: Duration,
    ) -> HashMap<String, RouteAvailabilitySnapshot> {
        self.routes
            .read()
            .await
            .iter()
            .map(|(subdomain, slot)| (subdomain.clone(), slot.availability(reconnect_grace)))
            .collect()
    }

    pub async fn has_pending_route(&self, subdomain: &str) -> bool {
        let now = Instant::now();
        let mut pending = self.pending_routes.write().await;
        pending.retain(|_, entry| entry.expires_at > now);
        pending.contains_key(subdomain)
    }

    pub async fn remove_route(&self, subdomain: &str) {
        let old_route = self.routes.write().await.remove(subdomain);
        if let Some(route) = old_route.as_ref() {
            route.transition.notify();
        }
        self.pending_routes.write().await.remove(subdomain);
        shutdown_logical_route(old_route);
    }

    pub async fn remove_route_if_present(&self, subdomain: &str) -> bool {
        let old_route = self.routes.write().await.remove(subdomain);
        let removed = old_route.is_some();
        if let Some(route) = old_route.as_ref() {
            route.transition.notify();
        }
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
        let mut active_removed = false;
        if slot.active.as_ref().and_then(RouteEntry::connection_id) == Some(connection_id) {
            if let Some(route) = slot.active.take() {
                removed.push(route);
                active_removed = true;
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
        if active_removed {
            slot.mark_reconnecting();
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

    pub(crate) async fn remove_route_intent_if_connection(
        &self,
        subdomain: &str,
        connection_id: &str,
    ) -> bool {
        let old_route = {
            let mut routes = self.routes.write().await;
            let removable = routes.get(subdomain).is_some_and(|slot| {
                let activatable_routes = slot.active.iter().chain(slot.candidates.values());
                let mut has_snapshot_connection = false;
                let mut has_replacement_connection = false;
                for route in activatable_routes {
                    if route.connection_id() == Some(connection_id) {
                        has_snapshot_connection = true;
                    } else {
                        has_replacement_connection = true;
                    }
                }

                // The old connection may disappear between the cleanup snapshot
                // and the committed database deletion. An empty logical route is
                // still that snapshot's stale intent, while any active/candidate
                // replacement must win the race and remain registered.
                !has_replacement_connection
                    && (has_snapshot_connection
                        || (slot.active.is_none() && slot.candidates.is_empty()))
            });
            removable.then(|| routes.remove(subdomain)).flatten()
        };
        let Some(old_route) = old_route else {
            return false;
        };
        old_route.transition.notify();
        self.pending_routes.write().await.remove(subdomain);
        shutdown_logical_route(Some(old_route));
        true
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
            slot.mark_reconnecting();
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

    async fn wait_for_active_subdomain_request(&self, subdomain: &str) -> RouteLookup {
        let (mut revision, _waiter) = {
            let routes = self.routes.read().await;
            let Some(slot) = routes.get(subdomain) else {
                return RouteLookup::Unknown;
            };
            if let Some(route) = slot.active.as_ref() {
                return RouteLookup::Active(route.clone(), route.acquire());
            }
            let Some(waiter) = slot.transition.try_acquire_waiter() else {
                return RouteLookup::Reconnecting;
            };
            (slot.transition.revision.subscribe(), waiter)
        };

        let deadline = tokio::time::Instant::now() + ROUTE_RECONNECT_WAIT_TIMEOUT;
        loop {
            {
                let routes = self.routes.read().await;
                let Some(slot) = routes.get(subdomain) else {
                    return RouteLookup::Unknown;
                };
                if let Some(route) = slot.active.as_ref() {
                    return RouteLookup::Active(route.clone(), route.acquire());
                }
            }
            match tokio::time::timeout_at(deadline, revision.changed()).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) | Err(_) => return RouteLookup::Reconnecting,
            }
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

    pub(crate) async fn active_client_route(&self, subdomain: &str) -> Option<RouteEntry> {
        self.routes
            .read()
            .await
            .get(subdomain)
            .and_then(|slot| slot.active.as_ref())
            .filter(|route| route.is_client_web())
            .cloned()
    }

    pub(crate) async fn active_client_route_for_installation(
        &self,
        installation_id: &str,
    ) -> Option<RouteEntry> {
        self.routes
            .read()
            .await
            .values()
            .filter_map(|slot| slot.active.as_ref())
            .find(|route| {
                route.is_client_web() && route.installation_id.as_deref() == Some(installation_id)
            })
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
        self.share_requests.inflight_by_share().await
    }

    /// Snapshot of in-flight request counts per share_id and app_type. Unknown
    /// app requests are intentionally omitted from this app-level view while
    /// still counted by `inflight_by_share`.
    pub async fn inflight_by_share_app(&self) -> HashMap<String, BTreeMap<String, usize>> {
        self.share_requests.inflight_by_share_app().await
    }

    /// Snapshot of in-flight request counts per share, app, and user email.
    /// Keys are `{share_id}:{app}:{email}`; unknown app is stored as `_`.
    pub async fn inflight_by_share_user(
        &self,
    ) -> HashMap<String, BTreeMap<String, BTreeMap<String, usize>>> {
        self.share_requests.inflight_by_share_user().await
    }

    pub async fn share_request_registry_snapshot(&self) -> ShareRequestRegistrySnapshot {
        self.share_requests.snapshot().await
    }

    pub fn release_stale_share_requests(
        &self,
        config: &crate::config::ProxyStreamConfig,
    ) -> Vec<ReleasedShareRequest> {
        self.share_requests.release_stale(config)
    }

    pub fn force_release_share_requests(
        &self,
        request_id: Option<&str>,
        share_id: Option<&str>,
        reason: &str,
    ) -> Vec<ReleasedShareRequest> {
        self.share_requests
            .force_release_matching(request_id, share_id, reason)
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
        for index in 0..count {
            if let Ok(permit) = self
                .try_acquire_share_permit(
                    &format!("test:{share_id}:{index}:{}", Uuid::new_v4()),
                    share_id,
                    None,
                    -1,
                    None,
                )
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
        request_id: &str,
        share_id: &str,
        app_type: Option<&str>,
        parallel_limit: i64,
        user_email: Option<&str>,
    ) -> Result<ShareConcurrencyPermit, ConcurrencyLimitExceeded> {
        self.share_requests
            .try_acquire(request_id, share_id, app_type, parallel_limit, user_email)
            .await
    }

    async fn try_acquire_free_share_ip_permit(
        &self,
        user_ip: &str,
        parallel_limit: i64,
    ) -> Result<KeyedConcurrencyPermit, ConcurrencyLimitExceeded> {
        self.free_share_ip_limiter
            .try_acquire(user_ip, parallel_limit)
            .await
    }

    async fn try_acquire_image_permit(
        &self,
        share_id: &str,
        parallel_limit: i64,
    ) -> Result<KeyedConcurrencyPermit, ConcurrencyLimitExceeded> {
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
    let market_email = market.email.trim().to_ascii_lowercase();
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
    let Some(request_app) = infer_share_request_app(&path_and_query) else {
        return simple_response(StatusCode::BAD_REQUEST, "unsupported-share-api-path");
    };
    let admission_request_id = header_str(&parts.headers, MARKET_REQUEST_ID_HEADER)
        .split_whitespace()
        .next()
        .filter(|value| is_valid_market_request_id(value))
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let active_subdomains = state.proxy.active_subdomains().await.into_iter().collect();
    let inflight_by_share = state.proxy.inflight_by_share().await;
    let authorized = match state
        .store
        .list_market_shares_for_app(
            &market_email,
            "main",
            &active_subdomains,
            &inflight_by_share,
            &request_app,
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
    let connection_listed_headers = connection_listed_header_names(&parts.headers);
    for (name, value) in &parts.headers {
        let n = name.as_str();
        if n.eq_ignore_ascii_case("host")
            || is_sensitive_upstream_credential_header(n)
            || n.eq_ignore_ascii_case(MARKET_REQUEST_ID_HEADER)
            || is_internal_share_context_header(n)
            || is_hop_by_hop_header(n)
            || connection_listed_headers.contains(n)
        {
            continue;
        }
        builder = builder.header(name, value);
    }
    builder = builder.header("X-CC-Switch-Share-Id", share_id.as_str());
    builder = builder.header(SHARE_DATA_SOURCE_HEADER, "market");

    let log_share_id = mask_token(&share_id);
    let live_request_id = Some(admission_request_id);
    if let Some(ref request_id) = live_request_id {
        builder = builder.header("X-CC-Switch-Request-Id", request_id.as_str());
    }
    let body = match read_proxy_request_body(
        body,
        proxy_request_body_limit(&path_and_query),
        Duration::from_secs(state.config.proxy_stream.request_body_timeout_secs),
    )
    .await
    {
        Ok(body) => body,
        Err(ProxyRequestBodyReadError::Timeout) => {
            state.metrics.record_proxy_request_body_timeout();
            warn!(path = %path_and_query, "market proxy request body timed out");
            return simple_response(StatusCode::REQUEST_TIMEOUT, "request-body-timeout");
        }
        Err(ProxyRequestBodyReadError::Rejected(error)) => {
            warn!(error = %error, path = %path_and_query, "market proxy request body rejected");
            return simple_response(StatusCode::PAYLOAD_TOO_LARGE, "request-body-too-large");
        }
    };
    let share_permit = match state
        .proxy
        .try_acquire_share_permit(
            live_request_id
                .as_deref()
                .expect("market Share requests always have a request id"),
            &share_id,
            Some(&request_app),
            route.parallel_limit,
            Some(market_email.as_str()),
        )
        .await
    {
        Ok(permit) => permit,
        Err(exceeded) => {
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
            record_llm_admission_rejection(
                &state,
                &route,
                live_request_id
                    .as_deref()
                    .expect("market Share requests always have a request id"),
                &request_app,
                "market",
                Some(&market_email),
            );
            return llm_concurrency_response(
                &request_app,
                "cc_switch_share_concurrency_limit_exceeded",
                "share",
                exceeded.current,
                exceeded.limit,
                live_request_id
                    .as_deref()
                    .expect("market Share requests always have a request id"),
                format!(
                    "Share concurrency limit has been reached ({}/{}). Wait for an in-flight request to finish.",
                    exceeded.current, exceeded.limit
                ),
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
            Ok(permit) => Some(permit),
            Err(exceeded) => {
                let request_id = live_request_id
                    .as_deref()
                    .expect("market Share requests always have a request id");
                record_llm_admission_rejection(
                    &state,
                    &route,
                    request_id,
                    &request_app,
                    "market",
                    Some(&market_email),
                );
                return llm_concurrency_response(
                    &request_app,
                    "cc_switch_free_share_ip_concurrency_limit_exceeded",
                    "free_share_ip",
                    exceeded.current,
                    exceeded.limit,
                    request_id,
                    format!(
                        "Free Share IP concurrency limit has been reached ({}/{}). Wait for an in-flight request to finish.",
                        exceeded.current, exceeded.limit
                    ),
                );
            }
        }
    } else {
        None
    };
    if let Some(permit) = free_share_ip_permit.as_ref() {
        share_permit.register_keyed_permit(permit);
    }
    let market_permit = state.proxy.acquire_market_permit(&market_email).await;
    share_permit.register_keyed_permit(&market_permit);
    let share_cancellation = share_permit.cancellation_token();
    if route.is_share() || route.is_client_web() {
        let installation_id = route.installation_id().unwrap_or_default();
        let Some(control_secret_result) = await_share_request_or_cancel(
            &share_cancellation,
            state.store.installation_control_secret(installation_id),
        )
        .await
        else {
            return simple_response(StatusCode::SERVICE_UNAVAILABLE, "share-request-cancelled");
        };
        let control_secret = match control_secret_result {
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
        let route_id = route
            .share_id()
            .map(|share_id| format!("share:{share_id}"))
            .unwrap_or_else(|| format!("client:{installation_id}"));
        let signed_path_and_query = match outbound_request_path_and_query(&builder) {
            Ok(path_and_query) => path_and_query,
            Err(error) => {
                warn!(%error, "proxy ingress outbound request binding failed");
                return simple_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "ingress-context-signing-failed",
                );
            }
        };
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
                request_id: live_request_id
                    .clone()
                    .expect("market Share requests always have a request id"),
                user_email: Some(market_email.clone()),
                user_role: None,
                user_country: client_metadata.country_code.clone(),
                method: method.as_str().to_string(),
                path_and_query: signed_path_and_query,
                body_sha256: crate::ingress_context::body_sha256_hex(&body),
                signature_version: crate::ingress_context::SIGNATURE_VERSION,
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
    if await_share_request_or_cancel(
        &share_cancellation,
        state.recent_traffic.record_with_id(
            live_request_id
                .clone()
                .expect("market Share requests always have a request id"),
            share_id.clone(),
            route.share_name.clone(),
            Some(route.subdomain.clone()),
            client_metadata.country_code.clone(),
            Some(market_email.clone()),
        ),
    )
    .await
    .is_none()
    {
        return simple_response(StatusCode::SERVICE_UNAVAILABLE, "share-request-cancelled");
    }
    let mut recent_traffic_guard = live_request_id
        .as_ref()
        .map(|id| RecentTrafficGuard::new(state.recent_traffic.clone(), id.clone()));

    let upstream = match send_proxy_upstream_request(
        builder.body(reqwest::Body::from(body)),
        Duration::from_secs(state.config.proxy_stream.response_header_timeout_secs),
        Some(share_permit.cancellation_token()),
    )
    .await
    {
        Ok(response) => response,
        Err(ProxyUpstreamRequestError::ResponseHeaderTimeout) => {
            if let Some(guard) = recent_traffic_guard.as_mut() {
                guard.set_status(StatusCode::GATEWAY_TIMEOUT);
            }
            if share_permit.release_registry_lease() {
                state.metrics.record_proxy_response_header_timeout();
                warn!(method = %method, host = %host, path = %path_and_query, backend = %backend, share_id = %log_share_id, "market proxy upstream response headers timed out");
            }
            return simple_response(
                StatusCode::GATEWAY_TIMEOUT,
                "upstream-response-header-timeout",
            );
        }
        Err(ProxyUpstreamRequestError::Cancelled) => {
            if let Some(guard) = recent_traffic_guard.as_mut() {
                guard.set_status(StatusCode::SERVICE_UNAVAILABLE);
            }
            return simple_response(StatusCode::SERVICE_UNAVAILABLE, "share-request-cancelled");
        }
        Err(ProxyUpstreamRequestError::Request(err)) => {
            if let Some(guard) = recent_traffic_guard.as_mut() {
                guard.set_status(StatusCode::SERVICE_UNAVAILABLE);
            }
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

    share_permit.mark_response_headers_received();
    let status = upstream.status();
    if let Some(guard) = recent_traffic_guard.as_mut() {
        guard.set_status(status);
    }
    let response_headers = upstream.headers().clone();
    if let Some(response) =
        ingress_rejection_response(&state, status, &response_headers, &route, &path_and_query)
    {
        state.metrics.record_proxy_status(response.status());
        return response;
    }
    state.metrics.record_proxy_status(status);
    if llm_error_kind(status, &response_headers).as_deref() == Some("concurrency_limited") {
        record_llm_admission_rejection(
            &state,
            &route,
            live_request_id
                .as_deref()
                .expect("market Share requests always have a request id"),
            &request_app,
            "market",
            Some(&market_email),
        );
    }
    let is_event_stream = is_event_stream_response(&response_headers);
    let body_stream = proxy_response_body_stream(
        upstream.bytes_stream(),
        is_event_stream
            .then(|| ProxyStreamProtocol::from_path(&path_and_query))
            .flatten(),
        is_event_stream,
        ProxyResponseTimeouts::from(&state.config.proxy_stream),
        state.metrics.clone(),
        ProxyResponseLifecycle {
            _route: Some(route_inflight_guard),
            share: Some(share_permit),
            _free_share_ip: free_share_ip_permit,
            _market: Some(market_permit),
            _recent_traffic: recent_traffic_guard,
            _metrics: Some(metrics_permit),
            ..Default::default()
        },
    );
    let body = Body::from_stream(body_stream);

    let mut response = Response::new(body);
    *response.status_mut() = status;
    response.headers_mut().clear();
    copy_upstream_response_headers(&response_headers, response.headers_mut());
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

    let body_bytes = match read_proxy_request_body(
        body,
        proxy_request_body_limit(&path),
        Duration::from_secs(state.config.proxy_stream.request_body_timeout_secs),
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(ProxyRequestBodyReadError::Timeout) => {
            state.metrics.record_proxy_request_body_timeout();
            warn!(method = %method, host = %host, path = %path, "gateway proxy request body timed out");
            return simple_response(StatusCode::REQUEST_TIMEOUT, "request-body-timeout");
        }
        Err(ProxyRequestBodyReadError::Rejected(err)) => {
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
            return simple_response(StatusCode::PAYLOAD_TOO_LARGE, "request-body-too-large");
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
    let Some(request_app) = infer_share_request_app(&path_and_query) else {
        return simple_response(StatusCode::BAD_REQUEST, "unsupported-share-api-path");
    };
    let admission_request_id = Uuid::new_v4().to_string();

    let active_subdomains = state.proxy.active_subdomains().await.into_iter().collect();
    let inflight_by_share = state.proxy.inflight_by_share().await;
    let authorized = match state
        .store
        .list_gateway_shares_for_app(
            &gateway,
            "main",
            &active_subdomains,
            &inflight_by_share,
            &request_app,
        )
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
    let share_permit = match state
        .proxy
        .try_acquire_share_permit(
            &admission_request_id,
            &share_id,
            Some(&request_app),
            route.parallel_limit,
            None,
        )
        .await
    {
        Ok(permit) => permit,
        Err(exceeded) => {
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
            record_llm_admission_rejection(
                &state,
                &route,
                &admission_request_id,
                &request_app,
                "gateway",
                None,
            );
            return llm_concurrency_response(
                &request_app,
                "cc_switch_share_concurrency_limit_exceeded",
                "share",
                exceeded.current,
                exceeded.limit,
                &admission_request_id,
                format!(
                    "Share concurrency limit has been reached ({}/{}). Wait for an in-flight request to finish.",
                    exceeded.current, exceeded.limit
                ),
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
            Ok(permit) => Some(permit),
            Err(exceeded) => {
                record_llm_admission_rejection(
                    &state,
                    &route,
                    &admission_request_id,
                    &request_app,
                    "gateway",
                    None,
                );
                return llm_concurrency_response(
                    &request_app,
                    "cc_switch_free_share_ip_concurrency_limit_exceeded",
                    "free_share_ip",
                    exceeded.current,
                    exceeded.limit,
                    &admission_request_id,
                    format!(
                        "Free Share IP concurrency limit has been reached ({}/{}). Wait for an in-flight request to finish.",
                        exceeded.current, exceeded.limit
                    ),
                );
            }
        }
    } else {
        None
    };
    if let Some(permit) = free_share_ip_permit.as_ref() {
        share_permit.register_keyed_permit(permit);
    }

    let mut builder = state.proxy_http.request(method.clone(), target);
    let connection_listed_headers = connection_listed_header_names(&parts.headers);
    for (name, value) in &parts.headers {
        let n = name.as_str();
        if n.eq_ignore_ascii_case("host")
            || is_sensitive_upstream_credential_header(n)
            || is_internal_share_context_header(n)
            || is_hop_by_hop_header(n)
            || connection_listed_headers.contains(n)
        {
            continue;
        }
        builder = builder.header(name, value);
    }
    builder = builder.header("X-CC-Switch-Share-Id", share_id.as_str());
    builder = builder.header("X-CC-Switch-Share-Subdomain", route.subdomain.as_str());
    builder = builder.header(SHARE_DATA_SOURCE_HEADER, "gateway");
    builder = with_share_user_country_headers(builder, client_metadata.country_code.as_deref());

    let live_request_id = admission_request_id;
    builder = builder.header("X-CC-Switch-Request-Id", live_request_id.as_str());
    let share_cancellation = share_permit.cancellation_token();
    let Some(signed_builder) = await_share_request_or_cancel(
        &share_cancellation,
        with_signed_ingress_context(
            &state,
            builder,
            &route,
            format!("{}.{}", route.subdomain, state.config.tunnel_domain),
            &live_request_id,
            None,
            client_metadata.country_code.clone(),
            &method,
            &body_bytes,
        ),
    )
    .await
    else {
        return simple_response(StatusCode::SERVICE_UNAVAILABLE, "share-request-cancelled");
    };
    builder = match signed_builder {
        Ok(builder) => builder,
        Err(response) => return response,
    };
    if await_share_request_or_cancel(
        &share_cancellation,
        state.recent_traffic.record_with_id(
            live_request_id.clone(),
            share_id.clone(),
            route.share_name.clone(),
            Some(route.subdomain.clone()),
            client_metadata.country_code.clone(),
            None,
        ),
    )
    .await
    .is_none()
    {
        return simple_response(StatusCode::SERVICE_UNAVAILABLE, "share-request-cancelled");
    }
    let mut recent_traffic_guard =
        RecentTrafficGuard::new(state.recent_traffic.clone(), live_request_id.clone());

    let upstream = match send_proxy_upstream_request(
        builder.body(reqwest::Body::from(body_bytes)),
        Duration::from_secs(state.config.proxy_stream.response_header_timeout_secs),
        Some(share_permit.cancellation_token()),
    )
    .await
    {
        Ok(response) => response,
        Err(ProxyUpstreamRequestError::ResponseHeaderTimeout) => {
            recent_traffic_guard.set_status(StatusCode::GATEWAY_TIMEOUT);
            if share_permit.release_registry_lease() {
                state.metrics.record_proxy_response_header_timeout();
                warn!(method = %method, host = %host, path = %path_and_query, backend = %backend, share_id = %share_id, "gateway proxy upstream response headers timed out");
            }
            return simple_response(
                StatusCode::GATEWAY_TIMEOUT,
                "upstream-response-header-timeout",
            );
        }
        Err(ProxyUpstreamRequestError::Cancelled) => {
            recent_traffic_guard.set_status(StatusCode::SERVICE_UNAVAILABLE);
            return simple_response(StatusCode::SERVICE_UNAVAILABLE, "share-request-cancelled");
        }
        Err(ProxyUpstreamRequestError::Request(err)) => {
            recent_traffic_guard.set_status(StatusCode::SERVICE_UNAVAILABLE);
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

    share_permit.mark_response_headers_received();
    let status = upstream.status();
    recent_traffic_guard.set_status(status);
    let response_headers = upstream.headers().clone();
    if let Some(response) =
        ingress_rejection_response(&state, status, &response_headers, &route, &path_and_query)
    {
        state.metrics.record_proxy_status(response.status());
        return response;
    }
    state.metrics.record_proxy_status(status);
    if llm_error_kind(status, &response_headers).as_deref() == Some("concurrency_limited") {
        record_llm_admission_rejection(
            &state,
            &route,
            &live_request_id,
            &request_app,
            "gateway",
            None,
        );
    }
    let is_event_stream = is_event_stream_response(&response_headers);
    let body_stream = proxy_response_body_stream(
        upstream.bytes_stream(),
        is_event_stream
            .then(|| ProxyStreamProtocol::from_path(&path_and_query))
            .flatten(),
        is_event_stream,
        ProxyResponseTimeouts::from(&state.config.proxy_stream),
        state.metrics.clone(),
        ProxyResponseLifecycle {
            _route: Some(route_inflight_guard),
            share: Some(share_permit),
            _free_share_ip: free_share_ip_permit,
            _recent_traffic: Some(recent_traffic_guard),
            _metrics: Some(metrics_permit),
            ..Default::default()
        },
    );
    let body = Body::from_stream(body_stream);

    let mut response = Response::new(body);
    *response.status_mut() = status;
    response.headers_mut().clear();
    copy_upstream_response_headers(&response_headers, response.headers_mut());
    if is_event_stream {
        response.headers_mut().remove(header::CONTENT_LENGTH);
    }
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
            if let Some(subdomain) = route_subdomain.as_deref() {
                match state
                    .proxy
                    .wait_for_active_subdomain_request(subdomain)
                    .await
                {
                    RouteLookup::Active(route, guard) => (route, guard),
                    RouteLookup::Unknown => {
                        warn!(
                            method = %method,
                            host = %host,
                            path = %path_and_query,
                            client_ip = %user_ip,
                            client_country = %user_country,
                            client_asn = %user_asn,
                            user_agent = %user_agent,
                            "proxy request rejected: route removed while waiting for reconnect"
                        );
                        return simple_response(StatusCode::NOT_FOUND, "unregistered-subdomain");
                    }
                    RouteLookup::Reconnecting => {
                        warn!(
                            method = %method,
                            host = %host,
                            path = %path_and_query,
                            client_ip = %user_ip,
                            client_country = %user_country,
                            client_asn = %user_asn,
                            user_agent = %user_agent,
                            wait_timeout_ms = ROUTE_RECONNECT_WAIT_TIMEOUT.as_millis(),
                            "proxy request deferred: registered tunnel is reconnecting"
                        );
                        return reconnecting_response();
                    }
                }
            } else {
                return simple_response(StatusCode::NOT_FOUND, "unregistered-subdomain");
            }
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
        if !is_internal_share_router_path && !is_allowed_client_web_path(&path) {
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
                    route.installation_id().as_deref(),
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
                    infer_share_request_app(&path).as_deref(),
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
        let body_bytes = match read_proxy_request_body(
            body,
            proxy_request_body_limit(&path),
            Duration::from_secs(state.config.proxy_stream.request_body_timeout_secs),
        )
        .await
        {
            Ok(body) => body,
            Err(ProxyRequestBodyReadError::Timeout) => {
                state.metrics.record_proxy_request_body_timeout();
                warn!(method = %method, host = %host, path = %path_and_query, "image generation request body timed out");
                return json_error_response(StatusCode::REQUEST_TIMEOUT, "request-body-timeout");
            }
            Err(ProxyRequestBodyReadError::Rejected(err)) => {
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
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request-body-too-large",
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
    let connection_listed_headers = connection_listed_header_names(&parts.headers);
    for (name, value) in &parts.headers {
        let n = name.as_str();
        if n.eq_ignore_ascii_case("host")
            || is_hop_by_hop_header(n)
            || connection_listed_headers.contains(n)
        {
            continue;
        }
        // Strip client-supplied user/share credentials on share routes; router
        // authenticates the caller at the edge (user_api_token + email ACL)
        // and the cc-switch-server tunnel only needs the share id we inject below.
        if should_strip_direct_proxy_internal_header(n, is_internal_share_router_path) {
            continue;
        }
        if route.is_share() && is_sensitive_upstream_credential_header(n) {
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

    // Inject share id so cc-switch-server can identify the share on its tunnel side
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
    let admission_request_id =
        (!skips_share_edge_auth && route.is_share()).then(|| Uuid::new_v4().to_string());

    let body = match read_proxy_request_body(
        body,
        proxy_request_body_limit(&path),
        Duration::from_secs(state.config.proxy_stream.request_body_timeout_secs),
    )
    .await
    {
        Ok(body) => body,
        Err(ProxyRequestBodyReadError::Timeout) => {
            state.metrics.record_proxy_request_body_timeout();
            warn!(method = %method, host = %host, path = %path_and_query, "proxy request body timed out");
            return simple_response(StatusCode::REQUEST_TIMEOUT, "request-body-timeout");
        }
        Err(ProxyRequestBodyReadError::Rejected(err)) => {
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
            return simple_response(StatusCode::PAYLOAD_TOO_LARGE, "request-body-too-large");
        }
    };
    let share_permit = if skips_share_edge_auth {
        None
    } else if let Some(share_id) = route.share_id.as_deref() {
        let request_app = infer_share_request_app(&path);
        match state
            .proxy
            .try_acquire_share_permit(
                admission_request_id
                    .as_deref()
                    .expect("Share admission requests always have a request id"),
                share_id,
                request_app.as_deref(),
                route.parallel_limit,
                api_user_email.as_deref(),
            )
            .await
        {
            Ok(permit) => Some(permit),
            Err(exceeded) => {
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
                let request_id = admission_request_id
                    .as_deref()
                    .expect("Share admission requests always have a request id");
                let app = request_app.as_deref().unwrap_or("codex");
                record_llm_admission_rejection(&state, &route, request_id, app, "direct", None);
                return llm_concurrency_response(
                    app,
                    "cc_switch_share_concurrency_limit_exceeded",
                    "share",
                    exceeded.current,
                    exceeded.limit,
                    request_id,
                    format!(
                        "Share concurrency limit has been reached ({}/{}). Wait for an in-flight request to finish.",
                        exceeded.current, exceeded.limit
                    ),
                );
            }
        }
    } else {
        None
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
            Ok(permit) => Some(permit),
            Err(exceeded) => {
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
                let request_id = admission_request_id
                    .as_deref()
                    .expect("Share admission requests always have a request id");
                let app = infer_share_request_app(&path).unwrap_or_else(|| "codex".to_string());
                record_llm_admission_rejection(&state, &route, request_id, &app, "direct", None);
                return llm_concurrency_response(
                    &app,
                    "cc_switch_free_share_ip_concurrency_limit_exceeded",
                    "free_share_ip",
                    exceeded.current,
                    exceeded.limit,
                    request_id,
                    format!(
                        "Free Share IP concurrency limit has been reached ({}/{}). Wait for an in-flight request to finish.",
                        exceeded.current, exceeded.limit
                    ),
                );
            }
        }
    } else {
        None
    };
    if let (Some(share_permit), Some(free_share_ip_permit)) =
        (share_permit.as_ref(), free_share_ip_permit.as_ref())
    {
        share_permit.register_keyed_permit(free_share_ip_permit);
    }
    let share_cancellation = share_permit
        .as_ref()
        .map(ShareConcurrencyPermit::cancellation_token)
        .unwrap_or_default();

    // Generate the downstream identity before signing. Recording happens only after all
    // local request preparation succeeds, so early local failures cannot leak inflight rows.
    let live_request_id = if !skips_share_edge_auth && !is_share_router_probe {
        route.share_id.as_ref().map(|_| {
            admission_request_id
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string())
        })
    } else {
        None
    };
    if let Some(ref request_id) = live_request_id {
        builder = builder.header("X-CC-Switch-Request-Id", request_id.as_str());
    }
    if route.is_share() || route.is_client_web() {
        let installation_id = route.installation_id().unwrap_or_default();
        let Some(control_secret_result) = await_share_request_or_cancel(
            &share_cancellation,
            state.store.installation_control_secret(installation_id),
        )
        .await
        else {
            return simple_response(StatusCode::SERVICE_UNAVAILABLE, "share-request-cancelled");
        };
        let control_secret = match control_secret_result {
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
        let route_id = route
            .share_id()
            .map(|share_id| format!("share:{share_id}"))
            .unwrap_or_else(|| format!("client:{installation_id}"));
        let signed_path_and_query = match outbound_request_path_and_query(&builder) {
            Ok(path_and_query) => path_and_query,
            Err(error) => {
                warn!(%error, "proxy ingress outbound request binding failed");
                return simple_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "ingress-context-signing-failed",
                );
            }
        };
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
                request_id: live_request_id
                    .clone()
                    .unwrap_or_else(|| Uuid::new_v4().to_string()),
                user_email: api_user_email
                    .clone()
                    .or_else(|| client_web_session.as_ref().map(|(email, _)| email.clone())),
                user_role: client_web_session
                    .as_ref()
                    .map(|(_, is_admin)| if *is_admin { "admin" } else { "owner" }.to_string()),
                user_country: client_metadata.country_code.clone(),
                method: method.as_str().to_string(),
                path_and_query: signed_path_and_query,
                body_sha256: crate::ingress_context::body_sha256_hex(&body),
                signature_version: crate::ingress_context::SIGNATURE_VERSION,
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
    if let (Some(request_id), Some(share_id)) = (live_request_id.as_ref(), route.share_id.as_ref())
    {
        if await_share_request_or_cancel(
            &share_cancellation,
            state.recent_traffic.record_with_id(
                request_id.clone(),
                share_id.clone(),
                route.share_name.clone(),
                Some(route.subdomain.clone()),
                client_metadata.country_code.clone(),
                api_user_email.clone(),
            ),
        )
        .await
        .is_none()
        {
            return simple_response(StatusCode::SERVICE_UNAVAILABLE, "share-request-cancelled");
        }
    }
    let mut recent_traffic_guard = live_request_id
        .as_ref()
        .map(|id| RecentTrafficGuard::new(state.recent_traffic.clone(), id.clone()));
    let share_proxy_started = Instant::now();
    let share_request_app = infer_share_request_app(&path);

    let upstream = match send_proxy_upstream_request(
        builder.body(body),
        Duration::from_secs(state.config.proxy_stream.response_header_timeout_secs),
        share_permit
            .as_ref()
            .map(ShareConcurrencyPermit::cancellation_token),
    )
    .await
    {
        Ok(response) => response,
        Err(ProxyUpstreamRequestError::ResponseHeaderTimeout) => {
            if let Some(guard) = recent_traffic_guard.as_mut() {
                guard.set_status(StatusCode::GATEWAY_TIMEOUT);
            }
            let owns_share_release = share_permit
                .as_ref()
                .is_none_or(ShareConcurrencyPermit::release_registry_lease);
            if owns_share_release {
                state.metrics.record_proxy_response_header_timeout();
                warn!(method = %method, host = %host, path = %path_and_query, backend = %backend, share_id = %log_share_id, "proxy upstream response headers timed out");
            }
            let _share_llm_metrics_guard = share_llm_proxy_metrics_guard(
                &state,
                &route,
                &path,
                is_share_router_probe,
                skips_share_edge_auth,
                live_request_id.as_deref(),
                StatusCode::GATEWAY_TIMEOUT.as_u16(),
                Some("timeout".into()),
                share_proxy_started,
                share_request_app.clone(),
            );
            return simple_response(
                StatusCode::GATEWAY_TIMEOUT,
                "upstream-response-header-timeout",
            );
        }
        Err(ProxyUpstreamRequestError::Cancelled) => {
            if let Some(guard) = recent_traffic_guard.as_mut() {
                guard.set_status(StatusCode::SERVICE_UNAVAILABLE);
            }
            return simple_response(StatusCode::SERVICE_UNAVAILABLE, "share-request-cancelled");
        }
        Err(ProxyUpstreamRequestError::Request(err)) => {
            if let Some(guard) = recent_traffic_guard.as_mut() {
                guard.set_status(StatusCode::SERVICE_UNAVAILABLE);
            }
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
                retire_failed_client_web_probe(&state, &route).await;
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
                Some("upstream_error".into()),
                share_proxy_started,
                share_request_app.clone(),
            );
            return simple_response(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!("connection-lost: {err}"),
            );
        }
    };

    if let Some(permit) = share_permit.as_ref() {
        permit.mark_response_headers_received();
    }
    let upstream_status = upstream.status();
    let response_headers = upstream.headers().clone();
    let ingress_rejection = ingress_rejection_response(
        &state,
        upstream_status,
        &response_headers,
        &route,
        &path_and_query,
    );
    let status = ingress_rejection
        .as_ref()
        .map(Response::status)
        .unwrap_or(upstream_status);
    if let Some(guard) = recent_traffic_guard.as_mut() {
        guard.set_status(status);
    }
    state.metrics.record_proxy_status(status);
    let share_llm_metrics_guard = share_llm_proxy_metrics_guard(
        &state,
        &route,
        &path,
        is_share_router_probe,
        skips_share_edge_auth,
        live_request_id.as_deref(),
        status.as_u16(),
        llm_error_kind(status, &response_headers),
        share_proxy_started,
        share_request_app,
    );
    if is_health_check_request {
        if status.is_success() {
            state
                .proxy
                .clear_health_probe_failure(&route.subdomain)
                .await;
        } else {
            state
                .proxy
                .record_health_probe_failure(route.subdomain.clone())
                .await;
            retire_failed_client_web_probe(&state, &route).await;
        }
    }
    if let Some(response) = ingress_rejection {
        return response;
    }
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
    let body_stream = proxy_response_body_stream(
        upstream.bytes_stream(),
        is_event_stream
            .then(|| ProxyStreamProtocol::from_path(&path))
            .flatten(),
        is_event_stream,
        ProxyResponseTimeouts::from(&state.config.proxy_stream),
        state.metrics.clone(),
        ProxyResponseLifecycle {
            _route: Some(route_inflight_guard),
            share: share_permit,
            _free_share_ip: free_share_ip_permit,
            _recent_traffic: recent_traffic_guard,
            _share_llm_metrics: share_llm_metrics_guard,
            _metrics: Some(metrics_permit),
            ..Default::default()
        },
    );
    let body = Body::from_stream(body_stream);

    let mut response = Response::new(body);
    *response.status_mut() = status;
    response.headers_mut().clear();
    copy_upstream_response_headers(&response_headers, response.headers_mut());
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

async fn retire_failed_client_web_probe(state: &ServerState, route: &RouteEntry) {
    if !route.is_client_web() {
        return;
    }
    let Some(connection_id) = route.connection_id() else {
        return;
    };
    if state
        .proxy
        .remove_route_target_if_generation(&route.subdomain, connection_id, route.generation)
        .await
    {
        warn!(
            subdomain = %route.subdomain,
            installation_id = route.installation_id().unwrap_or("-"),
            connection_id,
            generation = route.generation,
            "retired client web route after backend health probe failed"
        );
    }
}

fn is_image_generation_submit_path(path: &str) -> bool {
    matches!(
        path.trim_start_matches('/'),
        "v1/images/generations" | "images/generations"
    )
}

fn proxy_request_body_limit(path_and_query: &str) -> usize {
    match path_and_query
        .split_once('?')
        .map_or(path_and_query, |(path, _)| path)
        .trim_start_matches('/')
    {
        "v1/images/generations" | "images/generations" | "v1/images/edits" | "images/edits" => {
            CODEX_IMAGES_REQUEST_BODY_LIMIT_BYTES
        }
        "v1/videos/generations" | "videos/generations" => MEDIA_REQUEST_BODY_LIMIT_BYTES,
        _ => DEFAULT_PROXY_REQUEST_BODY_LIMIT_BYTES,
    }
}

#[derive(Debug)]
enum ProxyRequestBodyReadError {
    Timeout,
    Rejected(axum::Error),
}

async fn read_proxy_request_body(
    body: Body,
    limit: usize,
    timeout: Duration,
) -> Result<Bytes, ProxyRequestBodyReadError> {
    match tokio::time::timeout(timeout, axum::body::to_bytes(body, limit)).await {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(error)) => Err(ProxyRequestBodyReadError::Rejected(error)),
        Err(_) => Err(ProxyRequestBodyReadError::Timeout),
    }
}

async fn await_share_request_or_cancel<F>(
    cancellation: &CancellationToken,
    future: F,
) -> Option<F::Output>
where
    F: Future,
{
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => None,
        output = future => Some(output),
    }
}

#[derive(Debug)]
enum ProxyUpstreamRequestError {
    ResponseHeaderTimeout,
    Cancelled,
    Request(reqwest::Error),
}

#[derive(Debug)]
enum ProxyUpstreamBodyReadError {
    Timeout,
    Cancelled,
    TooLarge,
    Request(reqwest::Error),
}

async fn send_proxy_upstream_request(
    request: reqwest::RequestBuilder,
    timeout: Duration,
    cancellation: Option<CancellationToken>,
) -> Result<reqwest::Response, ProxyUpstreamRequestError> {
    let cancellation = cancellation.unwrap_or_default();
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(ProxyUpstreamRequestError::Cancelled),
        result = tokio::time::timeout(timeout, request.send()) => match result {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => Err(ProxyUpstreamRequestError::Request(error)),
            Err(_) => Err(ProxyUpstreamRequestError::ResponseHeaderTimeout),
        },
    }
}

async fn read_proxy_upstream_body(
    response: reqwest::Response,
    limit: usize,
    timeout: Duration,
    cancellation: CancellationToken,
) -> Result<Bytes, ProxyUpstreamBodyReadError> {
    let read = async move {
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(ProxyUpstreamBodyReadError::Request)?;
            if body.len().saturating_add(chunk.len()) > limit {
                return Err(ProxyUpstreamBodyReadError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Bytes::from(body))
    };
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(ProxyUpstreamBodyReadError::Cancelled),
        result = tokio::time::timeout(timeout, read) => match result {
            Ok(result) => result,
            Err(_) => Err(ProxyUpstreamBodyReadError::Timeout),
        },
    }
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
    let admission_request_id = Uuid::new_v4().to_string();
    let share_permit = match state
        .proxy
        .try_acquire_share_permit(
            &admission_request_id,
            share_id,
            Some("codex"),
            route.parallel_limit,
            api_user_email.as_deref(),
        )
        .await
    {
        Ok(permit) => permit,
        Err(exceeded) => {
            record_llm_admission_rejection(
                state,
                route,
                &admission_request_id,
                "codex",
                "direct",
                None,
            );
            return llm_concurrency_response(
                "codex",
                "cc_switch_share_concurrency_limit_exceeded",
                "share",
                exceeded.current,
                exceeded.limit,
                &admission_request_id,
                format!(
                    "Share concurrency limit has been reached ({}/{}). Wait for an in-flight request to finish.",
                    exceeded.current, exceeded.limit
                ),
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
            Ok(permit) => Some(permit),
            Err(exceeded) => {
                record_llm_admission_rejection(
                    state,
                    route,
                    &admission_request_id,
                    "codex",
                    "direct",
                    None,
                );
                return llm_concurrency_response(
                    "codex",
                    "cc_switch_free_share_ip_concurrency_limit_exceeded",
                    "free_share_ip",
                    exceeded.current,
                    exceeded.limit,
                    &admission_request_id,
                    format!(
                        "Free Share IP concurrency limit has been reached ({}/{}). Wait for an in-flight request to finish.",
                        exceeded.current, exceeded.limit
                    ),
                );
            }
        }
    } else {
        None
    };
    if let Some(permit) = free_share_ip_permit.as_ref() {
        share_permit.register_keyed_permit(permit);
    }
    let image_permit = match state
        .proxy
        .try_acquire_image_permit(share_id, IMAGE_JOB_MAX_RUNNING_PER_SHARE as i64)
        .await
    {
        Ok(permit) => permit,
        Err(exceeded) => {
            record_llm_admission_rejection(
                state,
                route,
                &admission_request_id,
                "codex",
                "direct",
                None,
            );
            return llm_concurrency_response(
                "codex",
                "cc_switch_image_concurrency_limit_exceeded",
                "image",
                exceeded.current,
                exceeded.limit,
                &admission_request_id,
                format!(
                    "Image generation concurrency limit has been reached ({}/{}). Wait for the active image request to finish.",
                    exceeded.current, exceeded.limit
                ),
            );
        }
    };
    share_permit.register_keyed_permit(&image_permit);
    let share_cancellation = share_permit.cancellation_token();

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
    let request_id = admission_request_id;
    let Some(signed_builder) = await_share_request_or_cancel(
        &share_cancellation,
        with_signed_ingress_context(
            state,
            builder,
            route,
            format!("{}.{}", route.subdomain, state.config.tunnel_domain),
            &request_id,
            api_user_email.clone(),
            Some(user_country.clone()),
            &axum::http::Method::POST,
            &upstream_body,
        ),
    )
    .await
    else {
        return json_error_response(StatusCode::SERVICE_UNAVAILABLE, "share-request-cancelled");
    };
    builder = match signed_builder {
        Ok(builder) => builder,
        Err(response) => return response,
    };
    if await_share_request_or_cancel(
        &share_cancellation,
        state.recent_traffic.record_with_id(
            request_id.clone(),
            share_id.to_string(),
            route.share_name.clone(),
            Some(route.subdomain.clone()),
            Some(user_country.clone()),
            api_user_email.clone(),
        ),
    )
    .await
    .is_none()
    {
        return json_error_response(StatusCode::SERVICE_UNAVAILABLE, "share-request-cancelled");
    }
    let mut recent_traffic_guard = Some(RecentTrafficGuard::new(
        state.recent_traffic.clone(),
        request_id.clone(),
    ));

    let request_started = Instant::now();
    let upstream = match send_proxy_upstream_request(
        builder.body(upstream_body),
        Duration::from_secs(state.config.proxy_stream.response_header_timeout_secs),
        Some(share_permit.cancellation_token()),
    )
    .await
    {
        Ok(response) => response,
        Err(ProxyUpstreamRequestError::ResponseHeaderTimeout) => {
            if let Some(guard) = recent_traffic_guard.as_mut() {
                guard.set_status(StatusCode::GATEWAY_TIMEOUT);
            }
            if share_permit.release_registry_lease() {
                state.metrics.record_proxy_response_header_timeout();
            }
            let message = "upstream response headers timed out";
            if let Err(log_err) = record_image_stream_log(
                &state.store,
                &state.config,
                &log_meta,
                ImageStreamLogOutcome {
                    status: "failed",
                    status_code: Some(StatusCode::GATEWAY_TIMEOUT.as_u16()),
                    latency_ms: request_started.elapsed().as_millis() as u64,
                    completed_at: Some(chrono::Utc::now().timestamp()),
                    error_message: Some(message.into()),
                    result_mime_type: None,
                    result_size_bytes: None,
                    result_storage_key: None,
                    result_access_token: None,
                },
            )
            .await
            {
                warn!(request_id = %log_meta.request_id, error = %log_err, "record image stream header timeout log failed");
            }
            return json_error_response(StatusCode::GATEWAY_TIMEOUT, message);
        }
        Err(ProxyUpstreamRequestError::Cancelled) => {
            if let Some(guard) = recent_traffic_guard.as_mut() {
                guard.set_status(StatusCode::SERVICE_UNAVAILABLE);
            }
            let message = "image request cancelled while waiting for response headers";
            record_image_stream_failure(
                &state.store,
                &state.config,
                &log_meta,
                request_started,
                StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                message.into(),
            )
            .await;
            return json_error_response(StatusCode::SERVICE_UNAVAILABLE, message);
        }
        Err(ProxyUpstreamRequestError::Request(err)) => {
            if let Some(guard) = recent_traffic_guard.as_mut() {
                guard.set_status(StatusCode::SERVICE_UNAVAILABLE);
            }
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
    share_permit.mark_response_headers_received();
    let status = upstream.status();
    if let Some(guard) = recent_traffic_guard.as_mut() {
        guard.set_status(status);
    }
    let response_headers = upstream.headers().clone();
    if let Some(response) = ingress_rejection_response(
        state,
        status,
        &response_headers,
        route,
        "/v1/images/generations",
    ) {
        let mapped_status = response.status();
        state.metrics.record_proxy_status(mapped_status);
        let error_message = header_str(response.headers(), "x-share-router-error-reason");
        if let Err(err) = record_image_stream_log(
            &state.store,
            &state.config,
            &log_meta,
            ImageStreamLogOutcome {
                status: "failed",
                status_code: Some(mapped_status.as_u16()),
                latency_ms: request_started.elapsed().as_millis() as u64,
                completed_at: Some(chrono::Utc::now().timestamp()),
                error_message: Some(error_message.to_string()),
                result_mime_type: None,
                result_size_bytes: None,
                result_storage_key: None,
                result_access_token: None,
            },
        )
        .await
        {
            warn!(request_id = %log_meta.request_id, error = %err, "record image stream ingress rejection failed");
        }
        return response;
    }
    state.metrics.record_proxy_status(status);
    if llm_error_kind(status, &response_headers).as_deref() == Some("concurrency_limited") {
        record_llm_admission_rejection(state, route, &request_id, "codex", "direct", None);
    }
    if !status.is_success() {
        let status_code = status;
        let (response_body, error_message) = match read_proxy_upstream_body(
            upstream,
            MAX_PROXY_ERROR_RESPONSE_BODY_BYTES,
            Duration::from_secs(state.config.proxy_stream.first_event_timeout_secs),
            share_permit.cancellation_token(),
        )
        .await
        {
            Ok(body) => {
                let message = compact_prompt_preview(&String::from_utf8_lossy(&body), 1000);
                (Some(body), message)
            }
            Err(ProxyUpstreamBodyReadError::Timeout) => {
                if share_permit.release_registry_lease() {
                    state.metrics.record_proxy_stream_first_event_timeout();
                }
                (None, "upstream error response body timed out".into())
            }
            Err(ProxyUpstreamBodyReadError::Cancelled) => {
                (None, "upstream error response body was cancelled".into())
            }
            Err(ProxyUpstreamBodyReadError::TooLarge) => (
                None,
                format!(
                    "upstream error response body exceeded {} bytes",
                    MAX_PROXY_ERROR_RESPONSE_BODY_BYTES
                ),
            ),
            Err(ProxyUpstreamBodyReadError::Request(error)) => (
                None,
                format!("failed to read upstream error response: {error}"),
            ),
        };
        if let Err(err) = record_image_stream_log(
            &state.store,
            &state.config,
            &log_meta,
            ImageStreamLogOutcome {
                status: "failed",
                status_code: Some(status_code.as_u16()),
                latency_ms: request_started.elapsed().as_millis() as u64,
                completed_at: Some(chrono::Utc::now().timestamp()),
                error_message: Some(error_message.clone()),
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
        return match response_body {
            Some(body) => buffered_upstream_response(status_code, &response_headers, body),
            None => json_error_response(StatusCode::BAD_GATEWAY, &error_message),
        };
    }

    let stream = image_response_body_stream(
        upstream.bytes_stream(),
        output_format,
        state.store.clone(),
        state.config.clone(),
        state.metrics.clone(),
        ProxyResponseTimeouts::from(&state.config.proxy_stream),
        ProxyResponseLifecycle {
            _route: Some(route_inflight_guard),
            share: Some(share_permit),
            _free_share_ip: free_share_ip_permit,
            _image: Some(image_permit),
            _recent_traffic: recent_traffic_guard,
            _metrics: Some(metrics_permit),
            ..Default::default()
        },
        log_meta,
        request_started,
        status.as_u16(),
    );
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

#[allow(clippy::too_many_arguments)]
fn image_response_body_stream<S, E>(
    upstream_stream: S,
    output_format: String,
    log_store: AppStore,
    result_config: Config,
    stream_metrics: Arc<MetricsRegistry>,
    timeouts: ProxyResponseTimeouts,
    lifecycle: ProxyResponseLifecycle,
    log_meta: ImageStreamLogMeta,
    request_started: Instant,
    status_code: u16,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    let (sender, mut receiver) = mpsc::channel(PROXY_RESPONSE_CHANNEL_CAPACITY);
    lifecycle.mark_response_headers_received();
    let cancellation = lifecycle.cancellation_token();
    let hard_deadline = lifecycle.hard_deadline(timeouts.max_lifetime);
    tokio::spawn(async move {
        let mut upstream_stream = Box::pin(upstream_stream);
        let mut lifecycle = Some(lifecycle);
        let mut parser = ImageStreamSseParser::default();
        let mut meaningful_progress_seen = false;
        let mut progress_deadline = tokio::time::Instant::now() + timeouts.first_event;
        let completion_guard = ImageStreamCompletionGuard::new(
            log_store.clone(),
            result_config.clone(),
            log_meta.clone(),
            request_started,
            status_code,
        );
        let mut keepalive = tokio::time::interval(Duration::from_secs(15));
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => break,
                _ = sender.closed() => break,
                _ = tokio::time::sleep_until(hard_deadline) => {
                    let owns_share_release =
                        finish_proxy_response_lifecycle(&mut lifecycle);
                    if owns_share_release {
                        stream_metrics.record_proxy_request_hard_timeout();
                    }
                    let message = format!(
                        "image stream request hard lifetime exceeded after {} seconds",
                        timeouts.max_lifetime.as_secs()
                    );
                    record_image_stream_failure(
                        &log_store,
                        &result_config,
                        &log_meta,
                        request_started,
                        StatusCode::GATEWAY_TIMEOUT.as_u16(),
                        message.clone(),
                    )
                    .await;
                    completion_guard.mark_terminal();
                    let _ = sender.try_send(Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        message,
                    )));
                    break;
                }
                _ = tokio::time::sleep_until(progress_deadline) => {
                    let owns_share_release =
                        finish_proxy_response_lifecycle(&mut lifecycle);
                    let message = if meaningful_progress_seen {
                        if owns_share_release {
                            stream_metrics.record_proxy_stream_idle_timeout();
                        }
                        format!(
                            "image stream business idle timeout after {} seconds",
                            timeouts.idle.as_secs()
                        )
                    } else {
                        if owns_share_release {
                            stream_metrics.record_proxy_stream_first_event_timeout();
                        }
                        format!(
                            "image stream first event timeout after {} seconds",
                            timeouts.first_event.as_secs()
                        )
                    };
                    record_image_stream_failure(
                        &log_store,
                        &result_config,
                        &log_meta,
                        request_started,
                        StatusCode::GATEWAY_TIMEOUT.as_u16(),
                        message.clone(),
                    )
                    .await;
                    completion_guard.mark_terminal();
                    warn!(
                        request_id = %log_meta.request_id,
                        meaningful_progress_seen,
                        "image stream closed after protocol progress timeout"
                    );
                    let _ = sender.try_send(Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        message,
                    )));
                    break;
                }
                chunk = upstream_stream.next() => {
                    match chunk {
                        Some(Ok(bytes)) => {
                            let observation = match parser.feed(&bytes, &output_format) {
                                Ok(observation) => observation,
                                Err(ImageStreamParseError::EventTooLarge) => {
                                    stream_metrics.record_proxy_stream_parser_overflow();
                                    let message = format!(
                                        "image stream protocol event exceeded the {} byte limit",
                                        MAX_PROXY_IMAGE_STREAM_EVENT_BYTES
                                    );
                                    drop(lifecycle.take());
                                    record_image_stream_failure(
                                        &log_store,
                                        &result_config,
                                        &log_meta,
                                        request_started,
                                        StatusCode::BAD_GATEWAY.as_u16(),
                                        message.clone(),
                                    )
                                    .await;
                                    completion_guard.mark_terminal();
                                    let _ = sender.try_send(Err(std::io::Error::new(
                                        std::io::ErrorKind::InvalidData,
                                        message,
                                    )));
                                    break;
                                }
                            };
                            if observation.meaningful_progress {
                                meaningful_progress_seen = true;
                                progress_deadline = tokio::time::Instant::now() + timeouts.idle;
                                if let Some(lifecycle) = lifecycle.as_ref() {
                                    lifecycle.record_progress();
                                }
                            }
                            if let Some(terminal) = observation.terminal {
                                stream_metrics.record_proxy_stream_semantic_terminal();
                                drop(lifecycle.take());
                                let chunk_end = terminal.chunk_end.min(bytes.len());
                                let event = terminal.event;
                                let mut result_storage_key = None;
                                let mut result_access_token = None;
                                if event.status == "succeeded"
                                    && let (Some(image_bytes), Some(ext)) =
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
                                        Err(error) => warn!(
                                            request_id = %log_meta.request_id,
                                            error = %error,
                                            "write image result file failed"
                                        ),
                                    }
                                }
                                if let Err(error) = record_image_stream_log(
                                    &log_store,
                                    &result_config,
                                    &log_meta,
                                    ImageStreamLogOutcome {
                                        status: event.status,
                                        status_code: Some(status_code),
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
                                    warn!(request_id = %log_meta.request_id, error = %error, "record image stream terminal log failed");
                                }
                                completion_guard.mark_terminal();
                                if chunk_end > 0
                                    && let Err(error) = send_proxy_response_chunk(
                                        &sender,
                                        Ok(bytes.slice(..chunk_end)),
                                        &cancellation,
                                        timeouts.downstream_stall,
                                        hard_deadline,
                                    )
                                    .await
                                {
                                    let owns_share_release =
                                        finish_proxy_response_lifecycle(&mut lifecycle);
                                    record_proxy_chunk_send_failure(
                                        error,
                                        &stream_metrics,
                                        timeouts.downstream_stall,
                                        timeouts.max_lifetime,
                                        owns_share_release,
                                    );
                                }
                                break;
                            }
                            if let Err(error) = send_proxy_response_chunk(
                                &sender,
                                Ok(bytes),
                                &cancellation,
                                timeouts.downstream_stall,
                                hard_deadline,
                            )
                            .await
                            {
                                let owns_share_release =
                                    finish_proxy_response_lifecycle(&mut lifecycle);
                                record_proxy_chunk_send_failure(
                                    error,
                                    &stream_metrics,
                                    timeouts.downstream_stall,
                                    timeouts.max_lifetime,
                                    owns_share_release,
                                );
                                if matches!(
                                    error,
                                    ProxyChunkSendError::DownstreamStalled
                                        | ProxyChunkSendError::HardLifetime
                                ) {
                                    let message = match error {
                                        ProxyChunkSendError::DownstreamStalled => {
                                            "image response downstream stalled".to_string()
                                        }
                                        _ => "image request hard lifetime exceeded".to_string(),
                                    };
                                    record_image_stream_failure(
                                        &log_store,
                                        &result_config,
                                        &log_meta,
                                        request_started,
                                        StatusCode::GATEWAY_TIMEOUT.as_u16(),
                                        message,
                                    )
                                    .await;
                                    completion_guard.mark_terminal();
                                }
                                break;
                            }
                        }
                        Some(Err(error)) => {
                            stream_metrics.record_proxy_stream_upstream_error();
                            drop(lifecycle.take());
                            let message = format!("read upstream stream failed: {error}");
                            record_image_stream_failure(
                                &log_store,
                                &result_config,
                                &log_meta,
                                request_started,
                                status_code,
                                message.clone(),
                            )
                            .await;
                            completion_guard.mark_terminal();
                            let _ = send_proxy_response_chunk(
                                &sender,
                                Err(std::io::Error::other(message)),
                                &cancellation,
                                timeouts.downstream_stall,
                                hard_deadline,
                            )
                            .await;
                            break;
                        }
                        None => {
                            drop(lifecycle.take());
                            record_image_stream_failure(
                                &log_store,
                                &result_config,
                                &log_meta,
                                request_started,
                                status_code,
                                "stream ended before image_generation.completed".into(),
                            )
                            .await;
                            completion_guard.mark_terminal();
                            break;
                        }
                    }
                }
                _ = keepalive.tick() => {
                    if let Err(error) = send_proxy_response_chunk(
                        &sender,
                        Ok(Bytes::from_static(b": keepalive\n\n")),
                        &cancellation,
                        timeouts.downstream_stall,
                        hard_deadline,
                    )
                    .await
                    {
                        let owns_share_release =
                            finish_proxy_response_lifecycle(&mut lifecycle);
                        record_proxy_chunk_send_failure(
                            error,
                            &stream_metrics,
                            timeouts.downstream_stall,
                            timeouts.max_lifetime,
                            owns_share_release,
                        );
                        break;
                    }
                }
            }
        }
    });

    async_stream::stream! {
        while let Some(chunk) = receiver.recv().await {
            yield chunk;
        }
    }
}

async fn record_image_stream_failure(
    store: &AppStore,
    config: &Config,
    meta: &ImageStreamLogMeta,
    started: Instant,
    status_code: u16,
    message: String,
) {
    if let Err(error) = record_image_stream_log(
        store,
        config,
        meta,
        ImageStreamLogOutcome {
            status: "failed",
            status_code: Some(status_code),
            latency_ms: started.elapsed().as_millis() as u64,
            completed_at: Some(chrono::Utc::now().timestamp()),
            error_message: Some(message),
            result_mime_type: None,
            result_size_bytes: None,
            result_storage_key: None,
            result_access_token: None,
        },
    )
    .await
    {
        warn!(request_id = %meta.request_id, error = %error, "record image stream failure log failed");
    }
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

#[derive(Debug)]
struct ImageStreamTerminal {
    event: ImageStreamTerminalEvent,
    chunk_end: usize,
}

#[derive(Debug, Default)]
struct ImageStreamObservation {
    meaningful_progress: bool,
    terminal: Option<ImageStreamTerminal>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageStreamParseError {
    EventTooLarge,
}

#[derive(Default)]
struct ImageStreamSseParser {
    buffer: Vec<u8>,
    scan_from: usize,
}

impl ImageStreamSseParser {
    fn feed(
        &mut self,
        bytes: &[u8],
        output_format: &str,
    ) -> Result<ImageStreamObservation, ImageStreamParseError> {
        let previous_buffer_len = self.buffer.len();
        self.buffer.extend_from_slice(bytes);
        let mut consumed = 0usize;
        let mut observation = ImageStreamObservation::default();
        while let Some((index, separator_len)) = find_sse_separator(&self.buffer, self.scan_from) {
            if index > MAX_PROXY_IMAGE_STREAM_EVENT_BYTES {
                return Err(ImageStreamParseError::EventTooLarge);
            }
            let boundary_end = index + separator_len;
            let block = self.buffer[..index].to_vec();
            self.buffer.drain(..boundary_end);
            self.scan_from = 0;
            observation.meaningful_progress |= image_stream_sse_block_is_progress(&block);
            if let Some(event) = parse_image_stream_sse_block(&block, output_format) {
                let chunk_end = consumed
                    .saturating_add(boundary_end)
                    .saturating_sub(previous_buffer_len)
                    .min(bytes.len());
                self.buffer.clear();
                self.scan_from = 0;
                observation.terminal = Some(ImageStreamTerminal { event, chunk_end });
                return Ok(observation);
            }
            consumed = consumed.saturating_add(boundary_end);
        }
        if self.buffer.len() > MAX_PROXY_IMAGE_STREAM_EVENT_BYTES {
            return Err(ImageStreamParseError::EventTooLarge);
        }
        self.scan_from = self.buffer.len().saturating_sub(3);
        Ok(observation)
    }
}

fn image_stream_sse_block_is_progress(block: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(block) else {
        return block.iter().any(|byte| !byte.is_ascii_whitespace());
    };
    let mut event_name = None;
    let mut data = Vec::new();
    let mut has_other_field = false;
    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(value) = line.strip_prefix("event:") {
            event_name = Some(value.trim());
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.trim_start());
        } else {
            has_other_field = true;
        }
    }
    if event_name.is_some_and(|value| {
        value.eq_ignore_ascii_case("ping") || value.eq_ignore_ascii_case("keepalive")
    }) {
        return false;
    }
    let payload = data.join("\n");
    let payload = payload.trim();
    if payload.eq_ignore_ascii_case("ping") || payload.eq_ignore_ascii_case("keepalive") {
        return false;
    }
    let payload_is_keepalive = serde_json::from_str::<Value>(payload)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .or_else(|| value.get("event"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .is_some_and(|value| {
            value.eq_ignore_ascii_case("ping") || value.eq_ignore_ascii_case("keepalive")
        });
    !payload_is_keepalive && (!payload.is_empty() || event_name.is_some() || has_other_field)
}

fn find_sse_separator(buffer: &[u8], scan_from: usize) -> Option<(usize, usize)> {
    for index in scan_from.min(buffer.len())..buffer.len() {
        if buffer.get(index..index + 2) == Some(b"\n\n") {
            return Some((index, 2));
        }
        if buffer.get(index..index + 4) == Some(b"\r\n\r\n") {
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
    let payload_type = value
        .as_ref()
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let is_failed = matches!(
        event_name,
        "image_generation.failed"
            | "image_generation.cancelled"
            | "image_generation.canceled"
            | "image_edit.failed"
            | "image_edit.cancelled"
            | "image_edit.canceled"
            | "error"
    ) || matches!(
        payload_type.as_deref(),
        Some(
            "image_generation.failed"
                | "image_generation.cancelled"
                | "image_generation.canceled"
                | "image_edit.failed"
                | "image_edit.cancelled"
                | "image_edit.canceled"
                | "error"
        )
    );
    if is_failed {
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
    let value = value?;
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
    let is_completed = matches!(
        event_name,
        "image_generation.completed" | "image_edit.completed"
    ) || matches!(
        payload_type.as_deref(),
        Some("image_generation.completed" | "image_edit.completed")
    );
    if !is_completed {
        return None;
    }
    let b64 = value
        .get("data")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("b64_json"))
        .and_then(Value::as_str)
        .or_else(|| value.get("b64_json").and_then(Value::as_str));
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
    Some(ImageStreamTerminalEvent {
        status: "failed",
        error_message: Some(format!("{event_name} did not contain b64_json image data")),
        result_mime_type: None,
        result_size_bytes: None,
        result_ext: None,
        image_bytes: None,
    })
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

fn infer_share_request_app(path: &str) -> Option<String> {
    let path = path
        .split('?')
        .next()
        .unwrap_or(path)
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_ascii_lowercase();
    if path.starts_with("gemini/")
        || path.starts_with("v1beta/")
        || (path.starts_with("v1/models/")
            && (path.contains(":generatecontent") || path.contains(":streamgeneratecontent")))
    {
        return Some("gemini".to_string());
    }
    if path.starts_with("anthropic/")
        || path.starts_with("claude/")
        || path.starts_with("v1/messages")
    {
        return Some("claude".to_string());
    }
    if path.starts_with("codex/")
        || path.starts_with("openai/")
        || path.starts_with("backend-api/codex/")
        || path.starts_with("v1/chat/")
        || path.starts_with("v1/v1/chat/")
        || path.starts_with("v1/responses")
        || path.starts_with("v1/v1/responses")
        || path.starts_with("v1/images/generations")
        || path.starts_with("images/generations")
        || path == "responses"
        || path.starts_with("responses/")
        || path.starts_with("chat/")
    {
        return Some("codex".to_string());
    }
    None
}

fn llm_concurrency_response(
    app: &str,
    code: &'static str,
    scope: &'static str,
    current: usize,
    limit: usize,
    request_id: &str,
    message: String,
) -> Response {
    let surface = InferenceSurface::from_app(app);
    let body = match surface {
        InferenceSurface::OpenAi => serde_json::json!({
            "error": {
                "message": message,
                "type": "concurrency_limit_error",
                "code": code,
                "param": Value::Null,
                "details": {
                    "retryable": false,
                    "scope": scope,
                    "current": current,
                    "limit": limit,
                },
            },
            "request_id": request_id,
        }),
        InferenceSurface::Anthropic => serde_json::json!({
            "type": "error",
            "error": {
                "type": "concurrency_limit_error",
                "message": message,
                "code": code,
                "details": {
                    "retryable": false,
                    "scope": scope,
                    "current": current,
                    "limit": limit,
                },
            },
            "request_id": request_id,
        }),
        InferenceSurface::Gemini => serde_json::json!({
            "error": {
                "code": StatusCode::CONFLICT.as_u16(),
                "message": message,
                "status": "ABORTED",
                "details": [{
                    "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                    "reason": code.to_ascii_uppercase(),
                    "domain": "cc-switch",
                    "metadata": {
                        "code": code,
                        "retryable": "false",
                        "scope": scope,
                        "current": current.to_string(),
                        "limit": limit.to_string(),
                        "requestId": request_id,
                    },
                }],
            }
        }),
    };
    let mut response = json_response(StatusCode::CONFLICT, body);
    response
        .headers_mut()
        .insert("x-share-router-error", HeaderValue::from_static("true"));
    response.headers_mut().insert(
        "x-share-router-error-reason",
        HeaderValue::from_static(code),
    );
    response
        .headers_mut()
        .insert("x-cc-switch-error-code", HeaderValue::from_static(code));
    response
        .headers_mut()
        .insert("x-cc-switch-error-scope", HeaderValue::from_static(scope));
    if let Ok(value) = HeaderValue::from_str(request_id) {
        response
            .headers_mut()
            .insert("x-cc-switch-request-id", value.clone());
        response.headers_mut().insert("x-request-id", value);
    }
    if surface == InferenceSurface::Anthropic {
        response
            .headers_mut()
            .insert("x-should-retry", HeaderValue::from_static("false"));
    }
    response
}

fn record_llm_admission_rejection(
    state: &ServerState,
    route: &RouteEntry,
    request_id: &str,
    app_type: &str,
    route_type: &str,
    market_email: Option<&str>,
) {
    state.metrics.record_llm_request(LlmRequestMetric {
        timestamp: chrono::Utc::now().timestamp(),
        request_id: Some(request_id.to_string()),
        route_type: route_type.to_string(),
        market_email: market_email.map(str::to_string),
        share_id: route.share_id.clone(),
        subdomain: Some(route.subdomain.clone()),
        app_type: Some(app_type.to_string()),
        provider: None,
        requested_model: None,
        actual_model: None,
        status: "error".into(),
        error_kind: Some("concurrency_limited".into()),
        http_status: Some(StatusCode::CONFLICT.as_u16()),
        latency_ms: Some(0),
        ttft_ms: None,
        stream_started: false,
        stream_completed: false,
        usage_state: None,
        stream_status: None,
        usage_revision: 0,
        input_tokens: None,
        output_tokens: None,
        total_tokens: None,
        cache_read_tokens: None,
        cache_write_tokens: None,
        reasoning_tokens: None,
        estimated_cost_usd: None,
    });
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

fn ingress_rejection_response(
    state: &ServerState,
    upstream_status: StatusCode,
    upstream_headers: &HeaderMap,
    route: &RouteEntry,
    path: &str,
) -> Option<Response> {
    if upstream_status != StatusCode::UNAUTHORIZED {
        return None;
    }
    let reason = upstream_headers
        .get(crate::ingress_context::INTERNAL_INGRESS_ERROR_HEADER)?
        .to_str()
        .ok()?
        .trim();
    if reason.is_empty() {
        return None;
    }
    let reason = reason.chars().take(64).collect::<String>();
    let age_ms = internal_i64_header(
        upstream_headers,
        crate::ingress_context::INTERNAL_INGRESS_AGE_MS_HEADER,
    );
    let server_time_ms = internal_i64_header(
        upstream_headers,
        crate::ingress_context::INTERNAL_INGRESS_SERVER_TIME_MS_HEADER,
    );
    let is_freshness = matches!(reason.as_str(), "expired" | "future_timestamp");
    if state.clock_health.record_ingress_rejection(&reason) {
        warn!(
            ingress_error = %reason,
            ingress_age_ms = ?age_ms,
            ingress_server_time_ms = ?server_time_ms,
            installation_id = route.installation_id().unwrap_or("-"),
            share_id = route.share_id().unwrap_or("-"),
            subdomain = %route.subdomain,
            path,
            "client rejected Router ingress context"
        );
    }

    let (status, public_reason) = if is_freshness {
        (StatusCode::SERVICE_UNAVAILABLE, "ingress-clock-skew")
    } else {
        (StatusCode::BAD_GATEWAY, "ingress-contract-rejected")
    };
    let mut response = simple_response(status, public_reason);
    if is_freshness {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("5"));
    }
    Some(response)
}

fn internal_i64_header(headers: &HeaderMap, name: &'static str) -> Option<i64> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
}

fn is_internal_upstream_response_header(name: &str) -> bool {
    name.starts_with("x-cc-switch-internal-")
}

fn copy_upstream_response_headers(source: &HeaderMap, target: &mut HeaderMap) {
    let connection_listed_headers = connection_listed_header_names(source);
    for (name, value) in source {
        if is_hop_by_hop_header(name.as_str())
            || is_internal_upstream_response_header(name.as_str())
            || connection_listed_headers.contains(name.as_str())
        {
            continue;
        }
        target.append(name.clone(), value.clone());
    }
}

fn buffered_upstream_response(status: StatusCode, headers: &HeaderMap, body: Bytes) -> Response {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    copy_upstream_response_headers(headers, response.headers_mut());
    response
}

fn reconnecting_response() -> Response {
    let mut response = simple_response(StatusCode::SERVICE_UNAVAILABLE, "tunnel-reconnecting");
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    response
}

fn outbound_request_path_and_query(builder: &reqwest::RequestBuilder) -> Result<String, String> {
    let request = builder
        .try_clone()
        .ok_or_else(|| "outbound request cannot be cloned before signing".to_string())?
        .build()
        .map_err(|error| format!("build outbound request before signing: {error}"))?;
    let url = request.url();
    let mut path_and_query = url.path().to_string();
    if let Some(query) = url.query() {
        path_and_query.push('?');
        path_and_query.push_str(query);
    }
    crate::ingress_context::normalize_path_and_query(&path_and_query)
        .ok_or_else(|| "outbound request path and query are invalid".to_string())
}

async fn with_signed_ingress_context(
    state: &ServerState,
    builder: reqwest::RequestBuilder,
    route: &RouteEntry,
    public_host: String,
    request_id: &str,
    user_email: Option<String>,
    user_country: Option<String>,
    method: &axum::http::Method,
    body: &[u8],
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
    let path_and_query = outbound_request_path_and_query(&builder).map_err(|error| {
        warn!(%error, "proxy ingress outbound request binding failed");
        simple_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "ingress-context-signing-failed",
        )
    })?;
    let signed = crate::ingress_context::sign(
        crate::ingress_context::IngressContext {
            signature_version: crate::ingress_context::SIGNATURE_VERSION,
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
            request_id: request_id.to_string(),
            user_email,
            user_role: None,
            user_country,
            method: method.as_str().to_string(),
            path_and_query,
            body_sha256: crate::ingress_context::body_sha256_hex(body),
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
    name.starts_with("x-ctl-")
        || name.starts_with("x-cc-gateway-")
        || matches!(
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
                | "x-user-email"
                | "x-user-country"
                | "x-user-country-iso3"
                | "x-share-router-health-check"
                | "x-share-router-probe"
                | crate::ingress_context::INGRESS_CONTEXT_HEADER
                | crate::ingress_context::INGRESS_SIGNATURE_HEADER
        )
}

fn should_strip_direct_proxy_internal_header(
    name: &str,
    is_internal_share_router_path: bool,
) -> bool {
    is_internal_share_context_header(name)
        && !(is_internal_share_router_path && name.starts_with("x-ctl-"))
}

fn is_sensitive_upstream_credential_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("authorization")
        || name.eq_ignore_ascii_case("x-api-key")
        || name.eq_ignore_ascii_case("x-goog-api-key")
        || name.eq_ignore_ascii_case("api-key")
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
    if !crate::ctl_client::verify_control_request_signature(
        "GET",
        path_and_query,
        &control_secret,
        &[],
        timestamp_ms,
        nonce,
        signature,
        chrono::Utc::now().timestamp_millis(),
    ) {
        return false;
    }
    match state
        .store
        .consume_control_request_nonce(route_installation_id, nonce)
        .await
    {
        Ok(()) => true,
        Err(error) => {
            warn!(
                installation_id = route_installation_id,
                error = %error,
                "rejected replayed or invalid control request nonce"
            );
            false
        }
    }
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
    installation_id: Option<&str>,
) -> Result<Option<(String, bool)>, crate::error::AppError> {
    let Some(token) = client_web_bearer_token(headers) else {
        return Ok(None);
    };
    if let Some(installation_id) = installation_id
        && state
            .store
            .installation_provision_source(installation_id)
            .await?
            .as_deref()
            == Some(crate::client_market::PROVISION_SOURCE_ROUTER_MARKET)
    {
        // Market Clients are separate trust domains. Their bearer token must
        // pass through to cc-switch-server for local password/owner OTP auth;
        // Router sessions and Router admins are never delegated into them.
        return Ok(None);
    }
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

const PROXY_RESPONSE_CHANNEL_CAPACITY: usize = 8;

#[derive(Debug, Clone, Copy)]
struct ProxyResponseTimeouts {
    first_event: Duration,
    idle: Duration,
    downstream_stall: Duration,
    max_lifetime: Duration,
}

impl From<&crate::config::ProxyStreamConfig> for ProxyResponseTimeouts {
    fn from(config: &crate::config::ProxyStreamConfig) -> Self {
        Self {
            first_event: Duration::from_secs(config.first_event_timeout_secs),
            idle: Duration::from_secs(config.idle_timeout_secs),
            downstream_stall: Duration::from_secs(config.downstream_stall_timeout_secs),
            max_lifetime: Duration::from_secs(config.max_request_lifetime_secs),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProxyChunkSendError {
    ReceiverClosed,
    Cancelled,
    DownstreamStalled,
    HardLifetime,
}

async fn send_proxy_response_chunk(
    sender: &mpsc::Sender<Result<Bytes, std::io::Error>>,
    chunk: Result<Bytes, std::io::Error>,
    cancellation: &CancellationToken,
    downstream_stall: Duration,
    hard_deadline: tokio::time::Instant,
) -> Result<(), ProxyChunkSendError> {
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(ProxyChunkSendError::Cancelled),
        _ = sender.closed() => Err(ProxyChunkSendError::ReceiverClosed),
        _ = tokio::time::sleep_until(hard_deadline) => Err(ProxyChunkSendError::HardLifetime),
        result = tokio::time::timeout(downstream_stall, sender.send(chunk)) => match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(ProxyChunkSendError::ReceiverClosed),
            Err(_) => Err(ProxyChunkSendError::DownstreamStalled),
        },
    }
}

fn record_proxy_chunk_send_failure(
    error: ProxyChunkSendError,
    metrics: &MetricsRegistry,
    downstream_stall: Duration,
    max_lifetime: Duration,
    owns_share_release: bool,
) {
    if !owns_share_release {
        return;
    }
    match error {
        ProxyChunkSendError::DownstreamStalled => {
            metrics.record_proxy_downstream_stall_timeout();
            warn!(
                timeout_secs = downstream_stall.as_secs(),
                "proxy response pump closed after downstream stalled"
            );
        }
        ProxyChunkSendError::HardLifetime => {
            metrics.record_proxy_request_hard_timeout();
            warn!(
                timeout_secs = max_lifetime.as_secs(),
                "proxy response pump closed at request hard lifetime"
            );
        }
        ProxyChunkSendError::ReceiverClosed | ProxyChunkSendError::Cancelled => {}
    }
}

fn proxy_response_body_stream<S, E>(
    upstream_stream: S,
    protocol: Option<ProxyStreamProtocol>,
    is_event_stream: bool,
    timeouts: ProxyResponseTimeouts,
    metrics: Arc<MetricsRegistry>,
    lifecycle: ProxyResponseLifecycle,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    let (sender, mut receiver) = mpsc::channel(PROXY_RESPONSE_CHANNEL_CAPACITY);
    lifecycle.mark_response_headers_received();
    let cancellation = lifecycle.cancellation_token();
    let hard_deadline = lifecycle.hard_deadline(timeouts.max_lifetime);
    tokio::spawn(async move {
        let mut lifecycle = Some(lifecycle);
        let mut upstream_stream = Box::pin(upstream_stream);
        let mut detector = protocol.map(ProxyStreamDetector::new);
        let mut progress_deadline = tokio::time::Instant::now() + timeouts.first_event;
        let mut meaningful_progress_seen = false;

        loop {
            let next = tokio::select! {
                biased;
                _ = cancellation.cancelled() => break,
                _ = sender.closed() => break,
                _ = tokio::time::sleep_until(hard_deadline) => {
                    let owns_share_release =
                        finish_proxy_response_lifecycle(&mut lifecycle);
                    if owns_share_release {
                        metrics.record_proxy_request_hard_timeout();
                        warn!(
                            timeout_secs = timeouts.max_lifetime.as_secs(),
                            "proxy response pump closed at request hard lifetime"
                        );
                    }
                    let _ = sender.try_send(Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "proxy request hard lifetime exceeded",
                    )));
                    break;
                }
                _ = tokio::time::sleep_until(progress_deadline) => {
                    let owns_share_release =
                        finish_proxy_response_lifecycle(&mut lifecycle);
                    let message = if meaningful_progress_seen {
                        if owns_share_release {
                            metrics.record_proxy_stream_idle_timeout();
                            warn!(
                                timeout_secs = timeouts.idle.as_secs(),
                                "proxy stream closed after business idle timeout"
                            );
                        }
                        "proxy stream business idle timeout"
                    } else {
                        if owns_share_release {
                            metrics.record_proxy_stream_first_event_timeout();
                            warn!(
                                timeout_secs = timeouts.first_event.as_secs(),
                                "proxy stream closed after first event timeout"
                            );
                        }
                        "proxy stream first event timeout"
                    };
                    let _ = sender.try_send(Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        message,
                    )));
                    break;
                }
                next = upstream_stream.next() => next,
            };

            match next {
                Some(Ok(bytes)) => {
                    let mut terminal_end = None;
                    let meaningful_progress = if let Some(detector) = detector.as_mut() {
                        match detector.push(&bytes) {
                            Ok(observation) => {
                                terminal_end = observation.terminal_chunk_end;
                                observation.meaningful_progress
                            }
                            Err(ProxyStreamParseError::EventTooLarge) => {
                                metrics.record_proxy_stream_parser_overflow();
                                warn!("proxy stream protocol event exceeded parser capacity");
                                drop(lifecycle.take());
                                let error = std::io::Error::new(
                                    std::io::ErrorKind::InvalidData,
                                    "proxy stream protocol event exceeded parser capacity",
                                );
                                let _ = sender.try_send(Err(error));
                                break;
                            }
                        }
                    } else {
                        !bytes.is_empty()
                    };

                    if meaningful_progress {
                        meaningful_progress_seen = true;
                        progress_deadline = tokio::time::Instant::now() + timeouts.idle;
                        if let Some(lifecycle) = lifecycle.as_ref() {
                            lifecycle.record_progress();
                        }
                    }

                    let output = terminal_end
                        .map(|end| bytes.slice(..end.min(bytes.len())))
                        .unwrap_or(bytes);
                    if terminal_end.is_some() {
                        metrics.record_proxy_stream_semantic_terminal();
                        drop(lifecycle.take());
                    }
                    if !output.is_empty()
                        && let Err(error) = send_proxy_response_chunk(
                            &sender,
                            Ok(output),
                            &cancellation,
                            timeouts.downstream_stall,
                            hard_deadline,
                        )
                        .await
                    {
                        let owns_share_release = finish_proxy_response_lifecycle(&mut lifecycle);
                        record_proxy_chunk_send_failure(
                            error,
                            &metrics,
                            timeouts.downstream_stall,
                            timeouts.max_lifetime,
                            owns_share_release,
                        );
                        break;
                    }
                    if terminal_end.is_some() {
                        break;
                    }
                }
                Some(Err(error)) => {
                    metrics.record_proxy_stream_upstream_error();
                    warn!(error = %error, "proxy upstream response stream failed");
                    drop(lifecycle.take());
                    if !is_event_stream {
                        let send_error = send_proxy_response_chunk(
                            &sender,
                            Err(std::io::Error::other(error.to_string())),
                            &cancellation,
                            timeouts.downstream_stall,
                            hard_deadline,
                        )
                        .await;
                        if let Err(error) = send_error {
                            let owns_share_release =
                                finish_proxy_response_lifecycle(&mut lifecycle);
                            record_proxy_chunk_send_failure(
                                error,
                                &metrics,
                                timeouts.downstream_stall,
                                timeouts.max_lifetime,
                                owns_share_release,
                            );
                        }
                    }
                    break;
                }
                None => break,
            }
        }
    });

    async_stream::stream! {
        while let Some(chunk) = receiver.recv().await {
            yield chunk;
        }
    }
}

fn connection_listed_header_names(headers: &HeaderMap) -> HashSet<String> {
    headers
        .get_all("connection")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    use axum::routing::any;
    use ed25519_dalek::Signer;
    use sha2::{Digest, Sha256};

    use super::*;

    fn signed_registration_request(
        signing_key: &ed25519_dalek::SigningKey,
        nonce: &str,
    ) -> crate::models::RegisterInstallationRequest {
        let public_key = base64::engine::general_purpose::STANDARD
            .encode(signing_key.verifying_key().to_bytes());
        let timestamp_ms = Utc::now().timestamp_millis();
        let platform = "test";
        let app_version = "test";
        let canonical = format!(
            "{}\nregister_installation\n{}\n{}\n{}\n{}\n{}",
            crate::namespace::PROTOCOL_EPOCH,
            public_key,
            platform,
            app_version,
            nonce,
            timestamp_ms,
        );
        let signature = base64::engine::general_purpose::STANDARD
            .encode(signing_key.sign(canonical.as_bytes()).to_bytes());
        crate::models::RegisterInstallationRequest {
            protocol_epoch: crate::namespace::PROTOCOL_EPOCH.into(),
            public_key,
            platform: platform.into(),
            app_version: app_version.into(),
            instance_nonce: nonce.into(),
            timestamp_ms,
            signature,
        }
    }

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
        let _ = std::fs::remove_file(&config.database.path);
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
        let _ = std::fs::remove_file(&config.database.path);
    }

    #[tokio::test]
    async fn signed_internal_request_logs_reach_share_backend() {
        let backend_hits = Arc::new(AtomicUsize::new(0));
        let backend_listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let backend_addr = backend_listener.local_addr().unwrap();
        let backend = axum::Router::new()
            .fallback(any(
                |State(hits): State<Arc<AtomicUsize>>, headers: HeaderMap| async move {
                    hits.fetch_add(1, AtomicOrdering::SeqCst);
                    if [
                        "x-ctl-installation-id",
                        "x-ctl-timestamp-ms",
                        "x-ctl-nonce",
                        "x-ctl-signature",
                    ]
                    .iter()
                    .all(|name| headers.contains_key(*name))
                    {
                        StatusCode::OK
                    } else {
                        StatusCode::BAD_REQUEST
                    }
                },
            ))
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
                signed_registration_request(&signing_key, "nonce-register-signed-route"),
                crate::models::ClientMetadata {
                    ip: None,
                    country_code: None,
                },
            )
            .await
            .unwrap();
        let control_secret = registered.control_secret;
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
        let _ = std::fs::remove_file(&config.database.path);
    }

    #[tokio::test]
    async fn signed_ingress_context_reuses_router_request_id() {
        let config = proxy_test_config("signed-ingress-request-id");
        let proxy = Arc::new(ProxyRegistry::default());
        let state = proxy_test_state(&config, proxy.clone());
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[13_u8; 32]);
        let registered = state
            .store
            .register_installation(
                signed_registration_request(&signing_key, "nonce-signed-ingress-request-id"),
                crate::models::ClientMetadata {
                    ip: None,
                    country_code: None,
                },
            )
            .await
            .unwrap();
        proxy
            .set_route_with_kind(
                "share-a".into(),
                "127.0.0.1:1".into(),
                RouteKind::Share,
                Some(registered.installation_id),
                None,
                Some("share-a".into()),
                Some("Share A".into()),
                false,
                -1,
                None,
            )
            .await;
        let route = proxy.route_by_share_id("share-a").await.unwrap();
        let request_id = "req_router_admission_123";
        let request = with_signed_ingress_context(
            &state,
            reqwest::Client::new().post("http://127.0.0.1:1/v1/messages"),
            &route,
            "share-a.router.test".into(),
            request_id,
            Some("owner@example.com".into()),
            Some("JP".into()),
            &axum::http::Method::POST,
            br#"{"model":"claude-sonnet-4-6"}"#,
        )
        .await
        .unwrap()
        .build()
        .unwrap();
        let encoded = request
            .headers()
            .get(crate::ingress_context::INGRESS_CONTEXT_HEADER)
            .unwrap()
            .to_str()
            .unwrap();
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .unwrap();
        let context: crate::ingress_context::IngressContext =
            serde_json::from_slice(&decoded).unwrap();

        assert_eq!(context.request_id, request_id);
        let _ = std::fs::remove_file(&config.database.path);
    }

    fn proxy_test_state(config: &Config, proxy: Arc<ProxyRegistry>) -> ServerState {
        let metrics = crate::metrics::MetricsRegistry::new(config.metrics.clone());
        let dynamic = Arc::new(RwLock::new(
            crate::dynamic_settings::DynamicSettings::from_config(config),
        ));
        let alerting = crate::alerting::AlertingService::new(
            config.metrics.db_path.clone(),
            dynamic.clone(),
            config,
        )
        .unwrap();
        let (share_edit_events, _) = tokio::sync::broadcast::channel(16);
        ServerState {
            config: config.clone(),
            server_geo: crate::ServerGeo {
                lat: None,
                lon: None,
            },
            store: AppStore::new(config).unwrap(),
            server_logs: Arc::new(crate::server_logs::ServerLogStore::disabled_for_tests(
                std::env::temp_dir().join(format!(
                    "cc-switch-router-proxy-server-logs-{}",
                    Uuid::new_v4()
                )),
            )),
            client_logs: Arc::new(crate::client_logs::ClientLogAccessLimiter::default()),
            proxy,
            proxy_http: reqwest::Client::new(),
            resend: None,
            resend_usage_cache: Arc::new(Mutex::new(None)),
            dynamic,
            ssh_host_fingerprint: None,
            provision_ssh_key_path: config.provision_ssh_private_key_path.clone(),
            provision_ssh_authorized_keys_line: String::new(),
            provision_ssh_public_key: String::new(),
            client_market_job_secrets: Arc::new(Mutex::new(
                crate::client_market::ClientMarketJobSecrets::default(),
            )),
            client_market_terminal: Arc::new(Mutex::new(
                crate::client_market_terminal::TerminalSessionManager::default(),
            )),
            client_market_actions: Arc::new(
                crate::client_market_coordination::ClientMarketActionLocks::default(),
            ),
            client_subdomain_takeover_recovery_running: Arc::new(
                std::sync::atomic::AtomicBool::new(false),
            ),
            market_billing_controls: Arc::new(Mutex::new(())),
            recent_traffic: RecentTraffic::new(),
            abuse: Arc::new(crate::abuse::AbuseTracker::new()),
            ip_blacklist_stats: Arc::new(crate::ip_blacklist_stats::IpBlacklistStats::new()),
            upgrade_registry: Arc::new(crate::admin::upgrade::UpgradeRegistry::new()),
            share_edit_events,
            env_path: std::env::temp_dir().join("cc-switch-router-proxy-test.env"),
            start_instant: Instant::now(),
            scheduling_overrides: crate::scheduling_signals::OverrideStore::new(),
            clock_health: crate::clock_health::ClockHealthService::new(config.clock_health.clone())
                .unwrap(),
            metrics,
            alerting,
            registration_admission: Arc::new(
                crate::registration_admission::RegistrationAdmissionLimiter::new(
                    crate::registration_admission::RegistrationAdmissionPolicy::default(),
                ),
            ),
        }
    }

    fn proxy_test_config(name: &str) -> Config {
        let data_dir = std::env::temp_dir();
        let db_path = data_dir.join(format!(
            "cc-switch-router-proxy-{name}-{}.db",
            Uuid::new_v4()
        ));
        Config {
            api_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            ssh_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            tunnel_domain: "router.test".into(),
            ssh_public_addr: String::new(),
            ssh_transport: crate::config::SshTransportConfig::default(),
            proxy_stream: crate::config::ProxyStreamConfig::default(),
            use_localhost: true,
            lease_ttl_secs: 60,
            data_dir,
            database: crate::config::DatabaseConfig::local(db_path),
            host_key_path: std::env::temp_dir().join(format!(
                "cc-switch-router-proxy-{name}-{}.key",
                Uuid::new_v4()
            )),
            provision_ssh_private_key_path: std::env::temp_dir().join(format!(
                "cc-switch-router-proxy-{name}-{}-id_rsa",
                Uuid::new_v4()
            )),
            provision_ssh_public_key_path: std::env::temp_dir().join(format!(
                "cc-switch-router-proxy-{name}-{}-id_rsa.pub",
                Uuid::new_v4()
            )),
            cleanup_interval_secs: 300,
            lease_retention_secs: 24 * 60 * 60,
            request_log_retention_days: 30,
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
            auth_source_hourly_limit: 15,
            ip_blacklist: String::new(),
            free_share_ip_parallel_limit: 1,
            market_usd_cny_rate_micros: crate::market_billing::DEFAULT_USD_CNY_RATE_MICROS,
            ip_intel_endpoints: Vec::new(),
            verification_service_base_url: "https://tokenswitch.org".into(),
            verification_service_api_key: None,
            router_owner_email: None,
            admin_emails: HashSet::new(),
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
                alerting: crate::config::AlertingSettings::default(),
            },
            clock_health: crate::config::ClockHealthConfig::default(),
        }
    }

    #[tokio::test]
    async fn market_client_web_does_not_delegate_router_session() {
        let config = proxy_test_config("market-client-web-auth");
        let proxy = Arc::new(ProxyRegistry::default());
        let state = proxy_test_state(&config, proxy);
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[11_u8; 32]);
        let registered = state
            .store
            .register_installation(
                signed_registration_request(&signing_key, "nonce-market-client-web-auth"),
                crate::models::ClientMetadata {
                    ip: None,
                    country_code: None,
                },
            )
            .await
            .unwrap();
        let access_token = "router-session-for-market-client";
        let access_hash = base64::engine::general_purpose::STANDARD
            .encode(Sha256::digest(access_token.as_bytes()));
        let now = Utc::now();
        {
            let conn = state.store.conn.lock().await;
            conn.execute(
                "INSERT INTO users
                    (id, email_normalized, status, created_at, last_login_at)
                 VALUES ('router-user', 'owner@example.com', 'active', ?1, ?1)",
                crate::db::params![now.to_rfc3339()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO user_sessions
                    (id, user_id, auth_source_kind, auth_source_id,
                     access_token_hash, refresh_token_hash,
                     access_expires_at, refresh_expires_at, created_at, last_used_at)
                 VALUES ('router-session', 'router-user', 'client_installation', ?1,
                         ?2, 'refresh-hash',
                         ?3, ?4, ?5, ?5)",
                crate::db::params![
                    registered.installation_id,
                    access_hash,
                    (now + chrono::Duration::hours(1)).to_rfc3339(),
                    (now + chrono::Duration::days(1)).to_rfc3339(),
                    now.to_rfc3339(),
                ],
            )
            .unwrap();
        }
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {access_token}")).unwrap(),
        );
        assert!(
            resolve_client_web_bearer(
                &state,
                &headers,
                "owner@example.com",
                "share:read",
                Some(&registered.installation_id),
            )
            .await
            .unwrap()
            .is_some()
        );
        {
            let conn = state.store.conn.lock().await;
            conn.execute(
                "UPDATE installations SET provision_source = ?2 WHERE id = ?1",
                crate::db::params![
                    registered.installation_id,
                    crate::client_market::PROVISION_SOURCE_ROUTER_MARKET,
                ],
            )
            .unwrap();
        }
        assert!(
            resolve_client_web_bearer(
                &state,
                &headers,
                "owner@example.com",
                "share:read",
                Some(&registered.installation_id),
            )
            .await
            .unwrap()
            .is_none()
        );

        let _ = std::fs::remove_file(&config.database.path);
    }

    #[test]
    fn image_generation_paths_infer_codex_app() {
        assert_eq!(
            infer_share_request_app("/v1/images/generations").as_deref(),
            Some("codex")
        );
        assert_eq!(
            infer_share_request_app("/images/generations").as_deref(),
            Some("codex")
        );
    }

    #[tokio::test]
    async fn typed_ingress_rejections_are_mapped_without_leaking_internal_headers() {
        let config = proxy_test_config("typed-ingress-rejection");
        let proxy = Arc::new(ProxyRegistry::default());
        proxy
            .set_route_with_kind(
                "share-a".into(),
                "127.0.0.1:1".into(),
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
        let route = proxy.route_by_share_id("share-a").await.unwrap();
        let state = proxy_test_state(&config, proxy);

        let mut headers = HeaderMap::new();
        headers.insert(
            crate::ingress_context::INTERNAL_INGRESS_ERROR_HEADER,
            HeaderValue::from_static("expired"),
        );
        headers.insert(
            crate::ingress_context::INTERNAL_INGRESS_AGE_MS_HEADER,
            HeaderValue::from_static("31001"),
        );
        headers.insert(
            crate::ingress_context::INTERNAL_INGRESS_SERVER_TIME_MS_HEADER,
            HeaderValue::from_static("1750000000000"),
        );
        let response = ingress_rejection_response(
            &state,
            StatusCode::UNAUTHORIZED,
            &headers,
            &route,
            "/web-api/auth/methods",
        )
        .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.headers().get(header::RETRY_AFTER),
            Some(&HeaderValue::from_static("5"))
        );
        assert!(
            response
                .headers()
                .get(crate::ingress_context::INTERNAL_INGRESS_ERROR_HEADER)
                .is_none()
        );
        assert_eq!(state.clock_health.snapshot().await.ingress_expired_total, 1);

        headers.insert(
            crate::ingress_context::INTERNAL_INGRESS_ERROR_HEADER,
            HeaderValue::from_static("invalid_signature"),
        );
        let response = ingress_rejection_response(
            &state,
            StatusCode::UNAUTHORIZED,
            &headers,
            &route,
            "/web-api/auth/methods",
        )
        .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            state
                .clock_health
                .snapshot()
                .await
                .ingress_contract_error_total,
            1
        );

        assert!(
            ingress_rejection_response(
                &state,
                StatusCode::UNAUTHORIZED,
                &HeaderMap::new(),
                &route,
                "/web-api/auth/methods",
            )
            .is_none()
        );
        assert!(is_internal_upstream_response_header(
            "x-cc-switch-internal-anything"
        ));
        assert!(!is_internal_upstream_response_header(
            "x-application-header"
        ));
        headers.insert("x-application-header", HeaderValue::from_static("kept"));
        headers.insert(
            header::CONNECTION,
            HeaderValue::from_static("x-private-hop, x-second-hop"),
        );
        headers.insert("x-private-hop", HeaderValue::from_static("removed"));
        headers.insert("x-second-hop", HeaderValue::from_static("removed"));
        let mut copied = HeaderMap::new();
        copy_upstream_response_headers(&headers, &mut copied);
        assert_eq!(
            copied.get("x-application-header"),
            Some(&HeaderValue::from_static("kept"))
        );
        assert!(
            copied
                .get(crate::ingress_context::INTERNAL_INGRESS_ERROR_HEADER)
                .is_none()
        );
        assert!(copied.get("x-private-hop").is_none());
        assert!(copied.get("x-second-hop").is_none());
        assert_eq!(
            connection_listed_header_names(&headers),
            HashSet::from(["x-private-hop".to_string(), "x-second-hop".to_string()])
        );

        let _ = std::fs::remove_file(&config.database.path);
    }

    #[test]
    fn request_app_is_inferred_only_from_protocol_path() {
        for path in [
            "/v1/messages",
            "/anthropic/v1/messages",
            "/claude/v1/messages",
        ] {
            assert_eq!(infer_share_request_app(path).as_deref(), Some("claude"));
        }
        for path in [
            "/v1/chat/completions",
            "/v1/v1/chat/completions",
            "/chat/completions",
            "/v1/responses",
            "/responses",
            "/codex/v1/responses",
            "/openai/v1/responses",
            "/backend-api/codex/responses",
        ] {
            assert_eq!(infer_share_request_app(path).as_deref(), Some("codex"));
        }
        for path in [
            "/v1beta/models/gemini-2.5-flash:generateContent",
            "/gemini/v1beta/models/gemini-2.5-flash:streamGenerateContent",
            "/v1/models/gemini-2.5-flash:generateContent",
        ] {
            assert_eq!(infer_share_request_app(path).as_deref(), Some("gemini"));
        }
        assert_eq!(infer_share_request_app("/v1/models").as_deref(), None);
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
    fn ingress_binding_uses_the_canonical_reqwest_outbound_target() {
        let builder = reqwest::Client::new()
            .post("http://127.0.0.1/prefix/../v1/messages?beta=true&model=claude%2Fsonnet");
        assert_eq!(
            outbound_request_path_and_query(&builder).unwrap(),
            "/v1/messages?beta=true&model=claude%2Fsonnet"
        );

        let empty_query = reqwest::Client::new().get("http://127.0.0.1/v1/models?");
        assert_eq!(
            outbound_request_path_and_query(&empty_query).unwrap(),
            "/v1/models?"
        );
    }

    #[test]
    fn proxy_request_body_limits_match_server_ingress_envelopes() {
        for path in [
            "/v1/images/generations",
            "/images/generations?stream=true",
            "/v1/images/edits",
            "/images/edits?mask=true",
        ] {
            assert_eq!(
                proxy_request_body_limit(path),
                CODEX_IMAGES_REQUEST_BODY_LIMIT_BYTES,
                "{path}"
            );
        }
        for path in ["/v1/videos/generations", "/videos/generations?async=true"] {
            assert_eq!(
                proxy_request_body_limit(path),
                MEDIA_REQUEST_BODY_LIMIT_BYTES,
                "{path}"
            );
        }
        assert_eq!(
            proxy_request_body_limit("/v1/messages?beta=true"),
            DEFAULT_PROXY_REQUEST_BODY_LIMIT_BYTES
        );
    }

    #[tokio::test]
    async fn concurrency_rejections_use_surface_native_bodies_and_stable_headers() {
        for (app, pointer, expected) in [
            (
                "codex",
                "/error/code",
                "cc_switch_share_concurrency_limit_exceeded",
            ),
            (
                "claude",
                "/error/code",
                "cc_switch_share_concurrency_limit_exceeded",
            ),
            ("gemini", "/error/status", "ABORTED"),
        ] {
            let response = llm_concurrency_response(
                app,
                "cc_switch_share_concurrency_limit_exceeded",
                "share",
                4,
                4,
                "request-123",
                "Share concurrency limit has been reached (4/4).".to_string(),
            );
            assert_eq!(response.status(), StatusCode::CONFLICT);
            assert_eq!(
                response
                    .headers()
                    .get("x-cc-switch-error-code")
                    .and_then(|value| value.to_str().ok()),
                Some("cc_switch_share_concurrency_limit_exceeded")
            );
            assert_eq!(
                response
                    .headers()
                    .get("x-request-id")
                    .and_then(|value| value.to_str().ok()),
                Some("request-123")
            );
            assert!(!response.headers().contains_key(header::RETRY_AFTER));
            if app == "claude" {
                assert_eq!(
                    response
                        .headers()
                        .get("x-should-retry")
                        .and_then(|value| value.to_str().ok()),
                    Some("false")
                );
            }
            let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(
                body.pointer(pointer).and_then(Value::as_str),
                Some(expected)
            );
        }
    }

    #[tokio::test]
    async fn buffered_upstream_errors_preserve_native_contract() {
        let body = Bytes::from_static(
            br#"{"error":{"message":"Account concurrency reached","code":"cc_switch_provider_account_concurrency_limit_exceeded"}}"#,
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers.insert(
            "x-cc-switch-error-code",
            HeaderValue::from_static("cc_switch_provider_account_concurrency_limit_exceeded"),
        );
        headers.insert(
            crate::ingress_context::INTERNAL_INGRESS_ERROR_HEADER,
            HeaderValue::from_static("must-not-leak"),
        );

        let response = buffered_upstream_response(StatusCode::CONFLICT, &headers, body.clone());

        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response
                .headers()
                .get("x-cc-switch-error-code")
                .and_then(|value| value.to_str().ok()),
            Some("cc_switch_provider_account_concurrency_limit_exceeded")
        );
        assert!(
            response
                .headers()
                .get(crate::ingress_context::INTERNAL_INGRESS_ERROR_HEADER)
                .is_none()
        );
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap(),
            body
        );
    }

    #[test]
    fn metric_error_kind_only_treats_stable_local_codes_as_concurrency() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-cc-switch-error-code",
            HeaderValue::from_static("cc_switch_user_concurrency_limit_exceeded"),
        );
        assert_eq!(
            llm_error_kind(StatusCode::CONFLICT, &headers).as_deref(),
            Some("concurrency_limited")
        );
        assert_eq!(
            llm_error_kind(StatusCode::CONFLICT, &HeaderMap::new()).as_deref(),
            Some("upstream_error")
        );
        assert_eq!(
            llm_error_kind(StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new()).as_deref(),
            Some("rate_limited")
        );
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

    #[test]
    fn image_stream_parser_returns_exact_terminal_chunk_boundary() {
        let first = b"event: image_generation.partial\r\ndata: {\"progress\":50}\r\n\r\nevent: image_generation.completed\r\ndata: {\"data\":[{\"b64_json\":\"iVBORw0KGgo=\"}]";
        let second = b"}\r\n\r\n: upstream-keepalive\r\n\r\n";
        let terminal_suffix = b"}\r\n\r\n";
        let mut parser = ImageStreamSseParser::default();

        let first_observation = parser.feed(first, "png").unwrap();
        assert!(first_observation.meaningful_progress);
        assert!(first_observation.terminal.is_none());
        let terminal = parser
            .feed(second, "png")
            .unwrap()
            .terminal
            .expect("terminal event");

        assert_eq!(terminal.event.status, "succeeded");
        assert_eq!(terminal.chunk_end, terminal_suffix.len());
        assert_eq!(&second[..terminal.chunk_end], terminal_suffix);
        assert!(!String::from_utf8_lossy(&second[..terminal.chunk_end]).contains("keepalive"));
    }

    #[test]
    fn image_stream_parser_detects_terminal_event_across_single_byte_chunks() {
        let event = b"event: image_generation.completed\r\ndata: {\"type\":\"image_generation.completed\",\"b64_json\":\"iVBORw0KGgo=\"}\r\n\r\n";
        let mut parser = ImageStreamSseParser::default();

        for (index, byte) in event.iter().enumerate() {
            let observation = parser.feed(std::slice::from_ref(byte), "png").unwrap();
            if index + 1 == event.len() {
                let terminal = observation.terminal.expect("terminal event");
                assert_eq!(terminal.event.status, "succeeded");
                assert_eq!(terminal.chunk_end, 1);
            } else {
                assert!(observation.terminal.is_none(), "index={index}");
            }
        }
    }

    #[test]
    fn image_stream_parser_does_not_treat_partial_image_data_as_terminal() {
        let partial = br#"event: image_generation.partial_image
data: {"type":"image_generation.partial_image","b64_json":"iVBORw0KGgo="}

"#;
        let completed = br#"event: image_generation.completed
data: {"type":"image_generation.completed","b64_json":"iVBORw0KGgo="}

"#;
        let mut parser = ImageStreamSseParser::default();

        let partial_observation = parser.feed(partial, "png").unwrap();
        assert!(partial_observation.meaningful_progress);
        assert!(partial_observation.terminal.is_none());

        let completed_observation = parser.feed(completed, "png").unwrap();
        let terminal = completed_observation.terminal.expect("completed event");
        assert_eq!(terminal.event.status, "succeeded");
        assert_eq!(terminal.chunk_end, completed.len());
    }

    #[test]
    fn image_stream_parser_ignores_protocol_keepalives_for_progress() {
        let mut parser = ImageStreamSseParser::default();
        for keepalive in [
            b": keepalive\n\n".as_slice(),
            b"event: ping\ndata: {\"type\":\"ping\"}\n\n".as_slice(),
            b"data: {\"type\":\"keepalive\"}\n\n".as_slice(),
        ] {
            let observation = parser.feed(keepalive, "png").unwrap();
            assert!(!observation.meaningful_progress);
            assert!(observation.terminal.is_none());
        }
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

        assert!(matches!(
            limiter.try_acquire("share-1", 3).await,
            Err(ConcurrencyLimitExceeded {
                current: 3,
                limit: 3,
            })
        ));

        drop(permit_1);

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
    async fn hydrated_route_transitions_from_reconnecting_to_active_and_back() {
        let registry = ProxyRegistry::default();
        registry
            .declare_known_routes(vec!["known".into(), "second".into()])
            .await;

        assert_eq!(
            registry
                .route_availability("known", Duration::from_secs(180))
                .await
                .expect("known route")
                .state,
            RouteAvailability::Reconnecting
        );
        assert_eq!(
            registry
                .route_availability("known", Duration::ZERO)
                .await
                .expect("known route after grace")
                .state,
            RouteAvailability::Offline
        );

        registry
            .set_route(
                "known".into(),
                "127.0.0.1:3000".into(),
                Some("connection-1".into()),
                Some("share-1".into()),
                None,
                false,
                5,
                None,
            )
            .await;
        assert_eq!(
            registry
                .route_availability("known", Duration::from_secs(180))
                .await
                .expect("active route")
                .state,
            RouteAvailability::Active
        );

        registry
            .remove_route_if_connection("known", "connection-1")
            .await;
        assert_eq!(
            registry
                .route_availability("known", Duration::from_secs(180))
                .await
                .expect("retained route intention")
                .state,
            RouteAvailability::Reconnecting
        );
    }

    #[tokio::test]
    async fn failed_client_web_probe_retires_matching_route_generation() {
        let registry = ProxyRegistry::default();
        registry
            .set_route_with_kind(
                "client-probe".into(),
                "127.0.0.1:3000".into(),
                RouteKind::ClientWeb,
                Some("inst-client-probe".into()),
                Some("connection-client-probe".into()),
                None,
                None,
                false,
                -1,
                None,
            )
            .await;
        let route = registry
            .backend_for_host("client-probe.example.com", "example.com")
            .await
            .expect("client web route should start active");
        assert!(route.is_client_web());

        assert!(
            registry
                .remove_route_target_if_generation(
                    route.subdomain(),
                    route.connection_id().unwrap(),
                    route.generation(),
                )
                .await
        );
        assert!(matches!(
            registry
                .route_for_host_request("client-probe.example.com", "example.com")
                .await,
            RouteLookup::Reconnecting
        ));
    }

    #[tokio::test]
    async fn reconnecting_request_waits_for_route_activation() {
        let registry = Arc::new(ProxyRegistry::default());
        registry.declare_known_route("known".into()).await;
        let waiting_registry = registry.clone();
        let waiter = tokio::spawn(async move {
            waiting_registry
                .wait_for_active_subdomain_request("known")
                .await
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        registry
            .set_route(
                "known".into(),
                "127.0.0.1:3000".into(),
                Some("connection-1".into()),
                Some("share-1".into()),
                None,
                false,
                5,
                None,
            )
            .await;

        let lookup = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("waiter wakes before timeout")
            .expect("wait task succeeds");
        match lookup {
            RouteLookup::Active(route, _) => assert_eq!(route.backend, "127.0.0.1:3000"),
            other => panic!("expected active route after reconnect, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn reconnecting_request_wakes_as_unknown_when_intent_is_removed() {
        let registry = Arc::new(ProxyRegistry::default());
        registry.declare_known_route("known".into()).await;
        let waiting_registry = registry.clone();
        let waiter = tokio::spawn(async move {
            waiting_registry
                .wait_for_active_subdomain_request("known")
                .await
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        registry.remove_route("known").await;

        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), waiter)
                .await
                .expect("waiter wakes before timeout")
                .expect("wait task succeeds"),
            RouteLookup::Unknown
        ));
    }

    #[tokio::test]
    async fn authoritative_cleanup_removes_only_the_snapshotted_connection_and_intent() {
        let registry = ProxyRegistry::default();
        registry
            .set_route(
                "known".into(),
                "127.0.0.1:3000".into(),
                Some("connection-1".into()),
                Some("share-1".into()),
                None,
                false,
                5,
                None,
            )
            .await;

        assert!(
            !registry
                .remove_route_intent_if_connection("known", "connection-2")
                .await
        );
        assert!(
            registry
                .route_availability("known", Duration::ZERO)
                .await
                .is_some()
        );

        assert!(
            registry
                .remove_route_intent_if_connection("known", "connection-1")
                .await
        );
        assert!(
            registry
                .route_availability("known", Duration::ZERO)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn authoritative_cleanup_removes_empty_intent_after_snapshotted_connection_closes() {
        let registry = ProxyRegistry::default();
        registry
            .set_route(
                "known".into(),
                "127.0.0.1:3000".into(),
                Some("connection-1".into()),
                Some("share-1".into()),
                None,
                false,
                5,
                None,
            )
            .await;
        assert!(
            registry
                .remove_route_if_connection("known", "connection-1")
                .await
        );

        assert!(
            registry
                .remove_route_intent_if_connection("known", "connection-1")
                .await
        );
        assert!(
            registry
                .route_availability("known", Duration::ZERO)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn authoritative_cleanup_preserves_replacement_candidate() {
        let registry = ProxyRegistry::default();
        registry
            .set_route(
                "known".into(),
                "127.0.0.1:3000".into(),
                Some("connection-1".into()),
                Some("share-1".into()),
                None,
                false,
                5,
                None,
            )
            .await;
        registry
            .register_candidate_with_kind(
                "known".into(),
                "127.0.0.1:3001".into(),
                RouteKind::Share,
                None,
                Some("connection-2".into()),
                Some("share-1".into()),
                None,
                false,
                5,
                None,
                2,
                "rotation-2".into(),
            )
            .await
            .expect("register replacement candidate");

        assert!(
            !registry
                .remove_route_intent_if_connection("known", "connection-1")
                .await
        );
        assert!(
            registry
                .candidate_for_activation("known", "connection-2", "rotation-2", 2)
                .await
                .is_some()
        );
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
            "x-user-email",
            "x-user-country",
            "x-user-country-iso3",
            "x-share-router-health-check",
            "x-share-router-probe",
        ] {
            assert!(is_internal_share_context_header(name), "{name}");
        }
        assert!(!is_internal_share_context_header("x-api-key"));
        assert!(!is_internal_share_context_header("anthropic-version"));
    }

    #[test]
    fn signed_control_headers_reach_only_internal_share_router_endpoints() {
        for name in [
            "x-ctl-installation-id",
            "x-ctl-timestamp-ms",
            "x-ctl-nonce",
            "x-ctl-signature",
        ] {
            assert!(should_strip_direct_proxy_internal_header(name, false));
            assert!(!should_strip_direct_proxy_internal_header(name, true));
        }
        assert!(should_strip_direct_proxy_internal_header(
            "x-cc-switch-share-id",
            true
        ));
        assert!(should_strip_direct_proxy_internal_header(
            "x-share-router-probe",
            true
        ));
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

    #[tokio::test]
    async fn protocol_terminal_ends_pending_upstream_and_releases_registry() {
        let config = proxy_test_config("terminal-pending-stream");
        let proxy = Arc::new(ProxyRegistry::default());
        let metrics = crate::metrics::MetricsRegistry::new(config.metrics.clone());
        let permit = proxy
            .try_acquire_share_permit(
                "req-terminal",
                "share-terminal",
                Some("codex"),
                1,
                Some("user@example.com"),
            )
            .await
            .unwrap();
        let terminal = Bytes::from_static(concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ready\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n"
        ).as_bytes());
        let upstream = futures_util::stream::once(async move { Ok::<_, std::io::Error>(terminal) })
            .chain(futures_util::stream::pending());
        let body = proxy_response_body_stream(
            upstream,
            Some(ProxyStreamProtocol::OpenAiResponses),
            true,
            ProxyResponseTimeouts {
                first_event: Duration::from_secs(1),
                idle: Duration::from_secs(1),
                downstream_stall: Duration::from_secs(1),
                max_lifetime: Duration::from_secs(5),
            },
            metrics.clone(),
            ProxyResponseLifecycle {
                share: Some(permit),
                _metrics: Some(metrics.proxy_request_started()),
                ..Default::default()
            },
        );

        futures_util::pin_mut!(body);
        let chunk = tokio::time::timeout(Duration::from_millis(250), body.next())
            .await
            .expect("protocol terminal must produce the terminal chunk")
            .expect("protocol terminal chunk")
            .unwrap();
        assert!(String::from_utf8_lossy(&chunk).contains("response.completed"));
        assert!(proxy.inflight_by_share().await.is_empty());
        assert!(proxy.inflight_by_share_app().await.is_empty());
        assert!(proxy.inflight_by_share_user().await.is_empty());
        assert!(
            tokio::time::timeout(Duration::from_millis(250), body.next())
                .await
                .expect("protocol terminal must end a pending upstream stream")
                .is_none()
        );
        assert_eq!(
            metrics
                .router_status(&proxy)
                .await
                .proxy_stream_semantic_terminal_total,
            1
        );
    }

    #[tokio::test]
    async fn share_registry_keeps_all_concurrency_views_consistent() {
        let proxy = ProxyRegistry::default();
        let first = proxy
            .try_acquire_share_permit("req-a", "share-a", Some("codex"), 2, Some("A@example.com"))
            .await
            .unwrap();
        let second = proxy
            .try_acquire_share_permit("req-b", "share-a", Some("claude"), 2, Some("b@example.com"))
            .await
            .unwrap();
        assert_eq!(proxy.inflight_by_share().await.get("share-a"), Some(&2));
        assert_eq!(
            proxy.inflight_by_share_app().await["share-a"].get("codex"),
            Some(&1)
        );
        assert_eq!(
            proxy.inflight_by_share_user().await["share-a"]["codex"].get("a@example.com"),
            Some(&1)
        );
        assert_eq!(
            proxy
                .try_acquire_share_permit("req-c", "share-a", Some("gemini"), 2, None)
                .await
                .unwrap_err(),
            ConcurrencyLimitExceeded {
                current: 2,
                limit: 2
            }
        );

        drop(first);
        assert_eq!(proxy.inflight_by_share().await.get("share-a"), Some(&1));
        assert!(!proxy.inflight_by_share_app().await["share-a"].contains_key("codex"));
        drop(second);
        assert!(proxy.inflight_by_share().await.is_empty());
        assert!(proxy.inflight_by_share_app().await.is_empty());
        assert!(proxy.inflight_by_share_user().await.is_empty());
    }

    #[tokio::test]
    async fn stream_keepalives_do_not_extend_first_event_timeout() {
        let config = proxy_test_config("stream-keepalive-timeout");
        let proxy = ProxyRegistry::default();
        let metrics = crate::metrics::MetricsRegistry::new(config.metrics.clone());
        let upstream = futures_util::stream::unfold((), |_| async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Some((
                Ok::<_, std::io::Error>(Bytes::from_static(b": keepalive\n\n")),
                (),
            ))
        });
        let body = proxy_response_body_stream(
            upstream,
            Some(ProxyStreamProtocol::OpenAiResponses),
            true,
            ProxyResponseTimeouts {
                first_event: Duration::from_millis(60),
                idle: Duration::from_secs(1),
                downstream_stall: Duration::from_secs(1),
                max_lifetime: Duration::from_secs(5),
            },
            metrics.clone(),
            ProxyResponseLifecycle {
                _metrics: Some(metrics.proxy_request_started()),
                ..Default::default()
            },
        );

        let chunks = tokio::time::timeout(Duration::from_millis(250), body.collect::<Vec<_>>())
            .await
            .expect("keepalive-only stream must hit the first-event deadline");
        assert!(chunks.last().is_some_and(Result::is_err));
        let status = metrics.router_status(&proxy).await;
        assert_eq!(status.proxy_inflight, 0);
        assert_eq!(status.proxy_requests_total, 1);
        assert_eq!(status.proxy_stream_first_event_timeout_total, 1);
    }

    #[tokio::test]
    async fn unpolled_response_body_still_releases_on_first_event_timeout() {
        let config = proxy_test_config("unpolled-first-event-timeout");
        let proxy = ProxyRegistry::default();
        let metrics = crate::metrics::MetricsRegistry::new(config.metrics.clone());
        let permit = proxy
            .try_acquire_share_permit(
                "req-unpolled",
                "share-unpolled",
                Some("codex"),
                1,
                Some("user@example.com"),
            )
            .await
            .unwrap();
        let body = proxy_response_body_stream(
            futures_util::stream::pending::<Result<Bytes, std::io::Error>>(),
            Some(ProxyStreamProtocol::OpenAiResponses),
            true,
            ProxyResponseTimeouts {
                first_event: Duration::from_millis(40),
                idle: Duration::from_secs(1),
                downstream_stall: Duration::from_secs(1),
                max_lifetime: Duration::from_secs(5),
            },
            metrics.clone(),
            ProxyResponseLifecycle {
                share: Some(permit),
                _metrics: Some(metrics.proxy_request_started()),
                ..Default::default()
            },
        );

        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!(proxy.inflight_by_share().await.is_empty());
        assert_eq!(
            metrics
                .router_status(&proxy)
                .await
                .proxy_stream_first_event_timeout_total,
            1
        );
        drop(body);
    }

    #[tokio::test]
    async fn unpolled_image_response_still_releases_on_first_event_timeout() {
        let config = proxy_test_config("unpolled-image-first-event-timeout");
        let proxy = Arc::new(ProxyRegistry::default());
        let state = proxy_test_state(&config, proxy.clone());
        let permit = proxy
            .try_acquire_share_permit(
                "req-image-unpolled",
                "share-image-unpolled",
                Some("codex"),
                1,
                Some("user@example.com"),
            )
            .await
            .unwrap();
        let body = image_response_body_stream(
            futures_util::stream::pending::<Result<Bytes, std::io::Error>>(),
            "png".into(),
            state.store.clone(),
            config.clone(),
            state.metrics.clone(),
            ProxyResponseTimeouts {
                first_event: Duration::from_millis(40),
                idle: Duration::from_secs(1),
                downstream_stall: Duration::from_secs(1),
                max_lifetime: Duration::from_secs(5),
            },
            ProxyResponseLifecycle {
                share: Some(permit),
                ..Default::default()
            },
            ImageStreamLogMeta {
                request_id: "req-image-unpolled".into(),
                share_id: "share-image-unpolled".into(),
                installation_id: "installation-image-unpolled".into(),
                share_name: "Image Share".into(),
                provider_id: "provider-image".into(),
                provider_name: "Image Provider".into(),
                app_type: "codex".into(),
                model: "gpt-image".into(),
                created_at: Utc::now().timestamp(),
                prompt_preview: None,
                created_by_email: Some("user@example.com".into()),
                client_ip: None,
                user_country: None,
            },
            Instant::now(),
            StatusCode::OK.as_u16(),
        );

        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!(proxy.inflight_by_share().await.is_empty());
        drop(body);
        let _ = std::fs::remove_file(&config.database.path);
        let _ = std::fs::remove_file(&config.metrics.db_path);
    }

    #[tokio::test]
    async fn downstream_backpressure_releases_unpolled_response_body() {
        let config = proxy_test_config("downstream-backpressure");
        let proxy = ProxyRegistry::default();
        let metrics = crate::metrics::MetricsRegistry::new(config.metrics.clone());
        let permit = proxy
            .try_acquire_share_permit(
                "req-backpressure",
                "share-backpressure",
                Some("codex"),
                1,
                Some("user@example.com"),
            )
            .await
            .unwrap();
        let upstream = futures_util::stream::unfold(0_u64, |sequence| async move {
            Some((
                Ok::<_, std::io::Error>(Bytes::from(format!("chunk-{sequence}"))),
                sequence + 1,
            ))
        });
        let body = proxy_response_body_stream(
            upstream,
            None,
            false,
            ProxyResponseTimeouts {
                first_event: Duration::from_secs(1),
                idle: Duration::from_secs(1),
                downstream_stall: Duration::from_millis(40),
                max_lifetime: Duration::from_secs(5),
            },
            metrics.clone(),
            ProxyResponseLifecycle {
                share: Some(permit),
                _metrics: Some(metrics.proxy_request_started()),
                ..Default::default()
            },
        );

        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(proxy.inflight_by_share().await.is_empty());
        let status = metrics.router_status(&proxy).await;
        assert_eq!(status.proxy_inflight, 0);
        assert_eq!(status.proxy_requests_total, 1);
        assert_eq!(status.proxy_downstream_stall_timeout_total, 1);
        drop(body);
    }

    #[tokio::test]
    async fn hard_lifetime_releases_even_when_business_data_keeps_arriving() {
        let config = proxy_test_config("hard-lifetime");
        let proxy = ProxyRegistry::default();
        let metrics = crate::metrics::MetricsRegistry::new(config.metrics.clone());
        let permit = proxy
            .try_acquire_share_permit("req-hard", "share-hard", Some("codex"), 1, None)
            .await
            .unwrap();
        let upstream = futures_util::stream::unfold(0_u64, |sequence| async move {
            tokio::time::sleep(Duration::from_millis(2)).await;
            Some((
                Ok::<_, std::io::Error>(Bytes::from(format!("chunk-{sequence}"))),
                sequence + 1,
            ))
        });
        let mut body = Box::pin(proxy_response_body_stream(
            upstream,
            None,
            false,
            ProxyResponseTimeouts {
                first_event: Duration::from_secs(1),
                idle: Duration::from_secs(1),
                downstream_stall: Duration::from_secs(1),
                max_lifetime: Duration::from_millis(50),
            },
            metrics.clone(),
            ProxyResponseLifecycle {
                share: Some(permit),
                ..Default::default()
            },
        ));
        while tokio::time::timeout(Duration::from_millis(150), body.next())
            .await
            .ok()
            .flatten()
            .is_some_and(|chunk| chunk.is_ok())
        {}

        assert!(proxy.inflight_by_share().await.is_empty());
        assert_eq!(
            metrics
                .router_status(&proxy)
                .await
                .proxy_request_hard_timeout_total,
            1
        );
    }

    #[tokio::test]
    async fn manual_release_is_cancelling_and_idempotent() {
        let proxy = ProxyRegistry::default();
        let permit = proxy
            .try_acquire_share_permit(
                "req-manual",
                "share-manual",
                Some("claude"),
                1,
                Some("user@example.com"),
            )
            .await
            .unwrap();
        let cancellation = permit.cancellation_token();

        let released =
            proxy.force_release_share_requests(Some("req-manual"), None, "operator recovery");
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].reason, "operator recovery");
        assert!(cancellation.is_cancelled());
        assert!(proxy.inflight_by_share().await.is_empty());
        assert!(
            proxy
                .force_release_share_requests(Some("req-manual"), None, "repeat")
                .is_empty()
        );
        drop(permit);
        assert!(proxy.inflight_by_share().await.is_empty());
    }

    #[tokio::test]
    async fn response_pump_and_watchdog_share_one_release_owner() {
        let proxy = ProxyRegistry::default();
        let watchdog_permit = proxy
            .try_acquire_share_permit("req-watchdog", "share-a", Some("codex"), 1, None)
            .await
            .unwrap();
        let mut watchdog_lifecycle = Some(ProxyResponseLifecycle {
            share: Some(watchdog_permit),
            ..Default::default()
        });
        let keyed_permit = proxy
            .free_share_ip_limiter
            .try_acquire("127.0.0.1", 1)
            .await
            .unwrap();
        watchdog_lifecycle
            .as_ref()
            .and_then(|lifecycle| lifecycle.share.as_ref())
            .unwrap()
            .register_keyed_permit(&keyed_permit);
        assert_eq!(
            proxy
                .free_share_ip_limiter
                .snapshot()
                .await
                .get("127.0.0.1"),
            Some(&1)
        );
        assert_eq!(
            proxy
                .force_release_share_requests(Some("req-watchdog"), None, "watchdog")
                .len(),
            1
        );
        assert!(proxy.free_share_ip_limiter.snapshot().await.is_empty());
        assert!(!finish_proxy_response_lifecycle(&mut watchdog_lifecycle));
        drop(keyed_permit);

        let pump_permit = proxy
            .try_acquire_share_permit("req-pump", "share-a", Some("codex"), 1, None)
            .await
            .unwrap();
        let mut pump_lifecycle = Some(ProxyResponseLifecycle {
            share: Some(pump_permit),
            ..Default::default()
        });
        let pump_keyed_permit = proxy
            .free_share_ip_limiter
            .try_acquire("127.0.0.2", 1)
            .await
            .unwrap();
        pump_lifecycle
            .as_ref()
            .and_then(|lifecycle| lifecycle.share.as_ref())
            .unwrap()
            .register_keyed_permit(&pump_keyed_permit);
        assert!(finish_proxy_response_lifecycle(&mut pump_lifecycle));
        assert!(!finish_proxy_response_lifecycle(&mut pump_lifecycle));
        assert!(proxy.inflight_by_share().await.is_empty());
        assert!(proxy.free_share_ip_limiter.snapshot().await.is_empty());
        drop(pump_keyed_permit);
    }

    #[tokio::test]
    async fn response_headers_switch_watchdog_to_first_event_deadline() {
        let proxy = ProxyRegistry::default();
        let permit = proxy
            .try_acquire_share_permit("req-first-event", "share-a", Some("codex"), 1, None)
            .await
            .unwrap();
        permit.mark_response_headers_received();
        {
            let mut requests = proxy
                .share_requests
                .requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry = requests.values_mut().next().unwrap();
            entry.phase_started_at = Instant::now() - Duration::from_secs(6);
        }
        let config = crate::config::ProxyStreamConfig {
            response_header_timeout_secs: 5,
            first_event_timeout_secs: 600,
            ..Default::default()
        };
        assert!(proxy.release_stale_share_requests(&config).is_empty());

        {
            let mut requests = proxy
                .share_requests
                .requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry = requests.values_mut().next().unwrap();
            entry.phase_started_at = Instant::now() - Duration::from_secs(601);
        }
        let released = proxy.release_stale_share_requests(&config);
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].reason, "first_event_timeout");
        drop(permit);
    }

    #[tokio::test]
    async fn watchdog_releases_each_stale_phase_once() {
        let proxy = ProxyRegistry::default();
        let permits = vec![
            proxy
                .try_acquire_share_permit("req-headers", "share-a", Some("codex"), -1, None)
                .await
                .unwrap(),
            proxy
                .try_acquire_share_permit("req-first", "share-a", Some("codex"), -1, None)
                .await
                .unwrap(),
            proxy
                .try_acquire_share_permit("req-idle", "share-a", Some("codex"), -1, None)
                .await
                .unwrap(),
            proxy
                .try_acquire_share_permit("req-hard", "share-a", Some("codex"), -1, None)
                .await
                .unwrap(),
        ];
        let now = Instant::now();
        {
            let mut requests = proxy
                .share_requests
                .requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for entry in requests.values_mut() {
                match entry.request_id.as_str() {
                    "req-headers" => entry.phase_started_at = now - Duration::from_secs(121),
                    "req-first" => {
                        entry.phase = ShareRequestPhase::AwaitingFirstEvent;
                        entry.phase_started_at = now - Duration::from_secs(121);
                    }
                    "req-idle" => {
                        entry.phase = ShareRequestPhase::Streaming;
                        entry.last_progress_at = now - Duration::from_secs(901);
                    }
                    "req-hard" => entry.started_at = now - Duration::from_secs(7_201),
                    _ => unreachable!(),
                }
            }
        }

        let mut reasons = proxy
            .release_stale_share_requests(&crate::config::ProxyStreamConfig::default())
            .into_iter()
            .map(|released| released.reason)
            .collect::<Vec<_>>();
        reasons.sort();
        assert_eq!(
            reasons,
            vec![
                "business_idle_timeout",
                "first_event_timeout",
                "hard_lifetime_timeout",
                "response_header_timeout",
            ]
        );
        assert!(proxy.inflight_by_share().await.is_empty());
        assert!(
            proxy
                .release_stale_share_requests(&crate::config::ProxyStreamConfig::default())
                .is_empty()
        );
        for permit in permits {
            assert!(permit.cancellation_token().is_cancelled());
            drop(permit);
        }
    }

    #[tokio::test]
    async fn request_body_and_response_header_reads_are_bounded() {
        let body = Body::from_stream(futures_util::stream::pending::<
            Result<Bytes, std::convert::Infallible>,
        >());
        assert!(matches!(
            read_proxy_request_body(body, 1024, Duration::from_millis(20)).await,
            Err(ProxyRequestBodyReadError::Timeout)
        ));

        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            futures_util::future::pending::<()>().await;
        });
        let client = reqwest::Client::new();
        let result = send_proxy_upstream_request(
            client.get(format!("http://{address}/pending")),
            Duration::from_millis(30),
            None,
        )
        .await;
        assert!(matches!(
            result,
            Err(ProxyUpstreamRequestError::ResponseHeaderTimeout)
        ));
        server.abort();

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let result = send_proxy_upstream_request(
            client.get("http://127.0.0.1:1/cancelled"),
            Duration::from_secs(1),
            Some(cancellation),
        )
        .await;
        assert!(matches!(result, Err(ProxyUpstreamRequestError::Cancelled)));
    }

    #[tokio::test]
    async fn upstream_error_body_reads_are_size_bounded_and_cancellable() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                axum::Router::new().fallback(any(|| async { "oversized error body" })),
            )
            .await
            .unwrap();
        });
        let client = reqwest::Client::new();
        let response = client
            .get(format!("http://{address}/large"))
            .send()
            .await
            .unwrap();
        assert!(matches!(
            read_proxy_upstream_body(
                response,
                4,
                Duration::from_secs(1),
                CancellationToken::new(),
            )
            .await,
            Err(ProxyUpstreamBodyReadError::TooLarge)
        ));

        let response = client
            .get(format!("http://{address}/cancelled"))
            .send()
            .await
            .unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(matches!(
            read_proxy_upstream_body(response, 1024, Duration::from_secs(1), cancellation).await,
            Err(ProxyUpstreamBodyReadError::Cancelled)
        ));
        server.abort();
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
