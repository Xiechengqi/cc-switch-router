//! Durable inbound processing for the user-facing Telegram bot.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

use crate::config::{Config, TelegramBotMode};
use crate::db::{OptionalExtension, TransactionBehavior, params};
use crate::dynamic_settings::DynamicSettings;
use crate::error::AppError;
use crate::store::AppStore;
use crate::telegram::bind::{BindOutcome, telegram_config_fingerprint};
use crate::telegram::{self, TelegramFailure};

pub const WEBHOOK_PATH: &str = "/v1/integrations/telegram/webhook";
pub const WEBHOOK_SECRET_HEADER: &str = "x-telegram-bot-api-secret-token";

const POLL_TIMEOUT_SECS: u16 = 25;
const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(5);
const ERROR_BACKOFF: Duration = Duration::from_secs(10);
const FATAL_BACKOFF: Duration = Duration::from_secs(60);
const INBOUND_CLAIM_LEASE_SECS: i64 = 60;
const INBOUND_MAX_ATTEMPTS: i64 = 8;
const INBOUND_BATCH_SIZE: usize = 50;
const MAX_FAILED_BINDS_PER_HOUR: i64 = 10;

const COMMANDS: &[(&str, &str)] = &[
    ("start", "Bind this chat to your Router account"),
    ("status", "Show the account bound to this chat"),
    ("unbind", "Stop receiving Router notifications here"),
    ("help", "How to use this bot"),
];

#[derive(PartialEq, Eq)]
struct DesiredConfig {
    token: String,
    mode: TelegramBotMode,
    webhook_secret: Option<String>,
    fingerprint: String,
}

struct AppliedConfig {
    desired: DesiredConfig,
    bot_id: String,
}

#[derive(Debug)]
struct ClaimedInboundUpdate {
    update_id: i64,
    payload: Value,
    attempts: i64,
}

#[derive(Default)]
struct FailedBindTracker {
    chats: HashMap<String, (i64, i64)>,
}

impl FailedBindTracker {
    fn is_throttled(&mut self, chat_id: &str) -> bool {
        let now = Utc::now().timestamp();
        self.chats
            .retain(|_, (started_at, _)| now.saturating_sub(*started_at) < 3_600);
        let entry = self.chats.entry(chat_id.to_string()).or_insert((now, 0));
        if now.saturating_sub(entry.0) >= 3_600 {
            *entry = (now, 0);
        }
        entry.1 >= MAX_FAILED_BINDS_PER_HOUR
    }

    fn record_failure(&mut self, chat_id: &str) {
        let now = Utc::now().timestamp();
        let entry = self.chats.entry(chat_id.to_string()).or_insert((now, 0));
        entry.1 = entry.1.saturating_add(1);
    }

    fn clear(&mut self, chat_id: &str) {
        self.chats.remove(chat_id);
    }
}

fn failed_binds() -> &'static Mutex<FailedBindTracker> {
    static TRACKER: std::sync::OnceLock<Mutex<FailedBindTracker>> = std::sync::OnceLock::new();
    TRACKER.get_or_init(|| Mutex::new(FailedBindTracker::default()))
}

pub fn public_base_url(config: &Config) -> String {
    let scheme = if config.use_localhost { "http" } else { "https" };
    format!("{scheme}://{}", config.tunnel_domain.trim_end_matches('/'))
}

fn account_settings_url(config: &Config) -> String {
    format!("{}/account/notifications", public_base_url(config))
}

fn webhook_url(config: &Config) -> String {
    format!("{}{WEBHOOK_PATH}", public_base_url(config))
}

