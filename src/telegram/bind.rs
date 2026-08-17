//! Account-to-Telegram binding and user notification-channel preferences.

use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::db::{Connection, OptionalExtension, TransactionBehavior, params};
use crate::error::AppError;
use crate::models::{
    NotificationChannelSettingsResponse, NotificationSettingsResponse,
    TelegramBindLinkResponse, UpdateNotificationSettingsRequest,
};
use crate::notification_channels::{
    EMAIL_CHANNEL, NotificationChannelId, NotificationTarget, NotificationTargets,
    TELEGRAM_CHANNEL, normalize_enabled_channels,
};
use crate::store::AppStore;
use crate::telegram::BIND_TOKEN_BYTES;

const MAX_BIND_LINKS_PER_HOUR: i64 = 10;
const MAX_BIND_LINKS_PER_IP_PER_HOUR: i64 = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindOutcome {
    Bound {
        email: String,
        enabled_channels: Vec<String>,
    },
    AlreadyBound { email: String },
    InvalidToken,
    ChatTakenByAnotherAccount,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramBotRuntime {
    pub readiness: String,
    pub bot_id: Option<String>,
    pub username: Option<String>,
    pub config_fingerprint: Option<String>,
    pub generation: i64,
    pub last_error: Option<String>,
    pub verified_at: Option<String>,
}

impl TelegramBotRuntime {
    pub fn ready(&self) -> bool {
        self.readiness == "ready" && self.bot_id.is_some() && self.username.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramIdentityApplied {
    pub identity_changed: bool,
    pub invalidated_bindings: usize,
    pub generation: i64,
}

#[derive(Debug, Clone)]
struct UserChannelRow {
    channel: String,
    enabled: bool,
    state: String,
    target: Option<String>,
    target_label: Option<String>,
    provider_identity: Option<String>,
    revision: i64,
    verified_at: Option<String>,
}

pub fn generate_bind_token() -> String {
    let mut bytes = [0u8; BIND_TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn hash_bind_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.trim().as_bytes()))
}

pub fn normalize_bind_token(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.len() != BIND_TOKEN_BYTES * 2
        || !trimmed.chars().all(|character| character.is_ascii_hexdigit())
    {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

pub fn deep_link(bot_username: &str, token: &str) -> String {
    format!(
        "https://t.me/{}?start={token}",
        bot_username.trim().trim_start_matches('@')
    )
}

fn normalize_email(value: &str) -> Result<String, AppError> {
    let email = value.trim().to_ascii_lowercase();
    let Some((local, domain)) = email.split_once('@') else {
        return Err(AppError::BadRequest("invalid email".into()));
    };
    if local.is_empty() || domain.is_empty() || !domain.contains('.') || email.len() > 254 {
        return Err(AppError::BadRequest("invalid email".into()));
    }
    Ok(email)
}

fn ensure_user_id(conn: &Connection, email: &str) -> Result<String, AppError> {
    let now = Utc::now().to_rfc3339();
    let user_id = if let Some(id) = conn
        .query_row(
            "SELECT id FROM users WHERE email_normalized = ?1",
            params![email],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| AppError::Internal(format!("query notification user failed: {error}")))?
    {
        id
    } else {
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO users (id, email_normalized, status, created_at, last_login_at)
             VALUES (?1, ?2, 'active', ?3, ?3)",
            params![id, email, now],
        )
        .map_err(|error| {
            AppError::Internal(format!("insert notification user failed: {error}"))
        })?;
        id
    };
    ensure_email_channel(conn, &user_id, email, &now)?;
    Ok(user_id)
}

fn ensure_email_channel(
    conn: &Connection,
    user_id: &str,
    email: &str,
    now: &str,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO user_notification_channels (
            user_id, channel, enabled, state, target, revision, verified_at, created_at, updated_at
         ) VALUES (?1, 'email', 1, 'ready', ?2, 1, ?3, ?3, ?3)
         ON CONFLICT(user_id, channel) DO UPDATE SET
            target = excluded.target,
            state = 'ready',
            verified_at = COALESCE(user_notification_channels.verified_at, excluded.verified_at),
            updated_at = CASE
                WHEN user_notification_channels.target <> excluded.target THEN excluded.updated_at
                ELSE user_notification_channels.updated_at
            END",
        params![user_id, email, now],
    )
    .map_err(|error| AppError::Internal(format!("ensure email channel failed: {error}")))?;
    Ok(())
}

fn read_bot_runtime(conn: &Connection) -> Result<TelegramBotRuntime, AppError> {
    conn.query_row(
        "SELECT readiness, bot_id, username, config_fingerprint, generation,
                last_error, verified_at
         FROM telegram_bot_runtime WHERE id = 1",
        [],
        |row| {
            Ok(TelegramBotRuntime {
                readiness: row.get(0)?,
                bot_id: row.get(1)?,
                username: row.get(2)?,
                config_fingerprint: row.get(3)?,
                generation: row.get(4)?,
                last_error: row.get(5)?,
                verified_at: row.get(6)?,
            })
        },
    )
    .map_err(|error| AppError::Internal(format!("read Telegram bot runtime failed: {error}")))
}

fn read_user_channels(conn: &Connection, user_id: &str) -> Result<Vec<UserChannelRow>, AppError> {
    let mut statement = conn
        .prepare(
            "SELECT channel, enabled, state, target, target_label, provider_identity,
                    revision, verified_at
             FROM user_notification_channels WHERE user_id = ?1 ORDER BY channel",
        )
        .map_err(|error| {
            AppError::Internal(format!("prepare notification channels failed: {error}"))
        })?;
    let rows = statement
        .query_map(params![user_id], |row| {
            Ok(UserChannelRow {
                channel: row.get(0)?,
                enabled: row.get::<_, i64>(1)? != 0,
                state: row.get(2)?,
                target: row.get(3)?,
                target_label: row.get(4)?,
                provider_identity: row.get(5)?,
                revision: row.get(6)?,
                verified_at: row.get(7)?,
            })
        })
        .map_err(|error| {
            AppError::Internal(format!("query notification channels failed: {error}"))
        })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|error| {
        AppError::Internal(format!("read notification channels failed: {error}"))
    })
}

fn settings_response(
    email: &str,
    rows: &[UserChannelRow],
    runtime: &TelegramBotRuntime,
) -> NotificationSettingsResponse {
    let mut channels = Vec::with_capacity(2);
    let email_row = rows.iter().find(|row| row.channel == EMAIL_CHANNEL);
    channels.push(NotificationChannelSettingsResponse {
        channel: EMAIL_CHANNEL.into(),
        enabled: email_row.is_none_or(|row| row.enabled),
        available: true,
        state: "ready".into(),
        target_label: Some(email.to_string()),
        verified_at: email_row.and_then(|row| row.verified_at.clone()),
    });
    let telegram_row = rows.iter().find(|row| row.channel == TELEGRAM_CHANNEL);
    channels.push(NotificationChannelSettingsResponse {
        channel: TELEGRAM_CHANNEL.into(),
        enabled: telegram_row.is_some_and(|row| row.enabled),
        available: runtime.ready(),
        state: telegram_row
            .map(|row| row.state.clone())
            .unwrap_or_else(|| "unbound".into()),
        target_label: telegram_row.and_then(|row| row.target_label.clone()),
        verified_at: telegram_row.and_then(|row| row.verified_at.clone()),
    });
    let enabled_channels = rows
        .iter()
        .filter(|row| row.enabled && row.state == "ready")
        .map(|row| row.channel.clone())
        .collect();
    NotificationSettingsResponse {
        email: email.to_string(),
        enabled_channels,
        channels,
        telegram_bot_status: runtime.readiness.clone(),
        telegram_bot_username: runtime.username.clone(),
    }
}

fn revoke_active_bind_tokens(
    conn: &Connection,
    user_id: Option<&str>,
    now: &str,
) -> Result<(), AppError> {
    if let Some(user_id) = user_id {
        conn.execute(
            "UPDATE telegram_bind_tokens SET revoked_at = ?2
             WHERE user_id = ?1 AND consumed_at IS NULL AND revoked_at IS NULL",
            params![user_id, now],
        )
    } else {
        conn.execute(
            "UPDATE telegram_bind_tokens SET revoked_at = ?1
             WHERE consumed_at IS NULL AND revoked_at IS NULL",
            params![now],
        )
    }
    .map_err(|error| AppError::Internal(format!("revoke Telegram bind tokens failed: {error}")))?;
    Ok(())
}

fn release_delivery_events_for_retry(
    conn: &Connection,
    delivery_id: &str,
    now: &str,
) -> Result<(), AppError> {
    let event_ids = {
        let mut statement = conn
            .prepare("SELECT event_id FROM notification_delivery_items WHERE batch_id = ?1")
            .map_err(|error| {
                AppError::Internal(format!("prepare notification fallback events failed: {error}"))
            })?;
        let rows = statement
            .query_map(params![delivery_id], |row| row.get::<_, String>(0))
            .map_err(|error| {
                AppError::Internal(format!("query notification fallback events failed: {error}"))
            })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|error| {
            AppError::Internal(format!("read notification fallback events failed: {error}"))
        })?
    };
    conn.execute(
        "DELETE FROM notification_delivery_items WHERE batch_id = ?1",
        params![delivery_id],
    )
    .map_err(|error| {
        AppError::Internal(format!("release notification fallback items failed: {error}"))
    })?;
    for event_id in event_ids {
        let delivered_or_active = conn
            .query_row(
                "SELECT 1
                 FROM notification_delivery_items item
                 INNER JOIN notification_deliveries delivery ON delivery.id = item.batch_id
                 WHERE item.event_id = ?1
                   AND delivery.status IN ('pending', 'claimed', 'retry', 'blocked_config', 'sent')
                 LIMIT 1",
                params![event_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("check notification fallback state failed: {error}"))
            })?
            .is_some();
        if !delivered_or_active {
            conn.execute(
                "UPDATE client_notification_events
                 SET status = 'pending', suppression_reason = NULL, updated_at = ?2
                 WHERE id = ?1 AND status = 'batched'",
                params![event_id, now],
            )
            .map_err(|error| {
                AppError::Internal(format!("requeue notification fallback event failed: {error}"))
            })?;
        }
    }
    Ok(())
}

