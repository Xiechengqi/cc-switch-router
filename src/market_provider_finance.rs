//! Read-only delivery earnings. Matured net income belongs in the user's
//! consumption wallet; the Router never offers external Provider payouts.
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, header},
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::{
    ServerState,
    db::{Connection, OptionalExtension, params},
    error::AppError,
    market_wallet::serialize_units,
    market_wallet_policy::db_error,
};

fn no_store() -> [(header::HeaderName, &'static str); 1] {
    [(header::CACHE_CONTROL, "private, no-store")]
}

async fn session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<crate::models::AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))
}

// Stopping sales does not forfeit earned income. Disabled users and unresolved
// identities must not gain transferable funds via the settlement worker.
pub(crate) fn can_manage_earnings_tx(conn: &Connection, user: &str) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM market_provider_profiles p JOIN users u ON u.id = p.user_id
        WHERE p.user_id = ?1 AND p.claim_state = 'claimed' AND p.status != 'identity_conflict'
          AND u.status = 'active' AND u.email_verified_at IS NOT NULL)",
        params![user],
        |r| r.get(0),
    )
    .map_err(db_error("read earnings identity"))
}

/// Publishing needs a valid Provider identity, never a withdrawal address.
pub(crate) fn require_paid_offer_setup_tx(conn: &Connection, user: &str) -> Result<(), AppError> {
    let active: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM market_provider_profiles p JOIN users u ON u.id = p.user_id
        WHERE p.user_id = ?1 AND p.claim_state = 'claimed' AND p.status = 'active'
          AND u.status = 'active' AND u.email_verified_at IS NOT NULL)",
        params![user], |r| r.get(0)).map_err(db_error("read paid offer Provider identity"))?;
    if !active {
        return Err(AppError::Forbidden(
            "active verified Provider identity required".into(),
        ));
    }
    Ok(())
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/market-provider-finance/me", get(get_finance))
        .route("/v1/market-provider-finance/earnings", get(list_earnings))
        .route("/v1/market-provider-finance/holds", get(list_holds))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Summary {
    #[serde(serialize_with = "serialize_units")]
    gross_earned_units: i64,
    #[serde(serialize_with = "serialize_units")]
    commission_units: i64,
    #[serde(serialize_with = "serialize_units")]
    net_earned_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pending_units: i64,
    #[serde(serialize_with = "serialize_units")]
    ready_units: i64,
    #[serde(serialize_with = "serialize_units")]
    restricted_units: i64,
    #[serde(serialize_with = "serialize_units")]
    credited_units: i64,
}

