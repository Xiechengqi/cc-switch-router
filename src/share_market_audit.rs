//! Owner-only Share Market rental audit. Identity survives Share deletion via
//! `share_market_rental_identities`; every live lifecycle and monetary
//! projection is read from the original business tables and ledgers.
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

use crate::ServerState;
use crate::client_market_audit::{
    MONEY_FIELDS, db_error, no_store, private_response, text_filter, utc,
};
use crate::db::{Connection, OptionalExtension, params, types::Value as SqlValue};
use crate::error::AppError;

mod detail;
mod seller;
#[cfg(test)]
mod tests;

const LIST_PAGE_SIZE: i64 = 5;
const DETAIL_PAGE_SIZE: usize = 50;

pub(crate) const STATUSES: [&str; 9] = [
    "opening",
    "grant_failed",
    "trialing",
    "paid",
    "free",
    "paused",
    "ending",
    "ended",
    "unknown",
];
pub(crate) const ATTENTIONS: [&str; 7] = [
    "none",
    "finance_review",
    "disputed",
    "revoke_failed",
    "revoke_pending",
    "billing_control_failed",
    "settlement_pending",
];

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/admin/share-market/rental-audit", get(list))
        .route("/v1/admin/share-market/rental-audit/:id", get(detail::get))
        .route("/v1/share-market/me/rental-history", get(seller::list))
        .layer(axum::middleware::from_fn(private_response))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuditQuery {
    search: Option<String>,
    buyer_email: Option<String>,
    provider_email: Option<String>,
    status: Option<String>,
    attention: Option<String>,
    from: Option<String>,
    until: Option<String>,
    page: Option<u32>,
}

async fn list(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<AuditQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::api::require_owner_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction()
        .map_err(db_error("begin Share rental audit read"))?;
    let value = list_tx(&tx, &query)?;
    tx.commit()
        .map_err(db_error("finish Share rental audit read"))?;
    Ok((no_store(), Json(value)))
}

// Stored timestamps mix `to_rfc3339()` offsets and nanoseconds; project every
// output timestamp to one RFC3339 UTC millisecond form. Lifecycle decisions use
// the raw columns so an unparseable value can never hide an activation.
//
// No join to today's Share row except the cheap presence check. These CTEs
// carry indexed lifecycle facts only; amounts are batch-aggregated AFTER
// pagination in `attach_money`.
const FACTS: &str = "WITH rental_identities AS (SELECT * FROM share_market_rental_identities),
facts AS (
 SELECT a.id,a.share_id,a.share_name,a.share_subdomain,a.app_type,a.seat_id,a.seat_position,a.installation_id,
        a.buyer_user_id,a.provider_user_id,a.buyer_email,a.provider_email,a.provenance,a.created_at,
        s.status AS subscription_status,s.pricing_model,s.daily_rate_minor,s.cycle_price_minor,
        s.offer_revision,s.release_reason,COALESCE(s.service_started_at,s.activated_at) AS raw_activated_at,
        s.expires_at AS raw_expires_at,s.released_at AS raw_released_at,
        c.id AS contract_id,c.status AS contract_status,c.renewal_policy,c.ended_at AS contract_ended_at,
        t.id AS termination_id,t.status AS termination_status,t.cutoff_at AS termination_cutoff_at,
        json_extract(t.payload_json,'$.responsibility') AS responsibility,
        (SELECT o.status FROM share_control_operations o WHERE o.subscription_id=a.id AND o.action='revoke'
         ORDER BY o.share_sequence DESC LIMIT 1) AS revoke_operation_status,
        EXISTS(SELECT 1 FROM shares sh WHERE sh.share_id=a.share_id) AS share_present
 FROM rental_identities a
 LEFT JOIN share_market_subscriptions s ON s.id=a.id
 LEFT JOIN market_platform_contracts c ON c.product_kind='share' AND c.product_ref=a.id
 LEFT JOIN share_market_terminations t ON t.subscription_id=a.id
), lifecycle AS (
 SELECT facts.*,
  strftime('%Y-%m-%dT%H:%M:%fZ',raw_activated_at) AS activated_at,
  strftime('%Y-%m-%dT%H:%M:%fZ',raw_expires_at) AS expires_at,
  strftime('%Y-%m-%dT%H:%M:%fZ',COALESCE(termination_cutoff_at,contract_ended_at,raw_released_at)) AS ended_at,
  CASE WHEN subscription_status='grant_failed' OR contract_status='activation_failed'
            OR (subscription_status='released' AND raw_activated_at IS NULL) THEN 'grant_failed'
       WHEN subscription_status='released' OR (subscription_status IS NULL AND contract_status='ended') THEN 'ended'
       WHEN subscription_status IN ('revoke_pending','revoke_failed') OR contract_status IN ('ending','ended')
            OR termination_id IS NOT NULL THEN 'ending'
       WHEN subscription_status IN ('billing_suspend_pending','billing_suspended','billing_resume_pending','billing_control_failed')
            OR contract_status IN ('suspended','review_required') THEN 'paused'
       WHEN subscription_status='grant_pending' OR contract_status='pending_activation' THEN 'opening'
       WHEN contract_status='trialing' THEN 'trialing'
       WHEN contract_status='active' THEN 'paid'
       WHEN contract_id IS NULL AND pricing_model='free' AND subscription_status IN ('active_free','active') THEN 'free'
       ELSE 'unknown' END AS rental_status
 FROM facts
), records AS (
 SELECT lifecycle.*,
  CASE WHEN termination_status='finance_pending' OR contract_status='review_required' THEN 'finance_review'
       WHEN termination_status='disputed'
            OR (responsibility='undetermined' AND termination_status!='completed') THEN 'disputed'
       WHEN subscription_status='revoke_failed'
            OR (revoke_operation_status='rejected' AND subscription_status NOT IN ('released','grant_failed')) THEN 'revoke_failed'
       WHEN subscription_status='revoke_pending' OR termination_status='revoke_pending'
            OR (revoke_operation_status IN ('pending','dispatched')
                AND subscription_status NOT IN ('released','grant_failed')) THEN 'revoke_pending'
       WHEN subscription_status='billing_control_failed' THEN 'billing_control_failed'
       WHEN termination_status IS NOT NULL AND termination_status!='completed' THEN 'settlement_pending'
       WHEN rental_status IN ('ended','grant_failed') AND contract_status IS NOT NULL
            AND contract_status NOT IN ('ended','activation_failed') THEN 'settlement_pending'
       ELSE 'none' END AS attention
 FROM lifecycle
) ";

