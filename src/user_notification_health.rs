//! Runtime health and administrator-initiated delivery tests for user-facing
//! notification channels. Operator alert channels have a separate lifecycle in
//! `crate::alerting` and must not share health records with this module.

use axum::http::StatusCode;
use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::config::{BarkSettings, TelegramBotMode, TelegramBotSettings};
use crate::db::{OptionalExtension, params};
use crate::error::AppError;
use crate::notification_channels::{BARK_CHANNEL, TELEGRAM_CHANNEL};
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

#[derive(Debug, Clone)]
struct BarkTestBinding {
    encrypted_device_key: String,
    target_label: Option<String>,
    provider_identity: Option<String>,
    credential_revision: i64,
    credential_key_fingerprint: Option<String>,
    user_id: String,
    verified_at: Option<String>,
}

pub async fn channel_states(
    store: &AppStore,
    telegram_settings: &TelegramBotSettings,
    bark_settings: &BarkSettings,
    bark_cipher: Option<&crate::bark::CredentialCipher>,
    actor_email: &str,
) -> Result<Vec<UserNotificationChannelState>, AppError> {
    let runtime = store.telegram_bot_runtime().await?;
    let activity = if let Some(fingerprint) = telegram_config_fingerprint_for(telegram_settings) {
        store
            .user_notification_channel_check_activity(TELEGRAM_CHANNEL, &fingerprint)
            .await?
    } else {
        ChannelCheckActivity::default()
    };
    let binding = store.telegram_test_binding(actor_email).await?;
    let telegram = telegram_channel_state(telegram_settings, &runtime, activity, binding);

    let bark_provider_identity = crate::bark::provider_identity(&bark_settings.server_url).ok();
    let bark_config_fingerprint = if bark_settings.credential_config_current {
        bark_provider_identity
            .as_deref()
            .zip(bark_cipher)
            .map(|(provider_identity, cipher)| {
                crate::bark::binding_config_fingerprint(provider_identity, cipher.key_fingerprint())
            })
    } else {
        None
    };
    let bark_activity = if let Some(fingerprint) = bark_config_fingerprint.as_deref() {
        store
            .user_notification_channel_check_activity(BARK_CHANNEL, fingerprint)
            .await?
    } else {
        ChannelCheckActivity::default()
    };
    let bark_runtime = if let Some(provider_identity) = bark_provider_identity.as_deref() {
        store.bark_provider_runtime(provider_identity).await?
    } else {
        crate::bark::ProviderRuntime::default()
    };
    let bark_binding = store.bark_test_binding(actor_email).await?;
    let bark = bark_channel_state(
        bark_settings,
        bark_cipher,
        bark_provider_identity.as_deref(),
        &bark_runtime,
        bark_activity,
        bark_binding,
    );
    Ok(vec![telegram, bark])
}

