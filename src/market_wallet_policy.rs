//! One versioned policy for consumption wallets. Channel costs are not the
//! buyer's maintenance fee; historical orders never read today's fee rate.
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, header},
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::{
    ServerState,
    db::{Connection, TransactionBehavior, params},
    error::AppError,
};

pub(crate) fn db_error(context: &'static str) -> impl FnOnce(crate::db::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WalletPolicy {
    pub version: i64,
    pub maintenance_fee_bps: i64,
    pub transfers_enabled: bool,
    pub transfers_paused: bool,
    pub transfer_max_minor: i64,
    pub transfer_daily_max_minor: i64,
    pub balance_refundable: bool,
}

pub(crate) fn policy_tx(conn: &Connection) -> Result<WalletPolicy, AppError> {
    conn.query_row(
        "SELECT version, maintenance_fee_bps, transfers_enabled, transfers_paused,
                transfer_max_minor, transfer_daily_max_minor
         FROM market_wallet_policy WHERE id = 1",
        [],
        |row| {
            Ok(WalletPolicy {
                version: row.get(0)?,
                maintenance_fee_bps: row.get(1)?,
                transfers_enabled: row.get::<_, i64>(2)? == 1,
                transfers_paused: row.get::<_, i64>(3)? == 1,
                transfer_max_minor: row.get(4)?,
                transfer_daily_max_minor: row.get(5)?,
                balance_refundable: false,
            })
        },
    )
    .map_err(db_error("read wallet policy"))
}

/// Nonnegative, round-half-up to a cent, using a wide intermediate.
pub(crate) fn topup_amounts(credit_minor: i64, rate_bps: i64) -> Result<(i64, i64), AppError> {
    if credit_minor <= 0 || !(0..=10_000).contains(&rate_bps) {
        return Err(AppError::BadRequest(
            "invalid top-up amount or maintenance rate".into(),
        ));
    }
    let fee = (i128::from(credit_minor) * i128::from(rate_bps) + 5_000) / 10_000;
    let fee = i64::try_from(fee).map_err(|_| AppError::BadRequest("top-up fee overflow".into()))?;
    let pay = credit_minor
        .checked_add(fee)
        .ok_or_else(|| AppError::BadRequest("top-up total overflow".into()))?;
    crate::market_wallet::units_from_minor(pay)?;
    Ok((fee, pay))
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/market-wallet/policy", get(get_policy))
        .route(
            "/v1/admin/market-finance/wallet-policy",
            get(get_admin_policy).put(set_policy),
        )
}

async fn get_policy(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::api::resolve_router_session(&state, &headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))?;
    let conn = state.store.conn.lock().await;
    Ok((
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(policy_tx(&conn)?),
    ))
}

async fn get_admin_policy(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = crate::api::require_admin_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let mut policy = serde_json::to_value(policy_tx(&conn)?)
        .map_err(|_| AppError::Internal("encode wallet policy".into()))?;
    policy["canEdit"] =
        serde_json::json!(crate::api::is_finance_owner(&state.config, &actor.email));
    Ok(([(header::CACHE_CONTROL, "private, no-store")], Json(policy)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetPolicyRequest {
    expected_version: i64,
    maintenance_fee_bps: i64,
    transfers_enabled: bool,
    transfers_paused: bool,
    transfer_max_minor: i64,
    transfer_daily_max_minor: i64,
}

async fn set_policy(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<SetPolicyRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = crate::api::require_owner_session(&state, &headers).await?;
    if !(0..=10_000).contains(&input.maintenance_fee_bps)
        || !(1..=10_000_000).contains(&input.transfer_max_minor)
        || !(input.transfer_max_minor..=10_000_000).contains(&input.transfer_daily_max_minor)
    {
        return Err(AppError::BadRequest("invalid wallet policy limits".into()));
    }
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin wallet policy"))?;
    let previous = policy_tx(&tx)?;
    let time = crate::market_wallet::now();
    let changed = tx
        .execute(
            "UPDATE market_wallet_policy SET version = version + 1, maintenance_fee_bps = ?2,
         transfers_enabled = ?3, transfers_paused = ?4, transfer_max_minor = ?5,
         transfer_daily_max_minor = ?6, updated_at = ?7
         WHERE id = 1 AND version = ?1",
            params![
                input.expected_version,
                input.maintenance_fee_bps,
                input.transfers_enabled,
                input.transfers_paused,
                input.transfer_max_minor,
                input.transfer_daily_max_minor,
                time
            ],
        )
        .map_err(db_error("update wallet policy"))?;
    if changed != 1 {
        return Err(AppError::Conflict(
            "wallet policy version changed; reload before saving".into(),
        ));
    }
    let current = policy_tx(&tx)?;
    crate::market_notifications::owner_signal_tx(
        &tx,
        "wallet_policy_changed",
        "wallet_policy",
        &current.version.to_string(),
        serde_json::json!({ "actorUserId": actor.user_id, "before": previous, "after": current }),
        &time,
        false,
    )?;
    // The durable audit payload is kept with the existing financial audit.
    tx.execute(
        "INSERT INTO market_wallet_policy_events (id, actor_user_id, before_json, after_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![uuid::Uuid::new_v4().to_string(), actor.user_id,
            serde_json::to_string(&previous).map_err(|_| AppError::Internal("encode wallet policy".into()))?,
            serde_json::to_string(&current).map_err(|_| AppError::Internal("encode wallet policy".into()))?, time],
    ).map_err(db_error("audit wallet policy"))?;
    tx.commit().map_err(db_error("commit wallet policy"))?;
    Ok((
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(current),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maintenance_fee_is_additive_and_rounds_half_up() {
        assert_eq!(topup_amounts(1000, 500).unwrap(), (50, 1050));
        assert_eq!(topup_amounts(110, 500).unwrap(), (6, 116));
        assert_eq!(topup_amounts(100, 0).unwrap(), (0, 100));
        assert_eq!(topup_amounts(1, 1).unwrap(), (0, 1));
        assert!(topup_amounts(i64::MAX, 10_000).is_err());
        assert!(topup_amounts(0, 500).is_err());
    }
}