async fn get_finance(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let user = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let summary = crate::market_finance::provider_summary_tx(&conn, &user.user_id)?;
    let (restricted,pending): (i64,i64) = conn.query_row("SELECT COALESCE(SUM(CASE WHEN disputed != 0
        THEN net_amount_units - reversed_units - paid_units - converted_units ELSE held_units END),0),
        COALESCE(SUM(CASE WHEN disputed = 0 THEN net_amount_units - reversed_units - unlocked_units ELSE 0 END),0)
        FROM market_provider_income_view WHERE provider_user_id = ?1", params![user.user_id], |r| Ok((r.get(0)?,r.get(1)?)))
        .map_err(db_error("read pending and restricted earnings"))?;
    let is_provider = can_manage_earnings_tx(&conn, &user.user_id)?;
    let has_finance_activity: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM market_provider_income_view WHERE provider_user_id = ?1)",
            params![user.user_id],
            |r| r.get(0),
        )
        .map_err(db_error("read earning activity"))?;
    Ok((
        no_store(),
        Json(serde_json::json!({
            "isProvider": is_provider,
            "canPublishPaidOffers": require_paid_offer_setup_tx(&conn, &user.user_id).is_ok(),
            "hasFinanceActivity": has_finance_activity,
            "automaticCredit": true,
            "summary": Summary {
                gross_earned_units: summary.gross_earned_units,
                commission_units: summary.commission_units,
                net_earned_units: summary.net_earned_units,
                pending_units: pending,
                ready_units: summary.withdrawable_units,
                restricted_units: restricted,
                credited_units: summary.converted_units,
            },
        })),
    ))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PageQuery {
    cursor: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Page<T> {
    items: Vec<T>,
    next_cursor: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Earning {
    id: String,
    income_kind: String,
    #[serde(serialize_with = "serialize_units")]
    amount_units: i64,
    #[serde(serialize_with = "serialize_units")]
    gross_units: i64,
    #[serde(serialize_with = "serialize_units")]
    commission_units: i64,
    #[serde(serialize_with = "serialize_units")]
    commission_reversed_units: i64,
    commission_rate_bps: i64,
    commission_policy_version: i64,
    #[serde(serialize_with = "serialize_units")]
    reversed_units: i64,
    #[serde(serialize_with = "serialize_units")]
    unlocked_units: i64,
    #[serde(serialize_with = "serialize_units")]
    held_units: i64,
    #[serde(serialize_with = "serialize_units")]
    credited_units: i64,
    fulfilled_at: String,
    unlock_at: String,
    disputed: bool,
}
fn earnings_tx(
    conn: &Connection,
    user: &str,
    cursor: Option<&str>,
) -> Result<Page<Earning>, AppError> {
    let cursor_at = cursor.map(|id| conn.query_row("SELECT created_at FROM market_provider_income_view WHERE id = ?1 AND provider_user_id = ?2",
        params![id,user], |r| r.get::<_,String>(0)).optional().map_err(db_error("read earnings cursor"))?
        .ok_or_else(|| AppError::BadRequest("invalid earnings cursor".into()))).transpose()?;
    let mut stmt = conn.prepare("SELECT l.id,l.net_amount_units,l.amount_units,l.commission_units,l.commission_reversed_units,
        l.commission_rate_bps,l.commission_policy_version,l.reversed_units,l.unlocked_units,l.held_units,l.converted_units,
        l.fulfilled_at,l.unlock_at,l.disputed,l.income_kind
        FROM market_provider_income_view l
        WHERE l.provider_user_id = ?1 AND (?2 IS NULL OR l.created_at < ?2 OR (l.created_at = ?2 AND l.id < ?3))
        ORDER BY l.created_at DESC,l.id DESC LIMIT 51").map_err(db_error("prepare earnings"))?;
    let mut items = stmt
        .query_map(params![user, cursor_at, cursor], |r| {
            Ok(Earning {
                id: r.get(0)?,
                income_kind: r.get(14)?,
                amount_units: r.get(1)?,
                gross_units: r.get(2)?,
                commission_units: r.get(3)?,
                commission_reversed_units: r.get(4)?,
                commission_rate_bps: r.get(5)?,
                commission_policy_version: r.get(6)?,
                reversed_units: r.get(7)?,
                unlocked_units: r.get(8)?,
                held_units: r.get(9)?,
                credited_units: r.get(10)?,
                fulfilled_at: r.get(11)?,
                unlock_at: r.get(12)?,
                disputed: r.get(13)?,
            })
        })
        .map_err(db_error("query earnings"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read earnings"))?;
    let more = items.len() > 50;
    items.truncate(50);
    Ok(Page {
        next_cursor: more.then(|| items.last().unwrap().id.clone()),
        items,
    })
}
async fn list_earnings(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(q): Query<PageQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let user = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((
        no_store(),
        Json(earnings_tx(&conn, &user.user_id, q.cursor.as_deref())?),
    ))
}
async fn list_holds(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let user = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((
        no_store(),
        Json(crate::market_finance::active_earning_holds_tx(
            &conn,
            Some(&user.user_id),
        )?),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paid_provider_needs_identity_but_never_a_payout_address() {
        let conn = crate::test_support::database().unwrap();
        assert!(require_paid_offer_setup_tx(&conn, "provider").is_err());
        conn.execute("INSERT INTO users (id,email_normalized,status,email_verified_at,created_at,last_login_at)
            VALUES ('provider','provider@example.test','active','t','t','t')",[]).unwrap();
        conn.execute("INSERT INTO market_provider_profiles(id,canonical_email,user_id,display_name,claim_state,status,created_at,updated_at)
            VALUES ('profile','provider@example.test','provider','Provider','claimed','active','t','t')",[]).unwrap();
        require_paid_offer_setup_tx(&conn, "provider").unwrap();
        let addresses: i64 = conn
            .query_row("SELECT COUNT(*) FROM market_payout_destinations", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(addresses, 0);
        conn.execute("UPDATE market_provider_profiles SET status='paused'", [])
            .unwrap();
        assert!(require_paid_offer_setup_tx(&conn, "provider").is_err());
        assert!(can_manage_earnings_tx(&conn, "provider").unwrap());
    }

    #[test]
    fn earning_pages_are_owner_scoped_and_report_credit_without_payout_capabilities() {
        let conn = crate::test_support::database().unwrap();
        let at = "2026-10-01T00:00:00Z";
        for user in ["provider", "buyer", "other"] {
            conn.execute("INSERT INTO users(id,email_normalized,status,email_verified_at,created_at,last_login_at)
                VALUES(?1,?2,'active',?3,?3,?3)", params![user,format!("{user}@example.test"),at]).unwrap();
        }
        crate::market_wallet::credit_verified_topup_tx(&conn, "buyer", "topup", 100, "receipt", at)
            .unwrap();
        for index in 0..55 {
            crate::market_finance::record_metered_delivery_tx(
                &conn,
                "share",
                "contract",
                &format!("slice-{index}"),
                "buyer",
                "provider",
                at,
                "2026-10-01T00:00:01Z",
                1,
                "2026-10-01T00:00:01Z",
            )
            .unwrap();
        }
        let first = earnings_tx(&conn, "provider", None).unwrap();
        assert_eq!(first.items.len(), 50);
        let second = earnings_tx(&conn, "provider", first.next_cursor.as_deref()).unwrap();
        assert_eq!(second.items.len(), 5);
        assert!(second.next_cursor.is_none());
        assert!(
            second
                .items
                .iter()
                .all(|item| first.items.iter().all(|prior| prior.id != item.id))
        );
        assert!(earnings_tx(&conn, "other", None).unwrap().items.is_empty());
        assert!(matches!(
            earnings_tx(&conn, "other", first.next_cursor.as_deref()),
            Err(AppError::BadRequest(_))
        ));
        let item = serde_json::to_value(&first.items[0]).unwrap();
        assert_eq!(item["creditedUnits"], "0");
        assert_eq!(item["amountUnits"], "1");
        for retired in ["paidUnits", "convertedUnits", "destination", "canWithdraw"] {
            assert!(item.get(retired).is_none());
        }
    }
}
