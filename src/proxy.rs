use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::ServerState;
use crate::recent_traffic::RecentTraffic;

/// Per-subdomain routing info.
#[derive(Debug, Clone)]
pub(crate) struct RouteEntry {
    backend: String,
    /// Share token to inject as X-Share-Token when proxying.
    /// None for non-share tunnels.
    share_token: Option<String>,
    share_id: Option<String>,
    share_name: Option<String>,
    subdomain: String,
    is_free_share: bool,
    parallel_limit: i64,
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
    share_limiter: Arc<KeyedConcurrencyLimiter>,
    free_share_ip_limiter: Arc<KeyedConcurrencyLimiter>,
}

impl ProxyRegistry {
    pub async fn set_route(
        &self,
        subdomain: String,
        backend: String,
        share_token: Option<String>,
        share_id: Option<String>,
        share_name: Option<String>,
        is_free_share: bool,
        parallel_limit: i64,
    ) {
        self.routes.write().await.insert(
            subdomain.clone(),
            RouteEntry {
                backend,
                share_token,
                share_id,
                share_name,
                subdomain,
                is_free_share,
                parallel_limit,
            },
        );
    }

    pub async fn remove_route(&self, subdomain: &str) {
        self.routes.write().await.remove(subdomain);
    }

    pub(crate) async fn backend_for_host(
        &self,
        host: &str,
        tunnel_domain: &str,
    ) -> Option<RouteEntry> {
        let host_without_port = host.split(':').next().unwrap_or(host);
        let suffix = format!(".{tunnel_domain}");
        if !host_without_port.ends_with(&suffix) {
            return None;
        }
        let subdomain = host_without_port.trim_end_matches(&suffix);
        self.routes.read().await.get(subdomain).cloned()
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

    /// Snapshot of in-flight request counts per share_id. Share IDs absent from
    /// the map have zero in-flight requests.
    pub async fn inflight_by_share(&self) -> HashMap<String, usize> {
        self.share_limiter.snapshot().await
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

    let host_without_port = host.split(':').next().unwrap_or(&host);
    let expected_host = format!("{}.{}", market_subdomain, state.config.tunnel_domain);
    if host_without_port != expected_host {
        warn!(
            method = %method,
            host = %host,
            expected_host = %expected_host,
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
        Ok(shares) => shares.into_iter().any(|share| share.share_id == share_id),
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
    let route_share_token = route.share_token.clone();
    let target = format!("http://{backend}{path_and_query}");

    let mut builder = reqwest::Client::new().request(method.clone(), target);
    for (name, value) in &parts.headers {
        let n = name.as_str();
        if n.eq_ignore_ascii_case("host")
            || n.eq_ignore_ascii_case("authorization")
            || n.eq_ignore_ascii_case("x-share-token")
            || is_hop_by_hop_header(n)
        {
            continue;
        }
        builder = builder.header(name, value);
    }
    if let Some(ref tok) = route_share_token {
        builder = builder.header("X-Share-Token", tok.as_str());
    }

    let log_token = route_share_token
        .as_deref()
        .map(mask_token)
        .unwrap_or_else(|| "-".to_string());
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

    let body = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(body) => body,
        Err(err) => {
            warn!(
                method = %method,
                host = %host,
                path = %path_and_query,
                backend = %backend,
                share_token = %log_token,
                client_ip = %user_ip,
                client_country = %user_country,
                client_asn = %user_asn,
                user_agent = %user_agent,
                error = %err,
                "market proxy request body read failed"
            );
            return simple_response(
                StatusCode::BAD_REQUEST,
                &format!("failed-to-read-body: {err}"),
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

    let live_request_id = Some(
        state
            .recent_traffic
            .record(
                share_id.clone(),
                route.share_name.clone(),
                Some(route.subdomain.clone()),
                client_metadata.country_code.clone(),
            )
            .await,
    );
    if let Some(ref request_id) = live_request_id {
        builder = builder.header("X-CC-Switch-Request-Id", request_id.as_str());
    }
    let recent_traffic_guard = live_request_id.as_ref().map(|id| RecentTrafficGuard {
        traffic: state.recent_traffic.clone(),
        request_id: id.clone(),
    });

    let upstream = match builder.body(body).send().await {
        Ok(response) => response,
        Err(err) => {
            warn!(
                method = %method,
                host = %host,
                path = %path_and_query,
                backend = %backend,
                share_token = %log_token,
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
    let response_headers = upstream.headers().clone();
    let body_stream = {
        use futures_util::StreamExt;

        upstream.bytes_stream().map(move |chunk| {
            let _permit = &share_permit;
            let _free_share_ip_permit = &free_share_ip_permit;
            let _recent_traffic_guard = &recent_traffic_guard;
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
        response.headers_mut().insert(name, value.clone());
    }
    strip_connection_listed_headers(response.headers_mut());
    info!(
        method = %method,
        host = %host,
        path = %path_and_query,
        share_id = %share_id,
        backend = %backend,
        status = %status.as_u16(),
        share_token = %log_token,
        client_ip = %user_ip,
        client_country = %user_country,
        client_asn = %user_asn,
        user_agent = %user_agent,
        "market proxy request completed"
    );
    response
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
    let client_metadata = crate::client_meta::extract_client_metadata(&parts.headers, peer);
    let user_ip = client_metadata
        .ip
        .clone()
        .unwrap_or_else(|| peer.ip().to_string());
    let user_country = client_metadata.country_code.as_deref().unwrap_or("-");
    let user_asn = trusted_asn_header(&parts.headers, peer);
    let user_agent = header_str(&parts.headers, "user-agent");
    let is_internal_share_router_path =
        path.starts_with("/_share-router") || path.starts_with("/_portr");
    let is_share_router_probe = parts
        .headers
        .get("x-share-router-probe")
        .or_else(|| parts.headers.get("x-portr-probe"))
        .and_then(|value| value.to_str().ok())
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
        && matches!(path.as_str(), "/_share-router/health" | "/_portr/health");
    let is_legacy_share_router_ping =
        matches!(method, axum::http::Method::GET | axum::http::Method::HEAD)
            && (truthy_header(&parts.headers, "x-share-router-ping-request")
                || truthy_header(&parts.headers, "x-portr-ping-request"));

    let host_without_port = host.split(':').next().unwrap_or(&host);
    let tunnel_suffix = format!(".{}", state.config.tunnel_domain);
    if host_without_port != state.config.tunnel_domain
        && !host_without_port.ends_with(&tunnel_suffix)
    {
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

    let Some(route) = state
        .proxy
        .backend_for_host(&host, &state.config.tunnel_domain)
        .await
    else {
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
    let route_share_token = route.share_token.clone();
    if is_legacy_share_router_ping {
        let legacy_log_token = route_share_token
            .as_deref()
            .map(mask_token)
            .unwrap_or_else(|| "-".to_string());
        debug!(
            method = %method,
            host = %host,
            path = %path_and_query,
            backend = %backend,
            status = %StatusCode::NO_CONTENT.as_u16(),
            share_token = %legacy_log_token,
            client_ip = %user_ip,
            client_country = %user_country,
            client_asn = %user_asn,
            user_agent = %user_agent,
            "proxy legacy health ping completed"
        );
        return empty_response(StatusCode::NO_CONTENT);
    }

    // Determine effective share token: prefer client-supplied header,
    // fall back to the token registered with this tunnel route.
    let client_token = parts
        .headers
        .get("x-share-token")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let effective_token = client_token.or(route_share_token);

    let target = format!("http://{backend}{path_and_query}");

    let mut builder = reqwest::Client::new().request(method.clone(), target);
    for (name, value) in &parts.headers {
        let n = name.as_str();
        if n.eq_ignore_ascii_case("host") || is_hop_by_hop_header(n) {
            continue;
        }
        // Skip client-supplied x-share-token; we inject the effective one below.
        if n.eq_ignore_ascii_case("x-share-token") {
            continue;
        }
        builder = builder.header(name, value);
    }

    // Inject effective share token so cc-switch can track share usage.
    if let Some(ref tok) = effective_token {
        builder = builder.header("X-Share-Token", tok.as_str());
    }

    let log_token = effective_token
        .as_deref()
        .map(mask_token)
        .unwrap_or_else(|| "-".to_string());

    let share_permit = if is_internal_share_router_path {
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
                share_token = %log_token,
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
    let live_request_id = if !is_internal_share_router_path && !is_share_router_probe {
        if let Some(share_id) = route.share_id.as_deref() {
            Some(
                state
                    .recent_traffic
                    .record(
                        share_id.to_string(),
                        route.share_name.clone(),
                        Some(route.subdomain.clone()),
                        client_metadata.country_code.clone(),
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
            warn!(
                method = %method,
                host = %host,
                path = %path_and_query,
                backend = %backend,
                share_token = %log_token,
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
    let response_headers = upstream.headers().clone();

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
        response.headers_mut().insert(name, value.clone());
    }
    strip_connection_listed_headers(response.headers_mut());
    if is_share_router_probe {
        debug!(
            method = %method,
            host = %host,
            path = %path_and_query,
            backend = %backend,
            status = %status.as_u16(),
            share_token = %log_token,
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
            share_token = %log_token,
            client_ip = %user_ip,
            client_country = %user_country,
            client_asn = %user_asn,
            user_agent = %user_agent,
            "proxy request completed"
        );
    }
    response
}

fn simple_response(status: StatusCode, reason: &str) -> Response {
    let mut response = Response::new(Body::from(reason.to_string()));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert("x-share-router-error", HeaderValue::from_static("true"));
    response
        .headers_mut()
        .insert("x-portr-error", HeaderValue::from_static("true"));
    if let Ok(value) = HeaderValue::from_str(reason) {
        response
            .headers_mut()
            .insert("x-share-router-error-reason", value.clone());
        response.headers_mut().insert("x-portr-error-reason", value);
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
                Some("token-demo".into()),
                Some("share-1".into()),
                Some("Demo Share".into()),
                true,
                5,
            )
            .await;

        let route = registry
            .backend_for_host("demo.example.com", "example.com")
            .await
            .expect("route metadata");

        assert_eq!(route.backend, "127.0.0.1:3000");
        assert_eq!(route.share_token.as_deref(), Some("token-demo"));
        assert_eq!(route.share_id.as_deref(), Some("share-1"));
        assert!(route.is_free_share);
        assert_eq!(route.parallel_limit, 5);
    }
}

fn mask_token(token: &str) -> String {
    if token.len() <= 8 {
        return "*".repeat(token.len());
    }
    format!("{}...{}", &token[..4], &token[token.len() - 4..])
}
