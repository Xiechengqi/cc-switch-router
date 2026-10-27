//! Read-only, administrator-only projections; never expose frozen envelopes.
use super::*;

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Filters {
    cursor: Option<String>,
    status: Option<String>,
    purpose: Option<String>,
    recipient: Option<String>,
}

fn groups(conn: &Connection, sql: &str) -> Result<Value, AppError> {
    let mut s = conn
        .prepare(sql)
        .map_err(db_error("prepare notification diagnostics"))?;
    let rows = s
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(db_error("query notification diagnostics"))?;
    let mut result = serde_json::Map::new();
    for row in rows {
        let (key, count) = row.map_err(db_error("read notification diagnostics"))?;
        result.insert(key, json!(count));
    }
    Ok(Value::Object(result))
}
fn overview_tx(conn: &Connection) -> Result<Value, AppError> {
    let last:Value=conn.query_row("SELECT MAX(created_at),
        (SELECT MAX(d.sent_at) FROM market_notification_deliveries d WHERE d.channel='email' AND d.purpose='service_paused' AND d.status='sent'),
        SUM(CASE WHEN status IN ('queued','claimed') THEN 1 ELSE 0 END)
        FROM market_notification_outbox WHERE event_kind='contract_suspended'",[],|r|Ok(json!({
            "eventCreatedAt":r.get::<_,Option<String>>(0)?,"acceptedAt":r.get::<_,Option<String>>(1)?,
            "pending":r.get::<_,Option<i64>>(2)?.unwrap_or(0)}))).map_err(db_error("read pause diagnostics"))?;
    let health:Value=conn.query_row("SELECT last_poll_at,last_webhook_at,poll_error_code,poll_backoff_until
        FROM market_notification_tracking_health WHERE provider='resend'",[],|r|Ok(json!({
            "lastPollAt":r.get::<_,Option<String>>(0)?,"lastWebhookAt":r.get::<_,Option<String>>(1)?,
            "pollErrorCode":r.get::<_,Option<String>>(2)?,"pollBackoffUntil":r.get::<_,Option<String>>(3)?})))
        .map_err(db_error("read tracking health"))?;
    Ok(
        json!({"events":groups(conn,"SELECT status,COUNT(*) FROM market_notification_outbox GROUP BY status")?,
        "submissions":groups(conn,"SELECT status,COUNT(*) FROM market_notification_deliveries WHERE channel='email' GROUP BY status")?,
        "provider":groups(conn,"SELECT COALESCE(provider_status,'untracked'),COUNT(*) FROM market_notification_deliveries WHERE channel='email' AND status='sent' GROUP BY provider_status")?,
        "suppressions":groups(conn,"SELECT COALESCE(suppression_reason,'legacy_suppression'),COUNT(*) FROM market_notification_deliveries WHERE status='suppressed' GROUP BY suppression_reason")?,
        "pause":last,"tracking":health}),
    )
}
fn masked_email(value: Option<String>) -> String {
    value
        .and_then(|s| {
            s.split_once('@').map(|(local, domain)| {
                format!("{}***@{domain}", local.chars().take(2).collect::<String>())
            })
        })
        .unwrap_or_else(|| "—".into())
}
fn page_tx(conn: &Connection, filters: &Filters) -> Result<Value, AppError> {
    for filter in [
        &filters.cursor,
        &filters.status,
        &filters.purpose,
        &filters.recipient,
    ]
    .into_iter()
    .flatten()
    {
        if filter.is_empty() || filter.len() > 128 {
            return Err(AppError::BadRequest("invalid notification filter".into()));
        }
    }
    let cursor = filters
        .cursor
        .as_deref()
        .map(|id| {
            conn.query_row(
                "SELECT created_at FROM market_notification_deliveries WHERE id=?1",
                params![id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error("read notification diagnostic cursor"))?
            .ok_or_else(|| AppError::BadRequest("invalid notification cursor".into()))
        })
        .transpose()?;
    let mut s=conn.prepare("SELECT d.id,d.event_id,d.recipient_user_id,d.channel,d.purpose,d.status,d.provider_status,d.provider_message_id,
        d.attempts,d.created_at,d.first_attempt_at,d.sent_at,d.provider_status_at,d.provider_checked_at,
        d.suppression_reason,d.last_error,d.provider_error,n.object_id,n.event_kind,u.email_normalized
        FROM market_notification_deliveries d JOIN market_notification_outbox n ON n.id=d.event_id
        LEFT JOIN users u ON u.id=d.recipient_user_id
        WHERE (?1 IS NULL OR d.status=?1 OR d.provider_status=?1
            OR (?1='failed' AND d.provider_status IN ('bounced','complained')))
        AND (?2 IS NULL OR d.purpose=?2)
        AND (?3 IS NULL OR d.recipient_user_id=?3)
        AND (?4 IS NULL OR d.created_at<?4 OR (d.created_at=?4 AND d.id<?5))
        ORDER BY d.created_at DESC,d.id DESC LIMIT 51").map_err(db_error("prepare notification diagnostic page"))?;
    let mut rows=s.query_map(params![filters.status,filters.purpose,filters.recipient,cursor,filters.cursor],|r|Ok(json!({
        "id":r.get::<_,String>(0)?,"eventId":r.get::<_,String>(1)?,"recipientId":r.get::<_,String>(2)?,
        "channel":r.get::<_,String>(3)?,"purpose":r.get::<_,String>(4)?,"submissionStatus":r.get::<_,String>(5)?,
        "providerStatus":r.get::<_,Option<String>>(6)?,"providerMessageId":r.get::<_,Option<String>>(7)?,
        "attempts":r.get::<_,i64>(8)?,"createdAt":r.get::<_,String>(9)?,"firstAttemptAt":r.get::<_,Option<String>>(10)?,
        "acceptedAt":r.get::<_,Option<String>>(11)?,"providerStatusAt":r.get::<_,Option<String>>(12)?,
        "providerCheckedAt":r.get::<_,Option<String>>(13)?,"suppressionReason":r.get::<_,Option<String>>(14)?,
        "submissionError":r.get::<_,Option<String>>(15)?,"providerError":r.get::<_,Option<String>>(16)?,
        "objectId":r.get::<_,String>(17)?,"eventKind":r.get::<_,String>(18)?,"targetMasked":masked_email(r.get(19)?)})))
        .map_err(db_error("query notification diagnostic page"))?.collect::<Result<Vec<_>,_>>()
        .map_err(db_error("read notification diagnostic page"))?;
    let more = rows.len() > 50;
    rows.truncate(50);
    let cursor = if more {
        rows.last().and_then(|v| v.get("id")).cloned()
    } else {
        None
    };
    Ok(json!({"items":rows,"nextCursor":cursor}))
}
async fn diagnostics(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(filters): Query<Filters>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::api::require_admin_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let mut result = page_tx(&conn, &filters)?;
    result["overview"] = overview_tx(&conn)?;
    result["limits"] = json!(state.config.service_notice_limits);
    result["webhookConfigured"] = json!(
        state
            .config
            .resend_webhook_secret
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
    );
    Ok((no_store(), Json(result)))
}
pub(super) fn router() -> Router<ServerState> {
    Router::new().route("/v1/admin/market-notifications", get(diagnostics))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostics_are_paginated_and_never_include_envelopes_or_raw_email() {
        let c = crate::test_support::database().unwrap();
        c.execute("INSERT INTO users(id,email_normalized,status,created_at,last_login_at) VALUES('recipient','private@example.test','active','t','t')",[]).unwrap();
        for n in 0..53 {
            enqueue_user_notification_tx(
                &c,
                "buyer",
                "recipient",
                "wallet_transfer_sent",
                "transfer",
                "x",
                &format!("e{n}"),
                json!({}),
                "2026-10-27T00:00:00Z",
            )
            .unwrap();
            c.execute("INSERT INTO market_notification_deliveries(id,event_id,recipient_user_id,channel,purpose,semantic_key,status,envelope_json,idempotency_key,next_attempt_at,created_at)
                SELECT ?1,id,'recipient','email','receipt',?1,'suppressed','{\"secret\":\"DO_NOT_EXPOSE\"}',?1,created_at,created_at FROM market_notification_outbox WHERE dedupe_key=?2",
                params![format!("d{n:03}"),format!("e{n}")]).unwrap();
        }
        let first = page_tx(&c, &Filters::default()).unwrap();
        assert_eq!(first["items"].as_array().unwrap().len(), 50);
        let second = page_tx(
            &c,
            &Filters {
                cursor: first["nextCursor"].as_str().map(str::to_string),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(second["items"].as_array().unwrap().len(), 3);
        for result in [first, second] {
            let raw = result.to_string();
            assert!(!raw.contains("DO_NOT_EXPOSE"));
            assert!(!raw.contains("private@example.test"));
            assert!(raw.contains("pr***@example.test"));
        }
        assert_eq!(overview_tx(&c).unwrap()["submissions"]["suppressed"], 53);
    }
}
