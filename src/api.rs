use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::{any, get, post};
use axum::{Json, Router, response::Html};

use crate::ServerState;
use crate::error::AppError;
use crate::models::{
    ClientMetadata, DashboardPresenceRequest, DashboardPresenceResponse, DashboardResponse,
    HealthResponse, IssueLeaseRequest, IssueLeaseResponse, RegisterInstallationRequest,
    RegisterInstallationResponse, ShareBatchSyncRequest, ShareClaimSubdomainRequest,
    ShareDeleteRequest, ShareHeartbeatRequest, ShareRequestLogBatchSyncRequest, ShareSyncRequest,
};
use crate::proxy::proxy_handler;

const REGIONS: &str = include_str!("../regions");

pub fn router(state: ServerState) -> Router {
    Router::new()
        .route("/", get(admin_page))
        .route("/assets/world-map.svg", get(world_map_svg))
        .route("/favicon.ico", get(favicon))
        .route("/v1/healthz", get(health))
        .route("/v1/dashboard", get(dashboard))
        .route("/v1/regions", get(regions))
        .route("/v1/dashboard/presence", post(dashboard_presence))
        .route("/v1/installations/register", post(register_installation))
        .route("/v1/tunnels/lease", post(issue_lease))
        .route("/v1/shares/claim-subdomain", post(claim_share_subdomain))
        .route("/v1/shares/sync", post(sync_share))
        .route("/v1/shares/batch-sync", post(batch_sync_share))
        .route(
            "/v1/share-request-logs/batch-sync",
            post(batch_sync_share_request_logs),
        )
        .route("/v1/shares/heartbeat", post(share_heartbeat))
        .route("/v1/shares/delete", post(delete_share))
        .route("/*path", any(proxy_handler))
        .with_state(state)
}

async fn favicon() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { ok: true })
}

async fn register_installation(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<RegisterInstallationRequest>,
) -> Result<Json<RegisterInstallationResponse>, AppError> {
    let response = state
        .store
        .register_installation(input, extract_client_metadata(&headers, addr))
        .await?;
    Ok(Json(response))
}

async fn issue_lease(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<IssueLeaseRequest>,
) -> Result<Json<IssueLeaseResponse>, AppError> {
    let response = state
        .store
        .issue_lease(
            &state.config,
            &state.proxy,
            input,
            extract_client_metadata(&headers, addr),
        )
        .await?;
    Ok(Json(response))
}

async fn dashboard(State(state): State<ServerState>) -> Result<Json<DashboardResponse>, AppError> {
    Ok(Json(
        state
            .store
            .dashboard_snapshot(&state.config, &state.server_geo, &state.proxy)
            .await?,
    ))
}

async fn regions() -> Result<Json<Vec<String>>, AppError> {
    let regions = REGIONS
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    Ok(Json(regions))
}

async fn dashboard_presence(
    State(state): State<ServerState>,
    Json(input): Json<DashboardPresenceRequest>,
) -> Result<Json<DashboardPresenceResponse>, AppError> {
    let online_count = state.store.record_dashboard_presence(input).await?;
    Ok(Json(DashboardPresenceResponse { online_count }))
}

async fn admin_page() -> Html<&'static str> {
    Html(include_str!("ui/dashboard.html"))
}

async fn world_map_svg() -> impl axum::response::IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/svg+xml; charset=utf-8")],
        include_str!("ui/world-map.svg"),
    )
}

async fn sync_share(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ShareSyncRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    state
        .store
        .sync_share(input, extract_client_metadata(&headers, addr))
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn claim_share_subdomain(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ShareClaimSubdomainRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    state
        .store
        .claim_share_subdomain(input, extract_client_metadata(&headers, addr))
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn share_heartbeat(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ShareHeartbeatRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    state
        .store
        .record_share_heartbeat(input, extract_client_metadata(&headers, addr))
        .await?;
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
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ShareBatchSyncRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    state
        .store
        .batch_sync_shares(input, extract_client_metadata(&headers, addr))
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn batch_sync_share_request_logs(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ShareRequestLogBatchSyncRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    state
        .store
        .batch_sync_share_request_logs(input, extract_client_metadata(&headers, addr))
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

fn extract_client_metadata(headers: &HeaderMap, addr: SocketAddr) -> ClientMetadata {
    let forwarded_ip = headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .or_else(|| {
            headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(',').next())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        })
        .or_else(|| Some(addr.ip().to_string()));

    let country_code = headers
        .get("cf-ipcountry")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| v.len() == 2 && *v != "XX" && *v != "T1")
        .map(|v| v.to_ascii_uppercase());

    ClientMetadata {
        ip: forwarded_ip,
        country_code,
    }
}
