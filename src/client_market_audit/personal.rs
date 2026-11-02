//! Buyer-scoped read projection. Shared lifecycle/money, explicit public fields;
//! never return the Owner audit envelope or infer history from a live Host.
use super::*;
use crate::models::AuthSession;
use axum::body::Body;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

pub(super) fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/client-market/me/rentals", get(list))
        .route("/v1/client-market/me/rentals/:id", get(get_detail))
}

// Also covers extractor/auth/cursor/not-found errors, not just successful reads.
pub(crate) async fn private_response(request: Request<Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, no-store"),
    );
    response
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersonalQuery {
    view: Option<String>,
    page: Option<u32>,
    /// A focus is resolved inside this buyer's scope, including an older page.
    focus: Option<String>,
}

async fn session(state: &ServerState, headers: &HeaderMap) -> Result<AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("authenticated session required".into()))
}

async fn list(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(q): Query<PersonalQuery>,
) -> Result<Json<Value>, AppError> {
    let actor = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction()
        .map_err(db_error("begin personal rental read"))?;
    let value = list_tx(&tx, &actor.user_id, &q)?;
    tx.commit()
        .map_err(db_error("finish personal rental read"))?;
    Ok(Json(value))
}

fn list_tx(conn: &Connection, buyer: &str, q: &PersonalQuery) -> Result<Value, AppError> {
    let view = q.view.as_deref().unwrap_or("current");
    if !matches!(view, "current" | "history")
        || q.page == Some(0)
        || q.focus
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 200)
    {
        return Err(AppError::BadRequest("invalid personal rental query".into()));
    }
    let facts = facts(ReadScope::Buyer);
    let predicate = "rental_group=?2";
    let priority = if view == "current" {
        "(attention!='none')"
    } else {
        "(0+0)"
    };
    let total: i64 = conn
        .query_row(
            &format!("{facts}SELECT COUNT(*) FROM records WHERE {predicate}"),
            params![buyer, view],
            |r| r.get(0),
        )
        .map_err(db_error("count personal rentals"))?;
    let pages = (total / LIST_PAGE_SIZE + i64::from(total % LIST_PAGE_SIZE != 0)).max(1);
    let mut page = i64::from(q.page.unwrap_or(1)).min(pages);
    let mut focused_id = None;
    if let Some(focus) = q.focus.as_deref() {
        // Resolve immutable record ID OR the existing installation deep-link ID.
        let selected: Option<(String, String, i64)> = conn.query_row(
            &format!("{facts}SELECT id,created_at,{priority} FROM records WHERE {predicate} AND (id=?3 OR installation_id=?3)"),
            params![buyer, view, focus], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()
            .map_err(db_error("resolve personal rental focus"))?;
        if let Some((id, at, rank)) = selected {
            let before: i64 = conn.query_row(&format!("{facts}SELECT COUNT(*) FROM records WHERE {predicate}
                AND ({priority}>?3 OR ({priority}=?3 AND (created_at>?4 OR (created_at=?4 AND id>?5))))"),
                params![buyer, view, rank, at, id], |r| r.get(0)).map_err(db_error("locate personal rental page"))?;
            page = before / LIST_PAGE_SIZE + 1;
            focused_id = Some(id);
        }
    }
    let mut statement = conn
        .prepare(&format!(
            "{facts}SELECT {COLUMNS} FROM records WHERE {predicate}
        ORDER BY {priority} DESC,created_at DESC,id DESC LIMIT ?3 OFFSET ?4"
        ))
        .map_err(db_error("prepare personal rentals"))?;
    let mut rows = statement
        .query_map(
            params![buyer, view, LIST_PAGE_SIZE, (page - 1) * LIST_PAGE_SIZE],
            record_row,
        )
        .map_err(db_error("query personal rentals"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read personal rentals"))?;
    attach_money(conn, &mut rows)?;
    let items = rows
        .iter()
        .map(buyer_record)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(
        json!({"items":items,"view":view,"total":total,"page":page,"pageSize":LIST_PAGE_SIZE,
        "totalPages":pages,"focusedId":focused_id,"serverNow":Utc::now().to_rfc3339()}),
    )
}

/// An allow-list, not removal of a few known secrets from an evolving admin DTO.
fn fields(value: &Value, keys: &[&str]) -> Value {
    Value::Object(
        keys.iter()
            .map(|key| ((*key).to_string(), value[*key].clone()))
            .collect(),
    )
}
fn buyer_record(value: &Value) -> Result<Value, AppError> {
    let mut record = fields(
        value,
        &[
            "id",
            "installationId",
            "providerEmail",
            "clientSubdomain",
            "countryCode",
            "provenance",
            "createdAt",
            "activatedAt",
            "endedAt",
            "cleanupConfirmedAt",
            "contractId",
            "status",
            "cleanupStatus",
            "attention",
            "terminationId",
            "pricingModel",
            "cyclePriceMinor",
            "dailyRateMinor",
            "offerRevision",
            "trialSecondsRemaining",
            "renewalPolicy",
            "group",
            "periodEnd",
            "expiresAt",
            "freeDurationDays",
            "contractStatus",
        ],
    );
    record["money"] = if value["money"].is_null() {
        Value::Null
    } else {
        let mut money = fields(
            &value["money"],
            &[
                "chargedUnits",
                "returnedUnits",
                "unfulfilledUnits",
                "serviceUnits",
                "compensationUnits",
            ],
        );
        let amount = |key: &str| -> Result<i64, AppError> {
            money[key]
                .as_str()
                .and_then(|s| s.parse::<i64>().ok())
                .ok_or_else(|| AppError::Internal("invalid personal rental amount".into()))
        };
        let net = amount("chargedUnits")?
            .checked_sub(amount("returnedUnits")?)
            .filter(|n| *n >= 0)
            .ok_or_else(|| AppError::Internal("inconsistent personal rental amounts".into()))?;
        money["netUnits"] = json!(net.to_string());
        money
    };
    Ok(record)
}

async fn get_detail(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<detail::DetailQuery>,
) -> Result<Json<Value>, AppError> {
    let actor = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction()
        .map_err(db_error("begin personal rental detail"))?;
    let value = detail_tx(&tx, &actor, &id, &q)?;
    tx.commit()
        .map_err(db_error("finish personal rental detail"))?;
    Ok(Json(value))
}

fn detail_tx(
    conn: &Connection,
    actor: &AuthSession,
    id: &str,
    q: &detail::DetailQuery,
) -> Result<Value, AppError> {
    if id.is_empty()
        || id.len() > 200
        || q.period_cursor.is_some_and(|n| n <= 0)
        || q.event_cursor
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 200)
    {
        return Err(AppError::BadRequest(
            "invalid personal rental detail query".into(),
        ));
    }
    let facts = facts(ReadScope::Buyer);
    let record = conn
        .query_row(
            &format!("{facts}SELECT {COLUMNS} FROM records WHERE id=?2"),
            params![actor.user_id, id],
            record_row,
        )
        .optional()
        .map_err(db_error("read personal rental detail"))?
        .ok_or_else(|| AppError::NotFound("rental record not found".into()))?;
    // Ownership has been proven before reading any money, periods, or events.
    let detail = detail::record_detail_tx(conn, record, q)?;
    let record = buyer_record(&detail["record"])?;
    let rental = if record["group"] == "current" {
        if let Some(installation) = record["installationId"].as_str() {
            serde_json::to_value(
                crate::client_market_trade::load_rental_views(conn, Some(installation), actor)?
                    .into_iter()
                    .next(),
            )
            .map_err(|e| AppError::Internal(format!("serialize personal rental: {e}")))?
        } else {
            Value::Null
        }
    } else {
        Value::Null
    };
    let periods: Vec<_> = detail["periods"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            fields(
                p,
                &[
                    "id",
                    "sequence",
                    "startedAt",
                    "endedAt",
                    "debitedAt",
                    "amountUnits",
                    "serviceUnits",
                    "returnedUnits",
                    "compensationUnits",
                    "status",
                    "terminationRateBps",
                ],
            )
        })
        .collect();
    let events: Vec<_> = detail["events"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| fields(e, &["id", "action", "createdAt", "note"]))
        .collect();
    Ok(
        json!({"record":record,"rental":rental,"periods":periods,"periodsNextCursor":detail["periodsNextCursor"],
        "events":events,"eventsNextCursor":detail["eventsNextCursor"],"trialConsumedSeconds":detail["trialConsumedSeconds"],
        "settlement":detail["settlement"],"serverNow":detail["serverNow"]}),
    )
}

#[cfg(test)]
mod tests;
