//! Owner console read models: liability overview and a per-user finance view
//! looked up by email. Read-only; amounts are ledger units serialized as
//! strings like every other wallet API.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use super::{ServerState, TopupView, db_error, no_store, topups_tx, wallet_transactions_tx};
use crate::db::{Connection, OptionalExtension, params};
use crate::error::AppError;
use crate::market_wallet::serialize_units;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FinanceOverview {
    #[serde(serialize_with = "serialize_units")]
    wallet_posted_units: i64,
    #[serde(serialize_with = "serialize_units")]
    wallet_held_units: i64,
    #[serde(serialize_with = "serialize_units")]
    unfulfilled_units: i64,
    #[serde(serialize_with = "serialize_units")]
    provider_pending_units: i64,
    #[serde(serialize_with = "serialize_units")]
    provider_available_units: i64,
    #[serde(serialize_with = "serialize_units")]
    provider_held_units: i64,
    #[serde(serialize_with = "serialize_units")]
    buyer_refund_held_units: i64,
    #[serde(serialize_with = "serialize_units")]
    owner_loss_units: i64,
    /// Fee estimate from channel settings over credited top-ups, in cents.
    estimated_gateway_cost_minor: i64,
    credited_topup_minor: i64,
    open_cases: i64,
    open_critical_cases: i64,
    held_earning_lots: i64,
}

/// Each liability is the negated sum of its journal postings, so the overview
/// reads the immutable ledger rather than the cached projections.
fn overview_tx(conn: &Connection) -> Result<FinanceOverview, AppError> {
    let ledger = |kind: &str| -> Result<i64, AppError> {
        conn.query_row(
            "SELECT -COALESCE(SUM(signed_units), 0) FROM market_journal_postings
             WHERE account_kind = ?1",
            params![kind],
            |row| row.get::<_, i64>(0),
        )
        .map_err(db_error("sum ledger account"))
    };
    let (posted, held): (i64, i64) = conn
        .query_row(
            "SELECT COALESCE(SUM(posted_units), 0), COALESCE(SUM(held_units), 0)
             FROM market_wallet_accounts",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error("sum wallet accounts"))?;
    let (credited, cost): (i64, i64) = conn
        .query_row(
            "SELECT COALESCE(SUM(t.amount_minor), 0),
                    COALESCE(SUM(t.amount_minor * COALESCE(s.fee_rate_bps, 0) / 10000
                                 + COALESCE(s.fixed_fee_minor, 0)), 0)
             FROM market_topup_orders t
             LEFT JOIN market_payment_channel_settings s ON s.channel_id = t.channel_id
             WHERE t.status = 'credited'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error("sum credited top-ups"))?;
    let (open_cases, held_lots): (i64, i64) = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM market_finance_reconciliation_cases WHERE status = 'open'),
                    (SELECT COUNT(*) FROM market_provider_earning_lots WHERE disputed = 1)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error("count open finance work"))?;
    Ok(FinanceOverview {
        wallet_posted_units: posted,
        wallet_held_units: held,
        unfulfilled_units: ledger("unfulfilled")?,
        provider_pending_units: ledger("provider_pending")?,
        provider_available_units: ledger("provider_available")?,
        provider_held_units: ledger("provider_held")?,
        buyer_refund_held_units: ledger("buyer_refund_held")?,
        // owner_loss is a debit balance (cost), so it is not negated.
        owner_loss_units: -ledger("owner_loss")?,
        estimated_gateway_cost_minor: cost,
        credited_topup_minor: credited,
        open_cases,
        open_critical_cases: crate::market_reconcile::critical_case_count_tx(conn)?,
        held_earning_lots: held_lots,
    })
}

pub(super) async fn finance_overview(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::api::require_admin_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((no_store(), Json(overview_tx(&conn)?)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UserLookupQuery {
    email: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UserFinanceView {
    user_id: String,
    email: String,
    wallet: crate::market_wallet::WalletSnapshot,
    provider: crate::market_finance::ProviderFinanceSummary,
    transactions: Vec<serde_json::Value>,
    topups: Vec<TopupView>,
    earning_holds: Vec<crate::market_finance::EarningHoldView>,
}

pub(super) async fn user_finance_lookup(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<UserLookupQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::api::require_admin_session(&state, &headers).await?;
    let email = query.email.trim().to_ascii_lowercase();
    if email.is_empty() || email.len() > 320 {
        return Err(AppError::BadRequest("email is required".into()));
    }
    let conn = state.store.conn.lock().await;
    let user_id: String = conn
        .query_row(
            "SELECT id FROM users WHERE email_normalized = ?1",
            params![email],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error("find user by email"))?
        .ok_or_else(|| AppError::NotFound("no user with this email".into()))?;
    let transactions = wallet_transactions_tx(&conn, &user_id, None)?
        .items
        .into_iter()
        .map(|item| serde_json::to_value(item).unwrap_or_default())
        .collect();
    Ok((
        no_store(),
        Json(UserFinanceView {
            wallet: crate::market_wallet::wallet_snapshot_tx(&conn, &user_id)?,
            provider: crate::market_finance::provider_summary_tx(&conn, &user_id)?,
            transactions,
            topups: topups_tx(&conn, &user_id, None)?.items,
            earning_holds: crate::market_finance::active_earning_holds_tx(&conn, Some(&user_id))?,
            user_id,
            email,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_reads_liabilities_from_the_ledger() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::apply(&conn).unwrap();
        crate::market_wallet::credit_verified_topup_tx(
            &conn,
            "buyer",
            "order",
            250,
            "receipt",
            "2026-10-01T00:00:00Z",
        )
        .unwrap();
        let overview = overview_tx(&conn).unwrap();
        assert_eq!(
            overview.wallet_posted_units,
            250 * crate::market_billing::MONEY_UNITS_PER_MINOR
        );
        assert_eq!(overview.owner_loss_units, 0);
        assert_eq!(overview.open_critical_cases, 0);
    }
}
