use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::header::RETRY_AFTER;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::{ClientNotificationSettings, Config};
use crate::dynamic_settings::DynamicSettings;
use crate::error::AppError;
use crate::store::AppStore;

const DELIVERY_INTERVAL_SECS: u64 = 5;
const PRESENCE_RECONCILE_INTERVAL_SECS: u64 = 20;
const CLAIM_LEASE_SECS: i64 = 90;
const MAX_BATCHES_PER_CYCLE: usize = 25;
const MAX_DELIVERY_ATTEMPTS: u32 = 12;
const RESEND_EMAILS_ENDPOINT: &str = "https://api.resend.com/emails";
const MAX_DIGEST_CLIENTS: usize = 50;
pub(crate) const MIN_OFFLINE_ALERT_SECS: i64 = 180;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientNotificationPolicy {
    pub enabled: bool,
    pub alert_emails: Vec<String>,
    pub offline_alert_secs: i64,
    pub recovery_stable_secs: i64,
    pub cooldown_secs: i64,
    pub batch_window_secs: i64,
    pub storm_window_secs: i64,
    pub storm_min_clients: i64,
    pub storm_percent: i64,
    pub storm_reminder_secs: i64,
    pub recipient_hourly_limit: i64,
    pub global_hourly_limit: i64,
    pub registration_recipient_hourly_limit: i64,
    pub registration_global_hourly_limit: i64,
    pub notify_owner: bool,
}

impl From<&ClientNotificationSettings> for ClientNotificationPolicy {
    fn from(settings: &ClientNotificationSettings) -> Self {
        let mut alert_emails = settings
            .alert_emails
            .iter()
            .map(|email| email.trim().to_ascii_lowercase())
            .filter(|email| is_basic_email(email))
            .collect::<Vec<_>>();
        alert_emails.sort_unstable();
        alert_emails.dedup();
        let enabled = settings.enabled && !alert_emails.is_empty();
        let registration_recipient_hourly_limit =
            settings.registration_recipient_hourly_limit.clamp(1, 1_000);
        let registration_global_hourly_limit = settings
            .registration_global_hourly_limit
            .clamp(1, 10_000)
            .max(registration_recipient_hourly_limit);
        Self {
            enabled,
            alert_emails,
            offline_alert_secs: settings
                .offline_alert_secs
                .clamp(MIN_OFFLINE_ALERT_SECS, 86_400),
            recovery_stable_secs: settings.recovery_stable_secs.clamp(30, 3_600),
            cooldown_secs: settings.cooldown_secs.clamp(60, 604_800),
            batch_window_secs: settings.batch_window_secs.clamp(1, 600),
            storm_window_secs: settings.storm_window_secs.clamp(60, 3_600),
            storm_min_clients: settings.storm_min_clients.clamp(2, 10_000),
            storm_percent: settings.storm_percent.clamp(1, 100),
            storm_reminder_secs: settings.storm_reminder_secs.clamp(300, 86_400),
            recipient_hourly_limit: settings.recipient_hourly_limit.clamp(1, 10_000),
            global_hourly_limit: settings.global_hourly_limit.clamp(1, 100_000),
            registration_recipient_hourly_limit,
            registration_global_hourly_limit,
            notify_owner: settings.notify_owner,
        }
    }
}

impl ClientNotificationPolicy {
    pub fn for_runtime(
        settings: &ClientNotificationSettings,
        config: &Config,
    ) -> (Self, Option<String>) {
        let mut policy = Self::from(settings);
        if settings.enabled && !policy.enabled {
            return (
                policy,
                Some(
                    "client notifications disabled: at least one valid explicit alert recipient is required"
                        .to_string(),
                ),
            );
        }
        if !policy.enabled {
            return (policy, None);
        }
        let Some(max_offline_secs) = max_safe_offline_alert_secs(config) else {
            policy.enabled = false;
            return (
                policy,
                Some(format!(
                    "client notifications disabled: CLIENT_STALE_SECS={} does not leave a {MIN_OFFLINE_ALERT_SECS}-second notification window before cleanup",
                    config.client_stale_secs,
                )),
            );
        };
        if policy.offline_alert_secs > max_offline_secs {
            let configured = policy.offline_alert_secs;
            policy.offline_alert_secs = max_offline_secs;
            return (
                policy,
                Some(format!(
                    "client offline alert clamped from {configured}s to {max_offline_secs}s so notification reconciliation precedes cleanup"
                )),
            );
        }
        (policy, None)
    }
}

pub fn validate_notification_cleanup_window(
    settings: &ClientNotificationSettings,
    config: &Config,
) -> Result<(), String> {
    if !ClientNotificationPolicy::from(settings).enabled {
        return Ok(());
    }
    if settings.offline_alert_secs < MIN_OFFLINE_ALERT_SECS {
        return Err(format!(
            "CLIENT_OFFLINE_ALERT_SECS must be at least {MIN_OFFLINE_ALERT_SECS} seconds"
        ));
    }
    let Some(max_offline_secs) = max_safe_offline_alert_secs(config) else {
        return Err(format!(
            "CLIENT_STALE_SECS={} must exceed the cleanup safety margin by at least {MIN_OFFLINE_ALERT_SECS} seconds before client notifications can be configured",
            config.client_stale_secs,
        ));
    };
    let offline_alert_secs = settings
        .offline_alert_secs
        .clamp(MIN_OFFLINE_ALERT_SECS, 86_400);
    if offline_alert_secs > max_offline_secs {
        return Err(format!(
            "CLIENT_OFFLINE_ALERT_SECS must be at most {max_offline_secs} for the current CLIENT_STALE_SECS and cleanup interval"
        ));
    }
    Ok(())
}

fn max_safe_offline_alert_secs(config: &Config) -> Option<i64> {
    let cleanup_margin = i64::try_from(config.cleanup_interval_secs)
        .unwrap_or(i64::MAX)
        .max(60);
    let maximum = config.client_stale_secs.saturating_sub(cleanup_margin);
    (maximum >= MIN_OFFLINE_ALERT_SECS).then_some(maximum)
}

#[derive(Debug, Clone)]
pub struct NotificationTemplateContext {
    pub dashboard_url: String,
    pub sender: Option<String>,
    pub reply_to: Option<String>,
    pub delivery_configured: bool,
    /// Changes whenever a restart loads different delivery configuration.
    pub delivery_config_fingerprint: String,
}

