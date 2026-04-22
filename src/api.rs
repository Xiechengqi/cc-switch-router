use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::{any, get, post};
use axum::{Json, Router, response::Html};
use serde::{Deserialize, Serialize};

use crate::ServerState;
use crate::error::AppError;
use crate::models::{
    ClientMetadata, DashboardPresenceRequest, DashboardPresenceResponse, DashboardResponse,
    HealthResponse, IssueLeaseRequest, IssueLeaseResponse, PublicMapPointsResponse,
    RefreshSessionRequest, RegisterInstallationRequest, RegisterInstallationResponse,
    RequestEmailCodeRequest, RequestEmailCodeResponse, SessionStatusResponse,
    ShareBatchSyncRequest, ShareClaimSubdomainRequest, ShareDeleteRequest, ShareHeartbeatRequest,
    ShareRequestLogBatchSyncRequest, ShareSyncRequest, VerifyEmailCodeRequest,
    VerifyEmailCodeResponse,
};
use crate::proxy::proxy_handler;

const REGIONS: &str = include_str!("../regions");

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegionOption {
    name: String,
    url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionStatusQuery {
    installation_id: Option<String>,
}

pub fn router(state: ServerState) -> Router {
    Router::new()
        .route("/", get(admin_page))
        .route("/assets/world-map.svg", get(world_map_svg))
        .route("/favicon.ico", get(favicon))
        .route("/v1/healthz", get(health))
        .route("/v1/dashboard", get(dashboard))
        .route("/v1/public/map-points", get(public_map_points))
        .route("/v1/regions", get(regions))
        .route("/v1/dashboard/presence", post(dashboard_presence))
        .route("/v1/installations/register", post(register_installation))
        .route("/v1/auth/email/request-code", post(request_email_code))
        .route("/v1/auth/email/verify-code", post(verify_email_code))
        .route("/v1/auth/session/refresh", post(refresh_session))
        .route("/v1/auth/session/me", get(session_me))
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
    let mut response = state
        .store
        .issue_lease(
            &state.config,
            &state.proxy,
            input,
            extract_client_metadata(&headers, addr),
            extract_session_email(&state, &headers).await?.as_deref(),
        )
        .await?;
    response.ssh_host_fingerprint = state.ssh_host_fingerprint.clone();
    Ok(Json(response))
}

async fn dashboard(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<DashboardResponse>, AppError> {
    Ok(Json(
        state
            .store
            .dashboard_snapshot(
                &state.config,
                &state.server_geo,
                &state.proxy,
                extract_session_email(&state, &headers).await?.as_deref(),
            )
            .await?,
    ))
}

async fn public_map_points(
    State(state): State<ServerState>,
) -> Result<Json<PublicMapPointsResponse>, AppError> {
    Ok(Json(
        state.store.public_map_points(&state.server_geo).await?,
    ))
}

async fn regions() -> Result<Json<Vec<RegionOption>>, AppError> {
    let regions = REGIONS
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (name, url) = line
                .split_once(':')
                .ok_or_else(|| AppError::Internal(format!("invalid region entry: {line}")))?;
            let name = name.trim();
            let url = url.trim();
            if name.is_empty() || url.is_empty() {
                return Err(AppError::Internal(format!("invalid region entry: {line}")));
            }
            Ok(RegionOption {
                name: name.to_string(),
                url: url.to_string(),
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(regions))
}

async fn dashboard_presence(
    State(state): State<ServerState>,
    Json(input): Json<DashboardPresenceRequest>,
) -> Result<Json<DashboardPresenceResponse>, AppError> {
    let online_count = state.store.record_dashboard_presence(input).await?;
    let resend_usage_label = state
        .resend_usage_cache
        .lock()
        .await
        .as_ref()
        .and_then(|cache| {
            if cache.value.available && !cache.value.daily_usage_label.is_empty() {
                Some(cache.value.daily_usage_label.clone())
            } else {
                None
            }
        });
    Ok(Json(DashboardPresenceResponse {
        online_count,
        resend_usage_label,
    }))
}

async fn request_email_code(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<RequestEmailCodeRequest>,
) -> Result<Json<RequestEmailCodeResponse>, AppError> {
    Ok(Json(
        state
            .store
            .request_email_code(
                &state.config,
                state.resend.as_deref(),
                input,
                extract_client_metadata(&headers, addr),
            )
            .await?,
    ))
}

async fn verify_email_code(
    State(state): State<ServerState>,
    Json(input): Json<VerifyEmailCodeRequest>,
) -> Result<Json<VerifyEmailCodeResponse>, AppError> {
    Ok(Json(state.store.verify_email_code(&state.config, input).await?))
}

async fn refresh_session(
    State(state): State<ServerState>,
    Json(input): Json<RefreshSessionRequest>,
) -> Result<Json<VerifyEmailCodeResponse>, AppError> {
    Ok(Json(state.store.refresh_session(&state.config, input).await?))
}

async fn session_me(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<SessionStatusQuery>,
) -> Result<Json<SessionStatusResponse>, AppError> {
    Ok(Json(
        state
            .store
            .session_status(
                extract_bearer_token(&headers),
                query.installation_id.as_deref(),
            )
            .await?,
    ))
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
    let current_user_email = require_session_email(&state, &headers).await?;
    state
        .store
        .sync_share(input, extract_client_metadata(&headers, addr), &current_user_email)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn claim_share_subdomain(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ShareClaimSubdomainRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let current_user_email = require_session_email(&state, &headers).await?;
    state
        .store
        .claim_share_subdomain(
            input,
            extract_client_metadata(&headers, addr),
            &current_user_email,
        )
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
    headers: HeaderMap,
    Json(input): Json<ShareDeleteRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let current_user_email = require_session_email(&state, &headers).await?;
    state.store.delete_share(input, &current_user_email).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn batch_sync_share(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ShareBatchSyncRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let current_user_email = require_session_email(&state, &headers).await?;
    state
        .store
        .batch_sync_shares(
            input,
            extract_client_metadata(&headers, addr),
            &current_user_email,
        )
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn batch_sync_share_request_logs(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ShareRequestLogBatchSyncRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let current_user_email = require_session_email(&state, &headers).await?;
    state
        .store
        .batch_sync_share_request_logs(
            input,
            extract_client_metadata(&headers, addr),
            &current_user_email,
        )
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

fn extract_bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

async fn extract_session_email(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<Option<String>, AppError> {
    let Some(token) = extract_bearer_token(headers) else {
        return Ok(None);
    };
    Ok(state
        .store
        .resolve_session_by_access_token(token)
        .await?
        .map(|session| session.email))
}

async fn require_session_email(state: &ServerState, headers: &HeaderMap) -> Result<String, AppError> {
    extract_session_email(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("authenticated owner session required".into()))
}
