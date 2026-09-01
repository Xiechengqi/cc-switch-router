use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::Infallible;
use std::io::{Read, Seek, SeekFrom};
use std::net::SocketAddr;

use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::{MethodRouter, any, delete, get, patch, post, put};
use axum::{Json, Router};
use base64::Engine;
use chrono::Utc;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use tokio::time::{Duration, sleep};

use crate::ServerState;
use crate::abuse::ShareClientUnbanResponse;
use crate::admin::{
    restart::{RestartStrategy, schedule_restart},
    settings::{
        ApplyOutcome, SettingsSnapshotResponse, SettingsUpdateRequest, SettingsUpdateResponse,
        SettingsValidationResponse, apply_updates_to_dynamic, read_env_file, settings_revision,
        snapshot_response, validate_and_diff, validation_response, write_env_file_atomic,
    },
    upgrade::{UpgradeLogEntry, UpgradeStatus},
    version::{
        BINARY_INSTALL_PATH, BINARY_ROLLBACK_PATH, SERVICE_LOG_PATH, SERVICE_UNIT, ServiceManager,
        VersionResponse, build_info, detect_service_status, ensure_binary_writable,
        fetch_latest_release_meta, uptime_secs_from,
    },
};
use crate::client_meta::extract_client_metadata;
use crate::config::TelegramBotSettings;
use crate::error::AppError;
use crate::models::{
    AccountUsageResponse, AnnouncementResponse, AnnouncementSettings, AnnouncementSettingsUpdate,
    AuthSession, BindInstallationOwnerEmailRequest, BindInstallationOwnerEmailResponse,
    ChangeInstallationOwnerEmailRequest, ChangeInstallationOwnerEmailResponse,
    ClientChatDeliveriesResponse, ClientChatMessageListResponse, ClientChatReadRequest,
    ClientChatReadResponse, ClientChatRoomListResponse, ClientChatRoomLookupRequest,
    ClientChatRoomResponse, ClientChatVisitImportRequest, ClientChatVisitImportResponse,
    ClientOnlineCalendarResponse, ClientTunnelClaimRequest, ClientTunnelQuery,
    ClientTunnelResponse, ClientTunnelUpdateRequest, ClientWebRequestEmailCodeRequest,
    ClientWebVerifyEmailCodeRequest, DashboardPresenceRequest, DashboardPresenceResponse,
    DashboardResponse, DashboardTickerShare, DashboardUxEventRequest, DashboardUxEventResponse,
    GatewayRegistryRecord, GatewayRequestObservationBatch, GatewayShareView,
    GetInstallationOwnerEmailQuery, GetInstallationOwnerEmailResponse, HealthResponse,
    ImageGenerationRequestLogEntry, InstallationHeartbeatRequest, InstallationHeartbeatResponse,
    InstallationSetupCompletedRequest, InstallationSetupCompletedResponse,
    InstallationUpgradeTaskReportPayload, InstallationUpgradeTaskReportRequest,
    InstallationUpgradeTaskReportResponse, IssueLeaseRequest, IssueLeaseResponse,
    MapDisplaySettings, MapDisplaySettingsUpdate, NotificationSettingsResponse,
    PostClientChatMessageRequest, ProviderUsageResponse, PublicMapPointsResponse,
    PublicNetworkStatsResponse, RefreshSessionRequest, RegisterAuthDeviceRequest,
    RegisterAuthDeviceResponse, RegisterGatewayRequest, RegisterGatewayResponse,
    RegisterInstallationRequest, RegisterInstallationResponse, RenewLeaseRequest,
    RenewLeaseResponse, ReplaceUserModelRoutingRequest, ReportInstallationStatusRequest,
    ReportInstallationStatusResponse, RequestEmailCodeRequest, RequestEmailCodeResponse,
    SessionStatusResponse, ShareApiAuthResponse, ShareApiAuthUser, ShareApiContextResponse,
    ShareApiShareResponse, ShareBatchSyncRequest, ShareClaimSubdomainRequest, ShareDeleteRequest,
    ShareDescriptorBatchSyncResponse, ShareEditAckRequest, ShareEditAvailableEvent,
    ShareEditEventSignaturePayload, ShareHeartbeatRequest, ShareModelHealthCalendarResponse,
    SharePendingEditsRequest, SharePruneRequest, ShareRequestLogBatchSyncRequest,
    ShareRequestLogBatchSyncResponse, ShareRequestLogEntry, ShareRuntimeRefreshRequest,
    ShareSettingsPatch, ShareSettingsUpdateRequest, ShareSyncRequest,
    SubdomainAvailabilityResponse, TelegramBindLinkResponse, TunnelActivateRequest,
    TunnelStateRequest, TunnelStateResponse, UpdateNotificationSettingsRequest,
    UpdateUsageCardSettingsRequest, UpgradeInstallationRequest, UpgradeInstallationResponse,
    UpgradeInstallationStatusResponse, UsageCardSettingsResponse, UserApiTokenResetResponse,
    UserApiTokenResponse, UserModelRoutingResponse, UserModelRoutingTestHttp,
    UserModelRoutingTestRequest, UserModelRoutingTestResponse, UserSharesResponse,
    VerifyEmailCodeRequest, VerifyEmailCodeResponse,
};
use crate::notifications::{
    ClientNotificationDeliveriesResponse, ClientNotificationPolicy, NotificationTemplateContext,
    route_reconnect_grace, validate_notification_cleanup_window,
};
use crate::proxy::{
    ReleasedShareRequest, RouteAvailability, build_user_model_route_test_probe,
    execute_user_model_route_test, gateway_proxy_handler, is_user_model_api_host, proxy_handler,
    unified_model_test_curl, user_model_proxy_handler, with_unified_api_cors,
};
use crate::recent_traffic::{RecentRequestEvent, RecentTrafficSnapshot};
use crate::scheduling_signals::{
    ShareFeedbackKind, ShareFeedbackRequest, ShareFeedbackResponse, ShareHeadroomEntry,
    ShareHeadroomRequest, ShareHeadroomResponse,
};
use crate::store::{ShareForTest, image_result_path};
use tower_http::cors::{Any, CorsLayer};

fn public_cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::OPTIONS])
        .allow_headers(Any)
}

async fn apply_share_route_availability(state: &ServerState, shares: &mut [GatewayShareView]) {
    let reconnect_grace = {
        let dynamic = state.dynamic.read().await;
        route_reconnect_grace(&dynamic.client_notifications)
    };
    let routes = state
        .proxy
        .route_availability_snapshots(reconnect_grace)
        .await;
    for share in shares {
        let snapshot = routes.get(&share.subdomain);
        share.online = snapshot.is_some_and(|route| route.state == RouteAvailability::Active);
        share.route_state = snapshot
            .map(|route| route.state.as_str())
            .unwrap_or("offline")
            .to_string();
        share.route_state_since = snapshot.map(|route| route.since.to_rfc3339());
    }
}

const REGIONS: &str = include_str!("../regions");
const SHARE_EDIT_WAKE_RETRY_INTERVAL_SECS: u64 = 20;
const SHARE_EDIT_WAKE_RETRY_ATTEMPTS: usize = 3;
const DASHBOARD_REQUEST_TICKER_LIMIT: usize = 100;
const ROUTER_ACCESS_COOKIE: &str = "cc_switch_router_access";
const INSTALLATION_CONTROL_BODY_LIMIT_BYTES: usize = 16 * 1024;
const INSTALLATION_UPGRADE_TASK_PAYLOAD_BUDGET_BYTES: usize = 512 * 1024;
const INSTALLATION_UPGRADE_TASK_BODY_LIMIT_BYTES: usize =
    INSTALLATION_UPGRADE_TASK_PAYLOAD_BUDGET_BYTES + 32 * 1024;
const INSTALLATION_UPGRADE_STATUS_RESPONSE_MAX_BYTES: usize =
    INSTALLATION_UPGRADE_TASK_PAYLOAD_BUDGET_BYTES;

fn installation_control_body_limited<S>(route: MethodRouter<S>) -> MethodRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    route.layer(DefaultBodyLimit::max(INSTALLATION_CONTROL_BODY_LIMIT_BYTES))
}

fn installation_upgrade_task_body_limited<S>(route: MethodRouter<S>) -> MethodRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    route.layer(DefaultBodyLimit::max(
        INSTALLATION_UPGRADE_TASK_BODY_LIMIT_BYTES,
    ))
}

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
    period: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ShareModelHealthCalendarQuery {
    #[serde(default)]
    days: Option<u16>,
}

#[derive(Debug, Deserialize)]
struct ClientOnlineCalendarQuery {
    #[serde(default)]
    days: Option<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountUsageQuery {
    period: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublicUsageQuery {
    period: Option<String>,
    /// Cap the returned model rows. Omitted or `0` returns every row, which is
    /// what a cross-region aggregator needs: truncating each region before
    /// summing silently drops that region's long tail, and the model rows stop
    /// adding up to the total.
    models: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmbedUsageQuery {
    period: Option<String>,
    theme: Option<String>,
    models: Option<usize>,
    show_breakdown: Option<String>,
    show_models: Option<String>,
    compact: Option<String>,
    format: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallationUpgradeStatusQuery {
    task_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForceReleaseShareRequestsRequest {
    request_id: Option<String>,
    share_id: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ForceReleaseShareRequestsResponse {
    released_count: usize,
    released: Vec<ReleasedShareRequest>,
}

pub fn router(state: ServerState) -> Router {
    let middleware_state = state.clone();
    let public_api = Router::new()
        .route("/v1/healthz", get(health))
        .route("/v1/public/map-points", get(public_map_points))
        .route("/v1/public/network-stats", get(public_network_stats))
        .route("/v1/public/usage/global", get(public_usage_global))
        .route("/v1/public/embed/global.svg", get(embed_global_usage_svg))
        .route("/v1/public/embed/usage/:user_id", get(embed_user_usage_svg))
        .route("/v1/regions", get(regions))
        .route("/v1/announcement", get(announcement_get))
        .route(
            "/v1/chat/clients/:installation_id/room",
            get(client_chat_room),
        )
        .route(
            "/v1/chat/rooms/:room_id/messages",
            get(list_chat_messages).post(post_client_chat_message),
        )
        .route(
            "/v1/chat/rooms/:room_id/stream",
            get(client_chat_room_stream),
        )
        .layer(public_cors_layer())
        .with_state(state.clone());

    Router::new()
        .merge(public_api)
        .merge(crate::client_market::router())
        .merge(crate::client_market_trade::router())
        .merge(crate::client_market_terminal::router())
        .merge(crate::share_market::router())
        .merge(crate::market_access::router())
        .merge(crate::market_billing::router())
        .merge(crate::client_logs::router())
        .merge(crate::server_logs::router())
        .merge(retired_token_market_routes())
        .route("/", any(root_handler))
        .route("/install-client.sh", get(install_client_script))
        .route("/favicon.ico", get(favicon))
        .route("/v1/dashboard", get(dashboard))
        .route("/v1/map-display", get(map_display_get))
        .route("/v1/gateways/register", post(register_gateway))
        .route("/v1/gateway/shares", get(gateway_shares))
        .route("/v1/gateway/shares/headroom", post(gateway_shares_headroom))
        .route("/v1/gateway/shares/feedback", post(gateway_shares_feedback))
        .route(
            "/v1/gateway/request-logs/batch",
            post(batch_sync_gateway_request_logs),
        )
        .route("/v1/dashboard/presence", post(dashboard_presence))
        .route("/v1/dashboard/ux-events", post(dashboard_ux_event))
        .route(
            "/v1/installations/register",
            installation_control_body_limited(post(register_installation)),
        )
        .route(
            "/v1/auth/devices/register",
            installation_control_body_limited(post(register_auth_device)),
        )
        .route(
            "/v1/installations/heartbeat",
            installation_control_body_limited(post(installation_heartbeat)),
        )
        .route(
            "/v1/installations/setup-completed",
            installation_control_body_limited(post(installation_setup_completed)),
        )
        .route(
            "/v1/installations/report-status",
            post(report_installation_status),
        )
        .route(
            "/v1/installations/upgrade-task-report",
            installation_upgrade_task_body_limited(post(report_installation_upgrade_task)),
        )
        .route(
            "/v1/installations/:installation_id/upgrade",
            post(upgrade_installation),
        )
        .route(
            "/v1/installations/:installation_id/upgrade/status",
            get(upgrade_installation_status),
        )
        .route(
            "/v1/client-tunnel/subdomain-availability",
            get(check_client_tunnel_subdomain_availability),
        )
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
        .route(
            "/v1/installations/client-subdomain-takeover",
            post(client_subdomain_takeover),
        )
        .route(
            "/v1/installations/client-subdomain-takeover/authorization",
            post(client_subdomain_takeover_authorization),
        )
        .route("/v1/auth/email/request-code", post(request_email_code))
        .route("/v1/auth/email/verify-code", post(verify_email_code))
        .route(
            "/v1/client-web/auth/email/request-code",
            post(request_client_web_email_code),
        )
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
        .route(
            "/v1/me/model-routing",
            get(get_my_model_routing).put(replace_my_model_routing),
        )
        .route("/v1/me/model-routing/test", post(test_my_model_routing))
        .route(
            "/v1/me/usage-card",
            get(get_my_usage_card_settings).patch(update_my_usage_card_settings),
        )
        .route(
            "/v1/me/notifications",
            get(get_my_notification_settings).patch(update_my_notification_settings),
        )
        .route(
            "/v1/me/notifications/telegram/bind-link",
            post(create_my_telegram_bind_link),
        )
        .route(
            "/v1/me/notifications/telegram",
            delete(unbind_my_telegram_chat),
        )
        .route(
            crate::telegram::service::WEBHOOK_PATH,
            post(telegram_webhook),
        )
        .route("/v1/me/usage/consumer", get(my_usage_consumer))
        .route("/v1/me/usage/provider", get(my_usage_provider))
        .route("/v1/me/shares", get(my_shares))
        .route("/v1/tunnels/lease", post(issue_lease))
        .route("/v1/tunnels/lease/renew", post(renew_lease))
        .route("/v1/tunnels/activate", post(activate_tunnel))
        .route("/v1/tunnels/state", post(tunnel_state))
        .route("/v1/shares/claim-subdomain", post(claim_share_subdomain))
        .route("/v1/shares/sync", post(sync_share))
        .route(
            "/v1/shares/descriptor-batch-sync",
            post(batch_sync_share_descriptors),
        )
        .route("/v1/shares/batch-sync", post(batch_sync_share))
        .route("/v1/shares/runtime-refresh", post(refresh_share_runtime))
        .route(
            "/v1/shares/:share_id/settings",
            patch(update_share_settings),
        )
        .route(
            "/v1/shares/:share_id/client-bans",
            get(list_share_client_bans),
        )
        .route(
            "/v1/shares/:share_id/client-bans/:ban_id/unban",
            post(unban_share_client),
        )
        .route(
            "/v1/shares/:share_id/usage-by-email",
            get(share_usage_by_email),
        )
        .route(
            "/v1/shares/:share_id/user-limit-status",
            get(share_user_limit_status),
        )
        .route(
            "/v1/shares/:share_id/test-connection",
            post(test_share_connection),
        )
        .route(
            "/v1/shares/:share_id/model-health-calendar",
            get(get_share_model_health_calendar),
        )
        .route(
            "/v1/clients/:installation_id/online-calendar",
            get(get_client_online_calendar),
        )
        .route(
            "/v1/shares/:share_id/refresh-usage",
            post(refresh_share_usage),
        )
        .route(
            "/v1/shares/:share_id/request-logs",
            get(list_share_request_logs),
        )
        .route(
            "/v1/shares/:share_id/image-request-logs",
            get(list_share_image_generation_request_logs),
        )
        .route(
            "/v1/shares/:share_id/image-jobs",
            get(list_share_image_generation_jobs_compat),
        )
        .route(
            "/v1/image-results/:request_id",
            get(get_image_generation_result),
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
        .route("/v1/shares/prune", post(prune_shares))
        .route("/v1/chat/rooms/lookup", post(lookup_chat_rooms))
        .route("/v1/chat/rooms", get(list_visited_chat_rooms))
        .route("/v1/chat/meta", get(client_chat_meta))
        .route(
            "/v1/chat/rooms/:room_id/visit",
            put(record_client_chat_visit).delete(remove_client_chat_visit),
        )
        .route("/v1/chat/visits/import", post(import_chat_visits))
        .route("/v1/chat/rooms/:room_id/read", put(mark_client_chat_read))
        .route(
            "/v1/admin/chat/messages/:message_id",
            delete(admin_delete_client_chat_message),
        )
        .route(
            "/v1/admin/chat/deliveries",
            get(admin_client_chat_deliveries),
        )
        .route(
            "/v1/admin/chat/deliveries/:delivery_id/requeue",
            post(admin_requeue_client_chat_delivery),
        )
        .route(
            "/v1/admin/settings",
            get(admin_settings_get).patch(admin_settings_apply),
        )
        .route("/v1/admin/settings/validate", post(admin_settings_validate))
        .route(
            "/v1/admin/client-server-release/validate",
            post(admin_client_server_release_validate),
        )
        .route(
            "/v1/admin/client-notifications/deliveries",
            get(admin_client_notification_deliveries),
        )
        .route("/v1/admin/map-display", patch(admin_map_display_update))
        .route("/v1/admin/announcement", patch(admin_announcement_update))
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
        .route("/v1/admin/audit", get(admin_audit_list))
        .route(
            "/v1/admin/proxy/share-requests/force-release",
            post(admin_force_release_share_requests),
        )
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
        .route("/v1/admin/alerting/overview", get(admin_alerting_overview))
        .route("/v1/admin/alerting/channels", get(admin_alerting_channels))
        .route(
            "/v1/admin/alerting/channels/:channel/test",
            post(admin_alerting_channel_test),
        )
        .route(
            "/v1/admin/user-notifications/channels",
            get(admin_user_notification_channels),
        )
        .route(
            "/v1/admin/user-notifications/channels/:channel/test",
            post(admin_user_notification_channel_test),
        )
        .route(
            "/v1/admin/alerting/incidents/:incident_id/acknowledge",
            post(admin_alerting_incident_acknowledge),
        )
        .route(
            "/v1/admin/alerting/incidents/:incident_id/silence",
            post(admin_alerting_incident_silence),
        )
        .route(
            "/v1/admin/alerting/incidents/:incident_id/resume",
            post(admin_alerting_incident_resume),
        )
        .route(
            "/v1/admin/alerting/deliveries/:delivery_id/retry",
            post(admin_alerting_delivery_retry),
        )
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
    let is_user_model_api =
        is_user_model_api_host(request_host(&req).as_str(), &state.config.tunnel_domain);
    // Telegram's webhook senders live in ranges the operator does not control
    // and cannot enumerate; a blacklist entry that happens to cover one would
    // break account binding silently. The route carries its own authentication
    // (the `setWebhook` secret header), so exempting it costs nothing.
    if !is_user_model_api && req.uri().path() == crate::telegram::service::WEBHOOK_PATH {
        return next.run(req).await;
    }
    if let Some(ip) = source_ip_from_request(&req)
        && state.dynamic.read().await.is_ip_blacklisted(ip)
    {
        state.ip_blacklist_stats.record(ip, req.uri().path());
        let response = (StatusCode::FORBIDDEN, "IP blacklisted").into_response();
        return if is_user_model_api {
            with_unified_api_cors(response)
        } else {
            response
        };
    }
    if is_user_model_api {
        if req.uri().path() == "/v1/healthz" && matches!(*req.method(), Method::GET | Method::HEAD)
        {
            return with_unified_api_cors(next.run(req).await);
        }
        let peer = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0)
            .unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 0)));
        return user_model_proxy_handler(State(state), ConnectInfo(peer), req).await;
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

/// Retired Token Market routes fail closed with an explicit migration
/// response. They must never fall through to the generic proxy/UI handler,
/// which could otherwise expose a Client tunnel.
fn retired_token_market_routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/v1/markets", any(retired_capacity_endpoint))
        .route("/v1/markets/*path", any(retired_capacity_endpoint))
        .route("/v1/market", any(retired_capacity_endpoint))
        .route("/v1/market/*path", any(retired_capacity_endpoint))
        .route("/v1/admin/markets", any(retired_capacity_endpoint))
        .route("/v1/admin/markets/*path", any(retired_capacity_endpoint))
        .route("/_market/proxy", any(retired_capacity_endpoint))
        .route("/_market/proxy/*path", any(retired_capacity_endpoint))
}

/// Explicitly retire the pre-Gateway capacity API.  Returning `410 Gone`
/// makes stale clients fail closed and gives operators a deterministic signal
/// to migrate to `/v1/gateway/*` or the Share/Client Market APIs.
async fn retired_capacity_endpoint() -> StatusCode {
    StatusCode::GONE
}

const INSTALL_CLIENT_RELEASE_LINE: &str =
    "SERVER_RELEASE=\"latest\" # __CC_SWITCH_SERVER_RELEASE__";

async fn install_client_script(State(state): State<ServerState>, headers: HeaderMap) -> Response {
    let release = state.dynamic.read().await.client_server_release.clone();
    let script = match render_install_client_script(&release) {
        Ok(script) => script,
        Err(error) => return error.into_response(),
    };
    let etag = install_client_script_etag(&script);
    let not_modified = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(',').any(|candidate| {
                let candidate = candidate
                    .trim()
                    .strip_prefix("W/")
                    .unwrap_or(candidate.trim());
                candidate == etag.as_str() || candidate == "*"
            })
        });

    let mut response = Response::builder()
        .status(if not_modified {
            StatusCode::NOT_MODIFIED
        } else {
            StatusCode::OK
        })
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::ETAG, etag);
    if !not_modified {
        response = response.header(header::CONTENT_TYPE, "text/plain; charset=utf-8");
    }
    response
        .body(if not_modified {
            Body::empty()
        } else {
            Body::from(script)
        })
        .unwrap_or_else(|error| AppError::Internal(error.to_string()).into_response())
}

fn render_install_client_script(release: &str) -> Result<String, AppError> {
    let release = crate::client_server_release::normalize_client_server_release(release)
        .map_err(AppError::Internal)?;
    let template = include_str!("../install-client.sh");
    if template.matches(INSTALL_CLIENT_RELEASE_LINE).count() != 1 {
        return Err(AppError::Internal(
            "install-client.sh must contain exactly one Server release template marker".into(),
        ));
    }
    Ok(template.replace(
        INSTALL_CLIENT_RELEASE_LINE,
        &format!("SERVER_RELEASE=\"{release}\" # __CC_SWITCH_SERVER_RELEASE__"),
    ))
}

fn install_client_script_etag(script: &str) -> String {
    format!("\"{}\"", hex::encode(Sha256::digest(script.as_bytes())))
}

async fn health(State(state): State<ServerState>) -> impl IntoResponse {
    let snapshot = state.store.database_health_snapshot();
    let (status, response) = database_health_response(snapshot);
    (status, Json(response))
}

fn database_health_response(
    snapshot: crate::db::DatabaseHealthSnapshot,
) -> (StatusCode, HealthResponse) {
    let status = if snapshot.available {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        HealthResponse {
            ok: snapshot.available,
            database: crate::models::DatabaseHealthResponse {
                mode: snapshot.mode.as_str().to_string(),
                available: snapshot.available,
                last_attempt_at_ms: snapshot.last_attempt_at_ms,
                last_success_at_ms: snapshot.last_success_at_ms,
                last_failure_at_ms: snapshot.last_failure_at_ms,
                consecutive_failures: snapshot.consecutive_failures,
                last_frames_synced: snapshot.last_frames_synced,
            },
        },
    )
}

async fn share_headroom_impl(
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

/// 429/rate-limit feedback from a capacity consumer. Because the same owner_email
/// typically backs all shares with shared upstream credentials, the penalty
/// is applied to *every* share of that owner, not just the offending one.
/// The override decays via TTL (default 30m).
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
) -> Result<Json<Vec<GatewayShareView>>, AppError> {
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
        if let Some(email) = share.scheduling_owner_email.as_deref()
            && let Some(penalty) = state.scheduling_overrides.get(email)
        {
            share.signals.owner_penalty = (share.signals.owner_penalty * penalty).clamp(0.05, 1.0);
        }
    }
    apply_share_route_availability(&state, &mut shares).await;
    Ok(Json(shares))
}

