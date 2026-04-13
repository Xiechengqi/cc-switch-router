use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use std::collections::HashMap;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::ServerState;

#[derive(Debug, Default)]
pub struct ProxyRegistry {
    routes: RwLock<HashMap<String, String>>,
}

impl ProxyRegistry {
    pub async fn set_route(&self, subdomain: String, backend: String) {
        self.routes.write().await.insert(subdomain, backend);
    }

    pub async fn remove_route(&self, subdomain: &str) {
        self.routes.write().await.remove(subdomain);
    }

    pub async fn backend_for_host(&self, host: &str, tunnel_domain: &str) -> Option<String> {
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
    let token = parts
        .headers
        .get("x-share-token")
        .and_then(|value| value.to_str().ok())
        .map(mask_token);
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());

    let Some(backend) = state
        .proxy
        .backend_for_host(&host, &state.config.tunnel_domain)
        .await
    else {
        warn!(
            method = %method,
            host = %host,
            path = %path_and_query,
            share_token = token.as_deref().unwrap_or("-"),
            "proxy request rejected: unregistered subdomain"
        );
        return simple_response(StatusCode::NOT_FOUND, "unregistered-subdomain");
    };

    let target = format!("http://{backend}{path_and_query}");

    let mut builder = reqwest::Client::new().request(method.clone(), target);
    for (name, value) in &parts.headers {
        if name.as_str().eq_ignore_ascii_case("host") || is_hop_by_hop_header(name.as_str()) {
            continue;
        }
        builder = builder.header(name, value);
    }

    let body = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(body) => body,
        Err(err) => {
            warn!(
                method = %method,
                host = %host,
                path = %path_and_query,
                backend = %backend,
                share_token = token.as_deref().unwrap_or("-"),
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
                share_token = token.as_deref().unwrap_or("-"),
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
    // downstream clients receive chunks in real time and cc-switch's
    // usage collector fires correctly when the stream ends.
    let body_stream = upstream.bytes_stream();
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
        backend = %backend,
        status = %status.as_u16(),
        share_token = token.as_deref().unwrap_or("-"),
        "proxy request completed"
    );
    response
}

fn simple_response(status: StatusCode, reason: &str) -> Response {
    let mut response = Response::new(Body::from(reason.to_string()));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert("x-portr-error", HeaderValue::from_static("true"));
    if let Ok(value) = HeaderValue::from_str(reason) {
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

fn mask_token(token: &str) -> String {
    if token.len() <= 8 {
        return "*".repeat(token.len());
    }
    format!("{}...{}", &token[..4], &token[token.len() - 4..])
}