pub async fn run_telegram_bot_service(
    store: AppStore,
    dynamic: Arc<RwLock<DynamicSettings>>,
    config: Config,
) -> anyhow::Result<()> {
    let update_http = telegram::build_update_http_client("cc-switch-router/0.1 telegram-updates")?;
    let send_http = telegram::build_send_http_client("cc-switch-router/0.1 telegram-replies")?;
    let worker_id = format!("telegram-inbound-{}", Uuid::new_v4());
    let mut applied: Option<AppliedConfig> = None;

    loop {
        let settings = dynamic.read().await.telegram_bot.clone();
        let Some(token) = settings
            .token()
            .map(str::to_string)
            .filter(|_| settings.enabled)
        else {
            if applied.take().is_some() {
                tracing::info!("telegram bot disabled; update listener idle");
            }
            if let Err(error) = store.mark_telegram_bot_disabled().await {
                tracing::warn!(error = %error, "persist Telegram disabled state failed");
            }
            tokio::time::sleep(IDLE_POLL_INTERVAL).await;
            continue;
        };
        let mode = settings.mode;
        let webhook_secret = settings.webhook_secret.clone();
        let fingerprint = telegram_config_fingerprint(
            &token,
            mode.as_str(),
            webhook_secret.as_deref(),
        );
        let desired = DesiredConfig {
            token,
            mode,
            webhook_secret,
            fingerprint,
        };
        let needs_reconcile = applied
            .as_ref()
            .is_none_or(|current| current.desired != desired);
        if needs_reconcile {
            if let Err(error) = store
                .mark_telegram_bot_reconciling(&desired.fingerprint)
                .await
            {
                tracing::warn!(error = %error, "persist Telegram reconcile state failed");
                tokio::time::sleep(ERROR_BACKOFF).await;
                continue;
            }
            match reconcile(&store, &update_http, &dynamic, &config, desired).await {
                Ok(next) => applied = Some(next),
                Err(failure) => {
                    let fingerprint = settings
                        .token()
                        .map(|token| {
                            telegram_config_fingerprint(
                                token,
                                settings.mode.as_str(),
                                settings.webhook_secret.as_deref(),
                            )
                        })
                        .unwrap_or_default();
                    let _ = store
                        .mark_telegram_bot_error(&fingerprint, &failure.message)
                        .await;
                    tracing::warn!(error = %failure.message, "telegram bot setup failed; retrying");
                    tokio::time::sleep(if failure.retryable {
                        ERROR_BACKOFF
                    } else {
                        FATAL_BACKOFF
                    })
                    .await;
                    continue;
                }
            }
        }
        let Some(current) = applied.as_ref() else {
            continue;
        };

        if let Err(error) = process_inbound_updates(
            &store,
            &send_http,
            &current.desired.token,
            &current.bot_id,
            &config,
            &worker_id,
        )
        .await
        {
            tracing::warn!(error = %error, "Telegram durable inbox processing failed");
        }

        if current.desired.mode == TelegramBotMode::Webhook {
            tokio::time::sleep(IDLE_POLL_INTERVAL).await;
            continue;
        }

        let offset = match store.telegram_poll_offset(&current.bot_id).await {
            Ok(offset) => offset,
            Err(error) => {
                tracing::warn!(error = %error, "read Telegram poll cursor failed");
                tokio::time::sleep(ERROR_BACKOFF).await;
                continue;
            }
        };
        match telegram::get_updates(
            &update_http,
            &current.desired.token,
            offset,
            POLL_TIMEOUT_SECS,
        )
        .await
        {
            Ok(updates) => {
                if let Err(error) = store
                    .persist_telegram_updates(&current.bot_id, &updates)
                    .await
                {
                    tracing::warn!(error = %error, "persist Telegram updates failed; cursor retained");
                    tokio::time::sleep(ERROR_BACKOFF).await;
                }
            }
            Err(failure) if failure.http_status == Some(409) => {
                tracing::error!(
                    "telegram getUpdates conflict: another process or webhook consumes this bot"
                );
                tokio::time::sleep(FATAL_BACKOFF).await;
            }
            Err(failure) => {
                tracing::warn!(error = %failure.message, "telegram getUpdates failed");
                tokio::time::sleep(ERROR_BACKOFF).await;
            }
        }
    }
}

