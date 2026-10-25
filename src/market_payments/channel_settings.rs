//! Channel settings that do not touch merchant identity or secrets: hosted vs
//! direct checkout, allowed rails, per-order bounds and the fee model used for
//! Owner reporting. Editing them never creates a credential revision, so a
//! live channel stays live. Each order freezes the values it was created with.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;

use super::{
    ChannelView, MAX_TOPUP_MINOR, MIN_TOPUP_MINOR, ServerState, audit_tx, channel_view_tx,
    db_error, no_store, now, token_rail,
};
use crate::db::{Connection, TransactionBehavior, params};
use crate::error::AppError;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ChannelSettingsRequest {
    pub order_mode: String,
    pub allowed_methods: Vec<String>,
    pub min_topup_minor: Option<i64>,
    pub max_topup_minor: Option<i64>,
    pub fee_rate_bps: i64,
    pub fixed_fee_minor: i64,
    pub expected_version: i64,
}

pub(super) fn validate_settings(input: &ChannelSettingsRequest) -> Result<Vec<String>, AppError> {
    let invalid = |message: &str| Err(AppError::BadRequest(message.into()));
    if !matches!(input.order_mode.as_str(), "cashier" | "direct") {
        return invalid("order mode must be cashier or direct");
    }
    if input.allowed_methods.len() > 32 {
        return invalid("at most 32 payment methods");
    }
    let mut methods = Vec::new();
    for method in &input.allowed_methods {
        if token_rail(method).is_none() {
            return invalid("unsupported DujiaoPay token");
        }
        if !methods.contains(method) {
            methods.push(method.clone());
        }
    }
    if input.order_mode == "direct" && methods.is_empty() {
        return invalid("direct checkout needs at least one payment method");
    }
    let bounded = |value: Option<i64>| {
        value.is_none_or(|amount| (MIN_TOPUP_MINOR..=MAX_TOPUP_MINOR).contains(&amount))
    };
    if !bounded(input.min_topup_minor) || !bounded(input.max_topup_minor) {
        return invalid("top-up bounds are outside the supported range");
    }
    if let (Some(min), Some(max)) = (input.min_topup_minor, input.max_topup_minor)
        && min > max
    {
        return invalid("minimum top-up exceeds the maximum");
    }
    if !(0..=10_000).contains(&input.fee_rate_bps) || !(0..=1_000_000).contains(&input.fixed_fee_minor)
    {
        return invalid("fee settings are out of range");
    }
    Ok(methods)
}

pub(super) fn update_channel_settings_tx(
    conn: &Connection,
    channel_id: &str,
    input: &ChannelSettingsRequest,
    time: &str,
) -> Result<(), AppError> {
    let methods = validate_settings(input)?;
    let methods_json = serde_json::to_string(&methods)
        .map_err(|_| AppError::Internal("encode allowed methods failed".into()))?;
    let changed = conn
        .execute(
            "UPDATE market_payment_channel_settings
             SET order_mode = ?2, allowed_methods_json = ?3, min_topup_minor = ?4,
                 max_topup_minor = ?5, fee_rate_bps = ?6, fixed_fee_minor = ?7,
                 version = version + 1, updated_at = ?8
             WHERE channel_id = ?1 AND version = ?9
               AND EXISTS (SELECT 1 FROM market_payment_channels c
                           WHERE c.id = ?1 AND c.archived_at IS NULL)",
            params![
                channel_id,
                input.order_mode,
                methods_json,
                input.min_topup_minor,
                input.max_topup_minor,
                input.fee_rate_bps,
                input.fixed_fee_minor,
                time,
                input.expected_version
            ],
        )
        .map_err(db_error("update payment channel settings"))?;
    if changed != 1 {
        return Err(AppError::Conflict(
            "payment channel settings changed or channel is archived".into(),
        ));
    }
    Ok(())
}

pub(super) async fn update_channel_settings(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ChannelSettingsRequest>,
) -> Result<(axum::http::HeaderMap, Json<ChannelView>), AppError> {
    let session = crate::api::require_owner_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin payment channel settings"))?;
    let time = now();
    update_channel_settings_tx(&tx, &id, &input, &time)?;
    audit_tx(
        &tx,
        &session.user_id,
        "channel_settings_updated",
        "payment_channel",
        &id,
        Some(&input.order_mode),
        &time,
    )?;
    tx.commit()
        .map_err(db_error("commit payment channel settings"))?;
    let mut headers = axum::http::HeaderMap::new();
    let [(name, value)] = no_store();
    headers.insert(name, axum::http::HeaderValue::from_static(value));
    Ok((headers, Json(channel_view_tx(&conn, &id)?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(order_mode: &str, methods: &[&str]) -> ChannelSettingsRequest {
        ChannelSettingsRequest {
            order_mode: order_mode.into(),
            allowed_methods: methods.iter().map(|m| m.to_string()).collect(),
            min_topup_minor: Some(500),
            max_topup_minor: Some(50_000),
            fee_rate_bps: 100,
            fixed_fee_minor: 0,
            expected_version: 1,
        }
    }

    #[test]
    fn settings_require_tabled_rails_and_coherent_bounds() {
        assert_eq!(
            validate_settings(&request("direct", &["tron-usdt", "tron-usdt"])).unwrap(),
            vec!["tron-usdt".to_string()]
        );
        assert!(validate_settings(&request("direct", &[])).is_err());
        assert!(validate_settings(&request("cashier", &[])).is_ok());
        assert!(validate_settings(&request("cashier", &["tron-usdt-trc20"])).is_err());
        assert!(validate_settings(&request("qr", &["tron-usdt"])).is_err());
        let mut inverted = request("cashier", &[]);
        inverted.min_topup_minor = Some(10_000);
        inverted.max_topup_minor = Some(500);
        assert!(validate_settings(&inverted).is_err());
        let mut too_small = request("cashier", &[]);
        too_small.min_topup_minor = Some(1);
        assert!(validate_settings(&too_small).is_err());
    }

    #[test]
    fn settings_edit_is_versioned_and_does_not_touch_rollout() {
        let conn = crate::test_support::database().unwrap();
        conn.execute(
            "INSERT INTO market_payment_channels
             (id, gateway_kind, display_name, enabled, current_revision, created_at, updated_at)
             VALUES ('channel', 'dujiaopay', 'Gateway', 1, 1, '2026-10-01', '2026-10-01')",
            [],
        )
        .unwrap();
        update_channel_settings_tx(&conn, "channel", &request("direct", &["tron-usdt"]), "t1")
            .unwrap();
        assert!(
            update_channel_settings_tx(&conn, "channel", &request("cashier", &[]), "t2").is_err()
        );
        let (mode, version, rollout): (String, i64, String) = conn
            .query_row(
                "SELECT s.order_mode, s.version, c.rollout_state
                 FROM market_payment_channel_settings s
                 JOIN market_payment_channels c ON c.id = s.channel_id",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((mode.as_str(), version, rollout.as_str()), ("direct", 2, "live"));
    }
}
