use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, watch};
use tracing::{debug, info, warn};

use crate::ServerState;
use crate::recent_traffic::RecentTraffic;

const MARKET_REQUEST_ID_HEADER: &str = "x-cc-switch-market-request-id";
const HEALTH_PROBE_FAILURE_CACHE_TTL: Duration = Duration::from_secs(2);
const CLIENT_WEB_USER_EMAIL_HEADER: &str = "x-cc-switch-web-user-email";
const CLIENT_WEB_ROLE_HEADER: &str = "x-cc-switch-web-role";
const CLIENT_WEB_INSTALLATION_ID_HEADER: &str = "x-cc-switch-installation-id";
const CLIENT_WEB_SUBDOMAIN_HEADER: &str = "x-cc-switch-client-tunnel-subdomain";

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
}

#[derive(Debug, Clone)]
struct PendingRouteEntry {
    expires_at: Instant,
}

impl RouteEntry {
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
    routes: RwLock<HashMap<String, RouteEntry>>,
    pending_routes: RwLock<HashMap<String, PendingRouteEntry>>,
    health_probe_failures: Mutex<HashMap<String, Instant>>,
    share_limiter: Arc<KeyedConcurrencyLimiter>,
    free_share_ip_limiter: Arc<KeyedConcurrencyLimiter>,
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
        let old_route = {
            let mut routes = self.routes.write().await;
            routes.insert(
                subdomain.clone(),
                RouteEntry {
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
                },
            )
        };
        if let Some(shutdown) = old_route.and_then(|route| route.shutdown) {
            shutdown.shutdown();
        }
    }

    pub async fn mark_route_pending(&self, subdomain: String, ttl: Duration) {
        self.pending_routes.write().await.insert(
            subdomain,
            PendingRouteEntry {
                expires_at: Instant::now() + ttl,
            },
        );
    }

    pub async fn has_pending_route(&self, subdomain: &str) -> bool {
        let now = Instant::now();
        let mut pending = self.pending_routes.write().await;
        pending.retain(|_, entry| entry.expires_at > now);
        pending.contains_key(subdomain)
    }

    pub async fn remove_route(&self, subdomain: &str) {
        let old_route = self.routes.write().await.remove(subdomain);
        if let Some(shutdown) = old_route.and_then(|route| route.shutdown) {
            shutdown.shutdown();
        }
    }

    pub async fn remove_route_if_connection(&self, subdomain: &str, connection_id: &str) {
        let mut routes = self.routes.write().await;
        let should_remove = routes
            .get(subdomain)
            .and_then(|route| route.connection_id())
            == Some(connection_id);
        let old_route = if should_remove {
            routes.remove(subdomain)
        } else {
            None
        };
        drop(routes);
        if let Some(shutdown) = old_route.and_then(|route| route.shutdown) {
            shutdown.shutdown();
        }
    }

    pub(crate) async fn backend_for_host(
        &self,
        host: &str,
        tunnel_domain: &str,
    ) -> Option<RouteEntry> {
        let subdomain = subdomain_for_host(host, tunnel_domain)?;
        self.routes.read().await.get(&subdomain).cloned()
    }

    pub(crate) async fn route_by_share_id(&self, share_id: &str) -> Option<RouteEntry> {
        self.routes
            .read()
            .await
            .values()
            .find(|route| route.share_id.as_deref() == Some(share_id))
            .cloned()
    }

    pub async fn active_subdomains(&self) -> Vec<String> {
        self.routes.read().await.keys().cloned().collect()
    }

    pub async fn counts(&self) -> ProxyRegistryCounts {
        let now = Instant::now();
        let mut pending = self.pending_routes.write().await;
        pending.retain(|_, entry| entry.expires_at > now);
        let mut failures = self.health_probe_failures.lock().await;
        failures.retain(|_, expires_at| *expires_at > now);
        ProxyRegistryCounts {
            active_routes: self.routes.read().await.len(),
            pending_routes: pending.len(),
            health_probe_failure_cache: failures.len(),
        }
    }

    /// Snapshot of in-flight request counts per share_id. Share IDs absent from
    /// the map have zero in-flight requests.
    pub async fn inflight_by_share(&self) -> HashMap<String, usize> {
        self.share_limiter.snapshot().await
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
            if let Some(permit) = self.try_acquire_share_permit(share_id, -1).await {
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
        parallel_limit: i64,
    ) -> Option<KeyedConcurrencyPermit> {
        self.share_limiter
            .try_acquire(share_id, parallel_limit)
            .await
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

    let Some(route) = state.proxy.route_by_share_id(&share_id).await else {
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
            || is_hop_by_hop_header(n)
        {
            continue;
        }
        builder = builder.header(name, value);
    }
    builder = builder.header("X-CC-Switch-Share-Id", share_id.as_str());

    let log_share_id = mask_token(&share_id);
    let share_permit = match state
        .proxy
        .try_acquire_share_permit(&share_id, route.parallel_limit)
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
    let body_stream = {
        use futures_util::StreamExt;

        upstream.bytes_stream().map(move |chunk| {
            let _permit = &share_permit;
            let _free_share_ip_permit = &free_share_ip_permit;
            let _market_permit = &market_permit;
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

    let Some(route) = state.proxy.route_by_share_id(&share_id).await else {
        return simple_response(StatusCode::NOT_FOUND, "share-offline");
    };
    let backend = route.backend.clone();
    let target = format!("http://{backend}{path_and_query}");

    let metrics_permit = state.metrics.proxy_request_started();
    let share_permit = match state
        .proxy
        .try_acquire_share_permit(&share_id, route.parallel_limit)
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
            || is_hop_by_hop_header(n)
        {
            continue;
        }
        builder = builder.header(name, value);
    }
    builder = builder.header("X-CC-Switch-Share-Id", share_id.as_str());
    builder = builder.header("X-CC-Switch-Share-Subdomain", route.subdomain.as_str());

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
    let (parts, body) = req.into_parts();
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
    let is_internal_share_router_path = path.starts_with("/_share-router");
    let is_share_router_probe = parts
        .headers
        .get("x-share-router-probe")
        .and_then(|value| value.to_str().ok())
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
        && path == "/_share-router/health";
    let is_share_model_health_check = truthy_header(&parts.headers, "x-share-router-health-check");
    let is_legacy_share_router_ping =
        matches!(method, axum::http::Method::GET | axum::http::Method::HEAD)
            && truthy_header(&parts.headers, "x-share-router-ping-request");

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
    let Some(route) = state
        .proxy
        .backend_for_host(&host, &state.config.tunnel_domain)
        .await
    else {
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
            "proxy request rejected: unregistered subdomain"
        );
        return simple_response(StatusCode::NOT_FOUND, "unregistered-subdomain");
    };
    let backend = route.backend.clone();
    let log_share_id = route
        .share_id
        .as_deref()
        .map(mask_token)
        .unwrap_or_else(|| "-".to_string());
    let is_health_check_request = is_share_router_probe || is_share_model_health_check;
    let is_direct_share_web_request = route.is_share() && is_allowed_direct_share_web_path(&path);
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
    if is_legacy_share_router_ping {
        debug!(
            method = %method,
            host = %host,
            path = %path_and_query,
            backend = %backend,
            status = %StatusCode::NO_CONTENT.as_u16(),
            share_id = %log_share_id,
            client_ip = %user_ip,
            client_country = %user_country,
            client_asn = %user_asn,
            user_agent = %user_agent,
            "proxy legacy health ping completed"
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
        if is_client_web_auth_required_path(&path) {
            let owner_email = match state
                .store
                .client_tunnel_owner_email(&route.subdomain)
                .await
            {
                Ok(Some(email)) => email,
                Ok(None) => {
                    return simple_response(StatusCode::NOT_FOUND, "client-tunnel-not-found");
                }
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
                Ok(None) => return simple_response(StatusCode::UNAUTHORIZED, "login-required"),
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
            if session.0 != owner_email && !session.1 {
                return simple_response(StatusCode::FORBIDDEN, "client-web-forbidden");
            }
            client_web_session = Some(session);
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
    if !is_internal_share_router_path
        && !is_share_model_health_check
        && !is_direct_share_web_request
    {
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
        if n.eq_ignore_ascii_case("x-cc-switch-user-email") {
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
                || n.eq_ignore_ascii_case("authorization")
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
    if let Some(ref email) = api_user_email {
        builder = builder.header("X-CC-Switch-User-Email", email.as_str());
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

    let share_permit = if is_internal_share_router_path
        || is_share_model_health_check
        || is_direct_share_web_request
    {
        None
    } else if let Some(share_id) = route.share_id.as_deref() {
        match state
            .proxy
            .try_acquire_share_permit(share_id, route.parallel_limit)
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

    let free_share_ip_permit = if !is_internal_share_router_path
        && !is_share_model_health_check
        && !is_direct_share_web_request
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
    let live_request_id = if !is_internal_share_router_path
        && !is_share_router_probe
        && !is_share_model_health_check
        && !is_direct_share_web_request
    {
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
    // Bind a completion guard to the recorded request id. While this binding
    // lives at function scope it covers the early-return-on-upstream-error
    // path; once the body stream is constructed we move it into the streaming
    // closure so completion fires when the upstream stream actually ends.
    let recent_traffic_guard = live_request_id.as_ref().map(|id| RecentTrafficGuard {
        traffic: state.recent_traffic.clone(),
        request_id: id.clone(),
    });

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
            return simple_response(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!("connection-lost: {err}"),
            );
        }
    };

    let status = upstream.status();
    state.metrics.record_proxy_status(status);
    if is_health_check_request {
        state
            .proxy
            .clear_health_probe_failure(&route.subdomain)
            .await;
    }
    let response_headers = upstream.headers().clone();
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
    let body_stream = {
        use futures_util::StreamExt;

        upstream.bytes_stream().map(move |chunk| {
            let _permit = &share_permit;
            let _free_share_ip_permit = &free_share_ip_permit;
            // Hold the recent-traffic guard until the upstream stream ends so
            // the dashboard ticker keeps the row marked in-flight for the full
            // request lifecycle (success, client disconnect, or chunk error).
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

fn truthy_header(headers: &HeaderMap, name: &str) -> bool {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
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
        || path.starts_with("/assets/")
        || path == "/web-api/context"
        || path.starts_with("/web-api/invoke/")
}

fn is_allowed_client_web_path(path: &str) -> bool {
    (path == "/"
        || path == "/favicon.ico"
        || path.starts_with("/assets/")
        || path == "/web-api/auth/email/request-code"
        || path == "/web-api/auth/email/verify-code"
        || path == "/web-api/auth/session/refresh"
        || path == "/web-api/context"
        || path.starts_with("/web-api/invoke/"))
        && !path.starts_with("/_ctl/")
        && !path.starts_with("/_share-router/")
        && !is_allowed_direct_share_api_path(path)
}

fn is_client_web_auth_required_path(path: &str) -> bool {
    path == "/web-api/context" || path.starts_with("/web-api/invoke/")
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
    use super::*;

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
        assert!(is_allowed_direct_share_proxy_path("/assets/index-abc.js"));
        assert!(is_allowed_direct_share_proxy_path("/web-api/context"));
        assert!(is_allowed_direct_share_proxy_path(
            "/web-api/invoke/list_shares"
        ));
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
