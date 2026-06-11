use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::io::{Read, Seek, SeekFrom};
use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::{any, delete, get, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
        BINARY_INSTALL_PATH, BINARY_ROLLBACK_PATH, SERVICE_LOG_PATH, SERVICE_UNIT, ServiceManager,
        VersionResponse, build_info, detect_service_status, ensure_binary_writable,
        fetch_latest_release_meta, uptime_secs_from,
    },
};
use crate::client_meta::extract_client_metadata;
use crate::dynamic_settings::DynamicSettings;
use crate::error::AppError;
use crate::models::{
    AuthSession, BindInstallationOwnerEmailRequest, BindInstallationOwnerEmailResponse,
    BoardMessageListResponse, BoardMessageToggleRequest, BoardMessageView, BoardMetaResponse,
    ChangeInstallationOwnerEmailRequest, ChangeInstallationOwnerEmailResponse,
    ClientTunnelClaimRequest, ClientTunnelQuery, ClientTunnelResponse, ClientTunnelUpdateRequest,
    DashboardMarketRequestLogView, DashboardPresenceRequest, DashboardPresenceResponse,
    DashboardResponse, DashboardTickerShare, GatewayRegistryRecord, GetInstallationOwnerEmailQuery,
    GetInstallationOwnerEmailResponse, HealthResponse, ImageGenerationJobEntry, IssueLeaseRequest,
    IssueLeaseResponse, MarketDisabledSharesUpdateRequest, MarketDisabledSharesUpdateResponse,
    MarketMaintenanceUpdateRequest, MarketMaintenanceUpdateResponse,
    MarketNotificationEmailLogView, MarketNotificationEmailRequest,
    MarketNotificationEmailResponse, MarketRequestLogBatchSyncRequest,
    MarketShareRuntimeStateReleaseRequest, MarketShareRuntimeStateReleaseResponse,
    MarketShareRuntimeStateSyncRequest, MarketShareRuntimeStateSyncResponse, MarketShareView,
    MarketsResponse, PostBoardMessageRequest, PublicMapPointsResponse, RefreshSessionRequest,
    RegisterGatewayRequest, RegisterGatewayResponse, RegisterInstallationRequest,
    RegisterInstallationResponse, RegisterMarketRequest, RequestEmailCodeRequest,
    RequestEmailCodeResponse, SessionStatusResponse, ShareApiAuthResponse, ShareApiAuthUser,
    ShareApiContextResponse, ShareApiShareResponse, ShareBatchSyncRequest,
    ShareClaimSubdomainRequest, ShareDeleteRequest, ShareEditAckRequest, ShareEditAvailableEvent,
    ShareEditEventSignaturePayload, ShareHeartbeatRequest, ShareMarketGrantRequest,
    ShareMarketGrantResponse, ShareMarketGrantStatusResponse, SharePendingEditsRequest,
    ShareRequestLogBatchSyncRequest, ShareRequestLogEntry, ShareRuntimeRefreshRequest,
    ShareSettingsPatch, ShareSettingsUpdateRequest, ShareSyncRequest, UserApiTokenResetResponse,
    UserApiTokenResponse, UserSharesResponse, VerifyEmailCodeRequest, VerifyEmailCodeResponse,
};
use crate::proxy::{gateway_proxy_handler, market_proxy_handler, proxy_handler};
use crate::recent_traffic::{RecentRequestEvent, RecentTrafficSnapshot};
use crate::scheduling_signals::{
    ShareFeedbackKind, ShareFeedbackRequest, ShareFeedbackResponse, ShareHeadroomEntry,
    ShareHeadroomRequest, ShareHeadroomResponse,
};
use crate::store::{BoardAuthor, ShareForTest};