fn cancel_channel_deliveries(
    conn: &Connection,
    email: &str,
    channel: &str,
    now: &str,
    reason: &str,
) -> Result<(), AppError> {
    let delivery_ids = {
        let mut statement = conn
            .prepare(
                "SELECT id FROM notification_deliveries
                 WHERE LOWER(recipient) = LOWER(?1) AND channel = ?2
                   AND (
                       status IN ('pending', 'retry', 'blocked_config')
                       OR (status = 'claimed' AND NOT EXISTS (
                           SELECT 1 FROM notification_delivery_attempts attempt
                           WHERE attempt.delivery_id = notification_deliveries.id
                             AND attempt.status = 'started'
                       ))
                   )",
            )
            .map_err(|error| {
                AppError::Internal(format!("prepare channel delivery cancellation failed: {error}"))
            })?;
        let rows = statement
            .query_map(params![email, channel], |row| row.get::<_, String>(0))
            .map_err(|error| {
                AppError::Internal(format!("query channel delivery cancellation failed: {error}"))
            })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|error| {
            AppError::Internal(format!("read channel delivery cancellation failed: {error}"))
        })?
    };
    for delivery_id in delivery_ids {
        conn.execute(
            "UPDATE notification_deliveries
             SET status = 'cancelled_channel_changed', failure_kind = 'channel_changed',
                 error_message = ?2, blocked_reason_code = NULL, next_attempt_at = NULL,
                 claim_owner = NULL, claim_expires_at = NULL, updated_at = ?3
             WHERE id = ?1",
            params![delivery_id, reason, now],
        )
        .map_err(|error| {
            AppError::Internal(format!("cancel channel notification delivery failed: {error}"))
        })?;
        conn.execute(
            "UPDATE notification_delivery_attempts
             SET status = 'cancelled', finished_at = ?2, error_message = ?3
             WHERE delivery_id = ?1 AND status = 'reserved'",
            params![delivery_id, now, reason],
        )
        .map_err(|error| {
            AppError::Internal(format!("cancel channel delivery reservation failed: {error}"))
        })?;
        release_delivery_events_for_retry(conn, &delivery_id, now)?;
    }
    Ok(())
}

