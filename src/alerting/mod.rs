pub mod channels;
pub mod models;
pub mod store;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use chrono::Utc;
use tokio::sync::RwLock;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::config::{AlertingSettings, Config};
use crate::dynamic_settings::DynamicSettings;
use crate::error::AppError;
use crate::store::AppStore;

use self::models::{
    AlertChannelPolicy, AlertChannelState, AlertChannelTestResponse, AlertCondition,
    AlertDeliveryPolicy, AlertIncident, AlertingOverview,
};
use self::store::{AlertDeliveryResult, AlertStore};

const DELIVERY_CLAIM_SECS: i64 = 60;
const MAX_DELIVERIES_PER_CYCLE: usize = 20;
const MAX_SIGNALS_PER_CYCLE: usize = 50;
const MAX_DELIVERY_ATTEMPTS: u32 = 12;

#[derive(Debug)]
pub struct AlertingService {
    store: AlertStore,
    dynamic: Arc<RwLock<DynamicSettings>>,
    http: reqwest::Client,
    dashboard_url: String,
}

impl AlertingService {
    pub fn new(
        metrics_db_path: PathBuf,
        dynamic: Arc<RwLock<DynamicSettings>>,
        config: &Config,
    ) -> Result<Arc<Self>, AppError> {
        let scheme = if config.use_localhost {
            "http"
        } else {
            "https"
        };
        let dashboard_url = format!("{scheme}://{}", config.tunnel_domain.trim_end_matches('/'));
        let http = channels::build_http_client().map_err(|error| {
            AppError::Internal(format!("build alert HTTP client failed: {error}"))
        })?;
        Ok(Arc::new(Self {
            store: AlertStore::new(metrics_db_path),
            dynamic,
            http,
            dashboard_url,
        }))
    }

    pub fn store(&self) -> &AlertStore {
        &self.store
    }

    pub async fn init(&self) -> Result<(), AppError> {
        self.store.init().await
    }

    pub async fn reconcile_metrics(
        &self,
        conditions: Vec<AlertCondition>,
        now: i64,
    ) -> Result<(), AppError> {
        self.reconcile_scope("metrics", conditions, now).await
    }

    pub async fn reconcile_clock(
        &self,
        conditions: Vec<AlertCondition>,
        now: i64,
    ) -> Result<(), AppError> {
        self.reconcile_scope("clock", conditions, now).await
    }

    async fn reconcile_scope(
        &self,
        scope: &str,
        conditions: Vec<AlertCondition>,
        now: i64,
    ) -> Result<(), AppError> {
        let settings = self.settings().await;
        self.store
            .reconcile_conditions(
                scope.into(),
                conditions,
                now,
                settings.repeat_interval_secs,
                self.delivery_policy(&settings),
            )
            .await?;
        Ok(())
    }

    pub async fn active_incidents(&self) -> Result<Vec<AlertIncident>, AppError> {
        self.store.list_incidents(500, true).await
    }

    pub async fn overview(&self, limit: usize) -> Result<AlertingOverview, AppError> {
        let settings = self.settings().await;
        let incidents = self.store.list_incidents(limit, false).await?;
        let deliveries = self.store.list_deliveries(limit).await?;
        let channels = self.channel_states_for(&settings).await?;
        let counts = self.store.overview_counts().await?;
        Ok(AlertingOverview {
            active_count: counts.active,
            critical_count: counts.critical,
            resolved_count: counts.resolved,
            failed_delivery_count: counts.failed_deliveries,
            incidents,
            deliveries,
            channels,
        })
    }

    pub async fn channel_states(&self) -> Result<Vec<AlertChannelState>, AppError> {
        let settings = self.settings().await;
        self.channel_states_for(&settings).await
    }

    pub async fn test_channel(&self, channel: &str) -> Result<AlertChannelTestResponse, AppError> {
        let settings = self.settings().await;
        if !channels::is_registered(channel) {
            return Err(AppError::NotFound("alert channel not found".into()));
        }
        if !channel_configured(&settings, channel) {
            return Err(AppError::Conflict(format!(
                "{channel} alert channel is not fully configured"
            )));
        }
        let tested_at = Utc::now().timestamp();
        let text = format!(
            "[CC-Switch Router] Alert channel test\nChannel: {channel}\nTime: {}\nDashboard: {}/settings/",
            chrono::DateTime::from_timestamp(tested_at, 0)
                .map(|value| value.to_rfc3339())
                .unwrap_or_else(|| tested_at.to_string()),
            self.dashboard_url.trim_end_matches('/')
        );
        match channels::send(&self.http, &settings, channel, &text).await {
            Ok(success) => {
                self.store
                    .record_channel_test(
                        channel.into(),
                        true,
                        success.provider_message_id.clone(),
                        Some(success.http_status),
                        None,
                        None,
                        None,
                        None,
                        tested_at,
                    )
                    .await?;
                Ok(AlertChannelTestResponse {
                    ok: true,
                    channel: channel.into(),
                    provider_message_id: success.provider_message_id,
                    tested_at,
                })
            }
            Err(failure) => {
                self.store
                    .record_channel_test(
                        channel.into(),
                        false,
                        None,
                        failure.http_status,
                        Some(failure.message.clone()),
                        Some(failure.failure_code.clone()),
                        Some(failure.failure_hint.clone()),
                        failure.failure_details.clone(),
                        tested_at,
                    )
                    .await?;
                Err(AppError::Coded {
                    status: if failure.http_status.is_some_and(|status| status == 401) {
                        StatusCode::UNAUTHORIZED
                    } else if failure.http_status.is_some_and(|status| status == 429) {
                        StatusCode::TOO_MANY_REQUESTS
                    } else {
                        StatusCode::BAD_GATEWAY
                    },
                    code: "ALERT_CHANNEL_TEST_FAILED",
                    message: failure.failure_hint.clone(),
                    details: serde_json::json!({
                        "channel": channel,
                        "httpStatus": failure.http_status,
                        "retryable": failure.retryable,
                        "failureCode": failure.failure_code,
                        "failureHint": failure.failure_hint,
                        "diagnostics": failure.failure_details,
                        "technicalError": failure.message,
                    }),
                })
            }
        }
    }

