//! Resend receipts are observations, never instructions to send another email.
use super::*;
use axum::body::Bytes;
use axum::extract::DefaultBodyLimit;
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::StreamExt;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

const EMAILS: &str = "https://api.resend.com/emails";
const BODY_LIMIT: usize = 64 * 1024;

#[derive(Default)]
struct WebhookBudget {
    sources: std::collections::HashMap<std::net::IpAddr, (std::time::Instant, usize)>,
    window: Option<std::time::Instant>,
    count: usize,
}
impl WebhookBudget {
    fn allow(&mut self, ip: std::net::IpAddr, now: std::time::Instant) -> bool {
        if self
            .window
            .is_none_or(|at| now.saturating_duration_since(at) >= Duration::from_secs(60))
        {
            self.window = Some(now);
            self.count = 0;
            self.sources
                .retain(|_, (at, _)| now.saturating_duration_since(*at) < Duration::from_secs(60));
        }
        if self.count >= 600 {
            return false;
        }
        if !self.sources.contains_key(&ip) && self.sources.len() >= 4096 {
            return false;
        }
        let (at, count) = self.sources.entry(ip).or_insert((now, 0));
        if now.saturating_duration_since(*at) >= Duration::from_secs(60) {
            *at = now;
            *count = 0;
        }
        if *count >= 120 {
            return false;
        }
        *count += 1;
        self.count += 1;
        true
    }
}
pub(super) fn allow_webhook_ip(ip: std::net::IpAddr) -> bool {
    static BUDGET: std::sync::OnceLock<std::sync::Mutex<WebhookBudget>> =
        std::sync::OnceLock::new();
    BUDGET
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .allow(ip, std::time::Instant::now())
}

#[cfg(test)]
mod tests;

fn rank(status: &str) -> i64 {
    match status {
        "accepted" => 1,
        "delayed" => 2,
        "failed" | "suppressed" => 3,
        "delivered" => 4,
        "bounced" => 5,
        "complained" => 6,
        _ => 0,
    }
}
fn normalized(event: &str) -> Option<&'static str> {
    match event.trim_start_matches("email.") {
        "sent" | "queued" => Some("accepted"),
        "delivery_delayed" => Some("delayed"),
        "delivered" | "opened" | "clicked" => Some("delivered"),
        "failed" => Some("failed"),
        "bounced" => Some("bounced"),
        "complained" => Some("complained"),
        "suppressed" => Some("suppressed"),
        _ => None,
    }
}
fn error_code(value: Option<&str>) -> Option<String> {
    // A bounded machine code, not an arbitrary SMTP response, address or body.
    value
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 80
                && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        .map(str::to_string)
}

