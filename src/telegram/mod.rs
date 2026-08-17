//! Low-level Telegram Bot API client shared by two independent consumers:
//!
//! * `crate::alerting` — send-only operator alerts pinned to one chat.
//! * `crate::telegram::bind` / `crate::notifications` — the user-facing
//!   notification bot, which additionally *receives* `/start <token>` deep
//!   links to bind a Router account to a private chat.
//!
//! Everything here is transport: no policy, no persistence. Errors are shaped
//! as [`TelegramFailure`] so both consumers can reuse the same retry
//! classification (`retryable` + optional `retry_at` honoring Telegram's own
//! `parameters.retry_after`).

pub mod bind;
pub mod service;

use std::error::Error as StdError;
use std::time::Duration;

use chrono::Utc;
use reqwest::header::RETRY_AFTER;
use serde_json::Value;

/// Telegram rejects `sendMessage` payloads above 4096 UTF-16 code units. We cut
/// a little earlier so an appended ellipsis still fits.
pub const MAX_MESSAGE_CHARS: usize = 4_000;

/// Telegram's `start` deep-link parameter accepts 1..=64 chars from
/// `A-Za-z0-9_-`. Our tokens are 16 random bytes rendered as 32 hex chars.
pub const BIND_TOKEN_BYTES: usize = 16;

#[derive(Debug, Clone)]
pub struct TelegramSuccess {
    pub provider_message_id: Option<String>,
    pub http_status: u16,
}

#[derive(Debug, Clone)]
pub struct TelegramFailure {
    pub retryable: bool,
    pub retry_at: Option<i64>,
    pub http_status: Option<u16>,
    pub message: String,
    /// Telegram told us this chat can never be delivered to again: the user
    /// blocked the bot, deleted the chat, or the id is bogus. Callers unbind
    /// instead of retrying.
    pub chat_unreachable: bool,
}

impl TelegramFailure {
    pub fn config(message: impl Into<String>) -> Self {
        Self {
            retryable: false,
            retry_at: None,
            http_status: None,
            message: message.into(),
            chat_unreachable: false,
        }
    }
}

/// Result of `getMe`, used to prove a bot token works and to learn the
/// `@username` needed for `https://t.me/<username>?start=<token>` deep links.
#[derive(Debug, Clone)]
pub struct BotIdentity {
    pub id: i64,
    pub username: String,
    pub can_read_all_group_messages: bool,
}

pub fn build_update_http_client(user_agent: &str) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent(user_agent.to_string())
        .connect_timeout(Duration::from_secs(10))
        // Long polling holds the response open for `timeout` seconds; the read
        // timeout has to outlive it or every poll aborts mid-flight.
        .timeout(Duration::from_secs(60))
        .build()
}

pub fn build_send_http_client(user_agent: &str) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent(user_agent.to_string())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .build()
}

fn endpoint(token: &str, method: &str) -> String {
    format!("https://api.telegram.org/bot{token}/{method}")
}

/// POST a Bot API method and unwrap the `{ok, result}` envelope.
async fn call(
    http: &reqwest::Client,
    token: &str,
    method: &str,
    payload: &Value,
) -> Result<(Value, u16), TelegramFailure> {
    let response = http
        .post(endpoint(token, method))
        .json(payload)
        .send()
        .await
        .map_err(|error| request_failure(method, error, Some(token)))?;
    let status = response.status();
    let header_retry_at = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after);
    let body = response.text().await.map_err(|error| TelegramFailure {
        retryable: true,
        retry_at: header_retry_at,
        http_status: Some(status.as_u16()),
        message: sanitize(
            &format!(
                "read Telegram {method} response failed after HTTP {}: {error}",
                status.as_u16()
            ),
            Some(token),
        ),
        chat_unreachable: false,
    })?;
    let value = serde_json::from_str::<Value>(&body).unwrap_or_else(|_| serde_json::json!({}));
    if status.is_success() && value.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok((
            value.get("result").cloned().unwrap_or(Value::Null),
            status.as_u16(),
        ));
    }
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or(&body);
    let telegram_retry_at = value
        .pointer("/parameters/retry_after")
        .and_then(Value::as_i64)
        .map(|seconds| {
            Utc::now()
                .timestamp()
                .saturating_add(seconds.clamp(1, 86_400))
        });
    Err(TelegramFailure {
        retryable: is_retryable_status(status.as_u16()),
        retry_at: telegram_retry_at.or(header_retry_at),
        http_status: Some(status.as_u16()),
        message: sanitize(
            &format!(
                "Telegram {method} returned HTTP {}: {}",
                status.as_u16(),
                truncate(description, 1_000)
            ),
            Some(token),
        ),
        chat_unreachable: is_chat_unreachable(status.as_u16(), description),
    })
}

