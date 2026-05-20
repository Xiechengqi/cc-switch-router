use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::{any, delete, get, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::time::{Duration, sleep};

use crate::ServerState;
use crate::admin::{
    restart::{RestartStrategy, schedule_restart},
    settings::{
        SettingsSchemaResponse, SettingsUpdateRequest, SettingsUpdateResponse,
        SettingsValuesResponse, apply_updates_to_dynamic, read_env_file, schema_response,
        validate_and_diff, values_response, write_env_file_atomic,
    },
    upgrade::{UpgradeLogEntry, UpgradeStatus},
    version::{
        BINARY_INSTALL_PATH, BINARY_ROLLBACK_PATH, SERVICE_UNIT, ServiceManager, VersionResponse,
        build_info, detect_service_status, ensure_binary_writable, fetch_latest_release_meta,
        uptime_secs_from,
    },
};
use crate::client_meta::extract_client_metadata;
use crate::dynamic_settings::DynamicSettings;
use crate::error::AppError;
use crate::models::{
    AuthSession, BindInstallationOwnerEmailRequest, BindInstallationOwnerEmailResponse,
    BoardMessageListResponse, BoardMessageToggleRequest, BoardMessageView, BoardMetaResponse,
    ChangeInstallationOwnerEmailRequest, ChangeInstallationOwnerEmailResponse,
    DashboardMarketRequestLogView, DashboardPresenceRequest, DashboardPresenceResponse,
    DashboardResponse, DashboardTickerShare, GetInstallationOwnerEmailQuery,
    GetInstallationOwnerEmailResponse, HealthResponse, IssueLeaseRequest, IssueLeaseResponse,
    MarketDisabledSharesUpdateRequest, MarketDisabledSharesUpdateResponse,
    MarketNotificationEmailLogView, MarketNotificationEmailRequest,
    MarketNotificationEmailResponse, MarketRequestLogBatchSyncRequest, MarketShareView,
    MarketsResponse, PostBoardMessageRequest, PublicMapPointsResponse, RefreshSessionRequest,
    RegisterInstallationRequest, RegisterInstallationResponse, RegisterMarketRequest,
    RequestEmailCodeRequest, RequestEmailCodeResponse, SessionStatusResponse,
    ShareBatchSyncRequest, ShareClaimSubdomainRequest, ShareDeleteRequest, ShareEditAckRequest,
    ShareEditAvailableEvent, ShareEditEventSignaturePayload, ShareHeartbeatRequest,
    SharePendingEditsRequest, ShareRequestLogBatchSyncRequest, ShareRequestLogEntry,
    ShareRuntimeRefreshRequest, ShareSettingsUpdateRequest, ShareSyncRequest,
    VerifyEmailCodeRequest, VerifyEmailCodeResponse,
};
use crate::proxy::{market_proxy_handler, proxy_handler};
use crate::recent_traffic::{RecentRequestEvent, RecentTrafficSnapshot};
use crate::store::BoardAuthor;

const REGIONS: &str = include_str!("../regions");
const SHARE_EDIT_WAKE_RETRY_INTERVAL_SECS: u64 = 20;
const SHARE_EDIT_WAKE_RETRY_ATTEMPTS: usize = 3;
const DASHBOARD_REQUEST_TICKER_LIMIT: usize = 5;

mod ui_assets {
    include!(concat!(env!("OUT_DIR"), "/ui_assets.rs"));
}

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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareEditEventsQuery {
    installation_id: String,
    timestamp_ms: i64,
    nonce: String,
    signature: String,
}

pub fn router(state: ServerState) -> Router {
    let middleware_state = state.clone();
    Router::new()
        .route("/", any(root_handler))
        .route("/favicon.ico", get(favicon))
        .route("/v1/healthz", get(health))
        .route("/v1/dashboard", get(dashboard))
        .route("/v1/markets", get(markets))
        .route("/v1/markets/register", post(register_market))
        .route("/v1/market/shares", get(market_shares))
        .route(
            "/v1/admin/markets/:market_email/linked-shares",
            get(admin_market_linked_shares),
        )
        .route(
            "/v1/admin/markets/:market_email/disabled-shares",
            patch(admin_update_market_disabled_shares),
        )
        .route(
            "/v1/market/request-logs/batch",
            post(batch_sync_market_request_logs),
        )
        .route(
            "/v1/market/notifications/email",
            post(send_market_notification_email),
        )
        .route(
            "/v1/market/notifications/emails",
            get(list_market_notification_emails),
        )
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
        .route("/v1/shares/runtime-refresh", post(refresh_share_runtime))
        .route(
            "/v1/shares/:share_id/settings",
            patch(update_share_settings),
        )
        .route("/v1/shares/pending-edits", post(pending_share_edits))
        .route("/v1/shares/edit-ack", post(ack_share_edit))
        .route("/v1/shares/edit-events", get(share_edit_events))
        .route(
            "/v1/share-request-logs/batch-sync",
            post(batch_sync_share_request_logs),
        )
        .route("/v1/shares/heartbeat", post(share_heartbeat))
        .route("/v1/shares/delete", post(delete_share))
        .route("/v1/board/messages", get(list_board_messages))
        .route("/v1/board/messages", post(post_board_message))
        .route("/v1/board/messages/:id/pin", post(pin_board_message))
        .route(
            "/v1/board/messages/:id/feature",
            post(feature_board_message),
        )
        .route("/v1/board/messages/:id", delete(delete_board_message))
        .route("/v1/board/meta", get(board_meta))
        .route("/v1/admin/settings/schema", get(admin_settings_schema))
        .route(
            "/v1/admin/settings/values",
            get(admin_settings_values).patch(admin_settings_apply),
        )
        .route("/v1/admin/version", get(admin_version))
        .route("/v1/admin/restart", post(admin_restart))
        .route("/v1/admin/upgrade", post(admin_upgrade_start))
        .route("/v1/admin/rollback", post(admin_rollback))
        .route("/v1/admin/upgrade/stream", get(admin_upgrade_stream))
        .route("/v1/admin/telegram/test", post(admin_telegram_test))
        .route("/v1/admin/audit", get(admin_audit_list))
        .route("/_market/proxy/:share_id/*path", any(market_proxy_handler))
        .route("/*path", any(ui_or_proxy_handler))
        .layer(middleware::from_fn_with_state(
            middleware_state,
            ip_blacklist_middleware,
        ))
        .with_state(state)
}

async fn ip_blacklist_middleware(
    State(state): State<ServerState>,
    req: Request,
    next: Next,
) -> Response {
    if let Some(ip) = source_ip_from_request(&req) {
        if state.dynamic.read().await.is_ip_blacklisted(ip) {
            tracing::warn!(client_ip = %ip, path = %req.uri().path(), "request blocked by IP blacklist");
            return (StatusCode::FORBIDDEN, "IP blacklisted").into_response();
        }
    }
    next.run(req).await
}

fn source_ip_from_request(req: &Request) -> Option<std::net::IpAddr> {
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>()?.0;
    let metadata = extract_client_metadata(req.headers(), peer);
    metadata.ip.as_deref()?.parse().ok()
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

async fn admin_market_linked_shares(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(market_email): Path<String>,
) -> Result<Json<Vec<MarketShareView>>, AppError> {
    let current_user_email = require_session_email(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .list_manageable_market_shares(
                &market_email,
                &current_user_email,
                &state.proxy.active_subdomains().await.into_iter().collect(),
                &state.proxy.inflight_by_share().await,
            )
            .await?,
    ))
}

async fn admin_update_market_disabled_shares(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(market_email): Path<String>,
    Json(input): Json<MarketDisabledSharesUpdateRequest>,
) -> Result<Json<MarketDisabledSharesUpdateResponse>, AppError> {
    let current_user_email = require_session_email(&state, &headers).await?;
    let response = state
        .store
        .update_market_disabled_shares(&market_email, &current_user_email, input)
        .await?;
    let metadata = extract_client_metadata(&headers, addr);
    let payload = serde_json::json!({
        "marketEmail": market_email,
        "disabledShareIds": response.disabled_share_ids.clone(),
    });
    let _ = state
        .store
        .record_admin_audit(
            Some(&current_user_email),
            "market.disabled_shares.update",
            Some(&payload),
            metadata.ip.as_deref(),
        )
        .await;
    Ok(Json(response))
}

async fn batch_sync_market_request_logs(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(mut input): Json<MarketRequestLogBatchSyncRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let market = authenticate_market(&state, &headers, "market:request_logs:write").await?;
    let snapshot = state.recent_traffic.snapshot().await;
    enrich_market_request_logs_with_live_country(&mut input.logs, &snapshot);
    let count = state
        .store
        .batch_sync_market_request_logs(&market, input)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true, "synced": count })))
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
    let mut events_by_id = persisted_ticker_request_events(response)
        .into_iter()
        .map(|event| (event.request_id.clone(), event))
        .collect::<HashMap<_, _>>();
    let request_log_ids = response
        .ticker_shares
        .iter()
        .flat_map(|share| share.recent_requests.iter())
        .map(|log| log.request_id.as_str())
        .chain(
            response
                .market_request_logs
                .iter()
                .map(|log| log.request_id.as_str()),
        )
        .collect::<HashSet<_>>();
    let confirmed_live_events = snapshot
        .events
        .iter()
        .filter(|event| request_log_ids.contains(event.request_id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    for event in &confirmed_live_events {
        events_by_id.insert(event.request_id.clone(), event.clone());
    }
    let mut events = events_by_id.into_values().collect::<Vec<_>>();
    events.sort_by(|left, right| left.started_at.cmp(&right.started_at));
    if events.len() > DASHBOARD_REQUEST_TICKER_LIMIT {
        events.drain(0..events.len() - DASHBOARD_REQUEST_TICKER_LIMIT);
    }
    let mut country_counts = HashMap::new();
    for event in &confirmed_live_events {
        if let Some(iso3) = event.user_country_iso3.as_deref() {
            *country_counts.entry(iso3.to_string()).or_insert(0) += 1;
        }
    }
    (events, country_counts)
}

fn live_country_by_request_id(
    snapshot: &RecentTrafficSnapshot,
) -> HashMap<String, (Option<String>, Option<String>)> {
    snapshot
        .events
        .iter()
        .filter(|event| event.user_country.is_some() || event.user_country_iso3.is_some())
        .map(|event| {
            (
                event.request_id.clone(),
                (event.user_country.clone(), event.user_country_iso3.clone()),
            )
        })
        .collect()
}

fn enrich_market_request_logs_with_live_country(
    logs: &mut [crate::models::MarketRequestLogEntry],
    snapshot: &RecentTrafficSnapshot,
) {
    let country_by_request_id = live_country_by_request_id(snapshot);
    for log in logs {
        if log.user_country.is_some() && log.user_country_iso3.is_some() {
            continue;
        }
        if let Some((user_country, user_country_iso3)) = country_by_request_id.get(&log.request_id)
        {
            if log.user_country.is_none() {
                log.user_country = user_country.clone();
            }
            if log.user_country_iso3.is_none() {
                log.user_country_iso3 = user_country_iso3.clone();
            }
        }
    }
}

fn persisted_ticker_request_events(response: &DashboardResponse) -> Vec<RecentRequestEvent> {
    let mut events = Vec::new();
    for share in &response.ticker_shares {
        for log in &share.recent_requests {
            events.push(share_log_to_ticker_event(share, log));
        }
    }
    for log in &response.market_request_logs {
        events.push(market_log_to_ticker_event(log));
    }
    events
}

fn share_log_to_ticker_event(
    share: &DashboardTickerShare,
    log: &ShareRequestLogEntry,
) -> RecentRequestEvent {
    RecentRequestEvent {
        request_id: log.request_id.clone(),
        share_id: log.share_id.clone(),
        share_name: Some(if log.share_name.is_empty() {
            share.share_name.clone()
        } else {
            log.share_name.clone()
        }),
        share_subdomain: Some(share.subdomain.clone()),
        user_country: log.user_country.clone(),
        user_country_iso3: log.user_country_iso3.clone(),
        started_at: chrono::DateTime::<chrono::Utc>::from_timestamp(log.created_at, 0)
            .unwrap_or_else(chrono::Utc::now),
        is_inflight: false,
    }
}

fn market_log_to_ticker_event(log: &DashboardMarketRequestLogView) -> RecentRequestEvent {
    RecentRequestEvent {
        request_id: log.request_id.clone(),
        share_id: log.share_id.clone().unwrap_or_default(),
        share_name: log.share_subdomain.clone(),
        share_subdomain: log.share_subdomain.clone(),
        user_country: log.user_country.clone(),
        user_country_iso3: log.user_country_iso3.clone(),
        started_at: parse_dashboard_log_time(&log.created_at),
        is_inflight: false,
    }
}

fn parse_dashboard_log_time(value: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now())
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
    let mut response = state
        .store
        .session_status(
            extract_bearer_token(&headers),
            query.installation_id.as_deref(),
        )
        .await?;
    if let Some(user) = response.user.as_ref() {
        response.is_admin = state.dynamic.read().await.is_admin(&user.email);
    }
    Ok(Json(response))
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
        if let Some(response) = ui_response("index.html") {
            return response;
        }
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "frontend assets are missing; run frontend build before cargo build",
        )
            .into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

