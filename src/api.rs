use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{any, get, post};
use axum::{
    Json, Router,
    response::{Html, Redirect},
};

use crate::ServerState;
use crate::error::AppError;
use crate::models::{
    DashboardResponse, HealthResponse, IssueLeaseRequest, IssueLeaseResponse,
    RegisterInstallationRequest, RegisterInstallationResponse, ShareBatchSyncRequest,
    ShareDeleteRequest, ShareSyncRequest,
};
use crate::proxy::proxy_handler;

pub fn router(state: ServerState) -> Router {
    Router::new()
        .route("/", get(root))
        .route("/favicon.ico", get(favicon))
        .route("/v1/healthz", get(health))
        .route("/v1/dashboard", get(dashboard))
        .route("/v1/installations/register", post(register_installation))
        .route("/v1/tunnels/lease", post(issue_lease))
        .route("/v1/shares/sync", post(sync_share))
        .route("/v1/shares/batch-sync", post(batch_sync_share))
        .route("/v1/shares/delete", post(delete_share))
        .route("/admin", get(admin_page))
        .route("/admin/login", get(root))
        .route("/*path", any(proxy_handler))
        .with_state(state)
}

async fn root() -> Redirect {
    Redirect::to("/admin")
}

async fn favicon() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { ok: true })
}

async fn register_installation(
    State(state): State<ServerState>,
    Json(input): Json<RegisterInstallationRequest>,
) -> Result<Json<RegisterInstallationResponse>, AppError> {
    let response = state.store.register_installation(input).await?;
    Ok(Json(response))
}

async fn issue_lease(
    State(state): State<ServerState>,
    Json(input): Json<IssueLeaseRequest>,
) -> Result<Json<IssueLeaseResponse>, AppError> {
    let response = state
        .store
        .issue_lease(&state.config, &state.proxy, input)
        .await?;
    Ok(Json(response))
}

async fn dashboard(
    State(state): State<ServerState>,
) -> Result<Json<DashboardResponse>, AppError> {
    Ok(Json(state.store.dashboard_snapshot(&state.proxy).await?))
}

async fn admin_page() -> Html<&'static str> {
    Html(include_str!("ui/dashboard.html"))
}

async fn sync_share(
    State(state): State<ServerState>,
    Json(input): Json<ShareSyncRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    state.store.sync_share(input).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn delete_share(
    State(state): State<ServerState>,
    Json(input): Json<ShareDeleteRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    state.store.delete_share(input).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn batch_sync_share(
    State(state): State<ServerState>,
    Json(input): Json<ShareBatchSyncRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    state.store.batch_sync_shares(input).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