impl NotificationTemplateContext {
    pub fn from_config(config: &Config) -> Self {
        let scheme = if config.use_localhost {
            "http"
        } else {
            "https"
        };
        let dashboard_url = format!("{scheme}://{}", config.tunnel_domain.trim_end_matches('/'));
        let sender = notification_sender(config);
        let reply_to = config
            .resend_reply_to
            .as_deref()
            .map(strip_header_controls)
            .filter(|value| is_basic_email(value));
        let delivery_config_fingerprint = delivery_config_fingerprint(
            config.resend_api_key.as_deref(),
            sender.as_deref(),
            reply_to.as_deref(),
        );
        let delivery_configured = config
            .resend_api_key
            .as_deref()
            .is_some_and(|key| !key.trim().is_empty())
            && sender.is_some();
        Self {
            dashboard_url,
            sender,
            reply_to,
            delivery_configured,
            delivery_config_fingerprint,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct NotificationReconcileStats {
    pub baselined: u64,
    pub offline_events: u64,
    pub recovered: u64,
    pub suppressed_disabled: u64,
    pub suppressed_recipient_removed: u64,
}

impl NotificationReconcileStats {
    fn has_activity(&self) -> bool {
        self.baselined > 0
            || self.offline_events > 0
            || self.recovered > 0
            || self.suppressed_disabled > 0
            || self.suppressed_recipient_removed > 0
    }
}

#[derive(Debug, Clone, Default)]
pub struct NotificationAggregateStats {
    pub batches_created: u64,
    pub events_batched: u64,
    pub incident_digests: u64,
    pub deferred_by_recipient_cap: u64,
    pub deferred_by_global_cap: u64,
}

impl NotificationAggregateStats {
    fn has_activity(&self) -> bool {
        self.batches_created > 0
            || self.events_batched > 0
            || self.incident_digests > 0
            || self.deferred_by_recipient_cap > 0
            || self.deferred_by_global_cap > 0
    }
}

/// A fully frozen Resend request. Retried deliveries must reuse every field and
/// the idempotency key byte-for-byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientNotificationBatch {
    pub id: String,
    pub recipient: String,
    pub from: String,
    pub reply_to: Option<String>,
    pub subject: String,
    pub html: String,
    pub idempotency_key: String,
    pub attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientNotificationClaim {
    Batch(ClientNotificationBatch),
    SuppressedByRateLimit,
    Empty,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClientNotificationDeliveryView {
    pub id: String,
    pub delivery_kind: String,
    pub event_kind: String,
    pub event_count: u64,
    pub recipient_masked: String,
    pub status: String,
    pub attempts: u32,
    pub created_at: String,
    pub next_attempt_at: Option<String>,
    pub sent_at: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClientNotificationDeliveriesResponse {
    pub deliveries: Vec<ClientNotificationDeliveryView>,
}

#[async_trait]
pub trait ClientNotificationStore: Clone + Send + Sync + 'static {
    async fn reconcile_client_notification_events(
        &self,
        policy: &ClientNotificationPolicy,
        now: DateTime<Utc>,
    ) -> Result<NotificationReconcileStats, AppError>;

    async fn aggregate_client_notification_batches(
        &self,
        policy: &ClientNotificationPolicy,
        template: &NotificationTemplateContext,
        now: DateTime<Utc>,
    ) -> Result<NotificationAggregateStats, AppError>;

    async fn claim_client_notification_batch(
        &self,
        worker_id: &str,
        now: DateTime<Utc>,
        lease_secs: i64,
    ) -> Result<ClientNotificationClaim, AppError>;

    /// Returns false after atomically cancelling a batch whose offline episode,
    /// recipient authorization, or notification activation is no longer valid.
    async fn validate_client_notification_batch(
        &self,
        batch_id: &str,
        worker_id: &str,
        policy: &ClientNotificationPolicy,
        now: DateTime<Utc>,
    ) -> Result<bool, AppError>;

    async fn mark_client_notification_batch_sent(
        &self,
        batch_id: &str,
        worker_id: &str,
        provider_message_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AppError>;

    async fn mark_client_notification_batch_retry(
        &self,
        batch_id: &str,
        worker_id: &str,
        error: &str,
        next_attempt_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<(), AppError>;

    async fn mark_client_notification_batch_dead_letter(
        &self,
        batch_id: &str,
        worker_id: &str,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AppError>;

    async fn mark_client_notification_batch_blocked_config(
        &self,
        batch_id: &str,
        worker_id: &str,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AppError>;
}

#[async_trait]
impl ClientNotificationStore for AppStore {
    async fn reconcile_client_notification_events(
        &self,
        policy: &ClientNotificationPolicy,
        now: DateTime<Utc>,
    ) -> Result<NotificationReconcileStats, AppError> {
        AppStore::reconcile_client_notification_events(self, policy, now).await
    }

    async fn aggregate_client_notification_batches(
        &self,
        policy: &ClientNotificationPolicy,
        template: &NotificationTemplateContext,
        now: DateTime<Utc>,
    ) -> Result<NotificationAggregateStats, AppError> {
        AppStore::aggregate_client_notification_batches(self, policy, template, now).await
    }

    async fn claim_client_notification_batch(
        &self,
        worker_id: &str,
        now: DateTime<Utc>,
        lease_secs: i64,
    ) -> Result<ClientNotificationClaim, AppError> {
        AppStore::claim_client_notification_batch(self, worker_id, now, lease_secs).await
    }

    async fn validate_client_notification_batch(
        &self,
        batch_id: &str,
        worker_id: &str,
        policy: &ClientNotificationPolicy,
        now: DateTime<Utc>,
    ) -> Result<bool, AppError> {
        AppStore::validate_client_notification_batch(self, batch_id, worker_id, policy, now).await
    }

    async fn mark_client_notification_batch_sent(
        &self,
        batch_id: &str,
        worker_id: &str,
        provider_message_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        AppStore::mark_client_notification_batch_sent(
            self,
            batch_id,
            worker_id,
            provider_message_id,
            now,
        )
        .await
    }

    async fn mark_client_notification_batch_retry(
        &self,
        batch_id: &str,
        worker_id: &str,
        error: &str,
        next_attempt_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        AppStore::mark_client_notification_batch_retry(
            self,
            batch_id,
            worker_id,
            error,
            next_attempt_at,
            now,
        )
        .await
    }

    async fn mark_client_notification_batch_dead_letter(
        &self,
        batch_id: &str,
        worker_id: &str,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        AppStore::mark_client_notification_batch_dead_letter(self, batch_id, worker_id, error, now)
            .await
    }

    async fn mark_client_notification_batch_blocked_config(
        &self,
        batch_id: &str,
        worker_id: &str,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        AppStore::mark_client_notification_batch_blocked_config(
            self, batch_id, worker_id, reason, now,
        )
        .await
    }
}

pub async fn run_client_notification_service(
    store: AppStore,
    dynamic: Arc<RwLock<DynamicSettings>>,
    config: Config,
) -> anyhow::Result<()> {
    let http = reqwest::Client::builder()
        .user_agent("cc-switch-router/0.1 client-notifications")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()?;
    let worker_id = format!("router-{}", Uuid::new_v4());
    let template = NotificationTemplateContext::from_config(&config);
    let mut interval = tokio::time::interval(Duration::from_secs(DELIVERY_INTERVAL_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_policy_warning: Option<String> = None;
    let mut tick_index = 0_u64;

    loop {
        interval.tick().await;
        let settings = dynamic.read().await.client_notifications.clone();
        let (policy, policy_warning) = ClientNotificationPolicy::for_runtime(&settings, &config);
        if policy_warning != last_policy_warning {
            if let Some(message) = policy_warning.as_deref() {
                warn!("{message}");
            }
            last_policy_warning = policy_warning;
        }
        if let Err(error) = run_notification_cycle(
            &store,
            &policy,
            &template,
            config.resend_api_key.as_deref(),
            &http,
            &worker_id,
            presence_reconcile_due(tick_index),
        )
        .await
        {
            warn!(error = %error, "client notification cycle failed");
        }
        tick_index = tick_index.wrapping_add(1);
    }
}

async fn run_notification_cycle<S: ClientNotificationStore>(
    store: &S,
    policy: &ClientNotificationPolicy,
    template: &NotificationTemplateContext,
    resend_api_key: Option<&str>,
    http: &reqwest::Client,
    worker_id: &str,
    reconcile_presence: bool,
) -> Result<(), AppError> {
    let now = Utc::now();
    if reconcile_presence {
        let reconciled = store
            .reconcile_client_notification_events(policy, now)
            .await?;
        if reconciled.has_activity() {
            info!(
                enabled = policy.enabled,
                baselined = reconciled.baselined,
                offline_events = reconciled.offline_events,
                recovered = reconciled.recovered,
                suppressed_disabled = reconciled.suppressed_disabled,
                suppressed_recipient_removed = reconciled.suppressed_recipient_removed,
                "client notification presence reconciled"
            );
        }
    }

    // Scheduled reconciliation still runs while disabled so the persistent
    // baseline advances and disabled-period events can never be replayed.
    if !policy.enabled {
        return Ok(());
    }

    let aggregated = store
        .aggregate_client_notification_batches(policy, template, now)
        .await?;
    if aggregated.has_activity() {
        info!(
            batches_created = aggregated.batches_created,
            events_batched = aggregated.events_batched,
            incident_digests = aggregated.incident_digests,
            recipient_cap_deferred = aggregated.deferred_by_recipient_cap,
            global_cap_deferred = aggregated.deferred_by_global_cap,
            "client notification events aggregated"
        );
    }

    for _ in 0..MAX_BATCHES_PER_CYCLE {
        let claim_now = Utc::now();
        let batch = match store
            .claim_client_notification_batch(worker_id, claim_now, CLAIM_LEASE_SECS)
            .await?
        {
            ClientNotificationClaim::Batch(batch) => batch,
            ClientNotificationClaim::SuppressedByRateLimit => continue,
            ClientNotificationClaim::Empty => break,
        };

        if !store
            .validate_client_notification_batch(&batch.id, worker_id, policy, Utc::now())
            .await?
        {
            info!(batch_id = %batch.id, "cancelled stale client notification batch");
            continue;
        }

        let Some(api_key) = resend_api_key.filter(|key| !key.trim().is_empty()) else {
            store
                .mark_client_notification_batch_blocked_config(
                    &batch.id,
                    worker_id,
                    "resend API key is not configured",
                    Utc::now(),
                )
                .await?;
            warn!(batch_id = %batch.id, "client notification blocked by missing Resend API key");
            continue;
        };
        if !valid_frozen_batch(&batch) {
            store
                .mark_client_notification_batch_blocked_config(
                    &batch.id,
                    worker_id,
                    "frozen email envelope is invalid",
                    Utc::now(),
                )
                .await?;
            warn!(batch_id = %batch.id, "client notification blocked by invalid email envelope");
            continue;
        }

        match send_resend_email(http, api_key, &batch).await {
            Ok(provider_message_id) => {
                store
                    .mark_client_notification_batch_sent(
                        &batch.id,
                        worker_id,
                        &provider_message_id,
                        Utc::now(),
                    )
                    .await?;
                info!(batch_id = %batch.id, provider_message_id, "client notification sent");
            }
            Err(failure) => {
                record_delivery_failure(store, &batch, worker_id, failure).await?;
            }
        }
    }
    Ok(())
}

async fn record_delivery_failure<S: ClientNotificationStore>(
    store: &S,
    batch: &ClientNotificationBatch,
    worker_id: &str,
    failure: DeliveryFailure,
) -> Result<(), AppError> {
    let error = sanitize_delivery_error(&failure.message);
    if failure.retryable && batch.attempts < MAX_DELIVERY_ATTEMPTS {
        let next_attempt_at = failure.retry_at.unwrap_or_else(|| {
            Utc::now() + chrono::Duration::seconds(retry_delay_secs(batch.attempts, &batch.id))
        });
        store
            .mark_client_notification_batch_retry(
                &batch.id,
                worker_id,
                &error,
                next_attempt_at,
                Utc::now(),
            )
            .await?;
        warn!(
            batch_id = %batch.id,
            attempts = batch.attempts,
            next_attempt_at = %next_attempt_at,
            error,
            "client notification delivery deferred"
        );
    } else {
        store
            .mark_client_notification_batch_dead_letter(&batch.id, worker_id, &error, Utc::now())
            .await?;
        warn!(
            batch_id = %batch.id,
            attempts = batch.attempts,
            error,
            "client notification moved to dead letter"
        );
    }
    Ok(())
}

fn presence_reconcile_due(tick_index: u64) -> bool {
    let ticks_per_reconcile = (PRESENCE_RECONCILE_INTERVAL_SECS / DELIVERY_INTERVAL_SECS).max(1);
    tick_index % ticks_per_reconcile == 0
}

#[derive(Serialize)]
struct ResendEmailRequest<'a> {
    from: &'a str,
    to: [&'a str; 1],
    subject: &'a str,
    html: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to: Option<&'a str>,
}

#[derive(Debug)]
struct DeliveryFailure {
    retryable: bool,
    retry_at: Option<DateTime<Utc>>,
    message: String,
}

async fn send_resend_email(
    http: &reqwest::Client,
    api_key: &str,
    batch: &ClientNotificationBatch,
) -> Result<String, DeliveryFailure> {
    send_resend_email_to(http, api_key, batch, RESEND_EMAILS_ENDPOINT).await
}

async fn send_resend_email_to(
    http: &reqwest::Client,
    api_key: &str,
    batch: &ClientNotificationBatch,
    endpoint: &str,
) -> Result<String, DeliveryFailure> {
    let response = http
        .post(endpoint)
        .bearer_auth(api_key)
        .header("Idempotency-Key", &batch.idempotency_key)
        .json(&ResendEmailRequest {
            from: &batch.from,
            to: [&batch.recipient],
            subject: &batch.subject,
            html: &batch.html,
            reply_to: batch.reply_to.as_deref(),
        })
        .send()
        .await
        .map_err(|error| DeliveryFailure {
            retryable: error.is_timeout() || error.is_connect() || error.is_request(),
            retry_at: None,
            message: format!("Resend request failed: {error}"),
        })?;

    let status = response.status();
    let retry_at = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| parse_retry_after(value, Utc::now()));
    let body = response.text().await.map_err(|error| DeliveryFailure {
        // A successful response may already represent an accepted email. The
        // fixed idempotency key makes this retry safe.
        retryable: true,
        retry_at: None,
        message: format!("read Resend response failed after HTTP {status}: {error}"),
    })?;

    if status.is_success() {
        let value: serde_json::Value =
            serde_json::from_str(&body).map_err(|error| DeliveryFailure {
                retryable: true,
                retry_at: None,
                message: format!("parse successful Resend response failed: {error}"),
            })?;
        return value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .map(str::to_string)
            .ok_or_else(|| DeliveryFailure {
                retryable: true,
                retry_at: None,
                message: "successful Resend response omitted message id".to_string(),
            });
    }

    let error_name = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|value| {
            value
                .get("name")
                .and_then(|name| name.as_str())
                .map(str::to_string)
        })
        .map(|name| sanitize_delivery_error(&name));
    let retryable = status.as_u16() == 408
        || status.as_u16() == 425
        || status.as_u16() == 429
        || status.is_server_error()
        || (status.as_u16() == 409
            && error_name.as_deref() == Some("concurrent_idempotent_requests"));
    let body = sanitize_delivery_error(&body);
    let message = format!(
        "Resend returned HTTP {}{}: {}",
        status.as_u16(),
        error_name
            .as_deref()
            .map(|name| format!(" ({name})"))
            .unwrap_or_default(),
        body
    );
    Err(DeliveryFailure {
        retryable,
        retry_at,
        message: sanitize_delivery_error(&message),
    })
}

fn parse_retry_after(value: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    if let Ok(seconds) = value.trim().parse::<i64>() {
        return Some(now + chrono::Duration::seconds(seconds.clamp(1, 86_400)));
    }
    DateTime::parse_from_rfc2822(value.trim())
        .ok()
        .map(|value| value.with_timezone(&Utc))
        .filter(|value| *value > now)
        .map(|value| value.min(now + chrono::Duration::hours(24)))
}

fn retry_delay_secs(attempts: u32, stable_id: &str) -> i64 {
    let exponent = attempts.min(9);
    let base = 30_i64.saturating_mul(1_i64 << exponent).min(6 * 60 * 60);
    let jitter_seed = stable_id.bytes().fold(0_u64, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(u64::from(byte))
    });
    let jitter = (jitter_seed % ((base / 5).max(1) as u64)) as i64;
    base + jitter
}

fn valid_frozen_batch(batch: &ClientNotificationBatch) -> bool {
    is_basic_email(&batch.recipient)
        && is_valid_sender(&batch.from)
        && !batch.subject.trim().is_empty()
        && batch.subject.len() <= 200
        && !contains_header_controls(&batch.subject)
        && !batch.html.trim().is_empty()
        && batch.html.len() <= 256 * 1024
        && (1..=256).contains(&batch.idempotency_key.len())
        && !contains_header_controls(&batch.idempotency_key)
        && batch
            .reply_to
            .as_deref()
            .map(is_basic_email)
            .unwrap_or(true)
}

pub(crate) fn is_basic_email(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() || value.len() > 320 || !value.is_ascii() || contains_header_controls(value)
    {
        return false;
    }
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    if local.is_empty()
        || local.len() > 64
        || domain.is_empty()
        || domain.contains('@')
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || !local.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'/'
                        | b'='
                        | b'?'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'{'
                        | b'|'
                        | b'}'
                        | b'~'
                        | b'.'
                )
        })
    {
        return false;
    }
    let labels = domain.split('.').collect::<Vec<_>>();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

pub fn mask_email_address(value: &str) -> String {
    let Some((local, domain)) = value.trim().split_once('@') else {
        return "***".to_string();
    };
    let visible = local.chars().next().unwrap_or('*');
    format!("{visible}***@{domain}")
}

pub fn mask_email_like_tokens(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut copied_until = 0;
    let mut search_from = 0;
    while let Some(relative_at) = value[search_from..].find('@') {
        let at = search_from + relative_at;
        let local_start = value[..at]
            .char_indices()
            .rev()
            .take_while(|(_, character)| is_email_local_character(*character))
            .map(|(index, _)| index)
            .last()
            .unwrap_or(at);
        let domain_start = at + 1;
        let mut domain_end = domain_start;
        for (offset, character) in value[domain_start..].char_indices() {
            if !is_email_domain_character(character) {
                break;
            }
            domain_end = domain_start + offset + character.len_utf8();
        }
        while domain_end > domain_start && value[..domain_end].ends_with('.') {
            domain_end -= 1;
        }
        let candidate = &value[local_start..domain_end];
        if local_start >= copied_until && is_basic_email(candidate) {
            output.push_str(&value[copied_until..local_start]);
            output.push_str(&mask_email_address(candidate));
            copied_until = domain_end;
            search_from = domain_end;
        } else {
            search_from = domain_start;
        }
    }
    output.push_str(&value[copied_until..]);
    output
}

fn is_email_local_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '.' | '_' | '%' | '+' | '-' | '\'')
}

fn is_email_domain_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '.' | '-')
}

fn is_valid_sender(value: &str) -> bool {
    let value = value.trim();
    if contains_header_controls(value) {
        return false;
    }
    if let Some((display_name, address)) = value.rsplit_once('<') {
        return !display_name.trim().is_empty()
            && address
                .strip_suffix('>')
                .map(str::trim)
                .map(is_basic_email)
                .unwrap_or(false);
    }
    is_basic_email(value)
}

fn contains_header_controls(value: &str) -> bool {
    value
        .chars()
        .any(|character| matches!(character, '\r' | '\n' | '\0'))
}

fn strip_header_controls(value: &str) -> String {
    value
        .chars()
        .filter(|character| !matches!(character, '\r' | '\n' | '\0'))
        .collect::<String>()
        .trim()
        .to_string()
}

fn notification_sender(config: &Config) -> Option<String> {
    let raw = strip_header_controls(config.resend_from.as_deref()?);
    if raw.is_empty() || raw.len() > 500 {
        return None;
    }
    if let Some((display_name, address)) = raw.rsplit_once('<') {
        let address = address.strip_suffix('>')?.trim();
        if display_name.trim().is_empty() || !is_basic_email(address) {
            return None;
        }
        return Some(raw);
    }
    if !is_basic_email(&raw) {
        return None;
    }
    let name = config
        .resend_from_name
        .as_deref()
        .map(strip_header_controls)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "TokenSwitch".to_string());
    let name: String = name
        .chars()
        .filter(|character| !matches!(character, '<' | '>' | '"'))
        .take(100)
        .collect();
    let name = if name.trim().is_empty() {
        "TokenSwitch"
    } else {
        name.trim()
    };
    Some(format!("{name} <{raw}>"))
}