pub async fn send_message(
    http: &reqwest::Client,
    token: &str,
    chat_id: &str,
    topic_id: Option<i64>,
    text: &str,
) -> Result<TelegramSuccess, TelegramFailure> {
    let token = require(Some(token), "Telegram bot token is not configured")?;
    let chat_id = require(Some(chat_id), "Telegram chat id is not configured")?;
    let mut payload = serde_json::json!({
        "chat_id": chat_id,
        "text": truncate(text, MAX_MESSAGE_CHARS),
        "disable_web_page_preview": true,
    });
    if let Some(topic_id) = topic_id {
        payload["message_thread_id"] = serde_json::json!(topic_id);
    }
    let (result, http_status) = call(http, token, "sendMessage", &payload).await?;
    Ok(TelegramSuccess {
        provider_message_id: result.get("message_id").and_then(|value| match value {
            Value::Number(number) => Some(number.to_string()),
            Value::String(value) => Some(value.clone()),
            _ => None,
        }),
        http_status,
    })
}

pub async fn get_me(http: &reqwest::Client, token: &str) -> Result<BotIdentity, TelegramFailure> {
    let token = require(Some(token), "Telegram bot token is not configured")?;
    let (result, _) = call(http, token, "getMe", &serde_json::json!({})).await?;
    let username = result
        .get("username")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .trim_start_matches('@')
        .to_string();
    if username.is_empty() {
        return Err(TelegramFailure::config(
            "Telegram getMe returned a bot without a username",
        ));
    }
    Ok(BotIdentity {
        id: result.get("id").and_then(Value::as_i64).unwrap_or_default(),
        username,
        can_read_all_group_messages: result
            .get("can_read_all_group_messages")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// Register the commands users see in the Telegram UI's `/` menu.
pub async fn set_my_commands(
    http: &reqwest::Client,
    token: &str,
    commands: &[(&str, &str)],
) -> Result<(), TelegramFailure> {
    let token = require(Some(token), "Telegram bot token is not configured")?;
    let payload = serde_json::json!({
        "commands": commands
            .iter()
            .map(|(command, description)| serde_json::json!({
                "command": command,
                "description": description,
            }))
            .collect::<Vec<_>>(),
        "scope": { "type": "all_private_chats" },
    });
    call(http, token, "setMyCommands", &payload).await?;
    Ok(())
}

pub async fn set_webhook(
    http: &reqwest::Client,
    token: &str,
    url: &str,
    secret: &str,
) -> Result<(), TelegramFailure> {
    let token = require(Some(token), "Telegram bot token is not configured")?;
    let payload = serde_json::json!({
        "url": url,
        "secret_token": secret,
        "allowed_updates": ["message"],
        "drop_pending_updates": false,
        "max_connections": 20,
    });
    call(http, token, "setWebhook", &payload).await?;
    Ok(())
}

pub async fn delete_webhook(http: &reqwest::Client, token: &str) -> Result<(), TelegramFailure> {
    let token = require(Some(token), "Telegram bot token is not configured")?;
    call(
        http,
        token,
        "deleteWebhook",
        &serde_json::json!({ "drop_pending_updates": false }),
    )
    .await?;
    Ok(())
}

/// Long-poll for updates. Returns raw update objects; the caller advances
/// `offset` past the highest `update_id` it has processed.
pub async fn get_updates(
    http: &reqwest::Client,
    token: &str,
    offset: i64,
    timeout_secs: u16,
) -> Result<Vec<Value>, TelegramFailure> {
    let token = require(Some(token), "Telegram bot token is not configured")?;
    let payload = serde_json::json!({
        "offset": offset,
        "timeout": timeout_secs,
        "limit": 50,
        "allowed_updates": ["message"],
    });
    let (result, _) = call(http, token, "getUpdates", &payload).await?;
    Ok(match result {
        Value::Array(updates) => updates,
        _ => Vec::new(),
    })
}

fn require<'a>(value: Option<&'a str>, message: &str) -> Result<&'a str, TelegramFailure> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| TelegramFailure::config(message))
}

fn request_failure(method: &str, error: reqwest::Error, secret: Option<&str>) -> TelegramFailure {
    let kind = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_request() {
        "request"
    } else {
        "transport"
    };
    TelegramFailure {
        retryable: error.is_timeout() || error.is_connect() || error.is_request(),
        retry_at: None,
        http_status: error.status().map(|status| status.as_u16()),
        message: sanitize(
            &format!(
                "Telegram {method} request failed ({kind}): {}",
                error_chain(&error)
            ),
            secret,
        ),
        chat_unreachable: false,
    }
}

fn error_chain(error: &(dyn StdError + 'static)) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = error.source();
    while let Some(cause) = source {
        let message = cause.to_string();
        if parts.last().is_none_or(|previous| previous != &message) {
            parts.push(message);
        }
        if parts.len() >= 6 {
            break;
        }
        source = cause.source();
    }
    parts.join("; cause: ")
}

pub fn is_retryable_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500..=599)
}