async fn ui_or_proxy_handler(
    State(state): State<ServerState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    if should_proxy_host(&state, request_host(&req)).await {
        return proxy_handler(State(state), ConnectInfo(peer), req).await;
    }
    if matches!(*req.method(), Method::GET | Method::HEAD) {
        if let Some(response) = ui_response_for_request_path(req.uri().path()) {
            return response;
        }
    }
    proxy_handler(State(state), ConnectInfo(peer), req).await
}

fn request_host(req: &Request) -> String {
    req.headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

async fn should_proxy_host(state: &ServerState, host: String) -> bool {
    state
        .proxy
        .backend_for_host(&host, &state.config.tunnel_domain)
        .await
        .is_some()
}

fn ui_response_for_request_path(path: &str) -> Option<Response> {
    let trimmed = path.trim_start_matches('/');
    let candidates = [
        trimmed.to_string(),
        format!("{}/index.html", trimmed.trim_end_matches('/')),
        format!("{}index.html", trimmed),
    ];
    for candidate in candidates {
        if candidate.is_empty() {
            continue;
        }
        if let Some(response) = ui_response(&candidate) {
            return Some(response);
        }
    }
    None
}

fn ui_response(path: &str) -> Option<Response> {
    let asset = ui_assets::ui_asset(path)?;
    let cache_control = if asset.immutable {
        "public, max-age=31536000, immutable"
    } else if asset.content_type.starts_with("text/html") {
        "no-cache"
    } else {
        "public, max-age=2592000"
    };
    Response::builder()
        .header(header::CONTENT_TYPE, asset.content_type)
        .header(header::CACHE_CONTROL, cache_control)
        .header("X-UI-Asset", asset.path)
        .body(Body::from(asset.bytes))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn confirmed_request_events_accepts_market_request_logs() {
        let event = RecentRequestEvent {
            request_id: "req_market_confirmed".into(),
            share_id: "share-1".into(),
            share_name: Some("Share".into()),
            share_subdomain: Some("share-sub".into()),
            user_country: Some("US".into()),
            user_country_iso3: Some("USA".into()),
            started_at: Utc::now(),
            is_inflight: true,
        };
        let snapshot = RecentTrafficSnapshot {
            country_counts: HashMap::new(),
            events: vec![event],
            recent_events: Vec::new(),
        };
        let response = DashboardResponse {
            generated_at: Utc::now(),
            stats: crate::models::DashboardStats {
                clients: 0,
                active_shares: 0,
                total_active_requests: 0,
            },
            map: crate::models::DashboardMap {
                server: None,
                clients: Vec::new(),
            },
            clients: Vec::new(),
            markets: Vec::new(),
            ticker_shares: Vec::new(),
            country_counts: HashMap::new(),
            user_country_counts: HashMap::new(),
            recent_request_events: Vec::new(),
            market_request_logs: vec![crate::models::DashboardMarketRequestLogView {
                request_id: "req_market_confirmed".into(),
                market_id: "market-1".into(),
                market_email: "market@example.com".into(),
                market_subdomain: "market".into(),
                user_email: None,
                api_key_prefix: None,
                router_id: None,
                share_id: Some("share-1".into()),
                share_subdomain: Some("share-sub".into()),
                model: Some("gpt-5".into()),
                request_agent: "codex".into(),
                requested_model: "gpt-5".into(),
                actual_model: "gpt-5".into(),
                actual_model_source: "official".into(),
                status: "streaming".into(),
                status_code: Some(200),
                latency_ms: Some(1),
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                usage_amount_usd: None,
                created_at: Utc::now().to_rfc3339(),
                settled_at: None,
                user_country: None,
                user_country_iso3: None,
            }],
        };

        let (events, country_counts) = confirmed_request_events(&snapshot, &response);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].request_id, "req_market_confirmed");
        assert_eq!(country_counts.get("USA"), Some(&1));
    }

    #[test]
    fn confirmed_request_events_restores_last_five_from_persisted_share_logs() {
        let response = DashboardResponse {
            generated_at: Utc::now(),
            stats: crate::models::DashboardStats {
                clients: 0,
                active_shares: 0,
                total_active_requests: 0,
            },
            map: crate::models::DashboardMap {
                server: None,
                clients: Vec::new(),
            },
            clients: Vec::new(),
            markets: Vec::new(),
            ticker_shares: vec![crate::models::DashboardTickerShare {
                share_id: "share-1".into(),
                share_name: "Share".into(),
                subdomain: "share-sub".into(),
                recent_requests: (1..=7)
                    .map(|index| share_log(&format!("req-{index}"), index))
                    .collect(),
            }],
            country_counts: HashMap::new(),
            user_country_counts: HashMap::new(),
            recent_request_events: Vec::new(),
            market_request_logs: Vec::new(),
        };
        let snapshot = RecentTrafficSnapshot {
            country_counts: HashMap::new(),
            events: Vec::new(),
            recent_events: Vec::new(),
        };

        let (events, country_counts) = confirmed_request_events(&snapshot, &response);

        assert_eq!(country_counts.len(), 0);
        assert_eq!(
            events
                .iter()
                .map(|event| event.request_id.as_str())
                .collect::<Vec<_>>(),
            vec!["req-3", "req-4", "req-5", "req-6", "req-7"]
        );
        assert!(events.iter().all(|event| !event.is_inflight));
    }

    #[test]
    fn confirmed_request_events_restores_country_from_persisted_logs() {
        let mut share_log = share_log("req-country-share", 1);
        share_log.user_country = Some("JP".into());
        share_log.user_country_iso3 = Some("JPN".into());
        let response = DashboardResponse {
            generated_at: Utc::now(),
            stats: crate::models::DashboardStats {
                clients: 0,
                active_shares: 0,
                total_active_requests: 0,
            },
            map: crate::models::DashboardMap {
                server: None,
                clients: Vec::new(),
            },
            clients: Vec::new(),
            markets: Vec::new(),
            ticker_shares: vec![crate::models::DashboardTickerShare {
                share_id: "share-1".into(),
                share_name: "Share".into(),
                subdomain: "share-sub".into(),
                recent_requests: vec![share_log],
            }],
            country_counts: HashMap::new(),
            user_country_counts: HashMap::new(),
            recent_request_events: Vec::new(),
            market_request_logs: Vec::new(),
        };
        let snapshot = RecentTrafficSnapshot {
            country_counts: HashMap::new(),
            events: Vec::new(),
            recent_events: Vec::new(),
        };

        let (events, _) = confirmed_request_events(&snapshot, &response);

        assert_eq!(events[0].user_country.as_deref(), Some("JP"));
        assert_eq!(events[0].user_country_iso3.as_deref(), Some("JPN"));
    }

    #[test]
    fn confirmed_request_events_prefers_live_event_over_persisted_copy() {
        let live = RecentRequestEvent {
            request_id: "req-1".into(),
            share_id: "share-1".into(),
            share_name: Some("Live Share".into()),
            share_subdomain: Some("live-sub".into()),
            user_country: Some("US".into()),
            user_country_iso3: Some("USA".into()),
            started_at: Utc::now(),
            is_inflight: true,
        };
        let snapshot = RecentTrafficSnapshot {
            country_counts: HashMap::new(),
            events: vec![live],
            recent_events: Vec::new(),
        };
        let response = DashboardResponse {
            generated_at: Utc::now(),
            stats: crate::models::DashboardStats {
                clients: 0,
                active_shares: 0,
                total_active_requests: 0,
            },
            map: crate::models::DashboardMap {
                server: None,
                clients: Vec::new(),
            },
            clients: Vec::new(),
            markets: Vec::new(),
            ticker_shares: vec![crate::models::DashboardTickerShare {
                share_id: "share-1".into(),
                share_name: "Persisted Share".into(),
                subdomain: "persisted-sub".into(),
                recent_requests: vec![share_log("req-1", 1)],
            }],
            country_counts: HashMap::new(),
            user_country_counts: HashMap::new(),
            recent_request_events: Vec::new(),
            market_request_logs: Vec::new(),
        };

        let (events, country_counts) = confirmed_request_events(&snapshot, &response);

        assert_eq!(events.len(), 1);
        assert!(events[0].is_inflight);
        assert_eq!(events[0].share_subdomain.as_deref(), Some("live-sub"));
        assert_eq!(country_counts.get("USA"), Some(&1));
    }

    fn share_log(request_id: &str, created_at: i64) -> crate::models::ShareRequestLogEntry {
        crate::models::ShareRequestLogEntry {
            request_id: request_id.into(),
            share_id: "share-1".into(),
            share_name: "Share".into(),
            provider_id: "provider-1".into(),
            provider_name: "Provider".into(),
            app_type: "codex".into(),
            model: "gpt-5".into(),
            request_model: "gpt-5".into(),
            request_agent: "codex".into(),
            requested_model: "gpt-5".into(),
            actual_model: "gpt-5".into(),
            actual_model_source: "official".into(),
            status_code: 200,
            latency_ms: 1,
            first_token_ms: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            is_streaming: false,
            session_id: None,
            user_country: None,
            user_country_iso3: None,
            created_at,
        }
    }

    /// Regression guard for the SSE late-subscriber bug Codex flagged: a
    /// client that connects after the upgrade task has already flipped its
    /// status used to block on `rx.recv()` forever. The fix is to surface a
    /// `done` event purely from the status snapshot, with no further log
    /// traffic required.
    #[tokio::test]
    async fn emit_done_if_finished_succeeds_for_post_completion_subscribers() {
        let status = std::sync::Arc::new(tokio::sync::Mutex::new(UpgradeStatus::Success));
        let event = emit_done_if_finished(&status).await;
        let event = event.expect("done event expected for completed upgrade");
        let serialized = format!("{event:?}");
        assert!(
            serialized.contains("done"),
            "event payload missing done marker: {serialized}"
        );
        assert!(
            serialized.contains("success"),
            "event payload missing success status: {serialized}"
        );
    }

    #[tokio::test]
    async fn emit_done_if_finished_returns_none_while_running() {
        let status = std::sync::Arc::new(tokio::sync::Mutex::new(UpgradeStatus::Running));
        assert!(emit_done_if_finished(&status).await.is_none());
    }
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