    pub fn delivery_policy(&self, settings: &AlertingSettings) -> AlertDeliveryPolicy {
        self.delivery_policy_sync(settings)
    }

    pub async fn current_delivery_policy(&self) -> AlertDeliveryPolicy {
        let settings = self.settings().await;
        self.delivery_policy_sync(&settings)
    }

    fn delivery_policy_sync(&self, settings: &AlertingSettings) -> AlertDeliveryPolicy {
        let delivery_channels = channels::REGISTERED_CHANNELS
            .iter()
            .copied()
            .filter(|channel| {
                channel_enabled(settings, channel) && channel_configured(settings, channel)
            })
            .filter_map(|channel| {
                channel_min_severity(settings, channel).map(|min_severity| AlertChannelPolicy {
                    channel: channel.into(),
                    min_severity: normalize_min_severity(min_severity).into(),
                })
            })
            .collect();
        AlertDeliveryPolicy {
            enabled: settings.enabled,
            dashboard_url: self.dashboard_url.clone(),
            channels: delivery_channels,
        }
    }

    async fn settings(&self) -> AlertingSettings {
        self.dynamic.read().await.alerting.clone()
    }

    async fn channel_states_for(
        &self,
        settings: &AlertingSettings,
    ) -> Result<Vec<AlertChannelState>, AppError> {
        let activity = self.store.channel_activity().await?;
        Ok(channels::REGISTERED_CHANNELS
            .iter()
            .copied()
            .map(|channel| {
                let enabled = channel_enabled(settings, channel);
                let configured = channel_configured(settings, channel);
                let channel_activity = activity.get(channel).cloned().unwrap_or_default();
                let last_attempt_at = channel_activity.last_attempt_at;
                let last_success_at = channel_activity.last_success_at;
                let last_error = channel_activity.last_error.clone();
                let status = if !settings.enabled || !enabled {
                    "disabled"
                } else if !configured {
                    "misconfigured"
                } else if last_error.is_some()
                    && last_success_at.is_none_or(|success| {
                        last_attempt_at.is_some_and(|attempt| attempt >= success)
                    })
                {
                    "degraded"
                } else if last_success_at.is_some() {
                    "healthy"
                } else {
                    "ready"
                };
                AlertChannelState {
                    channel: channel.into(),
                    enabled,
                    configured,
                    status: status.into(),
                    last_attempt_at,
                    last_success_at,
                    last_error,
                    failure_code: channel_activity.failure_code,
                    failure_hint: channel_activity.failure_hint,
                    failure_details: channel_activity.failure_details,
                }
            })
            .collect())
    }

