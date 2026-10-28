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
#[cfg(test)]
mod tests;

const PAGE_SIZE: usize = 50;
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

/// Called in the same transaction as an order commit. Captures identity only,
/// never a password, SSH credentials, deployment logs, mutable state or money.
pub(crate) fn capture_job_identity_tx(conn: &Connection, job: &str) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO client_market_rental_identities
         (id,job_id,host_id,buyer_user_id,provider_user_id,buyer_email,provider_email,
          client_subdomain,host_ip,host_port,country_code,provenance,created_at)
         SELECT 'job:'||j.id,j.id,j.host_id,j.client_owner_user_id,
                COALESCE(qi.provider_id,h.provider_id),j.client_owner_email,
                COALESCE(qi.host_owner_email,j.host_owner_email),j.subdomain,
                h.ip,h.port,COALESCE(qi.country_code,h.country_code),'order_snapshot',
                strftime('%Y-%m-%dT%H:%M:%fZ',j.created_at)
         FROM provisioning_jobs j LEFT JOIN router_ssh_hosts h ON h.id=j.host_id
         LEFT JOIN client_market_allocation_quote_items qi ON qi.quote_id=j.quote_id AND qi.host_id=j.host_id
         WHERE j.id=?1 AND j.type='create'
         ON CONFLICT(job_id) DO NOTHING",
        params![job],
    ).map_err(db_error("capture rental audit identity"))?;
    Ok(())
}

pub(crate) fn bind_installation_tx(
    conn: &Connection,
    job: &str,
    installation: &str,
) -> Result<(), AppError> {
    capture_job_identity_tx(conn, job)?;
    conn.execute(
        "UPDATE client_market_rental_identities SET installation_id=?2
         WHERE job_id=?1 AND installation_id IS NULL",
        params![job, installation],
    )
    .map_err(db_error("bind rental audit installation"))?;
    let bound: bool = conn
        .query_row(
            "SELECT installation_id=?2 FROM client_market_rental_identities WHERE job_id=?1",
            params![job, installation],
            |r| r.get(0),
        )
        .map_err(db_error("verify rental audit installation"))?;
    if !bound {
        return Err(AppError::Conflict(
            "rental audit identity is bound to another installation".into(),
        ));
    }
    Ok(())
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/admin/client-market/rental-audit", get(list))
        .route("/v1/admin/client-market/rental-audit/:id", get(detail::get))
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
    cursor: Option<String>,
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
const FACTS: &str = "WITH facts AS (
 SELECT a.id,a.job_id,a.installation_id,COALESCE(a.host_id,s.host_id,j.host_id) AS host_id,
        a.buyer_user_id,a.provider_user_id,a.buyer_email,a.provider_email,
        a.client_subdomain,a.host_ip,a.host_port,a.country_code,a.provenance,a.created_at,
        s.status AS subscription_status,s.activated_at,
        COALESCE(t.cutoff_at,c.ended_at,s.released_at) AS ended_at,
        c.id AS contract_id,c.status AS contract_status,
        COALESCE(s.pricing_model,qi.pricing_model) AS pricing_model,
        COALESCE(s.cycle_price_minor,qi.cycle_price_minor,c.cycle_price_minor) AS cycle_price_minor,
        COALESCE(s.daily_rate_minor,qi.daily_rate_minor,c.daily_rate_minor) AS daily_rate_minor,
        COALESCE(s.offer_revision,qi.offer_revision,c.offer_revision) AS offer_revision,
        c.trial_seconds_remaining,c.renewal_policy,
        j.status AS job_status,t.id AS termination_id,t.status AS termination_status,
        t.responsibility,t.dispute_used,t.provider_responded_at,
        COALESCE(t.cleanup_confirmed_at,r.confirmed_at) AS cleanup_confirmed_at,
        (SELECT cleanup.status FROM provisioning_jobs cleanup
         WHERE cleanup.installation_id=a.installation_id AND cleanup.type='cleanup'
         ORDER BY cleanup.created_at DESC,cleanup.id DESC LIMIT 1) AS cleanup_job_status,
        EXISTS(SELECT 1 FROM router_ssh_hosts h WHERE h.id=COALESCE(a.host_id,s.host_id,j.host_id)) AS host_present
 FROM client_market_rental_identities a
 LEFT JOIN client_market_subscriptions s ON s.installation_id=a.installation_id
 LEFT JOIN provisioning_jobs j ON j.id=a.job_id
 LEFT JOIN client_market_allocation_quote_items qi ON qi.quote_id=j.quote_id AND qi.host_id=COALESCE(a.host_id,s.host_id,j.host_id)
 LEFT JOIN market_platform_contracts c ON c.id=COALESCE(s.platform_contract_id,
    (SELECT original.id FROM market_platform_contracts original
     WHERE original.product_kind='client_host' AND original.product_ref=a.job_id))
 LEFT JOIN client_market_terminations t ON t.installation_id=a.installation_id
 LEFT JOIN client_cleanup_receipts r ON r.installation_id=a.installation_id
), lifecycle AS (
 SELECT facts.*,
  CASE WHEN (job_status='failed' AND activated_at IS NULL) OR contract_status='activation_failed' THEN 'activation_failed'
       WHEN subscription_status IN ('releasing','release_failed','released') OR contract_status IN ('ending','ended') THEN 'ended'
       WHEN contract_status='trialing' THEN 'trialing'
       WHEN contract_status IN ('suspended','review_required') OR subscription_status='billing_suspended' THEN 'paused'
       WHEN contract_status='active' THEN 'paid'
       WHEN subscription_status='active' AND COALESCE(pricing_model,'free')='free' THEN 'free'
       WHEN job_status IN ('pending','running') OR contract_status='pending_activation' THEN 'opening'
       ELSE 'unknown' END AS rental_status,
  CASE WHEN cleanup_confirmed_at IS NOT NULL THEN 'verified'
       WHEN cleanup_job_status='failed' OR termination_status='cleanup_failed' OR subscription_status='release_failed' THEN 'failed'
       WHEN cleanup_job_status IN ('pending','running') OR termination_status='cleanup_pending' OR subscription_status='releasing' OR contract_status='ending' THEN 'pending'
       WHEN subscription_status='released' THEN 'unknown'
       ELSE 'not_required' END AS cleanup_status
 FROM facts
), records AS (
 SELECT lifecycle.*,
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
 cycle_price_minor,daily_rate_minor,offer_revision,trial_seconds_remaining,renewal_policy,cleanup_confirmed_at";

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
        "money":null
    }))
}