const REGIONS: &str = include_str!("../regions");
const SHARE_EDIT_WAKE_RETRY_INTERVAL_SECS: u64 = 20;
const SHARE_EDIT_WAKE_RETRY_ATTEMPTS: usize = 3;
const DASHBOARD_REQUEST_TICKER_LIMIT: usize = 5;
const ROUTER_ACCESS_COOKIE: &str = "cc_switch_router_access";

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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareApiAuthQuery {
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareUsageByEmailQuery {
    app: Option<String>,
    period: Option<String>,
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
        .route("/v1/share-market/shares", get(share_market_shares))
        .route(
            "/v1/share-market/shares/:share_id/grants",
            post(share_market_create_grant),
        )
        .route(
            "/v1/share-market/shares/:share_id/grants/:router_edit_id",
            get(share_market_grant_status),
        )
        .route("/v1/market/shares/headroom", post(market_shares_headroom))
        .route("/v1/market/shares/feedback", post(market_shares_feedback))
        .route("/v1/market/share-states", post(market_share_states))
        .route("/v1/gateways/register", post(register_gateway))
        .route("/v1/gateway/shares", get(gateway_shares))
        .route("/v1/gateway/shares/headroom", post(gateway_shares_headroom))
        .route("/v1/gateway/shares/feedback", post(gateway_shares_feedback))
        .route(
            "/v1/gateway/request-logs/batch",
            post(batch_sync_gateway_request_logs),
        )
        .route(
            "/v1/admin/markets/:market_email/linked-shares",
            get(admin_market_linked_shares),
        )
        .route(
            "/v1/admin/markets/:market_email/disabled-shares",
            patch(admin_update_market_disabled_shares),
        )
        .route(
            "/v1/admin/markets/:market_email/maintenance",
            patch(admin_update_market_maintenance),
        )
        .route(
            "/v1/admin/markets/:market_email/share-states/release",
            post(admin_release_market_share_state),
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
        .route(
            "/v1/installations/client-tunnel",
            get(get_client_tunnel).patch(update_client_tunnel),
        )
        .route(
            "/v1/installations/client-tunnel/claim",
            post(claim_client_tunnel),
        )
        .route("/v1/auth/email/request-code", post(request_email_code))
        .route("/v1/auth/email/verify-code", post(verify_email_code))
        .route(
            "/v1/client-web/auth/email/verify-code",
            post(verify_client_web_email_code),
        )
        .route("/v1/auth/session/refresh", post(refresh_session))
        .route("/v1/auth/session/logout", post(logout_session))
        .route("/v1/auth/session/me", get(session_me))
        .route("/share-api/context", get(share_api_context))
        .route("/share-api/share", get(share_api_share))
        .route("/share-api/auth/me", get(share_api_auth_me))
        .route(
            "/share-api/share/settings",
            patch(share_api_update_settings),
        )
        .route("/v1/me/api-token", get(get_default_api_token))
        .route("/v1/me/api-token/reset", post(reset_default_api_token))
        .route("/v1/me/shares", get(my_shares))
        .route("/v1/tunnels/lease", post(issue_lease))
        .route("/v1/shares/claim-subdomain", post(claim_share_subdomain))
        .route("/v1/shares/sync", post(sync_share))
        .route("/v1/shares/batch-sync", post(batch_sync_share))
        .route("/v1/shares/runtime-refresh", post(refresh_share_runtime))
        .route(
            "/v1/shares/:share_id/settings",
            patch(update_share_settings),
        )
        .route(
            "/v1/shares/:share_id/usage-by-email",
            get(share_usage_by_email),
        )
        .route(
            "/v1/shares/:share_id/test-connection",
            post(test_share_connection),
        )
        .route(
            "/v1/shares/:share_id/image-jobs",
            get(list_share_image_generation_jobs),
        )
        .route(
            "/v1/shares/:share_id/image-jobs/:job_id/result",
            get(get_share_image_generation_job_result),
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
        .route("/v1/admin/logs/router/tail", get(admin_router_log_tail))
        .route(
            "/v1/admin/logs/router/download",
            get(admin_router_log_download),
        )
        .route("/v1/admin/telegram/test", post(admin_telegram_test))
        .route("/v1/admin/audit", get(admin_audit_list))
        .route("/v1/admin/metrics/snapshot", get(admin_metrics_snapshot))
        .route("/v1/admin/metrics/host/info", get(admin_metrics_host_info))
        .route(
            "/v1/admin/metrics/host/status",
            get(admin_metrics_host_status),
        )
        .route("/v1/admin/metrics/series", get(admin_metrics_series))
        .route(
            "/v1/admin/metrics/llm/snapshot",
            get(admin_metrics_llm_snapshot),
        )
        .route("/v1/admin/metrics/llm/series", get(admin_metrics_series))
        .route("/v1/admin/metrics/llm/top", get(admin_metrics_llm_top))
        .route("/v1/admin/metrics/llm/errors", get(admin_metrics_events))
        .route(
            "/v1/admin/metrics/llm/failover",
            get(admin_metrics_llm_failover),
        )
        .route("/v1/admin/metrics/events", get(admin_metrics_events))
        .route("/v1/admin/metrics", delete(admin_metrics_clear))
        .route("/_market/proxy/:share_id/*path", any(market_proxy_handler))
        .route(
            "/_gateway/proxy/:share_id/*path",
            any(gateway_proxy_handler),
        )
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
    if let Some(ip) = source_ip_from_request(&req)
        && state.dynamic.read().await.is_ip_blacklisted(ip)
    {
        state.ip_blacklist_stats.record(ip, req.uri().path());
        return (StatusCode::FORBIDDEN, "IP blacklisted").into_response();
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
    if market.market_kind != "usage" {
        return Err(AppError::Forbidden(
            "market:shares API is only available to usage markets".into(),
        ));
    }
    let active_subdomains = state.proxy.active_subdomains().await.into_iter().collect();
    let inflight_by_share = state.proxy.inflight_by_share().await;
    let mut shares = state
        .store
        .list_market_shares(
            &market.email,
            "main",
            &active_subdomains,
            &inflight_by_share,
            true,
        )
        .await?;
    // Overlay per-owner penalty from the in-memory override store. Done at the
    // edge so the store layer stays unaware of the runtime feedback channel.
    for share in &mut shares {
        if let Some(email) = share.owner_email.as_deref() {
            if let Some(penalty) = state.scheduling_overrides.get(email) {
                share.signals.owner_penalty =
                    (share.signals.owner_penalty * penalty).clamp(0.05, 1.0);
            }
        }
    }
    Ok(Json(shares))
}

async fn share_market_shares(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Vec<MarketShareView>>, AppError> {
    let market = authenticate_market(&state, &headers, "market:shares:read").await?;
    if market.market_kind != "share" {
        return Err(AppError::Forbidden(
            "share-market shares API is only available to share markets".into(),
        ));
    }
    let active_subdomains = state.proxy.active_subdomains().await.into_iter().collect();
    let inflight_by_share = state.proxy.inflight_by_share().await;
    Ok(Json(
        state
            .store
            .list_share_market_delegated_shares(
                &market.email,
                "main",
                &active_subdomains,
                &inflight_by_share,
            )
            .await?,
    ))
}

async fn share_market_create_grant(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
    Json(input): Json<ShareMarketGrantRequest>,
) -> Result<Json<ShareMarketGrantResponse>, AppError> {
    let market = authenticate_market(&state, &headers, "market:share_grants:write").await?;
    if market.market_kind != "share" {
        return Err(AppError::Forbidden(
            "share-market grants API is only available to share markets".into(),
        ));
    }
    Ok(Json(
        state
            .store
            .create_share_market_grant(&market.email, &share_id, input)
            .await?,
    ))
}

async fn share_market_grant_status(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((share_id, router_edit_id)): Path<(String, String)>,
) -> Result<Json<ShareMarketGrantStatusResponse>, AppError> {
    let market = authenticate_market(&state, &headers, "market:share_grants:write").await?;
    if market.market_kind != "share" {
        return Err(AppError::Forbidden(
            "share-market grants API is only available to share markets".into(),
        ));
    }
    Ok(Json(
        state
            .store
            .share_market_grant_status(&market.email, &share_id, &router_edit_id)
            .await?,
    ))
}

/// Per-request real-time headroom probe. The market normally consumes the
/// 30s-stale snapshot embedded in `MarketShareView`, but right before
/// scheduling a request it can POST a small batch of candidate share_ids to
/// learn their live `inflight` counts. This avoids over-packing a saturated
/// share while still keeping the steady-state cost low.
async fn market_shares_headroom(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ShareHeadroomRequest>,
) -> Result<Json<ShareHeadroomResponse>, AppError> {
    let _market = authenticate_market(&state, &headers, "market:shares:read").await?;
    market_shares_headroom_impl(&state, input).await
}

async fn market_shares_headroom_impl(
    state: &ServerState,
    input: ShareHeadroomRequest,
) -> Result<Json<ShareHeadroomResponse>, AppError> {
    if input.share_ids.is_empty() {
        return Ok(Json(ShareHeadroomResponse {
            queried_at: chrono::Utc::now().to_rfc3339(),
            entries: Vec::new(),
        }));
    }
    // De-dupe + cap to avoid abusive payloads. 256 is well above any sane
    // candidate pool the scheduler would build for a single request.
    let mut wanted: HashSet<String> = HashSet::new();
    for id in input.share_ids.into_iter().take(256) {
        wanted.insert(id);
    }

    let inflight = state.proxy.inflight_by_share().await;
    let parallel_limits = state.store.share_parallel_limits(&wanted).await?;
    let entries: Vec<ShareHeadroomEntry> = wanted
        .iter()
        .map(|share_id| {
            let active = *inflight.get(share_id).unwrap_or(&0);
            let limit = parallel_limits
                .get(share_id)
                .copied()
                .unwrap_or(crate::models::default_share_parallel_limit());
            let headroom = crate::scheduling_signals::compute_headroom(active, limit);
            ShareHeadroomEntry {
                share_id: share_id.clone(),
                active_requests: active,
                parallel_limit: limit,
                headroom,
            }
        })
        .collect();
    Ok(Json(ShareHeadroomResponse {
        queried_at: chrono::Utc::now().to_rfc3339(),
        entries,
    }))
}

/// 429/rate-limit feedback from a market. Because the same owner_email
/// typically backs all shares with shared upstream credentials, the penalty
/// is applied to *every* share of that owner, not just the offending one.
/// The override decays via TTL (default 30m).
async fn market_shares_feedback(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ShareFeedbackRequest>,
) -> Result<Json<ShareFeedbackResponse>, AppError> {
    let _market = authenticate_market(&state, &headers, "market:shares:read").await?;
    apply_share_feedback(&state, input, "market").await
}

async fn market_share_states(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<MarketShareRuntimeStateSyncRequest>,
) -> Result<Json<MarketShareRuntimeStateSyncResponse>, AppError> {
    let market = authenticate_market(&state, &headers, "market:share_states:write").await?;
    let synced = state
        .store
        .sync_market_share_runtime_states(&market.email, input.replace, input.states)
        .await?;
    Ok(Json(MarketShareRuntimeStateSyncResponse {
        ok: true,
        synced,
    }))
}

async fn apply_share_feedback(
    state: &ServerState,
    input: ShareFeedbackRequest,
    source: &str,
) -> Result<Json<ShareFeedbackResponse>, AppError> {
    let owner = state
        .store
        .lookup_share_owner_email(&input.share_id)
        .await?;
    let Some(owner_email) = owner else {
        return Ok(Json(ShareFeedbackResponse {
            ok: false,
            owner_scope: None,
            applied_penalty: 1.0,
            expires_in_secs: 0,
        }));
    };

    let (default_penalty, default_ttl_secs) = match input.kind {
        ShareFeedbackKind::RateLimited => (0.5_f64, 30 * 60_u64),
        ShareFeedbackKind::QuotaExhausted => (0.05_f64, 7 * 24 * 60 * 60_u64),
    };
    let penalty = input.penalty.unwrap_or(default_penalty);
    let ttl_cap = match input.kind {
        ShareFeedbackKind::RateLimited => 24 * 60 * 60,
        ShareFeedbackKind::QuotaExhausted => 31 * 24 * 60 * 60,
    };
    let ttl_secs = input.ttl_secs.unwrap_or(default_ttl_secs).min(ttl_cap);
    state.scheduling_overrides.set(
        &owner_email,
        penalty,
        Some(std::time::Duration::from_secs(ttl_secs)),
    );

    tracing::info!(
        share_id = %input.share_id,
        owner = %owner_email,
        penalty,
        ttl_secs,
        source,
        "applied share feedback penalty"
    );
    Ok(Json(ShareFeedbackResponse {
        ok: true,
        owner_scope: Some(owner_email),
        applied_penalty: penalty.clamp(0.05, 1.0),
        expires_in_secs: ttl_secs,
    }))
}

async fn register_gateway(
    State(state): State<ServerState>,
    Json(input): Json<RegisterGatewayRequest>,
) -> Result<Json<RegisterGatewayResponse>, AppError> {
    Ok(Json(state.store.register_gateway(input).await?))
}

async fn gateway_shares(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Vec<MarketShareView>>, AppError> {
    let gateway = authenticate_gateway(
        &state,
        &headers,
        "gateway:shares:read",
        "gateway:shares:read",
        &empty_body_sha256_hex(),
    )
    .await?;
    let active_subdomains = state.proxy.active_subdomains().await.into_iter().collect();
    let inflight_by_share = state.proxy.inflight_by_share().await;
    let mut shares = state
        .store
        .list_gateway_shares(&gateway, "main", &active_subdomains, &inflight_by_share)
        .await?;
    for share in &mut shares {
        if let Some(email) = share.owner_email.as_deref()
            && let Some(penalty) = state.scheduling_overrides.get(email)
        {
            share.signals.owner_penalty = (share.signals.owner_penalty * penalty).clamp(0.05, 1.0);
        }
    }
    Ok(Json(shares))
}

async fn gateway_shares_headroom(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ShareHeadroomRequest>,
) -> Result<Json<ShareHeadroomResponse>, AppError> {
    let body_hash = json_body_sha256_hex(&input)?;
    let gateway = authenticate_gateway(
        &state,
        &headers,
        "gateway:shares:read",
        "gateway:shares:headroom",
        &body_hash,
    )
    .await?;
    drop(gateway);
    market_shares_headroom_impl(&state, input).await
}

async fn gateway_shares_feedback(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ShareFeedbackRequest>,
) -> Result<Json<ShareFeedbackResponse>, AppError> {
    let body_hash = json_body_sha256_hex(&input)?;
    let gateway = authenticate_gateway(
        &state,
        &headers,
        "gateway:feedback:write",
        "gateway:shares:feedback",
        &body_hash,
    )
    .await?;
    drop(gateway);
    apply_share_feedback(&state, input, "gateway").await
}

async fn batch_sync_gateway_request_logs(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<MarketRequestLogBatchSyncRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let body_hash = json_body_sha256_hex(&input)?;
    let gateway = authenticate_gateway(
        &state,
        &headers,
        "gateway:request_logs:write",
        "gateway:request_logs:batch",
        &body_hash,
    )
    .await?;
    let count = state
        .store
        .batch_sync_gateway_request_logs(&gateway, input)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true, "synced": count })))
}

async fn admin_market_linked_shares(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(market_email): Path<String>,
) -> Result<Json<Vec<MarketShareView>>, AppError> {
    let current_user_email = require_session_email(&state, &headers).await?;
    let is_admin = state.dynamic.read().await.is_admin(&current_user_email);
    Ok(Json(
        state
            .store
            .list_manageable_market_shares(
                &market_email,
                &current_user_email,
                is_admin,
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
    let is_admin = state.dynamic.read().await.is_admin(&current_user_email);
    let response = state
        .store
        .update_market_disabled_shares(&market_email, &current_user_email, is_admin, input)
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

async fn admin_update_market_maintenance(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(market_email): Path<String>,
    Json(input): Json<MarketMaintenanceUpdateRequest>,
) -> Result<Json<MarketMaintenanceUpdateResponse>, AppError> {
    let current_user_email = require_session_email(&state, &headers).await?;
    let is_admin = state.dynamic.read().await.is_admin(&current_user_email);
    let response = state
        .store
        .update_market_maintenance(&market_email, &current_user_email, is_admin, input)
        .await?;
    let metadata = extract_client_metadata(&headers, addr);
    let payload = serde_json::json!({
        "marketEmail": market_email,
        "maintenanceEnabled": response.maintenance_enabled,
    });
    let _ = state
        .store
        .record_admin_audit(
            Some(&current_user_email),
            "market.maintenance.update",
            Some(&payload),
            metadata.ip.as_deref(),
        )
        .await;
    Ok(Json(response))
}

async fn admin_release_market_share_state(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(market_email): Path<String>,
    Json(input): Json<MarketShareRuntimeStateReleaseRequest>,
) -> Result<Json<MarketShareRuntimeStateReleaseResponse>, AppError> {
    let current_user_email = require_session_email(&state, &headers).await?;
    let is_admin = state.dynamic.read().await.is_admin(&current_user_email);
    let market = state
        .store
        .ensure_market_manager(&market_email, &current_user_email, is_admin)
        .await?;
    let token = extract_bearer_token(&headers)
        .ok_or_else(|| AppError::Unauthorized("missing router session bearer token".into()))?;
    let route = state
        .proxy
        .backend_for_host(
            &format!("{}.{}", market.subdomain, state.config.tunnel_domain),
            &state.config.tunnel_domain,
        )
        .await
        .ok_or_else(|| AppError::Conflict("market is offline".into()))?;
    let url = format!(
        "http://{}/market-api/router/share-states/release",
        route.route_target()
    );
    let response = state
        .proxy_http
        .post(&url)
        .bearer_auth(token)
        .json(&input)
        .send()
        .await
        .map_err(|err| AppError::Internal(format!("release market share state failed: {err}")))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| AppError::Internal(format!("read market release response failed: {err}")))?;
    if !status.is_success() {
        let message = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|value| {
                value
                    .get("message")
                    .and_then(|message| message.as_str())
                    .or_else(|| {
                        value
                            .get("error")
                            .and_then(|error| error.get("message"))
                            .and_then(|message| message.as_str())
                    })
                    .map(ToOwned::to_owned)
            })
            .unwrap_or(text);
        return Err(AppError::BadRequest(format!(
            "market release rejected: {message}"
        )));
    }
    let response: MarketShareRuntimeStateReleaseResponse =
        serde_json::from_str(&text).map_err(|err| {
            AppError::Internal(format!("parse market release response failed: {err}"))
        })?;
    let metadata = extract_client_metadata(&headers, addr);
    let payload = serde_json::json!({
        "marketEmail": market_email,
        "release": input,
        "released": response.released,
        "synced": response.synced,
    });
    let _ = state
        .store
        .record_admin_audit(
            Some(&current_user_email),
            "market.share_state.release",
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
    let metric_logs = input.logs.clone();
    let count = state
        .store
        .batch_sync_market_request_logs(&market, input)
        .await?;
    state
        .metrics
        .record_market_request_logs(&market.email, &metric_logs);
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

async fn get_client_tunnel(
    State(state): State<ServerState>,
    Query(query): Query<ClientTunnelQuery>,
) -> Result<Json<ClientTunnelResponse>, AppError> {
    Ok(Json(
        state.store.get_client_tunnel(&state.config, query).await?,
    ))
}

async fn claim_client_tunnel(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ClientTunnelClaimRequest>,
) -> Result<Json<ClientTunnelResponse>, AppError> {
    Ok(Json(
        state
            .store
            .claim_client_tunnel(
                &state.config,
                input,
                extract_client_metadata(&headers, addr),
            )
            .await?,
    ))
}

async fn update_client_tunnel(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ClientTunnelUpdateRequest>,
) -> Result<Json<ClientTunnelResponse>, AppError> {
    Ok(Json(
        state
            .store
            .update_client_tunnel(
                &state.config,
                input,
                extract_client_metadata(&headers, addr),
            )
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

async fn share_api_context(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ShareApiContextResponse>, AppError> {
    let route = share_route_from_headers(&state, &headers).await?;
    let share_id = route
        .share_id()
        .ok_or_else(|| AppError::NotFound("share route not found".into()))?
        .to_string();
    Ok(Json(ShareApiContextResponse {
        mode: "share".to_string(),
        share_id,
        subdomain: route.subdomain().to_string(),
    }))
}

async fn share_api_auth_me(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ShareApiAuthQuery>,
) -> Result<Json<ShareApiAuthResponse>, AppError> {
    let route = share_route_from_headers(&state, &headers).await?;
    let share_id = route
        .share_id()
        .ok_or_else(|| AppError::NotFound("share route not found".into()))?;
    Ok(Json(
        share_api_auth_response(&state, &headers, share_id, query.email.as_deref()).await?,
    ))
}

async fn share_api_share(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ShareApiAuthQuery>,
) -> Result<Json<ShareApiShareResponse>, AppError> {
    let route = share_route_from_headers(&state, &headers).await?;
    let share_id = route
        .share_id()
        .ok_or_else(|| AppError::NotFound("share route not found".into()))?
        .to_string();
    let auth = share_api_auth_response(&state, &headers, &share_id, query.email.as_deref()).await?;
    if !auth.authenticated {
        return Err(AppError::Unauthorized("api token required".into()));
    }
    if !auth.can_manage {
        return Err(AppError::Forbidden(
            "only share owner api token can view share settings".into(),
        ));
    }
    let viewer_email = auth.user.as_ref().map(|user| user.email.as_str());
    let share = state
        .store
        .share_view_for_share_url(
            &share_id,
            &state.proxy.active_subdomains().await.into_iter().collect(),
            &state.proxy.inflight_by_share().await,
            viewer_email,
        )
        .await?;
    Ok(Json(ShareApiShareResponse { share, auth }))
}

async fn share_api_update_settings(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ShareApiAuthQuery>,
    Json(input): Json<ShareSettingsUpdateRequest>,
) -> Result<Json<crate::models::ShareSettingsUpdateResponse>, AppError> {
    let route = share_route_from_headers(&state, &headers).await?;
    let share_id = route
        .share_id()
        .ok_or_else(|| AppError::NotFound("share route not found".into()))?
        .to_string();
    let auth = share_api_auth_response(&state, &headers, &share_id, query.email.as_deref()).await?;
    if !auth.can_manage {
        return Err(AppError::Forbidden(
            "only share owner api token can edit share settings".into(),
        ));
    }
    let email = auth
        .user
        .map(|user| user.email)
        .ok_or_else(|| AppError::Unauthorized("api token required".into()))?;
    let response = update_share_settings_with_email(&state, &share_id, &email, input.patch).await?;
    Ok(Json(response))
}

async fn share_route_from_headers(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<crate::proxy::RouteEntry, AppError> {
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    state
        .proxy
        .backend_for_host(host, &state.config.tunnel_domain)
        .await
        .filter(|route| route.share_id().is_some())
        .ok_or_else(|| AppError::NotFound("share route not found".into()))
}

async fn share_api_auth_response(
    state: &ServerState,
    headers: &HeaderMap,
    share_id: &str,
    requested_email: Option<&str>,
) -> Result<ShareApiAuthResponse, AppError> {
    let Some(token) = extract_router_api_token(headers) else {
        return Ok(ShareApiAuthResponse {
            authenticated: false,
            user: None,
            can_manage: false,
        });
    };
    let Some(principal) = state
        .store
        .resolve_user_api_token(token, "share:write")
        .await?
    else {
        return Ok(ShareApiAuthResponse {
            authenticated: false,
            user: None,
            can_manage: false,
        });
    };
    if let Some(email) = requested_email
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if !principal.email.eq_ignore_ascii_case(email) {
            return Err(AppError::Unauthorized(
                "api token does not belong to requested email".into(),
            ));
        }
    }
    let owner = state.store.lookup_share_owner_email(share_id).await?;
    let can_manage = owner
        .as_deref()
        .is_some_and(|owner| owner.eq_ignore_ascii_case(&principal.email));
    Ok(ShareApiAuthResponse {
        authenticated: true,
        user: Some(ShareApiAuthUser {
            email: principal.email,
            scopes: principal.scopes,
        }),
        can_manage,
    })
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

fn live_request_context_by_request_id(
    snapshot: &RecentTrafficSnapshot,
) -> HashMap<String, (Option<String>, Option<String>, Option<String>)> {
    snapshot
        .events
        .iter()
        .filter(|event| {
            event.user_country.is_some()
                || event.user_country_iso3.is_some()
                || event.user_email.is_some()
        })
        .map(|event| {
            (
                event.request_id.clone(),
                (
                    event.user_country.clone(),
                    event.user_country_iso3.clone(),
                    event.user_email.clone(),
                ),
            )
        })
        .collect()
}

fn enrich_market_request_logs_with_live_country(
    logs: &mut [crate::models::MarketRequestLogEntry],
    snapshot: &RecentTrafficSnapshot,
) {
    let context_by_request_id = live_request_context_by_request_id(snapshot);
    for log in logs {
        if log.user_country.is_some() && log.user_country_iso3.is_some() {
            continue;
        }
        if let Some((user_country, user_country_iso3, _)) =
            context_by_request_id.get(&log.request_id)
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
        user_email: log.user_email.clone(),
        started_at: chrono::DateTime::<chrono::Utc>::from_timestamp(log.created_at, 0)
            .unwrap_or_else(chrono::Utc::now),
        is_inflight: false,
        is_health_check: log.is_health_check,
        health_status: log.is_health_check.then(|| {
            if (200..400).contains(&log.status_code) {
                "success".to_string()
            } else {
                "failed".to_string()
            }
        }),
        health_app_type: log.is_health_check.then(|| log.app_type.clone()),
        health_model: log.is_health_check.then(|| {
            if log.requested_model.is_empty() {
                log.model.clone()
            } else {
                log.requested_model.clone()
            }
        }),
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
        user_email: log.user_email.clone(),
        started_at: parse_dashboard_log_time(&log.created_at),
        is_inflight: false,
        is_health_check: false,
        health_status: None,
        health_app_type: None,
        health_model: None,
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
) -> Result<Response, AppError> {
    let response = state.store.verify_email_code(&state.config, input).await?;
    Ok(with_session_cookie(&state, Json(response)))
}

async fn verify_client_web_email_code(
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
) -> Result<Response, AppError> {
    let response = state.store.refresh_session(&state.config, input).await?;
    Ok(with_session_cookie(&state, Json(response)))
}

async fn logout_session(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if let Some(access_token) = extract_bearer_token(&headers) {
        state
            .store
            .revoke_session_by_access_token(access_token)
            .await?;
    }
    if let Some(access_token) = extract_router_access_cookie(&headers) {
        state
            .store
            .revoke_session_by_access_token(access_token)
            .await?;
    }
    Ok(with_clear_session_cookie(
        &state,
        Json(serde_json::json!({ "ok": true })),
    ))
}

async fn session_me(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<SessionStatusQuery>,
) -> Result<Json<SessionStatusResponse>, AppError> {
    if dev_auth_bypass_enabled() && extract_session_token(&headers).is_none() {
        return Ok(Json(dev_session_status()));
    }
    let mut response = state
        .store
        .session_status(
            extract_session_token(&headers),
            query.installation_id.as_deref(),
        )
        .await?;
    if let Some(user) = response.user.as_ref() {
        response.is_admin = state.dynamic.read().await.is_admin(&user.email);
    }
    Ok(Json(response))
}

async fn get_default_api_token(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<UserApiTokenResponse>, AppError> {
    let email = require_session_email(&state, &headers).await?;
    Ok(Json(state.store.get_default_api_token(&email).await?))
}

async fn reset_default_api_token(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<UserApiTokenResetResponse>, AppError> {
    let email = require_session_email(&state, &headers).await?;
    Ok(Json(state.store.reset_default_api_token(&email).await?))
}

async fn my_shares(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<UserSharesResponse>, AppError> {
    let email = require_user_email(&state, &headers, "share:read").await?;
    Ok(Json(
        state
            .store
            .list_user_shares(
                &state.config,
                &email,
                &state.proxy.active_subdomains().await.into_iter().collect(),
                &state.proxy.inflight_by_share().await,
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
    if let Some(route) = state
        .proxy
        .backend_for_host(&host, &state.config.tunnel_domain)
        .await
    {
        if !route.is_client_web()
            && matches!(*req.method(), Method::GET | Method::HEAD)
            && is_router_share_ui_path(req.uri().path())
            && !is_market_host(&state, &host).await
        {
            if let Some(response) = ui_response_for_request_path(req.uri().path()) {
                return response;
            }
            if let Some(response) = ui_response("index.html") {
                return response;
            }
        }
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
        if matches!(*req.method(), Method::GET | Method::HEAD)
            && is_router_share_ui_path(req.uri().path())
            && !is_market_host(&state, &request_host(&req)).await
        {
            if let Some(response) = ui_response_for_request_path(req.uri().path()) {
                return response;
            }
            if let Some(response) = ui_response("index.html") {
                return response;
            }
        }
        return proxy_handler(State(state), ConnectInfo(peer), req).await;
    }
    if matches!(*req.method(), Method::GET | Method::HEAD) {
        if let Some(response) = ui_response_for_request_path(req.uri().path()) {
            return response;
        }
    }
    proxy_handler(State(state), ConnectInfo(peer), req).await
}

/// True if the request's Host belongs to a market subdomain. Used to skip the
/// router's bundled "share landing UI" so the market's own web app reaches
/// the user. Naming heuristic (`market` / `market-*`, see
/// `Config::is_market_subdomain`) catches the common case cheaply; the DB
/// lookup catches market deployments that registered under another name.
async fn is_market_host(state: &ServerState, host: &str) -> bool {
    let Some(subdomain) = crate::proxy::subdomain_for_host(host, &state.config.tunnel_domain)
    else {
        return false;
    };
    if state.config.is_market_subdomain(&subdomain) {
        return true;
    }
    state.store.is_market_subdomain(&subdomain).await
}

fn is_router_share_ui_path(path: &str) -> bool {
    path == "/"
        || path == "/favicon.ico"
        || path == "/router-logo.svg"
        || path == "/world-map.svg"
        || path.starts_with("/_next/")
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
    fn router_api_token_extraction_accepts_share_client_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-goog-api-key", "gemini-router-token".parse().unwrap());
        assert_eq!(
            extract_router_api_token(&headers),
            Some("gemini-router-token")
        );

        headers.insert("x-api-key", "router-token".parse().unwrap());
        assert_eq!(extract_router_api_token(&headers), Some("router-token"));

        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer bearer-token".parse().unwrap(),
        );
        assert_eq!(extract_router_api_token(&headers), Some("bearer-token"));
    }

    #[test]
    fn share_ui_static_paths_do_not_capture_api_requests() {
        for path in [
            "/",
            "/favicon.ico",
            "/router-logo.svg",
            "/world-map.svg",
            "/_next/static/chunks/app.js",
        ] {
            assert!(is_router_share_ui_path(path), "{path} should be router UI");
        }

        for path in [
            "/v1/messages",
            "/v1/chat/completions",
            "/share-api/share",
            "/api/health",
            "/assets/index.js",
        ] {
            assert!(
                !is_router_share_ui_path(path),
                "{path} should not be router UI"
            );
        }
    }

    #[test]
    fn clear_session_cookie_covers_host_and_domain_cookie() {
        let cookies = build_clear_session_cookies("jptokenswitch.cc", false);
        assert_eq!(cookies.len(), 2);
        assert!(cookies[0].contains("cc_switch_router_access="));
        assert!(cookies[0].contains("Max-Age=0"));
        assert!(!cookies[0].contains("Domain="));
        assert!(cookies[1].contains("Domain=.jptokenswitch.cc"));
        assert!(cookies[1].contains("Secure"));
    }

    #[test]
    fn clear_session_cookie_omits_domain_for_localhost() {
        let cookies = build_clear_session_cookies("localhost", true);
        assert_eq!(cookies.len(), 1);
        assert!(cookies[0].contains("Max-Age=0"));
        assert!(!cookies[0].contains("Domain="));
    }

    #[test]
    fn confirmed_request_events_accepts_market_request_logs() {
        let event = RecentRequestEvent {
            request_id: "req_market_confirmed".into(),
            share_id: "share-1".into(),
            share_name: Some("Share".into()),
            share_subdomain: Some("share-sub".into()),
            user_country: Some("US".into()),
            user_country_iso3: Some("USA".into()),
            user_email: Some("user@example.com".into()),
            started_at: Utc::now(),
            is_inflight: true,
            is_health_check: false,
            health_status: None,
            health_app_type: None,
            health_model: None,
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
            shares: Vec::new(),
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
                error_message: None,
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
            shares: Vec::new(),
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
            shares: Vec::new(),
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
            user_email: Some("live-user@example.com".into()),
            started_at: Utc::now(),
            is_inflight: true,
            is_health_check: false,
            health_status: None,
            health_app_type: None,
            health_model: None,
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
            shares: Vec::new(),
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
            user_email: None,
            created_at,
            is_health_check: false,
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

    #[test]
    fn gemini_app_probe_uses_canonical_model_name() {
        let probe = app_probe_for_kind("gemini", "text").expect("gemini probe should exist");

        assert_eq!(
            probe.path,
            "/v1beta/models/gemini-2.5-flash:generateContent"
        );
    }

    #[test]
    fn codex_image_test_requires_enabled_bound_provider() {
        let share = ShareForTest {
            subdomain: "share-sub".into(),
            owner_email: "owner@example.com".into(),
            shared_with_emails: Vec::new(),
            bindings: std::collections::BTreeMap::from([("codex".into(), "provider-1".into())]),
            app_providers: crate::models::ShareAppProviders {
                codex: vec![crate::models::ShareAppProvider {
                    id: "provider-1".into(),
                    name: "OpenAI Official".into(),
                    app: "codex".into(),
                    kind: Some("official_oauth".into()),
                    provider_type: Some("codex_oauth".into()),
                    is_current: true,
                    enabled: true,
                    codex_image_generation_enabled: true,
                    for_sale_official_price_percent: None,
                    account_email: None,
                    api_url: None,
                    quota: None,
                    models: Vec::new(),
                }],
                ..crate::models::ShareAppProviders::default()
            },
        };

        assert!(share_codex_image_generation_enabled(&share));
    }

    #[test]
    fn codex_image_test_rejects_unbound_provider_capability() {
        let share = ShareForTest {
            subdomain: "share-sub".into(),
            owner_email: "owner@example.com".into(),
            shared_with_emails: Vec::new(),
            bindings: std::collections::BTreeMap::from([("codex".into(), "provider-1".into())]),
            app_providers: crate::models::ShareAppProviders {
                codex: vec![crate::models::ShareAppProvider {
                    id: "provider-2".into(),
                    name: "Other Codex".into(),
                    app: "codex".into(),
                    kind: Some("official_oauth".into()),
                    provider_type: Some("codex_oauth".into()),
                    is_current: false,
                    enabled: true,
                    codex_image_generation_enabled: true,
                    for_sale_official_price_percent: None,
                    account_email: None,
                    api_url: None,
                    quota: None,
                    models: Vec::new(),
                }],
                ..crate::models::ShareAppProviders::default()
            },
        };

        assert!(!share_codex_image_generation_enabled(&share));
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
    let current_user_email = require_user_email(&state, &headers, "share:write").await?;
    Ok(Json(
        update_share_settings_with_email(&state, &share_id, &current_user_email, input.patch)
            .await?,
    ))
}

async fn share_usage_by_email(
    State(state): State<ServerState>,
    Path(share_id): Path<String>,
    Query(query): Query<ShareUsageByEmailQuery>,
) -> Result<Json<crate::models::ShareUsageByEmailResponse>, AppError> {
    Ok(Json(
        state
            .store
            .share_usage_by_email(
                &share_id,
                query.app.as_deref().unwrap_or("claude"),
                query.period.as_deref().unwrap_or("24h"),
            )
            .await?,
    ))
}

async fn update_share_settings_with_email(
    state: &ServerState,
    share_id: &str,
    current_user_email: &str,
    patch: ShareSettingsPatch,
) -> Result<crate::models::ShareSettingsUpdateResponse, AppError> {
    let mut response = state
        .store
        .create_share_settings_edit(share_id, current_user_email, patch)
        .await?;

    // Happy path: if the owning installation is online and supports the control
    // API, apply the (normalized) patch synchronously by calling the client's
    // local `/_ctl/apply_share_settings` over its reverse tunnel. The client
    // stays authoritative — it applies to its own config and reports back the
    // descriptor it wrote; the store only persists that report after verifying
    // it satisfies the patch. Transport failures fall back to the async path;
    // a client that rejects or under-applies surfaces as a hard error.
    let installation_id = response.edit.installation_id.clone();
    let route = state.proxy.route_by_share_id(share_id).await;
    let control_secret = state
        .store
        .installation_control_secret(&installation_id)
        .await
        .unwrap_or(None);

    if let (Some(route), Some(secret)) = (route, control_secret) {
        match crate::ctl_client::apply_share_settings(
            route.route_target(),
            &installation_id,
            &secret,
            share_id,
            &response.edit.patch,
        )
        .await
        {
            Ok(returned_share) => {
                state
                    .store
                    .apply_share_edit_directly(&response.edit.id, returned_share)
                    .await?;
                response.applied_synchronously = true;
                return Ok(response);
            }
            Err(err) if err.is_transport() => {
                tracing::info!(
                    share_id = %share_id,
                    installation_id = %installation_id,
                    error = %err,
                    "control RPC unavailable; falling back to async share edit"
                );
                // fall through to async path
            }
            Err(err) => {
                let message = err.to_string();
                let _ = state
                    .store
                    .mark_share_edit_rejected(&response.edit.id, &message)
                    .await;
                return Err(AppError::UnprocessableEntity(message));
            }
        }
    }

    let _ = state.share_edit_events.send(ShareEditAvailableEvent {
        kind: "share_edit_available".to_string(),
        installation_id: response.edit.installation_id.clone(),
        share_id: response.edit.share_id.clone(),
        revision: response.edit.revision,
    });
    schedule_share_edit_wake_retries(state.clone(), response.edit.clone());
    Ok(response)
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
    let live_context_map = live_request_context_by_request_id(&snapshot);
    let metric_logs = input.logs.clone();
    state
        .store
        .batch_sync_share_request_logs(
            input,
            extract_client_metadata(&headers, addr),
            "",
            live_context_map,
        )
        .await?;
    state.metrics.record_share_request_logs(&metric_logs);
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
        &refresh.share_id,
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

fn extract_session_token(headers: &HeaderMap) -> Option<&str> {
    extract_bearer_token(headers).or_else(|| extract_router_access_cookie(headers))
}

fn extract_router_access_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookie| {
            cookie.split(';').find_map(|part| {
                let (name, value) = part.trim().split_once('=')?;
                (name == ROUTER_ACCESS_COOKIE)
                    .then_some(value.trim())
                    .filter(|value| !value.is_empty())
            })
        })
}

fn with_session_cookie(
    state: &ServerState,
    Json(response): Json<VerifyEmailCodeResponse>,
) -> Response {
    let cookie = build_session_cookie(
        &state.config.tunnel_domain,
        state.config.use_localhost,
        &response.access_token,
        response.expires_at,
    );
    let mut output = Json(response).into_response();
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        output.headers_mut().append(header::SET_COOKIE, value);
    }
    output
}

fn with_clear_session_cookie<T: Serialize>(
    state: &ServerState,
    Json(response): Json<T>,
) -> Response {
    let cookies =
        build_clear_session_cookies(&state.config.tunnel_domain, state.config.use_localhost);
    let mut output = Json(response).into_response();
    for cookie in cookies {
        if let Ok(value) = HeaderValue::from_str(&cookie) {
            output.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    output
}

fn build_session_cookie(
    tunnel_domain: &str,
    use_localhost: bool,
    access_token: &str,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> String {
    let max_age = (expires_at - chrono::Utc::now()).num_seconds().max(0);
    let mut parts = vec![
        format!("{ROUTER_ACCESS_COOKIE}={access_token}"),
        "Path=/".to_string(),
        "HttpOnly".to_string(),
        "SameSite=Lax".to_string(),
        format!("Max-Age={max_age}"),
    ];
    if !use_localhost && cookie_domain_allowed(tunnel_domain) {
        parts.push(format!(
            "Domain=.{}",
            tunnel_domain.trim().trim_end_matches('.')
        ));
        parts.push("Secure".to_string());
    }
    parts.join("; ")
}

fn build_clear_session_cookies(tunnel_domain: &str, use_localhost: bool) -> Vec<String> {
    let base = vec![
        format!("{ROUTER_ACCESS_COOKIE}="),
        "Path=/".to_string(),
        "HttpOnly".to_string(),
        "SameSite=Lax".to_string(),
        "Max-Age=0".to_string(),
        "Expires=Thu, 01 Jan 1970 00:00:00 GMT".to_string(),
    ];
    let mut cookies = vec![base.join("; ")];
    if !use_localhost && cookie_domain_allowed(tunnel_domain) {
        let mut domain_cookie = base;
        domain_cookie.push(format!(
            "Domain=.{}",
            tunnel_domain.trim().trim_end_matches('.')
        ));
        domain_cookie.push("Secure".to_string());
        cookies.push(domain_cookie.join("; "));
    }
    cookies
}

fn cookie_domain_allowed(tunnel_domain: &str) -> bool {
    let value = tunnel_domain.trim().trim_end_matches('.');
    if value.eq_ignore_ascii_case("localhost") || value.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    value.contains('.')
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

async fn authenticate_gateway(
    state: &ServerState,
    headers: &HeaderMap,
    required_scope: &str,
    action: &str,
    body_sha256_hex: &str,
) -> Result<GatewayRegistryRecord, AppError> {
    let gateway_id = required_header(headers, "x-cc-gateway-id")?;
    let timestamp_ms = required_header(headers, "x-cc-gateway-timestamp-ms")?
        .parse::<i64>()
        .map_err(|_| AppError::Unauthorized("invalid gateway timestamp".into()))?;
    let nonce = required_header(headers, "x-cc-gateway-nonce")?;
    let signature = required_header(headers, "x-cc-gateway-signature")?;
    state
        .store
        .authenticate_gateway_signed_request(
            gateway_id,
            required_scope,
            action,
            body_sha256_hex,
            timestamp_ms,
            nonce,
            signature,
        )
        .await
}

fn required_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, AppError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::Unauthorized(format!("missing {name} header")))
}

fn json_body_sha256_hex<T: Serialize>(value: &T) -> Result<String, AppError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|e| AppError::Internal(format!("serialize signed gateway body failed: {e}")))?;
    Ok(sha256_hex(&bytes))
}

fn empty_body_sha256_hex() -> String {
    sha256_hex(&[])
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

async fn extract_session_email(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<Option<String>, AppError> {
    let Some(token) = extract_session_token(headers) else {
        return Ok(dev_auth_bypass_enabled().then(dev_auth_email));
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

async fn require_user_email(
    state: &ServerState,
    headers: &HeaderMap,
    required_scope: &str,
) -> Result<String, AppError> {
    if let Some(email) = extract_session_email(state, headers).await? {
        return Ok(email);
    }
    let token = extract_router_api_token(headers)
        .ok_or_else(|| AppError::Unauthorized("authenticated user token required".into()))?;
    state
        .store
        .resolve_user_api_token(token, required_scope)
        .await?
        .map(|principal| principal.email)
        .ok_or_else(|| AppError::Unauthorized("invalid user api token".into()))
}

pub(crate) fn extract_router_api_token(headers: &HeaderMap) -> Option<&str> {
    extract_bearer_token(headers).or_else(|| {
        ["x-api-key", "x-goog-api-key"]
            .iter()
            .find_map(|name| headers.get(*name).and_then(|value| value.to_str().ok()))
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
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

pub(crate) async fn resolve_router_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<Option<AuthSession>, AppError> {
    let Some(token) = extract_session_token(headers) else {
        return Ok(dev_auth_bypass_enabled().then(dev_auth_session));
    };
    state.store.resolve_session_by_access_token(token).await
}

fn dev_auth_email() -> String {
    std::env::var("CC_SWITCH_ROUTER_DEV_AUTH_EMAIL")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "dev-admin@localhost".into())
}

fn dev_auth_bypass_enabled() -> bool {
    #[cfg(debug_assertions)]
    {
        match std::env::var("CC_SWITCH_ROUTER_DEV_AUTH_BYPASS")
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("0" | "false" | "no" | "off") => false,
            Some("1" | "true" | "yes" | "on") => true,
            Some(_) | None => true,
        }
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

fn dev_auth_session() -> AuthSession {
    let now = chrono::Utc::now();
    let email = dev_auth_email();
    AuthSession {
        session_id: "dev-auth-bypass-session".into(),
        user_id: "dev-auth-bypass-user".into(),
        email,
        installation_id: "dev-auth-bypass-installation".into(),
        access_token_hash: String::new(),
        refresh_token_hash: String::new(),
        access_expires_at: now + chrono::Duration::days(365),
        refresh_expires_at: now + chrono::Duration::days(365),
        created_at: now,
        last_used_at: now,
    }
}

fn dev_session_status() -> SessionStatusResponse {
    let session = dev_auth_session();
    SessionStatusResponse {
        authenticated: true,
        user: Some(crate::models::AuthUser {
            id: session.user_id,
            email: session.email,
        }),
        expires_at: Some(session.access_expires_at),
        installation_owner_email: None,
        is_admin: true,
    }
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
    if dev_auth_bypass_enabled() && extract_bearer_token(headers).is_none() {
        return Ok(dev_auth_session());
    }
    let session = resolve_router_session(state, headers)
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
    let session = resolve_router_session(&state, &headers).await?;
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
    let session = resolve_router_session(&state, &headers).await?;
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
    let session = resolve_router_session(&state, &headers).await?;
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
    let session = resolve_router_session(&state, &headers).await?;
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
    let session = resolve_router_session(&state, &headers).await?;
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RouterLogTailQuery {
    /// Fallback bearer for EventSource (no header support). Use HTTPS in
    /// production; tokens are short-lived (auth_session_ttl_secs).
    #[serde(default)]
    access_token: Option<String>,
}

async fn admin_router_log_tail(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<RouterLogTailQuery>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, AppError> {
    require_admin_for_stream(&state, &headers, query.access_token.as_deref()).await?;
    let stream = async_stream::stream! {
        let path = SERVICE_LOG_PATH.to_string();
        let mut offset = 0u64;
        let mut partial = String::new();
        let mut missing_reported;

        match read_last_log_lines(&path, 100) {
            Ok((lines, next_offset)) => {
                offset = next_offset;
                missing_reported = false;
                yield Ok(router_log_event("ready", serde_json::json!({
                    "path": path,
                    "tailLines": lines.len(),
                })));
                for line in lines {
                    yield Ok(router_log_line_event(&line, true));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                missing_reported = true;
                yield Ok(router_log_event("missing", serde_json::json!({
                    "path": path,
                    "message": "log file not found",
                })));
            }
            Err(err) => {
                missing_reported = true;
                yield Ok(router_log_event("error", serde_json::json!({
                    "path": path,
                    "message": format!("read log failed: {err}"),
                })));
            }
        }

        loop {
            sleep(Duration::from_secs(1)).await;
            let metadata = match tokio::fs::metadata(&path).await {
                Ok(metadata) => metadata,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    if !missing_reported {
                        missing_reported = true;
                        yield Ok(router_log_event("missing", serde_json::json!({
                            "path": path,
                            "message": "log file not found",
                        })));
                    }
                    continue;
                }
                Err(err) => {
                    yield Ok(router_log_event("error", serde_json::json!({
                        "path": path,
                        "message": format!("stat log failed: {err}"),
                    })));
                    continue;
                }
            };
            missing_reported = false;
            let len = metadata.len();
            if len < offset {
                offset = 0;
                partial.clear();
                yield Ok(router_log_event("reset", serde_json::json!({
                    "path": path,
                    "message": "log file was truncated; continuing from the beginning",
                })));
            }
            if len == offset {
                continue;
            }

            let mut file = match tokio::fs::File::open(&path).await {
                Ok(file) => file,
                Err(err) => {
                    yield Ok(router_log_event("error", serde_json::json!({
                        "path": path,
                        "message": format!("open log failed: {err}"),
                    })));
                    continue;
                }
            };
            if let Err(err) = tokio::io::AsyncSeekExt::seek(&mut file, SeekFrom::Start(offset)).await {
                yield Ok(router_log_event("error", serde_json::json!({
                    "path": path,
                    "message": format!("seek log failed: {err}"),
                })));
                continue;
            }
            let mut bytes = Vec::new();
            if let Err(err) = tokio::io::AsyncReadExt::read_to_end(&mut file, &mut bytes).await {
                yield Ok(router_log_event("error", serde_json::json!({
                    "path": path,
                    "message": format!("read log failed: {err}"),
                })));
                continue;
            }
            offset = len;
            if bytes.is_empty() {
                continue;
            }
            partial.push_str(&String::from_utf8_lossy(&bytes));
            let ended_with_newline = partial.ends_with('\n') || partial.ends_with('\r');
            let mut lines = partial
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>();
            if ended_with_newline {
                partial.clear();
            } else {
                partial = lines.pop().unwrap_or_default();
            }
            for line in lines {
                yield Ok(router_log_line_event(&line, false));
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new().interval(std::time::Duration::from_secs(15)),
    ))
}

async fn admin_router_log_download(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    require_admin_session(&state, &headers).await?;
    let bytes = tokio::fs::read(SERVICE_LOG_PATH)
        .await
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => AppError::NotFound("log file not found".into()),
            _ => AppError::Internal(format!("read log file failed: {err}")),
        })?;
    let mut response = Body::from(bytes).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"cc-switch-router.log\""),
    );
    Ok(response)
}

async fn require_admin_for_stream(
    state: &ServerState,
    headers: &HeaderMap,
    access_token: Option<&str>,
) -> Result<(), AppError> {
    let session = if let Some(token) = access_token {
        state
            .store
            .resolve_session_by_access_token(token)
            .await?
            .ok_or_else(|| AppError::Unauthorized("session not found".into()))?
    } else {
        require_admin_session(state, headers).await?
    };
    if !state.dynamic.read().await.is_admin(&session.email) {
        return Err(AppError::Forbidden("admin privilege required".into()));
    }
    Ok(())
}

fn read_last_log_lines(path: &str, max_lines: usize) -> std::io::Result<(Vec<String>, u64)> {
    const CHUNK_SIZE: usize = 8192;
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len == 0 || max_lines == 0 {
        return Ok((Vec::new(), len));
    }

    let mut pos = len;
    let mut chunks = Vec::new();
    let mut newline_count = 0usize;
    while pos > 0 && newline_count <= max_lines {
        let read_len = CHUNK_SIZE.min(pos as usize);
        pos -= read_len as u64;
        file.seek(SeekFrom::Start(pos))?;
        let mut chunk = vec![0u8; read_len];
        file.read_exact(&mut chunk)?;
        newline_count += chunk.iter().filter(|byte| **byte == b'\n').count();
        chunks.push(chunk);
    }

    chunks.reverse();
    let bytes = chunks.concat();
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines().map(str::to_string).collect::<Vec<_>>();
    if lines.len() > max_lines {
        lines = lines.split_off(lines.len() - max_lines);
    }
    Ok((lines, len))
}

fn router_log_line_event(line: &str, historical: bool) -> Event {
    router_log_event(
        "line",
        serde_json::json!({
            "line": clamp_log_line(line),
            "historical": historical,
        }),
    )
}

fn router_log_event(event: &'static str, payload: serde_json::Value) -> Event {
    Event::default()
        .event(event)
        .data(serde_json::to_string(&payload).unwrap_or_default())
}

fn clamp_log_line(line: &str) -> String {
    const MAX_CHARS: usize = 16 * 1024;
    if line.chars().count() <= MAX_CHARS {
        return line.to_string();
    }
    let mut value = line.chars().take(MAX_CHARS).collect::<String>();
    value.push_str(" ...[truncated]");
    value
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

async fn admin_metrics_snapshot(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<crate::metrics::models::MetricsSnapshot>, AppError> {
    require_admin_session(&state, &headers).await?;
    Ok(Json(
        state.metrics.snapshot(&state.config, &state.proxy).await?,
    ))
}

async fn admin_metrics_host_info(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<crate::metrics::models::HostMetricsInfo>, AppError> {
    require_admin_session(&state, &headers).await?;
    Ok(Json(state.metrics.host_info(&state.config).await))
}

async fn admin_metrics_host_status(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<crate::metrics::models::HostMetricsStatus>, AppError> {
    require_admin_session(&state, &headers).await?;
    Ok(Json(state.metrics.current_host_status(&state.config).await))
}

async fn admin_metrics_series(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<crate::metrics::models::MetricsRangeQuery>,
) -> Result<Json<crate::metrics::models::MetricsSeriesResponse>, AppError> {
    require_admin_session(&state, &headers).await?;
    let range = query.range.unwrap_or_else(|| "1h".into());
    let range_secs = crate::metrics::store::parse_duration_to_secs(&range)
        .ok_or_else(|| AppError::BadRequest("invalid metrics range".into()))?;
    let step = query
        .step
        .unwrap_or_else(|| crate::metrics::store::default_step_label(range_secs));
    Ok(Json(state.metrics.store().series(range, step).await?))
}

async fn admin_metrics_llm_snapshot(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<crate::metrics::models::MetricsRangeQuery>,
) -> Result<Json<crate::metrics::models::LlmMetricsSnapshot>, AppError> {
    require_admin_session(&state, &headers).await?;
    let range = query.range.unwrap_or_else(|| "5m".into());
    let range_secs = crate::metrics::store::parse_duration_to_secs(&range)
        .ok_or_else(|| AppError::BadRequest("invalid metrics range".into()))?;
    Ok(Json(state.metrics.store().llm_snapshot(range_secs).await?))
}

async fn admin_metrics_llm_top(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<crate::metrics::models::MetricsRangeQuery>,
) -> Result<Json<crate::metrics::models::LlmTopResponse>, AppError> {
    require_admin_session(&state, &headers).await?;
    let range = query.range.unwrap_or_else(|| "1h".into());
    let by = query
        .by
        .or(query.group_by)
        .unwrap_or_else(|| "tokens".into());
    Ok(Json(
        state
            .metrics
            .store()
            .llm_top(range, by, query.limit.unwrap_or(10).min(50))
            .await?,
    ))
}

async fn admin_metrics_events(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<crate::metrics::models::MetricsRangeQuery>,
) -> Result<Json<Vec<crate::metrics::models::MetricEvent>>, AppError> {
    require_admin_session(&state, &headers).await?;
    Ok(Json(
        state
            .metrics
            .store()
            .events(query.limit.unwrap_or(100).min(500))
            .await?,
    ))
}

async fn admin_metrics_llm_failover(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<crate::metrics::models::MetricsRangeQuery>,
) -> Result<Json<crate::metrics::models::LlmReliabilityResponse>, AppError> {
    require_admin_session(&state, &headers).await?;
    let range = query.range.unwrap_or_else(|| "1h".into());
    Ok(Json(
        state
            .metrics
            .store()
            .llm_reliability(range, query.limit.unwrap_or(10).min(50))
            .await?,
    ))
}

async fn admin_metrics_clear(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<Json<crate::metrics::models::ClearMetricsResponse>, AppError> {
    let session = require_admin_session(&state, &headers).await?;
    let result = state.metrics.store().clear().await?;
    let payload =
        serde_json::to_value(&result).unwrap_or_else(|_| serde_json::json!({ "ok": true }));
    let metadata = extract_client_metadata(&headers, addr);
    let _ = state
        .store
        .record_admin_audit(
            Some(&session.email),
            "metrics.clear",
            Some(&payload),
            metadata.ip.as_deref(),
        )
        .await;
    Ok(Json(result))
}

// ─────────────────────────────────────────────────────────────────────────────
// P18: test-connection — dashboard 可以通过 share 的 subdomain + 调用者的 api token
// 向 claude / codex / gemini 发一个最小探针（max_tokens=1），把原始 HTTP
// 响应回传给前端展示。后端中转是因为 share subdomain 不同源，CORS 不通。
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareConnectionTestRequest {
    /// "claude" | "codex" | "gemini"
    app: String,
    /// "text" | "image"; image is only supported for codex shares.
    #[serde(default)]
    kind: Option<String>,
    /// 可选，毫秒；默认 15000，上限 30000
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareConnectionTestResponse {
    request: TestRequestEcho,
    response: Option<TestResponseEcho>,
    duration_ms: u64,
    /// 网络层错误（DNS / 连接 / 超时）时填写
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ImageGenerationJobsQuery {
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageGenerationJobsResponse {
    jobs: Vec<ImageGenerationJobEntry>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TestRequestEcho {
    method: String,
    url: String,
    headers: Vec<[String; 2]>,
    body: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TestResponseEcho {
    status_code: u16,
    status_text: String,
    headers: Vec<[String; 2]>,
    body_text: String,
    body_truncated: bool,
}

const TEST_BODY_CAP: usize = 64 * 1024;

struct AppProbe {
    method: &'static str,
    path: &'static str,
    body: &'static str,
}

fn app_probe_for_kind(app: &str, kind: &str) -> Option<AppProbe> {
    match (app, kind) {
        ("claude", "text") => Some(AppProbe {
            method: "POST",
            path: "/v1/messages",
            // cc-switch client 的 ensure_claude_oauth_billing_header_system 会在
            // ClaudeOAuth provider 路径下自动注入 x-anthropic-billing-header system
            // 块 + cch 签名，所以这里用最简形式即可——与 api.anthropic.com 官方文档
            // 示例完全一致。
            //
            // `stream: true` 是为了兼容 GitHub Copilot 这类 Anthropic-on-OpenAI 绑定：
            // cc-switch 在非流式路径上会把上游响应做 openai_to_anthropic 转换；如果
            // 上游碰巧返回 `choices: []`（短回复、Copilot 内部行为等），转换器会抛
            // "Empty choices array"。claude-cli 真实流量恒为 stream:true，走 SSE
            // passthrough 不触发这条转换，所以也对齐这一行为。
            body: r#"{"model":"claude-opus-4-7","max_tokens":16,"stream":true,"messages":[{"role":"user","content":"who are you"}]}"#,
        }),
        ("codex", "text") => Some(AppProbe {
            method: "POST",
            path: "/v1/responses",
            // gpt-5 系 Responses API：input 必须是 message 数组（"Input must be a
            // list"），不能是裸字符串。max_output_tokens=16 是允许 reasoning trace
            // 启动的最小值。
            body: r#"{"model":"gpt-5.5","input":[{"role":"user","content":"who are you"}],"max_output_tokens":16}"#,
        }),
        ("codex", "image") => Some(AppProbe {
            method: "POST",
            path: "/v1/images/generations/async",
            body: r#"{"model":"gpt-5.5","prompt":"A small robot painting a sunrise","size":"1024x1024","response_format":"b64_json","output_format":"png"}"#,
        }),
        ("gemini", "text") => Some(AppProbe {
            method: "POST",
            path: "/v1beta/models/gemini-2.5-flash:generateContent",
            // 同 claude/codex 思路：避免极小 maxOutputTokens 触发上游 OAuth 网关的
            // 探针检测。Gemini 2.5 Flash 也是 reasoning model。
            body: r#"{"contents":[{"parts":[{"text":"who are you"}]}],"generationConfig":{"maxOutputTokens":16}}"#,
        }),
        _ => None,
    }
}

fn share_codex_image_generation_enabled(share: &ShareForTest) -> bool {
    let Some(bound_provider_id) = share
        .bindings
        .get("codex")
        .map(String::as_str)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };

    share.app_providers.codex.iter().any(|provider| {
        provider.id == bound_provider_id
            && provider.enabled
            && provider.codex_image_generation_enabled
    })
}

async fn list_share_image_generation_jobs(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
    Query(query): Query<ImageGenerationJobsQuery>,
) -> Result<Json<ImageGenerationJobsResponse>, AppError> {
    require_share_image_job_view_access(&state, &headers, &share_id).await?;

    let jobs = state
        .store
        .list_image_generation_jobs_for_share(&share_id, query.limit.unwrap_or(50))
        .await?;
    Ok(Json(ImageGenerationJobsResponse { jobs }))
}

async fn get_share_image_generation_job_result(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((share_id, job_id)): Path<(String, String)>,
) -> Result<Response, AppError> {
    require_share_image_job_view_access(&state, &headers, &share_id).await?;
    let job = state
        .store
        .get_image_generation_job(&job_id)
        .await?
        .ok_or_else(|| AppError::NotFound("image job not found".into()))?;
    if job.share_id != share_id {
        return Err(AppError::NotFound("image job not found".into()));
    }
    if job.status != "succeeded" {
        return Err(AppError::Conflict("image job result is not ready".into()));
    }
    if job
        .expires_at
        .map(|expires_at| expires_at < chrono::Utc::now().timestamp())
        .unwrap_or(true)
    {
        return Err(AppError::NotFound("image job result expired".into()));
    }
    let key = job
        .result_storage_key
        .as_deref()
        .filter(|key| is_safe_image_result_key(key))
        .ok_or_else(|| AppError::NotFound("image result not found".into()))?;
    let path = image_result_path(&state.config, key);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| AppError::NotFound("image result not found".into()))?;
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = StatusCode::OK;
    if let Some(mime) = job.result_mime_type.as_deref()
        && let Ok(value) = HeaderValue::from_str(mime)
    {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=300"),
    );
    Ok(response)
}

async fn require_share_image_job_view_access(
    state: &ServerState,
    headers: &HeaderMap,
    share_id: &str,
) -> Result<(), AppError> {
    let current_user_email = require_user_email(state, headers, "share:read").await?;
    let share = state
        .store
        .get_share_for_test(share_id)
        .await?
        .ok_or_else(|| AppError::NotFound("share not found".into()))?;

    let is_admin = state.dynamic.read().await.is_admin(&current_user_email);
    let is_owner = share.owner_email.eq_ignore_ascii_case(&current_user_email);
    let is_shared_with = share
        .shared_with_emails
        .iter()
        .any(|email| email.eq_ignore_ascii_case(&current_user_email));

    if !is_admin && !is_owner && !is_shared_with {
        return Err(AppError::Forbidden(
            "only the share owner, invited users, or admins can view image jobs".into(),
        ));
    }
    Ok(())
}

fn image_result_path(config: &crate::config::Config, key: &str) -> std::path::PathBuf {
    config
        .db_path
        .parent()
        .map(|path| path.join("image-results"))
        .unwrap_or_else(|| std::path::PathBuf::from("image-results"))
        .join(key)
}

fn is_safe_image_result_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 160
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

async fn test_share_connection(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
    Json(input): Json<ShareConnectionTestRequest>,
) -> Result<Json<ShareConnectionTestResponse>, AppError> {
    let current_user_email = require_user_email(&state, &headers, "share:read").await?;

    let probe_kind = input
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("text");
    let probe = app_probe_for_kind(&input.app, probe_kind).ok_or_else(|| {
        AppError::BadRequest(format!(
            "unsupported app probe: app={} kind={probe_kind}",
            input.app
        ))
    })?;

    // Load share — verify caller is owner or admin
    let share = state
        .store
        .get_share_for_test(&share_id)
        .await?
        .ok_or_else(|| AppError::NotFound("share not found".into()))?;

    let is_admin = state.dynamic.read().await.is_admin(&current_user_email);
    let is_owner = share.owner_email.eq_ignore_ascii_case(&current_user_email);
    let is_shared_with = share
        .shared_with_emails
        .iter()
        .any(|e| e.eq_ignore_ascii_case(&current_user_email));

    if !is_admin && !is_owner && !is_shared_with {
        return Err(AppError::Forbidden(
            "only the share owner, invited users, or admins can test this share".into(),
        ));
    }

    if input.app == "codex"
        && probe_kind == "image"
        && !share_codex_image_generation_enabled(&share)
    {
        return Err(AppError::BadRequest(
            "codex image generation is not enabled for the bound provider".into(),
        ));
    }

    let subdomain = share.subdomain;

    // Fetch the caller's own api token (not the share owner's)
    let api_token = state
        .store
        .get_default_api_token(&current_user_email)
        .await
        .map_err(|e| AppError::Internal(format!("fetch api token failed: {e}")))?
        .api_token
        .ok_or_else(|| {
            AppError::Internal("api token plaintext not available; reset your token first".into())
        })?;

    // Build URLs. `public_url` is what we display in the curl preview / echo
    // back to the user. `local_url` is what reqwest actually hits — the same
    // axum HTTP listener as we're running on, addressed by 127.0.0.1, with a
    // Host header that matches the public subdomain. share proxy routes by
    // Host, so the routing decision is identical.
    let public_url = format!("{}{}", state.config.tunnel_url(&subdomain), probe.path);
    let local_url = format!("http://{}{}", state.config.api_addr, probe.path);
    let public_host = format!("{}.{}", subdomain, state.config.tunnel_domain);

    // Echo headers with redacted token for response
    let echo_headers = vec![
        [
            "Authorization".to_string(),
            format!(
                "Bearer {}...(redacted)",
                &api_token.chars().take(14).collect::<String>()
            ),
        ],
        ["Content-Type".to_string(), "application/json".to_string()],
    ];

    let timeout_ms = input.timeout_ms.unwrap_or(15_000).min(30_000);
    let client = reqwest::Client::builder()
        .user_agent("cc-switch-router/0.1 test-connection")
        .timeout(std::time::Duration::from_millis(timeout_ms))
        // No redirects: a 3xx mid-flight would otherwise drop Authorization
        // on the second hop (reqwest's default behaviour for cross-origin).
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| AppError::Internal(format!("create test client failed: {e}")))?;

    let request_echo = TestRequestEcho {
        method: probe.method.to_string(),
        url: public_url.clone(),
        headers: echo_headers,
        body: Some(probe.body.to_string()),
    };

    let started = std::time::Instant::now();
    let result = client
        .post(&local_url)
        .header("Host", &public_host)
        .bearer_auth(&api_token)
        .header("Content-Type", "application/json")
        .body(probe.body)
        .send()
        .await;
    let duration_ms = started.elapsed().as_millis() as u64;

    match result {
        Err(err) => {
            tracing::info!(
                tag = "test-connection",
                share_id = %share_id,
                app = %input.app,
                error = %err,
                duration_ms,
                "test-connection network error"
            );
            Ok(Json(ShareConnectionTestResponse {
                request: request_echo,
                response: None,
                duration_ms,
                error: Some(err.to_string()),
            }))
        }
        Ok(resp) => {
            let status_code = resp.status().as_u16();
            let status_text = resp.status().canonical_reason().unwrap_or("").to_string();
            let resp_headers: Vec<[String; 2]> = resp
                .headers()
                .iter()
                .map(|(k, v)| [k.as_str().to_string(), v.to_str().unwrap_or("").to_string()])
                .collect();
            let body_bytes = resp.bytes().await.unwrap_or_default();
            let body_truncated = body_bytes.len() > TEST_BODY_CAP;
            let body_slice = &body_bytes[..body_bytes.len().min(TEST_BODY_CAP)];
            let body_text = String::from_utf8_lossy(body_slice).into_owned();

            tracing::info!(
                tag = "test-connection",
                share_id = %share_id,
                app = %input.app,
                status = status_code,
                duration_ms,
                "test-connection completed"
            );
            Ok(Json(ShareConnectionTestResponse {
                request: request_echo,
                response: Some(TestResponseEcho {
                    status_code,
                    status_text,
                    headers: resp_headers,
                    body_text,
                    body_truncated,
                }),
                duration_ms,
                error: None,
            }))
        }
    }
}