async fn update_share_settings(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
    Json(input): Json<ShareSettingsUpdateRequest>,
) -> Result<Json<crate::models::ShareSettingsUpdateResponse>, AppError> {
    let current_user_email = require_session_email(&state, &headers).await?;
    let response = state
        .store
        .create_share_settings_edit(&share_id, &current_user_email, input.patch)
        .await?;
    let _ = state.share_edit_events.send(ShareEditAvailableEvent {
        kind: "share_edit_available".to_string(),
        installation_id: response.edit.installation_id.clone(),
        share_id: response.edit.share_id.clone(),
        revision: response.edit.revision,
    });
    schedule_share_edit_wake_retries(state.clone(), response.edit.clone());
    Ok(Json(response))
}

fn schedule_share_edit_wake_retries(state: ServerState, edit: crate::models::ShareEditView) {
    tokio::spawn(async move {
        for attempt in 1..=SHARE_EDIT_WAKE_RETRY_ATTEMPTS {
            sleep(Duration::from_secs(SHARE_EDIT_WAKE_RETRY_INTERVAL_SECS)).await;
            match state
                .store
                .is_share_edit_pending(&edit.id, edit.revision)
                .await
            {
                Ok(true) => {
                    tracing::info!(
                        edit_id = %edit.id,
                        share_id = %edit.share_id,
                        installation_id = %edit.installation_id,
                        revision = edit.revision,
                        attempt,
                        "share edit still pending; rebroadcasting wake event"
                    );
                    let _ = state.share_edit_events.send(ShareEditAvailableEvent {
                        kind: "share_edit_available".to_string(),
                        installation_id: edit.installation_id.clone(),
                        share_id: edit.share_id.clone(),
                        revision: edit.revision,
                    });
                }
                Ok(false) => break,
                Err(err) => {
                    tracing::warn!(
                        edit_id = %edit.id,
                        share_id = %edit.share_id,
                        revision = edit.revision,
                        error = %err,
                        "failed to check share edit pending state for wake retry"
                    );
                    break;
                }
            }
        }
    });
}

