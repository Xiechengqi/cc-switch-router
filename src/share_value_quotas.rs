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
    routing::{get, post},
};
use serde::Deserialize;

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/admin/share-value-pricing", get(settings).post(publish))
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
        .layer(DefaultBodyLimit::max(
            crate::share_value_pricing::MAX_SNAPSHOT_BYTES + 65536,
        ))
}
fn private() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers
}
fn control_error(error: crate::ctl_client::CtlError) -> AppError {
    if let crate::ctl_client::CtlError::Rejected { status, code, .. } = &error {
        if *status == 409 {
            return AppError::coded_conflict(
                "quota_revision_conflict",
                "Quota policy or usage changed; refresh the authoritative preview before retrying",
                serde_json::json!({"previewRequired":true}),
            );
        }
        if *status == 400 && code.as_deref() == Some("quota_target_invalid") {
            return AppError::Coded {
                status: axum::http::StatusCode::BAD_REQUEST,
                code: "quota_target_invalid",
                message: "The target cannot be below observed usage or outside the supported range"
                    .into(),
                details: serde_json::json!({}),
            };
        }
    }
    let code = match &error {
        crate::ctl_client::CtlError::Unreachable(_) => "quota_server_offline",
        crate::ctl_client::CtlError::Timeout => "quota_server_timeout",
        crate::ctl_client::CtlError::Rejected {
            status: 404 | 405, ..
        } => "quota_server_upgrade_required",
        crate::ctl_client::CtlError::Rejected {
            status: 401 | 403, ..
        } => "quota_control_unauthorized",
        crate::ctl_client::CtlError::Malformed(_) => "quota_server_invalid_response",
        _ => "quota_unavailable",
    };
    AppError::Coded {status:axum::http::StatusCode::SERVICE_UNAVAILABLE,code:"quota_unavailable",
        message:"USD-equivalent metering is unavailable; check Server storage, coverage and acknowledged tariff readiness".into(),
        details:serde_json::json!({"readinessCode":code})}
}
async fn health(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Json<serde_json::Value>), AppError> {
    crate::api::require_owner_session(&state, &headers).await?;
    Ok((private(), Json(state.store.value_quota_health().await?)))
}
async fn preview(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
    Json(input): Json<serde_json::Value>,
) -> Result<(HeaderMap, Json<serde_json::Value>), AppError> {
    let session = crate::api::resolve_router_session(&state, &headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))?;
    let owner = state
        .store
        .lookup_share_owner_email(&share_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Share not found".into()))?;
    if !owner.eq_ignore_ascii_case(&session.email)
        && !crate::api::is_finance_owner(&state.config, &session.email)
    {
        return Err(AppError::Forbidden(
            "Share owner or Router owner required".into(),
        ));
    }
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
    let route = state
        .proxy
        .route_by_share_id(&share_id)
        .await
        .ok_or_else(|| control_error(crate::ctl_client::CtlError::Unreachable(String::new())))?;
    let installation = route
        .installation_id()
        .ok_or_else(|| AppError::ServiceUnavailable("Share installation unavailable".into()))?;
    let secret = state
        .store
        .installation_control_secret(installation)
        .await?
        .ok_or_else(|| AppError::ServiceUnavailable("Share control unavailable".into()))?;
    let mut response = crate::ctl_client::preview_value_quota(
        route.route_target(),
        installation,
        &secret,
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
        return Err(control_error(crate::ctl_client::CtlError::Malformed(
            String::new(),
        )));
    }
    let pricing = state.store.value_pricing_readiness(installation).await?;
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
    Ok((private(), Json(response)))
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
) -> Result<(HeaderMap, Json<serde_json::Value>), AppError> {
    let session = crate::api::resolve_router_session(&state, &headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))?;
    let owner = state
        .store
        .lookup_share_owner_email(&share_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Share not found".into()))?;
    if !owner.eq_ignore_ascii_case(&session.email)
        && !crate::api::is_finance_owner(&state.config, &session.email)
    {
        return Err(AppError::Forbidden(
            "Share owner or Router owner required".into(),
        ));
    }
    let route = state
        .proxy
        .route_by_share_id(&share_id)
        .await
        .ok_or_else(|| AppError::ServiceUnavailable("Share client offline".into()))?;
    let installation = route
        .installation_id()
        .ok_or_else(|| AppError::ServiceUnavailable("Share installation unavailable".into()))?;
    let secret = state
        .store
        .installation_control_secret(installation)
        .await?
        .ok_or_else(|| AppError::ServiceUnavailable("Share control unavailable".into()))?;
    let result = crate::ctl_client::edit_value_usage(
        route.route_target(),
        installation,
        &secret,
        &share_id,
        input.expected_config_revision,
        &session.email,
        &input.edits,
    )
    .await
    .map_err(control_error)?;
    Ok((private(), Json(result)))
}
async fn settings(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<
    (
        HeaderMap,
        Json<crate::store::share_value_quotas::PricingSettings>,
    ),
    AppError,
> {
    crate::api::require_owner_session(&state, &headers).await?;
    Ok((private(), Json(state.store.value_pricing_settings().await?)))
}
async fn publish(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<PublishPricing>,
) -> Result<
    (
        HeaderMap,
        Json<crate::store::share_value_quotas::PricingSettings>,
    ),
    AppError,
> {
    let session = crate::api::require_owner_session(&state, &headers).await?;
    Ok((
        private(),
        Json(
            state
                .store
                .publish_value_pricing(input, &session.email)
                .await?,
        ),
    ))
}
async fn fetch(
    State(state): State<ServerState>,
    Json(input): Json<SignedValueRequest>,
) -> Result<(HeaderMap, Json<serde_json::Value>), AppError> {
    Ok((
        private(),
        Json(
            state
                .store
                .signed_value_operation("share_value_pricing_fetch", input)
                .await?,
        ),
    ))
}
async fn ack(
    State(state): State<ServerState>,
    Json(input): Json<SignedValueRequest>,
) -> Result<(HeaderMap, Json<serde_json::Value>), AppError> {
    Ok((
        private(),
        Json(
            state
                .store
                .signed_value_operation("share_value_pricing_ack", input)
                .await?,
        ),
    ))
}
async fn watermark(
    State(state): State<ServerState>,
    Path(share_id): Path<String>,
    Json(input): Json<SignedValueRequest>,
) -> Result<(HeaderMap, Json<serde_json::Value>), AppError> {
    if input.payload.get("shareId").and_then(|v| v.as_str()) != Some(share_id.as_str()) {
        return Err(AppError::BadRequest(
            "Share path does not match signed body".into(),
        ));
    }
    Ok((
        private(),
        Json(
            state
                .store
                .signed_value_operation("share_value_quota_watermark", input)
                .await?,
        ),
    ))
}
async fn status(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(share_id): Path<String>,
) -> Result<(HeaderMap, Json<serde_json::Value>), AppError> {
    let session = crate::api::resolve_router_session(&state, &headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))?;
    let owner = state
        .store
        .lookup_share_owner_email(&share_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Share not found".into()))?;
    if !owner.eq_ignore_ascii_case(&session.email)
        && !crate::api::is_finance_owner(&state.config, &session.email)
    {
        return Err(AppError::Forbidden(
            "Share owner or Router owner required".into(),
        ));
    }
    Ok((
        private(),
        Json(
            serde_json::json!({"items":state.store.share_value_quota_status(&share_id,None).await?}),
        ),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MineQuery {
    share_id: String,
}
async fn mine(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<MineQuery>,
) -> Result<(HeaderMap, Json<serde_json::Value>), AppError> {
    let session = crate::api::resolve_router_session(&state, &headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))?;
    Ok((
        private(),
        Json(
            serde_json::json!({"items":state.store.share_value_quota_status(&query.share_id,Some(&session.email.to_ascii_lowercase())).await?}),
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn share_value_private_headers_and_errors_never_forward_upstream_diagnostics() {
        assert_eq!(
            private().get(header::CACHE_CONTROL).unwrap(),
            "private, no-store"
        );
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
}