fn apply_status_tx(
    conn: &Connection,
    message: &str,
    status: &str,
    at: &str,
    error: Option<&str>,
) -> Result<(), AppError> {
    let old: Option<(Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT provider_status,provider_status_at FROM market_notification_deliveries
         WHERE channel='email' AND status='sent' AND provider_message_id=?1 LIMIT 1",
            params![message],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db_error("read provider delivery projection"))?;
    let Some((prior, prior_at)) = old else {
        return Ok(());
    };
    if rank(status) < rank(prior.as_deref().unwrap_or("")) {
        return Ok(());
    }
    if rank(status) == rank(prior.as_deref().unwrap_or(""))
        && prior_at
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            > DateTime::parse_from_rfc3339(at).ok()
    {
        return Ok(());
    }
    conn.execute("UPDATE market_notification_deliveries SET provider_status=?2,provider_status_at=?3,
        provider_error=?4,provider_next_check_at=CASE WHEN ?2 IN ('accepted','delayed') THEN provider_next_check_at ELSE NULL END
        WHERE channel='email' AND status='sent' AND provider_message_id=?1",params![message,status,at,error])
        .map_err(db_error("project provider delivery state"))?;
    Ok(())
}

pub(super) fn accepted_tx(conn: &Connection, id: &str, time: &str) -> Result<(), AppError> {
    conn.execute("UPDATE market_notification_deliveries SET provider_status=COALESCE(provider_status,'accepted'),
        provider_status_at=COALESCE(provider_status_at,?2),provider_next_check_at=?2
        WHERE id=?1 AND channel='email' AND status='sent'",params![id,time])
        .map_err(db_error("record provider acceptance"))?;
    let message: Option<String>=conn.query_row("SELECT provider_message_id FROM market_notification_deliveries WHERE id=?1 AND channel='email'",
        params![id],|r|r.get(0)).optional().map_err(db_error("read accepted message"))?.flatten();
    if let Some(message) = message {
        let rows = {
            let mut s = conn
                .prepare(
                    "SELECT status,occurred_at,error_code FROM market_notification_provider_events
                WHERE provider_message_id=?1 ORDER BY occurred_at,id",
                )
                .map_err(db_error("prepare early provider events"))?;
            s.query_map(params![message], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(db_error("query early provider events"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error("read early provider events"))?
        };
        for (status, at, error) in rows {
            apply_status_tx(conn, &message, &status, &at, error.as_deref())?;
        }
    }
    Ok(())
}

fn record_event_tx(
    conn: &Connection,
    id: &str,
    message: &str,
    status: &str,
    occurred: &str,
    received: &str,
    source: &str,
    error: Option<&str>,
) -> Result<bool, AppError> {
    let changed=conn.execute("INSERT OR IGNORE INTO market_notification_provider_events
        (id,provider_message_id,status,occurred_at,received_at,source,error_code) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![id,message,status,occurred,received,source,error]).map_err(db_error("record provider delivery event"))?;
    if changed == 1 {
        apply_status_tx(conn, message, status, occurred, error)?;
    }
    Ok(changed == 1)
}

fn verify(
    secret: &str,
    headers: &HeaderMap,
    body: &[u8],
    now: DateTime<Utc>,
) -> Result<String, AppError> {
    let invalid = || AppError::Unauthorized("invalid email delivery signature".into());
    let header = |key: &str| {
        headers
            .get(key)
            .and_then(|s| s.to_str().ok())
            .ok_or_else(invalid)
    };
    let id = header("svix-id")?;
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(invalid());
    }
    let timestamp = header("svix-timestamp")?;
    let seconds = timestamp.parse::<i64>().map_err(|_| invalid())?;
    if now.timestamp().abs_diff(seconds) > 300 {
        return Err(invalid());
    }
    let key = STANDARD
        .decode(secret.strip_prefix("whsec_").unwrap_or(secret))
        .map_err(|_| invalid())?;
    if key.len() < 16 || key.len() > 128 {
        return Err(invalid());
    }
    let signatures = header("svix-signature")?;
    if signatures.len() > 2048 {
        return Err(invalid());
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).map_err(|_| invalid())?;
    mac.update(id.as_bytes());
    mac.update(b".");
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body);
    for part in signatures.split_ascii_whitespace().take(16) {
        if let Some(value) = part
            .strip_prefix("v1,")
            .and_then(|s| STANDARD.decode(s).ok())
        {
            if mac.clone().verify_slice(&value).is_ok() {
                return Ok(id.into());
            }
        }
    }
    Err(invalid())
}

fn webhook_tx(conn: &Connection, id: &str, body: &[u8], time: &str) -> Result<bool, AppError> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| AppError::BadRequest("invalid delivery event".into()))?;
    let Some(status) = value
        .get("type")
        .and_then(Value::as_str)
        .and_then(normalized)
    else {
        return Ok(false);
    };
    let message = value
        .pointer("/data/email_id")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| AppError::BadRequest("invalid provider message id".into()))?
        .to_string();
    let occurred = value
        .get("created_at")
        .and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .ok_or_else(|| AppError::BadRequest("invalid provider event time".into()))?
        .with_timezone(&Utc)
        .to_rfc3339();
    if DateTime::parse_from_rfc3339(&occurred).ok()
        > DateTime::parse_from_rfc3339(time)
            .ok()
            .map(|at| at + chrono::Duration::minutes(5))
    {
        return Err(AppError::BadRequest(
            "provider event time is in the future".into(),
        ));
    }
    let known: bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM market_notification_deliveries WHERE channel='email' AND provider_message_id=?1)",
        params![message],|r|r.get(0)).map_err(db_error("bind provider webhook"))?;
    if !known {
        let count:i64=conn.query_row("SELECT COUNT(*) FROM market_notification_provider_events e
            WHERE NOT EXISTS(SELECT 1 FROM market_notification_deliveries d WHERE d.channel='email' AND d.provider_message_id=e.provider_message_id)",[],|r|r.get(0))
            .map_err(db_error("bound early delivery events"))?;
        if count >= 1000 {
            return Ok(false);
        } // Polling recovers any early event not retained.
    }
    let error = error_code(value.pointer("/data/failed/reason").and_then(Value::as_str));
    let changed = record_event_tx(
        conn,
        &format!("webhook:{id}"),
        &message,
        status,
        &occurred,
        time,
        "webhook",
        error.as_deref(),
    )?;
    conn.execute(
        "UPDATE market_notification_tracking_health SET last_webhook_at=?1 WHERE provider='resend'",
        params![time],
    )
    .map_err(db_error("record delivery webhook health"))?;
    Ok(changed)
}

