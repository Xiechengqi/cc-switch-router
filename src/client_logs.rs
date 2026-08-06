use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

use crate::ServerState;
use crate::ctl_client::{ClientLogTailReply, CtlError};
use crate::error::AppError;

const PUBLIC_LOG_LINE_LIMIT: usize = 10;
const FULL_LOG_LINE_LIMIT: usize = 100;
const LOG_RESPONSE_MAX_BYTES: usize = 256 * 1024;
const LOG_LINE_MAX_BYTES: usize = 16 * 1024;
const GLOBAL_CONCURRENCY_LIMIT: usize = 16;
const PER_CLIENT_MIN_INTERVAL: Duration = Duration::from_secs(1);
const RATE_ENTRY_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug)]
pub struct ClientLogAccessLimiter {
    global: Arc<Semaphore>,
    clients: Mutex<HashMap<String, Weak<Semaphore>>>,
    last_started: Mutex<HashMap<String, Instant>>,
}

impl Default for ClientLogAccessLimiter {
    fn default() -> Self {
        Self {
            global: Arc::new(Semaphore::new(GLOBAL_CONCURRENCY_LIMIT)),
            clients: Mutex::new(HashMap::new()),
            last_started: Mutex::new(HashMap::new()),
        }
    }
}

struct ClientLogAccessPermit {
    _global: OwnedSemaphorePermit,
    _client: OwnedSemaphorePermit,
}