pub async fn test_channel(
    store: &AppStore,
    telegram_settings: &TelegramBotSettings,
    bark_settings: &BarkSettings,
    bark_cipher: Option<&crate::bark::CredentialCipher>,
    actor_email: &str,
    channel: &str,
    dashboard_url: &str,
) -> Result<UserNotificationChannelTestResponse, AppError> {
    if channel == BARK_CHANNEL {
        return test_bark_channel(
            store,
            bark_settings,
            bark_cipher,
            actor_email,
            dashboard_url,
        )
        .await;
    }
    if channel != TELEGRAM_CHANNEL {
        return Err(AppError::NotFound(
            "user notification channel not found".into(),
        ));
    }
    if !telegram_settings.enabled {
        return Err(AppError::coded_conflict(
            "USER_NOTIFICATION_CHANNEL_DISABLED",
            "Telegram user notifications are disabled",
            serde_json::json!({ "channel": channel }),
        ));
    }
    if !telegram_configured(telegram_settings) {
        return Err(AppError::coded_conflict(
            "USER_NOTIFICATION_CHANNEL_MISCONFIGURED",
            "Telegram user notification Bot configuration is incomplete",
            serde_json::json!({ "channel": channel }),
        ));
    }

    let token = telegram_settings.token().unwrap_or_default();
    let fingerprint = telegram_config_fingerprint(
        token,
        telegram_settings.mode.as_str(),
        telegram_settings.webhook_secret.as_deref(),
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
            if failure.chat_unreachable {
                // A target-specific rejection is still a successful Bot API
                // round trip. Clear an older provider-wide transport outage
                // while retaining the failed channel check below.
                store
                    .mark_telegram_bot_delivery_healthy(&fingerprint)
                    .await?;
            } else {
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

async fn test_bark_channel(
    store: &AppStore,
    settings: &BarkSettings,
    cipher: Option<&crate::bark::CredentialCipher>,
    actor_email: &str,
    dashboard_url: &str,
) -> Result<UserNotificationChannelTestResponse, AppError> {
    if !settings.enabled {
        return Err(AppError::coded_conflict(
            "USER_NOTIFICATION_CHANNEL_DISABLED",
            "Bark user notifications are disabled",
            serde_json::json!({ "channel": BARK_CHANNEL }),
        ));
    }
    if !settings.credential_config_current {
        return Err(AppError::coded_conflict(
            "USER_NOTIFICATION_CHANNEL_MISCONFIGURED",
            "Bark credential configuration changed; restart Router before sending a test",
            serde_json::json!({ "channel": BARK_CHANNEL }),
        ));
    }
    let provider_identity =
        crate::bark::provider_identity(&settings.server_url).map_err(|error| {
            AppError::coded_conflict(
                "USER_NOTIFICATION_CHANNEL_MISCONFIGURED",
                error,
                serde_json::json!({ "channel": BARK_CHANNEL }),
            )
        })?;
    let cipher = cipher.ok_or_else(|| {
        AppError::coded_conflict(
            "USER_NOTIFICATION_CHANNEL_MISCONFIGURED",
            "Bark credential encryption key is not loaded; restart Router after configuring it",
            serde_json::json!({ "channel": BARK_CHANNEL }),
        )
    })?;
    let config_fingerprint =
        crate::bark::binding_config_fingerprint(&provider_identity, cipher.key_fingerprint());
    let now = Utc::now();
    // This is an explicit administrator recovery probe. It intentionally
    // bypasses the automatic delivery circuit, just like user binding
    // verification, so a successful test can clear the durable circuit early.
    let binding = store.bark_test_binding(actor_email).await?.ok_or_else(|| {
        AppError::coded_conflict(
            "USER_NOTIFICATION_BARK_BINDING_REQUIRED",
            "bind the current administrator account to Bark before sending a test",
            serde_json::json!({ "channel": BARK_CHANNEL }),
        )
    })?;
    if binding.provider_identity.as_deref() != Some(provider_identity.as_str()) {
        return Err(AppError::coded_conflict(
            "USER_NOTIFICATION_BARK_REBIND_REQUIRED",
            "the current administrator Bark binding belongs to a different Bark Server",
            serde_json::json!({ "channel": BARK_CHANNEL }),
        ));
    }
    if binding.credential_key_fingerprint.as_deref() != Some(cipher.key_fingerprint()) {
        return Err(AppError::coded_conflict(
            "USER_NOTIFICATION_BARK_REBIND_REQUIRED",
            "the current administrator Bark binding was encrypted with a different credential key",
            serde_json::json!({ "channel": BARK_CHANNEL }),
        ));
    }
    let aad = crate::bark::credential_aad(
        &binding.user_id,
        binding.credential_revision,
        &provider_identity,
    );
    let device_key = cipher.open(&binding.encrypted_device_key, &aad)?;
    let settings_url = format!(
        "{}/account/notifications",
        dashboard_url.trim_end_matches('/')
    );
    let body = format!(
        "This is a Bark channel test from CC-Switch Router.\n\nAccount: {}\nTime: {}",
        actor_email.trim().to_ascii_lowercase(),
        now.to_rfc3339()
    );
    let request_id = format!("user-notification-bark-test-{}", Uuid::new_v4());
    let request = crate::bark::PushRequest {
        device_key: &device_key,
        title: "通知渠道测试 / CHANNEL TEST",
        body: &body,
        group: "CC-Switch Router",
        url: &settings_url,
        id: &request_id,
        level: "active",
    };
    let http =
        crate::bark::build_http_client("cc-switch-router/0.1 user-notification-bark-channel-test")
            .map_err(|error| {
                AppError::Internal(format!(
                    "build Bark channel test HTTP client failed: {error}"
                ))
            })?;
    match crate::bark::send(&http, &settings.server_url, &request).await {
        Ok(success) => {
            store
                .mark_bark_provider_healthy(&provider_identity, now)
                .await?;
            store
                .record_user_notification_channel_check(
                    BARK_CHANNEL,
                    &config_fingerprint,
                    Some(&provider_identity),
                    actor_email,
                    binding.target_label.as_deref(),
                    true,
                    success.provider_message_id.as_deref(),
                    Some(success.http_status),
                    None,
                    None,
                    None,
                    None,
                    &now.to_rfc3339(),
                )
                .await?;
            Ok(UserNotificationChannelTestResponse {
                ok: true,
                channel: BARK_CHANNEL.into(),
                target_label: binding.target_label,
                provider_message_id: success.provider_message_id,
                tested_at: now.to_rfc3339(),
            })
        }
        Err(failure) => {
            let binding_invalidated = if failure.target_invalid {
                // The Provider request started with this exact encrypted
                // target. If the administrator rebound while it was in
                // flight, the CAS deliberately leaves the new binding alone.
                let invalidated = store
                    .invalidate_bark_binding_after_test_failure(
                        &binding.user_id,
                        &provider_identity,
                        &binding.encrypted_device_key,
                        now,
                    )
                    .await?;
                store
                    .mark_bark_provider_healthy(&provider_identity, now)
                    .await?;
                invalidated
            } else {
                store
                    .mark_bark_provider_failure(
                        &provider_identity,
                        Some(&failure.code),
                        Some(&failure.hint),
                        &failure.message,
                        now,
                    )
                    .await?;
                false
            };
            store
                .record_user_notification_channel_check(
                    BARK_CHANNEL,
                    &config_fingerprint,
                    Some(&provider_identity),
                    actor_email,
                    binding.target_label.as_deref(),
                    false,
                    None,
                    failure.http_status,
                    Some(&failure.message),
                    Some(&failure.code),
                    Some(&failure.hint),
                    None,
                    &now.to_rfc3339(),
                )
                .await?;
            Err(AppError::Coded {
                status: StatusCode::CONFLICT,
                code: "USER_NOTIFICATION_CHANNEL_TEST_FAILED",
                message: failure.hint.clone(),
                details: serde_json::json!({
                    "channel": BARK_CHANNEL,
                    "httpStatus": failure.http_status,
                    "retryable": failure.retryable,
                    "failureCode": failure.code,
                    "failureHint": failure.hint,
                    "technicalError": failure.message,
                    "bindingInvalidated": binding_invalidated,
                }),
            })
        }
    }
}

fn bark_channel_state(
    settings: &BarkSettings,
    cipher: Option<&crate::bark::CredentialCipher>,
    provider_identity: Option<&str>,
    runtime: &crate::bark::ProviderRuntime,
    activity: ChannelCheckActivity,
    binding: Option<BarkTestBinding>,
) -> UserNotificationChannelState {
    let configured =
        settings.credential_config_current && cipher.is_some() && provider_identity.is_some();
    let diagnostics_visible = settings.enabled && configured && runtime.configuration_active;
    let circuit_open = runtime
        .circuit_open_until
        .as_deref()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .is_some_and(|until| until.timestamp() > Utc::now().timestamp());
    let runtime_ready = diagnostics_visible && !circuit_open;
    let degraded = diagnostics_visible
        && (runtime.status == "degraded" || circuit_open || activity.last_error.is_some());
    let status = if !settings.enabled {
        "disabled"
    } else if !configured {
        "misconfigured"
    } else if !runtime.configuration_active {
        "reconciling"
    } else if degraded {
        "degraded"
    } else if activity.last_success_at.is_some() || runtime.last_success_at.is_some() {
        "healthy"
    } else {
        "ready"
    };
    let binding_matches = diagnostics_visible
        && binding.as_ref().is_some_and(|binding| {
            binding.provider_identity.as_deref() == provider_identity
                && binding.credential_key_fingerprint.as_deref()
                    == cipher.map(crate::bark::CredentialCipher::key_fingerprint)
                && cipher.is_some_and(|cipher| {
                    let aad = crate::bark::credential_aad(
                        &binding.user_id,
                        binding.credential_revision,
                        provider_identity.unwrap_or_default(),
                    );
                    cipher.open(&binding.encrypted_device_key, &aad).is_ok()
                })
        });
    let runtime_failure_active = diagnostics_visible && runtime.status == "degraded";
    let (failure_code, failure_hint, failure_details, last_error) = if diagnostics_visible {
        if runtime_failure_active {
            (
                runtime.last_failure_code.clone(),
                runtime.last_failure_hint.clone(),
                None,
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
    UserNotificationChannelState {
        channel: BARK_CHANNEL.into(),
        enabled: settings.enabled,
        configured,
        status: status.into(),
        runtime_ready,
        transport_status: if !diagnostics_visible {
            "unknown"
        } else if degraded {
            "degraded"
        } else {
            "healthy"
        }
        .into(),
        provider_label: provider_identity
            .and_then(|_| crate::bark::normalize_server_url(&settings.server_url).ok()),
        runtime_verified_at: diagnostics_visible
            .then(|| runtime.last_success_at.clone())
            .flatten(),
        last_attempt_at: activity.last_attempt_at,
        last_success_at: activity
            .last_success_at
            .or_else(|| runtime.last_success_at.clone()),
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
    async fn bark_test_binding(&self, email: &str) -> Result<Option<BarkTestBinding>, AppError> {
        let email = email.trim().to_ascii_lowercase();
        let conn = self.conn.lock().await;
        conn.query_row(
            "SELECT channel.target, channel.target_label, channel.provider_identity,
                    channel.credential_revision, channel.credential_key_fingerprint,
                    channel.user_id, channel.verified_at
             FROM user_notification_channels channel
             INNER JOIN users ON users.id = channel.user_id
             WHERE users.email_normalized = ?1 AND channel.channel = 'bark'
               AND channel.state = 'ready' AND channel.target IS NOT NULL
             LIMIT 1",
            params![email],
            |row| {
                Ok(BarkTestBinding {
                    encrypted_device_key: row.get(0)?,
                    target_label: row.get(1)?,
                    provider_identity: row.get(2)?,
                    credential_revision: row.get(3)?,
                    credential_key_fingerprint: row.get(4)?,
                    user_id: row.get(5)?,
                    verified_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(|error| {
            AppError::Internal(format!(
                "read administrator Bark test binding failed: {error}"
            ))
        })
    }

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

    #[test]
    fn pending_bark_credential_restart_is_not_reported_ready() {
        let mut settings = BarkSettings {
            enabled: true,
            credential_master_key: Some("11".repeat(32)),
            ..BarkSettings::default()
        };
        let cipher = crate::bark::CredentialCipher::from_settings(&settings)
            .expect("valid Bark credential key")
            .expect("configured Bark credential key");
        let provider_identity =
            crate::bark::provider_identity(&settings.server_url).expect("valid Bark Server");
        settings.credential_config_current = false;
        let state = bark_channel_state(
            &settings,
            Some(&cipher),
            Some(&provider_identity),
            &crate::bark::ProviderRuntime {
                configuration_active: true,
                status: "ready".into(),
                ..crate::bark::ProviderRuntime::default()
            },
            ChannelCheckActivity::default(),
            None,
        );
        assert!(!state.configured);
        assert!(!state.runtime_ready);
        assert!(!state.test_target_available);
        assert_eq!(state.status, "misconfigured");
    }

    #[test]
    fn bark_open_circuit_keeps_a_matching_target_available_for_manual_recovery() {
        let settings = BarkSettings {
            enabled: true,
            credential_master_key: Some("11".repeat(32)),
            ..BarkSettings::default()
        };
        let cipher = crate::bark::CredentialCipher::from_settings(&settings)
            .expect("valid Bark credential key")
            .expect("configured Bark credential key");
        let provider_identity =
            crate::bark::provider_identity(&settings.server_url).expect("valid Bark Server");
        let credential_revision = 3;
        let aad = crate::bark::credential_aad("admin", credential_revision, &provider_identity);
        let encrypted_device_key = cipher
            .seal("device_key_123", &aad)
            .expect("encrypt Bark test binding");
        let state = bark_channel_state(
            &settings,
            Some(&cipher),
            Some(&provider_identity),
            &crate::bark::ProviderRuntime {
                configuration_active: true,
                status: "degraded".into(),
                circuit_open_until: Some((Utc::now() + chrono::Duration::minutes(5)).to_rfc3339()),
                ..crate::bark::ProviderRuntime::default()
            },
            ChannelCheckActivity::default(),
            Some(BarkTestBinding {
                encrypted_device_key,
                target_label: Some("••••_123".into()),
                provider_identity: Some(provider_identity.clone()),
                credential_revision,
                credential_key_fingerprint: Some(cipher.key_fingerprint().into()),
                user_id: "admin".into(),
                verified_at: Some(Utc::now().to_rfc3339()),
            }),
        );
        assert_eq!(state.status, "degraded");
        assert!(!state.runtime_ready);
        assert!(state.test_target_available);
    }

    #[test]
    fn bark_state_hides_stale_diagnostics_when_disabled_or_runtime_mismatched() {
        let mut settings = BarkSettings {
            enabled: true,
            credential_master_key: Some("11".repeat(32)),
            ..BarkSettings::default()
        };
        let cipher = crate::bark::CredentialCipher::from_settings(&settings)
            .expect("valid Bark credential key")
            .expect("configured Bark credential key");
        let provider_identity =
            crate::bark::provider_identity(&settings.server_url).expect("valid Bark Server");
        let stale_activity = || ChannelCheckActivity {
            last_attempt_at: Some("2026-01-02T00:00:00Z".into()),
            last_success_at: None,
            last_error: Some("old Provider failure".into()),
            failure_code: Some("server_error".into()),
            failure_hint: Some("old Provider hint".into()),
            failure_details: Some(serde_json::json!({ "old": true })),
        };
        let stale_runtime = crate::bark::ProviderRuntime {
            configuration_active: false,
            status: "degraded".into(),
            last_error: Some("old runtime failure".into()),
            last_failure_code: Some("server_error".into()),
            last_failure_hint: Some("old runtime hint".into()),
            last_failure_at: Some("2026-01-02T00:00:00Z".into()),
            ..crate::bark::ProviderRuntime::default()
        };

        let reconciling = bark_channel_state(
            &settings,
            Some(&cipher),
            Some(&provider_identity),
            &stale_runtime,
            stale_activity(),
            None,
        );
        assert_eq!(reconciling.status, "reconciling");
        assert_eq!(reconciling.transport_status, "unknown");
        assert!(!reconciling.runtime_ready);
        assert!(reconciling.last_error.is_none());
        assert!(reconciling.failure_code.is_none());
        assert!(reconciling.failure_hint.is_none());
        assert!(reconciling.failure_details.is_none());
        assert!(reconciling.last_failure_at.is_none());

        settings.enabled = false;
        let disabled = bark_channel_state(
            &settings,
            Some(&cipher),
            Some(&provider_identity),
            &crate::bark::ProviderRuntime {
                configuration_active: true,
                ..stale_runtime
            },
            stale_activity(),
            None,
        );
        assert_eq!(disabled.status, "disabled");
        assert_eq!(disabled.transport_status, "unknown");
        assert!(disabled.last_error.is_none());
        assert!(disabled.failure_code.is_none());
        assert!(disabled.failure_hint.is_none());
        assert!(disabled.failure_details.is_none());
    }

    #[test]
    fn bark_channel_check_fingerprint_changes_with_the_credential_key() {
        let provider_identity =
            crate::bark::provider_identity("https://api.day.app").expect("valid Bark Server");
        let first_settings = BarkSettings {
            enabled: true,
            credential_master_key: Some("21".repeat(32)),
            ..BarkSettings::default()
        };
        let second_settings = BarkSettings {
            credential_master_key: Some("22".repeat(32)),
            ..first_settings.clone()
        };
        let first_cipher = crate::bark::CredentialCipher::from_settings(&first_settings)
            .expect("valid first Bark key")
            .expect("configured first Bark key");
        let second_cipher = crate::bark::CredentialCipher::from_settings(&second_settings)
            .expect("valid second Bark key")
            .expect("configured second Bark key");

        let first = crate::bark::binding_config_fingerprint(
            &provider_identity,
            first_cipher.key_fingerprint(),
        );
        let second = crate::bark::binding_config_fingerprint(
            &provider_identity,
            second_cipher.key_fingerprint(),
        );
        assert_ne!(first, second);
        assert_eq!(first.len(), 64);
        assert_eq!(second.len(), 64);
    }
}
