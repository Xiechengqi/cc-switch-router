//! Owner-only rental audit. Identity survives resource deletion; all live
//! lifecycle and monetary projections are read from the original business data.
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, header};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

use crate::ServerState;
use crate::db::{Connection, OptionalExtension, params, types::Value as SqlValue};
use crate::error::AppError;

mod detail;
mod personal;
pub(crate) use personal::{billed_time_tx, private_response};
#[cfg(test)]
mod tests;

const LIST_PAGE_SIZE: i64 = 5;
const DETAIL_PAGE_SIZE: usize = 50;
fn db_error(context: &'static str) -> impl FnOnce(crate::db::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}
fn utc(value: &str) -> Result<String, AppError> {
    DateTime::parse_from_rfc3339(value)
        .map(|v| {
            v.with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Millis, true)
        })
        .map_err(|_| AppError::BadRequest("invalid audit timestamp".into()))
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/admin/client-market/rental-audit", get(list))
        .route("/v1/admin/client-market/rental-audit/:id", get(detail::get))
        .merge(personal::router())
        .layer(axum::middleware::from_fn(private_response))
}
fn no_store() -> [(header::HeaderName, &'static str); 1] {
    [(header::CACHE_CONTROL, "private, no-store")]
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
        .map_err(db_error("begin rental audit read"))?;
    let value = list_tx(&tx, &query)?;
    tx.commit().map_err(db_error("finish rental audit read"))?;
    Ok((no_store(), Json(value)))
}

// No join to today's Host ownership or active installation. These CTEs carry
// cheap lifecycle facts only; amounts are batch-aggregated AFTER pagination.
enum ReadScope {
    Owner,
    Buyer,
}

// Scope the immutable identity source BEFORE joins, counts and monetary reads.
// The buyer ID is always the first bound parameter, never an email or URL role.
fn facts(scope: ReadScope) -> String {
    let predicate = match scope {
        ReadScope::Owner => "",
        ReadScope::Buyer => " WHERE buyer_user_id=?1",
    };
    format!(
        "WITH rental_identities AS (SELECT * FROM client_market_rental_identities{predicate}), {LIFECYCLE}"
    )
}

const LIFECYCLE: &str = "facts AS (
 SELECT a.id,a.job_id,a.installation_id,a.host_id,a.buyer_user_id,a.provider_user_id,a.buyer_email,a.provider_email,
        a.client_subdomain,a.host_ip,a.host_port,a.country_code,a.provenance,a.created_at,
        r.state AS subscription_status, strftime('%Y-%m-%dT%H:%M:%fZ',r.activated_at,'unixepoch') AS activated_at,
        COALESCE(t.cutoff_at,c.ended_at,strftime('%Y-%m-%dT%H:%M:%fZ',coalesce(r.cutoff_at,r.released_at),'unixepoch')) AS ended_at,
        c.id AS contract_id,c.status AS contract_status,
        CASE WHEN q.cycle_price_minor>0 THEN 'prepaid_calendar_month' ELSE 'free' END AS pricing_model,
        NULLIF(q.cycle_price_minor,0) AS cycle_price_minor,NULL AS daily_rate_minor,q.revision AS offer_revision,
        c.trial_seconds_remaining,c.renewal_policy,
        (SELECT p.period_end FROM market_platform_periods p WHERE p.contract_id=c.id
         AND p.sequence=c.current_period_sequence) AS period_end,
        CASE WHEN q.free_days IS NOT NULL THEN strftime('%Y-%m-%dT%H:%M:%fZ',r.grant_until,'unixepoch') END AS expires_at,
        q.free_days AS free_duration_days,t.id AS termination_id,t.status AS termination_status,
        t.responsibility,t.dispute_used,t.provider_responded_at,
        COALESCE(t.cleanup_confirmed_at,receipt.confirmed_at) AS cleanup_confirmed_at,
        EXISTS(SELECT 1 FROM installations i WHERE i.id=a.host_id AND i.lifecycle='active') AS host_present
 FROM rental_identities a
 JOIN online_client_leases r ON r.id=a.installation_id
 JOIN online_client_quotes q ON q.id=r.quote_id
 LEFT JOIN market_platform_contracts c ON c.product_kind='client_host' AND c.product_ref=r.id
 LEFT JOIN client_market_terminations t ON t.installation_id=r.id
 LEFT JOIN client_cleanup_receipts receipt ON receipt.installation_id=r.id AND receipt.host_id=r.installation_id
), lifecycle AS (
 SELECT facts.*,
  CASE WHEN (ended_at IS NOT NULL AND activated_at IS NULL) OR contract_status='activation_failed' THEN 'activation_failed'
       WHEN ended_at IS NOT NULL OR subscription_status IN ('ending','cleaning','released') OR contract_status IN ('ending','ended') THEN 'ended'
       WHEN contract_status='trialing' THEN 'trialing'
       WHEN contract_status IN ('suspended','review_required') THEN 'paused'
       WHEN contract_status='active' THEN 'paid'
       WHEN subscription_status='active' AND COALESCE(pricing_model,'free')='free' THEN 'free'
       WHEN subscription_status='starting' OR contract_status='pending_activation' THEN 'opening'
       ELSE 'unknown' END AS rental_status,
  CASE WHEN cleanup_confirmed_at IS NOT NULL AND subscription_status IN ('ending','cleaning') THEN 'pending'
       WHEN cleanup_confirmed_at IS NOT NULL THEN 'verified'
       WHEN termination_status='cleanup_failed' THEN 'failed'
       WHEN termination_status='cleanup_pending' OR subscription_status IN ('ending','cleaning') OR contract_status='ending' THEN 'pending'
       WHEN subscription_status='released' THEN 'unknown'
       ELSE 'not_required' END AS cleanup_status
 FROM facts
), records AS (
 SELECT lifecycle.*,
  CASE WHEN rental_status IN ('ended','activation_failed') THEN 'history' ELSE 'current' END AS rental_group,
  CASE WHEN termination_status='finance_pending' OR contract_status='review_required' THEN 'finance_review'
       WHEN dispute_used=1 AND responsibility='undetermined' THEN
          CASE WHEN provider_responded_at IS NULL THEN 'waiting_provider' ELSE 'owner_decision' END
       WHEN cleanup_status='failed' THEN 'cleanup_failed'
       WHEN cleanup_status='pending' THEN 'cleanup_pending'
       WHEN termination_status IS NOT NULL AND termination_status!='completed' THEN 'settlement_pending'
       WHEN rental_status='ended' AND contract_status IS NOT NULL AND contract_status!='ended' THEN 'settlement_pending'
       WHEN rental_status='ended' AND cleanup_status='unknown' THEN 'cleanup_unknown'
       ELSE 'none' END AS attention
 FROM lifecycle
) ";