async fn gateway_shares_headroom(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ShareHeadroomResponse>, AppError> {
    let body_hash = sha256_hex(&body);
    let gateway = authenticate_gateway(
        &state,
        &headers,
        "gateway:shares:read",
        "gateway:shares:headroom",
        &body_hash,
    )
    .await?;
    let input: ShareHeadroomRequest = parse_signed_gateway_json(&body)?;
    require_gateway_share_access(&state, &gateway, input.share_ids.iter()).await?;
    share_headroom_impl(&state, input).await
}

async fn gateway_shares_feedback(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ShareFeedbackResponse>, AppError> {
    let body_hash = sha256_hex(&body);
    let gateway = authenticate_gateway(
        &state,
        &headers,
        "gateway:feedback:write",
        "gateway:shares:feedback",
        &body_hash,
    )
    .await?;
    let input: ShareFeedbackRequest = parse_signed_gateway_json(&body)?;
    require_gateway_share_access(&state, &gateway, std::iter::once(&input.share_id)).await?;
    apply_share_feedback(&state, input, "gateway").await
}

async fn require_gateway_share_access<'a>(
    state: &ServerState,
    gateway: &GatewayRegistryRecord,
    share_ids: impl IntoIterator<Item = &'a String>,
) -> Result<(), AppError> {
    let requested = share_ids.into_iter().collect::<HashSet<_>>();
    if requested.is_empty() {
        return Ok(());
    }
    let active_subdomains = state.proxy.active_subdomains().await.into_iter().collect();
    let inflight_by_share = state.proxy.inflight_by_share().await;
    let authorized = state
        .store
        .list_gateway_shares(gateway, "main", &active_subdomains, &inflight_by_share)
        .await?
        .into_iter()
        .map(|share| share.share_id)
        .collect::<HashSet<_>>();
    if requested
        .into_iter()
        .any(|share_id| !authorized.contains(share_id))
    {
        return Err(AppError::Forbidden(
            "Share is not authorized for this Gateway".into(),
        ));
    }
    Ok(())
}

async fn batch_sync_gateway_request_logs(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, AppError> {
    let body_hash = sha256_hex(&body);
    let gateway = authenticate_gateway(
        &state,
        &headers,
        "gateway:request_logs:write",
        "gateway:request_logs:batch",
        &body_hash,
    )
    .await?;
    let input: GatewayRequestObservationBatch = parse_signed_gateway_json(&body)?;
    require_gateway_share_access(
        &state,
        &gateway,
        input.logs.iter().filter_map(|log| log.share_id.as_ref()),
    )
    .await?;
    let metric_logs = input.logs.clone();
    let count = state
        .store
        .batch_sync_gateway_request_logs(&gateway, input)
        .await?;
    state
        .metrics
        .record_gateway_request_observations(&gateway.id, &metric_logs);
    Ok(Json(serde_json::json!({ "ok": true, "synced": count })))
}

async fn register_installation(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<RegisterInstallationRequest>,
) -> Result<Json<RegisterInstallationResponse>, AppError> {
    let metadata = extract_client_metadata(&headers, addr);
    state
        .registration_admission
        .check_attempt(metadata.ip.as_deref(), &input.public_key)
        .map_err(|rejection| AppError::RateLimited {
            message: "installation registration rate limit exceeded".into(),
            retry_after_secs: rejection.retry_after_secs,
        })?;
    let response = state
        .store
        .register_installation_with_admission(
            input,
            metadata,
            state.registration_admission.policy(),
        )
        .await?;
    Ok(Json(response))
}

async fn register_auth_device(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<RegisterAuthDeviceRequest>,
) -> Result<Json<RegisterAuthDeviceResponse>, AppError> {
    let metadata = extract_client_metadata(&headers, addr);
    state
        .registration_admission
        .check_attempt(metadata.ip.as_deref(), &input.public_key)
        .map_err(|rejection| AppError::RateLimited {
            message: "auth device registration rate limit exceeded".into(),
            retry_after_secs: rejection.retry_after_secs,
        })?;
    Ok(Json(
        state
            .store
            .register_auth_device_with_admission(
                input,
                metadata,
                state.registration_admission.policy(),
            )
            .await?,
    ))
}

async fn report_installation_status(
    State(state): State<ServerState>,
    Json(input): Json<ReportInstallationStatusRequest>,
) -> Result<Json<ReportInstallationStatusResponse>, AppError> {
    Ok(Json(state.store.report_installation_status(input).await?))
}

async fn report_installation_upgrade_task(
    State(state): State<ServerState>,
    Json(input): Json<InstallationUpgradeTaskReportRequest>,
) -> Result<Json<InstallationUpgradeTaskReportResponse>, AppError> {
    Ok(Json(
        state.store.report_installation_upgrade_task(input).await?,
    ))
}

async fn installation_heartbeat(
    State(state): State<ServerState>,
    Json(input): Json<InstallationHeartbeatRequest>,
) -> Result<Json<InstallationHeartbeatResponse>, AppError> {
    Ok(Json(
        state.store.record_installation_heartbeat(input).await?,
    ))
}

async fn installation_setup_completed(
    State(state): State<ServerState>,
    Json(input): Json<InstallationSetupCompletedRequest>,
) -> Result<Json<InstallationSetupCompletedResponse>, AppError> {
    Ok(Json(state.store.complete_installation_setup(input).await?))
}

async fn upgrade_installation(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(installation_id): Path<String>,
    Json(input): Json<UpgradeInstallationRequest>,
) -> Result<Json<UpgradeInstallationResponse>, AppError> {
    let session_email = require_session_email(&state, &headers).await?;
    let (tunnel_url, _) = state
        .store
        .prepare_installation_upgrade(&state.config, &installation_id, &session_email)
        .await?;
    let release = state.dynamic.read().await.client_server_release.clone();
    let validation = state
        .client_server_release_validator
        .validate(&release)
        .await
        .map_err(|error| {
            AppError::ServiceUnavailable(format!(
                "validate client server release before upgrade failed: {error}"
            ))
        })?;
    ensure_client_server_release_valid(validation.clone())?;
    let target_commit = validation
        .target_commitish
        .as_deref()
        .filter(|value| {
            (7..=40).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .or_else(|| {
            ((release.len() == 7) && release.bytes().all(|byte| byte.is_ascii_hexdigit()))
                .then_some(release.as_str())
        });
    if let Some(target_commit) = target_commit {
        state
            .store
            .ensure_installation_upgrade_target_allowed(target_commit)
            .await?;
    }
    // Speak directly to the live client-web tunnel backend with a signed ingress
    // context. Unauthenticated `x-cc-switch-web-*` headers are stripped by the
    // Client's `verify_router_ingress` middleware; going through the public URL
    // also fails for Market Clients because the edge proxy will not re-inject
    // Router session identity.
    let path_and_query = "/web-api/invoke/start_admin_upgrade";
    let body = serde_json::to_vec(&serde_json::json!({
        "restartAfter": input.restart_after,
        "force": true,
    }))
    .map_err(|error| {
        AppError::Internal(format!("serialize client upgrade request failed: {error}"))
    })?;
    let target = client_upgrade_target(
        &state,
        &tunnel_url,
        &installation_id,
        &session_email,
        "POST",
        path_and_query,
        &body,
    )
    .await?;
    let response = state
        .proxy_http
        .post(format!("{}{path_and_query}", target.backend_base))
        .header(
            crate::ingress_context::INGRESS_CONTEXT_HEADER,
            target.ingress.encoded_context,
        )
        .header(
            crate::ingress_context::INGRESS_SIGNATURE_HEADER,
            target.ingress.signature,
        )
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(body)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|error| AppError::Internal(format!("client upgrade request failed: {error}")))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::Internal(format!(
            "client upgrade failed: {status}: {body}"
        )));
    }
    let payload: serde_json::Value = response.json().await.map_err(|error| {
        AppError::Internal(format!("parse client upgrade response failed: {error}"))
    })?;
    let task_id = payload
        .get("taskId")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::Internal("client upgrade response missing taskId".into()))?
        .to_string();
    state
        .store
        .record_installation_upgrade_started(
            &installation_id,
            &task_id,
            &session_email,
            input.restart_after,
        )
        .await?;
    Ok(Json(UpgradeInstallationResponse { ok: true, task_id }))
}

async fn upgrade_installation_status(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(installation_id): Path<String>,
    Query(query): Query<InstallationUpgradeStatusQuery>,
) -> Result<Json<UpgradeInstallationStatusResponse>, AppError> {
    let session_email = require_session_email(&state, &headers).await?;
    let mut status = state
        .store
        .installation_upgrade_status_for_owner(
            &installation_id,
            query.task_id.as_deref(),
            &session_email,
        )
        .await?;
    if status.status == "running" && status.status_sync != "reported" {
        match reconcile_installation_upgrade_status_from_client(
            &state,
            &installation_id,
            &session_email,
            &status.task_id,
        )
        .await
        {
            Ok(()) => {
                status = state
                    .store
                    .installation_upgrade_status_for_owner(
                        &installation_id,
                        Some(&status.task_id),
                        &session_email,
                    )
                    .await?;
            }
            Err(error) => {
                tracing::debug!(
                    installation_id,
                    task_id = %status.task_id,
                    error = %error,
                    "client upgrade status reconciliation remains pending"
                );
                if status.status_sync != "lost" {
                    status.status_sync = "unavailable".into();
                }
            }
        }
    }
    Ok(Json(status))
}

async fn reconcile_installation_upgrade_status_from_client(
    state: &ServerState,
    installation_id: &str,
    session_email: &str,
    task_id: &str,
) -> Result<(), AppError> {
    let tunnel_url = state
        .store
        .installation_upgrade_tunnel_for_owner(&state.config, installation_id, session_email)
        .await?;
    let path_and_query = format!(
        "/web-api/admin/upgrade/status?taskId={}",
        url::form_urlencoded::byte_serialize(task_id.as_bytes()).collect::<String>()
    );
    let target = client_upgrade_target(
        state,
        &tunnel_url,
        installation_id,
        session_email,
        "GET",
        &path_and_query,
        &[],
    )
    .await?;
    let response = state
        .proxy_http
        .get(format!("{}{}", target.backend_base, path_and_query))
        .header(
            crate::ingress_context::INGRESS_CONTEXT_HEADER,
            target.ingress.encoded_context,
        )
        .header(
            crate::ingress_context::INGRESS_SIGNATURE_HEADER,
            target.ingress.signature,
        )
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|error| {
            AppError::ServiceUnavailable(format!("client upgrade status request failed: {error}"))
        })?;
    let status = response.status();
    let body = read_bounded_upgrade_status_body(response).await?;
    if !status.is_success() {
        let body = String::from_utf8_lossy(&body)
            .chars()
            .take(2_000)
            .collect::<String>();
        return Err(AppError::ServiceUnavailable(format!(
            "client upgrade status failed: {status}: {body}"
        )));
    }
    let payload =
        serde_json::from_slice::<InstallationUpgradeTaskReportPayload>(&body).map_err(|error| {
            AppError::ServiceUnavailable(format!("parse client upgrade status failed: {error}"))
        })?;
    state
        .store
        .reconcile_installation_upgrade_status_from_client(installation_id, task_id, payload)
        .await
}

