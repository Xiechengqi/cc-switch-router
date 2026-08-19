//! Operator-alert channel dispatch.
//!
//! Transport lives in [`crate::telegram`]; this module only maps the alerting
//! settings onto it and keeps the alert-specific `ChannelSend*` shapes stable
//! for `crate::alerting`'s outbox.

use crate::config::AlertingSettings;
use crate::telegram::{self, TelegramFailure};

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
            failure_code: "configuration".into(),
            failure_hint: "The selected alert channel is not supported.".into(),
            failure_details: None,
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

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