async fn reconcile(
    store: &AppStore,
    http: &reqwest::Client,
    dynamic: &Arc<RwLock<DynamicSettings>>,
    config: &Config,
    desired: DesiredConfig,
) -> Result<AppliedConfig, TelegramFailure> {
    let identity = telegram::get_me(http, &desired.token).await?;
    telegram::set_my_commands(http, &desired.token, COMMANDS).await?;
    match desired.mode {
        TelegramBotMode::Webhook => {
            let secret = desired.webhook_secret.as_deref().unwrap_or_default();
            if secret.trim().is_empty() {
                return Err(TelegramFailure::config(
                    "webhook mode requires CC_SWITCH_ROUTER_TELEGRAM_WEBHOOK_SECRET",
                ));
            }
            telegram::set_webhook(http, &desired.token, &webhook_url(config), secret).await?;
        }
        TelegramBotMode::Polling => {
            telegram::delete_webhook(http, &desired.token).await?;
        }
    }
    let bot_id = identity.id.to_string();
    store
        .apply_telegram_bot_identity(&bot_id, &identity.username, &desired.fingerprint)
        .await
        .map_err(|error| TelegramFailure::config(error.to_string()))?;
    let _ = dynamic;
    tracing::info!(
        bot_id = %bot_id,
        bot = %identity.username,
        mode = desired.mode.as_str(),
        "telegram bot identity confirmed"
    );
    if identity.can_read_all_group_messages {
        tracing::warn!(bot = %identity.username, "Telegram bot group privacy mode is disabled");
    }
    Ok(AppliedConfig {
        desired,
        bot_id,
    })
}

pub async fn handle_webhook_update(
    store: &AppStore,
    dynamic: &Arc<RwLock<DynamicSettings>>,
    _config: &Config,
    secret_header: Option<&str>,
    update: Value,
) -> Result<(), AppError> {
    let settings = dynamic.read().await.telegram_bot.clone();
    if !settings.is_operational() || settings.mode != TelegramBotMode::Webhook {
        return Err(AppError::NotFound("not found".into()));
    }
    let expected = settings.webhook_secret.as_deref().unwrap_or_default();
    let provided = secret_header.unwrap_or_default();
    if expected.trim().is_empty() || !constant_time_eq(expected.trim(), provided.trim()) {
        return Err(AppError::Unauthorized("invalid webhook secret".into()));
    }
    let runtime = store.telegram_bot_runtime().await?;
    if !runtime.ready() {
        return Err(AppError::ServiceUnavailable(
            "Telegram bot is reconciling".into(),
        ));
    }
    let bot_id = runtime.bot_id.as_deref().unwrap_or_default();
    store.persist_telegram_updates(bot_id, &[update]).await?;
    Ok(())
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0u8, |accumulator, (left, right)| accumulator | (left ^ right))
        == 0
}

