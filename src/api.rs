use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Query, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::ServerState;
use crate::client_meta::extract_client_metadata;
use crate::error::AppError;
use crate::models::{
    BindInstallationOwnerEmailRequest, BindInstallationOwnerEmailResponse,
    ChangeInstallationOwnerEmailRequest, ChangeInstallationOwnerEmailResponse,
    DashboardPresenceRequest, DashboardPresenceResponse, DashboardResponse,
    GetInstallationOwnerEmailQuery, GetInstallationOwnerEmailResponse, HealthResponse,
    IssueLeaseRequest, IssueLeaseResponse, MarketNotificationEmailLogView,
    MarketNotificationEmailRequest, MarketNotificationEmailResponse, MarketShareView,
    MarketsResponse, PublicMapPointsResponse, RefreshSessionRequest, RegisterInstallationRequest,
    RegisterInstallationResponse, RegisterMarketRequest, RequestEmailCodeRequest,
    RequestEmailCodeResponse, SessionStatusResponse, ShareBatchSyncRequest,
    ShareClaimSubdomainRequest, ShareDeleteRequest, ShareHeartbeatRequest,
    ShareRequestLogBatchSyncRequest, ShareSyncRequest, VerifyEmailCodeRequest,
    VerifyEmailCodeResponse,
};
use crate::proxy::{market_proxy_handler, proxy_handler};
use crate::recent_traffic::{RecentRequestEvent, RecentTrafficSnapshot};

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
        .route("/", any(root_handler))
        .route("/assets/world-map.svg", get(world_map_svg))
        .route("/favicon.ico", get(favicon))
        .route("/v1/healthz", get(health))
        .route("/v1/dashboard", get(dashboard))
        .route("/v1/markets", get(markets))
        .route("/v1/markets/register", post(register_market))
        .route("/v1/market/shares", get(market_shares))
        .route("/v1/market/notifications/email", post(send_market_notification_email))
        .route("/v1/market/notifications/emails", get(list_market_notification_emails))
        .route("/v1/markets/tunnel/lease", post(issue_market_lease))
        .route("/v1/public/map-points", get(public_map_points))
        .route("/v1/regions", get(regions))
        .route("/v1/dashboard/presence", post(dashboard_presence))
        .route("/v1/installations/register", post(register_installation))
        .route(
            "/v1/installations/bind-owner-email",
            post(bind_installation_owner_email),
        )
        .route(
            "/v1/installations/change-owner-email",
            post(change_installation_owner_email),
        )
        .route(
            "/v1/installations/owner-email",
            get(get_installation_owner_email),
        )
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
        .route("/_market/proxy/:share_id/*path", any(market_proxy_handler))
        .route("/*path", any(proxy_handler))
        .with_state(state)
}

async fn favicon() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { ok: true })
}

async fn markets(State(state): State<ServerState>) -> Result<Json<MarketsResponse>, AppError> {
    Ok(Json(MarketsResponse {
        markets: state.store.list_public_markets().await?,
    }))
}

async fn register_market(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<RegisterMarketRequest>,
) -> Result<Json<crate::models::PublicMarketConfig>, AppError> {
    let email = require_session_email(&state, &headers).await?;
    Ok(Json(state.store.register_market(&email, input).await?))
}

