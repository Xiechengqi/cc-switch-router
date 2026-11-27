//! Whether the signed-in user sells on the Client market at all.
//!
//! The market page shows "My listings" only to users who own a Client or still
//! have seller history to look back on. `seller-inventory` cannot answer that:
//! it lists listings and rentals only, so an owner who never listed would look
//! like a non-seller and lose the way to list. This read is cheap (four counts)
//! and safe to repeat on every market refresh.
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::ServerState;
use crate::db::{Connection, params};
use crate::error::AppError;

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/client-market/online/seller-summary", get(summary))
        .layer(axum::middleware::from_fn(
            crate::client_market_audit::private_response,
        ))
}

async fn summary(State(state): State<ServerState>, headers: HeaderMap) -> Result<Json<Value>, AppError> {
    let session = crate::api::resolve_router_session(&state, &headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))?;
    let conn = state.store.conn.lock().await;
    Ok(Json(summary_tx(&conn, &session.user_id, &session.email)?))
}

fn summary_tx(conn: &Connection, user_id: &str, email: &str) -> Result<Value, AppError> {
    // Ownership follows the installation's owner email, as listing preflight does.
    // Fenced (retired) installations no longer count as Clients the user has.
    let (owned, listed, rented, history): (i64, i64, i64, i64) = conn
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM installations
                 WHERE lower(trim(owner_email))=lower(trim(?2)) AND lifecycle='active'),
               (SELECT COUNT(*) FROM online_client_listings WHERE seller_id=?1 AND state!='unlisted'),
               (SELECT COUNT(*) FROM online_client_leases
                 WHERE seller_id=?1 AND buyer_id!=seller_id AND released_at IS NULL),
               (SELECT COUNT(*) FROM online_client_leases
                 WHERE seller_id=?1 AND buyer_id!=seller_id AND state='released')",
            params![user_id, email],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(|e| AppError::Internal(format!("read Client seller summary: {e}")))?;
    Ok(json!({
        "ownedClients": owned,
        "listed": listed,
        "rented": rented,
        "history": history,
        // One flag so every caller agrees on who sees "My listings": owners, and
        // former sellers whose Clients are gone but whose income history remains.
        "canSell": owned > 0 || listed > 0 || rented > 0 || history > 0,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = crate::db::Connection::open_in_memory().unwrap();
        crate::schema::apply(&conn).unwrap();
        conn
    }

    #[test]
    fn client_market_seller_summary_rejects_non_owners_without_history() {
        let conn = db();
        let value = summary_tx(&conn, "user", "nobody@example.test").unwrap();
        assert_eq!(value["canSell"], false);
        assert_eq!(value["ownedClients"], 0);
    }

    #[test]
    fn client_market_seller_summary_counts_active_clients_only() {
        let conn = db();
        conn.execute(
            "INSERT INTO installations(id,public_key,platform,app_version,created_at,last_seen_at,control_secret_b64,owner_email)
             VALUES('mine','k','linux','1','t','t','s',' Seller@Example.test ')",
            [],
        )
        .unwrap();
        let value = summary_tx(&conn, "user", "seller@example.test").unwrap();
        assert_eq!(value["ownedClients"], 1);
        assert_eq!(value["canSell"], true);
        conn.execute("UPDATE installations SET lifecycle='fenced' WHERE id='mine'", []).unwrap();
        let value = summary_tx(&conn, "user", "seller@example.test").unwrap();
        assert_eq!(value["ownedClients"], 0);
        assert_eq!(value["canSell"], false);
    }
}