fn delivery_config_fingerprint(
    api_key: Option<&str>,
    sender: Option<&str>,
    reply_to: Option<&str>,
) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(api_key.unwrap_or_default().as_bytes());
    digest.update([0]);
    digest.update(sender.unwrap_or_default().as_bytes());
    digest.update([0]);
    digest.update(reply_to.unwrap_or_default().as_bytes());
    hex::encode(digest.finalize())
}

fn truncate_error(value: &str) -> String {
    value.chars().take(1_000).collect()
}

fn sanitize_delivery_error(value: &str) -> String {
    truncate_error(&mask_email_like_tokens(value))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedNotificationEmail {
    pub subject: String,
    pub html: String,
}

#[derive(Debug, Clone)]
pub struct RegistrationEmailData {
    pub installation_id: String,
    pub platform: String,
    pub version: Option<String>,
    pub country_code: Option<String>,
    pub registered_at: String,
    pub dashboard_url: String,
}

#[derive(Debug, Clone)]
pub struct RegistrationOverflowEmailData {
    pub count: u64,
    pub window_start: String,
    pub window_end: String,
    pub dashboard_url: String,
}

#[derive(Debug, Clone)]
pub struct OfflineEmailData {
    pub installation_id: String,
    pub tunnel_subdomain: Option<String>,
    pub owner_email: Option<String>,
    pub version: Option<String>,
    pub last_authenticated_seen_at: String,
    pub offline_since: String,
    pub dashboard_url: String,
}

#[derive(Debug, Clone)]
pub struct DigestEmailData {
    pub event_label: String,
    pub incident: bool,
    pub clients: Vec<String>,
    pub occurred_at: String,
    pub dashboard_url: String,
}

pub fn render_registration_email(data: &RegistrationEmailData) -> RenderedNotificationEmail {
    let id = short_installation_id(&data.installation_id);
    let subject = format!("New client registered: {id}");
    let rows = vec![
        ("Client", id),
        ("Platform", display_value(Some(&data.platform))),
        ("Version", display_value(data.version.as_deref())),
        ("Country", display_value(data.country_code.as_deref())),
        ("Registered", display_value(Some(&data.registered_at))),
    ];
    RenderedNotificationEmail {
        subject,
        html: render_email_document(
            "New client registered",
            "A client completed authenticated registration with this router.",
            &rows,
            &data.dashboard_url,
        ),
    }
}

pub fn render_registration_overflow_email(
    data: &RegistrationOverflowEmailData,
) -> RenderedNotificationEmail {
    let subject = "Client registration summary".to_string();
    let rows = vec![
        ("Registrations", data.count.to_string()),
        ("Window start", display_value(Some(&data.window_start))),
        ("Window end", display_value(Some(&data.window_end))),
    ];
    RenderedNotificationEmail {
        subject,
        html: render_email_document(
            "Client registration notification summary",
            "Registration activity exceeded the configured delivery lane. This summary replaces the individual notifications that could not be sent.",
            &rows,
            &display_value(Some(&data.dashboard_url)),
        ),
    }
}

pub fn render_offline_email(data: &OfflineEmailData) -> RenderedNotificationEmail {
    let id = short_installation_id(&data.installation_id);
    let subject = format!("Client offline: {id}");
    let rows = vec![
        ("Client", id),
        ("Tunnel", display_value(data.tunnel_subdomain.as_deref())),
        ("Owner", display_value(data.owner_email.as_deref())),
        ("Version", display_value(data.version.as_deref())),
        (
            "Last authenticated heartbeat",
            display_value(Some(&data.last_authenticated_seen_at)),
        ),
        (
            "Offline confirmed",
            display_value(Some(&data.offline_since)),
        ),
    ];
    RenderedNotificationEmail {
        subject,
        html: render_email_document(
            "Client offline",
            "The router did not receive an authenticated heartbeat within the configured window.",
            &rows,
            &data.dashboard_url,
        ),
    }
}

pub fn render_digest_email(data: &DigestEmailData) -> RenderedNotificationEmail {
    let event_label = sanitize_subject(&data.event_label);
    let prefix = if data.incident {
        "Client incident"
    } else {
        "Client activity"
    };
    let subject = format!("{prefix}: {} {}", data.clients.len(), event_label)
        .chars()
        .take(150)
        .collect();
    let mut clients = data
        .clients
        .iter()
        .take(MAX_DIGEST_CLIENTS)
        .map(|id| format!("<li>{}</li>", html_escape(&short_installation_id(id))))
        .collect::<Vec<_>>()
        .join("");
    let remaining = data.clients.len().saturating_sub(MAX_DIGEST_CLIENTS);
    if remaining > 0 {
        clients.push_str(&format!("<li>+{remaining} more</li>"));
    }
    let html = format!(
        "<!doctype html><html><body style=\"font-family:Arial,sans-serif;color:#202124\"><h1 style=\"font-size:20px\">{}</h1><p>{} clients reported {} at {}.</p><ul>{}</ul><p><a href=\"{}\">Open router dashboard</a></p></body></html>",
        html_escape(prefix),
        data.clients.len(),
        html_escape(&event_label),
        html_escape(&data.occurred_at),
        clients,
        html_escape(&data.dashboard_url),
    );
    RenderedNotificationEmail { subject, html }
}

fn render_email_document(
    title: &str,
    introduction: &str,
    rows: &[(&str, String)],
    dashboard_url: &str,
) -> String {
    let rows = rows
        .iter()
        .map(|(label, value)| {
            format!(
                "<tr><th align=\"left\" style=\"padding:6px 12px 6px 0\">{}</th><td style=\"padding:6px 0\">{}</td></tr>",
                html_escape(label),
                html_escape(value)
            )
        })
        .collect::<Vec<_>>()
        .join("");
    format!(
        "<!doctype html><html><body style=\"font-family:Arial,sans-serif;color:#202124\"><h1 style=\"font-size:20px\">{}</h1><p>{}</p><table>{}</table><p><a href=\"{}\">Open router dashboard</a></p></body></html>",
        html_escape(title),
        html_escape(introduction),
        rows,
        html_escape(dashboard_url),
    )
}

fn short_installation_id(value: &str) -> String {
    value.chars().take(8).collect()
}

fn display_value(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Unknown")
        .chars()
        .take(200)
        .collect()
}

fn sanitize_subject(value: &str) -> String {
    strip_header_controls(value).chars().take(80).collect()
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::Router;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::routing::post;
    use chrono::TimeZone;
    use tokio::sync::Mutex;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct CapturedRequest {
        idempotency_key: String,
        body: Vec<u8>,
    }

    #[derive(Clone)]
    struct MockResendState {
        status: StatusCode,
        body: String,
        retry_after: Option<String>,
        requests: Arc<Mutex<Vec<CapturedRequest>>>,
    }

    #[derive(Clone, Default)]
    struct CycleStore {
        claims: Arc<Mutex<VecDeque<ClientNotificationClaim>>>,
        validations: Arc<Mutex<VecDeque<bool>>>,
        blocked_batches: Arc<Mutex<Vec<String>>>,
        retry_errors: Arc<Mutex<Vec<String>>>,
        dead_letter_errors: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl ClientNotificationStore for CycleStore {
        async fn reconcile_client_notification_events(
            &self,
            _policy: &ClientNotificationPolicy,
            _now: DateTime<Utc>,
        ) -> Result<NotificationReconcileStats, AppError> {
            Ok(NotificationReconcileStats::default())
        }

        async fn aggregate_client_notification_batches(
            &self,
            _policy: &ClientNotificationPolicy,
            _template: &NotificationTemplateContext,
            _now: DateTime<Utc>,
        ) -> Result<NotificationAggregateStats, AppError> {
            Ok(NotificationAggregateStats::default())
        }

        async fn claim_client_notification_batch(
            &self,
            _worker_id: &str,
            _now: DateTime<Utc>,
            _lease_secs: i64,
        ) -> Result<ClientNotificationClaim, AppError> {
            Ok(self
                .claims
                .lock()
                .await
                .pop_front()
                .unwrap_or(ClientNotificationClaim::Empty))
        }

        async fn validate_client_notification_batch(
            &self,
            _batch_id: &str,
            _worker_id: &str,
            _policy: &ClientNotificationPolicy,
            _now: DateTime<Utc>,
        ) -> Result<bool, AppError> {
            Ok(self.validations.lock().await.pop_front().unwrap_or(true))
        }

        async fn mark_client_notification_batch_sent(
            &self,
            _batch_id: &str,
            _worker_id: &str,
            _provider_message_id: &str,
            _now: DateTime<Utc>,
        ) -> Result<(), AppError> {
            Ok(())
        }

        async fn mark_client_notification_batch_retry(
            &self,
            _batch_id: &str,
            _worker_id: &str,
            error: &str,
            _next_attempt_at: DateTime<Utc>,
            _now: DateTime<Utc>,
        ) -> Result<(), AppError> {
            self.retry_errors.lock().await.push(error.to_string());
            Ok(())
        }

        async fn mark_client_notification_batch_dead_letter(
            &self,
            _batch_id: &str,
            _worker_id: &str,
            error: &str,
            _now: DateTime<Utc>,
        ) -> Result<(), AppError> {
            self.dead_letter_errors.lock().await.push(error.to_string());
            Ok(())
        }

        async fn mark_client_notification_batch_blocked_config(
            &self,
            batch_id: &str,
            _worker_id: &str,
            _reason: &str,
            _now: DateTime<Utc>,
        ) -> Result<(), AppError> {
            self.blocked_batches.lock().await.push(batch_id.to_string());
            Ok(())
        }
    }

    async fn mock_resend_handler(
        State(state): State<MockResendState>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        state.requests.lock().await.push(CapturedRequest {
            idempotency_key: headers
                .get("Idempotency-Key")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string(),
            body: body.to_vec(),
        });
        let mut response = (state.status, state.body).into_response();
        response
            .headers_mut()
            .insert("content-type", HeaderValue::from_static("application/json"));
        if let Some(value) = state.retry_after {
            response
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from_str(&value).unwrap());
        }
        response
    }

    async fn start_mock_resend(
        status: StatusCode,
        body: &str,
        retry_after: Option<&str>,
    ) -> (
        String,
        Arc<Mutex<Vec<CapturedRequest>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let state = MockResendState {
            status,
            body: body.to_string(),
            retry_after: retry_after.map(str::to_string),
            requests: requests.clone(),
        };
        let app = Router::new()
            .route("/emails", post(mock_resend_handler))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/emails"), requests, task)
    }

    fn test_batch() -> ClientNotificationBatch {
        ClientNotificationBatch {
            id: "b1".into(),
            recipient: "ops@example.com".into(),
            from: "Router <noreply@example.com>".into(),
            reply_to: Some("support@example.com".into()),
            subject: "Client offline".into(),
            html: "<p>offline</p>".into(),
            idempotency_key: "client-notification/b1".into(),
            attempts: 1,
        }
    }

    #[tokio::test]
    async fn notification_cycle_continues_after_cap_suppression_and_stale_batch() {
        let mut stale = test_batch();
        stale.id = "stale-batch".into();
        let mut valid = test_batch();
        valid.id = "valid-batch".into();
        let store = CycleStore {
            claims: Arc::new(Mutex::new(VecDeque::from([
                ClientNotificationClaim::SuppressedByRateLimit,
                ClientNotificationClaim::Batch(stale),
                ClientNotificationClaim::Batch(valid),
                ClientNotificationClaim::Empty,
            ]))),
            validations: Arc::new(Mutex::new(VecDeque::from([false, true]))),
            blocked_batches: Arc::new(Mutex::new(Vec::new())),
            retry_errors: Arc::new(Mutex::new(Vec::new())),
            dead_letter_errors: Arc::new(Mutex::new(Vec::new())),
        };
        let policy = ClientNotificationPolicy::from(&ClientNotificationSettings {
            enabled: true,
            alert_emails: vec!["ops@example.com".into()],
            ..ClientNotificationSettings::default()
        });
        let template = NotificationTemplateContext {
            dashboard_url: "https://router.example.com".into(),
            sender: Some("Router <router@example.com>".into()),
            reply_to: None,
            delivery_configured: false,
            delivery_config_fingerprint: "test".into(),
        };

        run_notification_cycle(
            &store,
            &policy,
            &template,
            None,
            &reqwest::Client::new(),
            "cycle-worker",
            false,
        )
        .await
        .expect("run notification cycle");

        assert_eq!(
            *store.blocked_batches.lock().await,
            vec!["valid-batch".to_string()]
        );
        assert!(store.claims.lock().await.is_empty());
    }

    #[tokio::test]
    async fn delivery_failure_errors_are_masked_before_persistence() {
        let store = CycleStore::default();
        let retry_batch = test_batch();
        record_delivery_failure(
            &store,
            &retry_batch,
            "cycle-worker",
            DeliveryFailure {
                retryable: true,
                retry_at: None,
                message: "recipient ops@example.com; sender noreply@example.com".into(),
            },
        )
        .await
        .unwrap();

        let mut dead_letter_batch = test_batch();
        dead_letter_batch.attempts = MAX_DELIVERY_ATTEMPTS;
        record_delivery_failure(
            &store,
            &dead_letter_batch,
            "cycle-worker",
            DeliveryFailure {
                retryable: true,
                retry_at: None,
                message: "contact audit@vendor.test about owner@example.com".into(),
            },
        )
        .await
        .unwrap();

        assert_eq!(
            *store.retry_errors.lock().await,
            vec!["recipient o***@example.com; sender n***@example.com"]
        );
        assert_eq!(
            *store.dead_letter_errors.lock().await,
            vec!["contact a***@vendor.test about o***@example.com"]
        );
    }

    #[test]
    fn policy_normalizes_unsafe_boot_values_and_recipients() {
        let settings = ClientNotificationSettings {
            enabled: true,
            alert_emails: vec![
                " Ops@Example.com ".into(),
                "ops@example.com".into(),
                "invalid".into(),
            ],
            offline_alert_secs: -1,
            recovery_stable_secs: i64::MAX,
            storm_percent: 101,
            registration_recipient_hourly_limit: i64::MAX,
            registration_global_hourly_limit: -1,
            ..ClientNotificationSettings::default()
        };
        let policy = ClientNotificationPolicy::from(&settings);
        assert_eq!(policy.alert_emails, vec!["ops@example.com"]);
        assert_eq!(policy.offline_alert_secs, MIN_OFFLINE_ALERT_SECS);
        assert_eq!(policy.recovery_stable_secs, 3_600);
        assert_eq!(policy.storm_percent, 100);
        assert_eq!(policy.registration_recipient_hourly_limit, 1_000);
        assert_eq!(policy.registration_global_hourly_limit, 1_000);

        let no_recipients = ClientNotificationPolicy::from(&ClientNotificationSettings {
            enabled: true,
            ..ClientNotificationSettings::default()
        });
        assert!(!no_recipients.enabled);
    }

    #[test]
    fn runtime_policy_preserves_normalized_registration_caps() {
        let mut config = Config::from_env();
        config.client_stale_secs = 3_600;
        config.cleanup_interval_secs = 300;
        let settings = ClientNotificationSettings {
            enabled: true,
            alert_emails: vec!["ops@example.com".into()],
            registration_recipient_hourly_limit: 7,
            registration_global_hourly_limit: 4,
            ..ClientNotificationSettings::default()
        };

        let (policy, warning) = ClientNotificationPolicy::for_runtime(&settings, &config);

        assert!(warning.is_none());
        assert!(policy.enabled);
        assert_eq!(policy.registration_recipient_hourly_limit, 7);
        assert_eq!(policy.registration_global_hourly_limit, 7);
    }

    #[test]
    fn retry_after_supports_delta_and_http_date() {
        let now = Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap();
        assert_eq!(
            parse_retry_after("120", now),
            Some(now + chrono::Duration::seconds(120))
        );
        assert_eq!(
            parse_retry_after("Wed, 15 Jul 2026 12:05:00 GMT", now),
            Some(now + chrono::Duration::minutes(5))
        );
    }

    #[test]
    fn retry_backoff_is_bounded_and_stable() {
        assert_eq!(
            retry_delay_secs(3, "batch-a"),
            retry_delay_secs(3, "batch-a")
        );
        assert!(retry_delay_secs(30, "batch-a") <= 6 * 60 * 60 + 72 * 60);
        assert!(retry_delay_secs(4, "batch-a") > retry_delay_secs(1, "batch-a"));
    }

    #[test]
    fn delivery_ticks_every_five_seconds_and_presence_every_twenty() {
        let due = (0..10)
            .filter(|tick| presence_reconcile_due(*tick))
            .collect::<Vec<_>>();
        assert_eq!(DELIVERY_INTERVAL_SECS, 5);
        assert_eq!(PRESENCE_RECONCILE_INTERVAL_SECS, 20);
        assert_eq!(due, vec![0, 4, 8]);
    }

    #[test]
    fn templates_escape_values_and_never_include_full_installation_id() {
        let full_id = "12345678-secret-rest";
        let email = render_offline_email(&OfflineEmailData {
            installation_id: full_id.into(),
            tunnel_subdomain: Some("client<script>".into()),
            owner_email: Some("owner@example.com".into()),
            version: Some("1.2.3".into()),
            last_authenticated_seen_at: "2026-07-15T12:00:00Z".into(),
            offline_since: "2026-07-15T12:03:00Z".into(),
            dashboard_url: "https://router.example.com".into(),
        });
        assert!(email.html.contains("12345678"));
        assert!(!email.html.contains(full_id));
        assert!(!email.html.contains("<script>"));
        assert!(email.html.contains("client&lt;script&gt;"));
    }

    #[test]
    fn digest_template_bounds_client_list_and_reports_remainder() {
        let email = render_digest_email(&DigestEmailData {
            event_label: "offline clients".into(),
            incident: true,
            clients: (0..55).map(|index| format!("client-{index:04}")).collect(),
            occurred_at: "2026-07-15T12:00:00Z".into(),
            dashboard_url: "https://router.example.com".into(),
        });
        assert_eq!(email.html.matches("<li>").count(), MAX_DIGEST_CLIENTS + 1);
        assert!(email.html.contains("+5 more"));
        assert!(email.subject.chars().count() <= 150);
    }

    #[test]
    fn registration_overflow_template_is_fixed_bounded_and_escaped() {
        let untrusted = format!("<script>{}</script>", "x".repeat(500));
        let email = render_registration_overflow_email(&RegistrationOverflowEmailData {
            count: u64::MAX,
            window_start: untrusted.clone(),
            window_end: untrusted.clone(),
            dashboard_url: format!("https://router.example.com/?next={untrusted}"),
        });
        assert_eq!(email.subject, "Client registration summary");
        assert!(!email.html.contains("<script>"));
        assert!(email.html.contains("&lt;script&gt;"));
        assert!(email.html.len() < 3_000);
    }

    #[test]
    fn frozen_batch_rejects_header_injection() {
        let mut batch = test_batch();
        assert!(valid_frozen_batch(&batch));
        batch.subject = "safe\r\nBcc: victim@example.com".into();
        assert!(!valid_frozen_batch(&batch));
    }

    #[test]
    fn basic_email_validation_rejects_addresses_resend_cannot_deliver() {
        assert!(is_basic_email("alerts+router@example.com"));
        assert!(!is_basic_email("ops@@example.com"));
        assert!(!is_basic_email("ops @example.com"));
        assert!(!is_basic_email("ops@example..com"));
        assert!(!is_basic_email("ops@-example.com"));
        assert!(!is_basic_email("ops@example"));
    }

    #[test]
    fn delivery_view_masks_recipient_local_part() {
        assert_eq!(
            mask_email_address("operations@example.com"),
            "o***@example.com"
        );
        assert_eq!(mask_email_address("invalid"), "***");
    }

    #[test]
    fn error_text_masks_every_email_like_token_and_preserves_punctuation() {
        assert_eq!(
            mask_email_like_tokens(
                "Recipient Alice.Example@Example.COM, sender alerts+router@mail.example.net."
            ),
            "Recipient A***@Example.COM, sender a***@mail.example.net."
        );
        assert_eq!(
            mask_email_like_tokens("not-an-email@example and bare @ token"),
            "not-an-email@example and bare @ token"
        );
    }

    #[tokio::test]
    async fn resend_success_returns_provider_message_id() {
        let (endpoint, _requests, server) =
            start_mock_resend(StatusCode::OK, r#"{"id":"email_123"}"#, None).await;
        let result =
            send_resend_email_to(&reqwest::Client::new(), "re_test", &test_batch(), &endpoint)
                .await
                .unwrap();
        server.abort();
        assert_eq!(result, "email_123");
    }

    #[tokio::test]
    async fn resend_429_honors_retry_after() {
        let (endpoint, _requests, server) = start_mock_resend(
            StatusCode::TOO_MANY_REQUESTS,
            r#"{"name":"rate_limit_exceeded","message":"slow down"}"#,
            Some("120"),
        )
        .await;
        let before = Utc::now();
        let failure =
            send_resend_email_to(&reqwest::Client::new(), "re_test", &test_batch(), &endpoint)
                .await
                .unwrap_err();
        server.abort();
        assert!(failure.retryable);
        let retry_at = failure.retry_at.expect("Retry-After parsed");
        assert!(retry_at >= before + chrono::Duration::seconds(120));
        assert!(retry_at <= Utc::now() + chrono::Duration::seconds(121));
    }

    #[tokio::test]
    async fn resend_500_is_retryable_and_400_is_permanent() {
        let (endpoint_500, _requests, server_500) = start_mock_resend(
            StatusCode::INTERNAL_SERVER_ERROR,
            r#"{"name":"internal_server_error"}"#,
            None,
        )
        .await;
        let failure_500 = send_resend_email_to(
            &reqwest::Client::new(),
            "re_test",
            &test_batch(),
            &endpoint_500,
        )
        .await
        .unwrap_err();
        server_500.abort();
        assert!(failure_500.retryable);

        let (endpoint_400, _requests, server_400) = start_mock_resend(
            StatusCode::BAD_REQUEST,
            r#"{"name":"validation_error"}"#,
            None,
        )
        .await;
        let failure_400 = send_resend_email_to(
            &reqwest::Client::new(),
            "re_test",
            &test_batch(),
            &endpoint_400,
        )
        .await
        .unwrap_err();
        server_400.abort();
        assert!(!failure_400.retryable);
    }

    #[tokio::test]
    async fn resend_error_response_masks_all_email_addresses() {
        let body = r#"{"name":"validation_ops@example.com","message":"recipient ops@example.com rejected sender noreply@example.com; contact audit@vendor.test"}"#;
        let (endpoint, _requests, server) =
            start_mock_resend(StatusCode::BAD_REQUEST, body, None).await;
        let failure =
            send_resend_email_to(&reqwest::Client::new(), "re_test", &test_batch(), &endpoint)
                .await
                .unwrap_err();
        server.abort();

        for address in [
            "validation_ops@example.com",
            "ops@example.com",
            "noreply@example.com",
            "audit@vendor.test",
        ] {
            assert!(!failure.message.contains(address));
        }
        for masked in [
            "v***@example.com",
            "o***@example.com",
            "n***@example.com",
            "a***@vendor.test",
        ] {
            assert!(failure.message.contains(masked));
        }
    }

    #[tokio::test]
    async fn resend_retries_reuse_identical_idempotency_key_and_payload() {
        let (endpoint, requests, server) = start_mock_resend(
            StatusCode::INTERNAL_SERVER_ERROR,
            r#"{"name":"internal_server_error"}"#,
            None,
        )
        .await;
        let client = reqwest::Client::new();
        let batch = test_batch();
        for _ in 0..2 {
            let failure = send_resend_email_to(&client, "re_test", &batch, &endpoint)
                .await
                .unwrap_err();
            assert!(failure.retryable);
        }
        server.abort();
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0], requests[1]);
        assert_eq!(requests[0].idempotency_key, batch.idempotency_key);
    }
}
