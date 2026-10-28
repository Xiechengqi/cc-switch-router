//! Semantic notification policy and independently persisted channel attempts.
//! No network call is made while holding a database lock.
use super::*;
use crate::notifications::{DeliveryFailure, FrozenEmailEnvelope};
use sha2::{Digest, Sha256};

fn text_field<'a>(item: &'a Claimed, key: &str) -> &'a str {
    item.payload.get(key).and_then(Value::as_str).unwrap_or("")
}
pub(super) fn purpose(item: &Claimed) -> Option<&'static str> {
    match item.event_kind.as_str() {
        "topup_closed_unpaid" | "refund_status" | "earnings_converted" => None,
        "c2c_order_updated" => {
            let role = text_field(item, "role");
            match text_field(item, "action") {
                "released" => Some("c2c_receipt"),
                "paid" if role == "seller" => Some("c2c_payment_reported"),
                "release_reminder" if role == "seller" => Some("c2c_release_reminder"),
                "disputed" | "late_payment_reported" | "overdue_escalated" => {
                    if text_field(item, "actorId") == item.recipient_user_id {
                        None
                    } else {
                        Some("c2c_review")
                    }
                }
                "return_to_seller" | "record_external_resolution" => Some("c2c_resolution"),
                _ => None,
            }
        }
        "c2c_seller_changed" => match text_field(item, "reason") {
            "paid_order_overdue" | "suspended" | "unsuspended" | "auto_accept_consent_required" => {
                Some("c2c_seller_safety")
            }
            _ => None,
        },
        "wallet_low_runway" | "renewal_upcoming_shortfall" => Some("funding_summary"),
        "contract_resumed" => Some("service_recovery_summary"),
        "topup_credited" => Some("topup_receipt"),
        "topup_review_required" => Some("topup_payment_review"),
        "topup_reversed" => Some("topup_adjustment"),
        "wallet_transfer_sent" => Some("transfer_sent_receipt"),
        "wallet_transfer_received" => Some("transfer_received_receipt"),
        "commission_policy_changed" => Some("provider_fee_change"),
        "earnings_held" => Some("provider_hold"),
        "earnings_released" => Some("provider_hold_released"),
        "contract_suspended" => Some("service_paused"),
        "service_status_summary" => Some("service_status_summary"),
        "contract_terminating_soon" => Some("service_ending_warning"),
        "contract_terminated" => Some("service_ended"),
        "client_termination_settled" => Some("client_termination_receipt"),
        "client_termination_claim" => Some("client_termination_claim"),
        "dispute_resolved" => Some("service_dispute_decision"),
        _ => None,
    }
}

pub(super) fn semantic_key(item: &Claimed, purpose: &str) -> String {
    let created = text_field(item, "notificationTime");
    if purpose == "service_paused" {
        let contract = text_field(item, "contractId");
        let started = text_field(item, "suspensionStartedAt");
        if !contract.is_empty() && !started.is_empty() {
            // Keep the historical key shape so an attempted legacy envelope
            // keeps its original provider idempotency key after an upgrade.
            return format!("service_paused:contract_suspended:{contract}:{started}");
        }
    }
    if purpose == "funding_summary" {
        return format!("funding:{}", created.get(..10).unwrap_or(&item.id));
    }
    if purpose == "service_recovery_summary" {
        let bucket = DateTime::parse_from_rfc3339(created)
            .map(|t| t.timestamp() / 300)
            .unwrap_or(0);
        return format!("service-recovery:{bucket}");
    }
    if purpose == "c2c_payment_reported"
        || purpose == "c2c_release_reminder"
        || purpose == "c2c_receipt"
    {
        return format!("{purpose}:{}", text_field(item, "orderId"));
    }
    format!(
        "{purpose}:{}",
        item.payload
            .get("notificationEventKey")
            .and_then(Value::as_str)
            .unwrap_or(&item.id)
    )
}

pub(super) fn english_sender(configured: &str) -> Option<String> {
    let address = if let Some((_, tail)) = configured.rsplit_once('<') {
        tail.strip_suffix('>')?.trim()
    } else {
        configured.trim()
    };
    if !address.is_ascii()
        || !address.contains('@')
        || address
            .bytes()
            .any(|c| c.is_ascii_whitespace() || matches!(c, b'<' | b'>'))
    {
        return None;
    }
    Some(format!("TokenSwitch <{address}>"))
}

pub(super) fn failure(
    message: &str,
    retryable: bool,
    retry_at: Option<DateTime<Utc>>,
) -> DeliveryFailure {
    DeliveryFailure {
        retryable,
        retry_at,
        message: message.into(),
        failure_code: None,
        failure_hint: None,
        failure_details: None,
    }
}

