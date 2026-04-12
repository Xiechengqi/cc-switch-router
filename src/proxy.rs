use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::Response;
use std::collections::HashMap;
use tokio::sync::RwLock;

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
}

pub async fn proxy_handler(State(state): State<ServerState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let host = parts
        .headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();

    let Some(backend) = state
        .proxy
        .backend_for_host(&host, &state.config.tunnel_domain)
        .await
    else {
        return simple_response(StatusCode::NOT_FOUND, "unregistered-subdomain");
    };

    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    let target = format!("http://{backend}{path_and_query}");

    let method = parts.method.clone();
    let mut builder = reqwest::Client::new().request(method, target);
    for (name, value) in &parts.headers {
        if name.as_str().eq_ignore_ascii_case("host") {
            continue;
        }
        builder = builder.header(name, value);
    }

    let body = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(body) => body,
        Err(err) => {
            return simple_response(
                StatusCode::BAD_REQUEST,
                &format!("failed-to-read-body: {err}"),
            );
        }
    };

    let upstream = match builder.body(body).send().await {
        Ok(response) => response,
        Err(err) => {
            return simple_response(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!("connection-lost: {err}"),
            );
        }
    };

    let status = upstream.status();
    let headers = upstream.headers().clone();
    let body = upstream.bytes().await.unwrap_or_default();
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response.headers_mut().clear();
    for (name, value) in &headers {
        response.headers_mut().insert(name, value.clone());
    }
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