impl ClientLogAccessLimiter {
    async fn try_acquire(
        self: &Arc<Self>,
        installation_id: &str,
    ) -> Result<ClientLogAccessPermit, AppError> {
        let global =
            self.global
                .clone()
                .try_acquire_owned()
                .map_err(|_| AppError::RateLimited {
                    message: "Client log service is busy".into(),
                    retry_after_secs: 1,
                })?;
        let client_semaphore = {
            let mut clients = self.clients.lock().await;
            clients.retain(|_, semaphore| semaphore.strong_count() > 0);
            if let Some(semaphore) = clients.get(installation_id).and_then(Weak::upgrade) {
                semaphore
            } else {
                let semaphore = Arc::new(Semaphore::new(1));
                clients.insert(installation_id.to_string(), Arc::downgrade(&semaphore));
                semaphore
            }
        };
        let client = client_semaphore
            .try_acquire_owned()
            .map_err(|_| AppError::RateLimited {
                message: "Client log request is already in progress".into(),
                retry_after_secs: 1,
            })?;
        let now = Instant::now();
        let mut last_started = self.last_started.lock().await;
        last_started.retain(|_, at| now.duration_since(*at) <= RATE_ENTRY_TTL);
        if last_started
            .get(installation_id)
            .is_some_and(|at| now.duration_since(*at) < PER_CLIENT_MIN_INTERVAL)
        {
            return Err(AppError::RateLimited {
                message: "Client logs are being refreshed too frequently".into(),
                retry_after_secs: 1,
            });
        }
        last_started.insert(installation_id.to_string(), now);
        Ok(ClientLogAccessPermit {
            _global: global,
            _client: client,
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientLogsResponse {
    installation_id: String,
    content: String,
    lines: usize,
    limit: usize,
    truncated: bool,
    full_log_access: bool,
    fetched_at: String,
}

pub fn router() -> Router<ServerState> {
    Router::new().route("/v1/clients/:installation_id/logs", get(client_logs))
}

async fn client_logs(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(installation_id): Path<String>,
) -> Response {
    let mut response = match client_logs_inner(&state, &headers, &installation_id).await {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => error.into_response(),
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

async fn client_logs_inner(
    state: &ServerState,
    headers: &HeaderMap,
    installation_id: &str,
) -> Result<ClientLogsResponse, AppError> {
    let installation_id = installation_id.trim();
    if installation_id.is_empty() || installation_id.len() > 128 {
        return Err(AppError::BadRequest(
            "invalid Client installation id".into(),
        ));
    }
    let target = state
        .store
        .client_log_target(installation_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Client not found".into()))?;
    if !target.log_collection_enabled {
        return Err(AppError::Conflict(
            "Client log collection is disabled".into(),
        ));
    }

    let session = crate::api::resolve_router_session(state, headers).await?;
    let is_router_owner = session.as_ref().is_some_and(|session| {
        state
            .config
            .official_provider_email()
            .is_some_and(|owner| owner.eq_ignore_ascii_case(session.email.trim()))
    });
    let is_client_owner = if let Some(session) = session.as_ref() {
        state
            .store
            .is_verified_installation_owner(installation_id, &session.email)
            .await?
    } else {
        false
    };
    let full_log_access = is_router_owner || is_client_owner;
    let limit = if full_log_access {
        FULL_LOG_LINE_LIMIT
    } else {
        PUBLIC_LOG_LINE_LIMIT
    };
    let _permit = state.client_logs.try_acquire(installation_id).await?;
    let route = state
        .proxy
        .active_client_route(&target.subdomain)
        .await
        .ok_or_else(|| {
            AppError::ServiceUnavailable(
                "Client logs are unavailable while the Client is offline".into(),
            )
        })?;
    if route.installation_id() != Some(installation_id) {
        return Err(AppError::ServiceUnavailable(
            "Client log route does not match the installation".into(),
        ));
    }
    let reply = crate::ctl_client::fetch_client_log_tail(
        &state.proxy_http,
        route.route_target(),
        installation_id,
        &target.control_secret,
        limit,
    )
    .await
    .map_err(map_control_error)?;
    let mut content = validate_client_reply(&reply, limit)?;
    if !full_log_access {
        content = public_log_projection(&content);
    }
    let lines = content.lines().count();
    Ok(ClientLogsResponse {
        installation_id: installation_id.to_string(),
        content,
        lines,
        limit,
        truncated: reply.truncated,
        full_log_access,
        fetched_at: chrono::Utc::now().to_rfc3339(),
    })
}

fn map_control_error(error: CtlError) -> AppError {
    match error {
        CtlError::Unreachable(_) | CtlError::Timeout => {
            AppError::ServiceUnavailable("Client logs are temporarily unavailable".into())
        }
        CtlError::Rejected { .. } => {
            AppError::ServiceUnavailable("Client rejected the log request".into())
        }
        CtlError::Malformed(_) => {
            AppError::Internal("Client returned an invalid log response".into())
        }
    }
}

fn validate_client_reply(reply: &ClientLogTailReply, limit: usize) -> Result<String, AppError> {
    if reply.content.len() > LOG_RESPONSE_MAX_BYTES {
        return Err(AppError::Internal(
            "Client log response is too large".into(),
        ));
    }
    let lines = reply.content.lines().collect::<Vec<_>>();
    if lines.len() > limit
        || reply.lines != lines.len()
        || lines.iter().any(|line| line.len() > LOG_LINE_MAX_BYTES)
    {
        return Err(AppError::Internal(
            "Client returned an invalid log response".into(),
        ));
    }
    Ok(lines
        .into_iter()
        .map(|line| line.trim_end_matches('\r'))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn public_log_projection(content: &str) -> String {
    content
        .lines()
        .map(public_log_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn public_log_line(line: &str) -> String {
    line.split_whitespace()
        .map(|token| {
            let trimmed = token.trim_matches(|character: char| {
                matches!(
                    character,
                    ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\''
                )
            });
            let value = trimmed
                .rsplit_once('=')
                .map(|(_, value)| value)
                .unwrap_or(trimmed)
                .trim_matches(|character: char| {
                    matches!(
                        character,
                        ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\''
                    )
                });
            if value.contains('@') {
                "[email]"
            } else if contains_ip_address(value) {
                "[ip]"
            } else if value.contains("http://") || value.contains("https://") {
                "[url]"
            } else if value.len() > 96 {
                "[value]"
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn contains_ip_address(value: &str) -> bool {
    value.parse::<std::net::IpAddr>().is_ok()
        || value.parse::<std::net::SocketAddr>().is_ok()
        || value
            .split(|character: char| {
                !(character.is_ascii_hexdigit() || matches!(character, '.' | ':' | '%'))
            })
            .filter(|candidate| candidate.contains('.') || candidate.contains(':'))
            .map(|candidate| candidate.split('%').next().unwrap_or(candidate))
            .any(|candidate| candidate.parse::<std::net::IpAddr>().is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_projection_removes_identity_and_network_values() {
        let projected = public_log_projection(
            "owner=user@example.com ip=203.0.113.4 wrapped=Some(198.51.100.8) socket=[2001:db8::1]:443 url=https://client.example/path ordinary=value",
        );
        assert_eq!(projected, "[email] [ip] [ip] [ip] [url] ordinary=value");
    }

    #[test]
    fn client_reply_must_match_the_authorized_limit() {
        let reply = ClientLogTailReply {
            ok: true,
            lines: 2,
            truncated: false,
            content: "one\ntwo".into(),
        };
        assert_eq!(validate_client_reply(&reply, 2).unwrap(), "one\ntwo");
        assert!(validate_client_reply(&reply, 1).is_err());
    }

    #[tokio::test]
    async fn limiter_rejects_immediate_repeat_for_one_client() {
        let limiter = Arc::new(ClientLogAccessLimiter::default());
        drop(limiter.try_acquire("client-a").await.unwrap());
        assert!(matches!(
            limiter.try_acquire("client-a").await,
            Err(AppError::RateLimited { .. })
        ));
    }
}