async fn webhook(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let secret = state
        .config
        .resend_webhook_secret
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            AppError::ServiceUnavailable("email delivery webhook is not configured".into())
        })?;
    let now = Utc::now();
    let id = verify(secret, &headers, &body, now)?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin delivery webhook"))?;
    webhook_tx(&tx, &id, &body, &now.to_rfc3339())?;
    tx.commit().map_err(db_error("commit delivery webhook"))?;
    Ok((no_store(), Json(json!({"ok":true}))))
}
pub(super) fn router() -> Router<ServerState> {
    Router::new()
        .route(RESEND_WEBHOOK_PATH, post(webhook))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}

struct Poll {
    id: String,
    message: String,
    token: String,
    attempts: i64,
}
fn claim_poll_tx(
    conn: &Connection,
    fingerprint: &str,
    time: &str,
) -> Result<Option<Poll>, AppError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin delivery tracking claim"))?;
    let backoff:Option<String>=tx.query_row("SELECT poll_backoff_until FROM market_notification_tracking_health WHERE provider='resend'",[],|r|r.get(0))
        .map_err(db_error("read tracking circuit"))?;
    if backoff.as_deref().is_some_and(|at| at > time) {
        tx.commit().map_err(db_error("close tracking claim"))?;
        return Ok(None);
    }
    let now = DateTime::parse_from_rfc3339(time)
        .map_err(|_| AppError::Internal("invalid tracking claim time".into()))?;
    let horizon = (now - chrono::Duration::days(7)).to_rfc3339();
    tx.execute("UPDATE market_notification_deliveries SET provider_next_check_at=NULL,provider_error='tracking_window_elapsed'
        WHERE channel='email' AND status='sent' AND provider_status IN ('accepted','delayed') AND provider_next_check_at IS NOT NULL
        AND julianday(sent_at)<julianday(?1)",params![horizon]).map_err(db_error("bound historical tracking"))?;
    tx.execute("UPDATE market_notification_deliveries SET provider_next_check_at=NULL,provider_error='tracking_credential_changed'
        WHERE channel='email' AND status='sent' AND provider_status IN ('accepted','delayed') AND provider_next_check_at IS NOT NULL
        AND COALESCE(CASE WHEN json_valid(envelope_json) THEN json_extract(envelope_json,'$.credentialFingerprint') END,'')!=?1",
        params![fingerprint]).map_err(db_error("fence historical tracking account"))?;
    let row:Option<(String,String,i64,String)>=tx.query_row("SELECT id,provider_message_id,provider_check_attempts,envelope_json
        FROM market_notification_deliveries WHERE channel='email' AND status='sent' AND provider_status IN ('accepted','delayed')
        AND provider_next_check_at<=?1 AND (provider_poll_expires_at IS NULL OR provider_poll_expires_at<=?1)
        ORDER BY provider_next_check_at,id LIMIT 1",params![time],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))
        .optional().map_err(db_error("select pending delivery observation"))?;
    let result = if let Some((id, message, attempts, envelope)) = row {
        let matches = serde_json::from_str::<Value>(&envelope)
            .ok()
            .and_then(|v| {
                v.get("credentialFingerprint")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .as_deref()
            == Some(fingerprint);
        if !matches || Uuid::parse_str(&message).is_err() {
            tx.execute("UPDATE market_notification_deliveries SET provider_next_check_at=NULL,provider_error=?2 WHERE id=?1",
                params![id,if matches {"tracking_invalid_message_id"}else{"tracking_credential_changed"}]).map_err(db_error("fence tracking credentials"))?;
            None
        } else {
            let token = Uuid::new_v4().to_string();
            tx.execute("UPDATE market_notification_deliveries SET provider_poll_token=?2,provider_poll_expires_at=?3,provider_check_attempts=provider_check_attempts+1 WHERE id=?1",
                params![id,token,(now+chrono::Duration::seconds(60)).to_rfc3339()]).map_err(db_error("lease delivery observation"))?;
            Some(Poll {
                id,
                message,
                token,
                attempts: attempts + 1,
            })
        }
    } else {
        None
    };
    tx.commit()
        .map_err(db_error("commit delivery tracking claim"))?;
    Ok(result)
}

struct PollFailure {
    code: &'static str,
    delay: i64,
    global: bool,
}
async fn lookup(
    http: &reqwest::Client,
    key: &str,
    message: &str,
    endpoint: &str,
) -> Result<&'static str, PollFailure> {
    let response = http
        .get(format!("{endpoint}/{message}"))
        .bearer_auth(key)
        .send()
        .await
        .map_err(|_| PollFailure {
            code: "tracking_network_error",
            delay: 300,
            global: true,
        })?;
    if !response.status().is_success() {
        let (code, delay, global) = match response.status().as_u16() {
            401 | 403 => ("tracking_read_permission_required", 21600, true),
            429 => (
                "tracking_rate_limited",
                response
                    .headers()
                    .get("retry-after")
                    .and_then(|h| h.to_str().ok())
                    .and_then(|h| {
                        h.parse::<i64>().ok().or_else(|| {
                            httpdate::parse_http_date(h).ok().map(|at| {
                                DateTime::<Utc>::from(at)
                                    .signed_duration_since(Utc::now())
                                    .num_seconds()
                            })
                        })
                    })
                    .unwrap_or(300)
                    .clamp(60, 86400),
                true,
            ),
            404 => ("tracking_message_not_found", 21600, false),
            _ => ("tracking_gateway_error", 300, true),
        };
        return Err(PollFailure {
            code,
            delay,
            global,
        });
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| PollFailure {
            code: "tracking_response_error",
            delay: 300,
            global: true,
        })?;
        if body.len() + chunk.len() > BODY_LIMIT {
            return Err(PollFailure {
                code: "tracking_response_too_large",
                delay: 3600,
                global: true,
            });
        }
        body.extend_from_slice(&chunk);
    }
    let v: Value = serde_json::from_slice(&body).map_err(|_| PollFailure {
        code: "tracking_invalid_response",
        delay: 3600,
        global: true,
    })?;
    if v.get("id").and_then(Value::as_str) != Some(message) {
        return Err(PollFailure {
            code: "tracking_identity_mismatch",
            delay: 21600,
            global: true,
        });
    }
    v.get("last_event")
        .and_then(Value::as_str)
        .and_then(normalized)
        .ok_or(PollFailure {
            code: "tracking_unknown_status",
            delay: 3600,
            global: false,
        })
}
fn finish_poll_tx(
    conn: &Connection,
    poll: &Poll,
    result: Result<&str, PollFailure>,
    time: &str,
) -> Result<(), AppError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin delivery observation"))?;
    let owned:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM market_notification_deliveries WHERE id=?1 AND provider_poll_token=?2)",
        params![poll.id,poll.token],|r|r.get(0)).map_err(db_error("check observation lease"))?;
    if !owned {
        return Ok(());
    }
    let at = DateTime::parse_from_rfc3339(time)
        .map_err(|_| AppError::Internal("invalid observation time".into()))?;
    let (delay, error) = match result {
        Ok(status) => {
            record_event_tx(
                &tx,
                &format!("poll:{}:{status}", poll.id),
                &poll.message,
                status,
                time,
                time,
                "poll",
                None,
            )?;
            tx.execute("UPDATE market_notification_tracking_health SET last_poll_at=?1,poll_error_code=NULL,poll_backoff_until=NULL WHERE provider='resend'",params![time])
                .map_err(db_error("record tracking success"))?;
            (
                (60_i64 * 2_i64.pow(poll.attempts.clamp(1, 8) as u32)).min(21600),
                None,
            )
        }
        Err(f) => {
            if f.global {
                tx.execute("UPDATE market_notification_tracking_health SET last_poll_at=?1,poll_error_code=?2,poll_backoff_until=?3 WHERE provider='resend'",
                params![time,f.code,(at+chrono::Duration::seconds(f.delay)).to_rfc3339()]).map_err(db_error("open delivery tracking circuit"))?;
            }
            (f.delay, Some(f.code))
        }
    };
    tx.execute("UPDATE market_notification_deliveries SET provider_checked_at=?3,provider_poll_token=NULL,provider_poll_expires_at=NULL,
        provider_error=CASE WHEN ?4 IS NOT NULL THEN ?4 WHEN provider_error LIKE 'tracking_%' THEN NULL ELSE provider_error END,
        provider_next_check_at=CASE WHEN provider_status IN ('accepted','delayed') THEN ?5 ELSE NULL END
        WHERE id=?1 AND provider_poll_token=?2",params![poll.id,poll.token,time,error,(at+chrono::Duration::seconds(delay)).to_rfc3339()])
        .map_err(db_error("finish delivery tracking lease"))?;
    tx.commit()
        .map_err(db_error("commit delivery observation"))?;
    Ok(())
}

