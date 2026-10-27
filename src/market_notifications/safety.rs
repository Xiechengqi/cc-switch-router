//! A second line of defence independent of event/idempotency keys. All checks
//! and first-attempt reservations run inside the channel claim transaction.
use super::*;
use crate::config::ServiceNoticeLimits;

pub(super) const SERVICE_PURPOSES: &str = "'service_paused','service_recovery_summary','service_ending_warning','service_ended','funding_summary','service_status_summary'";

pub(super) fn is_service(purpose: &str) -> bool {
    matches!(
        purpose,
        "service_paused"
            | "service_recovery_summary"
            | "service_ending_warning"
            | "service_ended"
            | "funding_summary"
            | "service_status_summary"
    )
}

pub(super) fn reason_tx(
    conn: &Connection,
    item: &Claimed,
    id: &str,
    channel: &str,
    purpose: &str,
    limits: &ServiceNoticeLimits,
    time: &str,
) -> Result<Option<&'static str>, AppError> {
    if !is_service(purpose) {
        return Ok(None);
    }
    let now = DateTime::parse_from_rfc3339(time)
        .map_err(|_| AppError::Internal("invalid service notice time".into()))?;
    let created = item
        .payload
        .get("notificationTime")
        .and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok());
    if created.is_none_or(|at| {
        (now - at).num_seconds() > limits.max_age_secs || at > now + chrono::Duration::minutes(5)
    }) {
        return Ok(Some("service_notice_expired"));
    }
    // Queries exclude this task: a retry reuses its slot, but a newly lowered
    // limit or emergency zero budget must still stop unaccepted retries.
    let hourly = (now - chrono::Duration::hours(1)).to_rfc3339();
    let daily = (now - chrono::Duration::days(1)).to_rfc3339();
    let count = |sql: &str, args: &[&dyn crate::db::ToSql]| -> Result<i64, AppError> {
        conn.query_row(sql, args, |r| r.get(0))
            .map_err(db_error("read service notice budget"))
    };
    let user_sql = format!(
        "SELECT COUNT(*) FROM market_notification_deliveries WHERE id!=?1
        AND recipient_user_id=?2 AND channel=?3 AND purpose IN ({SERVICE_PURPOSES})
        AND first_attempt_at>=substr(?4,1,10)
        AND julianday(first_attempt_at)>=julianday(?4)"
    );
    if count(
        &user_sql,
        &[&id, &item.recipient_user_id, &channel, &hourly],
    )? >= limits.user_hourly_limit
    {
        return Ok(Some("service_notice_user_budget"));
    }
    if channel == "email" {
        let sql = format!(
            "SELECT COUNT(*) FROM market_notification_deliveries WHERE id!=?1 AND channel='email'
            AND first_attempt_at>=substr(?2,1,10)
            AND purpose IN ({SERVICE_PURPOSES}) AND julianday(first_attempt_at)>=julianday(?2)"
        );
        if count(&sql, &[&id, &daily])? >= limits.email_daily_limit {
            return Ok(Some("service_notice_email_circuit"));
        }
    }
    if purpose == "service_paused" {
        let contract = item
            .payload
            .get("contractId")
            .and_then(Value::as_str)
            .unwrap_or("");
        if contract.is_empty() {
            return Ok(Some("service_notice_identity_missing"));
        }
        let since = (now - chrono::Duration::seconds(limits.pause_cooldown_secs)).to_rfc3339();
        let sql = "SELECT COUNT(*) FROM market_notification_deliveries d
            JOIN market_notification_outbox n ON n.id=d.event_id
            WHERE d.id!=?1 AND d.recipient_user_id=?2 AND d.channel=?3 AND d.purpose='service_paused'
            AND n.object_id=?4 AND julianday(d.first_attempt_at)>=julianday(?5)";
        if count(
            sql,
            &[&id, &item.recipient_user_id, &channel, &contract, &since],
        )? > 0
        {
            return Ok(Some("service_pause_cooldown"));
        }
        if count(
            sql,
            &[&id, &item.recipient_user_id, &channel, &contract, &daily],
        )? >= limits.pause_daily_limit
        {
            return Ok(Some("service_pause_daily_budget"));
        }
    }
    Ok(None)
}