async fn pending_share_edits(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<SharePendingEditsRequest>,
) -> Result<Json<crate::models::SharePendingEditsResponse>, AppError> {
    Ok(Json(
        state
            .store
            .pending_share_edits(input, extract_client_metadata(&headers, addr))
            .await?,
    ))
}

async fn ack_share_edit(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ShareEditAckRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    state
        .store
        .ack_share_edit(input, extract_client_metadata(&headers, addr))
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn share_edit_events(
    State(state): State<ServerState>,
    Query(query): Query<ShareEditEventsQuery>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, AppError> {
    let payload = ShareEditEventSignaturePayload {
        installation_id: query.installation_id.clone(),
    };
    state
        .store
        .verify_share_edit_event_stream(
            &query.installation_id,
            &payload,
            query.timestamp_ms,
            &query.nonce,
            &query.signature,
        )
        .await?;
    let installation_id = query.installation_id;
    let mut rx = state.share_edit_events.subscribe();
    let stream = async_stream::stream! {
        yield Ok(Event::default().event("ready").data("{}"));
        loop {
            match rx.recv().await {
                Ok(event) if event.installation_id == installation_id => {
                    let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string());
                    yield Ok(Event::default().event("share_edit_available").data(data));
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    yield Ok(Event::default().event("resync").data("{}"));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Ok(Sse::new(stream))
}

async fn batch_sync_share_request_logs(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ShareRequestLogBatchSyncRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let snapshot = state.recent_traffic.snapshot().await;
    let live_country_map = live_country_by_request_id(&snapshot);
    state
        .store
        .batch_sync_share_request_logs(
            input,
            extract_client_metadata(&headers, addr),
            "",
            live_country_map,
        )
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn refresh_share_runtime(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ShareRuntimeRefreshRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let refresh = state
        .store
        .prepare_share_runtime_refresh(input, extract_client_metadata(&headers, addr))
        .await?;

    if !state
        .proxy
        .active_subdomains()
        .await
        .contains(&refresh.subdomain)
    {
        return Err(AppError::BadRequest(format!(
            "share subdomain is not active: {}",
            refresh.subdomain
        )));
    }

    let client = reqwest::Client::builder()
        .user_agent("cc-switch-router/0.1 share-runtime-refresh")
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| AppError::Internal(format!("create runtime refresh client failed: {e}")))?;
    let snapshot = crate::store::fetch_share_runtime_snapshot_from_route(
        &state.config,
        &client,
        &refresh.subdomain,
    )
    .await?;
    state.store.record_share_runtime_snapshot(snapshot).await?;

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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BoardListQuery {
    #[serde(default)]
    tab: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    /// RFC3339 timestamp; if present, the server returns only changes since that time.
    #[serde(default)]
    since: Option<String>,
}

async fn resolve_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<Option<AuthSession>, AppError> {
    let Some(token) = extract_bearer_token(headers) else {
        return Ok(None);
    };
    state.store.resolve_session_by_access_token(token).await
}

fn extract_guest_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-board-guest-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 80)
        .map(str::to_string)
}

async fn require_admin_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<AuthSession, AppError> {
    let session = resolve_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))?;
    if !state.dynamic.read().await.is_admin(&session.email) {
        return Err(AppError::Forbidden("admin privilege required".into()));
    }
    Ok(session)
}

async fn list_board_messages(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<BoardListQuery>,
) -> Result<Json<BoardMessageListResponse>, AppError> {
    let session = resolve_session(&state, &headers).await?;
    let guest_id = extract_guest_id(&headers);
    let viewer_user_id = session.as_ref().map(|s| s.user_id.clone());
    let since = query
        .since
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc));
    let response = state
        .store
        .list_board_messages(
            query.tab.as_deref().unwrap_or("all"),
            query.limit.unwrap_or(50),
            viewer_user_id.as_deref(),
            guest_id.as_deref(),
            since,
        )
        .await?;
    Ok(Json(response))
}

