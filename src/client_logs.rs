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

const OWNER_LOG_LINE_LIMIT: usize = 100;
const PUBLIC_LOG_LINE_LIMIT: usize = 10;
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
    let session = crate::api::resolve_router_session(state, headers).await?;
    let is_client_owner = match session.as_ref() {
        Some(session) => {
            state
                .store
                .is_verified_installation_owner(installation_id, &session.email)
                .await?
        }
        None => false,
    };
    let line_limit = client_log_line_limit(is_client_owner);
    if !state
        .store
        .client_log_installation_exists(installation_id)
        .await?
    {
        return Err(AppError::NotFound("Client not found".into()));
    }
    let _permit = state.client_logs.try_acquire(installation_id).await?;
    let route = state
        .proxy
        .active_client_route_for_installation(installation_id)
        .await
        .ok_or_else(|| AppError::ServiceUnavailable("Client is offline or reconnecting".into()))?;
    let control_secret = state
        .store
        .installation_control_secret(installation_id)
        .await?
        .ok_or_else(|| AppError::ServiceUnavailable("Client control is unavailable".into()))?;
    let reply = crate::ctl_client::fetch_client_log_tail(
        route.route_target(),
        installation_id,
        &control_secret,
        line_limit,
    )
    .await
    .map_err(map_control_error)?;
    client_logs_response(installation_id, line_limit, reply)
}

fn client_log_line_limit(is_client_owner: bool) -> usize {
    if is_client_owner {
        OWNER_LOG_LINE_LIMIT
    } else {
        PUBLIC_LOG_LINE_LIMIT
    }
}

fn map_control_error(error: CtlError) -> AppError {
    match error {
        CtlError::Rejected { status: 403, .. } => AppError::Conflict(
            "Client log collection is disabled; enable INFO log collection on the Server".into(),
        ),
        error if error.is_transport() => {
            AppError::ServiceUnavailable("Client process logs are temporarily unavailable".into())
        }
        error => AppError::Internal(format!("read Client process logs failed: {error}")),
    }
}

fn client_logs_response(
    installation_id: &str,
    line_limit: usize,
    reply: ClientLogTailReply,
) -> Result<ClientLogsResponse, AppError> {
    let content_lines = reply.content.lines().count();
    if reply.lines != content_lines
        || reply.lines > line_limit
        || reply
            .content
            .lines()
            .any(|line| line.len() > LOG_LINE_MAX_BYTES)
    {
        return Err(AppError::Internal(
            "Client returned an invalid process log response".into(),
        ));
    }
    Ok(ClientLogsResponse {
        installation_id: installation_id.to_string(),
        content: reply.content,
        lines: reply.lines,
        limit: line_limit,
        truncated: reply.truncated,
        fetched_at: chrono::Utc::now().to_rfc3339(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_preserves_server_process_log_content() {
        let content = concat!(
            "2026-08-10T10:36:57.612Z  INFO cc_switch_server::state: server process log started ",
            "process_id=42 provider=\"openai official\"\n",
            "2026-08-10T10:36:58.612Z  WARN cc_switch_server::clients::router: retry  ",
            "attempt=2"
        );
        let response = client_logs_response(
            "installation-a",
            OWNER_LOG_LINE_LIMIT,
            ClientLogTailReply {
                ok: true,
                lines: 2,
                truncated: false,
                content: content.to_string(),
            },
        )
        .unwrap();

        assert_eq!(response.content, content);
        assert_eq!(response.lines, 2);
        assert_eq!(response.limit, OWNER_LOG_LINE_LIMIT);
    }

    #[test]
    fn process_log_limit_is_full_for_owner_and_ten_for_everyone_else() {
        assert_eq!(client_log_line_limit(true), OWNER_LOG_LINE_LIMIT);
        assert_eq!(client_log_line_limit(false), PUBLIC_LOG_LINE_LIMIT);
    }

    #[test]
    fn response_rejects_inconsistent_line_metadata_without_reformatting() {
        let result = client_logs_response(
            "installation-a",
            PUBLIC_LOG_LINE_LIMIT,
            ClientLogTailReply {
                ok: true,
                lines: 1,
                truncated: false,
                content: "first\nsecond".into(),
            },
        );

        assert!(matches!(result, Err(AppError::Internal(_))));
    }

    #[test]
    fn control_error_mapping_distinguishes_disabled_and_transport_failures() {
        assert!(matches!(
            map_control_error(CtlError::Rejected {
                status: 403,
                body: "disabled".into(),
                message: None,
                code: None,
                retryable: Some(false),
                current_config_revision: None,
                current_share: None,
            }),
            AppError::Conflict(_)
        ));
        assert!(matches!(
            map_control_error(CtlError::Timeout),
            AppError::ServiceUnavailable(_)
        ));
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
