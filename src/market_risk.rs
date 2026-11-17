//! Bounded aggregate exposure; every admission is rechecked in its money
//! transaction. Policy changes never rewrite historical journals or evidence.
use crate::{
    ServerState,
    db::{Connection, TransactionBehavior, params},
    error::AppError,
    market_wallet_policy::db_error,
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, header},
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RiskPolicy {
    pub version: i64,
    pub topup_user_day_minor: i64,
    pub topup_channel_day_minor: i64,
    pub topup_global_day_minor: i64,
    pub topup_global_hour_minor: i64,
    pub incoming_user_day_minor: i64,
    pub chain_verification_required: bool,
    pub topup_circuit_open: bool,
}
pub(crate) fn policy_tx(conn: &Connection) -> Result<RiskPolicy, AppError> {
    conn.query_row("SELECT version,topup_user_day_minor,topup_channel_day_minor,topup_global_day_minor,topup_global_hour_minor,incoming_user_day_minor,chain_verification_required,topup_circuit_open FROM market_financial_risk_policy WHERE id=1",[],|r|Ok(RiskPolicy{version:r.get(0)?,topup_user_day_minor:r.get(1)?,topup_channel_day_minor:r.get(2)?,topup_global_day_minor:r.get(3)?,topup_global_hour_minor:r.get(4)?,incoming_user_day_minor:r.get(5)?,chain_verification_required:r.get(6)?,topup_circuit_open:r.get(7)?})).map_err(db_error("read wallet risk policy"))
}
fn limit(code: &'static str, message: &str) -> AppError {
    AppError::coded_conflict(code, message, json!({}))
}
pub(crate) fn require_topup_admission_tx(
    conn: &Connection,
    user: &str,
    channel: &str,
    minor: i64,
    time: &str,
) -> Result<(), AppError> {
    let p = policy_tx(conn)?;
    if p.topup_circuit_open {
        return Err(limit(
            "TOPUP_CIRCUIT_OPEN",
            "new top-ups are paused pending financial review",
        ));
    }
    let (own,ch,all,hour):(i64,i64,i64,i64)=conn.query_row("SELECT COALESCE(SUM(CASE WHEN o.user_id=?1 THEN o.amount_minor ELSE 0 END),0),COALESCE(SUM(CASE WHEN o.channel_id=?2 THEN o.amount_minor ELSE 0 END),0),COALESCE(SUM(o.amount_minor),0),COALESCE(SUM(CASE WHEN r.order_id IS NULL OR julianday(r.observed_at)>julianday(?3)-1.0/24 THEN o.amount_minor ELSE 0 END),0) FROM market_topup_orders o LEFT JOIN market_topup_receipts r ON r.order_id=o.id WHERE (r.order_id IS NOT NULL AND julianday(r.observed_at)>julianday(?3)-1) OR (r.order_id IS NULL AND o.status NOT IN ('expired','cancelled','failed','credited'))",params![user,channel,time],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(db_error("sum reserved top-up exposure"))?;
    for (used, max) in [
        (own, p.topup_user_day_minor),
        (ch, p.topup_channel_day_minor),
        (all, p.topup_global_day_minor),
        (hour, p.topup_global_hour_minor),
    ] {
        if used.checked_add(minor).is_none_or(|v| v > max) {
            return Err(limit(
                "TOPUP_AGGREGATE_LIMIT",
                "top-up exposure limit reached; wait for the rolling limit or ask the owner to review the policy",
            ));
        }
    }
    Ok(())
}
/// Return a review reason, instead of failing the transaction, so the circuit
/// trip and its notification survive together with the paid-order observation.
pub(crate) fn settlement_review_tx(
    conn: &Connection,
    order: &str,
    channel: &str,
    minor: i64,
    time: &str,
) -> Result<Option<&'static str>, AppError> {
    let p = policy_tx(conn)?;
    if p.topup_circuit_open {
        return Ok(Some("topup_circuit_open"));
    }
    let (day,ch,hour,own):(i64,i64,i64,i64)=conn.query_row("SELECT COALESCE(SUM(o.amount_minor),0),COALESCE(SUM(CASE WHEN o.channel_id=?1 THEN o.amount_minor ELSE 0 END),0),COALESCE(SUM(CASE WHEN julianday(r.observed_at)>julianday(?2)-1.0/24 THEN o.amount_minor ELSE 0 END),0),COALESCE(SUM(CASE WHEN o.user_id=(SELECT user_id FROM market_topup_orders WHERE id=?3) THEN o.amount_minor ELSE 0 END),0) FROM market_topup_orders o JOIN market_topup_receipts r ON r.order_id=o.id WHERE julianday(r.observed_at)>julianday(?2)-1",params![channel,time,order],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(db_error("sum credited top-up velocity"))?;
    if own
        .checked_add(minor)
        .is_none_or(|n| n > p.topup_user_day_minor)
    {
        return Ok(Some("topup_user_credit_limit"));
    }
    let exceeded = [
        (day, p.topup_global_day_minor),
        (ch, p.topup_channel_day_minor),
        (hour, p.topup_global_hour_minor),
    ]
    .into_iter()
    .any(|(used, max)| used.checked_add(minor).is_none_or(|v| v > max));
    if !exceeded {
        return Ok(None);
    }
    conn.execute("UPDATE market_financial_risk_policy SET topup_circuit_open=1,version=version+1,updated_at=?1 WHERE id=1",params![time]).map_err(db_error("trip top-up velocity circuit"))?;
    conn.execute("INSERT OR IGNORE INTO market_financial_risk_events(id,kind,subject_id,object_id,details_json,created_at) VALUES(?1,'topup_circuit_trip','global',?2,?3,?4)",params![Uuid::new_v4().to_string(),order,json!({"channel":channel,"dayMinor":day,"hourMinor":hour,"incomingMinor":minor,"policyVersion":p.version}).to_string(),time]).map_err(db_error("audit top-up circuit trip"))?;
    crate::market_notifications::owner_signal_tx(
        conn,
        "topup_risk_circuit",
        "topup",
        order,
        json!({"reason":"aggregate_credit_velocity"}),
        time,
        false,
    )?;
    Ok(Some("topup_velocity_exceeded"))
}
pub(crate) fn require_incoming_tx(
    conn: &Connection,
    user: &str,
    units: i64,
    time: &str,
    include_reservations: bool,
) -> Result<(), AppError> {
    let p = policy_tx(conn)?;
    let used:i64=conn.query_row("SELECT (SELECT COALESCE(SUM(amount_units),0) FROM market_wallet_transfers WHERE recipient_user_id=?1 AND julianday(created_at)>julianday(?2)-1)+(SELECT COALESCE(SUM(amount_units),0) FROM market_c2c_orders WHERE buyer_id=?1 AND ((status='released' AND julianday(released_at)>julianday(?2)-1) OR (?3=1 AND status IN ('awaiting_seller','awaiting_payment','paid','disputed'))))",params![user,time,include_reservations],|r|r.get(0)).map_err(db_error("sum wallet incoming risk"))?;
    let max = crate::market_wallet::units_from_minor(p.incoming_user_day_minor)?;
    if used.checked_add(units).is_none_or(|v| v > max) {
        return Err(limit(
            "WALLET_INCOMING_LIMIT",
            "recipient's rolling incoming credit limit would be exceeded",
        ));
    }
    Ok(())
}
pub fn router() -> Router<ServerState> {
    Router::new().route(
        "/v1/admin/market-finance/risk-policy",
        get(get_policy).put(set_policy),
    )
}
async fn get_policy(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::api::require_owner_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(policy_tx(&conn)?),
    ))
}
async fn set_policy(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<RiskPolicy>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = crate::market_finance_auth::write_owner_session(&state, &headers).await?;
    for value in [
        input.topup_user_day_minor,
        input.topup_channel_day_minor,
        input.topup_global_day_minor,
        input.topup_global_hour_minor,
        input.incoming_user_day_minor,
    ] {
        if !(100..=1_000_000_000).contains(&value) {
            return Err(AppError::BadRequest(
                "risk limits must be between US$1 and US$10 million".into(),
            ));
        }
    }
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin risk policy update"))?;
    let time = crate::market_wallet::now();
    crate::market_finance_auth::require_live_session_tx(&tx, &actor, &time)?;
    let before = policy_tx(&tx)?;
    if tx.execute("UPDATE market_financial_risk_policy SET version=version+1,topup_user_day_minor=?2,topup_channel_day_minor=?3,topup_global_day_minor=?4,topup_global_hour_minor=?5,incoming_user_day_minor=?6,chain_verification_required=?7,topup_circuit_open=?8,updated_at=?9 WHERE id=1 AND version=?1",params![input.version,input.topup_user_day_minor,input.topup_channel_day_minor,input.topup_global_day_minor,input.topup_global_hour_minor,input.incoming_user_day_minor,input.chain_verification_required,input.topup_circuit_open,time]).map_err(db_error("save wallet risk policy"))?!=1{return Err(AppError::Conflict("risk policy changed; reload".into()))}
    conn.execute("INSERT INTO market_financial_risk_events(id,kind,subject_id,object_id,details_json,created_at) VALUES(?1,'risk_policy_changed',?2,?1,?3,?4)",params![Uuid::new_v4().to_string(),actor.user_id,json!({"before":before,"after":input,"proofId":actor.financial_proof_id}).to_string(),time]).map_err(db_error("audit risk policy update"))?;
    tx.commit().map_err(db_error("commit risk policy"))?;
    Ok((
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(policy_tx(&conn)?),
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incoming_limit_counts_transfers_and_pending_c2c() {
        let c = crate::test_support::database().unwrap();
        c.execute(
            "UPDATE market_financial_risk_policy SET incoming_user_day_minor=100 WHERE id=1",
            [],
        )
        .unwrap();
        assert!(require_incoming_tx(&c, "u", 8_640_000, "2026-11-17T00:00:00Z", true).is_ok());
        assert!(require_incoming_tx(&c, "u", 8_640_001, "2026-11-17T00:00:00Z", true).is_err());
    }
}
