//! Private Share value-quota control plane. Never mounted on public usage APIs.
use crate::{ServerState, error::AppError, store::share_value_quotas::SignedValueRequest};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, HeaderValue, header},
    middleware,
    response::Response,
    routing::{get, post, put},
};
use serde::Deserialize;

/// Signed Server control payloads (fetch/ack/watermark) and browser control
/// bodies are small; tariffs only travel Router → Server, in fetch responses.
const CONTROL_BODY_LIMIT: usize = 256 * 1024;

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/admin/share-value-pricing/health", get(health))
        .route("/v1/shares/value-pricing/fetch", post(fetch))
        .route("/v1/shares/value-pricing/ack", post(ack))
        .route("/v1/shares/value-pricing/active", post(active))
        .route(
            "/v1/shares/:share_id/value-quota-watermarks",
            post(watermark),
        )
        .route("/v1/shares/value-quota-watermarks", post(watermarks))
        .route("/v1/shares/:share_id/value-quota-status", get(status))
        .route("/v1/me/share-value-quotas", get(mine))
        .route("/v1/shares/:share_id/value-quota-preview", post(preview))
        .route(
            "/v1/shares/:share_id/capability-preview",
            post(capability_preview),
        )
        .route(
            "/v1/shares/:share_id/request-metering",
            post(request_metering),
        )
        .route(
            "/v1/shares/:share_id/quota-history-prepare",
            post(prepare_history),
        )
        .route("/v1/shares/:share_id/value-usage-edits", post(edit_usage))
        .route(
            "/v1/shares/:share_id/market-value-quota-readiness",
            get(market_readiness),
        )
        .route("/v1/shares/:share_id/model-prices", get(list_model_prices))
        // A sibling path, so every model key (even `suggestions`) is priceable.
        .route(
            "/v1/shares/:share_id/model-price-suggestions",
            get(model_price_suggestions),
        )
        .route(
            "/v1/shares/:share_id/model-prices/:model_key",
            put(upsert_model_price).delete(delete_model_price),
        )
        .layer(DefaultBodyLimit::max(CONTROL_BODY_LIMIT))
        // Every response here, including errors and extractor rejections, is private.
        .layer(middleware::map_response(no_store))
}
async fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}
/// Share Owner or Router Owner, shared by preview, usage edits and status.
async fn require_share_manager(
    state: &ServerState,
    headers: &HeaderMap,
    share_id: &str,
) -> Result<crate::models::AuthSession, AppError> {
    let session = crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))?;
    let owner = state
        .store
        .lookup_share_owner_email(share_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Share not found".into()))?;
    if !owner.eq_ignore_ascii_case(&session.email)
        && !crate::api::is_finance_owner(&state.config, &session.email)
    {
        return Err(AppError::Forbidden(
            "Share owner or Router owner required".into(),
        ));
    }
    Ok(session)
}
/// Whether a signed-in user manages the Share (same rule as
/// [`require_share_manager`]); a missing Share is a 404.
pub(crate) async fn session_manages_share(
    state: &ServerState,
    session: &crate::models::AuthSession,
    share_id: &str,
) -> Result<bool, AppError> {
    let owner = state
        .store
        .lookup_share_owner_email(share_id)
        .await?
        .ok_or_else(crate::store::share_value_quotas::owner_prices::share_not_found)?;
    Ok(owner.eq_ignore_ascii_case(&session.email)
        || crate::api::is_finance_owner(&state.config, &session.email))
}
/// [`require_share_manager`] with the owner price editor's coded errors.
async fn require_price_manager(
    state: &ServerState,
    headers: &HeaderMap,
    share_id: &str,
) -> Result<crate::models::AuthSession, AppError> {
    let session = crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))?;
    if !session_manages_share(state, &session, share_id).await? {
        return Err(AppError::coded_forbidden(
            "SHARE_FORBIDDEN",
            "Share owner or Router owner required",
            serde_json::json!({}),
        ));
    }
    Ok(session)
}
struct ControlTarget {
    backend: String,
    installation: String,
    secret: String,
}
/// Offline/unbound Shares yield the same structured `quota_unavailable` for
/// preview and usage edits.
async fn control_target(state: &ServerState, share_id: &str) -> Result<ControlTarget, AppError> {
    let offline = || quota_unavailable("quota_server_offline");
    let route = state
        .proxy
        .route_by_share_id(share_id)
        .await
        .ok_or_else(offline)?;
    let installation = route.installation_id().ok_or_else(offline)?.to_string();
    let secret = state
        .store
        .installation_control_secret(&installation)
        .await?
        .ok_or_else(|| quota_unavailable("quota_control_unauthorized"))?;
    Ok(ControlTarget {
        backend: route.route_target().to_string(),
        installation,
        secret,
    })
}
/// Client-error codes a Server may return for preview/correction. Only these
/// exact codes are forwarded; any other 4xx falls back to a status default.
const FORWARDED_CONTROL_CODES: &[&str] = &[
    "invalid_policy",
    "quota_target_invalid",
    "quota_revision_conflict",
    "value_quota_not_active",
    "value_quota_busy",
    "quota_target_not_found",
];
/// A machine reason token copied from a Server error's `details.reasonCode`;
/// anything that is not a short lowercase token is dropped.
fn server_reason_code(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let code = value.get("details")?.get("reasonCode")?.as_str()?;
    (!code.is_empty()
        && code.len() <= 64
        && code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'))
    .then(|| code.to_string())
}
fn control_error(error: crate::ctl_client::CtlError) -> AppError {
    use axum::http::StatusCode;
    if let crate::ctl_client::CtlError::Rejected {
        status,
        code,
        message,
        body,
        ..
    } = &error
    {
        // A JSON error object (axum route-not-found has an empty body) means the
        // Server understood the route and rejected the target itself.
        let structured = code.is_some() || message.is_some();
        if let Some(known) = [
            "SHARE_CAPABILITY_DISABLED",
            "CAPABILITY_NOT_SUPPORTED",
            "CAPABILITY_CONTEXT_REQUIRED",
            "CAPABILITY_REQUEST_INVALID",
            "CAPABILITY_EXECUTION_QUARANTINED",
            "CAPABILITY_STATE_CHANGED",
            "CAPABILITY_UPGRADE_REQUIRED",
            "QUOTA_COVERAGE_REQUIRED",
            "CAPABILITY_PREVIEW_EXPIRED",
            "MARKET_CAPABILITY_CONFLICT",
        ]
        .into_iter()
        .find(|known| code.as_deref() == Some(*known))
        {
            return AppError::Coded {
                status: StatusCode::from_u16(*status).unwrap_or(StatusCode::CONFLICT),
                code: known,
                message: known.into(),
                details: serde_json::json!({"reasonCode":known,"previewRequired":true}),
            };
        }
        match *status {
            404 | 405 if !structured => return quota_unavailable("quota_server_upgrade_required"),
            401 | 403 => return quota_unavailable("quota_control_unauthorized"),
            400..=499 => {
                let known = code
                    .as_deref()
                    .and_then(|code| FORWARDED_CONTROL_CODES.iter().find(|k| **k == code))
                    .copied();
                let code = known.unwrap_or(match *status {
                    404 => "quota_target_not_found",
                    409 => "quota_revision_conflict",
                    429 => "value_quota_busy",
                    _ => "invalid_policy",
                });
                let reason_code = server_reason_code(body).unwrap_or_else(|| {
                    if *status == 400 && known.is_none() {
                        "invalid_policy".to_string()
                    } else {
                        code.to_string()
                    }
                });
                // Messages are Router-owned; upstream text is never forwarded.
                let message = match code {
                    "quota_revision_conflict" => {
                        "Quota policy or usage changed; refresh the authoritative preview before retrying"
                    }
                    "quota_target_invalid" => {
                        "The target cannot be below observed usage or outside the supported range"
                    }
                    "value_quota_not_active" => {
                        "This user has no active USD-equivalent quota to correct"
                    }
                    "value_quota_busy" => "USD-equivalent metering is busy; retry shortly",
                    "quota_target_not_found" => "The Share or user was not found on the Server",
                    _ => "This limit or period is not valid for a USD-equivalent quota",
                };
                let mut details = serde_json::json!({"reasonCode": reason_code});
                if code == "quota_revision_conflict" {
                    details["previewRequired"] = true.into();
                }
                return AppError::Coded {
                    status: StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_REQUEST),
                    code,
                    message: message.into(),
                    details,
                };
            }
            _ => {}
        }
    }
    let code = match &error {
        crate::ctl_client::CtlError::Unreachable(_) => "quota_server_offline",
        crate::ctl_client::CtlError::Timeout => "quota_server_timeout",
        crate::ctl_client::CtlError::Rejected { code, .. }
            if code.as_deref() == Some("price_unavailable") =>
        {
            "price_unavailable"
        }
        crate::ctl_client::CtlError::Malformed(_) => "quota_server_invalid_response",
        _ => "quota_unavailable",
    };
    quota_unavailable(code)
}
fn quota_unavailable(readiness_code: &'static str) -> AppError {
    AppError::Coded {
        status: axum::http::StatusCode::SERVICE_UNAVAILABLE,
        code: "quota_unavailable",
        message: "USD-equivalent metering is unavailable; check Server storage, coverage and acknowledged tariff readiness".into(),
        details: serde_json::json!({"readinessCode": readiness_code}),
    }
}
async fn health(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::api::require_owner_session(&state, &headers).await?;
    Ok(Json(state.store.value_quota_health().await?))
}
async fn prepare_history(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
    Json(input): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_share_manager(&state, &headers, &share_id).await?;
    let revision = input
        .get("expectedConfigRevision")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| AppError::BadRequest("expectedConfigRevision is required".into()))?;
    let emails: Vec<String> = serde_json::from_value(
        input
            .get("emails")
            .cloned()
            .ok_or_else(|| AppError::BadRequest("emails are required".into()))?,
    )
    .map_err(|e| AppError::BadRequest(e.to_string()))?;
    if emails.is_empty() || emails.len() > 500 {
        return Err(AppError::BadRequest("invalid users".into()));
    }
    let target = control_target(&state, &share_id).await?;
    crate::ctl_client::prepare_quota_history(
        &target.backend,
        &target.installation,
        &target.secret,
        &share_id,
        revision,
        &emails,
        input
            .get("restart")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    )
    .await
    .map(Json)
    .map_err(control_error)
}