/// Distinguish "this chat is permanently gone" from other 4xx. Telegram has no
/// machine-readable code for it, so the description is the only signal.
fn is_chat_unreachable(status: u16, description: &str) -> bool {
    if !matches!(status, 400 | 403) {
        return false;
    }
    let lowered = description.to_ascii_lowercase();
    lowered.contains("bot was blocked by the user")
        || lowered.contains("user is deactivated")
        || lowered.contains("chat not found")
        || lowered.contains("bot was kicked")
        || lowered.contains("peer_id_invalid")
        || lowered.contains("have no rights to send a message")
}

fn parse_retry_after(value: &str) -> Option<i64> {
    let now = Utc::now();
    if let Ok(seconds) = value.trim().parse::<i64>() {
        return Some(now.timestamp().saturating_add(seconds.clamp(1, 86_400)));
    }
    chrono::DateTime::parse_from_rfc2822(value.trim())
        .ok()
        .map(|value| value.with_timezone(&Utc))
        .filter(|value| *value > now)
        .map(|value| value.min(now + chrono::Duration::hours(24)).timestamp())
}

/// Collapse newlines and scrub the bot token before anything reaches a log or
/// a database row.
pub fn sanitize(value: &str, secret: Option<&str>) -> String {
    let mut sanitized = value.replace(['\r', '\n'], " ");
    if let Some(secret) = secret.filter(|value| !value.is_empty()) {
        sanitized = sanitized.replace(secret, "[REDACTED]");
    }
    truncate(&sanitized, 1_200).to_string()
}

pub fn truncate(value: &str, max_chars: usize) -> &str {
    if value.chars().count() <= max_chars {
        return value;
    }
    let end = value
        .char_indices()
        .nth(max_chars)
        .map(|(index, _)| index)
        .unwrap_or(value.len());
    &value[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;

    #[test]
    fn bot_token_is_redacted_from_messages() {
        let message = sanitize(
            "request to https://api.telegram.org/botsecret-token/sendMessage failed",
            Some("secret-token"),
        );
        assert!(!message.contains("secret-token"));
        assert!(message.contains("[REDACTED]"));
    }

    #[test]
    fn error_chain_preserves_nested_transport_cause() {
        #[derive(Debug)]
        struct NestedError;

        impl fmt::Display for NestedError {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("dns lookup failed")
            }
        }

        impl StdError for NestedError {}

        #[derive(Debug)]
        struct RequestError {
            source: NestedError,
        }

        impl fmt::Display for RequestError {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("request failed")
            }
        }

        impl StdError for RequestError {
            fn source(&self) -> Option<&(dyn StdError + 'static)> {
                Some(&self.source)
            }
        }

        assert_eq!(
            error_chain(&RequestError {
                source: NestedError,
            }),
            "request failed; cause: dns lookup failed"
        );
    }

    #[test]
    fn retryable_statuses_match_telegram_guidance() {
        assert!(is_retryable_status(408));
        assert!(is_retryable_status(425));
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(503));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(403));
        assert!(!is_retryable_status(409));
    }

    #[test]
    fn blocked_and_missing_chats_are_permanent() {
        assert!(is_chat_unreachable(
            403,
            "Forbidden: bot was blocked by the user"
        ));
        assert!(is_chat_unreachable(400, "Bad Request: chat not found"));
        assert!(is_chat_unreachable(403, "Forbidden: user is deactivated"));
        // A generic bad request must not silently unbind the user.
        assert!(!is_chat_unreachable(
            400,
            "Bad Request: message text is empty"
        ));
        assert!(!is_chat_unreachable(429, "Too Many Requests"));
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        let value = "中文中文中文";
        assert_eq!(truncate(value, 3), "中文中");
        assert_eq!(truncate(value, 99), value);
    }

    #[test]
    fn mode_round_trips_through_env_text() {
        use crate::config::TelegramBotMode;
        assert_eq!(
            TelegramBotMode::parse("polling"),
            Some(TelegramBotMode::Polling)
        );
        assert_eq!(
            TelegramBotMode::parse("  WEBHOOK "),
            Some(TelegramBotMode::Webhook)
        );
        assert_eq!(TelegramBotMode::parse("carrier-pigeon"), None);
        assert_eq!(TelegramBotMode::Polling.as_str(), "polling");
    }
}