async fn read_bounded_upgrade_status_body(
    response: reqwest::Response,
) -> Result<Vec<u8>, AppError> {
    if response
        .content_length()
        .is_some_and(|length| length > INSTALLATION_UPGRADE_STATUS_RESPONSE_MAX_BYTES as u64)
    {
        return Err(AppError::ServiceUnavailable(format!(
            "client upgrade status response exceeds {INSTALLATION_UPGRADE_STATUS_RESPONSE_MAX_BYTES} bytes"
        )));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            AppError::ServiceUnavailable(format!(
                "client upgrade status response read failed: {error}"
            ))
        })?;
        if body.len().saturating_add(chunk.len()) > INSTALLATION_UPGRADE_STATUS_RESPONSE_MAX_BYTES {
            return Err(AppError::ServiceUnavailable(format!(
                "client upgrade status response exceeds {INSTALLATION_UPGRADE_STATUS_RESPONSE_MAX_BYTES} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

struct ClientUpgradeTarget {
    backend_base: String,
    ingress: crate::ingress_context::SignedIngressContext,
}

async fn client_upgrade_target(
    state: &ServerState,
    tunnel_url: &str,
    installation_id: &str,
    session_email: &str,
    method: &str,
    path_and_query: &str,
    body: &[u8],
) -> Result<ClientUpgradeTarget, AppError> {
    let host = tunnel_url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let public_host = host
        .split(':')
        .next()
        .unwrap_or(host)
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let route = state
        .proxy
        .backend_for_host(host, &state.config.tunnel_domain)
        .await
        .ok_or_else(|| AppError::Conflict("client tunnel is offline".into()))?;
    if !route.is_client_web() {
        return Err(AppError::Conflict(
            "client tunnel route is not available for upgrade".into(),
        ));
    }
    if let Some(route_installation_id) = route.installation_id()
        && route_installation_id != installation_id
    {
        return Err(AppError::Conflict(
            "client tunnel route does not match installation".into(),
        ));
    }
    let control_secret = state
        .store
        .installation_control_secret(installation_id)
        .await?
        .filter(|secret| !secret.trim().is_empty())
        .ok_or_else(|| {
            AppError::ServiceUnavailable("installation control secret is missing".into())
        })?;
    let owner_email = session_email.trim().to_ascii_lowercase();
    let ingress = crate::ingress_context::sign(
        crate::ingress_context::IngressContext {
            signature_version: crate::ingress_context::SIGNATURE_VERSION,
            protocol_epoch: crate::namespace::PROTOCOL_EPOCH.to_string(),
            router_id: state
                .config
                .tunnel_domain
                .trim_end_matches('.')
                .to_ascii_lowercase(),
            route_id: format!("client:{installation_id}"),
            installation_id: installation_id.to_string(),
            target_lane_id: installation_id.to_string(),
            public_host,
            share_id: None,
            request_id: uuid::Uuid::new_v4().to_string(),
            user_email: Some(owner_email),
            user_role: Some("owner".into()),
            user_country: None,
            is_health_check: false,
            method: method.to_string(),
            path_and_query: path_and_query.to_string(),
            body_sha256: crate::ingress_context::body_sha256_hex(body),
            issued_at_ms: chrono::Utc::now().timestamp_millis(),
        },
        &control_secret,
    )
    .map_err(|error| AppError::Internal(format!("sign client upgrade ingress failed: {error}")))?;
    Ok(ClientUpgradeTarget {
        backend_base: format!("http://{}", route.route_target()),
        ingress,
    })
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubdomainAvailabilityQuery {
    subdomain: String,
    #[serde(default)]
    installation_id: Option<String>,
    #[serde(default)]
    owner_email: Option<String>,
}

async fn check_client_tunnel_subdomain_availability(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<SubdomainAvailabilityQuery>,
) -> Result<Json<SubdomainAvailabilityResponse>, AppError> {
    let metadata = extract_client_metadata(&headers, addr);
    let session = resolve_router_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .check_client_tunnel_subdomain_availability(
                &state.config,
                &query.subdomain,
                query.installation_id.as_deref(),
                metadata.ip.as_deref(),
                session.as_ref(),
                query.owner_email.as_deref(),
            )
            .await?,
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

async fn client_subdomain_takeover(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<crate::client_subdomain_takeover::ClientSubdomainTakeoverRequest>,
) -> Result<Json<crate::client_subdomain_takeover::ClientSubdomainTakeoverResponse>, AppError> {
    let owner_email = require_session_email(&state, &headers).await?;
    Ok(Json(
        crate::client_subdomain_takeover::execute(state, &owner_email, input).await?,
    ))
}

async fn client_subdomain_takeover_authorization(
    State(state): State<ServerState>,
    Json(input): Json<
        crate::client_subdomain_takeover::ClientSubdomainTakeoverAuthorizationRequest,
    >,
) -> Result<
    Json<crate::client_subdomain_takeover::ClientSubdomainTakeoverAuthorizationResponse>,
    AppError,
> {
    Ok(Json(
        crate::client_subdomain_takeover::authorization(state, input).await?,
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

async fn renew_lease(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<RenewLeaseRequest>,
) -> Result<Json<RenewLeaseResponse>, AppError> {
    let metadata = extract_client_metadata(&headers, addr);
    let installation_id = input.installation_id.clone();
    let lease_id = input.renewal.lease_id.clone();
    let connection_id = input.renewal.connection_id.clone();
    let response = match state
        .store
        .renew_lease(&state.config, &state.proxy, input, metadata)
        .await
    {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(
                installation_id = %installation_id,
                lease_id = %lease_id,
                connection_id = %connection_id,
                error = %error,
                "tunnel lease renewal rejected"
            );
            return Err(error);
        }
    };
    tracing::debug!(
        installation_id = %installation_id,
        lease_id = %lease_id,
        connection_id = %connection_id,
        expires_at = %response.expires_at,
        "tunnel lease renewed in place"
    );
    Ok(Json(response))
}

async fn activate_tunnel(
    State(state): State<ServerState>,
    Json(input): Json<TunnelActivateRequest>,
) -> Result<Json<TunnelStateResponse>, AppError> {
    let installation_id = input.installation_id.clone();
    let route_id = input.activation.route_id.clone();
    let rotation_id = input.activation.rotation_id.clone();
    let generation = input.activation.generation;
    let response = state
        .store
        .activate_tunnel(&state.config, &state.proxy, &state.proxy_http, input)
        .await
        .map_err(|error| {
            tracing::warn!(
                installation_id = %installation_id,
                route_id = %route_id,
                rotation_id = %rotation_id,
                generation,
                error = %error,
                "tunnel candidate activation rejected"
            );
            error
        })?;
    tracing::info!(
        installation_id = %installation_id,
        route_id = %route_id,
        rotation_id = %rotation_id,
        generation,
        active_generation = ?response.active_generation,
        "tunnel candidate promoted"
    );
    Ok(Json(response))
}

async fn tunnel_state(
    State(state): State<ServerState>,
    Json(input): Json<TunnelStateRequest>,
) -> Result<Json<TunnelStateResponse>, AppError> {
    Ok(Json(
        state
            .store
            .tunnel_state(&state.config, &state.proxy, input)
            .await?,
    ))
}

async fn dashboard(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<DashboardResponse>, AppError> {
    let viewer_email = extract_dashboard_session_email(&state, &headers).await?;
    let mut runtime_config = state.config.clone();
    let viewer_is_admin = {
        let dynamic = state.dynamic.read().await;
        runtime_config.client_notifications = dynamic.client_notifications.clone();
        viewer_email
            .as_deref()
            .is_some_and(|email| dynamic.is_admin(email))
    };
    let mut response = state
        .store
        .dashboard_snapshot(
            &runtime_config,
            &state.server_geo,
            &state.proxy,
            viewer_email.as_deref(),
        )
        .await?;
    let snapshot = state.recent_traffic.snapshot().await;
    enrich_share_ticker_logs_with_live_country(&mut response.ticker_shares, &snapshot);
    let global_ticker_logs = state
        .store
        .list_dashboard_ticker_request_logs(DASHBOARD_REQUEST_TICKER_LIMIT)
        .await?;
    let (confirmed_events, confirmed_country_counts) =
        confirmed_request_events(&snapshot, &response, &global_ticker_logs);
    response.user_country_counts = confirmed_country_counts;
    response.recent_request_events = confirmed_events;
    apply_dashboard_request_log_visibility(&mut response, viewer_is_admin);
    Ok(Json(response))
}

async fn map_display_get(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<MapDisplaySettings>, AppError> {
    let _ = extract_session_email(&state, &headers).await?;
    Ok(Json(state.store.map_display_settings().await?))
}

async fn announcement_get(
    State(state): State<ServerState>,
) -> Result<Json<AnnouncementResponse>, AppError> {
    Ok(Json(state.store.announcement_response().await?))
}

async fn admin_map_display_update(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<MapDisplaySettingsUpdate>,
) -> Result<Json<MapDisplaySettings>, AppError> {
    let session = require_admin_session(&state, &headers).await?;
    let updated = state.store.update_map_display_settings(input).await?;
    let metadata = extract_client_metadata(&headers, addr);
    let payload = serde_json::to_value(&updated).unwrap_or_else(|_| serde_json::json!({}));
    let _ = state
        .store
        .record_admin_audit(
            Some(&session.email),
            "map_display.update",
            Some(&payload),
            metadata.ip.as_deref(),
        )
        .await;
    Ok(Json(updated))
}

async fn admin_announcement_update(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<AnnouncementSettingsUpdate>,
) -> Result<Json<AnnouncementSettings>, AppError> {
    let session = require_admin_session(&state, &headers).await?;
    let updated = state.store.update_announcement_settings(input).await?;
    let metadata = extract_client_metadata(&headers, addr);
    let payload = serde_json::to_value(&updated).unwrap_or_else(|_| serde_json::json!({}));
    let _ = state
        .store
        .record_admin_audit(
            Some(&session.email),
            "announcement.update",
            Some(&payload),
            metadata.ip.as_deref(),
        )
        .await;
    Ok(Json(updated))
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
    let response = update_share_settings_with_email(
        &state,
        &share_id,
        &email,
        input.patch,
        input.base_config_revision,
    )
    .await?;
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
    global_share_logs: &[ShareRequestLogEntry],
) -> (Vec<RecentRequestEvent>, HashMap<String, usize>) {
    let mut events_by_id = HashMap::new();
    for event in persisted_ticker_request_events(response, global_share_logs) {
        if let Some(existing) = events_by_id.get_mut(&event.request_id) {
            merge_persisted_ticker_event(existing, event);
        } else {
            events_by_id.insert(event.request_id.clone(), event);
        }
    }
    for event in &snapshot.recent_events {
        match events_by_id.get_mut(&event.request_id) {
            Some(existing) => {
                merge_ticker_event_country(existing, event);
                existing.is_inflight = event.is_inflight;
                if event.share_subdomain.is_some() {
                    existing.share_subdomain = event.share_subdomain.clone();
                }
                if event.share_name.is_some() {
                    existing.share_name = event.share_name.clone();
                }
                if event.status_code.is_some() {
                    existing.status_code = event.status_code;
                }
                if event.latency_ms.is_some() {
                    existing.latency_ms = event.latency_ms;
                }
            }
            None => {
                events_by_id.insert(event.request_id.clone(), event.clone());
            }
        }
    }
    for event in snapshot.events.iter() {
        if let Some(existing) = events_by_id.get_mut(&event.request_id) {
            merge_ticker_event_country(existing, event);
        }
    }
    let mut events = events_by_id
        .into_values()
        .filter(|event| !event.is_health_check)
        .collect::<Vec<_>>();
    events.sort_by(|left, right| left.started_at.cmp(&right.started_at));
    if events.len() > DASHBOARD_REQUEST_TICKER_LIMIT {
        events.drain(0..events.len() - DASHBOARD_REQUEST_TICKER_LIMIT);
    }
    (events, snapshot.country_counts.clone())
}

fn apply_dashboard_request_log_visibility(response: &mut DashboardResponse, viewer_is_admin: bool) {
    let share_log_access = response
        .shares
        .iter()
        .map(|share| {
            let can_view_all = viewer_is_admin || share.can_manage;
            (share.share_id.clone(), can_view_all)
        })
        .collect::<HashMap<_, _>>();
    for share in &mut response.shares {
        let can_view_all = share_log_access
            .get(&share.share_id)
            .copied()
            .unwrap_or(viewer_is_admin);
        if !can_view_all {
            remove_share_request_session_ids(&mut share.recent_requests);
        }
    }

    for share in &mut response.ticker_shares {
        let can_view_all = share_log_access
            .get(&share.share_id)
            .copied()
            .unwrap_or(viewer_is_admin);
        if !can_view_all {
            remove_share_request_session_ids(&mut share.recent_requests);
        }
    }
}

fn remove_share_request_session_ids(logs: &mut [ShareRequestLogEntry]) {
    for log in logs {
        log.session_id = None;
    }
}

fn merge_persisted_ticker_event(target: &mut RecentRequestEvent, mut source: RecentRequestEvent) {
    if source.share_id.trim().is_empty() {
        source.share_id = target.share_id.clone();
    }
    if option_string_is_blank(&source.share_name) {
        source.share_name = target.share_name.clone();
    }
    if option_string_is_blank(&source.share_subdomain) {
        source.share_subdomain = target.share_subdomain.clone();
    }
    if option_string_is_blank(&source.user_country) {
        source.user_country = target.user_country.clone();
    }
    if option_string_is_blank(&source.user_country_iso3) {
        source.user_country_iso3 = target.user_country_iso3.clone();
    }
    if option_string_is_blank(&source.user_email) {
        source.user_email = target.user_email.clone();
    }
    if !should_replace_ticker_usage(target, &source) {
        copy_ticker_usage(&mut source, target);
    }
    if source.request_agent.is_none() {
        source.request_agent = target.request_agent.clone();
    }
    if source.requested_model.is_none() {
        source.requested_model = target.requested_model.clone();
    }
    if source.actual_model.is_none() {
        source.actual_model = target.actual_model.clone();
    }
    if source.model.is_none() {
        source.model = target.model.clone();
    }
    if source.latency_ms.unwrap_or(0) == 0 {
        source.latency_ms = target.latency_ms;
    }
    if source.status_code.unwrap_or(0) == 0 {
        source.status_code = target.status_code;
    }
    if target.started_at < source.started_at {
        source.started_at = target.started_at;
    }
    *target = source;
}

fn option_string_is_blank(value: &Option<String>) -> bool {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
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

fn enrich_share_ticker_logs_with_live_country(
    ticker_shares: &mut [crate::models::DashboardTickerShare],
    snapshot: &RecentTrafficSnapshot,
) {
    let context_by_request_id = live_request_context_by_request_id(snapshot);
    for share in ticker_shares {
        for log in &mut share.recent_requests {
            if let Some((user_country, user_country_iso3, user_email)) =
                context_by_request_id.get(&log.request_id)
            {
                if log.user_country.is_none() {
                    log.user_country = user_country.clone();
                }
                if log.user_country_iso3.is_none() {
                    log.user_country_iso3 = user_country_iso3.clone();
                }
                if let Some(user_email) = user_email.as_ref() {
                    log.user_email = Some(user_email.clone());
                }
            }
        }
    }
}

fn merge_ticker_event_country(target: &mut RecentRequestEvent, source: &RecentRequestEvent) {
    if let Some(user_country) = source.user_country.as_ref() {
        target.user_country = Some(user_country.clone());
    }
    if let Some(user_country_iso3) = source.user_country_iso3.as_ref() {
        target.user_country_iso3 = Some(user_country_iso3.clone());
    }
    if let Some(user_email) = source.user_email.as_ref() {
        target.user_email = Some(user_email.clone());
    }
    merge_ticker_event_usage(target, source);
}

fn merge_ticker_event_usage(target: &mut RecentRequestEvent, source: &RecentRequestEvent) {
    if should_replace_ticker_usage(target, source) {
        copy_ticker_usage(target, source);
    }
    if target.request_agent.is_none() {
        target.request_agent = source.request_agent.clone();
    }
    if target.requested_model.is_none() {
        target.requested_model = source.requested_model.clone();
    }
    if target.actual_model.is_none() {
        target.actual_model = source.actual_model.clone();
    }
    if target.model.is_none() {
        target.model = source.model.clone();
    }
    if target.latency_ms.unwrap_or(0) == 0 {
        target.latency_ms = source.latency_ms;
    }
    if target.status_code.unwrap_or(0) == 0 {
        target.status_code = source.status_code;
    }
}

fn should_replace_ticker_usage(target: &RecentRequestEvent, source: &RecentRequestEvent) -> bool {
    if !has_ticker_usage(source) {
        return false;
    }
    if !has_ticker_usage(target) {
        return true;
    }
    match source
        .usage_revision
        .unwrap_or(0)
        .cmp(&target.usage_revision.unwrap_or(0))
    {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => {
            let source_rank = ticker_usage_state_rank(source.usage_state.as_deref());
            let target_rank = ticker_usage_state_rank(target.usage_state.as_deref());
            let source_has_richer_usage =
                match (ticker_usage_total(source), ticker_usage_total(target)) {
                    (Some(source_total), Some(target_total)) => source_total > target_total,
                    (Some(_), None) => true,
                    _ => false,
                };
            source_rank > target_rank || (source_rank == target_rank && source_has_richer_usage)
        }
    }
}

fn has_ticker_usage(event: &RecentRequestEvent) -> bool {
    event.usage_state.is_some()
        || event.usage_revision.is_some()
        || event.input_tokens.is_some()
        || event.output_tokens.is_some()
        || event.cache_read_tokens.is_some()
        || event.cache_creation_tokens.is_some()
        || event.total_tokens.is_some()
}

fn ticker_usage_state_rank(state: Option<&str>) -> u8 {
    match state {
        Some("observed") => 3,
        Some("missing" | "parse_error" | "interrupted") => 2,
        Some(_) => 1,
        None => 0,
    }
}

fn ticker_usage_total(event: &RecentRequestEvent) -> Option<u64> {
    event.total_tokens.or_else(|| {
        (event.input_tokens.is_some()
            || event.output_tokens.is_some()
            || event.cache_read_tokens.is_some()
            || event.cache_creation_tokens.is_some())
        .then(|| {
            u64::from(event.input_tokens.unwrap_or(0))
                + u64::from(event.output_tokens.unwrap_or(0))
                + u64::from(event.cache_read_tokens.unwrap_or(0))
                + u64::from(event.cache_creation_tokens.unwrap_or(0))
        })
    })
}

fn copy_ticker_usage(target: &mut RecentRequestEvent, source: &RecentRequestEvent) {
    target.input_tokens = source.input_tokens;
    target.output_tokens = source.output_tokens;
    target.cache_read_tokens = source.cache_read_tokens;
    target.cache_creation_tokens = source.cache_creation_tokens;
    target.total_tokens = source.total_tokens;
    target.usage_state = source.usage_state.clone();
    target.stream_status = source.stream_status.clone();
    target.usage_revision = source.usage_revision;
}

fn persisted_ticker_request_events(
    response: &DashboardResponse,
    global_share_logs: &[ShareRequestLogEntry],
) -> Vec<RecentRequestEvent> {
    let share_lookup = response
        .ticker_shares
        .iter()
        .map(|share| (share.share_id.as_str(), share))
        .collect::<HashMap<_, _>>();
    let mut events = Vec::new();
    for log in global_share_logs {
        let fallback = DashboardTickerShare {
            share_id: log.share_id.clone(),
            share_name: log.share_name.clone(),
            subdomain: String::new(),
            recent_requests: Vec::new(),
        };
        let share = share_lookup
            .get(log.share_id.as_str())
            .copied()
            .unwrap_or(&fallback);
        events.push(share_log_to_ticker_event(share, log));
    }
    events
}

fn share_log_to_ticker_event(
    share: &DashboardTickerShare,
    log: &ShareRequestLogEntry,
) -> RecentRequestEvent {
    let usage_observed = log.usage_state == "observed";
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
        input_tokens: usage_observed.then_some(log.input_tokens),
        output_tokens: usage_observed.then_some(log.output_tokens),
        cache_read_tokens: usage_observed.then_some(log.cache_read_tokens),
        cache_creation_tokens: usage_observed.then_some(log.cache_creation_tokens),
        total_tokens: usage_observed.then(|| share_log_total_tokens(log)),
        usage_state: Some(log.usage_state.clone()),
        stream_status: log.stream_status.clone(),
        usage_revision: Some(log.usage_revision),
        request_agent: (!log.request_agent.is_empty()).then(|| log.request_agent.clone()),
        requested_model: (!log.requested_model.is_empty()).then(|| log.requested_model.clone()),
        actual_model: (!log.actual_model.is_empty()).then(|| log.actual_model.clone()),
        model: (!log.model.is_empty()).then(|| log.model.clone()),
        latency_ms: (log.latency_ms > 0).then_some(log.latency_ms),
        status_code: (log.status_code > 0).then_some(log.status_code),
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

fn share_log_total_tokens(log: &ShareRequestLogEntry) -> u64 {
    u64::from(log.input_tokens)
        + u64::from(log.output_tokens)
        + u64::from(log.cache_read_tokens)
        + u64::from(log.cache_creation_tokens)
}

async fn public_map_points(
    State(state): State<ServerState>,
) -> Result<Json<PublicMapPointsResponse>, AppError> {
    Ok(Json(
        state.store.public_map_points(&state.server_geo).await?,
    ))
}

async fn public_network_stats(
    State(state): State<ServerState>,
) -> Result<Json<PublicNetworkStatsResponse>, AppError> {
    Ok(Json(state.store.public_network_stats().await?))
}

const GLOBAL_USAGE_CARD_CACHE_CONTROL: &str =
    "public, max-age=0, s-maxage=60, stale-while-revalidate=300";
const USER_USAGE_CARD_CACHE_CONTROL: &str = "no-store";
/// Deliberately not the SVG card's header. The edge rewrites the card's
/// `max-age` up to hours, which is fine for a badge and wrong for a figure the
/// site refreshes every minute.
const PUBLIC_USAGE_JSON_CACHE_CONTROL: &str =
    "public, max-age=30, s-maxage=60, stale-while-revalidate=300";

fn svg_usage_response(svg: String, cache_control: &'static str) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/svg+xml; charset=utf-8")
        .header(header::CACHE_CONTROL, cache_control)
        .body(Body::from(svg))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn embed_render_options(
    query: &EmbedUsageQuery,
    period_fallback: &str,
) -> crate::embed_usage::EmbedRenderOptions {
    crate::embed_usage::EmbedRenderOptions::from_query(
        query.period.as_deref().or(Some(period_fallback)),
        query.theme.as_deref(),
        query.models,
        query.show_breakdown.as_deref(),
        query.show_models.as_deref(),
        query.compact.as_deref(),
        query.format.as_deref(),
    )
}

/// JSON twin of `/v1/public/embed/global.svg`. Same `usage_global` data and the
/// same public exposure the card already carries, in a shape another service can
/// sum across regions. Aggregate only: no email, no share, no account, no money.
async fn public_usage_global(
    State(state): State<ServerState>,
    Query(query): Query<PublicUsageQuery>,
) -> Result<Response, AppError> {
    let period = query.period.as_deref().unwrap_or("24h");
    let mut data = state.store.usage_global(period).await?;
    if let Some(limit) = query.models.filter(|limit| *limit > 0) {
        data.models.truncate(limit);
    }
    Ok((
        [(header::CACHE_CONTROL, PUBLIC_USAGE_JSON_CACHE_CONTROL)],
        Json(data),
    )
        .into_response())
}

async fn embed_global_usage_svg(
    State(state): State<ServerState>,
    Query(query): Query<EmbedUsageQuery>,
) -> Response {
    let period = query.period.as_deref().unwrap_or("24h");
    let opts = embed_render_options(&query, period);
    match state.store.usage_global(period).await {
        Ok(mut data) => {
            data.models.truncate(opts.models);
            let mut opts = opts;
            opts.period = data.period.clone();
            svg_usage_response(
                crate::embed_usage::render_global_usage_svg(
                    &data,
                    &opts,
                    &state.config.tunnel_domain,
                ),
                GLOBAL_USAGE_CARD_CACHE_CONTROL,
            )
        }
        Err(err) => svg_usage_response(
            crate::embed_usage::render_usage_error_svg(&err.to_string(), &opts),
            GLOBAL_USAGE_CARD_CACHE_CONTROL,
        ),
    }
}

async fn embed_user_usage_svg(
    State(state): State<ServerState>,
    Path(user_id): Path<String>,
    Query(query): Query<EmbedUsageQuery>,
) -> Response {
    let period = query.period.as_deref().unwrap_or("24h");
    let opts = embed_render_options(&query, period);
    let user_id = user_id
        .strip_suffix(".svg")
        .unwrap_or(user_id.as_str())
        .trim()
        .to_string();
    match state
        .store
        .usage_consumer_by_user_id(&user_id, period)
        .await
    {
        Ok(Some((email, mut data))) => {
            data.models.truncate(opts.models);
            let mut opts = opts;
            opts.period = data.period.clone();
            svg_usage_response(
                crate::embed_usage::render_user_usage_svg(&email, &data, &opts),
                USER_USAGE_CARD_CACHE_CONTROL,
            )
        }
        Ok(None) => svg_usage_response(
            crate::embed_usage::render_usage_error_svg("usage not found or not public", &opts),
            USER_USAGE_CARD_CACHE_CONTROL,
        ),
        Err(err) => svg_usage_response(
            crate::embed_usage::render_usage_error_svg(&err.to_string(), &opts),
            USER_USAGE_CARD_CACHE_CONTROL,
        ),
    }
}

async fn get_my_usage_card_settings(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<UsageCardSettingsResponse>, AppError> {
    let email = require_session_email(&state, &headers).await?;
    Ok(Json(state.store.get_usage_card_settings(&email).await?))
}

async fn update_my_usage_card_settings(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(patch): Json<UpdateUsageCardSettingsRequest>,
) -> Result<Json<UsageCardSettingsResponse>, AppError> {
    let email = require_session_email(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .update_usage_card_settings(&email, patch)
            .await?,
    ))
}

/// Read the caller's notification channel preference plus the Router-level
/// Telegram bot availability, so the account page can render the whole section
/// from one request.
async fn get_my_notification_settings(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<NotificationSettingsResponse>, AppError> {
    let email = require_session_email(&state, &headers).await?;
    let mut response = state.store.get_notification_settings(&email).await?;
    let settings = state.dynamic.read().await.telegram_bot.clone();
    align_notification_runtime_response(&mut response, &settings);
    Ok(Json(response))
}

async fn update_my_notification_settings(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(patch): Json<UpdateNotificationSettingsRequest>,
) -> Result<Json<NotificationSettingsResponse>, AppError> {
    let email = require_session_email(&state, &headers).await?;
    let mut response = state
        .store
        .update_notification_settings(&email, patch)
        .await?;
    let settings = state.dynamic.read().await.telegram_bot.clone();
    align_notification_runtime_response(&mut response, &settings);
    Ok(Json(response))
}

/// The notification settings row is read from SQLite while the active Bot
/// configuration is read from the hot-reloaded settings snapshot. During a
/// token/mode change those two observations can briefly straddle the
/// reconcile boundary. Hide the old identity and diagnostics until the
/// runtime row carries the new fingerprint instead of showing a stale outage
/// as if it belonged to the newly selected Bot.
fn align_notification_runtime_response(
    response: &mut NotificationSettingsResponse,
    settings: &TelegramBotSettings,
) {
    let expected_fingerprint = settings.token().map(|token| {
        crate::telegram::bind::telegram_config_fingerprint(
            token,
            settings.mode.as_str(),
            settings.webhook_secret.as_deref(),
        )
    });
    response.telegram_bot_configured = settings.is_operational();
    if !settings.is_operational() {
        response.telegram_bot_status = "disabled".into();
        response.telegram_bot_transport_status = "unknown".into();
        response.telegram_bot_username = None;
        response.telegram_bot_failure_code = None;
        response.telegram_bot_failure_hint = None;
        response.telegram_bot_failure_details = None;
        response.telegram_bot_last_failure_at = None;
        for channel in &mut response.channels {
            if channel.channel == crate::notification_channels::TELEGRAM_CHANNEL {
                channel.available = false;
            }
        }
        return;
    }
    if response.telegram_bot_runtime_fingerprint.as_deref() == expected_fingerprint.as_deref() {
        // `mark_telegram_bot_reconciling` fences the new fingerprint before
        // the remote getMe call completes, while the old identity is retained
        // internally so a successful CAS can invalidate stale bindings. Do
        // not expose that retained username until the new identity is ready.
        if response.telegram_bot_status != "ready" {
            response.telegram_bot_username = None;
            for channel in &mut response.channels {
                if channel.channel == crate::notification_channels::TELEGRAM_CHANNEL {
                    channel.available = false;
                }
            }
        }
        return;
    }

    response.telegram_bot_status = if settings.is_operational() {
        "reconciling"
    } else {
        "disabled"
    }
    .into();
    response.telegram_bot_transport_status = "unknown".into();
    response.telegram_bot_username = None;
    response.telegram_bot_failure_code = None;
    response.telegram_bot_failure_hint = None;
    response.telegram_bot_failure_details = None;
    response.telegram_bot_last_failure_at = None;
    for channel in &mut response.channels {
        if channel.channel == crate::notification_channels::TELEGRAM_CHANNEL {
            channel.available = false;
        }
    }
}

/// Mint a single-use `t.me` deep link. The token is a bearer credential for
/// this account's notification channel, so it is minted only for a live
/// session and never returned to anyone else.
async fn create_my_telegram_bind_link(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<Json<TelegramBindLinkResponse>, AppError> {
    let email = require_session_email(&state, &headers).await?;
    let settings = state.dynamic.read().await.telegram_bot.clone();
    if !settings.is_operational() {
        return Err(AppError::ServiceUnavailable(
            "the Telegram notification bot is disabled or not configured".into(),
        ));
    }
    let metadata = extract_client_metadata(&headers, addr);
    Ok(Json(
        state
            .store
            .create_telegram_bind_link(&email, settings.bind_token_ttl_secs, metadata.ip.as_deref())
            .await?,
    ))
}

async fn unbind_my_telegram_chat(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<NotificationSettingsResponse>, AppError> {
    let email = require_session_email(&state, &headers).await?;
    let mut response = state.store.unbind_telegram(&email).await?;
    let settings = state.dynamic.read().await.telegram_bot.clone();
    align_notification_runtime_response(&mut response, &settings);
    Ok(Json(response))
}

/// Telegram's webhook callback. Authenticated solely by the secret header from
/// `setWebhook`; see `crate::telegram::service` for why this route is also
/// exempt from the IP blacklist.
async fn telegram_webhook(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(update): Json<serde_json::Value>,
) -> Result<StatusCode, AppError> {
    let secret = headers
        .get(crate::telegram::service::WEBHOOK_SECRET_HEADER)
        .and_then(|value| value.to_str().ok());
    crate::telegram::service::handle_webhook_update(
        &state.store,
        &state.dynamic,
        &state.config,
        secret,
        update,
    )
    .await?;
    Ok(StatusCode::OK)
}

async fn my_usage_consumer(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<AccountUsageQuery>,
) -> Result<Json<AccountUsageResponse>, AppError> {
    let email = require_session_email(&state, &headers).await?;
    let period = query.period.as_deref().unwrap_or("7d");
    Ok(Json(state.store.usage_consumer(&email, period).await?))
}

async fn my_usage_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<AccountUsageQuery>,
) -> Result<Json<ProviderUsageResponse>, AppError> {
    let email = require_session_email(&state, &headers).await?;
    let period = query.period.as_deref().unwrap_or("7d");
    Ok(Json(state.store.usage_provider(&email, period).await?))
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
    let telegram_url = state.dynamic.read().await.footer_telegram_link();
    Ok(Json(DashboardPresenceResponse {
        online_count,
        email_sent_24h,
        telegram_url,
    }))
}

async fn dashboard_ux_event(
    State(state): State<ServerState>,
    Json(input): Json<DashboardUxEventRequest>,
) -> Result<Json<DashboardUxEventResponse>, AppError> {
    if !state.config.ux_telemetry_enabled {
        return Ok(Json(DashboardUxEventResponse { accepted: false }));
    }
    state
        .store
        .record_dashboard_ux_event(input, state.config.ux_telemetry_retention_days)
        .await?;
    Ok(Json(DashboardUxEventResponse { accepted: true }))
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

async fn request_client_web_email_code(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ClientWebRequestEmailCodeRequest>,
) -> Result<Json<RequestEmailCodeResponse>, AppError> {
    Ok(Json(
        state
            .store
            .request_client_web_email_code(
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
    Json(input): Json<ClientWebVerifyEmailCodeRequest>,
) -> Result<Json<VerifyEmailCodeResponse>, AppError> {
    Ok(Json(
        state
            .store
            .verify_client_web_email_code(&state.config, input)
            .await?,
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
) -> Result<Json<SessionStatusResponse>, AppError> {
    if dev_auth_bypass_enabled() && extract_session_token(&headers).is_none() {
        return Ok(Json(dev_session_status()));
    }
    let session_token = resolve_session_auth_token(&state, &headers).await?;
    let mut response = state.store.session_status(session_token.as_deref()).await?;
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

async fn get_my_model_routing(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<UserModelRoutingResponse>, AppError> {
    let email = require_session_email(&state, &headers).await?;
    let active_subdomains = state.proxy.active_subdomains().await.into_iter().collect();
    Ok(Json(
        state
            .store
            .get_user_model_routing(&state.config, &email, &active_subdomains)
            .await?,
    ))
}

async fn replace_my_model_routing(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ReplaceUserModelRoutingRequest>,
) -> Result<Json<UserModelRoutingResponse>, AppError> {
    let email = require_session_email(&state, &headers).await?;
    let active_subdomains = state.proxy.active_subdomains().await.into_iter().collect();
    Ok(Json(
        state
            .store
            .replace_user_model_routing(
                &state.config,
                &email,
                input.expected_revision,
                input.routes,
                &active_subdomains,
            )
            .await?,
    ))
}

async fn test_my_model_routing(
    State(state): State<ServerState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<UserModelRoutingTestRequest>,
) -> Result<Json<UserModelRoutingTestResponse>, AppError> {
    let email = require_session_email(&state, &headers).await?;
    let user = state.store.ensure_user_by_email(&email).await?;
    let probe = build_user_model_route_test_probe(&input.app_type, &input.requested_model)?;
    let curl = unified_model_test_curl(&state.config.tunnel_url("api"), &probe);
    let started = std::time::Instant::now();
    match execute_user_model_route_test(state, peer, &user.id, &email, &probe).await {
        Ok(outcome) => Ok(Json(UserModelRoutingTestResponse {
            success: outcome.success,
            app_type: probe.app_type,
            requested_model: probe.requested_model,
            curl,
            target_share_id: Some(outcome.target_share_id),
            matched_wildcard: outcome.matched_wildcard,
            response: Some(UserModelRoutingTestHttp {
                status_code: outcome.status_code,
                status_text: outcome.status_text,
                headers: outcome.headers,
                body_text: outcome.body_text,
                body_truncated: outcome.body_truncated,
            }),
            duration_ms: outcome.duration_ms,
            error: outcome.error,
            code: None,
        })),
        Err(AppError::Coded {
            status: _,
            code,
            message,
            details: _,
        }) => Ok(Json(UserModelRoutingTestResponse {
            success: false,
            app_type: probe.app_type,
            requested_model: probe.requested_model,
            curl,
            target_share_id: None,
            matched_wildcard: false,
            response: None,
            duration_ms: started.elapsed().as_millis() as u64,
            error: Some(message),
            code: Some(code.to_string()),
        })),
        Err(error) => Err(error),
    }
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

fn is_router_share_ui_path(path: &str) -> bool {
    path == "/"
        || path == "/favicon.ico"
        || path == "/install-client.sh"
        || path == "/router-logo.svg"
        || path == "/world-map.svg"
        || path.starts_with("/flags/")
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn regions_endpoint_preserves_file_order_with_free_first() {
        let Json(regions) = regions().await.expect("embedded regions should be valid");

        let first = regions.first().expect("at least one region");
        assert_eq!(first.name, "free");
        assert_eq!(first.url, "freetokenswitch.cc");
        assert_eq!(
            regions
                .iter()
                .map(|region| region.name.as_str())
                .collect::<Vec<_>>(),
            ["free", "japan", "singapore", "hongkong", "usa"]
        );
    }

    #[test]
    fn install_client_script_renders_release_and_changes_entity_tag() {
        let latest = render_install_client_script("latest").unwrap();
        let pinned = render_install_client_script("AbC1234").unwrap();
        assert!(latest.contains("SERVER_RELEASE=\"latest\""));
        assert!(pinned.contains("SERVER_RELEASE=\"abc1234\""));
        assert!(!pinned.contains("SERVER_RELEASE=\"latest\""));
        assert_ne!(
            install_client_script_etag(&latest),
            install_client_script_etag(&pinned)
        );
        assert!(render_install_client_script("abc123").is_err());
    }

    #[test]
    fn database_health_response_reports_remote_outage_without_error_details() {
        let (status, response) = database_health_response(crate::db::DatabaseHealthSnapshot {
            mode: crate::db::ConnectionMode::TursoReplica,
            available: false,
            last_attempt_at_ms: Some(100),
            last_success_at_ms: Some(50),
            last_failure_at_ms: Some(100),
            consecutive_failures: 2,
            last_frames_synced: 7,
            last_error: Some("database unavailable: secret remote detail".into()),
        });

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(!response.ok);
        assert_eq!(response.database.mode, "turso");
        let payload = serde_json::to_value(response).expect("serialize health response");
        assert_eq!(payload["database"]["consecutiveFailures"], 2);
        assert!(payload["database"].get("lastError").is_none());
        assert!(!payload.to_string().contains("secret remote detail"));
    }

    #[test]
    fn personal_usage_cards_are_never_cached() {
        let response = svg_usage_response("<svg/>".to_string(), USER_USAGE_CARD_CACHE_CONTROL);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
    }

    #[test]
    fn gateway_signature_body_hash_binds_exact_wire_bytes() {
        let compact = br#"{"shareIds":["share-1"]}"#;
        let spaced = br#"{ "shareIds": ["share-1"] }"#;

        assert_ne!(sha256_hex(compact), sha256_hex(spaced));
    }

    #[test]
    fn gateway_observation_json_rejects_legacy_downstream_identity() {
        let minimal = br#"{
            "logs": [{
                "requestId": "req_gateway_minimal",
                "requestAgent": "codex",
                "requestedModel": "gpt-5",
                "actualModel": "gpt-5",
                "actualModelSource": "official",
                "status": "success",
                "createdAt": "2026-08-18T00:00:00Z"
            }]
        }"#;
        assert!(parse_signed_gateway_json::<GatewayRequestObservationBatch>(minimal).is_ok());

        let legacy = br#"{
            "logs": [{
                "requestId": "req_gateway_legacy_identity",
                "userEmail": "downstream@example.com",
                "requestAgent": "codex",
                "requestedModel": "gpt-5",
                "actualModel": "gpt-5",
                "actualModelSource": "official",
                "status": "success",
                "createdAt": "2026-08-18T00:00:00Z"
            }]
        }"#;
        assert!(parse_signed_gateway_json::<GatewayRequestObservationBatch>(legacy).is_err());
    }

    #[test]
    fn share_edit_event_stream_requests_initial_resync() {
        assert_eq!(
            initial_share_edit_stream_events(),
            [("ready", "{}"), ("resync", "{}")]
        );
    }

    async fn counted_json_handler(
        State(calls): State<Arc<AtomicUsize>>,
        Json(_value): Json<serde_json::Value>,
    ) -> StatusCode {
        calls.fetch_add(1, Ordering::SeqCst);
        StatusCode::OK
    }

    #[tokio::test]
    async fn registration_body_limit_rejects_before_handler() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/v1/installations/register",
                installation_control_body_limited(post(counted_json_handler)),
            )
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let oversized = format!(
            "{{\"platform\":\"{}\"}}",
            "x".repeat(INSTALLATION_CONTROL_BODY_LIMIT_BYTES)
        );

        let response = reqwest::Client::new()
            .post(format!("http://{address}/v1/installations/register"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(oversized)
            .send()
            .await
            .unwrap();
        server.abort();

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn retired_token_market_routes_construct_and_fail_closed() {
        let app: Router = retired_token_market_routes();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = reqwest::Client::new();

        for (method, path) in [
            (reqwest::Method::GET, "/v1/markets"),
            (reqwest::Method::POST, "/v1/markets/register"),
            (reqwest::Method::GET, "/v1/market/shares"),
            (
                reqwest::Method::PATCH,
                "/v1/admin/markets/legacy@example.com/maintenance",
            ),
            (reqwest::Method::POST, "/_market/proxy/share-1/v1/messages"),
        ] {
            let response = client
                .request(method, format!("http://{address}{path}"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::GONE, "legacy path {path}");
        }

        let response = client
            .get(format!("http://{address}/v1/gateway/shares"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        server.abort();
    }

    #[tokio::test]
    async fn upgrade_task_body_limit_accepts_diagnostics_and_rejects_oversize() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/v1/installations/upgrade-task-report",
                installation_upgrade_task_body_limited(post(counted_json_handler)),
            )
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = reqwest::Client::new();
        let bounded = format!(
            "{{\"logs\":\"{}\"}}",
            "x".repeat(INSTALLATION_UPGRADE_TASK_PAYLOAD_BUDGET_BYTES)
        );

        let accepted = client
            .post(format!(
                "http://{address}/v1/installations/upgrade-task-report"
            ))
            .header(header::CONTENT_TYPE, "application/json")
            .body(bounded)
            .send()
            .await
            .unwrap();
        let oversized = format!(
            "{{\"logs\":\"{}\"}}",
            "x".repeat(INSTALLATION_UPGRADE_TASK_BODY_LIMIT_BYTES)
        );
        let rejected = client
            .post(format!(
                "http://{address}/v1/installations/upgrade-task-report"
            ))
            .header(header::CONTENT_TYPE, "application/json")
            .body(oversized)
            .send()
            .await
            .unwrap();
        server.abort();

        assert_eq!(accepted.status(), StatusCode::OK);
        assert_eq!(rejected.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn upgrade_status_response_body_is_bounded() {
        async fn oversized_upgrade_status() -> Body {
            Body::from(vec![
                b'x';
                INSTALLATION_UPGRADE_STATUS_RESPONSE_MAX_BYTES + 1
            ])
        }

        let app = Router::new().route("/status", get(oversized_upgrade_status));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upgrade status server");
        let address = listener
            .local_addr()
            .expect("upgrade status server address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve oversized upgrade status");
        });

        let response = reqwest::get(format!("http://{address}/status"))
            .await
            .expect("request oversized upgrade status");
        let error = read_bounded_upgrade_status_body(response)
            .await
            .expect_err("oversized client status must be rejected");
        server.abort();

        assert!(error.to_string().contains("response exceeds"));
    }

    #[test]
    fn dashboard_session_policy_accepts_valid_session() {
        let viewer_email = require_dashboard_session_email(Some("owner@example.com".into()))
            .expect("valid dashboard session should resolve");

        assert_eq!(viewer_email, "owner@example.com");
    }

    #[test]
    fn public_chat_responses_are_not_cached_or_indexed() {
        let headers = public_chat_headers();
        assert_eq!(
            headers
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        assert_eq!(
            headers
                .get("x-robots-tag")
                .and_then(|value| value.to_str().ok()),
            Some("noindex, noarchive")
        );
    }

    #[test]
    fn dashboard_session_policy_rejects_invalid_or_expired_credentials() {
        let error = require_dashboard_session_email(None)
            .expect_err("present but invalid dashboard credentials must be rejected");

        assert_eq!(error.into_response().status(), StatusCode::UNAUTHORIZED);
    }

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
    fn session_token_extraction_distinguishes_missing_cookie_and_bearer() {
        let mut headers = HeaderMap::new();
        assert_eq!(extract_session_token(&headers), None);

        headers.insert(
            axum::http::header::COOKIE,
            "other=value; cc_switch_router_access=cookie-token"
                .parse()
                .unwrap(),
        );
        assert_eq!(extract_session_token(&headers), Some("cookie-token"));

        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer bearer-token".parse().unwrap(),
        );
        assert_eq!(extract_session_token(&headers), Some("bearer-token"));
    }

    #[test]
    fn share_ui_static_paths_do_not_capture_api_requests() {
        for path in [
            "/",
            "/favicon.ico",
            "/install-client.sh",
            "/router-logo.svg",
            "/world-map.svg",
            "/flags/1f1f9-1f1fc.png",
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
    fn share_log_ticker_event_preserves_email_and_usage() {
        let mut log = share_log("req-share-usage", 1);
        log.user_email = Some("share-user@example.com".into());
        log.input_tokens = 10;
        log.output_tokens = 20;
        log.cache_read_tokens = 3;
        log.cache_creation_tokens = 4;
        let event = share_log_to_ticker_event(&ticker_share(Vec::new()), &log);

        assert_eq!(event.user_email.as_deref(), Some("share-user@example.com"));
        assert_eq!(event.input_tokens, Some(10));
        assert_eq!(event.output_tokens, Some(20));
        assert_eq!(event.cache_read_tokens, Some(3));
        assert_eq!(event.cache_creation_tokens, Some(4));
        assert_eq!(event.total_tokens, Some(37));

        let json = serde_json::to_value(event).expect("serialize ticker event");
        assert_eq!(json["inputTokens"], 10);
        assert_eq!(json["outputTokens"], 20);
        assert_eq!(json["cacheReadTokens"], 3);
        assert_eq!(json["cacheCreationTokens"], 4);
        assert_eq!(json["totalTokens"], 37);
        assert_eq!(json["actualModel"], "gpt-5");
        assert_eq!(json["latencyMs"], 1);
    }

    #[test]
    fn share_log_ticker_event_distinguishes_pending_from_observed_zero() {
        let mut pending = share_log("req-share-pending", 1);
        pending.usage_state = "pending".into();
        pending.stream_status = Some("streaming".into());
        pending.usage_revision = 1;
        let pending_event = share_log_to_ticker_event(&ticker_share(Vec::new()), &pending);

        assert_eq!(pending_event.usage_state.as_deref(), Some("pending"));
        assert_eq!(pending_event.stream_status.as_deref(), Some("streaming"));
        assert_eq!(pending_event.usage_revision, Some(1));
        assert_eq!(pending_event.input_tokens, None);
        assert_eq!(pending_event.total_tokens, None);
        let pending_json = serde_json::to_value(pending_event).unwrap();
        assert!(pending_json.get("inputTokens").is_none());
        assert!(pending_json.get("totalTokens").is_none());

        let observed = share_log("req-share-zero", 2);
        let observed_event = share_log_to_ticker_event(&ticker_share(Vec::new()), &observed);
        assert_eq!(observed_event.usage_state.as_deref(), Some("observed"));
        assert_eq!(observed_event.input_tokens, Some(0));
        assert_eq!(observed_event.total_tokens, Some(0));
        let observed_json = serde_json::to_value(observed_event).unwrap();
        assert_eq!(observed_json["inputTokens"], 0);
        assert_eq!(observed_json["totalTokens"], 0);
    }

    #[test]
    fn ticker_usage_merge_keeps_highest_revision_terminal_state() {
        let mut pending = share_log("req-share-revision", 1);
        pending.usage_state = "pending".into();
        pending.stream_status = Some("streaming".into());
        pending.usage_revision = 1;
        let mut event = share_log_to_ticker_event(&ticker_share(Vec::new()), &pending);

        let mut observed = pending.clone();
        observed.usage_state = "observed".into();
        observed.stream_status = Some("completed".into());
        observed.usage_revision = 2;
        let observed_event = share_log_to_ticker_event(&ticker_share(Vec::new()), &observed);
        merge_persisted_ticker_event(&mut event, observed_event);

        assert_eq!(event.usage_state.as_deref(), Some("observed"));
        assert_eq!(event.stream_status.as_deref(), Some("completed"));
        assert_eq!(event.usage_revision, Some(2));
        assert_eq!(event.total_tokens, Some(0));

        let stale_event = share_log_to_ticker_event(&ticker_share(Vec::new()), &pending);
        merge_persisted_ticker_event(&mut event, stale_event);
        assert_eq!(event.usage_state.as_deref(), Some("observed"));
        assert_eq!(event.stream_status.as_deref(), Some("completed"));
        assert_eq!(event.usage_revision, Some(2));
        assert_eq!(event.total_tokens, Some(0));
    }

    #[test]
    fn confirmed_request_events_keep_display_fields_without_ticker_share_log() {
        let mut log = share_log("req-display", 1);
        log.actual_model = "gpt-5.6-sol".into();
        log.request_agent = "codex".into();
        log.latency_ms = 14_600;
        log.input_tokens = 86_000;
        let response = DashboardResponse {
            generated_at: Utc::now(),
            stats: crate::models::DashboardStats {
                clients: 0,
                active_shares: 0,
                total_active_requests: 0,
            },
            map: crate::models::DashboardMap {
                server: None,
                countries: Vec::new(),
            },
            map_display: MapDisplaySettings::default(),
            clients: Vec::new(),
            shares: Vec::new(),
            ticker_shares: vec![crate::models::DashboardTickerShare {
                share_id: "share-1".into(),
                share_name: "Share".into(),
                subdomain: "share-sub".into(),
                recent_requests: Vec::new(),
            }],
            country_counts: HashMap::new(),
            country_boards: HashMap::new(),
            user_country_counts: HashMap::new(),
            recent_request_events: Vec::new(),
        };
        let snapshot = RecentTrafficSnapshot {
            country_counts: HashMap::new(),
            events: Vec::new(),
            recent_events: Vec::new(),
        };
        let (events, _) = confirmed_request_events(&snapshot, &response, &[log]);
        let event = events
            .iter()
            .find(|event| event.request_id == "req-display")
            .expect("persisted event");
        assert_eq!(event.actual_model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(event.request_agent.as_deref(), Some("codex"));
        assert_eq!(event.latency_ms, Some(14_600));
        assert_eq!(event.total_tokens, Some(86_000));
    }

    #[test]
    fn live_context_overrides_email_without_erasing_persisted_usage() {
        let mut log = share_log("req-live-merge", 1);
        log.user_email = Some("persisted@example.com".into());
        log.input_tokens = 10;
        log.output_tokens = 20;
        log.cache_read_tokens = 3;
        log.cache_creation_tokens = 4;
        log.user_country = Some("JP".into());
        log.user_country_iso3 = Some("JPN".into());
        let mut persisted = share_log_to_ticker_event(&ticker_share(Vec::new()), &log);
        let live = live_event(
            "req-live-merge",
            Some("actual@example.com"),
            Some("US"),
            Some("USA"),
        );

        merge_ticker_event_country(&mut persisted, &live);

        assert_eq!(persisted.user_email.as_deref(), Some("actual@example.com"));
        assert_eq!(persisted.user_country.as_deref(), Some("US"));
        assert_eq!(persisted.user_country_iso3.as_deref(), Some("USA"));
        assert_eq!(persisted.input_tokens, Some(10));
        assert_eq!(persisted.output_tokens, Some(20));
        assert_eq!(persisted.cache_read_tokens, Some(3));
        assert_eq!(persisted.cache_creation_tokens, Some(4));
        assert_eq!(persisted.total_tokens, Some(37));
    }

    #[test]
    fn live_email_overrides_logs_even_when_country_is_complete() {
        let snapshot = RecentTrafficSnapshot {
            country_counts: HashMap::new(),
            events: vec![live_event(
                "req-email-override",
                Some("actual@example.com"),
                Some("US"),
                Some("USA"),
            )],
            recent_events: Vec::new(),
        };
        let mut share_log = share_log("req-email-override", 1);
        share_log.user_country = Some("JP".into());
        share_log.user_country_iso3 = Some("JPN".into());
        share_log.user_email = Some("stale@example.com".into());
        let mut ticker_shares = vec![ticker_share(vec![share_log])];
        enrich_share_ticker_logs_with_live_country(&mut ticker_shares, &snapshot);
        assert_eq!(
            ticker_shares[0].recent_requests[0].user_email.as_deref(),
            Some("actual@example.com")
        );
        assert_eq!(
            ticker_shares[0].recent_requests[0].user_country.as_deref(),
            Some("JP")
        );
    }

    #[test]
    fn health_checks_do_not_consume_request_ticker_limit() {
        let mut requests = vec![share_log("req-user", 1)];
        for index in 0..=DASHBOARD_REQUEST_TICKER_LIMIT {
            let mut health = share_log(&format!("req-health-{index}"), index as i64 + 2);
            health.is_health_check = true;
            requests.push(health);
        }
        let response = dashboard_response(vec![ticker_share(requests.clone())]);
        let snapshot = RecentTrafficSnapshot {
            country_counts: HashMap::new(),
            events: Vec::new(),
            recent_events: Vec::new(),
        };

        let (events, _) = confirmed_request_events(&snapshot, &response, &requests);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].request_id, "req-user");
        assert!(!events[0].is_health_check);
    }

    #[test]
    fn confirmed_request_events_restores_persisted_share_logs_up_to_limit() {
        let response = DashboardResponse {
            generated_at: Utc::now(),
            stats: crate::models::DashboardStats {
                clients: 0,
                active_shares: 0,
                total_active_requests: 0,
            },
            map: crate::models::DashboardMap {
                server: None,
                countries: Vec::new(),
            },
            map_display: MapDisplaySettings::default(),
            clients: Vec::new(),
            shares: Vec::new(),
            ticker_shares: vec![crate::models::DashboardTickerShare {
                share_id: "share-1".into(),
                share_name: "Share".into(),
                subdomain: "share-sub".into(),
                recent_requests: (1..=7)
                    .map(|index| share_log(&format!("req-{index}"), index))
                    .collect(),
            }],
            country_counts: HashMap::new(),
            country_boards: HashMap::new(),
            user_country_counts: HashMap::new(),
            recent_request_events: Vec::new(),
        };
        let snapshot = RecentTrafficSnapshot {
            country_counts: HashMap::new(),
            events: Vec::new(),
            recent_events: Vec::new(),
        };

        let (events, country_counts) =
            confirmed_request_events(&snapshot, &response, &global_logs_from_response(&response));

        assert_eq!(country_counts.len(), 0);
        assert_eq!(
            events
                .iter()
                .map(|event| event.request_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "req-1", "req-2", "req-3", "req-4", "req-5", "req-6", "req-7"
            ]
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
                countries: Vec::new(),
            },
            map_display: MapDisplaySettings::default(),
            clients: Vec::new(),
            shares: Vec::new(),
            ticker_shares: vec![crate::models::DashboardTickerShare {
                share_id: "share-1".into(),
                share_name: "Share".into(),
                subdomain: "share-sub".into(),
                recent_requests: vec![share_log],
            }],
            country_counts: HashMap::new(),
            country_boards: HashMap::new(),
            user_country_counts: HashMap::new(),
            recent_request_events: Vec::new(),
        };
        let snapshot = RecentTrafficSnapshot {
            country_counts: HashMap::new(),
            events: Vec::new(),
            recent_events: Vec::new(),
        };

        let (events, _) =
            confirmed_request_events(&snapshot, &response, &global_logs_from_response(&response));

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
            ..Default::default()
        };
        let snapshot = RecentTrafficSnapshot {
            country_counts: HashMap::from([("USA".to_string(), 1)]),
            events: vec![live.clone()],
            recent_events: vec![live],
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
                countries: Vec::new(),
            },
            map_display: MapDisplaySettings::default(),
            clients: Vec::new(),
            shares: Vec::new(),
            ticker_shares: vec![crate::models::DashboardTickerShare {
                share_id: "share-1".into(),
                share_name: "Persisted Share".into(),
                subdomain: "persisted-sub".into(),
                recent_requests: vec![share_log("req-1", 1)],
            }],
            country_counts: HashMap::new(),
            country_boards: HashMap::new(),
            user_country_counts: HashMap::new(),
            recent_request_events: Vec::new(),
        };

        let (events, country_counts) =
            confirmed_request_events(&snapshot, &response, &global_logs_from_response(&response));

        assert_eq!(events.len(), 1);
        assert!(events[0].is_inflight);
        assert_eq!(events[0].share_subdomain.as_deref(), Some("live-sub"));
        assert_eq!(country_counts.get("USA"), Some(&1));
    }

    #[test]
    fn confirmed_request_events_backfills_country_from_live_snapshot() {
        let live = RecentRequestEvent {
            request_id: "req-country-live".into(),
            share_id: "share-1".into(),
            share_name: Some("Share".into()),
            share_subdomain: Some("share-sub".into()),
            user_country: Some("US".into()),
            user_country_iso3: Some("USA".into()),
            started_at: Utc::now(),
            ..Default::default()
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
                countries: Vec::new(),
            },
            map_display: MapDisplaySettings::default(),
            clients: Vec::new(),
            shares: Vec::new(),
            ticker_shares: vec![crate::models::DashboardTickerShare {
                share_id: "share-1".into(),
                share_name: "Share".into(),
                subdomain: "share-sub".into(),
                recent_requests: vec![share_log("req-country-live", 1)],
            }],
            country_counts: HashMap::new(),
            country_boards: HashMap::new(),
            user_country_counts: HashMap::new(),
            recent_request_events: Vec::new(),
        };

        let (events, _) =
            confirmed_request_events(&snapshot, &response, &global_logs_from_response(&response));

        assert_eq!(events[0].user_country.as_deref(), Some("US"));
        assert_eq!(events[0].user_country_iso3.as_deref(), Some("USA"));
    }

    fn share_log(request_id: &str, created_at: i64) -> crate::models::ShareRequestLogEntry {
        crate::models::ShareRequestLogEntry {
            export_sequence: 0,
            request_id: request_id.into(),
            request_kind: "text".into(),
            operation: "responses".into(),
            parent_request_id: None,
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
            requested_reasoning_effort: None,
            effective_reasoning_effort: None,
            client_service_tier: None,
            effective_service_tier: None,
            service_tier_decision: None,
            usage_state: "observed".into(),
            stream_status: None,
            usage_revision: 0,
            error_message: None,
            status_code: 200,
            latency_ms: 1,
            first_token_ms: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            cache_usage_observed: true,
            usage_estimated: false,
            quota_tokens: None,
            is_streaming: false,
            session_id: None,
            user_country: None,
            user_country_iso3: None,
            user_email: None,
            media_task_id: None,
            media_status: None,
            video_duration_seconds: None,
            video_resolution: None,
            video_aspect_ratio: None,
            created_at,
            is_health_check: false,
        }
    }

    fn global_logs_from_response(response: &DashboardResponse) -> Vec<ShareRequestLogEntry> {
        response
            .ticker_shares
            .iter()
            .flat_map(|share| share.recent_requests.iter().cloned())
            .collect()
    }

    fn ticker_share(
        recent_requests: Vec<crate::models::ShareRequestLogEntry>,
    ) -> crate::models::DashboardTickerShare {
        crate::models::DashboardTickerShare {
            share_id: "share-1".into(),
            share_name: "Share".into(),
            subdomain: "share-sub".into(),
            recent_requests,
        }
    }

    fn live_event(
        request_id: &str,
        user_email: Option<&str>,
        user_country: Option<&str>,
        user_country_iso3: Option<&str>,
    ) -> RecentRequestEvent {
        RecentRequestEvent {
            request_id: request_id.into(),
            share_id: "share-1".into(),
            share_name: Some("Live Share".into()),
            share_subdomain: Some("live-share-sub".into()),
            user_country: user_country.map(str::to_string),
            user_country_iso3: user_country_iso3.map(str::to_string),
            user_email: user_email.map(str::to_string),
            started_at: Utc::now(),
            is_inflight: true,
            ..Default::default()
        }
    }

    fn dashboard_response(
        ticker_shares: Vec<crate::models::DashboardTickerShare>,
    ) -> DashboardResponse {
        DashboardResponse {
            generated_at: Utc::now(),
            stats: crate::models::DashboardStats {
                clients: 0,
                active_shares: 0,
                total_active_requests: 0,
            },
            map: crate::models::DashboardMap {
                server: None,
                countries: Vec::new(),
            },
            map_display: MapDisplaySettings::default(),
            clients: Vec::new(),
            shares: Vec::new(),
            ticker_shares,
            country_counts: HashMap::new(),
            country_boards: HashMap::new(),
            user_country_counts: HashMap::new(),
            recent_request_events: Vec::new(),
        }
    }

    #[test]
    fn dashboard_request_log_visibility_keeps_public_share_identity_without_internal_sessions() {
        let mut own_share_log = share_log("req-own", 2);
        own_share_log.user_email = Some("viewer@example.com".into());
        own_share_log.session_id = Some("own-session".into());
        let mut foreign_share_log = share_log("req-foreign", 1);
        foreign_share_log.user_email = Some("foreign@example.com".into());
        foreign_share_log.session_id = Some("foreign-session".into());

        let mut response = dashboard_response(vec![ticker_share(vec![
            own_share_log.clone(),
            foreign_share_log.clone(),
        ])]);
        response.recent_request_events = vec![
            RecentRequestEvent {
                request_id: "req-own".into(),
                share_id: "share-1".into(),
                user_email: Some("viewer@example.com".into()),
                ..Default::default()
            },
            RecentRequestEvent {
                request_id: "req-foreign".into(),
                share_id: "share-1".into(),
                user_email: Some("foreign@example.com".into()),
                ..Default::default()
            },
        ];

        apply_dashboard_request_log_visibility(&mut response, false);

        let ticker_logs = &response.ticker_shares[0].recent_requests;
        assert_eq!(ticker_logs.len(), 2);
        assert_eq!(
            ticker_logs[0].user_email.as_deref(),
            Some("viewer@example.com")
        );
        assert!(ticker_logs[0].session_id.is_none());
        assert_eq!(
            ticker_logs[1].user_email.as_deref(),
            Some("foreign@example.com")
        );
        assert!(ticker_logs[1].session_id.is_none());
        assert_eq!(
            response.recent_request_events[0].user_email.as_deref(),
            Some("viewer@example.com")
        );
        assert_eq!(
            response.recent_request_events[1].user_email.as_deref(),
            Some("foreign@example.com")
        );

        let mut detail_logs = vec![own_share_log, foreign_share_log];
        remove_share_request_session_ids(&mut detail_logs);
        assert_eq!(detail_logs.len(), 2);
        assert_eq!(detail_logs[0].request_id, "req-own");
        assert_eq!(
            detail_logs[1].user_email.as_deref(),
            Some("foreign@example.com")
        );
        assert!(detail_logs.iter().all(|log| log.session_id.is_none()));
    }

    #[test]
    fn dashboard_request_log_visibility_preserves_admin_identity() {
        let mut log = share_log("req-admin", 1);
        log.user_email = Some("buyer@example.com".into());
        log.session_id = Some("buyer-session".into());
        let mut response = dashboard_response(vec![ticker_share(vec![log])]);
        response.recent_request_events = vec![RecentRequestEvent {
            request_id: "req-admin".into(),
            share_id: "share-1".into(),
            user_email: Some("buyer@example.com".into()),
            ..Default::default()
        }];

        apply_dashboard_request_log_visibility(&mut response, true);

        assert_eq!(
            response.ticker_shares[0].recent_requests[0]
                .user_email
                .as_deref(),
            Some("buyer@example.com")
        );
        assert_eq!(
            response.ticker_shares[0].recent_requests[0]
                .session_id
                .as_deref(),
            Some("buyer-session")
        );
        assert_eq!(
            response.recent_request_events[0].user_email.as_deref(),
            Some("buyer@example.com")
        );
    }

    #[test]
    fn image_generation_prompt_redaction_keeps_owner_prompt_only() {
        let mut owner_logs = vec![image_generation_log("owner-log")];
        apply_image_generation_log_visibility(
            &mut owner_logs,
            &ImageRequestLogViewContext {
                can_view_prompt: true,
                can_view_result_url: true,
            },
        );
        assert_eq!(
            owner_logs[0].prompt_preview.as_deref(),
            Some("private prompt")
        );
        assert_eq!(
            owner_logs[0].result_url.as_deref(),
            Some("/v1/image-results/owner-log?token=owner-token")
        );

        let mut public_logs = vec![image_generation_log("public-log")];
        apply_image_generation_log_visibility(
            &mut public_logs,
            &ImageRequestLogViewContext {
                can_view_prompt: false,
                can_view_result_url: false,
            },
        );
        assert_eq!(public_logs[0].prompt_preview, None);
        assert_eq!(public_logs[0].result_url, None);
        assert_eq!(public_logs[0].model, "gpt-5.5");
        assert_eq!(public_logs[0].status_code, Some(200));
        assert_eq!(public_logs[0].result_size_bytes, Some(1024));
    }

    fn image_generation_log(request_id: &str) -> crate::models::ImageGenerationRequestLogEntry {
        crate::models::ImageGenerationRequestLogEntry {
            request_id: request_id.into(),
            share_id: "share-1".into(),
            share_name: "Share".into(),
            installation_id: "inst-1".into(),
            provider_id: "provider-1".into(),
            provider_name: "OpenAI Official".into(),
            app_type: "codex".into(),
            model: "gpt-5.5".into(),
            status: "succeeded".into(),
            status_code: Some(200),
            latency_ms: 1,
            created_at: 1,
            completed_at: Some(2),
            prompt_preview: Some("private prompt".into()),
            error_message: None,
            result_mime_type: Some("image/png".into()),
            result_size_bytes: Some(1024),
            result_url: None,
            result_storage_key: Some("share-1/owner-log.png".into()),
            result_access_token: Some("owner-token".into()),
            created_by_email: Some("user@example.com".into()),
            user_country: Some("US".into()),
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
    fn contract_probe_response_modes_are_explicit() {
        assert_eq!(
            probe_response_mode("json").unwrap(),
            ProbeResponseMode::Json
        );
        assert_eq!(
            probe_response_mode("anthropic_sse").unwrap(),
            ProbeResponseMode::AnthropicSse
        );
        assert_eq!(
            probe_response_mode("responses_sse").unwrap(),
            ProbeResponseMode::ResponsesSse
        );
        assert_eq!(
            probe_response_mode("gemini_sse").unwrap(),
            ProbeResponseMode::GeminiSse
        );
        assert!(probe_response_mode("unknown").is_err());
        assert_eq!(
            effective_probe_response_mode(
                ProbeResponseMode::ImageSse,
                Some("text/event-stream; charset=utf-8"),
            ),
            ProbeResponseMode::ImageSse
        );
        assert_eq!(
            effective_probe_response_mode(ProbeResponseMode::ImageSse, Some("application/json"),),
            ProbeResponseMode::ImageJson
        );
    }

    #[test]
    fn model_probe_test_respects_the_enabled_app_surface() {
        let share = ShareForTest {
            contract_version: 4,
            subdomain: "share-subdomain".into(),
            owner_email: "owner@example.com".into(),
            user_grants: Default::default(),
            bindings: Default::default(),
            support: crate::models::ShareSupport {
                claude: false,
                codex: true,
                gemini: false,
            },
            app_runtimes: Default::default(),
            app_providers: Default::default(),
            grok_media_policy: Default::default(),
        };

        assert!(share_model_probe_app_enabled(&share, "codex"));
        assert!(!share_model_probe_app_enabled(&share, "claude"));
        assert!(!share_model_probe_app_enabled(&share, "gemini"));
    }

    #[test]
    fn responses_probe_requires_completed_terminal_event_across_chunks() {
        let mut tracker = ProbeSseTracker::new(ProbeResponseMode::ResponsesSse);
        tracker.push(b"event: response.com");
        tracker.push(b"pleted\ndata: {\"type\":\"response.completed\"}\n\ndata: [DONE]\n\n");
        let (terminal_event, error) = tracker.finish();

        assert_eq!(terminal_event.as_deref(), Some("response.completed"));
        assert!(error.is_none());
    }

    #[test]
    fn responses_probe_rejects_failure_terminal_event() {
        let mut tracker = ProbeSseTracker::new(ProbeResponseMode::ResponsesSse);
        tracker
            .push(b"data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\"}}\n\n");
        let (terminal_event, error) = tracker.finish();

        assert_eq!(terminal_event.as_deref(), Some("response.failed"));
        assert!(error.unwrap().contains("response.failed"));
    }

    #[test]
    fn responses_probe_rejects_done_without_semantic_terminal_event() {
        let mut tracker = ProbeSseTracker::new(ProbeResponseMode::ResponsesSse);
        tracker.push(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n");
        tracker.push(b"data: [DONE]\n\n");
        let (terminal_event, error) = tracker.finish();

        assert!(terminal_event.is_none());
        assert!(error.unwrap().contains("before required terminal event"));
    }

    #[test]
    fn anthropic_and_gemini_probes_use_their_protocol_terminals() {
        let mut anthropic = ProbeSseTracker::new(ProbeResponseMode::AnthropicSse);
        anthropic.push(b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
        assert_eq!(anthropic.finish().0.as_deref(), Some("message_stop"));

        let mut gemini = ProbeSseTracker::new(ProbeResponseMode::GeminiSse);
        gemini.push(
            b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ok\"}]},\"finishReason\":\"STOP\"}]}\n\n",
        );
        assert_eq!(gemini.finish().0.as_deref(), Some("gemini.completed"));
    }

    #[tokio::test]
    async fn probe_body_drains_past_preview_before_accepting_terminal_event() {
        async fn streaming_probe_response() -> Body {
            let prefix = bytes::Bytes::from(vec![b'x'; TEST_BODY_CAP + 1_024]);
            let terminal = bytes::Bytes::from_static(
                b"\nevent: response.completed\ndata: {\"type\":\"response.completed\"}\n\n",
            );
            Body::from_stream(futures_util::stream::iter([
                Ok::<_, std::convert::Infallible>(prefix),
                Ok(terminal),
            ]))
        }

        let app = Router::new().route("/probe", get(streaming_probe_response));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind probe server");
        let address = listener.local_addr().expect("probe server address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve probe response");
        });

        let response = reqwest::get(format!("http://{address}/probe"))
            .await
            .expect("request probe response");
        let body = read_probe_body(response, ProbeResponseMode::ResponsesSse).await;
        server.abort();

        assert_eq!(body.preview.len(), TEST_BODY_CAP);
        assert!(body.total_bytes > body.preview.len());
        assert_eq!(body.terminal_event.as_deref(), Some("response.completed"));
        assert!(body.error.is_none());
    }

    #[tokio::test]
    async fn image_probe_accepts_legal_json_larger_than_the_text_probe_cap() {
        let mut payload = b"\n{\"data\":[{\"b64_json\":\"".to_vec();
        payload.extend(std::iter::repeat_n(b'a', TEST_JSON_PARSE_CAP + 1_024));
        payload.extend_from_slice(b"\"}]}\n");
        let app = Router::new().route(
            "/probe",
            get(move || {
                let payload = payload.clone();
                async move { ([(header::CONTENT_TYPE, "application/json")], payload) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind image JSON probe server");
        let address = listener.local_addr().expect("image JSON probe address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve image JSON probe response");
        });

        let response = reqwest::get(format!("http://{address}/probe"))
            .await
            .expect("request image JSON probe response");
        let body = read_probe_body(response, ProbeResponseMode::ImageJson).await;
        server.abort();

        assert_eq!(body.preview.len(), TEST_BODY_CAP);
        assert!(body.total_bytes > TEST_JSON_PARSE_CAP);
        assert_eq!(body.terminal_event.as_deref(), Some("image_json.completed"));
        assert!(body.error.is_none());
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

async fn prune_shares(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<SharePruneRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let pruned = state
        .store
        .prune_shares(input, extract_client_metadata(&headers, addr))
        .await?;
    Ok(Json(serde_json::json!({ "ok": true, "pruned": pruned })))
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

async fn batch_sync_share_descriptors(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ShareBatchSyncRequest>,
) -> Result<Json<ShareDescriptorBatchSyncResponse>, AppError> {
    let acks = state
        .store
        .batch_sync_share_descriptors(input, extract_client_metadata(&headers, addr))
        .await?;
    Ok(Json(ShareDescriptorBatchSyncResponse { ok: true, acks }))
}

async fn update_share_settings(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
    Json(input): Json<ShareSettingsUpdateRequest>,
) -> Result<Json<crate::models::ShareSettingsUpdateResponse>, AppError> {
    let current_user_email = require_user_email(&state, &headers, "share:write").await?;
    Ok(Json(
        update_share_settings_with_email(
            &state,
            &share_id,
            &current_user_email,
            input.patch,
            input.base_config_revision,
        )
        .await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareClientBanListQuery {
    #[serde(default = "default_share_client_ban_limit")]
    limit: usize,
    cursor: Option<String>,
}

fn default_share_client_ban_limit() -> usize {
    50
}

async fn require_share_owner(
    state: &ServerState,
    headers: &HeaderMap,
    share_id: &str,
) -> Result<String, AppError> {
    let email = require_user_email(state, headers, "share:write").await?;
    let owner = state
        .store
        .lookup_share_owner_email(share_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Share not found".into()))?;
    if !owner.eq_ignore_ascii_case(&email) {
        return Err(AppError::Forbidden(
            "only Share owner can manage client bans".into(),
        ));
    }
    Ok(email)
}

async fn list_share_client_bans(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
    Query(query): Query<ShareClientBanListQuery>,
) -> Result<(HeaderMap, Json<crate::abuse::ShareClientBanPage>), AppError> {
    require_share_owner(&state, &headers, &share_id).await?;
    let page = state
        .store
        .list_active_share_client_bans(&share_id, query.limit, query.cursor.as_deref())
        .await?;
    let mut response_headers = HeaderMap::new();
    response_headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((response_headers, Json(page)))
}

async fn unban_share_client(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path((share_id, ban_id)): Path<(String, String)>,
) -> Result<(HeaderMap, Json<ShareClientUnbanResponse>), AppError> {
    let actor_email = require_share_owner(&state, &headers, &share_id).await?;
    let metadata = extract_client_metadata(&headers, addr);
    let (client_ip, already_unbanned) = state
        .store
        .unban_share_client(&share_id, &ban_id, &actor_email, metadata.ip.as_deref())
        .await?;
    state.share_abuse.unban(&share_id, &client_ip).await;
    let audit_payload = serde_json::json!({
        "shareId": share_id,
        "banId": ban_id,
        "clientIp": client_ip,
        "alreadyUnbanned": already_unbanned,
    });
    let _ = state
        .store
        .record_admin_audit(
            Some(&actor_email),
            "share.client_ban.unban",
            Some(&audit_payload),
            metadata.ip.as_deref(),
        )
        .await;
    let mut response_headers = HeaderMap::new();
    response_headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((
        response_headers,
        Json(ShareClientUnbanResponse {
            ok: true,
            ban_id,
            already_unbanned,
        }),
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
            .share_usage_by_email(&share_id, query.period.as_deref().unwrap_or("24h"))
            .await?,
    ))
}

async fn share_user_limit_status(
    State(state): State<ServerState>,
    Path(share_id): Path<String>,
) -> Result<Json<crate::models::ShareUserLimitStatusResponse>, AppError> {
    Ok(Json(state.store.share_user_limit_status(&share_id).await?))
}

async fn update_share_settings_with_email(
    state: &ServerState,
    share_id: &str,
    current_user_email: &str,
    patch: ShareSettingsPatch,
    base_config_revision: Option<u64>,
) -> Result<crate::models::ShareSettingsUpdateResponse, AppError> {
    let mut response = state
        .store
        .create_share_settings_edit_at_revision(
            share_id,
            current_user_email,
            patch,
            base_config_revision,
        )
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
        for (event_name, data) in initial_share_edit_stream_events() {
            yield Ok(Event::default().event(event_name).data(data));
        }
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

fn initial_share_edit_stream_events() -> [(&'static str, &'static str); 2] {
    [("ready", "{}"), ("resync", "{}")]
}

async fn batch_sync_share_request_logs(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ShareRequestLogBatchSyncRequest>,
) -> Result<Json<ShareRequestLogBatchSyncResponse>, AppError> {
    let snapshot = state.recent_traffic.snapshot().await;
    let live_context_map = live_request_context_by_request_id(&snapshot);
    let sync = state
        .store
        .batch_sync_share_request_logs(
            input,
            extract_client_metadata(&headers, addr),
            "",
            live_context_map,
        )
        .await?;
    state.metrics.record_share_request_logs(&sync.accepted_logs);
    Ok(Json(ShareRequestLogBatchSyncResponse {
        ok: true,
        acks: sync.acks,
    }))
}

async fn refresh_share_runtime(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ShareRuntimeRefreshRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let installation_id = input.installation_id.clone();
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
        &state.store,
        &state.config,
        &client,
        &refresh.subdomain,
        &refresh.share_id,
        &installation_id,
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

fn parse_signed_gateway_json<T: DeserializeOwned>(body: &[u8]) -> Result<T, AppError> {
    serde_json::from_slice(body)
        .map_err(|error| AppError::BadRequest(format!("invalid signed gateway JSON body: {error}")))
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
    if let Some(session) = resolve_router_session(state, headers).await? {
        return Ok(Some(session.email));
    }
    Ok(dev_auth_bypass_enabled().then(dev_auth_email))
}

fn require_dashboard_session_email(email: Option<String>) -> Result<String, AppError> {
    email.ok_or_else(|| AppError::Unauthorized("session not found".into()))
}

async fn extract_dashboard_session_email(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<Option<String>, AppError> {
    if let Some(session) = resolve_router_session(state, headers).await? {
        return require_dashboard_session_email(Some(session.email)).map(Some);
    }
    Ok(dev_auth_bypass_enabled().then(dev_auth_email))
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
struct ClientChatMessageQuery {
    #[serde(default)]
    before_seq: Option<i64>,
    #[serde(default)]
    after_seq: Option<i64>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientChatStreamQuery {
    #[serde(default)]
    after_seq: Option<i64>,
}

fn session_token_candidates(headers: &HeaderMap) -> Vec<&str> {
    let mut candidates = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Some(bearer) = extract_bearer_token(headers) {
        seen.insert(bearer);
        candidates.push(bearer);
    }
    if let Some(cookie) = extract_router_access_cookie(headers) {
        if seen.insert(cookie) {
            candidates.push(cookie);
        }
    }
    candidates
}

async fn resolve_session_auth_token(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<Option<String>, AppError> {
    for token in session_token_candidates(headers) {
        if state
            .store
            .resolve_session_by_access_token(token)
            .await?
            .is_some()
        {
            return Ok(Some(token.to_string()));
        }
    }
    Ok(None)
}

pub(crate) async fn resolve_router_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<Option<AuthSession>, AppError> {
    if session_token_candidates(headers).is_empty() {
        return Ok(dev_auth_bypass_enabled().then(dev_auth_session));
    }
    for token in session_token_candidates(headers) {
        if let Some(session) = state.store.resolve_session_by_access_token(token).await? {
            return Ok(Some(session));
        }
    }
    Ok(None)
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
        auth_source_kind: "auth_device".into(),
        auth_source_id: "dev-auth-bypass-device".into(),
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
        is_admin: true,
    }
}

pub(crate) async fn require_admin_session(
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

async fn require_client_chat_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<AuthSession, AppError> {
    resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required to send chat messages".into()))
}

fn public_chat_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        HeaderName::from_static("x-robots-tag"),
        HeaderValue::from_static("noindex, noarchive"),
    );
    headers
}

async fn client_chat_room(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(installation_id): Path<String>,
) -> Result<(HeaderMap, Json<ClientChatRoomResponse>), AppError> {
    let metadata = extract_client_metadata(&headers, addr);
    state
        .store
        .enforce_client_chat_public_read_rate(metadata.ip.as_deref())
        .await?;
    let session = resolve_router_session(&state, &headers).await?;
    let room = state
        .store
        .get_client_chat_room_by_installation(
            &installation_id,
            session.as_ref().map(|session| session.user_id.as_str()),
        )
        .await?;
    Ok((public_chat_headers(), Json(ClientChatRoomResponse { room })))
}

async fn lookup_chat_rooms(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ClientChatRoomLookupRequest>,
) -> Result<(HeaderMap, Json<ClientChatRoomListResponse>), AppError> {
    let metadata = extract_client_metadata(&headers, addr);
    state
        .store
        .enforce_client_chat_public_read_rate(metadata.ip.as_deref())
        .await?;
    let session = resolve_router_session(&state, &headers).await?;
    Ok((
        public_chat_headers(),
        Json(
            state
                .store
                .lookup_chat_rooms(
                    input.installation_ids,
                    input.last_read_seq_by_installation,
                    session.as_ref().map(|session| session.user_id.as_str()),
                )
                .await?,
        ),
    ))
}

async fn list_visited_chat_rooms(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ClientChatRoomListResponse>, AppError> {
    let session = require_client_chat_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .list_visited_chat_rooms(&session.user_id)
            .await?,
    ))
}

async fn client_chat_meta(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_client_chat_session(&state, &headers).await?;
    let rooms = state
        .store
        .list_visited_chat_rooms(&session.user_id)
        .await?;
    Ok(Json(serde_json::json!({
        "totalUnread": rooms.total_unread,
    })))
}

async fn record_client_chat_visit(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(room_id): Path<String>,
) -> Result<Json<ClientChatRoomResponse>, AppError> {
    let session = require_client_chat_session(&state, &headers).await?;
    let room = state
        .store
        .record_client_chat_visit(&room_id, &session.user_id)
        .await?;
    Ok(Json(ClientChatRoomResponse { room }))
}

async fn remove_client_chat_visit(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(room_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let session = require_client_chat_session(&state, &headers).await?;
    state
        .store
        .remove_client_chat_visit(&room_id, &session.user_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn import_chat_visits(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ClientChatVisitImportRequest>,
) -> Result<Json<ClientChatVisitImportResponse>, AppError> {
    let session = require_client_chat_session(&state, &headers).await?;
    let imported = state
        .store
        .import_chat_visits(&session.user_id, input.visits)
        .await?;
    Ok(Json(ClientChatVisitImportResponse { imported }))
}

async fn client_chat_room_stream(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(room_id): Path<String>,
    Query(query): Query<ClientChatStreamQuery>,
) -> Result<
    Sse<
        impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
    >,
    AppError,
> {
    use std::time::Duration;

    let metadata = extract_client_metadata(&headers, addr);
    state
        .store
        .enforce_client_chat_public_read_rate(metadata.ip.as_deref())
        .await?;
    let room_id = room_id.trim().to_string();
    let session = resolve_router_session(&state, &headers).await?;
    let viewer_user_id = session.as_ref().map(|value| value.user_id.clone());
    state
        .store
        .get_chat_room_latest_seq(&room_id, viewer_user_id.as_deref())
        .await?;
    let mut cursor = query.after_seq.unwrap_or(0).max(0);
    let store = state.store.clone();
    let stream = async_stream::stream! {
        yield Ok(axum::response::sse::Event::default().event("ready").data("{}"));
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            match store.get_chat_room_latest_seq(&room_id, viewer_user_id.as_deref()).await {
                Ok(latest_seq) if latest_seq > cursor => {
                    cursor = latest_seq;
                    let payload = serde_json::json!({ "latestSeq": latest_seq }).to_string();
                    yield Ok(axum::response::sse::Event::default().event("update").data(payload));
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    ))
}

async fn list_chat_messages(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(room_id): Path<String>,
    Query(query): Query<ClientChatMessageQuery>,
) -> Result<(HeaderMap, Json<ClientChatMessageListResponse>), AppError> {
    let metadata = extract_client_metadata(&headers, addr);
    state
        .store
        .enforce_client_chat_public_read_rate(metadata.ip.as_deref())
        .await?;
    let session = resolve_router_session(&state, &headers).await?;
    Ok((
        public_chat_headers(),
        Json(
            state
                .store
                .list_chat_messages(
                    &room_id,
                    session.as_ref().map(|session| session.user_id.as_str()),
                    query.before_seq,
                    query.after_seq,
                    query.limit.unwrap_or(50),
                )
                .await?,
        ),
    ))
}

async fn post_client_chat_message(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(room_id): Path<String>,
    Json(input): Json<PostClientChatMessageRequest>,
) -> Result<Json<crate::models::ClientChatMessageView>, AppError> {
    let session = require_client_chat_session(&state, &headers).await?;
    if !NotificationTemplateContext::from_config(&state.config).delivery_configured {
        return Err(AppError::ServiceUnavailable(
            "chat sending is unavailable until Router email delivery is configured".into(),
        ));
    }
    Ok(Json(
        state
            .store
            .create_client_chat_message(&room_id, &session, input.body, input.client_message_id)
            .await?,
    ))
}

async fn mark_client_chat_read(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(room_id): Path<String>,
    Json(input): Json<ClientChatReadRequest>,
) -> Result<Json<ClientChatReadResponse>, AppError> {
    let session = require_client_chat_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .mark_client_chat_read(&room_id, &session.user_id, input.last_read_seq)
            .await?,
    ))
}

async fn admin_delete_client_chat_message(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(message_id): Path<String>,
) -> Result<Json<crate::models::ClientChatMessageView>, AppError> {
    let session = require_admin_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .delete_client_chat_message(&message_id, &session.email)
            .await?,
    ))
}

async fn admin_client_chat_deliveries(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ClientChatDeliveriesResponse>, AppError> {
    require_admin_session(&state, &headers).await?;
    let deliveries = state.store.list_client_chat_deliveries(100).await?;
    Ok(Json(ClientChatDeliveriesResponse { deliveries }))
}

async fn admin_requeue_client_chat_delivery(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(delivery_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin_session(&state, &headers).await?;
    state
        .store
        .requeue_client_chat_delivery(&delivery_id, chrono::Utc::now())
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn admin_settings_get(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<SettingsSnapshotResponse>, AppError> {
    require_admin_session(&state, &headers).await?;
    let settings_guard = state.dynamic.read().await;
    Ok(Json(snapshot_response(
        &state.env_path,
        &state.settings_runtime,
        &settings_guard,
        &state.config,
    )?))
}

async fn admin_client_notification_deliveries(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ClientNotificationDeliveriesResponse>, AppError> {
    require_admin_session(&state, &headers).await?;
    let deliveries = state.store.list_client_notification_deliveries(100).await?;
    Ok(Json(ClientNotificationDeliveriesResponse { deliveries }))
}

async fn admin_settings_validate(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<SettingsUpdateRequest>,
) -> Result<Json<SettingsValidationResponse>, AppError> {
    require_admin_session(&state, &headers).await?;
    let existing = read_env_file(&state.env_path)?;
    ensure_settings_revision(&existing, &input.expected_revision)?;
    let mut validation = validation_response(&existing, &input.updates);
    if !validation.valid {
        return Ok(Json(validation));
    }
    let outcome = validate_and_diff(&existing, &input.updates)?;
    if let Some(release) = changed_client_server_release(&outcome) {
        let release_validation = state
            .client_server_release_validator
            .validate(&release)
            .await
            .map_err(AppError::BadRequest)?;
        if !release_validation.valid {
            validation.valid = false;
            validation.field_errors.insert(
                crate::client_server_release::CLIENT_SERVER_RELEASE_ENV.to_string(),
                vec![release_validation.message],
            );
        }
    }
    Ok(Json(validation))
}

#[derive(Debug, Deserialize)]
struct ClientServerReleaseValidationRequest {
    release: String,
}

async fn admin_client_server_release_validate(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ClientServerReleaseValidationRequest>,
) -> Result<Json<crate::client_server_release::ClientServerReleaseValidation>, AppError> {
    require_admin_session(&state, &headers).await?;
    Ok(Json(
        state
            .client_server_release_validator
            .validate(&input.release)
            .await
            .map_err(AppError::BadRequest)?,
    ))
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

    // 1) Validate the optimistic revision and schema before taking the live
    // settings lock. GitHub validation can involve network I/O and must never
    // stall unrelated dynamic-settings readers.
    let existing = read_env_file(&state.env_path)?;
    ensure_settings_revision(&existing, &input.expected_revision)?;
    let outcome = validate_and_diff(&existing, &input.updates).map_err(|error| {
        let validation = validation_response(&existing, &input.updates);
        AppError::Coded {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "SETTINGS_VALIDATION_FAILED",
            message: error.to_string(),
            details: serde_json::to_value(validation)
                .unwrap_or_else(|_| serde_json::json!({ "valid": false })),
        }
    })?;
    if let Some(release) = changed_client_server_release(&outcome) {
        let release_validation = state
            .client_server_release_validator
            .validate(&release)
            .await
            .map_err(AppError::BadRequest)?;
        ensure_client_server_release_valid(release_validation)?;
    }

    // 2) Serialize apply operations, then confirm no other Settings write won
    // while the external validation was in flight.
    let mut dynamic_guard = state.dynamic.write().await;
    let locked_existing = read_env_file(&state.env_path)?;
    ensure_settings_revision(&locked_existing, &input.expected_revision)?;

    let mut next_dynamic = dynamic_guard.clone();
    apply_updates_to_dynamic(&mut next_dynamic, &input.updates, &state.config);
    let telegram_settings_changed = next_dynamic.telegram_bot != dynamic_guard.telegram_bot;
    let telegram_identity_config_changed = next_dynamic.telegram_bot.enabled
        != dynamic_guard.telegram_bot.enabled
        || next_dynamic.telegram_bot.bot_token != dynamic_guard.telegram_bot.bot_token
        || next_dynamic.telegram_bot.mode != dynamic_guard.telegram_bot.mode
        || next_dynamic.telegram_bot.webhook_secret != dynamic_guard.telegram_bot.webhook_secret;
    // Telegram reachability is runtime state, not configuration validity. The
    // background service performs getMe and retries while the persisted bot
    // state remains reconciling; settings writes must not depend on an external
    // network call while holding the dynamic-settings write lock.
    // Compare the actual runtime policies so re-applying a persisted value can
    // still advance the durable activation boundary after an interrupted sync.
    let needs_client_notification_sync = next_dynamic.client_notifications
        != dynamic_guard.client_notifications
        || telegram_settings_changed;
    let needs_client_notification_validation = needs_client_notification_sync
        || outcome.updated_keys.iter().any(|key| {
            key == "CC_SWITCH_ROUTER_CLIENT_STALE_SECS"
                || key == "CC_SWITCH_ROUTER_CLEANUP_INTERVAL_SECS"
        });
    if needs_client_notification_validation {
        let mut validation_config = state.config.clone();
        if let Some(value) = outcome
            .new_env_kv
            .get("CC_SWITCH_ROUTER_CLIENT_STALE_SECS")
            .and_then(|value| value.parse().ok())
        {
            validation_config.client_stale_secs = value;
        } else if outcome
            .updated_keys
            .iter()
            .any(|key| key == "CC_SWITCH_ROUTER_CLIENT_STALE_SECS")
        {
            validation_config.client_stale_secs = 60 * 60;
        }
        if let Some(value) = outcome
            .new_env_kv
            .get("CC_SWITCH_ROUTER_CLEANUP_INTERVAL_SECS")
            .and_then(|value| value.parse().ok())
        {
            validation_config.cleanup_interval_secs = value;
        } else if outcome
            .updated_keys
            .iter()
            .any(|key| key == "CC_SWITCH_ROUTER_CLEANUP_INTERVAL_SECS")
        {
            validation_config.cleanup_interval_secs = 300;
        }
        validate_notification_cleanup_window(
            &next_dynamic.client_notifications,
            &validation_config,
        )
        .map_err(AppError::BadRequest)?;
    }

    // 3) persist .env atomically (keeps .bak of the prior file).
    write_env_file_atomic(&state.env_path, &outcome.new_env_kv)?;

    // 4) persist the lifecycle-notification activation boundary before
    // publishing the new in-memory settings. The dynamic write lock keeps the
    // worker from observing a half-applied policy.
    if needs_client_notification_sync {
        let (policy, _) = ClientNotificationPolicy::for_runtime(
            &next_dynamic.client_notifications,
            &state.config,
        );
        let mut template = NotificationTemplateContext::from_config(&state.config);
        template.telegram = crate::notifications::TelegramNotificationContext::from_settings(
            &next_dynamic.telegram_bot,
        );
        if let Err(sync_error) = state
            .store
            .sync_client_notification_runtime(&policy, &template, chrono::Utc::now())
            .await
        {
            let rollback_env = existing
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<BTreeMap<_, _>>();
            if let Err(rollback_error) = write_env_file_atomic(&state.env_path, &rollback_env) {
                return Err(AppError::Internal(format!(
                    "sync client notification runtime failed: {sync_error}; env rollback also failed: {rollback_error}"
                )));
            }
            return Err(sync_error);
        }
    }
    if telegram_identity_config_changed {
        let runtime_result = if next_dynamic.telegram_bot.enabled {
            let token = next_dynamic.telegram_bot.token().unwrap_or_default();
            let fingerprint = crate::telegram::bind::telegram_config_fingerprint(
                token,
                next_dynamic.telegram_bot.mode.as_str(),
                next_dynamic.telegram_bot.webhook_secret.as_deref(),
            );
            state
                .store
                .mark_telegram_bot_reconciling(&fingerprint)
                .await
        } else {
            state.store.mark_telegram_bot_disabled().await
        };
        if let Err(runtime_error) = runtime_result {
            let rollback_env = existing
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<BTreeMap<_, _>>();
            if let Err(rollback_error) = write_env_file_atomic(&state.env_path, &rollback_env) {
                return Err(AppError::Internal(format!(
                    "sync Telegram runtime failed: {runtime_error}; env rollback also failed: {rollback_error}"
                )));
            }
            return Err(runtime_error);
        }
    }
    state
        .store
        .set_market_usd_cny_rate_micros(next_dynamic.market_usd_cny_rate_micros);
    *dynamic_guard = next_dynamic.clone();
    drop(dynamic_guard);

    // 5) audit.
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
        revision: settings_revision(
            &outcome
                .new_env_kv
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        ),
    }))
}

fn changed_client_server_release(outcome: &ApplyOutcome) -> Option<String> {
    outcome
        .updated_keys
        .iter()
        .any(|key| key == crate::client_server_release::CLIENT_SERVER_RELEASE_ENV)
        .then(|| {
            outcome
                .new_env_kv
                .get(crate::client_server_release::CLIENT_SERVER_RELEASE_ENV)
                .map(String::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(crate::client_server_release::DEFAULT_CLIENT_SERVER_RELEASE)
                .to_string()
        })
}

fn ensure_client_server_release_valid(
    validation: crate::client_server_release::ClientServerReleaseValidation,
) -> Result<(), AppError> {
    use crate::client_server_release::ClientServerReleaseValidationStatus as Status;

    if validation.valid {
        return Ok(());
    }
    let (status, code) = match validation.status {
        Status::NotFound => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "CLIENT_SERVER_RELEASE_NOT_FOUND",
        ),
        Status::IncompleteAssets => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "CLIENT_SERVER_RELEASE_INCOMPLETE",
        ),
        Status::CommitMismatch => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "CLIENT_SERVER_RELEASE_COMMIT_MISMATCH",
        ),
        Status::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "CLIENT_SERVER_RELEASE_VALIDATION_UNAVAILABLE",
        ),
        Status::Valid => return Ok(()),
    };
    Err(AppError::Coded {
        status,
        code,
        message: validation.message.clone(),
        details: serde_json::to_value(validation)
            .unwrap_or_else(|_| serde_json::json!({ "valid": false })),
    })
}

fn ensure_settings_revision(
    existing: &HashMap<String, String>,
    expected_revision: &str,
) -> Result<(), AppError> {
    let current_revision = settings_revision(existing);
    if expected_revision == current_revision {
        return Ok(());
    }
    Err(AppError::coded_conflict(
        "SETTINGS_REVISION_CONFLICT",
        "settings changed after this page was loaded; reload and review the latest values",
        serde_json::json!({ "currentRevision": current_revision }),
    ))
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
        state
            .metrics
            .snapshot(
                &state.config,
                &state.proxy,
                &state.store,
                &state.alerting,
                &state.clock_health,
            )
            .await?,
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

async fn admin_force_release_share_requests(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<ForceReleaseShareRequestsRequest>,
) -> Result<Json<ForceReleaseShareRequestsResponse>, AppError> {
    let session = require_admin_session(&state, &headers).await?;
    let request_id = input
        .request_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let share_id = input
        .share_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if request_id.is_some() == share_id.is_some() {
        return Err(AppError::BadRequest(
            "provide exactly one of requestId or shareId".into(),
        ));
    }
    let reason = input
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("admin_manual_release");
    if reason.chars().count() > 200 || reason.chars().any(char::is_control) {
        return Err(AppError::BadRequest(
            "reason must be at most 200 printable characters".into(),
        ));
    }

    let released = state
        .proxy
        .force_release_share_requests(request_id, share_id, reason);
    state
        .metrics
        .record_share_request_manual_release(released.len());
    let response = ForceReleaseShareRequestsResponse {
        released_count: released.len(),
        released,
    };
    let payload = serde_json::to_value(&response).unwrap_or_else(
        |_| serde_json::json!({ "releasedCount": response.released_count, "reason": reason }),
    );
    let metadata = extract_client_metadata(&headers, addr);
    if let Err(error) = state
        .store
        .record_admin_audit(
            Some(&session.email),
            "proxy.share_requests.force_release",
            Some(&payload),
            metadata.ip.as_deref(),
        )
        .await
    {
        tracing::warn!(error = %error, "record Share request force-release audit failed");
    }
    Ok(Json(response))
}

async fn admin_alerting_overview(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<crate::metrics::models::MetricsRangeQuery>,
) -> Result<Json<crate::alerting::models::AlertingOverview>, AppError> {
    require_admin_session(&state, &headers).await?;
    Ok(Json(
        state
            .alerting
            .overview(query.limit.unwrap_or(100).clamp(1, 500))
            .await?,
    ))
}

async fn admin_alerting_channels(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Vec<crate::alerting::models::AlertChannelState>>, AppError> {
    require_admin_session(&state, &headers).await?;
    Ok(Json(state.alerting.channel_states().await?))
}

async fn admin_alerting_channel_test(
    State(state): State<ServerState>,
    Path(channel): Path<String>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<Json<crate::alerting::models::AlertChannelTestResponse>, AppError> {
    let session = require_admin_session(&state, &headers).await?;
    let result = state.alerting.test_channel(&channel).await;
    record_notification_admin_audit(
        &state,
        &headers,
        addr,
        &session.email,
        "alerting.channel.test",
        serde_json::json!({ "channel": channel, "ok": result.is_ok() }),
    )
    .await;
    Ok(Json(result?))
}

async fn admin_alerting_incident_acknowledge(
    State(state): State<ServerState>,
    Path(incident_id): Path<String>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<crate::alerting::models::AlertAcknowledgeRequest>,
) -> Result<Json<crate::alerting::models::AlertIncident>, AppError> {
    let session = require_admin_session(&state, &headers).await?;
    let incident = state
        .alerting
        .store()
        .acknowledge(
            incident_id.clone(),
            session.email.clone(),
            input.note,
            Utc::now().timestamp(),
        )
        .await?;
    record_notification_admin_audit(
        &state,
        &headers,
        addr,
        &session.email,
        "alerting.incident.acknowledge",
        serde_json::json!({ "incidentId": incident_id }),
    )
    .await;
    Ok(Json(incident))
}

async fn admin_alerting_incident_silence(
    State(state): State<ServerState>,
    Path(incident_id): Path<String>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<crate::alerting::models::AlertSilenceRequest>,
) -> Result<Json<crate::alerting::models::AlertIncident>, AppError> {
    let session = require_admin_session(&state, &headers).await?;
    let incident = state
        .alerting
        .store()
        .silence(
            incident_id.clone(),
            session.email.clone(),
            input.note,
            Utc::now().timestamp(),
            input.duration_secs,
        )
        .await?;
    record_notification_admin_audit(
        &state,
        &headers,
        addr,
        &session.email,
        "alerting.incident.silence",
        serde_json::json!({
            "incidentId": incident_id,
            "durationSecs": input.duration_secs,
        }),
    )
    .await;
    Ok(Json(incident))
}

async fn admin_alerting_incident_resume(
    State(state): State<ServerState>,
    Path(incident_id): Path<String>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<crate::alerting::models::AlertAcknowledgeRequest>,
) -> Result<Json<crate::alerting::models::AlertIncident>, AppError> {
    let session = require_admin_session(&state, &headers).await?;
    let policy = state.alerting.current_delivery_policy().await;
    let incident = state
        .alerting
        .store()
        .resume(
            incident_id.clone(),
            session.email.clone(),
            input.note,
            Utc::now().timestamp(),
            policy,
        )
        .await?;
    record_notification_admin_audit(
        &state,
        &headers,
        addr,
        &session.email,
        "alerting.incident.resume",
        serde_json::json!({ "incidentId": incident_id }),
    )
    .await;
    Ok(Json(incident))
}

async fn admin_alerting_delivery_retry(
    State(state): State<ServerState>,
    Path(delivery_id): Path<String>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_admin_session(&state, &headers).await?;
    state
        .alerting
        .store()
        .retry_delivery(delivery_id.clone(), Utc::now().timestamp())
        .await?;
    record_notification_admin_audit(
        &state,
        &headers,
        addr,
        &session.email,
        "alerting.delivery.retry",
        serde_json::json!({ "deliveryId": delivery_id }),
    )
    .await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn admin_user_notification_channels(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Vec<crate::user_notification_health::UserNotificationChannelState>>, AppError> {
    let session = require_admin_session(&state, &headers).await?;
    let settings = state.dynamic.read().await.telegram_bot.clone();
    Ok(Json(
        crate::user_notification_health::channel_states(&state.store, &settings, &session.email)
            .await?,
    ))
}

async fn admin_user_notification_channel_test(
    State(state): State<ServerState>,
    Path(channel): Path<String>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<Json<crate::user_notification_health::UserNotificationChannelTestResponse>, AppError> {
    let session = require_admin_session(&state, &headers).await?;
    let settings = state.dynamic.read().await.telegram_bot.clone();
    let scheme = if state.config.use_localhost {
        "http"
    } else {
        "https"
    };
    let dashboard_url = format!(
        "{scheme}://{}",
        state.config.tunnel_domain.trim_end_matches('/')
    );
    let result = crate::user_notification_health::test_channel(
        &state.store,
        &settings,
        &session.email,
        &channel,
        &dashboard_url,
    )
    .await;
    record_notification_admin_audit(
        &state,
        &headers,
        addr,
        &session.email,
        "user_notifications.channel.test",
        serde_json::json!({ "channel": channel, "ok": result.is_ok() }),
    )
    .await;
    Ok(Json(result?))
}

async fn record_notification_admin_audit(
    state: &ServerState,
    headers: &HeaderMap,
    addr: SocketAddr,
    actor_email: &str,
    action: &str,
    payload: serde_json::Value,
) {
    let metadata = extract_client_metadata(headers, addr);
    let _ = state
        .store
        .record_admin_audit(
            Some(actor_email),
            action,
            Some(&payload),
            metadata.ip.as_deref(),
        )
        .await;
}

// ─────────────────────────────────────────────────────────────────────────────
// P18: test-connection — dashboard 通过 Share 的 subdomain 和调用者自己的
// API token 执行 Server-authoritative modelProbe，并把原始 HTTP 响应回传。
// 后端中转是因为 Share subdomain 不同源，浏览器不能直接调用。
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareConnectionTestRequest {
    /// "claude" | "codex" | "gemini"
    app: String,
    /// Legacy callers may still send "text"; all other old probe kinds are retired.
    #[serde(default)]
    kind: Option<String>,
    /// text | image_generation | image_edit | video_generation
    #[serde(default)]
    operation: Option<String>,
    /// 可选，毫秒；默认 15000，上限 30000
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareConnectionTestResponse {
    success: bool,
    request: TestRequestEcho,
    response: Option<TestResponseEcho>,
    duration_ms: u64,
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_event: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scheduling_recovery: Option<crate::store::ShareSchedulingRecovery>,
}

enum ConnectionTestBody {
    Json(String),
    ImageEdit(Vec<u8>),
}

struct PreparedConnectionTest {
    method: String,
    path: String,
    echo_body: String,
    body: ConnectionTestBody,
    response_mode: ProbeResponseMode,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareUsageRefreshRequest {
    app: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareUsageRefreshResponse {
    ok: bool,
    refreshed: Vec<crate::ctl_client::RefreshShareUsageItem>,
}

#[derive(Debug, Deserialize)]
struct ImageGenerationRequestLogsQuery {
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareRequestLogsQuery {
    #[serde(default)]
    app: Option<String>,
    #[serde(default, rename = "requestKind", alias = "request_kind")]
    request_kind: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareRequestLogsResponse {
    logs: Vec<ShareRequestLogEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
    has_more: bool,
}

#[derive(Debug, Deserialize)]
struct ImageGenerationResultQuery {
    token: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageGenerationRequestLogsResponse {
    logs: Vec<ImageGenerationRequestLogEntry>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageGenerationJobsCompatResponse {
    jobs: Vec<ImageGenerationJobCompatEntry>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageGenerationJobCompatEntry {
    job_id: String,
    share_id: String,
    share_name: String,
    installation_id: String,
    provider_id: String,
    provider_name: String,
    app_type: String,
    model: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status_code: Option<u16>,
    latency_ms: u64,
    queued_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result_mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result_size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_by_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_country: Option<String>,
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
const TEST_JSON_PARSE_CAP: usize = 1024 * 1024;
const TEST_IMAGE_JSON_PARSE_CAP: usize = 16 * 1024 * 1024;
const TEST_SSE_LINE_SCAN_CAP: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeResponseMode {
    Json,
    ImageJson,
    AnthropicSse,
    ResponsesSse,
    GeminiSse,
    ImageSse,
}

fn effective_probe_response_mode(
    mode: ProbeResponseMode,
    content_type: Option<&str>,
) -> ProbeResponseMode {
    if mode == ProbeResponseMode::ImageSse
        && !content_type.is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
        })
    {
        ProbeResponseMode::ImageJson
    } else {
        mode
    }
}

fn probe_response_mode(value: &str) -> Result<ProbeResponseMode, AppError> {
    match value {
        "json" => Ok(ProbeResponseMode::Json),
        "anthropic_sse" => Ok(ProbeResponseMode::AnthropicSse),
        "responses_sse" => Ok(ProbeResponseMode::ResponsesSse),
        "gemini_sse" => Ok(ProbeResponseMode::GeminiSse),
        _ => Err(AppError::Conflict(
            "Share modelProbe response mode is not supported; upgrade or resync cc-switch-server"
                .into(),
        )),
    }
}

fn share_model_probe_for_app<'a>(
    share: &'a ShareForTest,
    app: &str,
) -> Option<&'a crate::models::ProviderModelProbe> {
    match app {
        "claude" => share.app_runtimes.claude.as_ref(),
        "codex" => share.app_runtimes.codex.as_ref(),
        "gemini" => share.app_runtimes.gemini.as_ref(),
        _ => None,
    }
    .and_then(|runtime| runtime.model_probe.as_ref())
}

fn share_model_probe_app_enabled(share: &ShareForTest, app: &str) -> bool {
    match app {
        "claude" => share.support.claude,
        "codex" => share.support.codex,
        "gemini" => share.support.gemini,
        _ => false,
    }
}

struct ProbeBodyRead {
    preview: Vec<u8>,
    total_bytes: usize,
    terminal_event: Option<String>,
    error: Option<String>,
}

struct ProbeSseTracker {
    mode: ProbeResponseMode,
    line: Vec<u8>,
    line_truncated: bool,
    success_event: Option<String>,
    failure_event: Option<String>,
    saw_done: bool,
}

impl ProbeSseTracker {
    fn new(mode: ProbeResponseMode) -> Self {
        Self {
            mode,
            line: Vec::new(),
            line_truncated: false,
            success_event: None,
            failure_event: None,
            saw_done: false,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        for byte in bytes {
            if *byte == b'\n' {
                self.finish_line();
            } else if self.line.len() < TEST_SSE_LINE_SCAN_CAP {
                self.line.push(*byte);
            } else {
                self.line_truncated = true;
            }
        }
    }

    fn finish(mut self) -> (Option<String>, Option<String>) {
        if !self.line.is_empty() || self.line_truncated {
            self.finish_line();
        }
        if let Some(event) = self.failure_event {
            return (
                Some(event.clone()),
                Some(format!("stream ended with failure event {event}")),
            );
        }
        if let Some(event) = self.success_event {
            return (Some(event), None);
        }
        let expected = match self.mode {
            ProbeResponseMode::AnthropicSse => "message_stop",
            ProbeResponseMode::ResponsesSse => "response.completed",
            ProbeResponseMode::GeminiSse => "a Gemini finishReason",
            ProbeResponseMode::ImageSse => "an image generation terminal event",
            ProbeResponseMode::Json | ProbeResponseMode::ImageJson => "JSON response",
        };
        let message = if self.saw_done {
            format!("stream emitted [DONE] before required terminal event {expected}")
        } else {
            format!("stream ended before required terminal event {expected}")
        };
        (None, Some(message))
    }

    fn finish_line(&mut self) {
        if self.line.last() == Some(&b'\r') {
            self.line.pop();
        }
        let line = std::mem::take(&mut self.line);
        let truncated = std::mem::replace(&mut self.line_truncated, false);
        if let Some(event) = line.strip_prefix(b"event:") {
            self.observe_event(String::from_utf8_lossy(event).trim());
            return;
        }
        let Some(data) = line.strip_prefix(b"data:") else {
            return;
        };
        let data = trim_ascii(data);
        if data == b"[DONE]" {
            self.saw_done = true;
            return;
        }
        if self.mode == ProbeResponseMode::GeminiSse
            && let Some(event) = gemini_sse_event(data, truncated)
        {
            self.observe_event(&event);
            return;
        }
        if let Some(event) = sse_json_event_type(data, truncated) {
            self.observe_event(&event);
        }
    }

    fn observe_event(&mut self, event: &str) {
        let event = event.trim();
        let success = match self.mode {
            ProbeResponseMode::AnthropicSse => event == "message_stop",
            ProbeResponseMode::ResponsesSse => event == "response.completed",
            ProbeResponseMode::GeminiSse => event == "gemini.completed",
            ProbeResponseMode::ImageSse => matches!(
                event,
                "image_generation.completed" | "image_edit.completed" | "response.completed"
            ),
            ProbeResponseMode::Json | ProbeResponseMode::ImageJson => false,
        };
        let failure = match self.mode {
            ProbeResponseMode::AnthropicSse => event == "error",
            ProbeResponseMode::ResponsesSse => matches!(
                event,
                "response.failed"
                    | "response.incomplete"
                    | "response.cancelled"
                    | "response.canceled"
                    | "error"
            ),
            ProbeResponseMode::GeminiSse => event == "error",
            ProbeResponseMode::ImageSse => matches!(
                event,
                "image_generation.failed" | "image_edit.failed" | "response.failed" | "error"
            ),
            ProbeResponseMode::Json | ProbeResponseMode::ImageJson => false,
        };
        if failure && self.failure_event.is_none() {
            self.failure_event = Some(event.to_string());
        } else if success && self.success_event.is_none() {
            self.success_event = Some(event.to_string());
        }
    }
}

fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

fn sse_json_event_type(data: &[u8], truncated: bool) -> Option<String> {
    if !truncated
        && let Ok(value) = serde_json::from_slice::<serde_json::Value>(data)
        && let Some(event) = value.get("type").and_then(serde_json::Value::as_str)
    {
        return Some(event.to_string());
    }
    let prefix = &data[..data.len().min(512)];
    let key = b"\"type\"";
    let start = prefix.windows(key.len()).position(|window| window == key)? + key.len();
    let mut remainder = trim_ascii(&prefix[start..]);
    remainder = remainder.strip_prefix(b":")?;
    remainder = trim_ascii(remainder);
    remainder = remainder.strip_prefix(b"\"")?;
    let end = remainder.iter().position(|byte| *byte == b'\"')?;
    std::str::from_utf8(&remainder[..end])
        .ok()
        .map(str::to_string)
}

fn gemini_sse_event(data: &[u8], truncated: bool) -> Option<String> {
    if truncated {
        return None;
    }
    let value = serde_json::from_slice::<serde_json::Value>(data).ok()?;
    if value.get("error").is_some_and(|error| !error.is_null()) {
        return Some("error".to_string());
    }
    value
        .get("candidates")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|candidates| {
            candidates.iter().any(|candidate| {
                candidate
                    .get("finishReason")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|reason| !reason.trim().is_empty())
            })
        })
        .then(|| "gemini.completed".to_string())
}

async fn read_probe_body(resp: reqwest::Response, mode: ProbeResponseMode) -> ProbeBodyRead {
    let mut stream = resp.bytes_stream();
    let mut preview = Vec::new();
    let mut total_bytes = 0_usize;
    let mut json_body = Vec::new();
    let mut json_too_large = false;
    let json_parse_cap = if mode == ProbeResponseMode::ImageJson {
        TEST_IMAGE_JSON_PARSE_CAP
    } else {
        TEST_JSON_PARSE_CAP
    };
    let mut sse_tracker = (!matches!(mode, ProbeResponseMode::Json | ProbeResponseMode::ImageJson))
        .then(|| ProbeSseTracker::new(mode));

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                return ProbeBodyRead {
                    preview,
                    total_bytes,
                    terminal_event: None,
                    error: Some(format!("response body read failed: {error}")),
                };
            }
        };
        total_bytes = total_bytes.saturating_add(chunk.len());
        if preview.len() < TEST_BODY_CAP {
            let remaining = TEST_BODY_CAP - preview.len();
            preview.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        }
        if let Some(tracker) = sse_tracker.as_mut() {
            tracker.push(&chunk);
        } else if json_body.len().saturating_add(chunk.len()) <= json_parse_cap {
            json_body.extend_from_slice(&chunk);
        } else {
            json_too_large = true;
        }
    }

    if let Some(tracker) = sse_tracker {
        let (terminal_event, error) = tracker.finish();
        return ProbeBodyRead {
            preview,
            total_bytes,
            terminal_event,
            error,
        };
    }
    if json_too_large {
        return ProbeBodyRead {
            preview,
            total_bytes,
            terminal_event: None,
            error: Some(format!(
                "JSON response exceeds the {json_parse_cap} byte validation limit"
            )),
        };
    }
    let parsed = serde_json::from_slice::<serde_json::Value>(&json_body);
    let error = match parsed {
        Ok(value) if !value.get("error").is_some_and(|error| !error.is_null()) => None,
        Ok(_) => Some("JSON response contains an error object".to_string()),
        Err(error) => Some(format!("response body is not valid JSON: {error}")),
    };
    ProbeBodyRead {
        preview,
        total_bytes,
        terminal_event: error.is_none().then(|| {
            if mode == ProbeResponseMode::ImageJson {
                "image_json.completed".to_string()
            } else {
                "json.completed".to_string()
            }
        }),
        error,
    }
}

async fn refresh_share_usage(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
    Json(input): Json<ShareUsageRefreshRequest>,
) -> Result<Json<ShareUsageRefreshResponse>, AppError> {
    let current_user_email = require_user_email(&state, &headers, "share:read").await?;
    let share = state
        .store
        .get_share_for_test(&share_id)
        .await?
        .ok_or_else(|| AppError::NotFound("share not found".into()))?;

    let is_admin = state.dynamic.read().await.is_admin(&current_user_email);
    let is_owner = share.owner_email.eq_ignore_ascii_case(&current_user_email);
    if !is_admin && !is_owner {
        return Err(AppError::Forbidden(
            "only the share owner or admins can refresh this share usage".into(),
        ));
    }

    let app = input
        .app
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(app) = app {
        if !matches!(app, "claude" | "codex" | "gemini") {
            return Err(AppError::BadRequest(format!(
                "unsupported app for usage refresh: {app}"
            )));
        }
    }

    let route = state
        .proxy
        .route_by_share_id(&share_id)
        .await
        .ok_or_else(|| AppError::UnprocessableEntity("share client is offline".into()))?;
    let installation_id = route
        .installation_id()
        .ok_or_else(|| AppError::UnprocessableEntity("share installation is unavailable".into()))?;
    let control_secret = state
        .store
        .installation_control_secret(installation_id)
        .await?
        .ok_or_else(|| {
            AppError::UnprocessableEntity("share control secret is unavailable".into())
        })?;

    let reply = crate::ctl_client::refresh_share_usage(
        route.route_target(),
        installation_id,
        &control_secret,
        &share_id,
        app,
    )
    .await
    .map_err(|err| AppError::UnprocessableEntity(err.to_string()))?;

    Ok(Json(ShareUsageRefreshResponse {
        ok: true,
        refreshed: reply.refreshed,
    }))
}

async fn list_share_image_generation_request_logs(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
    Query(query): Query<ImageGenerationRequestLogsQuery>,
) -> Result<Json<ImageGenerationRequestLogsResponse>, AppError> {
    let view = image_request_log_view_context(&state, &headers, &share_id).await?;

    let mut logs = state
        .store
        .list_image_generation_request_logs_for_share(&share_id, query.limit.unwrap_or(10).min(10))
        .await?;
    apply_image_generation_log_visibility(&mut logs, &view);
    Ok(Json(ImageGenerationRequestLogsResponse { logs }))
}

async fn list_share_request_logs(
    State(state): State<ServerState>,
    Path(share_id): Path<String>,
    Query(query): Query<ShareRequestLogsQuery>,
) -> Result<Json<ShareRequestLogsResponse>, AppError> {
    state
        .store
        .get_share_for_test(&share_id)
        .await?
        .ok_or_else(|| AppError::NotFound("share not found".into()))?;
    let mut page = state
        .store
        .list_share_request_logs_page(
            &share_id,
            query.app.as_deref(),
            query.request_kind.as_deref(),
            None,
            query.cursor.as_deref(),
            query.limit.unwrap_or(50),
        )
        .await?;
    remove_share_request_session_ids(&mut page.logs);
    Ok(Json(ShareRequestLogsResponse {
        logs: page.logs,
        next_cursor: page.next_cursor,
        has_more: page.has_more,
    }))
}

async fn list_share_image_generation_jobs_compat(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
    Query(query): Query<ImageGenerationRequestLogsQuery>,
) -> Result<Json<ImageGenerationJobsCompatResponse>, AppError> {
    let view = image_request_log_view_context(&state, &headers, &share_id).await?;

    let mut logs = state
        .store
        .list_image_generation_request_logs_for_share(&share_id, query.limit.unwrap_or(10).min(10))
        .await?;
    apply_image_generation_log_visibility(&mut logs, &view);
    let jobs = logs
        .into_iter()
        .map(|log| ImageGenerationJobCompatEntry {
            job_id: log.request_id,
            share_id: log.share_id,
            share_name: log.share_name,
            installation_id: log.installation_id,
            provider_id: log.provider_id,
            provider_name: log.provider_name,
            app_type: log.app_type,
            model: log.model,
            status: log.status,
            status_code: log.status_code,
            latency_ms: log.latency_ms,
            queued_at: log.created_at,
            completed_at: log.completed_at,
            prompt_preview: log.prompt_preview,
            error_message: log.error_message,
            result_mime_type: log.result_mime_type,
            result_size_bytes: log.result_size_bytes,
            created_by_email: log.created_by_email,
            user_country: log.user_country,
        })
        .collect();
    Ok(Json(ImageGenerationJobsCompatResponse { jobs }))
}

struct ImageRequestLogViewContext {
    can_view_prompt: bool,
    can_view_result_url: bool,
}

async fn image_request_log_view_context(
    state: &ServerState,
    headers: &HeaderMap,
    share_id: &str,
) -> Result<ImageRequestLogViewContext, AppError> {
    let current_user_email = extract_session_email(state, headers).await?;
    let share = state
        .store
        .get_share_for_test(share_id)
        .await?
        .ok_or_else(|| AppError::NotFound("share not found".into()))?;

    let is_owner = current_user_email
        .as_deref()
        .map(|email| share.owner_email.eq_ignore_ascii_case(email))
        .unwrap_or(false);
    Ok(ImageRequestLogViewContext {
        can_view_prompt: is_owner,
        can_view_result_url: is_owner,
    })
}

fn apply_image_generation_log_visibility(
    logs: &mut [ImageGenerationRequestLogEntry],
    view: &ImageRequestLogViewContext,
) {
    for log in logs {
        if !view.can_view_prompt {
            log.prompt_preview = None;
        }
        if view.can_view_result_url {
            if let (Some(_storage_key), Some(token)) = (
                log.result_storage_key.as_deref(),
                log.result_access_token.as_deref(),
            ) {
                log.result_url = Some(format!(
                    "/v1/image-results/{}?token={}",
                    log.request_id, token
                ));
            }
        } else {
            log.result_url = None;
        }
    }
}

async fn get_image_generation_result(
    State(state): State<ServerState>,
    Path(request_id): Path<String>,
    Query(query): Query<ImageGenerationResultQuery>,
) -> Result<Response, AppError> {
    let Some(token) = query
        .token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Err(AppError::NotFound("image result not found".into()));
    };
    let Some(access) = state
        .store
        .get_image_generation_result_for_access(&request_id, token)
        .await?
    else {
        return Err(AppError::NotFound("image result not found".into()));
    };
    let Some(path) = image_result_path(&state.config, &access.storage_key) else {
        return Err(AppError::NotFound("image result not found".into()));
    };
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| AppError::NotFound("image result not found".into()))?;
    let content_type = access
        .mime_type
        .as_deref()
        .filter(|value| value.starts_with("image/"))
        .unwrap_or("application/octet-stream");
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "private, max-age=3600")
        .body(Body::from(bytes))
        .map_err(|e| AppError::Internal(format!("build image result response failed: {e}")))
}

async fn get_share_model_health_calendar(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
    Query(query): Query<ShareModelHealthCalendarQuery>,
) -> Result<Json<ShareModelHealthCalendarResponse>, AppError> {
    let viewer_email = extract_session_email(&state, &headers).await?;
    let is_admin = {
        let dynamic = state.dynamic.read().await;
        viewer_email
            .as_deref()
            .is_some_and(|email| dynamic.is_admin(email))
    };
    if !state
        .store
        .can_view_share_model_health_calendar(&share_id, viewer_email.as_deref(), is_admin)
        .await?
    {
        return Err(AppError::NotFound(
            "Share model health calendar not found".into(),
        ));
    }
    let calendar = state
        .store
        .share_model_health_calendar(&share_id, query.days.unwrap_or(365), Utc::now())
        .await?
        .ok_or_else(|| AppError::NotFound("Share model health calendar not found".into()))?;
    Ok(Json(calendar))
}

async fn get_client_online_calendar(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(installation_id): Path<String>,
    Query(query): Query<ClientOnlineCalendarQuery>,
) -> Result<Json<ClientOnlineCalendarResponse>, AppError> {
    let viewer_email = extract_session_email(&state, &headers).await?;
    let is_admin = {
        let dynamic = state.dynamic.read().await;
        viewer_email
            .as_deref()
            .is_some_and(|email| dynamic.is_admin(email))
    };
    if !state
        .store
        .can_view_client_online_calendar(&installation_id, viewer_email.as_deref(), is_admin)
        .await?
    {
        return Err(AppError::NotFound(
            "Client online calendar not found".into(),
        ));
    }
    let calendar = state
        .store
        .client_online_calendar(&installation_id, query.days.unwrap_or(365), Utc::now())
        .await?
        .ok_or_else(|| AppError::NotFound("Client online calendar not found".into()))?;
    Ok(Json(calendar))
}

async fn test_share_connection(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
    Json(input): Json<ShareConnectionTestRequest>,
) -> Result<Json<ShareConnectionTestResponse>, AppError> {
    let current_user_email = require_user_email(&state, &headers, "share:read").await?;
    let app = input.app.trim().to_ascii_lowercase();
    if !matches!(app.as_str(), "claude" | "codex" | "gemini") {
        return Err(AppError::BadRequest("unsupported Share App".into()));
    }
    let operation = input
        .operation
        .as_deref()
        .or(input.kind.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("text")
        .to_ascii_lowercase();
    if !matches!(
        operation.as_str(),
        "text" | "image_generation" | "image_edit" | "video_generation"
    ) {
        return Err(AppError::BadRequest(
            "unsupported Share connection test operation".into(),
        ));
    }

    let share = state
        .store
        .get_share_for_test(&share_id)
        .await?
        .ok_or_else(|| AppError::NotFound("share not found".into()))?;

    let is_admin = state.dynamic.read().await.is_admin(&current_user_email);
    let is_owner = share.owner_email.eq_ignore_ascii_case(&current_user_email);
    let is_shared_with = share.has_active_shareto(&current_user_email);

    if !is_admin && !is_owner && !is_shared_with {
        return Err(AppError::Forbidden(
            "only the share owner, invited users, or admins can test this share".into(),
        ));
    }
    if !share.bindings.contains_key(&app) {
        return Err(AppError::BadRequest(format!(
            "share does not have a {app} binding"
        )));
    }
    if !share_model_probe_app_enabled(&share, &app) {
        return Err(AppError::BadRequest(format!(
            "share does not enable the {app} API"
        )));
    }
    let prepared = if operation == "text" {
        let probe = share_model_probe_for_app(&share, &app)
            .cloned()
            .ok_or_else(|| {
                AppError::Conflict(format!(
                    "Share Contract modelProbe is unavailable (contract version {}); upgrade or resync cc-switch-server",
                    share.contract_version
                ))
            })?;
        let expected_api_type = match app.as_str() {
            "claude" => "anthropic",
            "codex" => "openai",
            "gemini" => "gemini",
            _ => unreachable!("Share App was validated above"),
        };
        if probe.api_type != expected_api_type
            || crate::store::validate_provider_model_probe(&app, &probe).is_err()
        {
            return Err(AppError::Conflict(
                "Share modelProbe is incompatible; upgrade or resync cc-switch-server".into(),
            ));
        }
        let body = serde_json::to_string(&probe.body).map_err(|error| {
            AppError::Internal(format!("encode Share modelProbe body failed: {error}"))
        })?;
        PreparedConnectionTest {
            method: probe.method,
            path: probe.path,
            echo_body: body.clone(),
            body: ConnectionTestBody::Json(body),
            response_mode: probe_response_mode(&probe.response_mode)?,
        }
    } else {
        if app != "codex" {
            return Err(AppError::BadRequest(
                "Grok media connection tests require the codex Share binding".into(),
            ));
        }
        let enabled = match operation.as_str() {
            "image_generation" => share.grok_media_policy.image_generation_enabled,
            "image_edit" => share.grok_media_policy.image_edit_enabled,
            "video_generation" => share.grok_media_policy.video_generation_enabled,
            _ => false,
        };
        if !enabled {
            return Err(AppError::Forbidden(format!(
                "{operation} is disabled by the Share Grok media policy"
            )));
        }
        match operation.as_str() {
            "image_generation" => {
                let body = serde_json::json!({
                    "model": "grok-imagine",
                    "prompt": "A small blue circle on a plain white background",
                    "n": 1,
                    "response_format": "b64_json"
                })
                .to_string();
                PreparedConnectionTest {
                    method: "POST".into(),
                    path: "/v1/images/generations".into(),
                    echo_body: body.clone(),
                    body: ConnectionTestBody::Json(body),
                    response_mode: ProbeResponseMode::ImageSse,
                }
            }
            "image_edit" => {
                const TEST_PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
                let image = base64::engine::general_purpose::STANDARD
                    .decode(TEST_PNG_BASE64)
                    .map_err(|error| {
                        AppError::Internal(format!("decode test image failed: {error}"))
                    })?;
                PreparedConnectionTest {
                    method: "POST".into(),
                    path: "/v1/images/edits".into(),
                    echo_body: "<multipart: model=grok-imagine, prompt, image=test.png>".into(),
                    body: ConnectionTestBody::ImageEdit(image),
                    response_mode: ProbeResponseMode::ImageSse,
                }
            }
            "video_generation" => {
                let body = serde_json::json!({
                    "model": "grok-imagine-video",
                    "prompt": "A blue circle slowly moving from left to right",
                    "duration": 6,
                    "resolution": "720p",
                    "aspect_ratio": "16:9"
                })
                .to_string();
                PreparedConnectionTest {
                    method: "POST".into(),
                    path: "/v1/videos/generations".into(),
                    echo_body: body.clone(),
                    body: ConnectionTestBody::Json(body),
                    response_mode: ProbeResponseMode::Json,
                }
            }
            _ => unreachable!("operation was validated"),
        }
    };
    let response_mode = prepared.response_mode;
    let subdomain = share.subdomain.clone();

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
    let public_url = format!("{}{}", state.config.tunnel_url(&subdomain), prepared.path);
    let local_url = format!("http://{}{}", state.config.api_addr, prepared.path);
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
        [
            "Content-Type".to_string(),
            if matches!(&prepared.body, ConnectionTestBody::Json(_)) {
                "application/json"
            } else {
                "multipart/form-data"
            }
            .to_string(),
        ],
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
        method: prepared.method.clone(),
        url: public_url.clone(),
        headers: echo_headers,
        body: Some(prepared.echo_body.clone()),
    };

    let started = std::time::Instant::now();
    let request = client
        .post(&local_url)
        .header("Host", &public_host)
        .bearer_auth(&api_token)
        .header("x-cc-switch-dashboard-test", "1");
    let mut request = match prepared.body {
        ConnectionTestBody::Json(body) => request
            .header("Content-Type", "application/json")
            .body(body),
        ConnectionTestBody::ImageEdit(image) => request.multipart(
            reqwest::multipart::Form::new()
                .text("model", "grok-imagine")
                .text("prompt", "Make the dot green")
                .part(
                    "image",
                    reqwest::multipart::Part::bytes(image)
                        .file_name("test.png")
                        .mime_str("image/png")
                        .map_err(|error| {
                            AppError::Internal(format!("build test image part failed: {error}"))
                        })?,
                ),
        ),
    };
    if response_mode != ProbeResponseMode::Json {
        request = request.header("Accept", "text/event-stream");
    }
    let result = request.send().await;
    match result {
        Err(err) => {
            let duration_ms = started.elapsed().as_millis() as u64;
            tracing::info!(
                tag = "test-connection",
                share_id = %share_id,
                app = %app,
                error = %err,
                duration_ms,
                "test-connection network error"
            );
            Ok(Json(ShareConnectionTestResponse {
                success: false,
                request: request_echo,
                response: None,
                duration_ms,
                error: Some(err.to_string()),
                terminal_event: None,
                scheduling_recovery: None,
            }))
        }
        Ok(resp) => {
            let status_code = resp.status().as_u16();
            let status_text = resp.status().canonical_reason().unwrap_or("").to_string();
            let response_mode = effective_probe_response_mode(
                response_mode,
                resp.headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok()),
            );
            let resp_headers: Vec<[String; 2]> = resp
                .headers()
                .iter()
                .map(|(k, v)| [k.as_str().to_string(), v.to_str().unwrap_or("").to_string()])
                .collect();
            let body = read_probe_body(resp, response_mode).await;
            let duration_ms = started.elapsed().as_millis() as u64;
            let body_truncated = body.total_bytes > body.preview.len();
            let body_text = String::from_utf8_lossy(&body.preview).into_owned();
            let success = (200..300).contains(&status_code) && body.error.is_none();

            tracing::info!(
                tag = "test-connection",
                share_id = %share_id,
                app = %app,
                status = status_code,
                duration_ms,
                success,
                terminal_event = body.terminal_event.as_deref().unwrap_or("-"),
                semantic_error = body.error.as_deref().unwrap_or("-"),
                "test-connection completed"
            );
            let scheduling_recovery = if success && operation == "text" {
                let recovery = state
                    .store
                    .recover_share_app_scheduling_after_successful_test(&share_id, &app)
                    .await?;
                if recovery.changed() {
                    tracing::info!(
                        tag = "test-connection",
                        share_id = %share_id,
                        app = %app,
                        share_model_health_deleted = recovery.share_model_health_deleted,
                        gateway_model_failures_deleted = recovery.gateway_model_failures_deleted,
                        gateway_runtime_states_deleted = recovery.gateway_runtime_states_deleted,
                        "test-connection recovered share app scheduling state"
                    );
                    Some(recovery)
                } else {
                    None
                }
            } else {
                None
            };
            Ok(Json(ShareConnectionTestResponse {
                success,
                request: request_echo,
                response: Some(TestResponseEcho {
                    status_code,
                    status_text,
                    headers: resp_headers,
                    body_text,
                    body_truncated,
                }),
                duration_ms,
                error: body.error,
                terminal_event: body.terminal_event,
                scheduling_recovery,
            }))
        }
    }
}
