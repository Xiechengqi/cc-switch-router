//! Operator-alert channel dispatch.
//!
//! Transport lives in [`crate::telegram`] and [`crate::bark`]; this module maps
//! alerting settings onto those providers and keeps the alert-specific
//! `ChannelSend*` shapes stable for `crate::alerting`'s outbox.

use crate::config::AlertingSettings;
use crate::telegram::{self, TelegramFailure};

pub const TELEGRAM_CHANNEL: &str = "telegram";
pub const BARK_CHANNEL: &str = "bark";
pub const REGISTERED_CHANNELS: &[&str] = &[TELEGRAM_CHANNEL, BARK_CHANNEL];

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
    pub failure_code: String,
    pub failure_hint: String,
    pub failure_details: Option<serde_json::Value>,
}

impl From<TelegramFailure> for ChannelSendFailure {
    fn from(failure: TelegramFailure) -> Self {
        Self {
            retryable: failure.retryable,
            retry_at: failure.retry_at,
            http_status: failure.http_status,
            message: failure.message,
            failure_code: failure.code.as_str().into(),
            failure_hint: failure.hint,
            failure_details: failure
                .diagnostics
                .and_then(|value| serde_json::to_value(value).ok()),
        }
    }
}

impl From<crate::bark::SendFailure> for ChannelSendFailure {
    fn from(failure: crate::bark::SendFailure) -> Self {
        Self {
            retryable: failure.retryable,
            retry_at: failure.retry_at,
            http_status: failure.http_status,
            message: failure.message,
            failure_code: failure.code,
            failure_hint: failure.hint,
            failure_details: None,
        }
    }
}

pub async fn send(
    telegram_http: &reqwest::Client,
    bark_http: &reqwest::Client,
    settings: &AlertingSettings,
    channel: &str,
    text: &str,
    message_id: &str,
    dashboard_url: &str,
) -> Result<ChannelSendSuccess, ChannelSendFailure> {
    match channel {
        TELEGRAM_CHANNEL => send_telegram(telegram_http, settings, text).await,
        BARK_CHANNEL => send_bark(bark_http, settings, text, message_id, dashboard_url).await,
        other => Err(ChannelSendFailure {
            retryable: false,
            retry_at: None,
            http_status: None,
            message: format!("unsupported alert channel: {other}"),
            failure_code: "configuration".into(),
            failure_hint: "The selected alert channel is not supported.".into(),
            failure_details: None,
        }),
    }
}

async fn send_bark(
    http: &reqwest::Client,
    settings: &AlertingSettings,
    text: &str,
    message_id: &str,
    dashboard_url: &str,
) -> Result<ChannelSendSuccess, ChannelSendFailure> {
    let device_key = required(
        settings.bark_device_key.as_deref(),
        "Bark device key is not configured",
    )?;
    crate::bark::validate_device_key(device_key).map_err(|error| ChannelSendFailure {
        retryable: false,
        retry_at: None,
        http_status: None,
        message: error.to_string(),
        failure_code: "configuration".into(),
        failure_hint: "Configure a valid Bark device key before testing.".into(),
        failure_details: None,
    })?;
    let body = truncate_utf8(text, 3_000);
    let request = crate::bark::PushRequest {
        device_key,
        title: "CC-Switch Router alert",
        body: &body,
        group: "CC-Switch Router",
        url: dashboard_url,
        id: message_id,
        level: "active",
    };
    let success = crate::bark::send(http, &settings.bark_server_url, &request).await?;
    Ok(ChannelSendSuccess {
        provider_message_id: success.provider_message_id,
        http_status: success.http_status,
    })
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
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
    // Operator alert bodies are composed from admin-configured templates, so
    // they go out verbatim: interpreting them as markup would let a stray
    // angle bracket in a hostname fail the whole alert.
    let success = telegram::send_message(
        http,
        token,
        chat_id,
        settings.telegram_topic_id,
        text,
        telegram::TelegramParseMode::Plain,
    )
    .await?;
    Ok(ChannelSendSuccess {
        provider_message_id: success.provider_message_id,
        http_status: success.http_status,
    })
}

#[allow(clippy::result_large_err)]
fn required<'a>(value: Option<&'a str>, message: &str) -> Result<&'a str, ChannelSendFailure> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ChannelSendFailure {
            retryable: false,
            retry_at: None,
            http_status: None,
            message: message.into(),
            failure_code: "configuration".into(),
            failure_hint: "Complete the alert channel configuration before testing.".into(),
            failure_details: None,
        })
}

pub fn build_http_client() -> Result<reqwest::Client, reqwest::Error> {
    telegram::build_send_http_client("cc-switch-router/0.1 operator-alerts")
}

pub fn build_bark_http_client() -> Result<reqwest::Client, reqwest::Error> {
    crate::bark::build_http_client("cc-switch-router/0.1 operator-bark-alerts")
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn channel_registry_rejects_unknown_ids() {
        assert!(is_registered(TELEGRAM_CHANNEL));
        assert!(is_registered(BARK_CHANNEL));
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
        let bark_http = build_bark_http_client().expect("build Bark test HTTP client");
        let settings = AlertingSettings::default();
        for channel in REGISTERED_CHANNELS {
            let failure = send(
                &http,
                &bark_http,
                &settings,
                channel,
                "test",
                "test-message",
                "https://router.example.com/settings/",
            )
            .await
            .expect_err("default channel settings should be incomplete");
            assert!(!failure.message.starts_with("unsupported alert channel:"));
        }
    }

    #[test]
    fn bark_body_truncation_preserves_utf8() {
        let body = "警".repeat(1_001);
        let truncated = truncate_utf8(&body, 3_000);
        assert_eq!(truncated.len(), 3_000);
        assert_eq!(truncated.chars().count(), 1_000);
    }
}