impl AppStore {
    pub async fn telegram_poll_offset(&self, bot_id: &str) -> Result<i64, AppError> {
        let conn = self.conn.lock().await;
        conn.query_row(
            "SELECT next_offset FROM telegram_poll_cursors WHERE bot_id = ?1",
            params![bot_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map(|offset| offset.unwrap_or(0))
        .map_err(|error| AppError::Internal(format!("read Telegram poll cursor failed: {error}")))
    }

    pub async fn persist_telegram_updates(
        &self,
        bot_id: &str,
        updates: &[Value],
    ) -> Result<i64, AppError> {
        if bot_id.trim().is_empty() {
            return Err(AppError::BadRequest("Telegram bot id is required".into()));
        }
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin Telegram update persistence failed: {error}"))
            })?;
        let current = tx
            .query_row(
                "SELECT next_offset FROM telegram_poll_cursors WHERE bot_id = ?1",
                params![bot_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("read Telegram update cursor failed: {error}"))
            })?
            .unwrap_or(0);
        let now = Utc::now().to_rfc3339();
        let mut next_offset = current;
        for update in updates {
            let update_id = update
                .get("update_id")
                .and_then(Value::as_i64)
                .ok_or_else(|| AppError::BadRequest("Telegram update_id is required".into()))?;
            tx.execute(
                "INSERT OR IGNORE INTO telegram_inbound_updates (
                    bot_id, update_id, payload_json, status, received_at, updated_at
                 ) VALUES (?1, ?2, ?3, 'pending', ?4, ?4)",
                params![bot_id, update_id, update.to_string(), now],
            )
            .map_err(|error| {
                AppError::Internal(format!("persist Telegram update failed: {error}"))
            })?;
            next_offset = next_offset.max(update_id.saturating_add(1));
        }
        tx.execute(
            "INSERT INTO telegram_poll_cursors (bot_id, next_offset, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(bot_id) DO UPDATE SET
                next_offset = MAX(next_offset, excluded.next_offset),
                updated_at = excluded.updated_at",
            params![bot_id, next_offset, now],
        )
        .map_err(|error| {
            AppError::Internal(format!("persist Telegram poll cursor failed: {error}"))
        })?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit Telegram updates failed: {error}"))
        })?;
        Ok(next_offset)
    }

    async fn claim_telegram_inbound_update(
        &self,
        bot_id: &str,
        worker_id: &str,
    ) -> Result<Option<ClaimedInboundUpdate>, AppError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin Telegram inbox claim failed: {error}"))
            })?;
        let now = Utc::now();
        let now_text = now.to_rfc3339();
        let candidate = tx
            .query_row(
                "SELECT update_id FROM telegram_inbound_updates
                 WHERE bot_id = ?1 AND (
                     (status IN ('pending', 'retry')
                      AND (next_attempt_at IS NULL OR next_attempt_at <= ?2))
                     OR (status = 'claimed' AND claim_expires_at <= ?2)
                 )
                 ORDER BY update_id LIMIT 1",
                params![bot_id, now_text],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("query Telegram inbox claim failed: {error}"))
            })?;
        let Some(update_id) = candidate else {
            tx.commit().map_err(|error| {
                AppError::Internal(format!("commit empty Telegram inbox claim failed: {error}"))
            })?;
            return Ok(None);
        };
        let lease = (now + ChronoDuration::seconds(INBOUND_CLAIM_LEASE_SECS)).to_rfc3339();
        let changed = tx
            .execute(
                "UPDATE telegram_inbound_updates
                 SET status = 'claimed', attempts = attempts + 1, claim_owner = ?3,
                     claim_expires_at = ?4, updated_at = ?2
                 WHERE bot_id = ?1 AND update_id = ?5 AND (
                     status IN ('pending', 'retry')
                     OR (status = 'claimed' AND claim_expires_at <= ?2)
                 )",
                params![bot_id, now_text, worker_id, lease, update_id],
            )
            .map_err(|error| {
                AppError::Internal(format!("claim Telegram inbox update failed: {error}"))
            })?;
        if changed != 1 {
            tx.commit().map_err(|error| {
                AppError::Internal(format!("commit lost Telegram inbox claim failed: {error}"))
            })?;
            return Ok(None);
        }
        let claimed = tx
            .query_row(
                "SELECT payload_json, attempts FROM telegram_inbound_updates
                 WHERE bot_id = ?1 AND update_id = ?2",
                params![bot_id, update_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(|error| {
                AppError::Internal(format!("read claimed Telegram update failed: {error}"))
            })?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit Telegram inbox claim failed: {error}"))
        })?;
        let payload = serde_json::from_str(&claimed.0).map_err(|error| {
            AppError::Internal(format!("decode Telegram inbox payload failed: {error}"))
        })?;
        Ok(Some(ClaimedInboundUpdate {
            update_id,
            payload,
            attempts: claimed.1,
        }))
    }

    async fn finish_telegram_inbound_update(
        &self,
        bot_id: &str,
        update_id: i64,
        worker_id: &str,
        status: &str,
        error: Option<&str>,
        next_attempt_at: Option<String>,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let now = Utc::now().to_rfc3339();
        let changed = conn
            .execute(
                "UPDATE telegram_inbound_updates
                 SET status = ?4, last_error = ?5, next_attempt_at = ?6,
                     claim_owner = NULL, claim_expires_at = NULL, updated_at = ?7,
                     completed_at = CASE WHEN ?4 IN ('completed', 'dead_letter') THEN ?7 ELSE NULL END
                 WHERE bot_id = ?1 AND update_id = ?2 AND status = 'claimed' AND claim_owner = ?3",
                params![bot_id, update_id, worker_id, status, error, next_attempt_at, now],
            )
            .map_err(|database_error| {
                AppError::Internal(format!("finish Telegram inbox update failed: {database_error}"))
            })?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "Telegram inbox claim is no longer owned by this worker".into(),
            ));
        }
        Ok(())
    }
}

