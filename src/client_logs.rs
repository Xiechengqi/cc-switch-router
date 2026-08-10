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
use crate::error::AppError;

const PUBLIC_LOG_LINE_LIMIT: usize = 10;
const FULL_LOG_LINE_LIMIT: usize = 100;
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

pub(crate) struct ClientLogAccessPermit {
    _global: OwnedSemaphorePermit,
    _client: OwnedSemaphorePermit,
}

impl ClientLogAccessLimiter {
    pub(crate) async fn try_acquire(
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
    let exists = state
        .store
        .client_log_installation_exists(installation_id)
        .await?;
    if !exists {
        return Err(AppError::NotFound("Client not found".into()));
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
    if !full_log_access && !state.dynamic.read().await.server_log_public_enabled {
        return Err(AppError::Forbidden(
            "public server logs are disabled".into(),
        ));
    }
    let limit = if full_log_access {
        FULL_LOG_LINE_LIMIT
    } else {
        PUBLIC_LOG_LINE_LIMIT
    };
    let _permit = state.client_logs.try_acquire(installation_id).await?;
    let tail = state
        .server_logs
        .client_text_tail(installation_id, !full_log_access, limit)
        .await?;
    Ok(ClientLogsResponse {
        installation_id: installation_id.to_string(),
        content: tail.content,
        lines: tail.lines,
        limit,
        truncated: tail.truncated,
        full_log_access,
        fetched_at: chrono::Utc::now().to_rfc3339(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
