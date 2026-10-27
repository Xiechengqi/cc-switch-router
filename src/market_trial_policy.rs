//! Market trial policy is deliberately independent from wallet fee policy.
use crate::market_wallet_policy::db_error;
use crate::{
    ServerState,
    db::{Connection, TransactionBehavior, params},
    error::AppError,
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, header},
    routing::get,
};
use serde::{Deserialize, Serialize};

pub(crate) const MAX_HOURS: i64 = 720;
pub(crate) const MAX_TOKENS: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrialPolicy {
    pub version: i64,
    pub client_hours: i64,
    pub share_hours: i64,
    pub share_tokens: u64,
}

pub(crate) fn policy_tx(conn: &Connection) -> Result<TrialPolicy, AppError> {
    conn.query_row("SELECT version, client_hours, share_hours, share_tokens FROM market_trial_policy WHERE id = 1", [], |r| Ok(TrialPolicy {
        version: r.get(0)?, client_hours: r.get(1)?, share_hours: r.get(2)?, share_tokens: r.get(3)?,
    })).map_err(db_error("read market trial policy"))
}

pub(crate) fn validate(hours: i64, tokens: u64) -> Result<(), AppError> {
    if !(0..=MAX_HOURS).contains(&hours) || tokens > MAX_TOKENS {
        return Err(AppError::BadRequest(
            "trial hours must be 0–720 and Tokens must be a nonnegative safe integer".into(),
        ));
    }
    Ok(())
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/market-trials/policy", get(get_policy))
        .route(
            "/v1/admin/market/trial-policy",
            get(get_admin_policy).put(set_policy),
        )
}

async fn get_policy(
    State(state): State<ServerState>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let conn = state.store.conn.lock().await;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(policy_tx(&conn)?),
    ))
}

async fn get_admin_policy(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = crate::api::require_admin_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let mut value =
        serde_json::to_value(policy_tx(&conn)?).map_err(|e| AppError::Internal(e.to_string()))?;
    value["canEdit"] = serde_json::json!(crate::api::is_finance_owner(&state.config, &actor.email));
    Ok(([(header::CACHE_CONTROL, "private, no-store")], Json(value)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetPolicy {
    expected_version: i64,
    client_hours: i64,
    share_hours: i64,
    share_tokens: u64,
}

async fn set_policy(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<SetPolicy>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = crate::api::require_owner_session(&state, &headers).await?;
    validate(input.client_hours, input.share_tokens)?;
    validate(input.share_hours, input.share_tokens)?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin trial policy"))?;
    let before = policy_tx(&tx)?;
    let now = crate::market_wallet::now();
    if tx.execute("UPDATE market_trial_policy SET version = version + 1, client_hours = ?2, share_hours = ?3, share_tokens = ?4, updated_at = ?5 WHERE id = 1 AND version = ?1", params![input.expected_version, input.client_hours, input.share_hours, input.share_tokens, now]).map_err(db_error("update trial policy"))? != 1 {
        return Err(AppError::Conflict("trial policy changed; reload before saving".into()));
    }
    let after = policy_tx(&tx)?;
    tx.execute(
        "INSERT INTO market_trial_policy_events VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            uuid::Uuid::new_v4().to_string(),
            actor.user_id,
            serde_json::to_string(&before).map_err(|e| AppError::Internal(e.to_string()))?,
            serde_json::to_string(&after).map_err(|e| AppError::Internal(e.to_string()))?,
            now
        ],
    )
    .map_err(db_error("audit trial policy"))?;
    tx.commit().map_err(db_error("commit trial policy"))?;
    Ok(([(header::CACHE_CONTROL, "private, no-store")], Json(after)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trial_boundaries_are_identical_for_all_products() {
        assert!(validate(0, 0).is_ok());
        assert!(validate(720, MAX_TOKENS).is_ok());
        assert!(validate(721, 1).is_err());
        assert!(validate(-1, 1).is_err());
        assert!(validate(12, MAX_TOKENS + 1).is_err());
    }
}