async fn market_shares(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Vec<MarketShareView>>, AppError> {
    let market = authenticate_market(&state, &headers, "market:shares:read").await?;
    let active_subdomains = state.proxy.active_subdomains().await.into_iter().collect();
    let inflight_by_share = state.proxy.inflight_by_share().await;
    let shares = state
        .store
        .list_market_shares(
            &market.email,
            "main",
            &active_subdomains,
            &inflight_by_share,
        )
        .await?;
    Ok(Json(shares))
}

async fn issue_market_lease(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<IssueLeaseResponse>, AppError> {
    let market = authenticate_market(&state, &headers, "market:proxy:use").await?;
    let market_email = market.email.clone();
    let market_subdomain = market.subdomain.clone();
    let mut response = match state
        .store
        .issue_market_lease(&state.config, &state.proxy, &market)
        .await
    {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(
                market_email = %market_email,
                requested_subdomain = %market_subdomain,
                error = %err,
                "market tunnel lease rejected"
            );
            return Err(err);
        }
    };
    response.ssh_host_fingerprint = state.ssh_host_fingerprint.clone();
    tracing::info!(
        market_email = %market_email,
        subdomain = %response.subdomain,
        connection_id = %response.connection_id,
        ssh_addr = %response.ssh_addr,
        "market tunnel lease issued"
    );
    Ok(Json(response))
}

async fn send_market_notification_email(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<MarketNotificationEmailRequest>,
) -> Result<Json<MarketNotificationEmailResponse>, AppError> {
    let market = authenticate_market(&state, &headers, "market:email:notify").await?;
    Ok(Json(
        state
            .store
            .send_market_notification_email(&state.config, state.resend.as_deref(), &market, input)
            .await?,
    ))
}

async fn list_market_notification_emails(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Vec<MarketNotificationEmailLogView>>, AppError> {
    let market = authenticate_market(&state, &headers, "market:email:notify").await?;
    Ok(Json(
        state
            .store
            .list_market_notification_emails(&market.email)
            .await?,
    ))
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

async fn bind_installation_owner_email(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<BindInstallationOwnerEmailRequest>,
) -> Result<Json<BindInstallationOwnerEmailResponse>, AppError> {
    Ok(Json(
        state
            .store
            .bind_installation_owner_email(&state.config, input, extract_bearer_token(&headers))
            .await?,
    ))
}

async fn change_installation_owner_email(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ChangeInstallationOwnerEmailRequest>,
) -> Result<Json<ChangeInstallationOwnerEmailResponse>, AppError> {
    Ok(Json(
        state
            .store
            .change_installation_owner_email(input, extract_bearer_token(&headers))
            .await?,
    ))
}

async fn get_installation_owner_email(
    State(state): State<ServerState>,
    Query(query): Query<GetInstallationOwnerEmailQuery>,
) -> Result<Json<GetInstallationOwnerEmailResponse>, AppError> {
    Ok(Json(
        state
            .store
            .get_installation_owner_email_status(query)
            .await?,
    ))
}

async fn issue_lease(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<IssueLeaseRequest>,
) -> Result<Json<IssueLeaseResponse>, AppError> {
    let metadata = extract_client_metadata(&headers, addr);
    let client_ip = metadata.ip.clone().unwrap_or_else(|| addr.ip().to_string());
    let client_country = metadata.country_code.clone().unwrap_or_else(|| "-".into());
    let requested_subdomain = input.requested_subdomain.clone();
    let installation_id = input.installation_id.clone();
    let share_id = input.share.as_ref().map(|share| share.share_id.clone());
    let mut response = match state
        .store
        .issue_lease(&state.config, &state.proxy, input, metadata, None)
        .await
    {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(
                installation_id = %installation_id,
                requested_subdomain = %requested_subdomain,
                share_id = share_id.as_deref().unwrap_or("-"),
                client_ip = %client_ip,
                client_country = %client_country,
                error = %err,
                "client tunnel lease rejected"
            );
            return Err(err);
        }
    };
    response.ssh_host_fingerprint = state.ssh_host_fingerprint.clone();
    tracing::info!(
        installation_id = %installation_id,
        requested_subdomain = %requested_subdomain,
        subdomain = %response.subdomain,
        share_id = share_id.as_deref().unwrap_or("-"),
        connection_id = %response.connection_id,
        ssh_addr = %response.ssh_addr,
        client_ip = %client_ip,
        client_country = %client_country,
        "client tunnel lease issued"
    );
    Ok(Json(response))
}

async fn dashboard(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<DashboardResponse>, AppError> {
    let mut response = state
        .store
        .dashboard_snapshot(
            &state.config,
            &state.server_geo,
            &state.proxy,
            extract_session_email(&state, &headers).await?.as_deref(),
        )
        .await?;
    let snapshot = state.recent_traffic.snapshot().await;
    let (confirmed_events, confirmed_country_counts) =
        confirmed_request_events(&snapshot, &response);
    response.user_country_counts = confirmed_country_counts;
    response.recent_request_events = confirmed_events;
    Ok(Json(response))
}

fn confirmed_request_events(
    snapshot: &RecentTrafficSnapshot,
    response: &DashboardResponse,
) -> (Vec<RecentRequestEvent>, HashMap<String, usize>) {
    let request_log_ids = response
        .ticker_shares
        .iter()
        .flat_map(|share| share.recent_requests.iter())
        .map(|log| log.request_id.as_str())
        .collect::<HashSet<_>>();
    let confirmed_events = snapshot
        .events
        .iter()
        .filter(|event| request_log_ids.contains(event.request_id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let mut country_counts = HashMap::new();
    for event in &confirmed_events {
        if let Some(iso3) = event.user_country_iso3.as_deref() {
            *country_counts.entry(iso3.to_string()).or_insert(0) += 1;
        }
    }
    let take_from = confirmed_events.len().saturating_sub(64);
    let events = confirmed_events.into_iter().skip(take_from).collect();
    (events, country_counts)
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
    let email_sent_24h = state.store.count_sent_emails_last_24h().await?;
    Ok(Json(DashboardPresenceResponse {
        online_count,
        email_sent_24h,
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
    Ok(Json(
        state.store.verify_email_code(&state.config, input).await?,
    ))
}

async fn refresh_session(
    State(state): State<ServerState>,
    Json(input): Json<RefreshSessionRequest>,
) -> Result<Json<VerifyEmailCodeResponse>, AppError> {
    Ok(Json(
        state.store.refresh_session(&state.config, input).await?,
    ))
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

async fn root_handler(
    State(state): State<ServerState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if state
        .proxy
        .backend_for_host(&host, &state.config.tunnel_domain)
        .await
        .is_some()
    {
        return proxy_handler(State(state), ConnectInfo(peer), req).await;
    }

    if matches!(*req.method(), Method::GET | Method::HEAD) {
        return Html(include_str!("ui/dashboard.html")).into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

const WORLD_MAP_SVG: &str = include_str!("ui/world-map.svg");

fn world_map_etag() -> &'static str {
    use std::sync::OnceLock;
    static ETAG: OnceLock<String> = OnceLock::new();
    ETAG.get_or_init(|| {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(WORLD_MAP_SVG.as_bytes());
        let digest = hasher.finalize();
        let hex: String = digest
            .iter()
            .take(8)
            .map(|b| format!("{:02x}", b))
            .collect();
        format!("\"wm-{}\"", hex)
    })
}

async fn world_map_svg(headers: HeaderMap) -> axum::response::Response {
    use axum::response::IntoResponse;
    let etag = world_map_etag();
    if let Some(if_none_match) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        && if_none_match == etag
    {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=2592000"),
            (header::ETAG, etag),
        ],
        WORLD_MAP_SVG,
    )
        .into_response()
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
        .sync_share(
            input,
            extract_client_metadata(&headers, addr),
            &current_user_email,
        )
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
        .claim_share_subdomain(
            &state.config,
            input,
            extract_client_metadata(&headers, addr),
            "",
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
    Json(input): Json<ShareDeleteRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    state.store.delete_share(input, "").await?;
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
        .batch_sync_shares(input, extract_client_metadata(&headers, addr), "")
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
        .batch_sync_share_request_logs(input, extract_client_metadata(&headers, addr), "")
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

fn extract_bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

async fn authenticate_market(
    state: &ServerState,
    headers: &HeaderMap,
    required_scope: &str,
) -> Result<crate::models::MarketRegistryRecord, AppError> {
    let token = extract_bearer_token(headers)
        .ok_or_else(|| AppError::Unauthorized("missing market session bearer token".into()))?;
    state
        .store
        .authenticate_market_session(token, required_scope)
        .await
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

async fn require_session_email(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<String, AppError> {
    extract_session_email(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("authenticated owner session required".into()))
}