fn text_filter(value: Option<&str>) -> Result<Option<String>, AppError> {
    let value = value.map(str::trim).filter(|v| !v.is_empty());
    if value.is_some_and(|v| v.len() > 254 || v.chars().any(char::is_control)) {
        return Err(AppError::BadRequest(
            "audit search is too long or contains control characters".into(),
        ));
    }
    Ok(value.map(str::to_string))
}

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
        // Exact identity matches and escaped subdomain/ID prefixes. No arbitrary
        // SQL patterns, unbounded payloads or searching secret-bearing logs.
        let escaped = value
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        args.push(SqlValue::Text(format!("{escaped}%")));
        let prefix = args.len();
        args.push(SqlValue::Text(value));
        let exact = args.len();
        sql.push_str(&format!(
            " AND (id LIKE ?{prefix} ESCAPE '\\' OR job_id LIKE ?{prefix} ESCAPE '\\'
           OR installation_id LIKE ?{prefix} ESCAPE '\\' OR host_id LIKE ?{prefix} ESCAPE '\\'
           OR client_subdomain LIKE ?{prefix} ESCAPE '\\' OR host_ip=?{exact}
           OR buyer_email=?{exact} COLLATE NOCASE OR provider_email=?{exact} COLLATE NOCASE)"
        ));
    }
    if let Some(value) = q.status.as_deref().filter(|s| !s.is_empty()) {
        if ![
            "opening",
            "activation_failed",
            "trialing",
            "paid",
            "free",
            "paused",
            "ended",
            "unknown",
        ]
        .contains(&value)
        {
            return Err(AppError::BadRequest("invalid rental audit status".into()));
        }
        args.push(SqlValue::Text(value.into()));
        sql.push_str(&format!(" AND rental_status=?{}", args.len()));
    }
    if let Some(value) = q.attention.as_deref().filter(|s| !s.is_empty()) {
        if value == "any" {
            sql.push_str(" AND attention!='none'");
        } else {
            if ![
                "finance_review",
                "waiting_provider",
                "owner_decision",
                "cleanup_failed",
                "cleanup_pending",
                "settlement_pending",
                "cleanup_unknown",
                "none",
            ]
            .contains(&value)
            {
                return Err(AppError::BadRequest(
                    "invalid rental audit attention".into(),
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

const COLUMNS: &str = "id,job_id,installation_id,host_id,buyer_email,provider_email,client_subdomain,
 host_ip,host_port,country_code,provenance,created_at,activated_at,ended_at,contract_id,
 rental_status,cleanup_status,attention,termination_id,host_present,pricing_model,
 cycle_price_minor,daily_rate_minor,offer_revision,trial_seconds_remaining,renewal_policy,cleanup_confirmed_at,
 rental_group,period_end,expires_at,free_duration_days,contract_status";

fn record_row(r: &crate::db::Row<'_>) -> crate::db::Result<Value> {
    Ok(json!({
        "id":r.get::<_,String>(0)?,"jobId":r.get::<_,Option<String>>(1)?,"installationId":r.get::<_,Option<String>>(2)?,
        "hostId":r.get::<_,Option<String>>(3)?,"buyerEmail":r.get::<_,Option<String>>(4)?,"providerEmail":r.get::<_,Option<String>>(5)?,
        "clientSubdomain":r.get::<_,Option<String>>(6)?,"hostIp":r.get::<_,Option<String>>(7)?,"hostPort":r.get::<_,Option<i64>>(8)?,
        "countryCode":r.get::<_,Option<String>>(9)?,"provenance":r.get::<_,String>(10)?,"createdAt":r.get::<_,String>(11)?,
        "activatedAt":r.get::<_,Option<String>>(12)?,"endedAt":r.get::<_,Option<String>>(13)?,"contractId":r.get::<_,Option<String>>(14)?,
        "status":r.get::<_,String>(15)?,"cleanupStatus":r.get::<_,String>(16)?,"attention":r.get::<_,String>(17)?,
        "terminationId":r.get::<_,Option<String>>(18)?,"hostPresent":r.get::<_,bool>(19)?,"pricingModel":r.get::<_,Option<String>>(20)?,
        "cyclePriceMinor":r.get::<_,Option<i64>>(21)?,"dailyRateMinor":r.get::<_,Option<i64>>(22)?,"offerRevision":r.get::<_,Option<i64>>(23)?,
        "trialSecondsRemaining":r.get::<_,Option<i64>>(24)?,"renewalPolicy":r.get::<_,Option<String>>(25)?,"cleanupConfirmedAt":r.get::<_,Option<String>>(26)?,
        "group":r.get::<_,String>(27)?,"periodEnd":r.get::<_,Option<String>>(28)?,"expiresAt":r.get::<_,Option<String>>(29)?,
        "freeDurationDays":r.get::<_,Option<i64>>(30)?,"contractStatus":r.get::<_,Option<String>>(31)?,
        "money":null
    }))
}

fn list_tx(conn: &Connection, q: &AuditQuery) -> Result<Value, AppError> {
    let facts = facts(ReadScope::Owner);
    let requested_page = q.page.unwrap_or(1);
    if requested_page == 0 {
        return Err(AppError::BadRequest("invalid rental audit page".into()));
    }
    let (predicate, mut args) = filters(q)?;
    let total: i64 = conn
        .query_row(
            &format!("{facts}SELECT COUNT(*) FROM records WHERE {predicate}"),
            args.clone(),
            |r| r.get(0),
        )
        .map_err(db_error("count rental audit records"))?;
    // Count and records share the caller's read transaction. A filter or live
    // lifecycle change may remove the requested page; return the last valid
    // page instead of an empty, stranded page. Empty results use page 1 of 1.
    let total_pages = (total / LIST_PAGE_SIZE + i64::from(total % LIST_PAGE_SIZE != 0)).max(1);
    let page = i64::from(requested_page).min(total_pages);
    args.push(SqlValue::Integer(LIST_PAGE_SIZE));
    let limit_arg = args.len();
    args.push(SqlValue::Integer((page - 1) * LIST_PAGE_SIZE));
    let offset_arg = args.len();
    let mut statement=conn.prepare(&format!("{facts}SELECT {COLUMNS} FROM records WHERE {predicate} ORDER BY created_at DESC,id DESC LIMIT ?{limit_arg} OFFSET ?{offset_arg}"))
        .map_err(db_error("prepare rental audit records"))?;
    let mut items = statement
        .query_map(args, record_row)
        .map_err(db_error("query rental audit records"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read rental audit records"))?;
    attach_money(conn, &mut items)?;
    Ok(
        json!({"items":items,"total":total,"page":page,"pageSize":LIST_PAGE_SIZE,
        "totalPages":total_pages,"serverNow":Utc::now().to_rfc3339()}),
    )
}

const MONEY_FIELDS: [&str; 9] = [
    "chargedUnits",
    "returnedUnits",
    "unfulfilledUnits",
    "serviceUnits",
    "providerNetUnits",
    "platformFeeUnits",
    "providerCreditedUnits",
    "compensationUnits",
    "compensationCreditedUnits",
];
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
        // Each source is independently grouped before merging. Joining periods,
        // slices and lots together would multiply both debits and earnings.
        let sql=format!("SELECT contract_id,SUM(amount_units),SUM(refunded_units),SUM(amount_units-fulfilled_units-refunded_units-termination_compensation_units),0,0,0,0,0,0
           FROM market_platform_periods WHERE contract_id IN ({marks}) GROUP BY contract_id
         UNION ALL SELECT contract_id,SUM(amount_units),0,0,0,0,0,0,0,0
           FROM market_usage_posting_batches WHERE contract_id IN ({marks}) GROUP BY contract_id
         UNION ALL SELECT d.contract_id,0,0,0,SUM(d.amount_units-COALESCE(l.reversed_units,0)-COALESCE(l.commission_reversed_units,0)),
           SUM(COALESCE(l.net_amount_units-l.reversed_units,0)),SUM(COALESCE(l.commission_units-l.commission_reversed_units,0)),
           SUM(COALESCE(l.converted_units,0)),0,0
           FROM market_delivery_slices d LEFT JOIN market_provider_earning_lots l ON l.delivery_slice_id=d.id
           WHERE d.contract_kind='client_host' AND d.contract_id IN ({marks}) GROUP BY d.contract_id
         UNION ALL SELECT t.contract_id,0,0,0,0,0,0,0,SUM(c.amount_units-c.reversed_units),SUM(c.credited_units)
           FROM client_market_terminations t JOIN client_termination_compensations c ON c.id=t.id
           WHERE t.contract_id IN ({marks}) GROUP BY t.contract_id");
        let mut statement = conn
            .prepare(&sql)
            .map_err(db_error("prepare audit monetary aggregates"))?;
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
            .map_err(db_error("query audit monetary aggregates"))?;
        for row in rows {
            let (id, values) = row.map_err(db_error("read audit monetary aggregate"))?;
            let current = amounts.entry(id).or_default();
            for (a, b) in current.iter_mut().zip(values) {
                *a = a.checked_add(b).ok_or_else(|| {
                    AppError::Internal("audit monetary aggregate overflow".into())
                })?;
            }
        }
    }
    for record in records {
        let free = record["status"] == "free" || record["pricingModel"] == "free";
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