async fn post_board_message(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<PostBoardMessageRequest>,
) -> Result<Json<BoardMessageView>, AppError> {
    let session = resolve_session(&state, &headers).await?;
    let metadata = extract_client_metadata(&headers, addr);
    let client_ip = metadata.ip.clone();
    let (board_settings, telegram_notify_all, is_admin_session) = {
        let dynamic = state.dynamic.read().await;
        let admin = session
            .as_ref()
            .map(|s| dynamic.is_admin(&s.email))
            .unwrap_or(false);
        (dynamic.board.clone(), dynamic.telegram.notify_all, admin)
    };
    let author = if let Some(session) = session.as_ref() {
        if is_admin_session {
            BoardAuthor::Admin {
                user_id: session.user_id.clone(),
                email: session.email.clone(),
            }
        } else {
            BoardAuthor::User {
                user_id: session.user_id.clone(),
                email: session.email.clone(),
            }
        }
    } else {
        let guest_id = extract_guest_id(&headers).ok_or_else(|| {
            AppError::BadRequest("anonymous posts require an X-Board-Guest-Id header".into())
        })?;
        BoardAuthor::Guest {
            guest_id,
            name: input.guest_name.clone(),
        }
    };
    let message = state
        .store
        .create_board_message(&board_settings, author, input.body, client_ip.as_deref())
        .await?;

    if telegram_notify_all {
        let notifier = state.telegram.read().await.clone();
        if let Some(notifier) = notifier {
            let payload = message.clone();
            tokio::spawn(async move {
                notifier.notify_new_message(&payload).await;
            });
        }
    }

    Ok(Json(message))
}

