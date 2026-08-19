//! Runtime health and administrator-initiated delivery tests for user-facing
//! notification channels. Operator alert channels have a separate lifecycle in
//! `crate::alerting` and must not share health records with this module.

use axum::http::StatusCode;
use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::config::{TelegramBotMode, TelegramBotSettings};
use crate::db::{OptionalExtension, params};
use crate::error::AppError;
use crate::notification_channels::TELEGRAM_CHANNEL;
use crate::store::AppStore;
use crate::telegram::bind::{TelegramBotRuntime, telegram_config_fingerprint};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserNotificationChannelState {
    pub channel: String,
    pub enabled: bool,
    pub configured: bool,
    pub status: String,
    pub runtime_ready: bool,
    pub transport_status: String,
    pub provider_label: Option<String>,
    pub runtime_verified_at: Option<String>,
    pub last_attempt_at: Option<String>,
    pub last_success_at: Option<String>,
    pub last_error: Option<String>,
    pub failure_code: Option<String>,
    pub failure_hint: Option<String>,
    pub failure_details: Option<Value>,
    pub last_failure_at: Option<String>,
    pub test_target_available: bool,
    pub test_target_label: Option<String>,
    pub binding_verified_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserNotificationChannelTestResponse {
    pub ok: bool,
    pub channel: String,
    pub target_label: Option<String>,
    pub provider_message_id: Option<String>,
    pub tested_at: String,
}

#[derive(Debug, Clone, Default)]
struct ChannelCheckActivity {
    last_attempt_at: Option<String>,
    last_success_at: Option<String>,
    last_error: Option<String>,
    failure_code: Option<String>,
    failure_hint: Option<String>,
    failure_details: Option<Value>,
}

#[derive(Debug, Clone)]
struct TelegramTestBinding {
    chat_id: String,
    target_label: Option<String>,
    provider_identity: Option<String>,
    verified_at: Option<String>,
}

pub async fn channel_states(
    store: &AppStore,
    settings: &TelegramBotSettings,
    actor_email: &str,
) -> Result<Vec<UserNotificationChannelState>, AppError> {
    let runtime = store.telegram_bot_runtime().await?;
    let activity = if let Some(fingerprint) = telegram_config_fingerprint_for(settings) {
        store
            .user_notification_channel_check_activity(TELEGRAM_CHANNEL, &fingerprint)
            .await?
    } else {
        ChannelCheckActivity::default()
    };
    let binding = store.telegram_test_binding(actor_email).await?;
    Ok(vec![telegram_channel_state(
        settings, &runtime, activity, binding,
    )])
}

pub async fn test_channel(
    store: &AppStore,
    settings: &TelegramBotSettings,
    actor_email: &str,
    channel: &str,
    dashboard_url: &str,
) -> Result<UserNotificationChannelTestResponse, AppError> {
    if channel != TELEGRAM_CHANNEL {
        return Err(AppError::NotFound(
            "user notification channel not found".into(),
        ));
    }
    if !settings.enabled {
        return Err(AppError::coded_conflict(
            "USER_NOTIFICATION_CHANNEL_DISABLED",
            "Telegram user notifications are disabled",
            serde_json::json!({ "channel": channel }),
        ));
    }
    if !telegram_configured(settings) {
        return Err(AppError::coded_conflict(
            "USER_NOTIFICATION_CHANNEL_MISCONFIGURED",
            "Telegram user notification Bot configuration is incomplete",
            serde_json::json!({ "channel": channel }),
        ));
    }

    let token = settings.token().unwrap_or_default();
    let fingerprint = telegram_config_fingerprint(
        token,
        settings.mode.as_str(),
        settings.webhook_secret.as_deref(),
    );
    let runtime = store.telegram_bot_runtime().await?;
    if !runtime.ready() || runtime.config_fingerprint.as_deref() != Some(fingerprint.as_str()) {
        let mut details = serde_json::json!({
            "channel": channel,
            "status": runtime.readiness,
            "transportStatus": runtime.transport_status,
        });
        if let Some(code) = runtime.last_failure_code.as_deref() {
            details["failureCode"] = Value::String(code.into());
        }
        if let Some(hint) = runtime.last_failure_hint.as_deref() {
            details["failureHint"] = Value::String(hint.into());
        }
        if let Some(error) = runtime.last_error.as_deref() {
            details["technicalError"] = Value::String(error.into());
        }
        if let Some(network) = runtime.last_failure_details.clone() {
            details["diagnostics"] = network;
        }
        return Err(AppError::coded_conflict(
            "USER_NOTIFICATION_BOT_NOT_READY",
            runtime
                .last_failure_hint
                .clone()
                .unwrap_or_else(|| "Telegram user notification Bot is not ready".into()),
            details,
        ));
    }

    let binding = store
        .telegram_test_binding(actor_email)
        .await?
        .ok_or_else(|| {
            AppError::coded_conflict(
                "USER_NOTIFICATION_TELEGRAM_BINDING_REQUIRED",
                "bind the current administrator account to Telegram before sending a test",
                serde_json::json!({ "channel": channel }),
            )
        })?;
    if binding.provider_identity.as_deref() != runtime.bot_id.as_deref() {
        return Err(AppError::coded_conflict(
            "USER_NOTIFICATION_TELEGRAM_REBIND_REQUIRED",
            "the current administrator Telegram binding belongs to a different Bot",
            serde_json::json!({ "channel": channel }),
        ));
    }

    let tested_at = Utc::now();
    // The test message is styled like a real notification on purpose: an
    // administrator checking the channel should see exactly the formatting a
    // production alert will arrive with, not a plain probe that proves less.
    let settings_url = format!(
        "{}/account/notifications",
        dashboard_url.trim_end_matches('/')
    );
    let message = crate::notifications::TelegramMessage::html(format!(
        "{info} <b>通知渠道测试 / CHANNEL TEST</b>\n\n<i>This is a test message from the CC-Switch Router notification bot.</i>\n\n<b>Account</b>  <code>{account}</code>\n<b>Bot</b>  <code>@{bot}</code>\n<b>Time</b>  <code>{time}</code>\n\n⚙️ <a href=\"{settings}\">管理通知渠道 / Manage notifications</a>",
        info = crate::notifications::NotificationSeverity::Info.badge(),
        account = crate::telegram::escape_html(&actor_email.trim().to_ascii_lowercase()),
        bot = crate::telegram::escape_html(runtime.username.as_deref().unwrap_or_default()),
        time = crate::telegram::escape_html(&tested_at.to_rfc3339()),
        settings = crate::telegram::escape_html(&settings_url),
    ));
    let http = crate::telegram::build_send_http_client(
        "cc-switch-router/0.1 user-notification-channel-test",
    )
    .map_err(|error| {
        AppError::Internal(format!(
            "build user notification test HTTP client failed: {error}"
        ))
    })?;
    match crate::telegram::send_message(
        &http,
        token,
        &binding.chat_id,
        None,
        &message.text,
        message.parse_mode,
    )
    .await
    {
        Ok(success) => {
            store
                .mark_telegram_bot_delivery_healthy(&fingerprint)
                .await?;
            store
                .record_user_notification_channel_check(
                    TELEGRAM_CHANNEL,
                    &fingerprint,
                    runtime.bot_id.as_deref(),
                    actor_email,
                    binding.target_label.as_deref(),
                    true,
                    success.provider_message_id.as_deref(),
                    Some(success.http_status),
                    None,
                    None,
                    None,
                    None,
                    &tested_at.to_rfc3339(),
                )
                .await?;
            Ok(UserNotificationChannelTestResponse {
                ok: true,
                channel: TELEGRAM_CHANNEL.into(),
                target_label: binding.target_label,
                provider_message_id: success.provider_message_id,
                tested_at: tested_at.to_rfc3339(),
            })
        }
        Err(failure) => {
            if !failure.chat_unreachable {
                let runtime_update =
                    if failure.code == crate::telegram::TelegramFailureCode::InvalidToken {
                        store.mark_telegram_bot_error(&fingerprint, &failure).await
                    } else {
                        store
                            .mark_telegram_bot_transport_failure(&fingerprint, &failure)
                            .await
                    };
                runtime_update?;
            }
            let failure_details = failure
                .diagnostics
                .as_ref()
                .and_then(|value| serde_json::to_string(value).ok());
            store
                .record_user_notification_channel_check(
                    TELEGRAM_CHANNEL,
                    &fingerprint,
                    runtime.bot_id.as_deref(),
                    actor_email,
                    binding.target_label.as_deref(),
                    false,
                    None,
                    failure.http_status,
                    Some(&failure.message),
                    Some(failure.code.as_str()),
                    Some(&failure.hint),
                    failure_details.as_deref(),
                    &tested_at.to_rfc3339(),
                )
                .await?;
            Err(AppError::Coded {
                status: StatusCode::CONFLICT,
                code: "USER_NOTIFICATION_CHANNEL_TEST_FAILED",
                message: failure.hint.clone(),
                details: serde_json::json!({
                    "channel": channel,
                    "httpStatus": failure.http_status,
                    "retryable": failure.retryable,
                    "failureCode": failure.code.as_str(),
                    "failureHint": failure.hint,
                    "diagnostics": failure.diagnostics,
                    "technicalError": failure.message,
                }),
            })
        }
    }
}

fn telegram_channel_state(
    settings: &TelegramBotSettings,
    runtime: &TelegramBotRuntime,
    activity: ChannelCheckActivity,
    binding: Option<TelegramTestBinding>,
) -> UserNotificationChannelState {
    let configured = telegram_configured(settings);
    let fingerprint = telegram_config_fingerprint_for(settings);
    let runtime_matches = fingerprint.as_deref() == runtime.config_fingerprint.as_deref();
    let runtime_ready = settings.enabled && configured && runtime_matches && runtime.ready();
    let diagnostics_visible = settings.enabled && configured && runtime_matches;
    let runtime_failure_active = diagnostics_visible
        && (runtime.transport_status == "degraded" || runtime.readiness == "error");
    let latest_test_failed = activity.last_error.is_some();
    let (failure_code, failure_hint, failure_details, last_error) = if diagnostics_visible {
        if runtime_failure_active {
            (
                runtime.last_failure_code.clone(),
                runtime.last_failure_hint.clone(),
                runtime.last_failure_details.clone(),
                runtime.last_error.clone(),
            )
        } else {
            (
                activity.failure_code.clone(),
                activity.failure_hint.clone(),
                activity.failure_details.clone(),
                activity.last_error.clone(),
            )
        }
    } else {
        (None, None, None, None)
    };
    let status = if !settings.enabled {
        "disabled"
    } else if !configured {
        "misconfigured"
    } else if !runtime_matches || matches!(runtime.readiness.as_str(), "disabled" | "reconciling") {
        "reconciling"
    } else if !runtime_ready || runtime_failure_active || latest_test_failed {
        "degraded"
    } else if activity.last_success_at.is_some() {
        "healthy"
    } else {
        "ready"
    };
    let binding_matches = runtime_ready
        && binding.as_ref().is_some_and(|binding| {
            binding.provider_identity.as_deref() == runtime.bot_id.as_deref()
        });
    UserNotificationChannelState {
        channel: TELEGRAM_CHANNEL.into(),
        enabled: settings.enabled,
        configured,
        status: status.into(),
        runtime_ready,
        transport_status: if diagnostics_visible {
            runtime.transport_status.clone()
        } else {
            "unknown".into()
        },
        provider_label: runtime_ready.then(|| runtime.username.clone()).flatten(),
        runtime_verified_at: runtime_ready.then(|| runtime.verified_at.clone()).flatten(),
        last_attempt_at: activity.last_attempt_at,
        last_success_at: activity.last_success_at,
        last_error,
        failure_code,
        failure_hint,
        failure_details,
        last_failure_at: runtime_failure_active
            .then(|| runtime.last_failure_at.clone())
            .flatten(),
        test_target_available: binding_matches,
        test_target_label: binding_matches
            .then(|| {
                binding
                    .as_ref()
                    .and_then(|value| value.target_label.clone())
            })
            .flatten(),
        binding_verified_at: binding_matches
            .then(|| binding.and_then(|value| value.verified_at))
            .flatten(),
    }
}

fn telegram_configured(settings: &TelegramBotSettings) -> bool {
    settings.token().is_some()
        && (settings.mode != TelegramBotMode::Webhook
            || settings
                .webhook_secret
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()))
}

