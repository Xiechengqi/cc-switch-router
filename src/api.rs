use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue};
use axum::routing::{any, get, post};
use axum::{
    Json, Router,
    response::{Html, IntoResponse, Redirect, Response},
};

use crate::ServerState;
use crate::error::AppError;
use crate::models::{
    AdminLoginRequest, DashboardResponse, HealthResponse, IssueLeaseRequest, IssueLeaseResponse,
    RegisterInstallationRequest, RegisterInstallationResponse, ShareBatchSyncRequest,
    ShareDeleteRequest, ShareSyncRequest,
};
use crate::proxy::proxy_handler;

pub fn router(state: ServerState) -> Router {
    Router::new()
        .route("/v1/healthz", get(health))
        .route("/v1/dashboard", get(dashboard))
        .route("/v1/installations/register", post(register_installation))
        .route("/v1/tunnels/lease", post(issue_lease))
        .route("/v1/shares/sync", post(sync_share))
        .route("/v1/shares/batch-sync", post(batch_sync_share))
        .route("/v1/shares/delete", post(delete_share))
        .route("/admin", get(admin_page))
        .route("/admin/login", get(admin_login_page))
        .route("/v1/admin/login", post(admin_login))
        .route("/v1/admin/logout", post(admin_logout))
        .route("/", any(proxy_handler))
        .route("/*path", any(proxy_handler))
        .with_state(state)
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
    headers: HeaderMap,
) -> Result<Json<DashboardResponse>, AppError> {
    if !is_admin_authorized(&state, &headers) {
        return Err(AppError::Unauthorized("admin auth required".into()));
    }
    Ok(Json(state.store.dashboard_snapshot().await?))
}

async fn admin_page(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if !is_admin_authorized(&state, &headers) {
        return Ok(Redirect::to("/admin/login").into_response());
    }
    Ok(Html(include_str!("ui/dashboard.html")).into_response())
}

async fn admin_login_page() -> Html<&'static str> {
    Html(include_str!("ui/login.html"))
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

async fn admin_login(
    State(state): State<ServerState>,
    Json(input): Json<AdminLoginRequest>,
) -> Result<Response, AppError> {
    if input.token != state.config.admin_token {
        return Err(AppError::Unauthorized("invalid admin token".into()));
    }
    let mut response = Json(serde_json::json!({ "ok": true })).into_response();
    response.headers_mut().insert(
        "set-cookie",
        HeaderValue::from_str(&format!(
            "portr_rs_admin={}; Path=/; HttpOnly; SameSite=Lax",
            state.config.admin_token
        ))
        .map_err(|e| AppError::Internal(format!("set cookie failed: {e}")))?,
    );
    Ok(response)
}

async fn admin_logout() -> Response {
    let mut response = Json(serde_json::json!({ "ok": true })).into_response();
    response.headers_mut().insert(
        "set-cookie",
        HeaderValue::from_static("portr_rs_admin=; Path=/; HttpOnly; Max-Age=0; SameSite=Lax"),
    );
    response
}

fn is_admin_authorized(state: &ServerState, headers: &HeaderMap) -> bool {
    if headers
        .get("x-admin-token")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|token| token == state.config.admin_token)
    {
        return true;
    }

    headers
        .get("cookie")
        .and_then(|value| value.to_str().ok())
        .map(|cookies| {
            cookies.split(';').any(|cookie| {
                let cookie = cookie.trim();
                cookie == format!("portr_rs_admin={}", state.config.admin_token)
            })
        })
        .unwrap_or(false)
}