fn enable_email_fallback(
    conn: &Connection,
    user_id: &str,
    email: &str,
    now: &str,
) -> Result<(), AppError> {
    ensure_email_channel(conn, user_id, email, now)?;
    conn.execute(
        "UPDATE user_notification_channels
         SET enabled = 1,
             revision = revision + CASE WHEN enabled = 0 THEN 1 ELSE 0 END,
             updated_at = ?2
         WHERE user_id = ?1 AND channel = 'email'",
        params![user_id, now],
    )
    .map_err(|error| AppError::Internal(format!("enable email fallback failed: {error}")))?;
    Ok(())
}

impl AppStore {
    pub async fn get_notification_settings(
        &self,
        email: &str,
    ) -> Result<NotificationSettingsResponse, AppError> {
        let email = normalize_email(email)?;
        let conn = self.conn.lock().await;
        let user_id = ensure_user_id(&conn, &email)?;
        let rows = read_user_channels(&conn, &user_id)?;
        let runtime = read_bot_runtime(&conn)?;
        Ok(settings_response(&email, &rows, &runtime))
    }

    pub async fn update_notification_settings(
        &self,
        email: &str,
        patch: UpdateNotificationSettingsRequest,
    ) -> Result<NotificationSettingsResponse, AppError> {
        let email = normalize_email(email)?;
        let requested = normalize_enabled_channels(&patch.enabled_channels)?;
        if requested
            .iter()
            .any(|channel| channel != EMAIL_CHANNEL && channel != TELEGRAM_CHANNEL)
        {
            return Err(AppError::BadRequest(
                "unsupported notification channel".into(),
            ));
        }
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin notification settings update failed: {error}"))
            })?;
        let user_id = ensure_user_id(&tx, &email)?;
        let runtime = read_bot_runtime(&tx)?;
        let now = Utc::now().to_rfc3339();
        if requested.contains(TELEGRAM_CHANNEL) {
            let telegram = read_user_channels(&tx, &user_id)?
                .into_iter()
                .find(|row| row.channel == TELEGRAM_CHANNEL);
            if !runtime.ready()
                || telegram.as_ref().is_none_or(|row| {
                    row.state != "ready"
                        || row.target.is_none()
                        || row.provider_identity != runtime.bot_id
                })
            {
                return Err(AppError::BadRequest(
                    "bind a Telegram account to the active bot before enabling this channel".into(),
                ));
            }
        }
        for channel in [EMAIL_CHANNEL, TELEGRAM_CHANNEL] {
            let enabled = requested.contains(channel);
            let current = tx
                .query_row(
                    "SELECT enabled FROM user_notification_channels
                     WHERE user_id = ?1 AND channel = ?2",
                    params![user_id, channel],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(|error| {
                    AppError::Internal(format!("read channel preference failed: {error}"))
                })?
                .unwrap_or(0)
                != 0;
            if channel == TELEGRAM_CHANNEL && current != enabled {
                tx.execute(
                    "UPDATE user_notification_channels
                     SET enabled = ?3, revision = revision + 1, updated_at = ?4
                     WHERE user_id = ?1 AND channel = ?2",
                    params![user_id, channel, i64::from(enabled), now],
                )
                .map_err(|error| {
                    AppError::Internal(format!("update Telegram preference failed: {error}"))
                })?;
            } else if channel == EMAIL_CHANNEL && current != enabled {
                tx.execute(
                    "UPDATE user_notification_channels
                     SET enabled = ?3, revision = revision + 1, updated_at = ?4
                     WHERE user_id = ?1 AND channel = ?2",
                    params![user_id, channel, i64::from(enabled), now],
                )
                .map_err(|error| {
                    AppError::Internal(format!("update email preference failed: {error}"))
                })?;
            }
            if current && !enabled {
                cancel_channel_deliveries(
                    &tx,
                    &email,
                    channel,
                    &now,
                    "notification channel disabled by user",
                )?;
            }
        }
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit notification settings update failed: {error}"))
        })?;
        drop(conn);
        self.get_notification_settings(&email).await
    }

    pub async fn create_telegram_bind_link(
        &self,
        email: &str,
        ttl_secs: i64,
        source_ip: Option<&str>,
    ) -> Result<TelegramBindLinkResponse, AppError> {
        let email = normalize_email(email)?;
        let now = Utc::now();
        let expires_at = now + Duration::seconds(ttl_secs.clamp(60, 86_400));
        let token = generate_bind_token();
        let token_hash = hash_bind_token(&token);
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin Telegram bind-link issue failed: {error}"))
            })?;
        let runtime = read_bot_runtime(&tx)?;
        if !runtime.ready() {
            return Err(AppError::ServiceUnavailable(
                "the Telegram bot is not ready".into(),
            ));
        }
        let bot_id = runtime.bot_id.as_deref().unwrap_or_default();
        let bot_username = runtime.username.as_deref().unwrap_or_default();
        let user_id = ensure_user_id(&tx, &email)?;
        let cutoff = (now - Duration::hours(1)).to_rfc3339();
        let recent: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM telegram_bind_tokens
                 WHERE user_id = ?1 AND created_at >= ?2",
                params![user_id, cutoff],
                |row| row.get(0),
            )
            .map_err(|error| {
                AppError::Internal(format!("count Telegram bind tokens failed: {error}"))
            })?;
        if recent >= MAX_BIND_LINKS_PER_HOUR {
            return Err(AppError::TooManyRequests(
                "too many Telegram bind links requested, try again later".into(),
            ));
        }
        if let Some(source_ip) = source_ip.filter(|value| !value.trim().is_empty()) {
            let recent_from_ip: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM telegram_bind_tokens
                     WHERE created_ip = ?1 AND created_at >= ?2",
                    params![source_ip, cutoff],
                    |row| row.get(0),
                )
                .map_err(|error| {
                    AppError::Internal(format!("count Telegram bind tokens by IP failed: {error}"))
                })?;
            if recent_from_ip >= MAX_BIND_LINKS_PER_IP_PER_HOUR {
                return Err(AppError::TooManyRequests(
                    "too many Telegram bind links requested from this address".into(),
                ));
            }
        }
        let now_text = now.to_rfc3339();
        revoke_active_bind_tokens(&tx, Some(&user_id), &now_text)?;
        tx.execute(
            "INSERT INTO telegram_bind_tokens (
                token_hash, user_id, email_normalized, bot_id, created_at, expires_at, created_ip
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                token_hash,
                user_id,
                email,
                bot_id,
                now_text,
                expires_at.to_rfc3339(),
                source_ip
            ],
        )
        .map_err(|error| {
            AppError::Internal(format!("insert Telegram bind token failed: {error}"))
        })?;
        tx.execute(
            "DELETE FROM telegram_bind_tokens WHERE created_at < ?1",
            params![(now - Duration::days(7)).to_rfc3339()],
        )
        .map_err(|error| {
            AppError::Internal(format!("clean Telegram bind tokens failed: {error}"))
        })?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit Telegram bind-link issue failed: {error}"))
        })?;
        Ok(TelegramBindLinkResponse {
            url: deep_link(bot_username, &token),
            token,
            bot_username: bot_username.to_string(),
            expires_at: expires_at.to_rfc3339(),
        })
    }

    pub async fn consume_telegram_bind_token(
        &self,
        bot_id: &str,
        token: &str,
        chat_id: &str,
        chat_username: Option<&str>,
    ) -> Result<BindOutcome, AppError> {
        let Some(token) = normalize_bind_token(token) else {
            return Ok(BindOutcome::InvalidToken);
        };
        let chat_id = chat_id.trim();
        if chat_id.is_empty() || bot_id.trim().is_empty() {
            return Ok(BindOutcome::InvalidToken);
        }
        let token_hash = hash_bind_token(&token);
        let now = Utc::now();
        let now_text = now.to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin Telegram bind failed: {error}"))
            })?;
        let record = tx
            .query_row(
                "SELECT user_id, email_normalized, bot_id, expires_at, revoked_at,
                        consumed_at, consumed_chat_id
                 FROM telegram_bind_tokens WHERE token_hash = ?1",
                params![token_hash],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("query Telegram bind token failed: {error}"))
            })?;
        let Some((user_id, email, token_bot_id, expires_at, revoked_at, consumed_at, consumed_chat)) =
            record
        else {
            return Ok(BindOutcome::InvalidToken);
        };
        if consumed_at.is_some() {
            return Ok(if consumed_chat.as_deref() == Some(chat_id) {
                BindOutcome::AlreadyBound { email }
            } else {
                BindOutcome::InvalidToken
            });
        }
        let expired = DateTime::parse_from_rfc3339(&expires_at)
            .map(|value| value.with_timezone(&Utc) <= now)
            .unwrap_or(true);
        if revoked_at.is_some() || expired || token_bot_id != bot_id {
            return Ok(BindOutcome::InvalidToken);
        }
        let chat_owner = tx
            .query_row(
                "SELECT user_id FROM user_notification_channels
                 WHERE channel = 'telegram' AND provider_identity = ?1 AND target = ?2
                   AND state = 'ready'",
                params![bot_id, chat_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("query Telegram chat owner failed: {error}"))
            })?;
        if chat_owner.as_deref().is_some_and(|owner| owner != user_id) {
            return Ok(BindOutcome::ChatTakenByAnotherAccount);
        }
        let username = chat_username
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.trim_start_matches('@').to_string());
        tx.execute(
            "INSERT INTO user_notification_channels (
                user_id, channel, enabled, state, target, target_label, provider_identity,
                revision, verified_at, created_at, updated_at
             ) VALUES (?1, 'telegram', 1, 'ready', ?2, ?3, ?4, 1, ?5, ?5, ?5)
             ON CONFLICT(user_id, channel) DO UPDATE SET
                enabled = 1, state = 'ready', target = excluded.target,
                target_label = excluded.target_label,
                provider_identity = excluded.provider_identity,
                revision = user_notification_channels.revision + 1,
                verified_at = excluded.verified_at, invalidated_at = NULL,
                updated_at = excluded.updated_at",
            params![user_id, chat_id, username, bot_id, now_text],
        )
        .map_err(|error| AppError::Internal(format!("bind Telegram channel failed: {error}")))?;
        tx.execute(
            "UPDATE telegram_bind_tokens
             SET consumed_at = ?1, consumed_chat_id = ?2
             WHERE token_hash = ?3",
            params![now_text, chat_id, token_hash],
        )
        .map_err(|error| AppError::Internal(format!("consume Telegram bind token failed: {error}")))?;
        revoke_active_bind_tokens(&tx, Some(&user_id), &now_text)?;
        let enabled_channels = read_user_channels(&tx, &user_id)?
            .into_iter()
            .filter(|row| row.enabled && row.state == "ready")
            .map(|row| row.channel)
            .collect();
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit Telegram bind failed: {error}"))
        })?;
        Ok(BindOutcome::Bound {
            email,
            enabled_channels,
        })
    }

    pub async fn unbind_telegram(
        &self,
        email: &str,
    ) -> Result<NotificationSettingsResponse, AppError> {
        let email = normalize_email(email)?;
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| AppError::Internal(format!("begin Telegram unbind failed: {error}")))?;
        let user_id = ensure_user_id(&tx, &email)?;
        let now = Utc::now().to_rfc3339();
        enable_email_fallback(&tx, &user_id, &email, &now)?;
        tx.execute(
            "UPDATE user_notification_channels
             SET enabled = 0, state = 'unbound', target = NULL, target_label = NULL,
                 provider_identity = NULL, revision = revision + 1,
                 invalidated_at = ?2, updated_at = ?2
             WHERE user_id = ?1 AND channel = 'telegram'",
            params![user_id, now],
        )
        .map_err(|error| AppError::Internal(format!("unbind Telegram channel failed: {error}")))?;
        revoke_active_bind_tokens(&tx, Some(&user_id), &now)?;
        cancel_channel_deliveries(
            &tx,
            &email,
            TELEGRAM_CHANNEL,
            &now,
            "Telegram channel unbound by user",
        )?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit Telegram unbind failed: {error}"))
        })?;
        drop(conn);
        self.get_notification_settings(&email).await
    }

    pub async fn unbind_telegram_chat(&self, chat_id: &str) -> Result<Option<String>, AppError> {
        let chat_id = chat_id.trim();
        if chat_id.is_empty() {
            return Ok(None);
        }
        let conn = self.conn.lock().await;
        let email = conn
            .query_row(
                "SELECT users.email_normalized
                 FROM user_notification_channels channel
                 INNER JOIN users ON users.id = channel.user_id
                 WHERE channel.channel = 'telegram' AND channel.target = ?1
                   AND channel.state = 'ready'",
                params![chat_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("query Telegram binding failed: {error}"))
            })?;
        drop(conn);
        if let Some(email) = email {
            self.unbind_telegram(&email).await?;
            Ok(Some(email))
        } else {
            Ok(None)
        }
    }

    pub async fn handle_unreachable_telegram_delivery(
        &self,
        delivery_id: &str,
        worker_id: &str,
        chat_id: &str,
        error_message: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<String>, AppError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin Telegram fallback failed: {error}"))
            })?;
        let delivery = tx
            .query_row(
                "SELECT recipient_user_id, provider_identity, channel_target
                 FROM notification_deliveries
                 WHERE id = ?1 AND channel = 'telegram' AND status = 'claimed'
                   AND claim_owner = ?2",
                params![delivery_id, worker_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("query unreachable Telegram delivery failed: {error}"))
            })?
            .ok_or_else(|| {
                AppError::Conflict(
                    "Telegram delivery claim is no longer owned by this worker".into(),
                )
            })?;
        if delivery.2.as_deref() != Some(chat_id) {
            return Err(AppError::Conflict(
                "Telegram delivery target changed before failure handling".into(),
            ));
        }
        let binding = match (delivery.0.as_deref(), delivery.1.as_deref()) {
            (Some(user_id), Some(provider_identity)) => tx
                .query_row(
                    "SELECT channel.user_id, users.email_normalized
                     FROM user_notification_channels channel
                     INNER JOIN users ON users.id = channel.user_id
                     WHERE channel.user_id = ?1 AND channel.channel = 'telegram'
                       AND channel.provider_identity = ?2 AND channel.target = ?3
                       AND channel.state = 'ready'",
                    params![user_id, provider_identity, chat_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(|error| {
                    AppError::Internal(format!(
                        "query unreachable Telegram binding failed: {error}"
                    ))
                })?,
            _ => None,
        };
        let now_text = now.to_rfc3339();
        let changed = tx.execute(
            "UPDATE notification_deliveries
             SET status = 'dead_letter', failure_kind = 'endpoint_unreachable',
                 error_message = ?3, blocked_reason_code = NULL,
                 next_attempt_at = NULL, claim_owner = NULL, claim_expires_at = NULL,
                 updated_at = ?4
             WHERE id = ?1 AND status = 'claimed' AND claim_owner = ?2",
            params![delivery_id, worker_id, error_message, now_text],
        )
        .map_err(|error| {
            AppError::Internal(format!("finish unreachable Telegram delivery failed: {error}"))
        })?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "Telegram delivery claim is no longer owned by this worker".into(),
            ));
        }
        tx.execute(
            "UPDATE notification_delivery_attempts
             SET status = 'failed', finished_at = ?2, error_message = ?3
             WHERE delivery_id = ?1 AND status = 'started'",
            params![delivery_id, now_text, error_message],
        )
        .map_err(|error| {
            AppError::Internal(format!("finish unreachable Telegram attempt failed: {error}"))
        })?;
        release_delivery_events_for_retry(&tx, delivery_id, &now_text)?;
        if let Some((user_id, email)) = binding.as_ref() {
            enable_email_fallback(&tx, user_id, email, &now_text)?;
            tx.execute(
                "UPDATE user_notification_channels
                 SET enabled = 0, state = 'invalid', target = NULL, target_label = NULL,
                     provider_identity = NULL, revision = revision + 1,
                     invalidated_at = ?2, updated_at = ?2
                 WHERE user_id = ?1 AND channel = 'telegram'",
                params![user_id, now_text],
            )
            .map_err(|error| {
                AppError::Internal(format!("invalidate Telegram binding failed: {error}"))
            })?;
            revoke_active_bind_tokens(&tx, Some(user_id), &now_text)?;
            cancel_channel_deliveries(
                &tx,
                email,
                TELEGRAM_CHANNEL,
                &now_text,
                "Telegram endpoint became unreachable",
            )?;
        }
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit Telegram fallback failed: {error}"))
        })?;
        Ok(binding.map(|(_, email)| email))
    }

    pub async fn notification_targets(
        &self,
        email: &str,
    ) -> Result<Option<NotificationTargets>, AppError> {
        let email = normalize_email(email)?;
        let conn = self.conn.lock().await;
        notification_targets_tx(&conn, &email)
    }

    pub async fn telegram_binding_for_chat(
        &self,
        chat_id: &str,
    ) -> Result<Option<NotificationTargets>, AppError> {
        let chat_id = chat_id.trim();
        if chat_id.is_empty() {
            return Ok(None);
        }
        let conn = self.conn.lock().await;
        let email = conn
            .query_row(
                "SELECT users.email_normalized
                 FROM user_notification_channels channel
                 INNER JOIN users ON users.id = channel.user_id
                 WHERE channel.channel = 'telegram' AND channel.target = ?1
                   AND channel.state = 'ready'",
                params![chat_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("query Telegram binding failed: {error}"))
            })?;
        email
            .as_deref()
            .map(|email| notification_targets_tx(&conn, email))
            .transpose()
            .map(Option::flatten)
    }

    pub async fn telegram_bot_runtime(&self) -> Result<TelegramBotRuntime, AppError> {
        let conn = self.conn.lock().await;
        read_bot_runtime(&conn)
    }

    pub async fn mark_telegram_bot_reconciling(
        &self,
        config_fingerprint: &str,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin Telegram reconcile state failed: {error}"))
            })?;
        let previous_fingerprint = tx
            .query_row(
                "SELECT config_fingerprint FROM telegram_bot_runtime WHERE id = 1",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(|error| {
                AppError::Internal(format!("read Telegram config fingerprint failed: {error}"))
            })?;
        let now = Utc::now().to_rfc3339();
        if previous_fingerprint.as_deref() != Some(config_fingerprint) {
            revoke_active_bind_tokens(&tx, None, &now)?;
        }
        tx.execute(
            "UPDATE telegram_bot_runtime
             SET readiness = 'reconciling', config_fingerprint = ?1,
                 last_error = NULL, updated_at = ?2 WHERE id = 1",
            params![config_fingerprint, now],
        )
        .map_err(|error| {
            AppError::Internal(format!("mark Telegram bot reconciling failed: {error}"))
        })?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit Telegram reconcile state failed: {error}"))
        })?;
        Ok(())
    }

    pub async fn mark_telegram_bot_disabled(&self) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin Telegram disabled state failed: {error}"))
            })?;
        let now = Utc::now().to_rfc3339();
        revoke_active_bind_tokens(&tx, None, &now)?;
        let bound_emails = {
            let mut statement = tx
                .prepare(
                    "SELECT DISTINCT users.email_normalized
                     FROM user_notification_channels channel
                     INNER JOIN users ON users.id = channel.user_id
                     WHERE channel.channel = 'telegram' AND channel.enabled = 1
                       AND channel.state = 'ready'",
                )
                .map_err(|error| {
                    AppError::Internal(format!(
                        "prepare Telegram disabled fallback failed: {error}"
                    ))
                })?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|error| {
                    AppError::Internal(format!(
                        "query Telegram disabled fallback failed: {error}"
                    ))
                })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(|error| {
                AppError::Internal(format!("read Telegram disabled fallback failed: {error}"))
            })?
        };
        for email in bound_emails {
            cancel_channel_deliveries(
                &tx,
                &email,
                TELEGRAM_CHANNEL,
                &now,
                "Telegram notification bot disabled",
            )?;
        }
        tx.execute(
            "UPDATE telegram_bot_runtime
             SET readiness = 'disabled', last_error = NULL, updated_at = ?1 WHERE id = 1",
            params![now],
        )
        .map_err(|error| {
            AppError::Internal(format!("mark Telegram bot disabled failed: {error}"))
        })?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit Telegram disabled state failed: {error}"))
        })?;
        Ok(())
    }

    pub async fn mark_telegram_bot_error(
        &self,
        config_fingerprint: &str,
        error: &str,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE telegram_bot_runtime
             SET readiness = 'error', config_fingerprint = ?1, last_error = ?2,
                 updated_at = ?3 WHERE id = 1",
            params![config_fingerprint, error, Utc::now().to_rfc3339()],
        )
        .map_err(|database_error| {
            AppError::Internal(format!("mark Telegram bot error failed: {database_error}"))
        })?;
        Ok(())
    }

    pub async fn apply_telegram_bot_identity(
        &self,
        bot_id: &str,
        username: &str,
        config_fingerprint: &str,
    ) -> Result<TelegramIdentityApplied, AppError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin Telegram identity apply failed: {error}"))
            })?;
        let previous = read_bot_runtime(&tx)?;
        let identity_changed = previous
            .bot_id
            .as_deref()
            .is_some_and(|previous_id| previous_id != bot_id);
        let config_changed = previous.config_fingerprint.as_deref() != Some(config_fingerprint);
        let now = Utc::now().to_rfc3339();
        if config_changed {
            revoke_active_bind_tokens(&tx, None, &now)?;
        }
        let affected = if identity_changed {
            let bindings = {
                let mut statement = tx
                    .prepare(
                        "SELECT channel.user_id, users.email_normalized
                         FROM user_notification_channels channel
                         INNER JOIN users ON users.id = channel.user_id
                         WHERE channel.channel = 'telegram' AND channel.state = 'ready'",
                    )
                    .map_err(|error| {
                        AppError::Internal(format!("prepare old Telegram bindings failed: {error}"))
                    })?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|error| {
                        AppError::Internal(format!("query old Telegram bindings failed: {error}"))
                    })?;
                rows.collect::<Result<Vec<_>, _>>().map_err(|error| {
                    AppError::Internal(format!("read old Telegram bindings failed: {error}"))
                })?
            };
            for (user_id, email) in &bindings {
                enable_email_fallback(&tx, user_id, email, &now)?;
                cancel_channel_deliveries(
                    &tx,
                    email,
                    TELEGRAM_CHANNEL,
                    &now,
                    "Telegram bot identity changed",
                )?;
            }
            tx.execute(
                "UPDATE user_notification_channels
                 SET enabled = 0, state = 'invalid', target = NULL, target_label = NULL,
                     provider_identity = NULL, revision = revision + 1,
                     invalidated_at = ?1, updated_at = ?1
                 WHERE channel = 'telegram' AND state = 'ready'",
                params![now],
            )
            .map_err(|error| {
                AppError::Internal(format!("invalidate old Telegram bindings failed: {error}"))
            })?;
            if let Some(previous_bot_id) = previous.bot_id.as_deref() {
                tx.execute(
                    "UPDATE telegram_inbound_updates
                     SET status = 'dead_letter', last_error = 'Telegram bot identity changed',
                         claim_owner = NULL, claim_expires_at = NULL,
                         completed_at = ?2, updated_at = ?2
                     WHERE bot_id = ?1 AND status IN ('pending', 'claimed', 'retry')",
                    params![previous_bot_id, now],
                )
                .map_err(|error| {
                    AppError::Internal(format!("retire old Telegram inbox failed: {error}"))
                })?;
            }
            bindings.len()
        } else {
            0
        };
        let generation = previous.generation
            + i64::from(identity_changed || config_changed || previous.readiness != "ready");
        tx.execute(
            "UPDATE telegram_bot_runtime
             SET readiness = 'ready', bot_id = ?1, username = ?2,
                 config_fingerprint = ?3, generation = ?4, last_error = NULL,
                 verified_at = ?5, updated_at = ?5 WHERE id = 1",
            params![bot_id, username, config_fingerprint, generation, now],
        )
        .map_err(|error| {
            AppError::Internal(format!("persist Telegram bot identity failed: {error}"))
        })?;
        tx.execute(
            "UPDATE notification_deliveries
             SET status = 'retry', next_attempt_at = ?2, error_message = NULL,
                 failure_kind = NULL, blocked_reason_code = NULL, updated_at = ?2
             WHERE channel = 'telegram' AND status = 'blocked_config'
               AND blocked_reason_code = 'telegram_bot_unavailable'
               AND provider_identity = ?1",
            params![bot_id, now],
        )
        .map_err(|error| {
            AppError::Internal(format!("resume Telegram notifications failed: {error}"))
        })?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit Telegram identity apply failed: {error}"))
        })?;
        Ok(TelegramIdentityApplied {
            identity_changed,
            invalidated_bindings: affected,
            generation,
        })
    }
}