fn telegram_config_fingerprint_for(settings: &TelegramBotSettings) -> Option<String> {
    settings.token().map(|token| {
        telegram_config_fingerprint(
            token,
            settings.mode.as_str(),
            settings.webhook_secret.as_deref(),
        )
    })
}

impl AppStore {
    async fn telegram_test_binding(
        &self,
        email: &str,
    ) -> Result<Option<TelegramTestBinding>, AppError> {
        let email = email.trim().to_ascii_lowercase();
        let conn = self.conn.lock().await;
        conn.query_row(
            "SELECT channel.target, channel.target_label, channel.provider_identity,
                    channel.verified_at
             FROM user_notification_channels channel
             INNER JOIN users ON users.id = channel.user_id
             WHERE users.email_normalized = ?1 AND channel.channel = 'telegram'
               AND channel.state = 'ready' AND channel.target IS NOT NULL
             LIMIT 1",
            params![email],
            |row| {
                Ok(TelegramTestBinding {
                    chat_id: row.get(0)?,
                    target_label: row.get(1)?,
                    provider_identity: row.get(2)?,
                    verified_at: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|error| {
            AppError::Internal(format!(
                "read administrator Telegram test binding failed: {error}"
            ))
        })
    }

    async fn user_notification_channel_check_activity(
        &self,
        channel: &str,
        config_fingerprint: &str,
    ) -> Result<ChannelCheckActivity, AppError> {
        let conn = self.conn.lock().await;
        let latest = conn
            .query_row(
                "SELECT tested_at, status, error_message, failure_code, failure_hint,
                    failure_details_json
             FROM user_notification_channel_checks
                 WHERE channel = ?1 AND config_fingerprint = ?2
                 ORDER BY tested_at DESC, id DESC LIMIT 1",
                params![channel, config_fingerprint],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!(
                    "read latest user notification channel check failed: {error}"
                ))
            })?;
        let last_success_at = conn
            .query_row(
                "SELECT tested_at FROM user_notification_channel_checks
                 WHERE channel = ?1 AND status = 'success'
                   AND config_fingerprint = ?2
                 ORDER BY tested_at DESC, id DESC LIMIT 1",
                params![channel, config_fingerprint],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!(
                    "read successful user notification channel check failed: {error}"
                ))
            })?;
        let last_error = latest
            .as_ref()
            .filter(|value| value.1 == "failed")
            .and_then(|value| value.2.clone());
        let failure_code = latest
            .as_ref()
            .filter(|value| value.1 == "failed")
            .and_then(|value| value.3.clone());
        let failure_hint = latest
            .as_ref()
            .filter(|value| value.1 == "failed")
            .and_then(|value| value.4.clone());
        let failure_details = latest
            .as_ref()
            .filter(|value| value.1 == "failed")
            .and_then(|value| value.5.as_deref())
            .and_then(|value| serde_json::from_str::<Value>(value).ok());
        Ok(ChannelCheckActivity {
            last_attempt_at: latest.as_ref().map(|value| value.0.clone()),
            last_success_at,
            last_error,
            failure_code,
            failure_hint,
            failure_details,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn record_user_notification_channel_check(
        &self,
        channel: &str,
        config_fingerprint: &str,
        provider_identity: Option<&str>,
        actor_email: &str,
        target_label: Option<&str>,
        success: bool,
        provider_message_id: Option<&str>,
        http_status: Option<u16>,
        error_message: Option<&str>,
        failure_code: Option<&str>,
        failure_hint: Option<&str>,
        failure_details_json: Option<&str>,
        tested_at: &str,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO user_notification_channel_checks (
                id, channel, config_fingerprint, provider_identity, status,
                actor_email, target_label, provider_message_id, http_status,
                error_message, failure_code, failure_hint, failure_details_json,
                tested_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                Uuid::new_v4().to_string(),
                channel,
                config_fingerprint,
                provider_identity,
                if success { "success" } else { "failed" },
                actor_email.trim().to_ascii_lowercase(),
                target_label,
                provider_message_id,
                http_status.map(i64::from),
                error_message,
                failure_code,
                failure_hint,
                failure_details_json,
                tested_at,
            ],
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "record user notification channel check failed: {error}"
            ))
        })?;
        conn.execute(
            "DELETE FROM user_notification_channel_checks
             WHERE id IN (
                 SELECT id FROM user_notification_channel_checks
                 ORDER BY tested_at DESC, id DESC LIMIT -1 OFFSET 200
             )",
            [],
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "prune user notification channel checks failed: {error}"
            ))
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(readiness: &str, fingerprint: Option<&str>) -> TelegramBotRuntime {
        TelegramBotRuntime {
            readiness: readiness.into(),
            transport_status: if readiness == "error" {
                "degraded".into()
            } else {
                "healthy".into()
            },
            bot_id: Some("123".into()),
            username: Some("router_bot".into()),
            config_fingerprint: fingerprint.map(str::to_string),
            generation: 1,
            last_error: (readiness == "error").then(|| "transport failed".into()),
            last_failure_code: (readiness == "error").then(|| "api_timeout".into()),
            last_failure_hint: (readiness == "error").then(|| "check DNS".into()),
            last_failure_details: None,
            last_failure_at: (readiness == "error").then(|| "2026-01-01T00:00:00Z".into()),
            verified_at: Some("2026-01-01T00:00:00Z".into()),
        }
    }

    fn settings() -> TelegramBotSettings {
        TelegramBotSettings {
            enabled: true,
            bot_token: Some("123:token".into()),
            mode: TelegramBotMode::Polling,
            webhook_secret: None,
            bind_token_ttl_secs: 900,
            recipient_hourly_limit: 10,
            global_hourly_limit: 50,
        }
    }

    #[test]
    fn state_separates_configuration_runtime_and_delivery_health() {
        let settings = settings();
        let fingerprint = telegram_config_fingerprint("123:token", "polling", None);
        let ready = telegram_channel_state(
            &settings,
            &runtime("ready", Some(&fingerprint)),
            ChannelCheckActivity::default(),
            None,
        );
        assert_eq!(ready.status, "ready");

        let failed = telegram_channel_state(
            &settings,
            &runtime("ready", Some(&fingerprint)),
            ChannelCheckActivity {
                last_attempt_at: Some("2026-01-02T00:00:00Z".into()),
                last_success_at: Some("2026-01-01T00:00:00Z".into()),
                last_error: Some("send failed".into()),
                failure_code: Some("api_timeout".into()),
                failure_hint: Some("check DNS".into()),
                failure_details: None,
            },
            None,
        );
        assert_eq!(failed.status, "degraded");

        let mut degraded_runtime = runtime("error", Some(&fingerprint));
        degraded_runtime.last_failure_details = Some(serde_json::json!({
            "host": "api.telegram.org",
            "resolvedAddresses": ["118.184.78.78"],
            "reachableAddresses": [],
        }));
        let runtime_failed = telegram_channel_state(
            &settings,
            &degraded_runtime,
            ChannelCheckActivity::default(),
            None,
        );
        assert_eq!(runtime_failed.status, "degraded");
        assert_eq!(runtime_failed.failure_code.as_deref(), Some("api_timeout"));
        assert_eq!(
            runtime_failed
                .failure_details
                .as_ref()
                .and_then(|details| details.get("resolvedAddresses")),
            Some(&serde_json::json!(["118.184.78.78"]))
        );

        let reconciling = telegram_channel_state(
            &settings,
            &runtime("ready", Some("old-fingerprint")),
            ChannelCheckActivity::default(),
            None,
        );
        assert_eq!(reconciling.status, "reconciling");

        let mut disabled_settings = settings;
        disabled_settings.enabled = false;
        let disabled = telegram_channel_state(
            &disabled_settings,
            &runtime("ready", Some(&fingerprint)),
            ChannelCheckActivity::default(),
            Some(TelegramTestBinding {
                chat_id: "42".into(),
                target_label: Some("admin".into()),
                provider_identity: Some("123".into()),
                verified_at: Some("2026-01-01T00:00:00Z".into()),
            }),
        );
        assert_eq!(disabled.status, "disabled");
        assert!(!disabled.runtime_ready);
        assert!(!disabled.test_target_available);
    }
}