pub(super) fn cleanup_tx(conn: &Connection, time: &str) -> Result<(), AppError> {
    // Retain known normalized receipts for 30 days, unmatched callbacks for one.
    conn.execute("DELETE FROM market_notification_provider_events WHERE julianday(received_at)<julianday(?1)-30
        OR (julianday(received_at)<julianday(?1)-1 AND NOT EXISTS(SELECT 1 FROM market_notification_deliveries d
            WHERE d.channel='email' AND d.provider_message_id=market_notification_provider_events.provider_message_id))",params![time])
        .map_err(db_error("expire normalized tracking events"))?;
    Ok(())
}

pub(super) async fn run_service(
    state: ServerState,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let http = reqwest::Client::builder()
        .user_agent("cc-switch-router/0.1 delivery-tracking")
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()?;
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _=tick.tick()=>{
                let now=Utc::now().to_rfc3339();
                {let conn=state.store.conn.lock().await; if cleanup_tx(&conn,&now).is_err(){tracing::warn!("delivery tracking retention deferred");}}
                let Some(key)=state.config.resend_api_key.as_deref().filter(|s|!s.trim().is_empty()) else {continue;};
                let fingerprint=hex::encode(Sha256::digest(key.as_bytes()));
                for _ in 0..5 {
                    if *shutdown.borrow(){break;}
                    let poll={let conn=state.store.conn.lock().await;claim_poll_tx(&conn,&fingerprint,&Utc::now().to_rfc3339())};
                    let poll=match poll{Ok(Some(p))=>p,Ok(None)=>break,Err(_)=>{tracing::warn!("delivery tracking claim deferred");break;}};
                    let result=lookup(&http,key,&poll.message,EMAILS).await;
                    {let conn=state.store.conn.lock().await;if finish_poll_tx(&conn,&poll,result,&Utc::now().to_rfc3339()).is_err(){tracing::warn!("delivery tracking receipt deferred");}}
                    tokio::select!{_=tokio::time::sleep(Duration::from_millis(750))=>{},_=shutdown.changed()=>{return Ok(());}}
                }
            },
            _=shutdown.changed()=>{break;}
        }
    }
    Ok(())
}
