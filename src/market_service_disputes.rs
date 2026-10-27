//! Disputes of wallet-funded service are an Owner-reviewed financial hold.
//! Only earnings that have not been allocated to an external payout can be
//! returned to the buyer. Every refund is a new balanced journal entry.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, header};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ServerState;
use crate::db::{Connection, OptionalExtension, TransactionBehavior, params};
use crate::error::AppError;
use crate::market_wallet::{
    credit_wallet_from_liability_tx, now, serialize_units, units_from_minor,
};
use crate::models::AuthSession;

fn db_error(context: &'static str) -> impl FnOnce(crate::db::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

fn no_store() -> [(header::HeaderName, &'static str); 1] {
    [(header::CACHE_CONTROL, "private, no-store")]
}

async fn require_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))
}

fn validate_reason_code(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(AppError::BadRequest("invalid dispute reason code".into()));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DisputeView {
    pub(crate) id: String,
    contract_id: String,
    buyer_user_id: String,
    provider_user_id: String,
    reason_code: String,
    status: String,
    resolution_code: Option<String>,
    #[serde(serialize_with = "serialize_units")]
    refunded_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pub(crate) refundable_units: i64,
    version: i64,
    created_at: String,
    resolved_at: Option<String>,
}

fn view_tx(conn: &Connection, id: &str) -> Result<DisputeView, AppError> {
    let mut view = conn
        .query_row(
            "SELECT id, contract_id, buyer_user_id, provider_user_id, reason_code,
                status, resolution_code, refunded_units, version, created_at, resolved_at
         FROM market_service_disputes WHERE id = ?1",
            params![id],
            |row| {
                Ok(DisputeView {
                    id: row.get(0)?,
                    contract_id: row.get(1)?,
                    buyer_user_id: row.get(2)?,
                    provider_user_id: row.get(3)?,
                    reason_code: row.get(4)?,
                    status: row.get(5)?,
                    resolution_code: row.get(6)?,
                    refunded_units: row.get(7)?,
                    refundable_units: 0,
                    version: row.get(8)?,
                    created_at: row.get(9)?,
                    resolved_at: row.get(10)?,
                })
            },
        )
        .map_err(db_error("read service dispute"))?;
    view.refundable_units = refundable_lots_tx(conn, id)?
        .iter()
        .try_fold(0_i64, |sum, lot| {
            sum.checked_add(lot.refundable_gross())
                .ok_or_else(|| AppError::Internal("service refund capacity overflow".into()))
        })?;
    Ok(view)
}

pub(crate) fn open_dispute_tx(
    conn: &Connection,
    contract_id: &str,
    buyer: &str,
    reason: &str,
    time: &str,
) -> Result<DisputeView, AppError> {
    validate_reason_code(reason)?;
    let (stored_buyer, provider, status): (String, String, String) = conn
        .query_row(
            "SELECT buyer_user_id, provider_user_id, status
             FROM market_platform_contracts WHERE id = ?1",
            params![contract_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(db_error("read disputed wallet contract"))?;
    if stored_buyer != buyer {
        return Err(AppError::Forbidden(
            "wallet contract belongs to another buyer".into(),
        ));
    }
    if matches!(
        status.as_str(),
        "pending_activation" | "activation_failed" | "trialing"
    ) {
        return Err(AppError::Conflict(
            "wallet contract has no paid service to dispute".into(),
        ));
    }
    let existing: Option<(String, String)> = conn
        .query_row(
            "SELECT id, reason_code FROM market_service_disputes
             WHERE contract_id = ?1 AND status = 'open'",
            params![contract_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(db_error("read open service dispute"))?;
    if let Some((id, stored_reason)) = existing {
        if stored_reason != reason {
            return Err(AppError::Conflict("service dispute is already open".into()));
        }
        return view_tx(conn, &id);
    }
    let previously_resolved: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM market_service_disputes
             WHERE contract_id = ?1 AND status = 'resolved')",
            params![contract_id],
            |row| row.get(0),
        )
        .map_err(db_error("read latest service dispute resolution"))?;
    // A partial refund may be supplemented without inventing new delivery.
    // A reviewed no-refund decision still requires new service evidence.
    // Keep earlier no-refund decisions protected even after a later partial
    // refund, which must not reopen already adjudicated older deliveries.
    let latest_resolution: Option<String> = conn
        .query_row(
            "SELECT resolved_at FROM market_service_disputes
         WHERE contract_id = ?1 AND status = 'resolved' AND refunded_units = 0
         ORDER BY julianday(resolved_at) DESC, id DESC LIMIT 1",
            params![contract_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error("read protected service dispute resolution"))?;
    if let Some(resolved_at) = latest_resolution.as_deref() {
        let new_delivery: i64 = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM market_delivery_slices
                 WHERE contract_id = ?1 AND julianday(created_at) > julianday(?2))",
                params![contract_id, resolved_at],
                |row| row.get(0),
            )
            .map_err(db_error("check delivery after service dispute resolution"))?;
        if new_delivery == 0 {
            return Err(AppError::Conflict(
                "service dispute was already reviewed; new delivery is required to reopen it"
                    .into(),
            ));
        }
    }
    if previously_resolved {
        let outstanding: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM market_provider_earning_lots lot
             JOIN market_delivery_slices slice ON slice.id = lot.delivery_slice_id
             WHERE slice.contract_id = ?1
               AND lot.amount_units > lot.reversed_units + lot.commission_reversed_units
               AND (?2 IS NULL OR julianday(slice.created_at) > julianday(?2)))",
                params![contract_id, latest_resolution],
                |row| row.get(0),
            )
            .map_err(db_error("check remaining disputed service"))?;
        if !outstanding {
            return Err(AppError::Conflict(
                "service earnings were fully refunded; new delivery is required to reopen a dispute".into(),
            ));
        }
    }
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO market_service_disputes
            (id, contract_id, buyer_user_id, provider_user_id, reason_code,
             status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'open', ?6)",
        params![id, contract_id, buyer, provider, reason, time],
    )
    .map_err(db_error("open service dispute"))?;
    conn.execute(
        "INSERT INTO market_service_dispute_lots (dispute_id, earning_lot_id)
         SELECT ?1, lot.id FROM market_provider_earning_lots lot
         JOIN market_delivery_slices slice ON slice.id = lot.delivery_slice_id
         WHERE slice.contract_id = ?2
           AND lot.amount_units - lot.reversed_units - lot.commission_reversed_units > 0
           AND (?3 IS NULL OR julianday(slice.created_at) > julianday(?3))",
        params![id, contract_id, latest_resolution],
    )
    .map_err(db_error("record earnings frozen by service dispute"))?;
    // Holds stack: a lot already held for another cause is registered too,
    // so resolving either cause alone never releases it early.
    crate::market_finance::hold_dispute_earnings_tx(conn, &id, time)?;
    conn.execute(
        "INSERT INTO market_finance_audit_events
            (id, actor_user_id, action, object_kind, object_id, reason_code, created_at)
         VALUES (?1, ?2, 'open', 'service_dispute', ?3, ?4, ?5)",
        params![Uuid::new_v4().to_string(), buyer, id, reason, time],
    )
    .map_err(db_error("audit service dispute opening"))?;
    view_tx(conn, &id)
}