fn filters(q: &AuditQuery) -> Result<(String, Vec<SqlValue>), AppError> {
    let mut sql = "1=1".to_string();
    let mut args = Vec::new();
    for (value, column) in [
        (&q.buyer_email, "buyer_email"),
        (&q.provider_email, "provider_email"),
    ] {
        if let Some(value) = text_filter(value.as_deref())? {
            args.push(SqlValue::Text(value.to_ascii_lowercase()));
            sql.push_str(&format!(" AND {column}=?{} COLLATE NOCASE", args.len()));
        }
    }
    if let Some(value) = text_filter(q.search.as_deref())? {
        // Exact identity matches and escaped ID/name/subdomain prefixes. No
        // arbitrary SQL patterns and no searching event payloads.
        let escaped = value
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        args.push(SqlValue::Text(format!("{escaped}%")));
        let prefix = args.len();
        args.push(SqlValue::Text(value));
        let exact = args.len();
        sql.push_str(&format!(
            " AND (id LIKE ?{prefix} ESCAPE '\\' OR share_id LIKE ?{prefix} ESCAPE '\\'
           OR seat_id LIKE ?{prefix} ESCAPE '\\' OR installation_id LIKE ?{prefix} ESCAPE '\\'
           OR share_name LIKE ?{prefix} ESCAPE '\\' OR share_subdomain LIKE ?{prefix} ESCAPE '\\'
           OR buyer_email=?{exact} COLLATE NOCASE OR provider_email=?{exact} COLLATE NOCASE)"
        ));
    }
    if let Some(value) = q.status.as_deref().filter(|s| !s.is_empty()) {
        if !STATUSES.contains(&value) {
            return Err(AppError::BadRequest(
                "invalid Share rental audit status".into(),
            ));
        }
        args.push(SqlValue::Text(value.into()));
        sql.push_str(&format!(" AND rental_status=?{}", args.len()));
    }
    if let Some(value) = q.attention.as_deref().filter(|s| !s.is_empty()) {
        if value == "any" {
            sql.push_str(" AND attention!='none'");
        } else {
            if !ATTENTIONS.contains(&value) {
                return Err(AppError::BadRequest(
                    "invalid Share rental audit attention".into(),
                ));
            }
            args.push(SqlValue::Text(value.into()));
            sql.push_str(&format!(" AND attention=?{}", args.len()));
        }
    }
    let from = q.from.as_deref().map(utc).transpose()?;
    let until = q.until.as_deref().map(utc).transpose()?;
    if from
        .as_ref()
        .zip(until.as_ref())
        .is_some_and(|(a, b)| a >= b)
    {
        return Err(AppError::BadRequest("audit end must follow start".into()));
    }
    for (value, op) in [(from, ">="), (until, "<")] {
        if let Some(value) = value {
            args.push(SqlValue::Text(value));
            sql.push_str(&format!(" AND created_at{op}?{}", args.len()));
        }
    }
    Ok((sql, args))
}

