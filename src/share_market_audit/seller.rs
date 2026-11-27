//! Seller-scoped Share rental history: the ended rentals of the caller's own
//! seats, for the bottom of "My listings". Same lifecycle and money as the
//! Owner audit, but an explicit allow-list of fields and only the Provider's
//! side of the money (what the buyer paid stays the buyer's).
use super::*;

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SellerHistoryQuery {
    page: Option<u32>,
}

pub(super) async fn list(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<SellerHistoryQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = crate::api::resolve_router_session(&state, &headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("authenticated session required".into()))?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction()
        .map_err(db_error("begin Share seller history read"))?;
    let value = list_tx(&tx, &actor.user_id, query.page.unwrap_or(1))?;
    tx.commit()
        .map_err(db_error("finish Share seller history read"))?;
    Ok((no_store(), Json(value)))
}

// Only rentals that were delivered and then ended: a grant that never started
// is the buyer's failed order, not part of what the seller rented out.
const PREDICATE: &str = "provider_user_id=?1 AND rental_status='ended'";

pub(super) fn list_tx(
    conn: &Connection,
    provider: &str,
    requested_page: u32,
) -> Result<Value, AppError> {
    if requested_page == 0 {
        return Err(AppError::BadRequest(
            "invalid Share seller history page".into(),
        ));
    }
    let total: i64 = conn
        .query_row(
            &format!("{FACTS}SELECT COUNT(*) FROM records WHERE {PREDICATE}"),
            params![provider],
            |r| r.get(0),
        )
        .map_err(db_error("count Share seller history"))?;
    let total_pages = (total / LIST_PAGE_SIZE + i64::from(total % LIST_PAGE_SIZE != 0)).max(1);
    let page = i64::from(requested_page).min(total_pages);
    let mut statement = conn
        .prepare(&format!(
            "{FACTS}SELECT {COLUMNS} FROM records WHERE {PREDICATE}
             ORDER BY COALESCE(ended_at,created_at) DESC,id DESC LIMIT ?2 OFFSET ?3"
        ))
        .map_err(db_error("prepare Share seller history"))?;
    let mut rows = statement
        .query_map(
            params![provider, LIST_PAGE_SIZE, (page - 1) * LIST_PAGE_SIZE],
            record_row,
        )
        .map_err(db_error("query Share seller history"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read Share seller history"))?;
    attach_money(conn, &mut rows)?;
    let items = rows
        .iter()
        .map(seller_record)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(
        json!({"items":items,"total":total,"page":page,"pageSize":LIST_PAGE_SIZE,
        "totalPages":total_pages,"serverNow":Utc::now().to_rfc3339()}),
    )
}

fn seller_record(value: &Value) -> Result<Value, AppError> {
    let mut record = Value::Object(
        [
            "id",
            "shareId",
            "shareName",
            "shareSubdomain",
            "appType",
            "seatPosition",
            "buyerEmail",
            "activatedAt",
            "endedAt",
            "pricingModel",
            "dailyRateMinor",
            "cyclePriceMinor",
            "releaseReason",
        ]
        .into_iter()
        .map(|key| (key.to_string(), value[key].clone()))
        .collect(),
    );
    // Income is what the Provider earned: net delivery earnings plus any
    // termination compensation. Missing evidence stays null, never zero.
    record["incomeUnits"] = if value["money"].is_null() {
        Value::Null
    } else {
        let amount = |key: &str| -> Result<i64, AppError> {
            value["money"][key]
                .as_str()
                .and_then(|s| s.parse::<i64>().ok())
                .ok_or_else(|| AppError::Internal("invalid Share seller history amount".into()))
        };
        let income = amount("providerNetUnits")?
            .checked_add(amount("compensationUnits")?)
            .ok_or_else(|| AppError::Internal("Share seller history amount overflow".into()))?;
        json!(income.to_string())
    };
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::super::tests::rental;
    use super::*;

    #[test]
    fn seller_history_lists_only_the_callers_ended_rentals() {
        let conn = crate::test_support::database().unwrap();
        rental(&conn, 1, "released");
        rental(&conn, 2, "active_free");
        rental(&conn, 3, "grant_failed");
        let mine = list_tx(&conn, "provider", 1).unwrap();
        assert_eq!(mine["total"], 1);
        let item = &mine["items"][0];
        assert_eq!(item["shareName"], "Share 1");
        assert_eq!(item["buyerEmail"], "buyer@example.test");
        // A free rental is provably zero income; no audit-only fields leak.
        assert_eq!(item["incomeUnits"], "0");
        assert!(item.get("providerEmail").is_none() && item.get("attention").is_none());
        assert_eq!(list_tx(&conn, "someone-else", 1).unwrap()["total"], 0);
        assert!(list_tx(&conn, "provider", 0).is_err());
    }
}