async fn pin_board_message(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<BoardMessageToggleRequest>,
) -> Result<Json<BoardMessageView>, AppError> {
    require_admin_session(&state, &headers).await?;
    let board_settings = state.dynamic.read().await.board.clone();
    let view = state
        .store
        .set_board_pinned(&board_settings, &id, input.value)
        .await?;
    Ok(Json(view))
}

async fn feature_board_message(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<BoardMessageToggleRequest>,
) -> Result<Json<BoardMessageView>, AppError> {
    require_admin_session(&state, &headers).await?;
    let view = state.store.set_board_featured(&id, input.value).await?;
    Ok(Json(view))
}

async fn delete_board_message(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = resolve_session(&state, &headers).await?;
    let (board_settings, is_admin) = {
        let dynamic = state.dynamic.read().await;
        let admin = session
            .as_ref()
            .map(|s| dynamic.is_admin(&s.email))
            .unwrap_or(false);
        (dynamic.board.clone(), admin)
    };
    let admin_email = if is_admin {
        session.as_ref().map(|s| s.email.clone())
    } else {
        None
    };
    let guest_id = extract_guest_id(&headers);
    state
        .store
        .delete_board_message(
            &board_settings,
            &id,
            is_admin,
            admin_email.as_deref(),
            guest_id.as_deref(),
        )
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn board_meta(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<BoardMetaResponse>, AppError> {
    let session = resolve_session(&state, &headers).await?;
    let (board_settings, can_post_as_admin) = {
        let dynamic = state.dynamic.read().await;
        let admin = session
            .as_ref()
            .map(|s| dynamic.is_admin(&s.email))
            .unwrap_or(false);
        (dynamic.board.clone(), admin)
    };
    let meta = state
        .store
        .board_meta(
            can_post_as_admin,
            board_settings.max_len,
            board_settings.guest_self_delete_secs,
        )
        .await?;
    Ok(Json(meta))
}

async fn admin_settings_schema(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<SettingsSchemaResponse>, AppError> {
    require_admin_session(&state, &headers).await?;
    Ok(Json(schema_response()))
}

async fn admin_settings_values(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<SettingsValuesResponse>, AppError> {
    require_admin_session(&state, &headers).await?;
    Ok(Json(values_response(&state.env_path)?))
}

async fn admin_settings_apply(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<SettingsUpdateRequest>,
) -> Result<Json<SettingsUpdateResponse>, AppError> {
    let session = require_admin_session(&state, &headers).await?;
    if input.updates.is_empty() {
        return Err(AppError::BadRequest("updates is empty".into()));
    }

    // 1) acquire write lock first so reads see the new dynamic state atomically.
    let mut dynamic_guard = state.dynamic.write().await;

    // 2) load current env, validate updates against the schema.
    let existing = read_env_file(&state.env_path)?;
    let outcome = validate_and_diff(&existing, &input.updates)?;

    // 3) persist .env atomically (keeps .bak of the prior file).
    write_env_file_atomic(&state.env_path, &outcome.new_env_kv)?;

    // 4) apply the diff to the live DynamicSettings. Only fields named in
    //    `updates` change; everything else keeps the current runtime value,
    //    so an unrelated PATCH cannot silently revert process-env overrides.
    //    Clears (Some("") / None) reset to the canonical default, which is
    //    what gives admin revocation immediate effect.
    apply_updates_to_dynamic(&mut dynamic_guard, &input.updates, &state.config);
    let next_dynamic = dynamic_guard.clone();
    drop(dynamic_guard);

    // 5) rebuild telegram notifier if its inputs changed.
    let needs_telegram = outcome
        .updated_keys
        .iter()
        .any(|k| k.starts_with("CC_SWITCH_ROUTER_TELEGRAM_"));
    if needs_telegram {
        let rebuilt = build_notifier_from_dynamic(&state, &next_dynamic).await;
        *state.telegram.write().await = rebuilt;
    }

    // 6) audit.
    let metadata = extract_client_metadata(&headers, addr);
    let payload = serde_json::json!({
        "updatedKeys": outcome.updated_keys,
        "restartRequiredKeys": outcome.restart_required_keys,
    });
    let _ = state
        .store
        .record_admin_audit(
            Some(&session.email),
            "settings.apply",
            Some(&payload),
            metadata.ip.as_deref(),
        )
        .await;

    let dynamic_groups: Vec<String> = outcome
        .dynamic_groups
        .iter()
        .map(|g| format!("{:?}", g))
        .collect();

    Ok(Json(SettingsUpdateResponse {
        updated_keys: outcome.updated_keys,
        unchanged_keys: outcome.unchanged_keys,
        restart_required_keys: outcome.restart_required_keys,
        dynamic_groups_refreshed: dynamic_groups,
        env_path: state.env_path.display().to_string(),
    }))
}

async fn build_notifier_from_dynamic(
    state: &ServerState,
    dynamic: &DynamicSettings,
) -> Option<std::sync::Arc<crate::board_telegram::TelegramNotifier>> {
    // Reuse the existing constructor by spoofing a Config-shaped view; simpler
    // than rewriting it for two callers. The notifier only inspects telegram_*,
    // tunnel_domain, and use_localhost — the rest can stay as the boot snapshot.
    let mut config = state.config.clone();
    config.telegram_bot_token = dynamic.telegram.bot_token.clone();
    config.telegram_chat_id = dynamic.telegram.chat_id.clone();
    config.telegram_topic_id = dynamic.telegram.topic_id;
    config.telegram_notify_all = dynamic.telegram.notify_all;
    config.telegram_notify_admin = dynamic.telegram.notify_admin;
    crate::board_telegram::TelegramNotifier::from_config(&config)
}

async fn admin_version(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<VersionResponse>, AppError> {
    let session = resolve_session(&state, &headers).await?;
    let is_admin = match session.as_ref() {
        Some(s) => state.dynamic.read().await.is_admin(&s.email),
        None => false,
    };
    let info = build_info();
    let service = detect_service_status();
    let client = reqwest::Client::builder()
        .user_agent("cc-switch-router/0.1 version-probe")
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| AppError::Internal(format!("version client failed: {e}")))?;
    let latest = fetch_latest_release_meta(&client).await;
    let mut response = VersionResponse {
        version: info.version,
        commit: info.commit,
        build_time: info.build_time,
        binary_path: BINARY_INSTALL_PATH,
        rollback_path: BINARY_ROLLBACK_PATH,
        rollback_available: std::path::Path::new(BINARY_ROLLBACK_PATH).exists(),
        uptime_secs: uptime_secs_from(state.start_instant),
        service,
        latest,
    };
    if !is_admin {
        response.service.unit_name = None;
        response.service.unit_file_state = None;
        if matches!(response.service.manager, ServiceManager::Systemd) {
            // Hide active_state details from anonymous viewers; only show on/off.
            response.service.active_state = if response.service.active {
                Some("active".into())
            } else {
                Some("inactive".into())
            };
        }
    } else {
        // Tag the unit name explicitly for clarity in the UI.
        if matches!(response.service.manager, ServiceManager::Systemd) {
            response.service.unit_name = Some(SERVICE_UNIT);
        }
    }
    Ok(Json(response))
}

async fn admin_restart(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_admin_session(&state, &headers).await?;
    let strategy = RestartStrategy::from_manager(detect_service_status().manager);
    let script = schedule_restart(strategy)?;
    let metadata = extract_client_metadata(&headers, addr);
    let payload = serde_json::json!({
        "strategy": strategy.label(),
        "script": script,
    });
    let _ = state
        .store
        .record_admin_audit(
            Some(&session.email),
            "service.restart",
            Some(&payload),
            metadata.ip.as_deref(),
        )
        .await;
    Ok(Json(serde_json::json!({
        "ok": true,
        "strategy": strategy.label(),
    })))
}

async fn admin_rollback(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<Json<crate::admin::upgrade::RollbackResponse>, AppError> {
    let session = require_admin_session(&state, &headers).await?;
    ensure_binary_writable()?;
    let response = crate::admin::upgrade::rollback_to_previous_binary().await?;
    let metadata = extract_client_metadata(&headers, addr);
    let payload = serde_json::json!({
        "strategy": response.strategy,
        "backupPath": response.backup_path,
    });
    let _ = state
        .store
        .record_admin_audit(
            Some(&session.email),
            "service.rollback",
            Some(&payload),
            metadata.ip.as_deref(),
        )
        .await;
    Ok(Json(response))
}

async fn admin_upgrade_start(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_admin_session(&state, &headers).await?;
    ensure_binary_writable()?;
    let client = reqwest::Client::builder()
        .user_agent("cc-switch-router/0.1 upgrade")
        .build()
        .map_err(|e| AppError::Internal(format!("upgrade client failed: {e}")))?;
    let handle = state
        .upgrade_registry
        .start(client, Some(session.email.clone()))
        .await?;
    let metadata = extract_client_metadata(&headers, addr);
    let payload = serde_json::json!({ "taskId": handle.task_id });
    let _ = state
        .store
        .record_admin_audit(
            Some(&session.email),
            "service.upgrade",
            Some(&payload),
            metadata.ip.as_deref(),
        )
        .await;
    Ok(Json(serde_json::json!({
        "taskId": handle.task_id,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpgradeStreamQuery {
    #[serde(default)]
    task_id: Option<String>,
    /// Fallback bearer for EventSource (no header support). Use HTTPS in
    /// production; tokens are short-lived (auth_session_ttl_secs).
    #[serde(default)]
    access_token: Option<String>,
}

async fn admin_upgrade_stream(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<UpgradeStreamQuery>,
) -> Result<
    axum::response::Sse<
        impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
    >,
    AppError,
> {
    let session = if let Some(token) = query.access_token.as_deref() {
        state
            .store
            .resolve_session_by_access_token(token)
            .await?
            .ok_or_else(|| AppError::Unauthorized("session not found".into()))?
    } else {
        let token = extract_bearer_token(&headers)
            .ok_or_else(|| AppError::Unauthorized("missing bearer token".into()))?;
        state
            .store
            .resolve_session_by_access_token(token)
            .await?
            .ok_or_else(|| AppError::Unauthorized("session not found".into()))?
    };
    if !state.dynamic.read().await.is_admin(&session.email) {
        return Err(AppError::Forbidden("admin privilege required".into()));
    }
    let handle = state
        .upgrade_registry
        .current()
        .await
        .ok_or_else(|| AppError::NotFound("no upgrade task running".into()))?;
    if let Some(expected) = query.task_id.as_deref() {
        if expected != handle.task_id {
            return Err(AppError::NotFound("upgrade task id does not match".into()));
        }
    }
    let history: Vec<UpgradeLogEntry> = handle.history.lock().await.clone();
    let receiver = handle.sender.subscribe();
    let status = handle.status.clone();
    let stream = async_stream::stream! {
        for entry in history {
            yield Ok(sse_event_from_entry(&entry));
        }
        // The upgrade task can finish before this subscription happens, in which
        // case no new broadcast events will ever arrive — without a periodic
        // status poll the stream would block forever. Check once up front, then
        // wake every 2s while waiting for log entries.
        if let Some(event) = emit_done_if_finished(&status).await {
            yield Ok(event);
            return;
        }
        let mut rx = receiver;
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await {
                Ok(Ok(entry)) => {
                    yield Ok(sse_event_from_entry(&entry));
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                    if let Some(event) = emit_done_if_finished(&status).await {
                        yield Ok(event);
                    }
                    break;
                }
                Err(_) => {
                    // Timeout: re-check status so we don't hang after the
                    // background task finishes between events.
                }
            }
            if let Some(event) = emit_done_if_finished(&status).await {
                // Drain any messages buffered after the status flipped.
                while let Ok(entry) = rx.try_recv() {
                    yield Ok(sse_event_from_entry(&entry));
                }
                yield Ok(event);
                break;
            }
        }
    };
    Ok(axum::response::Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new().interval(std::time::Duration::from_secs(15)),
    ))
}

async fn emit_done_if_finished(
    status: &std::sync::Arc<tokio::sync::Mutex<UpgradeStatus>>,
) -> Option<axum::response::sse::Event> {
    let current = *status.lock().await;
    if matches!(current, UpgradeStatus::Running) {
        return None;
    }
    let payload = serde_json::json!({
        "status": match current {
            UpgradeStatus::Success => "success",
            UpgradeStatus::Failed => "failed",
            UpgradeStatus::Running => "running",
        }
    });
    Some(
        axum::response::sse::Event::default()
            .event("done")
            .data(serde_json::to_string(&payload).unwrap_or_default()),
    )
}

fn sse_event_from_entry(entry: &UpgradeLogEntry) -> axum::response::sse::Event {
    let data = serde_json::to_string(entry).unwrap_or_default();
    axum::response::sse::Event::default()
        .event("log")
        .data(data)
}

async fn admin_telegram_test(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_admin_session(&state, &headers).await?;
    let notifier = state.telegram.read().await.clone().ok_or_else(|| {
        AppError::BadRequest("telegram is not configured (bot token / chat id missing)".into())
    })?;
    let preview = crate::models::BoardMessageView {
        id: "preview".into(),
        body: format!("🧪 settings test from {}", session.email),
        author_kind: "admin".into(),
        author_label: "Official".into(),
        is_mine: true,
        pinned: false,
        featured: false,
        created_at: chrono::Utc::now(),
        pinned_at: None,
        featured_at: None,
    };
    notifier.notify_new_message(&preview).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
struct AdminAuditQuery {
    #[serde(default)]
    limit: Option<usize>,
}

async fn admin_audit_list(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<AdminAuditQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin_session(&state, &headers).await?;
    let entries = state
        .store
        .list_admin_audit(query.limit.unwrap_or(50))
        .await?;
    Ok(Json(serde_json::json!({ "entries": entries })))
}