pub fn notification_targets_tx(
    conn: &Connection,
    email: &str,
) -> Result<Option<NotificationTargets>, AppError> {
    let normalized = email.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Ok(None);
    }
    let user_id = conn
        .query_row(
            "SELECT id FROM users WHERE email_normalized = ?1",
            params![normalized],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| {
            AppError::Internal(format!("query notification target user failed: {error}"))
        })?;
    let Some(user_id) = user_id else {
        return Ok(None);
    };
    let rows = read_user_channels(conn, &user_id)?;
    let targets = rows
        .into_iter()
        .filter(|row| row.enabled && row.state == "ready")
        .filter_map(|row| {
            let address = row.target?;
            let channel = NotificationChannelId::parse(&row.channel).ok()?;
            Some(NotificationTarget {
                channel,
                address,
                revision: row.revision,
                provider_identity: row.provider_identity,
            })
        })
        .collect::<Vec<_>>();
    Ok(Some(if targets.is_empty() {
        NotificationTargets::email_only(normalized)
    } else {
        NotificationTargets {
            email: normalized,
            targets,
        }
    }))
}

pub fn telegram_config_fingerprint(token: &str, mode: &str, webhook_secret: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.trim().as_bytes());
    hasher.update([0]);
    hasher.update(mode.trim().as_bytes());
    hasher.update([0]);
    hasher.update(webhook_secret.unwrap_or_default().trim().as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn generated_tokens_fit_the_deep_link_charset() {
        let token = generate_bind_token();
        assert_eq!(token.len(), BIND_TOKEN_BYTES * 2);
        assert!(token.chars().all(|character| character.is_ascii_hexdigit()));
        assert!(token.len() <= 64);
        assert_ne!(token, generate_bind_token());
    }

    #[test]
    fn malformed_bind_payloads_are_rejected_before_storage() {
        assert!(normalize_bind_token("").is_none());
        assert!(normalize_bind_token("not-hex").is_none());
        let token = generate_bind_token();
        assert_eq!(normalize_bind_token(&format!(" {token}\n")), Some(token));
    }

    #[test]
    fn config_fingerprint_does_not_expose_the_token() {
        let fingerprint = telegram_config_fingerprint("secret-token", "polling", None);
        assert_eq!(fingerprint.len(), 64);
        assert!(!fingerprint.contains("secret-token"));
    }

    #[test]
    fn enabled_channel_input_is_a_set() {
        let channels = normalize_enabled_channels(&[
            "telegram".into(),
            "email".into(),
            "telegram".into(),
        ])
        .expect("channels");
        assert_eq!(
            channels,
            BTreeSet::from([EMAIL_CHANNEL.into(), TELEGRAM_CHANNEL.into()])
        );
    }
}