async fn process_inbound_updates(
    store: &AppStore,
    http: &reqwest::Client,
    token: &str,
    bot_id: &str,
    config: &Config,
    worker_id: &str,
) -> Result<(), AppError> {
    for _ in 0..INBOUND_BATCH_SIZE {
        let Some(update) = store
            .claim_telegram_inbound_update(bot_id, worker_id)
            .await?
        else {
            break;
        };
        match handle_update(store, http, token, bot_id, config, &update.payload).await {
            Ok(()) => {
                store
                    .finish_telegram_inbound_update(
                        bot_id,
                        update.update_id,
                        worker_id,
                        "completed",
                        None,
                        None,
                    )
                    .await?;
            }
            Err(error) if update.attempts < INBOUND_MAX_ATTEMPTS => {
                let delay = 5_i64.saturating_mul(1_i64 << update.attempts.min(6));
                store
                    .finish_telegram_inbound_update(
                        bot_id,
                        update.update_id,
                        worker_id,
                        "retry",
                        Some(&error.to_string()),
                        Some((Utc::now() + ChronoDuration::seconds(delay)).to_rfc3339()),
                    )
                    .await?;
            }
            Err(error) => {
                store
                    .finish_telegram_inbound_update(
                        bot_id,
                        update.update_id,
                        worker_id,
                        "dead_letter",
                        Some(&error.to_string()),
                        None,
                    )
                    .await?;
                tracing::error!(update_id = update.update_id, error = %error, "Telegram update moved to dead letter");
            }
        }
    }
    Ok(())
}

struct IncomingMessage {
    chat_id: String,
    chat_type: String,
    text: String,
    username: Option<String>,
}

fn parse_message(update: &Value) -> Option<IncomingMessage> {
    let message = update.get("message")?;
    let chat = message.get("chat")?;
    let chat_id = match chat.get("id")? {
        Value::Number(number) => number.to_string(),
        Value::String(value) => value.clone(),
        _ => return None,
    };
    Some(IncomingMessage {
        chat_id,
        chat_type: chat
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        text: message
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string(),
        username: message
            .pointer("/from/username")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.trim_start_matches('@').to_string()),
    })
}

fn parse_command(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    let rest = text.strip_prefix('/')?;
    let (command, argument) = match rest.split_once(char::is_whitespace) {
        Some((command, argument)) => (command, argument.trim()),
        None => (rest, ""),
    };
    let command = command
        .split('@')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    (!command.is_empty()).then(|| (command, argument.to_string()))
}

async fn handle_update(
    store: &AppStore,
    http: &reqwest::Client,
    token: &str,
    bot_id: &str,
    config: &Config,
    update: &Value,
) -> Result<(), AppError> {
    let Some(message) = parse_message(update) else {
        return Ok(());
    };
    if message.chat_type != "private" {
        return Ok(());
    }
    let reply = match parse_command(&message.text) {
        Some((command, argument)) => match command.as_str() {
            "start" if !argument.is_empty() => {
                handle_bind(store, bot_id, &message, &argument, config).await?
            }
            "start" | "help" => help_text(config),
            "status" => handle_status(store, &message).await?,
            "unbind" | "stop" => handle_unbind(store, &message).await?,
            _ => help_text(config),
        },
        None if !message.text.is_empty() => {
            handle_bind(store, bot_id, &message, &message.text, config).await?
        }
        _ => return Ok(()),
    };
    match telegram::send_message(http, token, &message.chat_id, None, &reply).await {
        Ok(_) => Ok(()),
        Err(failure) if failure.chat_unreachable => {
            let _ = store.unbind_telegram_chat(&message.chat_id).await;
            Ok(())
        }
        Err(failure) => Err(AppError::Internal(failure.message)),
    }
}