fn list_tx(conn: &Connection, q: &AuditQuery) -> Result<Value, AppError> {
    let (mut predicate, mut args) = filters(q)?;
    let total: i64 = conn
        .query_row(
            &format!("{FACTS}SELECT COUNT(*) FROM records WHERE {predicate}"),
            args.clone(),
            |r| r.get(0),
        )
        .map_err(db_error("count rental audit records"))?;
    if let Some(cursor) = q.cursor.as_deref() {
        if cursor.len() > 200 {
            return Err(AppError::BadRequest("invalid rental audit cursor".into()));
        }
        let at: String = conn
            .query_row(
                "SELECT created_at FROM client_market_rental_identities WHERE id=?1",
                params![cursor],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error("read rental audit cursor"))?
            .ok_or_else(|| AppError::BadRequest("invalid rental audit cursor".into()))?;
        args.push(SqlValue::Text(at));
        let time_arg = args.len();
        args.push(SqlValue::Text(cursor.into()));
        let id_arg = args.len();
        predicate.push_str(&format!(
            " AND (created_at<?{time_arg} OR (created_at=?{time_arg} AND id<?{id_arg}))"
        ));
    }
    let mut statement=conn.prepare(&format!("{FACTS}SELECT {COLUMNS} FROM records WHERE {predicate} ORDER BY created_at DESC,id DESC LIMIT 51"))
        .map_err(db_error("prepare rental audit records"))?;
    let mut items = statement
        .query_map(args, record_row)
        .map_err(db_error("query rental audit records"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read rental audit records"))?;
    let next = if items.len() > PAGE_SIZE {
        items[PAGE_SIZE - 1]["id"].clone()
    } else {
        Value::Null
    };
    items.truncate(PAGE_SIZE);
    attach_money(conn, &mut items)?;
    Ok(json!({"items":items,"nextCursor":next,"total":total,"serverNow":Utc::now().to_rfc3339()}))
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