const COLUMNS: &str = "id,share_id,share_name,share_subdomain,app_type,seat_id,seat_position,installation_id,
 buyer_email,provider_email,provenance,created_at,activated_at,ended_at,contract_id,rental_status,attention,
 termination_id,share_present,pricing_model,daily_rate_minor,offer_revision,renewal_policy,expires_at,
 release_reason,cycle_price_minor,contract_status";

fn record_row(r: &crate::db::Row<'_>) -> crate::db::Result<Value> {
    let position = r.get::<_, Option<i64>>(6)?;
    Ok(json!({
        "id":r.get::<_,String>(0)?,"shareId":r.get::<_,String>(1)?,"shareName":r.get::<_,Option<String>>(2)?,
        "shareSubdomain":r.get::<_,Option<String>>(3)?,"appType":r.get::<_,Option<String>>(4)?,
        "seatId":r.get::<_,Option<String>>(5)?,"seatPosition":position,
        // Seats carry only a position; the label is that frozen position.
        "seatLabel":position.map(|p| p.to_string()),
        "installationId":r.get::<_,Option<String>>(7)?,"buyerEmail":r.get::<_,Option<String>>(8)?,
        "providerEmail":r.get::<_,Option<String>>(9)?,"provenance":r.get::<_,String>(10)?,
        "createdAt":r.get::<_,String>(11)?,"activatedAt":r.get::<_,Option<String>>(12)?,
        "endedAt":r.get::<_,Option<String>>(13)?,"contractId":r.get::<_,Option<String>>(14)?,
        "status":r.get::<_,String>(15)?,"attention":r.get::<_,String>(16)?,
        "terminationId":r.get::<_,Option<String>>(17)?,"sharePresent":r.get::<_,bool>(18)?,
        "pricingModel":r.get::<_,Option<String>>(19)?,"dailyRateMinor":r.get::<_,Option<i64>>(20)?,
        "offerRevision":r.get::<_,Option<i64>>(21)?,"renewalPolicy":r.get::<_,Option<String>>(22)?,
        "expiresAt":r.get::<_,Option<String>>(23)?,"releaseReason":r.get::<_,Option<String>>(24)?,
        "cyclePriceMinor":r.get::<_,Option<i64>>(25)?,"contractStatus":r.get::<_,Option<String>>(26)?,
        "money":null
    }))
}