async fn handle_bind(
    store: &AppStore,
    bot_id: &str,
    message: &IncomingMessage,
    payload: &str,
    config: &Config,
) -> Result<String, AppError> {
    if failed_binds().lock().await.is_throttled(&message.chat_id) {
        return Ok("尝试次数过多，请稍后再试。\nToo many attempts. Please try again later.".into());
    }
    let outcome = store
        .consume_telegram_bind_token(
            bot_id,
            payload,
            &message.chat_id,
            message.username.as_deref(),
        )
        .await?;
    Ok(match outcome {
        BindOutcome::Bound {
            email,
            enabled_channels,
        } => {
            failed_binds().lock().await.clear(&message.chat_id);
            format!(
                "绑定成功：{email}\n通知渠道：{}\nManage notifications: {}",
                enabled_channels.join(" + "),
                account_settings_url(config)
            )
        }
        BindOutcome::AlreadyBound { email } => format!(
            "此对话已绑定到 {email}。\nThis chat is already bound to {email}."
        ),
        BindOutcome::ChatTakenByAnotherAccount => {
            "此 Telegram 对话已绑定到另一个 Router 账号。\nThis chat belongs to another Router account.".into()
        }
        BindOutcome::InvalidToken => {
            failed_binds().lock().await.record_failure(&message.chat_id);
            format!(
                "绑定链接无效或已过期，请重新生成：{}\nThe binding link is invalid or expired.",
                account_settings_url(config)
            )
        }
    })
}

async fn handle_status(store: &AppStore, message: &IncomingMessage) -> Result<String, AppError> {
    let Some(targets) = store.telegram_binding_for_chat(&message.chat_id).await? else {
        return Ok("此对话尚未绑定任何 Router 账号。\nThis chat is not bound.".into());
    };
    Ok(format!(
        "已绑定账号：{}\n通知渠道：{}",
        targets.email,
        targets
            .enabled_channels()
            .into_iter()
            .collect::<Vec<_>>()
            .join(" + ")
    ))
}

async fn handle_unbind(store: &AppStore, message: &IncomingMessage) -> Result<String, AppError> {
    Ok(match store.unbind_telegram_chat(&message.chat_id).await? {
        Some(email) => format!(
            "已解绑 {email}，未送达通知将回到邮件渠道。\nUnbound; pending notifications fall back to email."
        ),
        None => "此对话尚未绑定任何 Router 账号。\nThis chat is not bound.".into(),
    })
}

fn help_text(config: &Config) -> String {
    format!(
        "CC-Switch Router 通知机器人\n\n在账户页绑定 Telegram：{}\n\n/status 查看状态\n/unbind 解除绑定",
        account_settings_url(config)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_survive_the_at_botname_suffix() {
        assert_eq!(
            parse_command("/start@my_router_bot abc123"),
            Some(("start".into(), "abc123".into()))
        );
        assert_eq!(parse_command("/STATUS"), Some(("status".into(), "".into())));
        assert_eq!(parse_command("hello"), None);
    }

    #[test]
    fn private_messages_preserve_numeric_chat_ids() {
        let update = serde_json::json!({
            "update_id": 1,
            "message": {
                "chat": {"id": 4242, "type": "private"},
                "from": {"username": "@someone"},
                "text": " /start abc "
            }
        });
        let message = parse_message(&update).expect("message");
        assert_eq!(message.chat_id, "4242");
        assert_eq!(message.username.as_deref(), Some("someone"));
    }

    #[test]
    fn webhook_secret_comparison_rejects_length_and_content_mismatch() {
        assert!(constant_time_eq("same-secret", "same-secret"));
        assert!(!constant_time_eq("same-secret", "other"));
        assert!(!constant_time_eq("same-secret", "same-secreu"));
    }
}