pub(super) fn suppress_tx(
    conn: &Connection,
    item: &Claimed,
    id: &str,
    purpose: &str,
    first: Option<&str>,
    reason: &str,
    limits: &ServiceNoticeLimits,
    time: &str,
) -> Result<String, AppError> {
    let status = if first.is_some() {
        "uncertain"
    } else {
        "suppressed"
    };
    conn.execute(
        "UPDATE market_notification_deliveries SET status=?2,suppression_reason=?3,
        suppressed_at=?4,last_error=?3,claim_token=NULL,claim_expires_at=NULL WHERE id=?1",
        params![id, status, reason, time],
    )
    .map_err(db_error("suppress unsafe service notice"))?;
    if first.is_none()
        && purpose != "service_status_summary"
        && matches!(
            reason,
            "service_pause_cooldown"
                | "service_pause_daily_budget"
                | "service_notice_user_budget"
                | "service_notice_email_circuit"
        )
    {
        let at = DateTime::parse_from_rfc3339(time)
            .map_err(|_| AppError::Internal("invalid digest time".into()))?;
        let due = (at + chrono::Duration::seconds(limits.pause_cooldown_secs)).to_rfc3339();
        conn.execute("INSERT INTO market_notification_service_digests(user_id,window_started_at,due_at,changed_at)
            VALUES(?1,?2,?3,?2) ON CONFLICT(user_id) DO UPDATE SET changed_at=excluded.changed_at,
            version=version+1, window_started_at=CASE WHEN version=queued_version THEN excluded.window_started_at ELSE window_started_at END,
            due_at=CASE WHEN version=queued_version THEN MAX(due_at,excluded.due_at) ELSE due_at END",
            params![item.recipient_user_id,time,due]).map_err(db_error("coalesce service status digest"))?;
        // Signal once per budget window, not once per discarded business event.
        let since = (at - chrono::Duration::hours(1)).to_rfc3339();
        let prior: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_notification_deliveries
            WHERE id!=?1 AND suppression_reason=?2 AND julianday(suppressed_at)>=julianday(?3)",
                params![id, reason, since],
                |r| r.get(0),
            )
            .map_err(db_error("read service circuit alert budget"))?;
        if prior == 0 {
            owner_signal_tx(
                conn,
                "service_notice_circuit",
                "notification_policy",
                reason,
                json!({"summary":"Service reminders were coalesced by the notification safety policy. Financial receipts and authentication are unaffected.","reason":reason}),
                time,
                false,
            )?;
        }
    }
    Ok(status.into())
}

pub(super) fn queue_digests_tx(
    conn: &Connection,
    time: &str,
    limits: &ServiceNoticeLimits,
) -> Result<usize, AppError> {
    let rows = {
        let mut s = conn
            .prepare(
                "SELECT user_id,window_started_at,version FROM market_notification_service_digests
            WHERE version>queued_version AND due_at<=?1 ORDER BY due_at,user_id LIMIT 25",
            )
            .map_err(db_error("prepare service digests"))?;
        s.query_map(params![time], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })
        .map_err(db_error("query service digests"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read service digests"))?
    };
    let now = DateTime::parse_from_rfc3339(time)
        .map_err(|_| AppError::Internal("invalid digest queue time".into()))?;
    for (user, start, version) in &rows {
        enqueue_user_notification_tx(
            conn,
            "buyer",
            user,
            "service_status_summary",
            "service_digest",
            user,
            &format!("service-status:{user}:{version}"),
            json!({"digestVersion":version,"windowStartedAt":start}),
            time,
        )?;
        conn.execute("UPDATE market_notification_service_digests SET queued_version=?2,due_at=?3 WHERE user_id=?1",
            params![user,version,(now+chrono::Duration::seconds(limits.pause_cooldown_secs)).to_rfc3339()])
            .map_err(db_error("advance coalesced service digest"))?;
    }
    Ok(rows.len())
}

/// Snapshot at delivery, not at the time of each discarded event. An email
/// describes current status, not hundreds of obsolete intermediate transitions.
pub(super) fn digest_counts_tx(
    conn: &Connection,
    item: &Claimed,
) -> Result<(i64, i64, i64), AppError> {
    let start = item
        .payload
        .get("windowStartedAt")
        .and_then(Value::as_str)
        .unwrap_or("");
    conn.query_row("SELECT COUNT(*),COALESCE(SUM(status='suspended'),0),COALESCE(SUM(status IN ('active','trialing')),0)
        FROM market_platform_contracts c WHERE c.buyer_user_id=?1 AND EXISTS(
            SELECT 1 FROM market_notification_deliveries d JOIN market_notification_outbox n ON n.id=d.event_id
            WHERE d.recipient_user_id=?1 AND n.object_id=c.id AND d.suppression_reason IS NOT NULL
            AND julianday(d.suppressed_at)>=julianday(?2))",params![item.recipient_user_id,start],
        |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(db_error("snapshot service status digest"))
}