fn list_tx(conn: &Connection, q: &AuditQuery) -> Result<Value, AppError> {
    let requested_page = q.page.unwrap_or(1);
    if requested_page == 0 {
        return Err(AppError::BadRequest(
            "invalid Share rental audit page".into(),
        ));
    }
    let (predicate, mut args) = filters(q)?;
    let total: i64 = conn
        .query_row(
            &format!("{FACTS}SELECT COUNT(*) FROM records WHERE {predicate}"),
            args.clone(),
            |r| r.get(0),
        )
        .map_err(db_error("count Share rental audit records"))?;
    // Count and page share the caller's read transaction. A shrinking result
    // returns the last valid page instead of a stranded empty page.
    let total_pages = (total / LIST_PAGE_SIZE + i64::from(total % LIST_PAGE_SIZE != 0)).max(1);
    let page = i64::from(requested_page).min(total_pages);
    args.push(SqlValue::Integer(LIST_PAGE_SIZE));
    let limit_arg = args.len();
    args.push(SqlValue::Integer((page - 1) * LIST_PAGE_SIZE));
    let offset_arg = args.len();
    let mut statement = conn
        .prepare(&format!(
            "{FACTS}SELECT {COLUMNS} FROM records WHERE {predicate}
             ORDER BY created_at DESC,id DESC LIMIT ?{limit_arg} OFFSET ?{offset_arg}"
        ))
        .map_err(db_error("prepare Share rental audit records"))?;
    let mut items = statement
        .query_map(args, record_row)
        .map_err(db_error("query Share rental audit records"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read Share rental audit records"))?;
    attach_money(conn, &mut items)?;
    Ok(
        json!({"items":items,"total":total,"page":page,"pageSize":LIST_PAGE_SIZE,
        "totalPages":total_pages,"serverNow":Utc::now().to_rfc3339()}),
    )
}

/// Same nine fields and sources as the Client audit, with the Share-specific
/// ledger columns:
/// - `unfulfilledUnits` also excludes `share_quota_units` (quota value already
///   delivered on immediate termination; it is a delivery slice, so it is in
///   `serviceUnits`). A disputed hold stays inside this remaining liability.
/// - Compensation comes from `share_termination_income` (kind
///   `share_termination`). It has no reversal column, so the gross award is
///   the compensation and `credited_units` is what matured into the
///   Provider's balance.
fn attach_money(conn: &Connection, records: &mut [Value]) -> Result<(), AppError> {
    let ids: Vec<String> = records
        .iter()
        .filter_map(|r| r["contractId"].as_str().map(str::to_string))
        .collect();
    let mut amounts: HashMap<String, [i64; 9]> =
        ids.iter().map(|id| (id.clone(), [0; 9])).collect();
    if !ids.is_empty() {
        let marks = (1..=ids.len())
            .map(|n| format!("?{n}"))
            .collect::<Vec<_>>()
            .join(",");
        // Each source is grouped independently before merging. Joining periods,
        // slices, lots and awards together would multiply debits and earnings.
        let sql=format!("SELECT contract_id,SUM(amount_units),SUM(refunded_units),
             SUM(amount_units-fulfilled_units-refunded_units-termination_compensation_units-share_quota_units),0,0,0,0,0,0
           FROM market_platform_periods WHERE contract_id IN ({marks}) GROUP BY contract_id
         UNION ALL SELECT contract_id,SUM(amount_units),0,0,0,0,0,0,0,0
           FROM market_usage_posting_batches WHERE contract_id IN ({marks}) GROUP BY contract_id
         UNION ALL SELECT d.contract_id,0,0,0,SUM(d.amount_units-COALESCE(l.reversed_units,0)-COALESCE(l.commission_reversed_units,0)),
           SUM(COALESCE(l.net_amount_units-l.reversed_units,0)),SUM(COALESCE(l.commission_units-l.commission_reversed_units,0)),
           SUM(COALESCE(l.converted_units,0)),0,0
           FROM market_delivery_slices d LEFT JOIN market_provider_earning_lots l ON l.delivery_slice_id=d.id
           WHERE d.contract_kind='share' AND d.contract_id IN ({marks}) GROUP BY d.contract_id
         UNION ALL SELECT t.contract_id,0,0,0,0,0,0,0,SUM(i.amount_units),SUM(i.credited_units)
           FROM share_market_terminations t JOIN share_termination_income i ON i.termination_id=t.id
           WHERE i.kind='share_termination' AND t.contract_id IN ({marks}) GROUP BY t.contract_id");
        let mut statement = conn
            .prepare(&sql)
            .map_err(db_error("prepare Share audit monetary aggregates"))?;
        let rows = statement
            .query_map(
                ids.into_iter().map(SqlValue::Text).collect::<Vec<_>>(),
                |r| {
                    let mut values = [0; 9];
                    for (i, v) in values.iter_mut().enumerate() {
                        *v = r.get(i + 1)?;
                    }
                    Ok((r.get::<_, String>(0)?, values))
                },
            )
            .map_err(db_error("query Share audit monetary aggregates"))?;
        for row in rows {
            let (id, values) = row.map_err(db_error("read Share audit monetary aggregate"))?;
            let current = amounts.entry(id).or_default();
            for (a, b) in current.iter_mut().zip(values) {
                *a = a.checked_add(b).ok_or_else(|| {
                    AppError::Internal("Share audit monetary aggregate overflow".into())
                })?;
            }
        }
    }
    for record in records {
        // Missing financial evidence stays null. Only a rental whose frozen
        // pricing is free (and so has no contract) is provably zero.
        let free = record["contractId"].is_null()
            && (record["status"] == "free" || record["pricingModel"] == "free");
        if let Some(values) = record["contractId"]
            .as_str()
            .and_then(|id| amounts.get(id))
            .copied()
            .or_else(|| free.then_some([0; 9]))
        {
            record["money"] = json!(
                MONEY_FIELDS
                    .into_iter()
                    .zip(values)
                    .map(|(k, v)| (k.to_string(), json!(v.to_string())))
                    .collect::<serde_json::Map<_, _>>()
            );
        }
    }
    Ok(())
}
