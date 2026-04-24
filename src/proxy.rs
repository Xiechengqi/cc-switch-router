use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::ServerState;

/// Per-subdomain routing info.
#[derive(Debug, Clone)]
pub(crate) struct RouteEntry {
    backend: String,
    /// Share token to inject as X-Share-Token when proxying.
    /// None for non-share tunnels.
    share_token: Option<String>,
    share_id: Option<String>,
    parallel_limit: i64,
}

#[derive(Debug, Default)]
struct ShareConcurrencyLimiter {
    shares: Mutex<HashMap<String, usize>>,
}

#[derive(Debug)]
struct ShareConcurrencyPermit {
    limiter: Arc<ShareConcurrencyLimiter>,
    share_id: String,
}

impl Drop for ShareConcurrencyPermit {
    fn drop(&mut self) {
        let limiter = self.limiter.clone();
        let share_id = self.share_id.clone();
        tokio::spawn(async move {
            let mut shares = limiter.shares.lock().await;
            let should_remove = match shares.get_mut(&share_id) {
                Some(inflight) if *inflight > 1 => {
                    *inflight -= 1;
                    false
                }
                Some(_) => true,
                None => false,
            };
            if should_remove {
                shares.remove(&share_id);
            }
        });
    }
}

impl ShareConcurrencyLimiter {
    /// Increment the in-flight counter for this share. Returns `None` when a
    /// non-negative `parallel_limit` has been reached (caller should reject the
    /// request). A negative `parallel_limit` means unlimited — we still track
    /// the in-flight count so it can be surfaced in the dashboard.
    async fn try_acquire(
        self: &Arc<Self>,
        share_id: &str,
        parallel_limit: i64,
    ) -> Option<ShareConcurrencyPermit> {
        let mut shares = self.shares.lock().await;
        let inflight = shares.entry(share_id.to_string()).or_insert(0);
        if parallel_limit >= 0 {
            let limit = parallel_limit as usize;
            if *inflight >= limit {
                return None;
            }
        }
        *inflight += 1;
        Some(ShareConcurrencyPermit {
            limiter: self.clone(),
            share_id: share_id.to_string(),
        })
    }

    async fn snapshot(&self) -> HashMap<String, usize> {
        self.shares.lock().await.clone()
    }
}

#[derive(Debug, Default)]
pub struct ProxyRegistry {
    routes: RwLock<HashMap<String, RouteEntry>>,
    limiter: Arc<ShareConcurrencyLimiter>,
}

impl ProxyRegistry {
    pub async fn set_route(
        &self,
        subdomain: String,
        backend: String,
        share_token: Option<String>,
        share_id: Option<String>,
        parallel_limit: i64,
    ) {
        self.routes.write().await.insert(
            subdomain,
            RouteEntry {
                backend,
                share_token,
                share_id,
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

    pub async fn active_subdomains(&self) -> Vec<String> {
        self.routes.read().await.keys().cloned().collect()
    }

    /// Snapshot of in-flight request counts per share_id. Share IDs absent from
    /// the map have zero in-flight requests.
    pub async fn inflight_by_share(&self) -> HashMap<String, usize> {
        self.limiter.snapshot().await
    }

    async fn try_acquire_share_permit(
        &self,
        share_id: &str,
        parallel_limit: i64,
    ) -> Option<ShareConcurrencyPermit> {
        self.limiter.try_acquire(share_id, parallel_limit).await
    }
}

pub async fn proxy_handler(State(state): State<ServerState>, req: Request) -> Response {
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
            "proxy request rejected: unregistered subdomain"
        );
        return simple_response(StatusCode::NOT_FOUND, "unregistered-subdomain");
    };
    let backend = route.backend.clone();
    let route_share_token = route.share_token.clone();

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
                error = %err,
                "proxy request body read failed"
            );
            return simple_response(
                StatusCode::BAD_REQUEST,
                &format!("failed-to-read-body: {err}"),
            );
        }
    };

    let upstream = match builder.body(body).send().await {
        Ok(response) => response,
        Err(err) => {
            warn!(
                method = %method,
                host = %host,
                path = %path_and_query,
                backend = %backend,
                share_token = %log_token,
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
        let limiter = Arc::new(ShareConcurrencyLimiter::default());

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
        let limiter = Arc::new(ShareConcurrencyLimiter::default());

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
        assert_eq!(route.parallel_limit, 5);
    }
}

fn mask_token(token: &str) -> String {
    if token.len() <= 8 {
        return "*".repeat(token.len());
    }
    format!("{}...{}", &token[..4], &token[token.len() - 4..])
}