    async fn process_cycle(&self, app_store: &AppStore, worker_id: &str) -> Result<(), AppError> {
        let now = Utc::now().timestamp();
        let settings = self.settings().await;
        let policy = self.delivery_policy_sync(&settings);
        self.store.expire_silences(now, policy.clone()).await?;

        let enabled_channels = policy
            .channels
            .iter()
            .map(|channel| channel.channel.clone())
            .collect::<HashSet<_>>();
        self.store
            .suppress_disabled_deliveries(enabled_channels, now)
            .await?;

        let signals = app_store
            .claim_operator_alert_signals(
                worker_id,
                Utc::now(),
                DELIVERY_CLAIM_SECS,
                MAX_SIGNALS_PER_CYCLE,
            )
            .await?;
        for signal in signals {
            let source_event_id = signal.source_event_id.clone();
            let attempts = signal.attempts;
            match self.store.ingest_signal(signal, policy.clone()).await {
                Ok(_) => {
                    app_store
                        .complete_operator_alert_signal(&source_event_id, worker_id, Utc::now())
                        .await?;
                }
                Err(error) => {
                    let delay = retry_delay_secs(attempts, &source_event_id);
                    app_store
                        .retry_operator_alert_signal(
                            &source_event_id,
                            worker_id,
                            &error.to_string(),
                            Utc::now() + chrono::Duration::seconds(delay),
                            Utc::now(),
                        )
                        .await?;
                    warn!(source_event_id, error = %error, "operator alert signal deferred");
                }
            }
        }

        for _ in 0..MAX_DELIVERIES_PER_CYCLE {
            let Some(delivery) = self
                .store
                .claim_delivery(
                    worker_id.into(),
                    Utc::now().timestamp(),
                    DELIVERY_CLAIM_SECS,
                )
                .await?
            else {
                break;
            };
            let current = self.settings().await;
            if !current.enabled || !channel_enabled(&current, &delivery.channel) {
                self.store
                    .finish_delivery(
                        delivery,
                        AlertDeliveryResult::Suppressed {
                            reason: "alerting or channel disabled before send".into(),
                        },
                        Utc::now().timestamp(),
                    )
                    .await?;
                continue;
            }
            let send_result = channels::send(
                &self.http,
                &current,
                &delivery.channel,
                &delivery.payload_text,
            )
            .await;
            let result = match send_result {
                Ok(success) => AlertDeliveryResult::Sent {
                    provider_message_id: success.provider_message_id,
                    http_status: Some(success.http_status),
                },
                Err(failure) if failure.retryable && delivery.attempts < MAX_DELIVERY_ATTEMPTS => {
                    AlertDeliveryResult::Retry {
                        error: failure.message,
                        failure_code: Some(failure.failure_code),
                        failure_hint: Some(failure.failure_hint),
                        failure_details: failure.failure_details,
                        next_attempt_at: failure.retry_at.unwrap_or_else(|| {
                            Utc::now()
                                .timestamp()
                                .saturating_add(retry_delay_secs(delivery.attempts, &delivery.id))
                        }),
                        http_status: failure.http_status,
                    }
                }
                Err(failure) => AlertDeliveryResult::DeadLetter {
                    error: failure.message,
                    failure_code: Some(failure.failure_code),
                    failure_hint: Some(failure.failure_hint),
                    failure_details: failure.failure_details,
                    http_status: failure.http_status,
                },
            };
            self.store
                .finish_delivery(delivery, result, Utc::now().timestamp())
                .await?;
        }
        Ok(())
    }
}

pub async fn run_alerting_service(
    service: Arc<AlertingService>,
    app_store: AppStore,
) -> anyhow::Result<()> {
    service.init().await?;
    let worker_id = format!("alerting-{}", Uuid::new_v4());
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_prune = Instant::now();
    loop {
        interval.tick().await;
        if let Err(error) = service.process_cycle(&app_store, &worker_id).await {
            warn!(error = %error, "operator alert cycle failed");
        }
        if last_prune.elapsed() >= Duration::from_secs(60 * 60) {
            let retention_days = service.settings().await.history_retention_days;
            if let Err(error) = service
                .store
                .prune(Utc::now().timestamp(), retention_days)
                .await
            {
                debug!(error = %error, "operator alert history prune failed");
            }
            let cutoff = Utc::now() - chrono::Duration::days(i64::from(retention_days.max(1)));
            if let Err(error) = app_store.prune_operator_alert_signals(cutoff).await {
                debug!(error = %error, "operator alert signal outbox prune failed");
            }
            last_prune = Instant::now();
        }
    }
}

fn channel_enabled(settings: &AlertingSettings, channel: &str) -> bool {
    match channel {
        channels::TELEGRAM_CHANNEL => settings.telegram_enabled,
        _ => false,
    }
}

fn channel_configured(settings: &AlertingSettings, channel: &str) -> bool {
    match channel {
        channels::TELEGRAM_CHANNEL => {
            configured(settings.telegram_bot_token.as_deref())
                && configured(settings.telegram_chat_id.as_deref())
        }
        _ => false,
    }
}

fn channel_min_severity<'a>(settings: &'a AlertingSettings, channel: &str) -> Option<&'a str> {
    match channel {
        channels::TELEGRAM_CHANNEL => Some(&settings.telegram_min_severity),
        _ => None,
    }
}

fn configured(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.trim().is_empty())
}

fn normalize_min_severity(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "critical" => "critical",
        "info" => "info",
        _ => "warning",
    }
}

fn retry_delay_secs(attempts: u32, stable_id: &str) -> i64 {
    let exponent = attempts.min(9);
    let base = 15_i64.saturating_mul(1_i64 << exponent).min(60 * 60);
    let jitter = stable_id.bytes().fold(0_u64, |value, byte| {
        value.wrapping_mul(31).wrapping_add(u64::from(byte))
    }) % (base.max(5) as u64 / 5).max(1);
    base + jitter as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_channel_has_runtime_settings() {
        let settings = AlertingSettings::default();
        for channel in channels::REGISTERED_CHANNELS {
            assert!(channel_min_severity(&settings, channel).is_some());
        }
    }
}
