use std::time::Duration;

use chrono::Utc;
use reqwest::header::RETRY_AFTER;
use serde_json::Value;

use crate::config::AlertingSettings;

pub const TELEGRAM_CHANNEL: &str = "telegram";
pub const REGISTERED_CHANNELS: &[&str] = &[TELEGRAM_CHANNEL];

#[derive(Debug, Clone)]
pub struct ChannelSendSuccess {
    pub provider_message_id: Option<String>,
    pub http_status: u16,
}

#[derive(Debug, Clone)]
pub struct ChannelSendFailure {
    pub retryable: bool,
    pub retry_at: Option<i64>,
    pub http_status: Option<u16>,
    pub message: String,
}

pub async fn send(
    http: &reqwest::Client,
    settings: &AlertingSettings,
    channel: &str,
    text: &str,
) -> Result<ChannelSendSuccess, ChannelSendFailure> {
    match channel {
        TELEGRAM_CHANNEL => send_telegram(http, settings, text).await,
        other => Err(ChannelSendFailure {
            retryable: false,
            retry_at: None,
            http_status: None,
            message: format!("unsupported alert channel: {other}"),
        }),
    }
}

pub fn is_registered(channel: &str) -> bool {
    REGISTERED_CHANNELS.contains(&channel)
}

async fn send_telegram(
    http: &reqwest::Client,
    settings: &AlertingSettings,
    text: &str,
) -> Result<ChannelSendSuccess, ChannelSendFailure> {
    let token = required(
        settings.telegram_bot_token.as_deref(),
        "Telegram bot token is not configured",
    )?;
    let chat_id = required(
        settings.telegram_chat_id.as_deref(),
        "Telegram chat id is not configured",
    )?;
    let endpoint = format!("https://api.telegram.org/bot{token}/sendMessage");
    let mut payload = serde_json::json!({
        "chat_id": chat_id,
        "text": text,
        "disable_web_page_preview": true,
    });
    if let Some(topic_id) = settings.telegram_topic_id {
        payload["message_thread_id"] = serde_json::json!(topic_id);
    }
    let response = http
        .post(&endpoint)
        .json(&payload)
        .send()
        .await
        .map_err(|error| request_failure("Telegram", error, Some(token)))?;
    let status = response.status();
    let retry_at = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after);
    let body = response.text().await.map_err(|error| ChannelSendFailure {
        retryable: true,
        retry_at: None,
        http_status: Some(status.as_u16()),
        message: sanitize(
            &format!(
                "read Telegram response failed after HTTP {}: {error}",
                status.as_u16()
            ),
            Some(token),
        ),
    })?;
    let value = serde_json::from_str::<Value>(&body).unwrap_or_else(|_| serde_json::json!({}));
    if status.is_success() && value.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(ChannelSendSuccess {
            provider_message_id: value.pointer("/result/message_id").and_then(
                |value| match value {
                    Value::Number(number) => Some(number.to_string()),
                    Value::String(value) => Some(value.clone()),
                    _ => None,
                },
            ),
            http_status: status.as_u16(),
        });
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
    Err(ChannelSendFailure {
        retryable: is_retryable_status(status.as_u16()),
        retry_at: retry_at.or(telegram_retry_at),
        http_status: Some(status.as_u16()),
        message: sanitize(
            &format!(
                "Telegram returned HTTP {}: {}",
                status.as_u16(),
                truncate(description, 1_000)
            ),
            Some(token),
        ),
    })
}

fn required<'a>(value: Option<&'a str>, message: &str) -> Result<&'a str, ChannelSendFailure> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ChannelSendFailure {
            retryable: false,
            retry_at: None,
            http_status: None,
            message: message.into(),
        })
}

fn request_failure(
    channel: &str,
    error: reqwest::Error,
    secret: Option<&str>,
) -> ChannelSendFailure {
    ChannelSendFailure {
        retryable: error.is_timeout() || error.is_connect() || error.is_request(),
        retry_at: None,
        http_status: error.status().map(|status| status.as_u16()),
        message: sanitize(&format!("{channel} request failed: {error}"), secret),
    }
}

fn is_retryable_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500..=599)
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

fn sanitize(value: &str, secret: Option<&str>) -> String {
    let mut sanitized = value.replace(['\r', '\n'], " ");
    if let Some(secret) = secret.filter(|value| !value.is_empty()) {
        sanitized = sanitized.replace(secret, "[REDACTED]");
    }
    truncate(&sanitized, 1_200).to_string()
}

fn truncate(value: &str, max_chars: usize) -> &str {
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

pub fn build_http_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent("cc-switch-router/0.1 operator-alerts")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn telegram_request_error_redacts_bot_token() {
        let message = sanitize(
            "request to https://api.telegram.org/botsecret-token/sendMessage failed",
            Some("secret-token"),
        );
        assert!(!message.contains("secret-token"));
        assert!(message.contains("[REDACTED]"));
    }

    #[test]
    fn standard_retryable_statuses_are_classified() {
        assert!(is_retryable_status(408));
        assert!(is_retryable_status(425));
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(503));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(409));
    }

    #[test]
    fn channel_registry_rejects_unknown_ids() {
        assert!(is_registered(TELEGRAM_CHANNEL));
        assert!(!is_registered("unregistered"));
    }

    #[test]
    fn channel_registry_ids_are_unique_and_storage_safe() {
        let mut seen = HashSet::new();
        for channel in REGISTERED_CHANNELS {
            assert!((1..=64).contains(&channel.len()));
            assert!(channel.bytes().all(|byte| byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || byte == b'_'
                || byte == b'-'));
            assert!(seen.insert(channel));
        }
    }

    #[tokio::test]
    async fn every_registered_channel_has_a_dispatcher() {
        let http = build_http_client().expect("build test HTTP client");
        let settings = AlertingSettings::default();
        for channel in REGISTERED_CHANNELS {
            let failure = send(&http, &settings, channel, "test")
                .await
                .expect_err("default channel settings should be incomplete");
            assert!(!failure.message.starts_with("unsupported alert channel:"));
        }
    }
}