#[derive(Debug)]
struct RefundableLot {
    id: String,
    amount: i64,
    reversed: i64,
    unlocked: i64,
    held: i64,
    paid: i64,
    converted: i64,
    fee: i64,
    fee_reversed: i64,
    fee_unlocked: i64,
}

impl RefundableLot {
    fn net(&self) -> i64 {
        self.amount - self.fee
    }
    fn refunded_gross(&self) -> i64 {
        self.reversed + self.fee_reversed
    }
    fn split_refund(&self, gross: i64) -> (i64, i64) {
        let cumulative = self.refunded_gross() + gross;
        let total_fee = ((i128::from(cumulative) * i128::from(self.fee)
            + i128::from(self.amount / 2))
            / i128::from(self.amount)) as i64;
        let fee = total_fee - self.fee_reversed;
        (gross - fee, fee)
    }
    // Allocated/paid Provider net cannot be clawed back. Find the exact gross
    // capacity at the original split (including 0/100% and tiny rounding).
    fn refundable_gross(&self) -> i64 {
        let capacity = self.net() - self.reversed - self.held - self.paid - self.converted;
        let mut low = 0;
        let mut high = self.amount - self.refunded_gross();
        while low < high {
            let mid = low + (high - low) / 2 + (high - low) % 2;
            if self.split_refund(mid).0 <= capacity {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        low
    }
}

fn refundable_lots_tx(conn: &Connection, dispute_id: &str) -> Result<Vec<RefundableLot>, AppError> {
    let mut statement = conn
        .prepare(
            "SELECT lot.id, lot.amount_units, lot.reversed_units, lot.unlocked_units,
                    lot.held_units, lot.paid_units, lot.commission_units,
                    lot.commission_reversed_units, lot.commission_unlocked_units, lot.converted_units
             FROM market_service_dispute_lots scope
             JOIN market_provider_earning_lots lot ON lot.id = scope.earning_lot_id
             JOIN market_delivery_slices slice ON slice.id = lot.delivery_slice_id
             WHERE scope.dispute_id = ?1
             ORDER BY slice.ended_at DESC, lot.id DESC",
        )
        .map_err(db_error("prepare service refund lots"))?;
    statement
        .query_map(params![dispute_id], |row| {
            Ok(RefundableLot {
                id: row.get(0)?,
                amount: row.get(1)?,
                reversed: row.get(2)?,
                unlocked: row.get(3)?,
                held: row.get(4)?,
                paid: row.get(5)?,
                converted: row.get(9)?,
                fee: row.get(6)?,
                fee_reversed: row.get(7)?,
                fee_unlocked: row.get(8)?,
            })
        })
        .map_err(db_error("query service refund lots"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read service refund lots"))
}

pub(crate) fn resolve_dispute_tx(
    conn: &Connection,
    dispute_id: &str,
    owner: &str,
    expected_version: i64,
    resolution: &str,
    refund_units: i64,
    time: &str,
) -> Result<DisputeView, AppError> {
    validate_reason_code(resolution)?;
    if refund_units < 0
        || (refund_units > 0 && refund_units % crate::market_common::MONEY_UNITS_PER_MINOR != 0)
    {
        return Err(AppError::BadRequest(
            "service refund must use exact USD cents".into(),
        ));
    }
    let dispute = view_tx(conn, dispute_id)?;
    if dispute.status != "open" || dispute.version != expected_version {
        return Err(AppError::Conflict("service dispute version changed".into()));
    }
    crate::market_reconcile::ensure_contract_finance_safe_tx(conn, &dispute.contract_id)?;
    let lots = refundable_lots_tx(conn, dispute_id)?;
    let refundable = lots.iter().try_fold(0_i64, |sum, lot| {
        let amount = lot.refundable_gross();
        sum.checked_add(amount)
            .ok_or_else(|| AppError::Internal("service refundable balance overflow".into()))
    })?;
    if refund_units > refundable {
        return Err(AppError::Conflict(
            "service adjustment requires Owner review: original Provider net is allocated, paid, or converted; cancel a pending payout or arrange recovery first".into(),
        ));
    }
    let mut left = refund_units;
    for lot in lots {
        if left == 0 {
            break;
        }
        if !crate::market_commission::lot_evidence_valid_tx(conn, &lot.id)? {
            return Err(AppError::Conflict(
                "service commission evidence requires Owner review".into(),
            ));
        }
        let taken = left.min(lot.refundable_gross());
        let (net_taken, fee_taken) = lot.split_refund(taken);
        let pending = lot.net() - lot.reversed - lot.unlocked;
        let available = lot.unlocked - lot.held - lot.paid - lot.converted;
        let from_pending = net_taken.min(pending);
        let from_available = (net_taken - from_pending).min(available);
        let fee_pending = lot.fee - lot.fee_reversed - lot.fee_unlocked;
        let fee_from_pending = fee_taken.min(fee_pending);
        let fee_from_revenue = fee_taken - fee_from_pending;
        if taken == 0 {
            continue;
        }
        if from_pending > 0 {
            let journal = credit_wallet_from_liability_tx(
                conn,
                &dispute.buyer_user_id,
                from_pending,
                "provider_pending",
                &dispute.provider_user_id,
                "service_dispute",
                dispute_id,
                &format!("refund_pending:{}", lot.id),
                time,
            )?;
            if !journal.inserted {
                return Err(AppError::Internal(
                    "service refund journal already exists".into(),
                ));
            }
        }
        if from_available > 0 {
            let journal = credit_wallet_from_liability_tx(
                conn,
                &dispute.buyer_user_id,
                from_available,
                "provider_available",
                &dispute.provider_user_id,
                "service_dispute",
                dispute_id,
                &format!("refund_available:{}", lot.id),
                time,
            )?;
            if !journal.inserted {
                return Err(AppError::Internal(
                    "service refund journal already exists".into(),
                ));
            }
        }
        for (fee, kind, action) in [
            (
                fee_from_pending,
                "platform_commission_pending",
                "refund_commission_pending",
            ),
            (
                fee_from_revenue,
                "platform_commission_revenue",
                "refund_commission_revenue",
            ),
        ] {
            if fee == 0 {
                continue;
            }
            let journal = credit_wallet_from_liability_tx(
                conn,
                &dispute.buyer_user_id,
                fee,
                kind,
                &lot.id,
                "service_dispute",
                dispute_id,
                &format!("{action}:{}", lot.id),
                time,
            )?;
            if !journal.inserted {
                return Err(AppError::Internal(
                    "commission refund journal already exists".into(),
                ));
            }
        }
        let changed = conn
            .execute(
                "UPDATE market_provider_earning_lots
                 SET reversed_units = reversed_units + ?2,
                     unlocked_units = unlocked_units - ?3,
                     commission_reversed_units = commission_reversed_units + ?9,
                     commission_unlocked_units = commission_unlocked_units - ?10
                 WHERE id = ?1 AND amount_units = ?4 AND reversed_units = ?5
                   AND unlocked_units = ?6 AND held_units = ?7 AND paid_units = ?8
                   AND disputed = 1 AND converted_units = ?11",
                params![
                    lot.id,
                    net_taken,
                    from_available,
                    lot.amount,
                    lot.reversed,
                    lot.unlocked,
                    lot.held,
                    lot.paid,
                    fee_taken,
                    fee_from_revenue,
                    lot.converted
                ],
            )
            .map_err(db_error("reverse disputed Provider earning"))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "service earning changed during refund".into(),
            ));
        }
        conn.execute(
            "INSERT INTO market_commission_refunds VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![dispute_id, lot.id, taken, net_taken, fee_taken, time],
        )
        .map_err(db_error("record service refund allocation"))?;
        left -= taken;
    }
    if left != 0 {
        return Err(AppError::Internal("service refund allocation drift".into()));
    }
    let changed = conn
        .execute(
            "UPDATE market_service_disputes
             SET status = 'resolved', resolution_code = ?2, refunded_units = ?3,
                 resolved_at = ?4, version = version + 1
             WHERE id = ?1 AND status = 'open' AND version = ?5",
            params![dispute_id, resolution, refund_units, time, expected_version],
        )
        .map_err(db_error("resolve service dispute"))?;
    if changed != 1 {
        return Err(AppError::Conflict("service dispute version changed".into()));
    }
    crate::market_finance::release_earning_holds_tx(
        conn, "dispute", dispute_id, owner, resolution, time,
    )?;
    conn.execute(
        "INSERT INTO market_finance_audit_events
            (id, actor_user_id, action, object_kind, object_id, reason_code, created_at)
         VALUES (?1, ?2, 'resolve', 'service_dispute', ?3, ?4, ?5)",
        params![
            Uuid::new_v4().to_string(),
            owner,
            dispute_id,
            resolution,
            time
        ],
    )
    .map_err(db_error("audit service dispute resolution"))?;
    view_tx(conn, dispute_id)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenRequest {
    reason_code: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolveRequest {
    expected_version: i64,
    resolution_code: String,
    refund_amount_minor: i64,
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route(
            "/v1/market-wallet/contracts/:id/disputes",
            post(open_dispute),
        )
        .route(
            "/v1/market-wallet/service-disputes",
            get(list_buyer_disputes),
        )
        .route(
            "/v1/admin/market-finance/service-disputes",
            get(list_owner_disputes),
        )
        .route(
            "/v1/admin/market-finance/service-disputes/:id/resolve",
            post(resolve_dispute),
        )
}

const OWNER_DISPUTE_REFUND_MINOR: i64 = 5_000;

async fn open_dispute(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<OpenRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin service dispute"))?;
    let time = now();
    let result = open_dispute_tx(&tx, &id, &session.user_id, &request.reason_code, &time)?;
    crate::market_notifications::owner_signal_tx(
        &tx,
        "dispute_opened",
        "service_dispute",
        &result.id,
        serde_json::json!({ "contractId": result.contract_id, "reason": result.reason_code }),
        &time,
        false,
    )?;
    tx.commit().map_err(db_error("commit service dispute"))?;
    Ok((no_store(), Json(result)))
}

async fn list_buyer_disputes(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let mut statement = conn
        .prepare(
            "SELECT id FROM market_service_disputes WHERE buyer_user_id = ?1
             ORDER BY created_at DESC, id DESC LIMIT 50",
        )
        .map_err(db_error("prepare buyer service disputes"))?;
    let ids = statement
        .query_map(params![session.user_id], |row| row.get::<_, String>(0))
        .map_err(db_error("query buyer service disputes"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read buyer service disputes"))?;
    let items = ids
        .iter()
        .map(|id| view_tx(&conn, id))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((no_store(), Json(items)))
}

async fn list_owner_disputes(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::api::require_admin_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let mut statement = conn
        .prepare(
            "SELECT id FROM market_service_disputes WHERE status = 'open'
             ORDER BY created_at, id LIMIT 200",
        )
        .map_err(db_error("prepare open service disputes"))?;
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(db_error("query open service disputes"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read open service disputes"))?;
    let items = ids
        .iter()
        .map(|id| view_tx(&conn, id))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((no_store(), Json(items)))
}

async fn resolve_dispute(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<ResolveRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = crate::api::require_admin_session(&state, &headers).await?;
    // Admins handle routine disputes; larger refunds move Owner money.
    if request.refund_amount_minor > OWNER_DISPUTE_REFUND_MINOR
        && !crate::api::is_finance_owner(&state.config, &session.email)
    {
        return Err(AppError::Forbidden(
            "refunds above $50 require the Router owner".into(),
        ));
    }
    let units = if request.refund_amount_minor == 0 {
        0
    } else {
        units_from_minor(request.refund_amount_minor)?
    };
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin service dispute resolution"))?;
    let result = resolve_dispute_tx(
        &tx,
        &id,
        &session.user_id,
        request.expected_version,
        &request.resolution_code,
        units,
        &now(),
    )?;
    crate::market_notifications::enqueue_user_notification_tx(
        &tx,
        "buyer",
        &result.buyer_user_id,
        "dispute_resolved",
        "service_dispute",
        &result.id,
        &format!("dispute_resolved:{}", result.id),
        serde_json::json!({
            "contractId": result.contract_id,
            "status": result.resolution_code,
            "amountMinor": request.refund_amount_minor,
        }),
        &now(),
    )?;
    tx.commit()
        .map_err(db_error("commit service dispute resolution"))?;
    Ok((no_store(), Json(result)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market_finance::{
        provider_summary_tx, record_monthly_delivery_tx, unlock_due_earnings_tx,
    };
    use crate::market_platform_contracts::{
        BillingModel, PrepareContract, activate_contract_tx, prepare_contract_tx,
    };
    use crate::market_wallet::{credit_verified_topup_tx, reconcile_wallet_tx, wallet_snapshot_tx};

    fn setup() -> (Connection, String) {
        let conn = crate::test_support::database().unwrap();
        let time = "2026-10-01T00:00:00+00:00";
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        credit_verified_topup_tx(&tx, "buyer", "topup", 1000, "receipt", time).unwrap();
        let contract_id = prepare_contract_tx(
            &tx,
            &PrepareContract {
                product_kind: "share",
                product_ref: "share-subscription",
                trial_scope_ref: "share",
                quote_id: "quote",
                buyer_user_id: "buyer",
                provider_user_id: "provider",
                offer_revision: 1,
                billing_model: BillingModel::CalendarMonth,
                price_minor: 500,
                auto_renew: false,
                trial_seconds: 0,
                trial_allowance_seconds: 0,
            },
            time,
        )
        .unwrap();
        activate_contract_tx(&tx, &contract_id, time, time).unwrap();
        let (period_id, debit_id, debited_at): (String, String, String) = tx.query_row(
            "SELECT id, debit_journal_id, debited_at FROM market_platform_periods WHERE contract_id = ?1",
            params![contract_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        record_monthly_delivery_tx(
            &tx,
            "share",
            &contract_id,
            &period_id,
            "delivered",
            "buyer",
            "provider",
            time,
            "2026-10-01T00:00:01+00:00",
            100 * 86_400,
            &debit_id,
            &debited_at,
            time,
        )
        .unwrap();
        tx.commit().unwrap();
        (conn, contract_id)
    }

    #[test]
    fn dispute_refunds_only_unpaid_earning_and_is_idempotent() {
        let (conn, contract_id) = setup();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let time = "2026-10-02T00:00:00+00:00";
        assert!(open_dispute_tx(&tx, &contract_id, "other", "service_unavailable", time).is_err());
        let dispute =
            open_dispute_tx(&tx, &contract_id, "buyer", "service_unavailable", time).unwrap();
        assert_eq!(dispute.refundable_units, 100 * 86_400);
        let replay =
            open_dispute_tx(&tx, &contract_id, "buyer", "service_unavailable", time).unwrap();
        assert_eq!(dispute.id, replay.id);
        assert!(open_dispute_tx(&tx, &contract_id, "buyer", "other_reason", time).is_err());
        assert_eq!(
            provider_summary_tx(&tx, "provider").unwrap().pending_units,
            100 * 86_400
        );
        let resolved = resolve_dispute_tx(
            &tx,
            &dispute.id,
            "owner",
            1,
            "partial_refund",
            50 * 86_400,
            time,
        )
        .unwrap();
        assert_eq!(resolved.refunded_units, 50 * 86_400);
        assert_eq!(resolved.refundable_units, 50 * 86_400);
        assert!(
            resolve_dispute_tx(
                &tx,
                &dispute.id,
                "owner",
                1,
                "partial_refund",
                50 * 86_400,
                time
            )
            .is_err()
        );
        assert_eq!(
            provider_summary_tx(&tx, "provider").unwrap().pending_units,
            50 * 86_400
        );
        assert_eq!(
            wallet_snapshot_tx(&tx, "buyer").unwrap().posted_units,
            550 * 86_400
        );
        reconcile_wallet_tx(&tx, "buyer").unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn matured_service_can_refund_available_earnings_but_not_payout_allocations() {
        let (conn, contract_id) = setup();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let time = "2026-10-09T00:00:00+00:00";
        unlock_due_earnings_tx(&tx, time, 10).unwrap();
        let dispute = open_dispute_tx(&tx, &contract_id, "buyer", "service_quality", time).unwrap();
        assert_eq!(dispute.refundable_units, 100 * 86_400);
        let lot_id: String = tx
            .query_row("SELECT id FROM market_provider_earning_lots", [], |row| {
                row.get(0)
            })
            .unwrap();
        // Simulate an already-reserved withdrawal with a balanced journal.
        crate::market_wallet::post_journal_tx(
            &tx,
            "test_payout",
            "reserved",
            "hold",
            "hold",
            &[
                crate::market_wallet::Posting {
                    kind: "provider_available",
                    subject_id: "provider",
                    signed_units: 40 * 86_400,
                },
                crate::market_wallet::Posting {
                    kind: "provider_held",
                    subject_id: "provider",
                    signed_units: -40 * 86_400,
                },
            ],
            time,
        )
        .unwrap();
        tx.execute(
            "UPDATE market_provider_earning_lots SET held_units = ?2 WHERE id = ?1",
            params![lot_id, 40 * 86_400],
        )
        .unwrap();
        assert_eq!(
            view_tx(&tx, &dispute.id).unwrap().refundable_units,
            60 * 86_400
        );
        assert!(
            resolve_dispute_tx(
                &tx,
                &dispute.id,
                "owner",
                1,
                "over_limit",
                61 * 86_400,
                time
            )
            .is_err()
        );
        let resolved = resolve_dispute_tx(
            &tx,
            &dispute.id,
            "owner",
            1,
            "partial_refund",
            60 * 86_400,
            time,
        )
        .unwrap();
        assert_eq!(resolved.refunded_units, 60 * 86_400);
        let summary = provider_summary_tx(&tx, "provider").unwrap();
        assert_eq!(summary.withdrawal_held_units, 40 * 86_400);
        assert_eq!(summary.available_units, 0);
        reconcile_wallet_tx(&tx, "buyer").unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn new_delivery_during_dispute_stays_frozen_until_resolution() {
        let (conn, contract_id) = setup();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let time = "2026-10-02T00:00:00+00:00";
        let dispute = open_dispute_tx(&tx, &contract_id, "buyer", "service_quality", time).unwrap();
        let (period_id, debit_id, debited_at): (String, String, String) = tx
            .query_row(
                "SELECT id, debit_journal_id, debited_at FROM market_platform_periods
                 WHERE contract_id = ?1",
                params![contract_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let slice = record_monthly_delivery_tx(
            &tx,
            "share",
            &contract_id,
            &period_id,
            "delivered_during_dispute",
            "buyer",
            "provider",
            "2026-10-01T00:00:01+00:00",
            "2026-10-01T00:00:02+00:00",
            86_400,
            &debit_id,
            &debited_at,
            time,
        )
        .unwrap();
        let frozen: i64 = tx
            .query_row(
                "SELECT lot.disputed FROM market_provider_earning_lots lot
                 WHERE lot.delivery_slice_id = ?1",
                params![slice],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(frozen, 1);
        resolve_dispute_tx(&tx, &dispute.id, "owner", 1, "no_refund", 0, time).unwrap();
        let remaining: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM market_provider_earning_lots lot
                 JOIN market_delivery_slices slice ON slice.id = lot.delivery_slice_id
                 WHERE slice.contract_id = ?1 AND lot.disputed = 1",
                params![contract_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
        tx.commit().unwrap();
    }

    #[test]
    fn resolved_dispute_cannot_repeatedly_freeze_the_same_delivery() {
        let (conn, contract_id) = setup();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let resolved_at = "2026-10-02T00:00:00+00:00";
        let dispute =
            open_dispute_tx(&tx, &contract_id, "buyer", "service_quality", resolved_at).unwrap();
        resolve_dispute_tx(&tx, &dispute.id, "owner", 1, "no_refund", 0, resolved_at).unwrap();
        assert!(matches!(
            open_dispute_tx(&tx, &contract_id, "buyer", "service_quality", resolved_at),
            Err(AppError::Conflict(_))
        ));
        let (period_id, debit_id, debited_at): (String, String, String) = tx
            .query_row(
                "SELECT id, debit_journal_id, debited_at FROM market_platform_periods
                 WHERE contract_id = ?1",
                params![contract_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        record_monthly_delivery_tx(
            &tx,
            "share",
            &contract_id,
            &period_id,
            "new_delivery_after_resolution",
            "buyer",
            "provider",
            "2026-10-01T00:00:01+00:00",
            "2026-10-01T00:00:02+00:00",
            86_400,
            &debit_id,
            &debited_at,
            "2026-10-03T00:00:00+00:00",
        )
        .unwrap();
        let reopened = open_dispute_tx(
            &tx,
            &contract_id,
            "buyer",
            "new_service_issue",
            "2026-10-03T00:00:01+00:00",
        )
        .unwrap();
        assert_ne!(reopened.id, dispute.id);
        assert_eq!(reopened.refundable_units, 86_400);
        let old_lot_frozen: i64 = tx
            .query_row(
                "SELECT lot.disputed FROM market_provider_earning_lots lot
                 JOIN market_delivery_slices slice ON slice.id = lot.delivery_slice_id
                 WHERE slice.contract_id = ?1 AND slice.source_id = 'delivered'",
                params![contract_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_lot_frozen, 0);
        tx.commit().unwrap();
    }

    #[test]
    fn resolving_dispute_does_not_release_an_existing_finance_case_hold() {
        let (conn, contract_id) = setup();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        crate::market_finance::hold_contract_earnings_tx(
            &tx,
            &contract_id,
            "cleanup_uncertain",
            "2026-10-02T00:00:00+00:00",
        )
        .unwrap();
        tx.execute(
            "INSERT INTO market_finance_reconciliation_cases
                (id, code, object_kind, object_id, status, created_at)
             VALUES ('case', 'wallet_cleanup_uncertain', 'platform_contract', ?1,
                     'open', '2026-10-02T00:00:00+00:00')",
            params![contract_id],
        )
        .unwrap();
        let dispute = open_dispute_tx(
            &tx,
            &contract_id,
            "buyer",
            "service_quality",
            "2026-10-02T00:00:00+00:00",
        )
        .unwrap();
        // The already-held lot is registered too: holds stack.
        let owned_lots: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM market_service_dispute_lots WHERE dispute_id = ?1",
                params![dispute.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(owned_lots, 1);
        resolve_dispute_tx(
            &tx,
            &dispute.id,
            "owner",
            1,
            "no_refund",
            0,
            "2026-10-02T01:00:00+00:00",
        )
        .unwrap();
        let held: i64 = tx
            .query_row(
                "SELECT disputed FROM market_provider_earning_lots",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(held, 1, "the cleanup hold outlives the dispute");
        // Closing the cleanup case releases the last hold.
        crate::market_reconcile::resolve_case_tx(
            &tx,
            "case",
            "owner",
            "cleanup_verified",
            "2026-10-03T00:00:00+00:00",
        )
        .unwrap();
        let held: i64 = tx
            .query_row(
                "SELECT disputed FROM market_provider_earning_lots",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(held, 0, "earnings are no longer frozen forever");
        tx.commit().unwrap();
    }
}