pub(super) struct Attempt {
    pub id: String,
    pub token: String,
    pub envelope: Value,
    pub key: String,
    pub attempts: i64,
}
pub(super) enum Prepared {
    Done(String),
    Deferred(DateTime<Utc>),
    Send(Attempt),
}

/// Persist before the provider request. A changed template/config can never
/// mutate the body under an already-used provider idempotency key.
#[cfg(test)]
pub(super) fn prepare_tx(
    conn: &Connection,
    item: &Claimed,
    channel: &str,
    purpose: &str,
    envelope: Value,
    ready: bool,
    time: &str,
) -> Result<Prepared, AppError> {
    prepare_with_limits_tx(
        conn,
        item,
        channel,
        purpose,
        envelope,
        ready,
        time,
        &crate::config::ServiceNoticeLimits::default(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_with_limits_tx(
    conn: &Connection,
    item: &Claimed,
    channel: &str,
    purpose: &str,
    envelope: Value,
    ready: bool,
    time: &str,
    limits: &crate::config::ServiceNoticeLimits,
) -> Result<Prepared, AppError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin channel notification claim"))?;
    let prepared = prepare_in_tx(&tx, item, channel, purpose, envelope, ready, time, limits)?;
    tx.commit()
        .map_err(db_error("commit channel notification claim"))?;
    Ok(prepared)
}

#[allow(clippy::too_many_arguments)]
fn prepare_in_tx(
    conn: &Connection,
    item: &Claimed,
    channel: &str,
    purpose: &str,
    envelope: Value,
    ready: bool,
    time: &str,
    limits: &crate::config::ServiceNoticeLimits,
) -> Result<Prepared, AppError> {
    let semantic = semantic_key(item, purpose);
    let id = Uuid::new_v4().to_string();
    let key = format!("market-delivery-{id}");
    conn.execute("INSERT OR IGNORE INTO market_notification_deliveries(id,event_id,recipient_user_id,channel,purpose,semantic_key,status,envelope_json,idempotency_key,next_attempt_at,created_at)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?10)",
        params![id,item.id,item.recipient_user_id,channel,purpose,semantic,if ready {"queued"}else{"blocked_config"},envelope.to_string(),key,time]).map_err(db_error("create channel notification"))?;
    let (id,status,saved,key,attempts,first,lease,next):(String,String,String,String,i64,Option<String>,Option<String>,String)=conn.query_row(
        "SELECT id,status,envelope_json,idempotency_key,attempts,first_attempt_at,claim_expires_at,next_attempt_at FROM market_notification_deliveries WHERE recipient_user_id=?1 AND channel=?2 AND semantic_key=?3",
        params![item.recipient_user_id,channel,semantic],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).map_err(db_error("read channel notification"))?;
    if matches!(
        status.as_str(),
        "sent" | "suppressed" | "failed" | "uncertain"
    ) {
        return Ok(Prepared::Done(status));
    }
    let saved: Value = serde_json::from_str(&saved)
        .map_err(|_| AppError::Internal("invalid frozen notification envelope".into()))?;
    let parse = |value: &str| {
        DateTime::parse_from_rfc3339(value)
            .map(|t| t.with_timezone(&Utc))
            .map_err(|_| AppError::Internal("invalid persisted notification deadline".into()))
    };
    if status == "claimed" && lease.as_deref().is_some_and(|at| at > time) {
        return Ok(Prepared::Deferred(parse(lease.as_deref().unwrap())?));
    }
    if status == "queued" && next.as_str() > time {
        return Ok(Prepared::Deferred(parse(&next)?));
    }
    // Telegram has no idempotency key. A crashed in-flight send cannot be
    // replayed safely: preserve an explicit uncertain result, not a duplicate.
    if channel == "telegram" && status == "claimed" && first.is_some() {
        conn.execute("UPDATE market_notification_deliveries SET status='uncertain',claim_token=NULL,claim_expires_at=NULL,last_error='Telegram acceptance uncertain after expired lease' WHERE id=?1",params![id]).map_err(db_error("record uncertain Telegram delivery"))?;
        return Ok(Prepared::Done("uncertain".into()));
    }
    if let Some(first) = first.as_deref() {
        let age = DateTime::parse_from_rfc3339(time)
            .ok()
            .zip(DateTime::parse_from_rfc3339(first).ok())
            .map(|(a, b)| a - b);
        let changed_account =
            saved.get("credentialFingerprint") != envelope.get("credentialFingerprint");
        if age.is_none_or(|age| age >= chrono::Duration::hours(23)) || changed_account {
            conn.execute("UPDATE market_notification_deliveries SET status='uncertain',claim_token=NULL,claim_expires_at=NULL,last_error='delivery acceptance uncertain; automatic resend suppressed' WHERE id=?1",params![id]).map_err(db_error("bound ambiguous notification retry"))?;
            return Ok(Prepared::Done("uncertain".into()));
        }
    }
    if !ready {
        // Explicitly unconfigured email is feed-only, not a successful email.
        // Configuring mail later does not flood users with historical receipts.
        conn.execute("UPDATE market_notification_deliveries SET status='blocked_config',last_error='notification channel is not configured' WHERE id=?1",params![id]).map_err(db_error("record disabled notification channel"))?;
        return Ok(Prepared::Done("blocked_config".into()));
    }
    if let Some(reason) = super::safety::reason_tx(conn, item, &id, channel, purpose, limits, time)?
    {
        let status = super::safety::suppress_tx(
            conn,
            item,
            &id,
            purpose,
            first.as_deref(),
            reason,
            limits,
            time,
        )?;
        return Ok(Prepared::Done(status));
    }
    if first.is_none() {
        conn.execute("UPDATE market_notification_deliveries SET envelope_json=?2,status='queued' WHERE id=?1 AND first_attempt_at IS NULL",params![id,envelope.to_string()]).map_err(db_error("freeze notification envelope"))?;
    } else if saved.get("target") != envelope.get("target") {
        conn.execute("UPDATE market_notification_deliveries SET status='suppressed',last_error='verified recipient changed' WHERE id=?1",params![id]).map_err(db_error("suppress old notification target"))?;
        return Ok(Prepared::Done("suppressed".into()));
    }
    let token = Uuid::new_v4().to_string();
    let at = DateTime::parse_from_rfc3339(time)
        .map_err(|_| AppError::Internal("invalid notification claim time".into()))?;
    let expires = (at + chrono::Duration::seconds(CLAIM_SECS)).to_rfc3339();
    let changed=conn.execute("UPDATE market_notification_deliveries SET status='claimed',claim_token=?2,claim_expires_at=?3,attempts=attempts+1,first_attempt_at=COALESCE(first_attempt_at,?4)
        WHERE id=?1 AND next_attempt_at<=?4 AND (status IN ('queued','blocked_config') OR (status='claimed' AND claim_expires_at<=?4))",
        params![id,token,expires,time]).map_err(db_error("claim channel notification"))?;
    if changed != 1 {
        return Err(AppError::ServiceUnavailable(
            "channel notification deferred".into(),
        ));
    }
    Ok(Prepared::Send(Attempt {
        id,
        token,
        envelope: if first.is_some() { saved } else { envelope },
        key,
        attempts: attempts + 1,
    }))
}

pub(super) fn finish_attempt_tx(
    conn: &Connection,
    a: &Attempt,
    result: &Result<String, DeliveryFailure>,
    time: &str,
) -> Result<bool, AppError> {
    let (status, provider, error, next) = match result {
        Ok(provider) => ("sent", Some(provider.as_str()), None, time.to_string()),
        Err(e) => {
            let at = DateTime::parse_from_rfc3339(time)
                .map_err(|_| AppError::Internal("invalid delivery finish time".into()))?
                .with_timezone(&Utc);
            let delay = 60_i64 * 2_i64.pow(a.attempts.clamp(0, 8) as u32);
            let next = e
                .retry_at
                .unwrap_or(at + chrono::Duration::seconds(delay.min(21600)))
                .max(at + chrono::Duration::seconds(1))
                .to_rfc3339();
            (
                if e.failure_code.as_deref() == Some("telegram_acceptance_uncertain") {
                    "uncertain"
                } else if e.retryable && a.attempts < MAX_ATTEMPTS {
                    "queued"
                } else {
                    "failed"
                },
                None,
                Some(crate::notifications::sanitize_delivery_error(&e.message)),
                next,
            )
        }
    };
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin channel result"))?;
    let changed=tx.execute("UPDATE market_notification_deliveries SET status=?3,provider_message_id=?4,last_error=?5,next_attempt_at=?6,sent_at=CASE WHEN ?3='sent' THEN ?7 ELSE sent_at END,claim_token=NULL,claim_expires_at=NULL
        WHERE id=?1 AND claim_token=?2 AND status='claimed'",params![a.id,a.token,status,provider,error,next,time]).map_err(db_error("finish channel notification"))?;
    if changed == 1 && status == "sent" {
        super::tracking::accepted_tx(&tx, &a.id, time)?;
    }
    tx.commit().map_err(db_error("commit channel result"))?;
    Ok(changed == 1)
}

fn merge_retry(current: &mut Option<DeliveryFailure>, next: DeliveryFailure) {
    if current
        .as_ref()
        .is_none_or(|prior| next.retry_at < prior.retry_at)
    {
        *current = Some(next);
    }
}

pub(super) async fn deliver(
    state: &ServerState,
    http: &reqwest::Client,
    item: &Claimed,
) -> Result<String, DeliveryFailure> {
    let Some(purpose) = purpose(item) else {
        return Ok("suppressed".into());
    };
    let (recipient, content) = {
        let conn = state.store.conn.lock().await;
        let time = crate::market_wallet::now();
        if !super::notice_current_tx(&conn, item, &time)
            .map_err(|e| failure(&e.to_string(), true, None))?
        {
            if let Some(lease) = super::policy::suppress_obsolete_deliveries_tx(&conn, item, &time)
                .map_err(|e| failure(&e.to_string(), true, None))?
            {
                return Err(failure(
                    "obsolete notification awaits an in-flight delivery result",
                    true,
                    Some(lease),
                ));
            }
            return Ok("superseded".into());
        }
        let recipient =
            recipient_tx(&conn, item).map_err(|e| failure(&e.to_string(), true, None))?;
        let mut content = Claimed {
            id: item.id.clone(),
            recipient_user_id: item.recipient_user_id.clone(),
            event_kind: item.event_kind.clone(),
            payload: item.payload.clone(),
            mandatory: item.mandatory,
            attempts: item.attempts,
        };
        if purpose == "service_status_summary" {
            let (total, paused, active) = super::safety::digest_counts_tx(&conn, item)
                .map_err(|e| failure(&e.to_string(), true, None))?;
            if total == 0 {
                return Ok("superseded".into());
            }
            content.payload["relatedCount"] = json!(total);
            content.payload["pausedCount"] = json!(paused);
            content.payload["activeCount"] = json!(active);
            content.payload["statusAsOf"] = json!(time);
        } else if super::safety::is_service(purpose) {
            super::email::enrich_service_tx(&conn, &mut content)
                .map_err(|e| failure(&e.to_string(), true, None))?;
        }
        if matches!(purpose, "funding_summary" | "service_recovery_summary") {
            let count =
                super::policy::related_count_tx(&conn, item, purpose, &crate::market_wallet::now())
                    .map_err(|e| failure(&e.to_string(), true, None))?;
            if count == 0 {
                return Ok("superseded".into());
            }
            content.payload["relatedCount"] = json!(count);
            if let Some(p) = content.payload.as_object_mut() {
                p.remove("contractId");
                p.remove("amountMinor");
                p.remove("renewsAt");
            }
        }
        (recipient, content)
    };
    if recipient.opted_out {
        return Ok("suppressed".into());
    }
    if item.event_kind == "c2c_order_updated" && text_field(item, "role") == "handler" {
        let dynamic = state.dynamic.read().await;
        if recipient
            .email
            .as_ref()
            .is_none_or(|email| !dynamic.is_admin(email))
        {
            return Ok("superseded".into());
        }
    }
    let scheme = if state.config.use_localhost {
        "http"
    } else {
        "https"
    };
    let dashboard = format!(
        "{scheme}://{}",
        state.config.tunnel_domain.trim_end_matches('/')
    );
    // Both channels reuse a semantic task; only failed channels are retried.
    let sender =
        crate::notifications::notification_sender(&state.config).and_then(|s| english_sender(&s));
    let email_ready = recipient.email.is_some()
        && sender.is_some()
        && state
            .config
            .resend_api_key
            .as_deref()
            .is_some_and(|v| !v.trim().is_empty());
    let (subject, html, text) = super::render_email(&content, &dashboard);
    let fingerprint = |key: &str| hex::encode(Sha256::digest(key.as_bytes()));
    let email = json!({"target":recipient.email,"from":sender,"replyTo":state.config.resend_reply_to,"subject":subject,"html":html,"text":text,"credentialFingerprint":state.config.resend_api_key.as_deref().map(fingerprint)});
    let mut channels = vec![(
        "email",
        email,
        email_ready,
        state.config.resend_api_key.clone(),
    )];
    if let Some(chat) = recipient.telegram_chat.as_deref() {
        let token = state
            .dynamic
            .read()
            .await
            .telegram_bot
            .token()
            .map(str::to_string);
        let (_, body) = super::render(&content, &dashboard);
        channels.push(("telegram",json!({"target":chat,"text":body,"credentialFingerprint":token.as_deref().map(fingerprint)}),token.is_some(),token));
    }
    let mut outcomes = Vec::new();
    let mut retry: Option<DeliveryFailure> = None;
    for (channel, envelope, ready, credential) in channels {
        let prepared = {
            let conn = state.store.conn.lock().await;
            let time = crate::market_wallet::now();
            // Recheck each channel immediately before claiming: Telegram/config
            // lookups and a sibling network attempt may have outlived the state.
            match super::notice_current_tx(&conn, item, &time) {
                Err(e) => Err(e),
                Ok(false) => super::policy::suppress_obsolete_deliveries_tx(&conn, item, &time)
                    .map(|lease| {
                        lease
                            .map(Prepared::Deferred)
                            .unwrap_or_else(|| Prepared::Done("superseded".into()))
                    }),
                Ok(true) => prepare_with_limits_tx(
                    &conn,
                    item,
                    channel,
                    purpose,
                    envelope,
                    ready,
                    &time,
                    &state.config.service_notice_limits,
                ),
            }
        };
        let prepared = match prepared {
            Ok(v) => v,
            Err(e) => {
                merge_retry(&mut retry, failure(&e.to_string(), true, None));
                continue;
            }
        };
        let a = match prepared {
            Prepared::Done(status) => {
                outcomes.push(format!("{channel}:{status}"));
                continue;
            }
            Prepared::Deferred(at) => {
                merge_retry(
                    &mut retry,
                    failure("channel delivery deferred", true, Some(at)),
                );
                continue;
            }
            Prepared::Send(a) => a,
        };
        let value = |key: &str| a.envelope.get(key).and_then(Value::as_str).unwrap_or("");
        let result = if channel == "email" {
            crate::notifications::send_resend_frozen_email(
                http,
                credential.as_deref().unwrap_or(""),
                FrozenEmailEnvelope {
                    from: value("from"),
                    recipient: value("target"),
                    subject: value("subject"),
                    html: value("html"),
                    text: value("text"),
                    reply_to: a.envelope.get("replyTo").and_then(Value::as_str),
                    idempotency_key: &a.key,
                },
            )
            .await
        } else {
            crate::telegram::send_message(
                http,
                credential.as_deref().unwrap_or(""),
                value("target"),
                None,
                value("text"),
                crate::telegram::TelegramParseMode::Plain,
            )
            .await
            .map(|sent| {
                sent.provider_message_id
                    .unwrap_or_else(|| "telegram-accepted".into())
            })
            .map_err(|e| {
                let mut result = failure(
                    &e.message,
                    e.retryable,
                    e.retry_at.and_then(|t| DateTime::from_timestamp(t, 0)),
                );
                if e.http_status.is_none()
                    && !matches!(
                        e.code,
                        crate::telegram::TelegramFailureCode::Configuration
                            | crate::telegram::TelegramFailureCode::DnsResolutionFailed
                            | crate::telegram::TelegramFailureCode::ApiEndpointUnreachable
                            | crate::telegram::TelegramFailureCode::TlsHandshakeFailed
                    )
                {
                    result.retryable = false;
                    result.failure_code = Some("telegram_acceptance_uncertain".into());
                }
                result
            })
        };
        let finished = {
            let conn = state.store.conn.lock().await;
            finish_attempt_tx(&conn, &a, &result, &crate::market_wallet::now())
        };
        if !finished.unwrap_or(false) {
            retry = Some(failure("notification claim ownership changed", true, None));
            continue;
        }
        match result {
            Ok(_) => outcomes.push(format!("{channel}:sent")),
            Err(e) => {
                tracing::warn!(
                    channel,
                    retryable = e.retryable,
                    "market channel notification failed"
                );
                if e.retryable && a.attempts < MAX_ATTEMPTS {
                    let next = {
                        let conn = state.store.conn.lock().await;
                        conn.query_row("SELECT next_attempt_at FROM market_notification_deliveries WHERE id=?1",params![a.id],|r|r.get::<_,String>(0)).ok()
                            .and_then(|at|DateTime::parse_from_rfc3339(&at).ok()).map(|at|at.with_timezone(&Utc))
                    };
                    merge_retry(
                        &mut retry,
                        failure("notification channel retry scheduled", true, next),
                    );
                } else {
                    let status =
                        if e.failure_code.as_deref() == Some("telegram_acceptance_uncertain") {
                            "uncertain"
                        } else {
                            "failed"
                        };
                    outcomes.push(format!("{channel}:{status}"));
                }
            }
        }
    }
    if let Some(error) = retry {
        Err(error)
    } else {
        Ok(outcomes.join(","))
    }
}
