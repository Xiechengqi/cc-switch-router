//! Private Share value-quota control plane. Never mounted on public usage APIs.
use crate::{
    ServerState,
    error::AppError,
    store::share_value_quotas::{PublishPricing, SignedValueRequest},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, HeaderValue, header},
    middleware,
    response::Response,
    routing::{get, post},
};
use serde::Deserialize;

/// Signed Server control payloads (fetch/ack/watermark) and browser control
/// bodies are small; only the Owner tariff publication carries a snapshot.
const CONTROL_BODY_LIMIT: usize = 256 * 1024;

pub fn router() -> Router<ServerState> {
    let publication = Router::new()
        .route("/v1/admin/share-value-pricing", get(settings).post(publish))
        .layer(DefaultBodyLimit::max(
            crate::share_value_pricing::MAX_SNAPSHOT_BYTES + 65536,
        ));
    Router::new()
        .route("/v1/admin/share-value-pricing/health", get(health))
        .route("/v1/shares/value-pricing/fetch", post(fetch))
        .route("/v1/shares/value-pricing/ack", post(ack))
        .route(
            "/v1/shares/:share_id/value-quota-watermarks",
            post(watermark),
        )
        .route("/v1/shares/:share_id/value-quota-status", get(status))
        .route("/v1/me/share-value-quotas", get(mine))
        .route("/v1/shares/:share_id/value-quota-preview", post(preview))
        .route("/v1/shares/:share_id/value-usage-edits", post(edit_usage))
        .layer(DefaultBodyLimit::max(CONTROL_BODY_LIMIT))
        .merge(publication)
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
    }
    if !published {
        response["ready"] = false.into();
        response["capabilityReady"] = false.into();
    }
    Ok(Json(response))
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
async fn settings(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<crate::store::share_value_quotas::PricingSettings>, AppError> {
    crate::api::require_owner_session(&state, &headers).await?;
    Ok(Json(state.store.value_pricing_settings().await?))
}
async fn publish(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<PublishPricing>,
) -> Result<Json<crate::store::share_value_quotas::PricingSettings>, AppError> {
    let session = crate::api::require_owner_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .publish_value_pricing(input, &session.email)
            .await?,
    ))
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

#[cfg(test)]
mod tests {
    use super::*;
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