async fn preview(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
    Json(input): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_share_manager(&state, &headers, &share_id).await?;
    let revision = input
        .get("expectedConfigRevision")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| AppError::BadRequest("expectedConfigRevision is required".into()))?;
    let policies = input
        .get("policies")
        .map(|v| serde_json::from_value(v.clone()))
        .transpose()
        .map_err(|e| AppError::BadRequest(e.to_string()))?
        .unwrap_or_default();
    let target = control_target(&state, &share_id).await?;
    let mut response = crate::ctl_client::preview_value_quota(
        &target.backend,
        &target.installation,
        &target.secret,
        &share_id,
        revision,
        &policies,
    )
    .await
    .map_err(control_error)?;
    if !response.is_object()
        || response
            .get("ready")
            .and_then(serde_json::Value::as_bool)
            .is_none()
        || response
            .get("configRevision")
            .and_then(serde_json::Value::as_u64)
            != Some(revision)
        || !response
            .get("rows")
            .is_some_and(serde_json::Value::is_array)
    {
        return Err(quota_unavailable("quota_server_invalid_response"));
    }
    let pricing = state
        .store
        .value_pricing_readiness(&target.installation)
        .await?;
    let published = response_pricing_published(&pricing);
    response["pricing"] = pricing;
    if let Some(checks) = response
        .get_mut("checks")
        .and_then(serde_json::Value::as_array_mut)
    {
        checks.push(serde_json::json!({"code":"tariff_published","ready":published}));
        if !published {
            mark_blocked_by_tariff(checks);
        }
    }
    if !published {
        response["ready"] = false.into();
        response["capabilityReady"] = false.into();
    }
    Ok(Json(response))
}
/// Readiness check a Server adds once it can enforce a USD-equivalent quota on
/// a Router-managed market grant. Older Servers omit it.
pub(crate) const MARKET_SEAT_QUOTA_CHECK: &str = "market_seat_quota";
/// Readiness check a Server adds once it enforces a one-time USD trial budget
/// (`trialValueLimitUsdMicros`) on a managed market grant. Older Servers omit it.
pub(crate) const MARKET_TRIAL_VALUE_CHECK: &str = "market_trial_value";
/// Reason reported when the seat would offer a USD trial budget but the
/// Server predates `market_trial_value`.
pub(crate) const SERVER_TRIAL_UPGRADE_REQUIRED: &str = "server_trial_upgrade_required";
fn market_value_unavailable(reason: String) -> AppError {
    AppError::coded_conflict(
        "share_market_value_quota_unavailable",
        "this Share cannot sell a USD-equivalent quota right now",
        serde_json::json!({ "reason": reason }),
    )
}
fn market_value_unavailable_checks(reason: String, checks: &[serde_json::Value]) -> AppError {
    AppError::coded_conflict(
        "share_market_value_quota_unavailable",
        "this Share cannot sell a USD-equivalent quota right now",
        serde_json::json!({"reason":reason,"checks":checks}),
    )
}
/// Only bounded diagnostic identities cross the control-plane boundary. Never
/// return arbitrary upstream messages, credentials, or unrecognized fields.
fn market_readiness_checks(
    response: &serde_json::Value,
    revision: u64,
) -> Option<Vec<serde_json::Value>> {
    if response["configRevision"].as_u64() != Some(revision)
        || !response["capabilityReady"].is_boolean()
    {
        return None;
    }
    let checks = response["checks"].as_array()?;
    if checks.is_empty() || checks.len() > 128 {
        return None;
    }
    checks
        .iter()
        .map(|check| {
            let code = check["code"].as_str()?;
            if code.is_empty()
                || code.len() > 64
                || !code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return None;
            }
            let mut safe = serde_json::json!({"code":code,"ready":check["ready"].as_bool()?});
            for key in [
                "app",
                "providerId",
                "profileId",
                "driverId",
                "model",
                "state",
                "scope",
                "reasonCode",
                "blockedBy",
            ] {
                if let Some(value) = check[key].as_str().filter(|value| {
                    !value.is_empty()
                        && value.len() <= 256
                        && value.trim() == *value
                        && !value.chars().any(char::is_control)
                }) {
                    safe[key] = value.into();
                }
            }
            if let Some(at) = check["effectiveFromMs"].as_u64() {
                safe["effectiveFromMs"] = at.into();
            }
            // Money is bound to Market seats: usage the Server would meter at
            // $0 for an owner grant is refused for a paid seat, so a missing
            // price blocks every Market gate (publish, edit, reopen, quote).
            if unpriced_for_market(check) {
                safe["ready"] = false.into();
            }
            Some(safe)
        })
        .collect()
}
/// Price-type reasons the Server meters at $0 for owner-managed grants only.
const UNPRICED_REASONS: [&str; 5] = [
    "model_unpriced",
    "model_unresolved",
    "tier_unpriced",
    "resource_unpriced",
    "tariff_not_effective",
];
fn unpriced_for_market(check: &serde_json::Value) -> bool {
    check["state"] == "unpriced"
        || (check["code"] == "model_price_coverage"
            && check["reasonCode"]
                .as_str()
                .is_some_and(|reason| UNPRICED_REASONS.contains(&reason)))
}
fn check_ready(checks: &[serde_json::Value], code: &str) -> bool {
    checks
        .iter()
        .any(|check| check["code"] == code && check["ready"] == true)
}
/// A seat may sell a USD-equivalent quota only while its Share's Server can
/// enforce one: the owner's USD capability (storage, tariff, text surfaces)
/// must be ready and the Server must run managed USD grants. Checked when a
/// USD seat is published, edited, reopened or quoted; the Server rejects the
/// grant itself if readiness is lost before the rental applies. A seat that
/// offers a budgeted USD trial (`value_trial`) also needs a Server that
/// enforces the USD trial budget; there is no silent time-only fallback.
pub(crate) async fn ensure_market_value_quota_ready(
    state: &ServerState,
    share_id: &str,
    value_trial: bool,
    resources: bool,
) -> Result<(), AppError> {
    let checks = market_value_quota_checks(state, share_id).await?;
    if resources && !check_ready(&checks, "resource_metering") {
        return Err(market_value_unavailable("resource_upgrade_required".into()));
    }
    if value_trial && !check_ready(&checks, MARKET_TRIAL_VALUE_CHECK) {
        return Err(market_value_unavailable(
            SERVER_TRIAL_UPGRADE_REQUIRED.into(),
        ));
    }
    Ok(())
}
/// The base USD seat gate; returns the Server's readiness checks on success.
async fn market_value_quota_checks(
    state: &ServerState,
    share_id: &str,
) -> Result<Vec<serde_json::Value>, AppError> {
    let unavailable = market_value_unavailable;
    let target = control_target(state, share_id)
        .await
        .map_err(|_| unavailable("quota_server_offline".into()))?;
    let revision = state.store.share_config_revision(share_id).await?;
    let response = crate::ctl_client::preview_value_quota(
        &target.backend,
        &target.installation,
        &target.secret,
        share_id,
        revision,
        &std::collections::BTreeMap::new(),
    )
    .await
    .map_err(|error| match control_error(error) {
        AppError::Coded { code, details, .. } => unavailable(
            details["readinessCode"]
                .as_str()
                .unwrap_or(code)
                .to_string(),
        ),
        _ => unavailable("quota_unavailable".into()),
    })?;
    let mut checks = market_readiness_checks(&response, revision)
        .ok_or_else(|| unavailable("quota_server_invalid_response".into()))?;
    if !check_ready(&checks, MARKET_SEAT_QUOTA_CHECK) {
        return Err(market_value_unavailable_checks(
            "server_upgrade_required".into(),
            &checks,
        ));
    }
    // Diagnose an unpublished Router tariff even when the Server already
    // reports a dependent ACK/model failure; expose the actionable root.
    let pricing = state
        .store
        .value_pricing_readiness(&target.installation)
        .await?;
    if !response_pricing_published(&pricing) {
        checks.push(serde_json::json!({"code":"tariff_published","ready":false}));
        mark_blocked_by_tariff(&mut checks);
    }
    if response["capabilityReady"] != true
        || checks.iter().any(|check| check["ready"] != true)
        || [
            "router_protocol",
            "sqlite_authority",
            "metering_storage",
            "acknowledged_tariff",
            "text_surface",
        ]
        .iter()
        .any(|code| !check_ready(&checks, code))
    {
        let failed = checks
            .iter()
            .find(|check| check["ready"] != true)
            .and_then(|check| check["code"].as_str())
            .unwrap_or("quota_unavailable");
        return Err(market_value_unavailable_checks(failed.to_string(), &checks));
    }
    Ok(checks)
}
/// Whether the seat editor may offer a USD-equivalent quota for this Share;
/// the same gate publish, edit, reopen and quote apply. `valueTrialReady`
/// tells whether a USD seat may also offer a budgeted USD trial.
async fn market_readiness(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_share_manager(&state, &headers, &share_id).await?;
    Ok(Json(
        match market_value_quota_checks(&state, &share_id).await {
            Ok(checks) => serde_json::json!({
                "ready": true,
                "valueTrialReady": check_ready(&checks, MARKET_TRIAL_VALUE_CHECK),
                "resourceReady": check_ready(&checks, "resource_metering"),
                "checks": checks,
            }),
            Err(AppError::Coded {
                code: "share_market_value_quota_unavailable",
                details,
                ..
            }) => {
                serde_json::json!({ "ready": false, "reason": details["reason"], "checks":details["checks"] })
            }
            Err(error) => return Err(error),
        },
    ))
}
/// An unacknowledged tariff is a consequence of an unpublished one; point the
/// failed check at its root unless the Server already named a blocker.
fn mark_blocked_by_tariff(checks: &mut [serde_json::Value]) {
    for check in checks.iter_mut().filter(|c| {
        c["code"] == "acknowledged_tariff"
            && c["ready"] != true
            && c.get("blockedBy").is_none_or(serde_json::Value::is_null)
    }) {
        check["blockedBy"] = "tariff_published".into();
    }
}
fn response_pricing_published(pricing: &serde_json::Value) -> bool {
    pricing["publishedRevision"]
        .as_str()
        .is_some_and(|r| !r.is_empty())
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EditUsage {
    expected_config_revision: u64,
    edits: std::collections::BTreeMap<String, crate::share_value_pricing::ValueUsageEdit>,
}
async fn edit_usage(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
    Json(input): Json<EditUsage>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_share_manager(&state, &headers, &share_id).await?;
    let target = control_target(&state, &share_id).await?;
    let result = crate::ctl_client::edit_value_usage(
        &target.backend,
        &target.installation,
        &target.secret,
        &share_id,
        input.expected_config_revision,
        &session.email,
        &input.edits,
    )
    .await
    .map_err(control_error)?;
    // Per-row failures arrive as `items[].error: {code}` and are forwarded as-is.
    Ok(Json(result))
}
async fn fetch(
    State(state): State<ServerState>,
    Json(input): Json<SignedValueRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(
        state
            .store
            .signed_value_operation("share_value_pricing_fetch", input)
            .await?,
    ))
}
async fn ack(
    State(state): State<ServerState>,
    Json(input): Json<SignedValueRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(
        state
            .store
            .signed_value_operation("share_value_pricing_ack", input)
            .await?,
    ))
}
async fn active(
    State(state): State<ServerState>,
    Json(input): Json<SignedValueRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(
        state
            .store
            .signed_value_operation("share_value_pricing_active", input)
            .await?,
    ))
}
async fn watermark(
    State(state): State<ServerState>,
    Path(share_id): Path<String>,
    Json(input): Json<SignedValueRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if input.payload.get("shareId").and_then(|v| v.as_str()) != Some(share_id.as_str()) {
        return Err(AppError::BadRequest(
            "Share path does not match signed body".into(),
        ));
    }
    Ok(Json(
        state
            .store
            .signed_value_operation("share_value_quota_watermark", input)
            .await?,
    ))
}
/// Batched projections (`valueQuotaWatermarkBatchV1`): one signed request,
/// one answer per item, each item applied on its own savepoint.
async fn watermarks(
    State(state): State<ServerState>,
    Json(input): Json<SignedValueRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(
        state
            .store
            .signed_value_operation("share_value_quota_watermarks", input)
            .await?,
    ))
}
async fn status(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_share_manager(&state, &headers, &share_id).await?;
    Ok(Json(
        serde_json::json!({"items":state.store.share_value_quota_status(&share_id,None).await?}),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MineQuery {
    share_id: String,
}
/// Grantee view: only the caller's own rows, self-contained (limit, usage,
/// risk, window, metering status, sync time and any scheduled USD policy).
async fn mine(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<MineQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = crate::api::resolve_router_session(&state, &headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))?;
    Ok(Json(
        serde_json::json!({"items":state.store.share_value_quota_status(&query.share_id,Some(&session.email.to_ascii_lowercase())).await?}),
    ))
}

async fn list_model_prices(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_price_manager(&state, &headers, &share_id).await?;
    let (installation_id, prices) = state.store.owner_model_prices(&share_id).await?;
    Ok(Json(
        serde_json::json!({"installationId":installation_id,"prices":prices}),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OwnerPriceRequest {
    #[serde(default)]
    display_name: Option<String>,
    rates: crate::api::AdminModelPriceRatesRequest,
    /// Absent or null: the price must not exist yet.
    #[serde(default)]
    expected_version: Option<i64>,
}
async fn upsert_model_price(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((share_id, model_key)): Path<(String, String)>,
    body: Result<Json<OwnerPriceRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<serde_json::Value>, AppError> {
    use crate::store::share_value_quotas::owner_prices::{OwnerPriceEdit, price_invalid};
    let session = require_price_manager(&state, &headers, &share_id).await?;
    let Json(input) = body.map_err(|rejection| {
        price_invalid(
            "body_invalid",
            serde_json::json!({"detail": rejection.body_text()}),
        )
    })?;
    let rates = crate::api::parse_model_price_rates(&input.rates).map_err(|error| {
        price_invalid(
            "rate_invalid",
            serde_json::json!({"detail": error.to_string()}),
        )
    })?;
    let (price, effective_from_ms) = state
        .store
        .upsert_owner_model_price(
            &share_id,
            OwnerPriceEdit {
                model_key,
                display_name: input.display_name,
                rates,
                expected_version: input.expected_version,
            },
            &session.email,
        )
        .await?;
    Ok(Json(
        serde_json::json!({"price":price,"effectiveFromMs":effective_from_ms}),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeletePriceQuery {
    expected_version: i64,
}
async fn delete_model_price(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((share_id, model_key)): Path<(String, String)>,
    query: Result<Query<DeletePriceQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_price_manager(&state, &headers, &share_id).await?;
    let Query(query) = query.map_err(|rejection| {
        crate::store::share_value_quotas::owner_prices::price_invalid(
            "expected_version_required",
            serde_json::json!({"detail": rejection.body_text()}),
        )
    })?;
    let effective_from_ms = state
        .store
        .delete_owner_model_price(
            &share_id,
            &model_key,
            query.expected_version,
            &session.email,
        )
        .await?;
    Ok(Json(
        serde_json::json!({"deleted":true,"effectiveFromMs":effective_from_ms}),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SuggestionQuery {
    model: String,
}
/// Catalog rates only, so Router admins (who price Router-wide from the same
/// dialog) may read them as well as Share managers.
async fn model_price_suggestions(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
    Query(query): Query<SuggestionQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = crate::api::resolve_router_session(&state, &headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))?;
    if !session_manages_share(&state, &session, &share_id).await?
        && !state.dynamic.read().await.is_admin(&session.email)
    {
        return Err(AppError::coded_forbidden(
            "SHARE_FORBIDDEN",
            "Share owner, Router owner or Router admin required",
            serde_json::json!({}),
        ));
    }
    if query.model.len() > 256 {
        return Err(
            crate::store::share_value_quotas::owner_prices::price_invalid(
                "model_key_invalid",
                serde_json::json!({"field": "model"}),
            ),
        );
    }
    Ok(Json(
        serde_json::json!({"suggestions":state.store.owner_model_price_suggestions(&query.model)}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn market_value_readiness_retains_only_bounded_diagnostics_and_valid_revisions() {
        let response = serde_json::json!({"configRevision":7,"capabilityReady":false,"checks":[
            {"code":"text_surface","ready":false,"app":"claude","providerId":"provider-1",
                "profileId":"claude.workbuddy_oauth","driverId":"special.workbuddy_oauth",
                "reasonCode":"runtime_unsupported","secret":"never forward","message":"upstream body"},
            {"code":"model_price_coverage","ready":false,"app":"codex","model":"auto","reasonCode":"model_unresolved",
                "effectiveFromMs":123,"state":"bad\nstate"}]});
        let checks = market_readiness_checks(&response, 7).unwrap();
        assert_eq!(checks[0]["providerId"], "provider-1");
        assert_eq!(checks[1]["reasonCode"], "model_unresolved");
        assert_eq!(checks[1]["model"], "auto");
        assert_eq!(checks[1]["effectiveFromMs"], 123);
        assert!(checks[0].get("secret").is_none());
        assert!(checks[0].get("message").is_none());
        assert!(checks[1].get("state").is_none());
        assert!(market_readiness_checks(&response, 8).is_none());
        for checks in [
            serde_json::json!([]),
            serde_json::json!([null]),
            serde_json::json!([{"code":"text_surface","ready":"true"}]),
        ] {
            let mut invalid = response.clone();
            invalid["checks"] = checks;
            assert!(market_readiness_checks(&invalid, 7).is_none());
        }
        if let AppError::Coded { details, .. } =
            market_value_unavailable_checks("text_surface".into(), &checks)
        {
            assert_eq!(details["checks"][0]["app"], "claude");
            assert!(!details.to_string().contains("never forward"));
        } else {
            panic!("expected structured error");
        }
    }
    #[test]
    fn share_value_unpriced_coverage_is_forwarded_and_blocks_market_seats() {
        // Owner grants meter it at $0 (`ready:true`), but a paid seat would be
        // refused by the Server, so every Market gate treats it as blocking.
        let response = serde_json::json!({"configRevision":3,"capabilityReady":true,"checks":[
            {"code":"model_price_coverage","ready":true,"state":"unpriced","app":"codex",
                "model":"kimi-k3-1","reasonCode":"model_unpriced"},
            {"code":"model_price_coverage","ready":true,"app":"claude",
                "model":"house-model","reasonCode":"tariff_not_effective"},
            {"code":"model_price_coverage","ready":true,"app":"gemini","model":"gemini-3-pro"}]});
        let checks = market_readiness_checks(&response, 3).unwrap();
        assert_eq!(checks[0]["state"], "unpriced");
        assert_eq!(checks[0]["reasonCode"], "model_unpriced");
        assert_eq!(checks[0]["model"], "kimi-k3-1");
        assert_eq!(checks[0]["ready"], false);
        assert_eq!(checks[1]["ready"], false);
        assert_eq!(checks[2]["ready"], true, "a priced model stays ready");
        assert!(checks.iter().any(|check| check["ready"] != true));
    }

    #[tokio::test]
    async fn share_value_errors_are_private_and_offline_is_structured() {
        use axum::response::IntoResponse;
        for error in [
            AppError::Forbidden("Share owner or Router owner required".into()),
            quota_unavailable("quota_server_offline"),
            AppError::BadRequest("invalid".into()),
        ] {
            let response = no_store(error.into_response()).await;
            assert_eq!(
                response.headers().get(header::CACHE_CONTROL).unwrap(),
                "private, no-store"
            );
        }
        let offline = quota_unavailable("quota_server_offline");
        assert_eq!(offline.code(), Some("quota_unavailable"));
        assert_eq!(
            control_error(crate::ctl_client::CtlError::Unreachable("x".into())).to_string(),
            offline.to_string()
        );
        if let AppError::Coded { details, .. } = offline {
            assert_eq!(details["readinessCode"], "quota_server_offline");
        } else {
            panic!("expected structured error");
        }
    }

    #[test]
    fn share_value_private_errors_never_forward_upstream_diagnostics() {
        let unavailable = control_error(crate::ctl_client::CtlError::Timeout);
        assert_eq!(unavailable.code(), Some("quota_unavailable"));
        if let AppError::Coded { details, .. } = unavailable {
            assert_eq!(details["readinessCode"], "quota_server_timeout");
        } else {
            panic!("expected structured error");
        }
        let old_server = control_error(crate::ctl_client::CtlError::Rejected {
            status: 404,
            body: "private upstream data".into(),
            message: None,
            code: None,
            retryable: None,
            current_config_revision: None,
            current_share: None,
        });
        if let AppError::Coded {
            details, message, ..
        } = old_server
        {
            assert_eq!(details["readinessCode"], "quota_server_upgrade_required");
            assert!(!message.contains("private upstream data"));
        } else {
            panic!("expected structured error");
        }
        let conflict = control_error(crate::ctl_client::CtlError::Rejected {
            status: 409,
            body: "private upstream data".into(),
            message: Some("private data".into()),
            code: Some("quota_revision_conflict".into()),
            retryable: None,
            current_config_revision: None,
            current_share: None,
        });
        assert_eq!(conflict.code(), Some("quota_revision_conflict"));
        assert!(!conflict.to_string().contains("private"));
    }

    #[test]
    fn capability_preview_errors_keep_codes_without_exposing_control_diagnostics() {
        for (status, code) in [
            (403, "SHARE_CAPABILITY_DISABLED"),
            (409, "CAPABILITY_STATE_CHANGED"),
            (409, "CAPABILITY_PREVIEW_EXPIRED"),
            (409, "QUOTA_COVERAGE_REQUIRED"),
            (503, "CAPABILITY_EXECUTION_QUARANTINED"),
        ] {
            let error = control_error(crate::ctl_client::CtlError::Rejected {
                status,
                body: "private upstream data".into(),
                message: Some("private upstream data".into()),
                code: Some(code.into()),
                retryable: Some(false),
                current_config_revision: None,
                current_share: None,
            });
            assert_eq!(error.code(), Some(code));
            let AppError::Coded {
                status: actual,
                details,
                message,
                ..
            } = error
            else {
                panic!("expected structured capability error");
            };
            assert_eq!(actual.as_u16(), status);
            assert_eq!(details["reasonCode"], code);
            assert_eq!(details["previewRequired"], true);
            assert!(!message.contains("private"));
        }
    }

    #[test]
    fn usd_trial_capability_is_a_distinct_ready_check() {
        let old_server = vec![
            serde_json::json!({"code": MARKET_SEAT_QUOTA_CHECK, "ready": true}),
            serde_json::json!({"code": "sqlite_authority", "ready": true}),
        ];
        assert!(check_ready(&old_server, MARKET_SEAT_QUOTA_CHECK));
        assert!(!check_ready(&old_server, MARKET_TRIAL_VALUE_CHECK));
        let mut current = old_server.clone();
        current.push(serde_json::json!({"code": MARKET_TRIAL_VALUE_CHECK, "ready": true}));
        assert!(check_ready(&current, MARKET_TRIAL_VALUE_CHECK));
        let not_ready = vec![serde_json::json!({"code": MARKET_TRIAL_VALUE_CHECK, "ready": false})];
        assert!(!check_ready(&not_ready, MARKET_TRIAL_VALUE_CHECK));
        let error = market_value_unavailable(SERVER_TRIAL_UPGRADE_REQUIRED.into());
        assert_eq!(error.code(), Some("share_market_value_quota_unavailable"));
        if let AppError::Coded { details, .. } = error {
            assert_eq!(details["reason"], "server_trial_upgrade_required");
        } else {
            panic!("expected structured error");
        }
    }

    #[test]
    fn unpublished_tariff_blocks_failed_acknowledgement_only() {
        let mut checks = vec![
            serde_json::json!({"code":"acknowledged_tariff","ready":false}),
            serde_json::json!({"code":"acknowledged_tariff","ready":false,"blockedBy":"sqlite_authority"}),
            serde_json::json!({"code":"acknowledged_tariff","ready":true}),
            serde_json::json!({"code":"sqlite_authority","ready":false}),
            serde_json::json!({"code":"tariff_published","ready":false}),
        ];
        mark_blocked_by_tariff(&mut checks);
        assert_eq!(checks[0]["blockedBy"], "tariff_published");
        // A Server-named blocker wins; ready and unrelated checks stay untouched.
        assert_eq!(checks[1]["blockedBy"], "sqlite_authority");
        assert!(checks[2].get("blockedBy").is_none());
        assert!(checks[3].get("blockedBy").is_none());
        assert!(checks[4].get("blockedBy").is_none());
    }

    fn rejected(status: u16, body: serde_json::Value) -> crate::ctl_client::CtlError {
        crate::ctl_client::CtlError::Rejected {
            status,
            body: if body.is_null() {
                String::new()
            } else {
                body.to_string()
            },
            message: body["error"].as_str().map(str::to_string),
            code: body["code"].as_str().map(str::to_string),
            retryable: None,
            current_config_revision: None,
            current_share: None,
        }
    }
    fn coded(error: AppError) -> (u16, &'static str, serde_json::Value, String) {
        match error {
            AppError::Coded {
                status,
                code,
                details,
                message,
            } => (status.as_u16(), code, details, message),
            other => panic!("expected structured error, got {other:?}"),
        }
    }

    #[test]
    fn share_value_client_errors_forward_code_and_reason() {
        // Preview validation: code and reason pass through, message stays Router-owned.
        let (status, code, details, message) = coded(control_error(rejected(
            400,
            serde_json::json!({"error":"secret detail","code":"invalid_policy",
                "details":{"reasonCode":"unverified_surface"}}),
        )));
        assert_eq!((status, code), (400, "invalid_policy"));
        assert_eq!(details["reasonCode"], "unverified_surface");
        assert!(!message.contains("secret"));
        // A 400 without a code still reports invalid_policy.
        let (status, code, details, _) = coded(control_error(rejected(
            400,
            serde_json::json!({"error":"bad"}),
        )));
        assert_eq!((status, code), (400, "invalid_policy"));
        assert_eq!(details["reasonCode"], "invalid_policy");
        // Unsafe reason tokens are not forwarded.
        let (_, _, details, _) = coded(control_error(rejected(
            400,
            serde_json::json!({"code":"invalid_policy","details":{"reasonCode":"<b>x</b>"}}),
        )));
        assert_eq!(details["reasonCode"], "invalid_policy");
        for (status, server_code) in [
            (400, "quota_target_invalid"),
            (409, "quota_target_invalid"),
            (409, "quota_revision_conflict"),
            (400, "value_quota_not_active"),
            (409, "value_quota_not_active"),
            (429, "value_quota_busy"),
        ] {
            let (got, code, details, _) = coded(control_error(rejected(
                status,
                serde_json::json!({"error":"x","code":server_code}),
            )));
            assert_eq!((got, code), (status, server_code));
            assert_eq!(details["reasonCode"], server_code);
            assert_eq!(
                details["previewRequired"].as_bool(),
                (server_code == "quota_revision_conflict").then_some(true)
            );
        }
        // Unknown share/user from a current Server is a 404 with a JSON body.
        let (status, code, _, _) = coded(control_error(rejected(
            404,
            serde_json::json!({"ok":false,"error":"Share not found","code":null}),
        )));
        assert_eq!((status, code), (404, "quota_target_not_found"));
        // A bare route-not-found means the Server predates the route.
        let (status, code, details, _) =
            coded(control_error(rejected(404, serde_json::Value::Null)));
        assert_eq!((status, code), (503, "quota_unavailable"));
        assert_eq!(details["readinessCode"], "quota_server_upgrade_required");
        // 5xx stays quota_unavailable; price_unavailable keeps its readiness code.
        for (server_code, readiness) in [
            ("quota_unavailable", "quota_unavailable"),
            ("price_unavailable", "price_unavailable"),
        ] {
            let (status, code, details, _) = coded(control_error(rejected(
                503,
                serde_json::json!({"error":"x","code":server_code}),
            )));
            assert_eq!((status, code), (503, "quota_unavailable"));
            assert_eq!(details["readinessCode"], readiness);
        }
    }
}

async fn capability_preview(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
    Json(input): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_share_manager(&state, &headers, &share_id).await?;
    forward_capability_preview(&state, &share_id, input)
        .await
        .map(Json)
}
async fn request_metering(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
    Json(input): Json<RequestMeteringInput>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    require_share_manager(&state, &headers, &share_id).await?;
    let result = forward_request_metering(&state, &share_id, input).await?;
    Ok(([(header::CACHE_CONTROL, "private, no-store")], Json(result)))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RequestMeteringInput {
    request_id: String,
}

pub(crate) async fn forward_request_metering(
    state: &ServerState,
    share_id: &str,
    input: RequestMeteringInput,
) -> Result<serde_json::Value, AppError> {
    let request = input.request_id;
    if request.is_empty()
        || request.len() > 192
        || request.trim() != request
        || request.chars().any(char::is_control)
    {
        return Err(AppError::BadRequest("invalid request identity".into()));
    }
    let result =
        forward_capability_preview(state, share_id, serde_json::json!({"requestId":request}))
            .await?;
    if result["shareId"] != share_id || result["requestId"] != request {
        return Err(AppError::UnprocessableEntity(
            "CAPABILITY_STATE_CHANGED".into(),
        ));
    }
    Ok(result)
}

pub(crate) async fn forward_capability_preview(
    state: &ServerState,
    share_id: &str,
    mut input: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    if !input.is_object() {
        return Err(AppError::BadRequest("invalid capability preview".into()));
    }
    input["shareId"] = share_id.into();
    let target = control_target(&state, &share_id).await?;
    let result = crate::ctl_client::preview_share_capabilities(
        &target.backend,
        &target.installation,
        &target.secret,
        input,
    )
    .await
    .map_err(control_error)?;
    if result["shareId"] != share_id || !result["rows"].is_array() {
        return Err(AppError::UnprocessableEntity(
            "CAPABILITY_UPGRADE_REQUIRED".into(),
        ));
    }
    Ok(result)
}
