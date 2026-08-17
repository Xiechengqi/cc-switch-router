use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::header::RETRY_AFTER;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::{ClientNotificationSettings, Config, TelegramBotSettings};
use crate::dynamic_settings::DynamicSettings;
use crate::error::AppError;
use crate::notification_channels::NotificationChannelId;
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
}

impl From<&ClientNotificationSettings> for ClientNotificationPolicy {
    fn from(settings: &ClientNotificationSettings) -> Self {
        let registration_recipient_hourly_limit =
            settings.registration_recipient_hourly_limit.clamp(1, 1_000);
        let registration_global_hourly_limit = settings
            .registration_global_hourly_limit
            .clamp(1, 10_000)
            .max(registration_recipient_hourly_limit);
        Self {
            enabled: settings.enabled,
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
        }
    }
}

impl ClientNotificationPolicy {
    pub fn for_runtime(
        settings: &ClientNotificationSettings,
        config: &Config,
    ) -> (Self, Option<String>) {
        let mut policy = Self::from(settings);
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

pub(crate) fn route_reconnect_grace(settings: &ClientNotificationSettings) -> Duration {
    Duration::from_secs(ClientNotificationPolicy::from(settings).offline_alert_secs as u64)
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
    /// Refreshed from dynamic settings on every delivery tick, so enabling or
    /// disabling the bot takes effect without a restart.
    pub telegram: TelegramNotificationContext,
}

/// Telegram half of the delivery configuration.
///
/// The caps are deliberately *not* the email caps: an email costs Resend
/// quota, a Telegram message does not, and the operator sizes them
/// independently. Attempts are counted per channel, so enabling Telegram does
/// not consume or delay an email delivery slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramNotificationContext {
    pub enabled: bool,
    pub recipient_hourly_limit: i64,
    pub global_hourly_limit: i64,
}

impl Default for TelegramNotificationContext {
    fn default() -> Self {
        Self::from_settings(&TelegramBotSettings::default())
    }
}

impl TelegramNotificationContext {
    pub fn from_settings(settings: &TelegramBotSettings) -> Self {
        Self {
            enabled: settings.is_operational(),
            recipient_hourly_limit: settings.recipient_hourly_limit.max(0),
            global_hourly_limit: settings.global_hourly_limit.max(0),
        }
    }
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
            telegram: TelegramNotificationContext::from_settings(&config.telegram_bot),
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

/// A fully frozen delivery request. Retried deliveries must reuse every field
/// and the idempotency key byte-for-byte.
///
/// `channel` decides how the frozen fields are read: `Email` uses the RFC
/// envelope (`from`/`reply_to`/`subject`/`html`), `Telegram` uses
/// `subject` + `text` as the message body and `channel_target` as the chat id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientNotificationBatch {
    pub id: String,
    pub attempt_id: String,
    pub recipient: String,
    pub recipient_user_id: Option<String>,
    pub channel: NotificationChannelId,
    pub channel_target: Option<String>,
    pub target_revision: i64,
    pub provider_identity: Option<String>,
    pub from: String,
    pub reply_to: Option<String>,
    pub subject: String,
    pub html: String,
    pub text: String,
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
    pub channel: String,
    pub delivery_kind: String,
    pub event_kind: String,
    pub event_count: u64,
    pub target_masked: String,
    pub status: String,
    pub failure_kind: Option<String>,
    pub blocked_reason_code: Option<String>,
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

    async fn start_client_notification_attempt(
        &self,
        batch_id: &str,
        attempt_id: &str,
        worker_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AppError>;

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
        failure_kind: &str,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AppError>;

    async fn mark_client_notification_batch_blocked_config(
        &self,
        batch_id: &str,
        worker_id: &str,
        reason_code: &str,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AppError>;

    async fn handle_unreachable_telegram_delivery(
        &self,
        batch_id: &str,
        worker_id: &str,
        chat_id: &str,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<String>, AppError>;
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

    async fn start_client_notification_attempt(
        &self,
        batch_id: &str,
        attempt_id: &str,
        worker_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        AppStore::start_client_notification_attempt(
            self, batch_id, attempt_id, worker_id, now,
        )
        .await
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
        failure_kind: &str,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        AppStore::mark_client_notification_batch_dead_letter(
            self,
            batch_id,
            worker_id,
            failure_kind,
            error,
            now,
        )
        .await
    }

    async fn mark_client_notification_batch_blocked_config(
        &self,
        batch_id: &str,
        worker_id: &str,
        reason_code: &str,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        AppStore::mark_client_notification_batch_blocked_config(
            self, batch_id, worker_id, reason_code, reason, now,
        )
        .await
    }

    async fn handle_unreachable_telegram_delivery(
        &self,
        batch_id: &str,
        worker_id: &str,
        chat_id: &str,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<String>, AppError> {
        AppStore::handle_unreachable_telegram_delivery(
            self, batch_id, worker_id, chat_id, error, now,
        )
        .await
    }
}

#[derive(Debug)]
enum TransportValidationFailure {
    BlockedConfig {
        reason_code: &'static str,
        message: &'static str,
    },
    InvalidPayload(&'static str),
}

#[derive(Debug)]
enum TransportSendFailure {
    Delivery(DeliveryFailure),
    EndpointUnreachable(String),
}

#[async_trait]
trait NotificationTransport: Send + Sync {
    fn channel(&self) -> &'static str;

    fn validate(
        &self,
        batch: &ClientNotificationBatch,
    ) -> Result<(), TransportValidationFailure>;

    async fn send(
        &self,
        batch: &ClientNotificationBatch,
    ) -> Result<String, TransportSendFailure>;
}

struct EmailNotificationTransport<'a> {
    http: &'a reqwest::Client,
    api_key: Option<&'a str>,
}

#[async_trait]
impl NotificationTransport for EmailNotificationTransport<'_> {
    fn channel(&self) -> &'static str {
        "email"
    }

    fn validate(
        &self,
        batch: &ClientNotificationBatch,
    ) -> Result<(), TransportValidationFailure> {
        if self.api_key.is_none_or(|key| key.trim().is_empty()) {
            return Err(TransportValidationFailure::BlockedConfig {
                reason_code: "email_provider_unavailable",
                message: "resend API key is not configured",
            });
        }
        if !valid_frozen_batch(batch) {
            return Err(TransportValidationFailure::InvalidPayload(
                "frozen email envelope is invalid",
            ));
        }
        Ok(())
    }

    async fn send(
        &self,
        batch: &ClientNotificationBatch,
    ) -> Result<String, TransportSendFailure> {
        send_resend_email(
            self.http,
            self.api_key.expect("validated email API key"),
            batch,
        )
        .await
        .map_err(TransportSendFailure::Delivery)
    }
}

struct TelegramNotificationTransport<'a> {
    http: &'a reqwest::Client,
    token: Option<&'a str>,
}

#[async_trait]
impl NotificationTransport for TelegramNotificationTransport<'_> {
    fn channel(&self) -> &'static str {
        "telegram"
    }

    fn validate(
        &self,
        batch: &ClientNotificationBatch,
    ) -> Result<(), TransportValidationFailure> {
        if self.token.is_none_or(|token| token.trim().is_empty()) {
            return Err(TransportValidationFailure::BlockedConfig {
                reason_code: "telegram_bot_unavailable",
                message: "telegram bot is not ready",
            });
        }
        if batch
            .channel_target
            .as_deref()
            .is_none_or(|chat_id| chat_id.trim().is_empty())
        {
            return Err(TransportValidationFailure::InvalidPayload(
                "telegram delivery has no chat id",
            ));
        }
        if !valid_telegram_batch(batch) {
            return Err(TransportValidationFailure::InvalidPayload(
                "frozen telegram message is invalid",
            ));
        }
        Ok(())
    }

    async fn send(
        &self,
        batch: &ClientNotificationBatch,
    ) -> Result<String, TransportSendFailure> {
        match crate::telegram::send_message(
            self.http,
            self.token.expect("validated Telegram token"),
            batch
                .channel_target
                .as_deref()
                .expect("validated Telegram chat id"),
            None,
            &batch.text,
        )
        .await
        {
            Ok(success) => Ok(success.provider_message_id.unwrap_or_default()),
            Err(failure) if failure.chat_unreachable => Err(
                TransportSendFailure::EndpointUnreachable(failure.message),
            ),
            Err(failure) => Err(TransportSendFailure::Delivery(
                telegram_delivery_failure(failure),
            )),
        }
    }
}

struct NotificationChannelRegistry<'a> {
    transports: HashMap<&'static str, Box<dyn NotificationTransport + 'a>>,
}

impl<'a> NotificationChannelRegistry<'a> {
    fn new(
        http: &'a reqwest::Client,
        resend_api_key: Option<&'a str>,
        telegram_bot_token: Option<&'a str>,
    ) -> Self {
        let transports: Vec<Box<dyn NotificationTransport + 'a>> = vec![
            Box::new(EmailNotificationTransport {
                http,
                api_key: resend_api_key,
            }),
            Box::new(TelegramNotificationTransport {
                http,
                token: telegram_bot_token,
            }),
        ];
        Self {
            transports: transports
                .into_iter()
                .map(|transport| (transport.channel(), transport))
                .collect(),
        }
    }

    fn get(&self, channel: &NotificationChannelId) -> Option<&dyn NotificationTransport> {
        self.transports.get(channel.as_str()).map(Box::as_ref)
    }
}

pub async fn run_client_notification_service(
    store: AppStore,
    dynamic: Arc<RwLock<DynamicSettings>>,
    config: Config,
    presence_grace: Duration,
) -> anyhow::Result<()> {
    let http = reqwest::Client::builder()
        .user_agent("cc-switch-router/0.1 client-notifications")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()?;
    let worker_id = format!("router-{}", Uuid::new_v4());
    let mut template = NotificationTemplateContext::from_config(&config);
    let mut interval = tokio::time::interval(Duration::from_secs(DELIVERY_INTERVAL_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_policy_warning: Option<String> = None;
    let mut tick_index = 0_u64;
    let started_at = Instant::now();

    loop {
        interval.tick().await;
        let (settings, telegram_bot) = {
            let dynamic = dynamic.read().await;
            (
                dynamic.client_notifications.clone(),
                dynamic.telegram_bot.clone(),
            )
        };
        // Telegram is hot-reloadable; the email envelope is not, so only the
        // Telegram half of the template is refreshed per tick.
        template.telegram = TelegramNotificationContext::from_settings(&telegram_bot);
        let telegram_fingerprint = telegram_bot.token().map(|token| {
            crate::telegram::bind::telegram_config_fingerprint(
                token,
                telegram_bot.mode.as_str(),
                telegram_bot.webhook_secret.as_deref(),
            )
        });
        let telegram_ready = match store.telegram_bot_runtime().await {
            Ok(runtime) => {
                runtime.ready()
                    && telegram_fingerprint.as_deref()
                        == runtime.config_fingerprint.as_deref()
            }
            Err(error) => {
                warn!(error = %error, "read Telegram runtime readiness failed");
                false
            }
        };
        template.telegram.enabled &= telegram_ready;
        let telegram_token = telegram_ready.then(|| telegram_bot.token()).flatten();
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
            telegram_token,
            &http,
            &worker_id,
            should_reconcile_presence(tick_index, started_at.elapsed(), presence_grace),
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
    telegram_bot_token: Option<&str>,
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

    let registry =
        NotificationChannelRegistry::new(http, resend_api_key, telegram_bot_token);
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

        let Some(transport) = registry.get(&batch.channel) else {
            store
                .mark_client_notification_batch_blocked_config(
                    &batch.id,
                    worker_id,
                    "channel_unregistered",
                    "notification channel is not registered in this Router",
                    Utc::now(),
                )
                .await?;
            continue;
        };
        match transport.validate(&batch) {
            Ok(()) => {}
            Err(TransportValidationFailure::BlockedConfig {
                reason_code,
                message,
            }) => {
                store
                    .mark_client_notification_batch_blocked_config(
                        &batch.id,
                        worker_id,
                        reason_code,
                        message,
                        Utc::now(),
                    )
                    .await?;
                warn!(batch_id = %batch.id, channel = %batch.channel, message, "notification blocked by channel configuration");
                continue;
            }
            Err(TransportValidationFailure::InvalidPayload(message)) => {
                store
                    .mark_client_notification_batch_dead_letter(
                        &batch.id,
                        worker_id,
                        "invalid_payload",
                        message,
                        Utc::now(),
                    )
                    .await?;
                warn!(batch_id = %batch.id, channel = %batch.channel, message, "notification has an invalid frozen payload");
                continue;
            }
        }
        store
            .start_client_notification_attempt(
                &batch.id,
                &batch.attempt_id,
                worker_id,
                Utc::now(),
            )
            .await?;
        match transport.send(&batch).await {
            Ok(provider_message_id) => {
                store
                    .mark_client_notification_batch_sent(
                        &batch.id,
                        worker_id,
                        &provider_message_id,
                        Utc::now(),
                    )
                    .await?;
                info!(batch_id = %batch.id, channel = %batch.channel, provider_message_id, "client notification sent");
            }
            Err(TransportSendFailure::EndpointUnreachable(message)) => {
                let error = sanitize_delivery_error(&message);
                let chat_id = batch.channel_target.as_deref().unwrap_or_default();
                let unbound = store
                    .handle_unreachable_telegram_delivery(
                        &batch.id,
                        worker_id,
                        chat_id,
                        &error,
                        Utc::now(),
                    )
                    .await?;
                warn!(batch_id = %batch.id, unbound = unbound.is_some(), error, "notification endpoint invalidated after permanent failure");
            }
            Err(TransportSendFailure::Delivery(failure)) => {
                record_delivery_failure(store, &batch, worker_id, failure).await?;
            }
        }
    }
    Ok(())
}

/// Telegram reports `retry_after` as a wall-clock unix timestamp; the delivery
/// pipeline speaks `DateTime<Utc>`.
fn telegram_delivery_failure(failure: crate::telegram::TelegramFailure) -> DeliveryFailure {
    DeliveryFailure {
        retryable: failure.retryable,
        retry_at: failure
            .retry_at
            .and_then(|timestamp| DateTime::from_timestamp(timestamp, 0)),
        message: failure.message,
    }
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
            .mark_client_notification_batch_dead_letter(
                &batch.id,
                worker_id,
                "permanent_transport",
                &error,
                Utc::now(),
            )
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

fn should_reconcile_presence(tick_index: u64, elapsed: Duration, presence_grace: Duration) -> bool {
    presence_reconcile_due(tick_index) && elapsed >= presence_grace
}

#[derive(Serialize)]
struct ResendEmailRequest<'a> {
    from: &'a str,
    to: [&'a str; 1],
    subject: &'a str,
    html: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FrozenEmailEnvelope<'a> {
    pub from: &'a str,
    pub recipient: &'a str,
    pub subject: &'a str,
    pub html: &'a str,
    pub text: &'a str,
    pub reply_to: Option<&'a str>,
    pub idempotency_key: &'a str,
}

#[derive(Debug)]
pub(crate) struct DeliveryFailure {
    pub retryable: bool,
    pub retry_at: Option<DateTime<Utc>>,
    pub message: String,
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
    send_resend_frozen_email_to(
        http,
        api_key,
        FrozenEmailEnvelope {
            from: &batch.from,
            recipient: &batch.recipient,
            subject: &batch.subject,
            html: &batch.html,
            text: &batch.text,
            reply_to: batch.reply_to.as_deref(),
            idempotency_key: &batch.idempotency_key,
        },
        endpoint,
    )
    .await
}

pub(crate) async fn send_resend_frozen_email(
    http: &reqwest::Client,
    api_key: &str,
    envelope: FrozenEmailEnvelope<'_>,
) -> Result<String, DeliveryFailure> {
    send_resend_frozen_email_to(http, api_key, envelope, RESEND_EMAILS_ENDPOINT).await
}

async fn send_resend_frozen_email_to(
    http: &reqwest::Client,
    api_key: &str,
    envelope: FrozenEmailEnvelope<'_>,
    endpoint: &str,
) -> Result<String, DeliveryFailure> {
    let response = http
        .post(endpoint)
        .bearer_auth(api_key)
        .header("Idempotency-Key", envelope.idempotency_key)
        .json(&ResendEmailRequest {
            from: envelope.from,
            to: [envelope.recipient],
            subject: envelope.subject,
            html: envelope.html,
            text: (!envelope.text.trim().is_empty()).then_some(envelope.text),
            reply_to: envelope.reply_to,
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

pub(crate) fn retry_delay_secs(attempts: u32, stable_id: &str) -> i64 {
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
        && batch.text.len() <= 128 * 1024
        && (1..=256).contains(&batch.idempotency_key.len())
        && !contains_header_controls(&batch.idempotency_key)
        && batch
            .reply_to
            .as_deref()
            .map(is_basic_email)
            .unwrap_or(true)
}

/// The Telegram counterpart of [`valid_frozen_batch`]. There is no envelope to
/// validate — only that a message body exists, fits Telegram's limit, and is
/// addressed to a plausible chat id (Telegram chat ids are signed 64-bit
/// integers; anything else means the binding column was corrupted).
fn valid_telegram_batch(batch: &ClientNotificationBatch) -> bool {
    let chat_id_ok = batch
        .channel_target
        .as_deref()
        .map(str::trim)
        .is_some_and(|chat_id| {
            !chat_id.is_empty() && chat_id.len() <= 32 && chat_id.parse::<i64>().is_ok()
        });
    chat_id_ok
        && is_basic_email(&batch.recipient)
        && !batch.text.trim().is_empty()
        && batch.text.chars().count() <= crate::telegram::MAX_MESSAGE_CHARS
        && (1..=256).contains(&batch.idempotency_key.len())
        && !contains_header_controls(&batch.idempotency_key)
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

pub fn mask_notification_target(
    channel: &str,
    recipient: &str,
    channel_target: Option<&str>,
) -> String {
    if channel == crate::notification_channels::EMAIL_CHANNEL {
        return mask_email_address(recipient);
    }
    let target = channel_target.unwrap_or_default().trim();
    if target.is_empty() {
        return "***".into();
    }
    let suffix = target
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("***{suffix}")
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

pub(crate) fn sanitize_delivery_error(value: &str) -> String {
    truncate_error(&mask_email_like_tokens(value))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedNotificationEmail {
    pub subject: String,
    pub html: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct RegistrationEmailData {
    pub installation_id: String,
    pub platform: String,
    pub version: Option<String>,
    pub country_code: Option<String>,
    pub setup_completed_at: String,
    pub owner_email: Option<String>,
    pub client_url: Option<String>,
    pub password_hint: Option<String>,
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
    pub client_url: Option<String>,
    pub last_heartbeat_at: String,
    pub offline_since: String,
    pub dashboard_url: String,
}

#[derive(Debug, Clone)]
pub struct DigestEmailClient {
    pub installation_id: String,
    pub client_url: Option<String>,
    pub password_hint: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DigestEmailData {
    pub event_label: String,
    pub incident: bool,
    pub clients: Vec<DigestEmailClient>,
    pub occurred_at: String,
    pub dashboard_url: String,
}

pub fn render_registration_email(data: &RegistrationEmailData) -> RenderedNotificationEmail {
    let id = short_installation_id(&data.installation_id);
    let subject = format!("Client registered and ready: {id}");
    let rows = vec![
        ("Client", id),
        ("Client URL", display_value(data.client_url.as_deref())),
        (
            "Web password hint",
            display_value(data.password_hint.as_deref()),
        ),
        ("Owner", display_value(data.owner_email.as_deref())),
        ("Platform", display_value(Some(&data.platform))),
        ("Version", display_value(data.version.as_deref())),
        ("Country", display_value(data.country_code.as_deref())),
        (
            "Setup completed",
            display_value(Some(&data.setup_completed_at)),
        ),
    ];
    let action_url = data.client_url.as_deref().unwrap_or(&data.dashboard_url);
    let action_label = if data.client_url.is_some() {
        "Open Client Web"
    } else {
        "Open Router dashboard"
    };
    let (html, text) = render_email_document(EmailDocument {
        title: "Client registration completed",
        preheader: "A Client completed trusted registration and is ready to use.",
        introduction: "A Client owned by this email address completed trusted registration with the Router.",
        rows: &rows,
        action_label,
        action_url,
        dashboard_url: &data.dashboard_url,
        note: Some(
            "This is not the complete password. Use the full Web password configured during setup; the Router never receives or emails the complete password.",
        ),
        extra_html: "",
        extra_text: "",
    });
    RenderedNotificationEmail {
        subject,
        html,
        text,
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
    let (html, text) = render_email_document(EmailDocument {
        title: "Client registration summary",
        preheader: "Multiple Clients completed trusted registration.",
        introduction: "Registration volume exceeded the individual notification lane, so the Router combined the activity into this summary.",
        rows: &rows,
        action_label: "Review Clients",
        action_url: &data.dashboard_url,
        dashboard_url: &data.dashboard_url,
        note: None,
        extra_html: "",
        extra_text: "",
    });
    RenderedNotificationEmail {
        subject,
        html,
        text,
    }
}

pub fn render_offline_email(data: &OfflineEmailData) -> RenderedNotificationEmail {
    let id = short_installation_id(&data.installation_id);
    let subject = format!("Client offline: {id}");
    let rows = vec![
        ("Client", id),
        ("Client URL", display_value(data.client_url.as_deref())),
        ("Subdomain", display_value(data.tunnel_subdomain.as_deref())),
        ("Owner", display_value(data.owner_email.as_deref())),
        ("Version", display_value(data.version.as_deref())),
        (
            "Last authenticated heartbeat",
            display_value(Some(&data.last_heartbeat_at)),
        ),
        (
            "Offline confirmed",
            display_value(Some(&data.offline_since)),
        ),
    ];
    let (html, text) = render_email_document(EmailDocument {
        title: "Client offline",
        preheader: "The Router confirmed that a Client is offline.",
        introduction: "The Router stopped receiving authenticated heartbeats from this Client for the configured confirmation window.",
        rows: &rows,
        action_label: "Review Client status",
        action_url: &data.dashboard_url,
        dashboard_url: &data.dashboard_url,
        note: Some(
            "Check that the Client service is running and can reach the Router. A recovery notification is intentionally not sent; the dashboard reflects recovery after stable heartbeats resume.",
        ),
        extra_html: "",
        extra_text: "",
    });
    RenderedNotificationEmail {
        subject,
        html,
        text,
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
    let mut clients_html = data
        .clients
        .iter()
        .take(MAX_DIGEST_CLIENTS)
        .map(|client| {
            let id = html_escape(&short_installation_id(&client.installation_id));
            let client_url = client.client_url.as_deref().map_or_else(
                || "Not available".to_string(),
                |url| {
                    let escaped = html_escape(url);
                    format!(
                        "<a href=\"{escaped}\" style=\"color:#0f766e;word-break:break-all\">{escaped}</a>"
                    )
                },
            );
            let password_hint = client.password_hint.as_deref().map_or_else(String::new, |hint| {
                format!(
                    "<br><span style=\"color:#64748b\">Web password hint:</span> <code>{}</code>",
                    html_escape(hint)
                )
            });
            format!(
                "<li style=\"margin:0 0 14px\"><strong>{id}</strong><br><span style=\"color:#64748b\">Client URL:</span> {client_url}{password_hint}</li>"
            )
        })
        .collect::<Vec<_>>()
        .join("");
    let mut clients_text = data
        .clients
        .iter()
        .take(MAX_DIGEST_CLIENTS)
        .map(|client| {
            let url = client.client_url.as_deref().unwrap_or("Not available");
            let hint = client
                .password_hint
                .as_deref()
                .map_or_else(String::new, |hint| format!("\n  Web password hint: {hint}"));
            format!(
                "- {}\n  Client URL: {url}{hint}",
                short_installation_id(&client.installation_id)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let remaining = data.clients.len().saturating_sub(MAX_DIGEST_CLIENTS);
    if remaining > 0 {
        clients_html.push_str(&format!(
            "<li style=\"margin:0 0 6px\">+{remaining} more</li>"
        ));
        clients_text.push_str(&format!("\n- +{remaining} more"));
    }
    let rows = vec![
        ("Event", event_label.clone()),
        ("Clients", data.clients.len().to_string()),
        ("Observed", display_value(Some(&data.occurred_at))),
    ];
    let extra_html = format!(
        "<h2 style=\"font-size:16px;line-height:24px;margin:24px 0 8px;color:#172033\">Affected Clients</h2><ul style=\"margin:0;padding-left:20px;color:#334155\">{clients_html}</ul>"
    );
    let extra_text = format!("Affected Clients:\n{clients_text}");
    let introduction = format!(
        "The Router grouped {} related Client {} into one notification.",
        data.clients.len(),
        event_label
    );
    let includes_password_hints = data
        .clients
        .iter()
        .any(|client| client.password_hint.is_some());
    let (html, text) = render_email_document(EmailDocument {
        title: prefix,
        preheader: &format!(
            "{} Client {} require review.",
            data.clients.len(),
            event_label
        ),
        introduction: &introduction,
        rows: &rows,
        action_label: "Open Router dashboard",
        action_url: &data.dashboard_url,
        dashboard_url: &data.dashboard_url,
        note: includes_password_hints.then_some(
            "Each Web password hint is partial, not the complete password. Use the full password configured during that Client's setup.",
        ),
        extra_html: &extra_html,
        extra_text: &extra_text,
    });
    RenderedNotificationEmail {
        subject,
        html,
        text,
    }
}

struct EmailDocument<'a> {
    title: &'a str,
    preheader: &'a str,
    introduction: &'a str,
    rows: &'a [(&'a str, String)],
    action_label: &'a str,
    action_url: &'a str,
    dashboard_url: &'a str,
    note: Option<&'a str>,
    extra_html: &'a str,
    extra_text: &'a str,
}

fn render_email_document(document: EmailDocument<'_>) -> (String, String) {
    let rows_html = document
        .rows
        .iter()
        .map(|(label, value)| {
            format!(
                "<tr><th align=\"left\" valign=\"top\" style=\"width:150px;padding:10px 16px 10px 0;border-bottom:1px solid #e2e8f0;color:#64748b;font-size:13px;font-weight:600\">{}</th><td style=\"padding:10px 0;border-bottom:1px solid #e2e8f0;color:#172033;font-size:14px;word-break:break-word\">{}</td></tr>",
                html_escape(label),
                html_escape(value)
            )
        })
        .collect::<Vec<_>>()
        .join("");
    let note_html = document.note.map_or_else(String::new, |note| {
        format!(
            "<div style=\"margin-top:20px;padding:12px 14px;background:#f8fafc;border-left:3px solid #94a3b8;color:#475569;font-size:13px;line-height:20px\">{}</div>",
            html_escape(note)
        )
    });
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{title}</title></head><body style=\"margin:0;padding:0;background:#f1f5f9;font-family:Arial,sans-serif;color:#172033\"><div style=\"display:none;max-height:0;overflow:hidden;opacity:0\">{preheader}</div><table role=\"presentation\" width=\"100%\" cellspacing=\"0\" cellpadding=\"0\" style=\"background:#f1f5f9\"><tr><td align=\"center\" style=\"padding:24px 12px\"><table role=\"presentation\" width=\"100%\" cellspacing=\"0\" cellpadding=\"0\" style=\"max-width:620px;background:#ffffff;border:1px solid #dbe3ee\"><tr><td style=\"padding:20px 28px;border-bottom:1px solid #e2e8f0;font-size:14px;font-weight:700;color:#0f766e\">CC-Switch Router</td></tr><tr><td style=\"padding:28px\"><h1 style=\"margin:0 0 12px;font-size:24px;line-height:32px;color:#172033\">{title}</h1><p style=\"margin:0 0 20px;font-size:15px;line-height:24px;color:#475569\">{introduction}</p><table role=\"presentation\" width=\"100%\" cellspacing=\"0\" cellpadding=\"0\">{rows_html}</table>{extra_html}{note_html}<p style=\"margin:24px 0 0\"><a href=\"{action_url}\" style=\"display:inline-block;padding:11px 18px;background:#0f766e;color:#ffffff;text-decoration:none;font-size:14px;font-weight:700\">{action_label}</a></p><p style=\"margin:16px 0 0;font-size:12px;line-height:18px;color:#64748b\">Router dashboard: <a href=\"{dashboard_url}\" style=\"color:#0f766e\">{dashboard_url}</a></p></td></tr><tr><td style=\"padding:16px 28px;background:#f8fafc;color:#64748b;font-size:11px;line-height:17px\">This transactional email was sent because this address is the currently verified Owner of the affected Client.</td></tr></table></td></tr></table></body></html>",
        title = html_escape(document.title),
        preheader = html_escape(document.preheader),
        introduction = html_escape(document.introduction),
        rows_html = rows_html,
        extra_html = document.extra_html,
        note_html = note_html,
        action_url = html_escape(document.action_url),
        action_label = html_escape(document.action_label),
        dashboard_url = html_escape(document.dashboard_url),
    );
    let rows_text = document
        .rows
        .iter()
        .map(|(label, value)| format!("{label}: {value}"))
        .collect::<Vec<_>>()
        .join("\n");
    let note_text = document
        .note
        .map(|note| format!("\n\nSecurity note: {note}"))
        .unwrap_or_default();
    let extra_text = if document.extra_text.trim().is_empty() {
        String::new()
    } else {
        format!("\n\n{}", document.extra_text.trim())
    };
    let text = format!(
        "{EMAIL_TEXT_BRAND_LINE}\n\n{title}\n\n{introduction}\n\n{rows_text}{extra_text}{note_text}\n\n{action_label}: {action_url}\nRouter dashboard: {dashboard_url}\n\n{EMAIL_TEXT_FOOTER}",
        title = document.title,
        introduction = document.introduction,
        rows_text = rows_text,
        extra_text = extra_text,
        note_text = note_text,
        action_label = document.action_label,
        action_url = document.action_url,
        dashboard_url = document.dashboard_url,
    )
    .trim()
    .to_string();
    (html, text)
}

/// First line of every plain-text email body. Telegram strips it: the chat
/// already shows who is talking.
const EMAIL_TEXT_BRAND_LINE: &str = "CC-Switch Router";
/// Last line of every plain-text email body. It explains *email* addressing,
/// which is meaningless in a chat the user bound themselves.
const EMAIL_TEXT_FOOTER: &str = "This transactional email was sent because this address is the currently verified Owner of the affected Client.";

/// Render the Telegram counterpart of an already-rendered notification.
///
/// Deliberately derived from the email's plain-text alternative rather than
/// written from scratch: one notification must not be able to say two
/// different things depending on where it lands. What changes is the framing —
/// the brand header and the RFC footer come off, the subject becomes the title
/// line, and the footer is replaced by a link to the page where the user can
/// change or drop the channel (the chat's own opt-out path).
///
/// Telegram is asked for plain text (no `parse_mode`), so nothing here needs
/// escaping — a client hostname containing `_` or `*` stays literal.
pub fn render_telegram_message(
    rendered: &RenderedNotificationEmail,
    dashboard_url: &str,
) -> String {
    let mut body = String::new();
    for line in rendered.text.lines() {
        let line = line.trim_end();
        if line == EMAIL_TEXT_BRAND_LINE || line == EMAIL_TEXT_FOOTER {
            continue;
        }
        if line.is_empty() && body.ends_with("\n\n") {
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }
    let settings_url = format!(
        "{}/account/notifications",
        dashboard_url.trim_end_matches('/')
    );
    let message = format!(
        "{subject}\n\n{body}\n管理通知渠道 / Manage notifications: {settings_url}",
        subject = rendered.subject.trim(),
        body = body.trim(),
    );
    message
        .chars()
        .take(crate::telegram::MAX_MESSAGE_CHARS)
        .collect()
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
        unbound_chats: Arc<Mutex<Vec<String>>>,
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

        async fn start_client_notification_attempt(
            &self,
            _batch_id: &str,
            _attempt_id: &str,
            _worker_id: &str,
            _now: DateTime<Utc>,
        ) -> Result<(), AppError> {
            Ok(())
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
            _failure_kind: &str,
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
            _reason_code: &str,
            _reason: &str,
            _now: DateTime<Utc>,
        ) -> Result<(), AppError> {
            self.blocked_batches.lock().await.push(batch_id.to_string());
            Ok(())
        }

        async fn handle_unreachable_telegram_delivery(
            &self,
            _batch_id: &str,
            _worker_id: &str,
            chat_id: &str,
            _error: &str,
            _now: DateTime<Utc>,
        ) -> Result<Option<String>, AppError> {
            self.unbound_chats.lock().await.push(chat_id.to_string());
            Ok(Some("owner@example.com".into()))
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
            attempt_id: "attempt-b1".into(),
            recipient: "ops@example.com".into(),
            recipient_user_id: Some("user-1".into()),
            channel: NotificationChannelId::email(),
            channel_target: None,
            target_revision: 1,
            provider_identity: None,
            from: "Router <noreply@example.com>".into(),
            reply_to: Some("support@example.com".into()),
            subject: "Client offline".into(),
            html: "<p>offline</p>".into(),
            text: "offline".into(),
            idempotency_key: "client-notification/b1".into(),
            attempts: 1,
        }
    }

    fn test_telegram_batch() -> ClientNotificationBatch {
        ClientNotificationBatch {
            id: "t1".into(),
            attempt_id: "attempt-t1".into(),
            recipient: "ops@example.com".into(),
            recipient_user_id: Some("user-1".into()),
            channel: NotificationChannelId::telegram(),
            channel_target: Some("4242".into()),
            target_revision: 1,
            provider_identity: Some("7".into()),
            from: String::new(),
            reply_to: None,
            subject: "Client offline".into(),
            html: String::new(),
            text: "Client offline\n\nClient: abcdef12".into(),
            idempotency_key: "client-notification/t1:telegram".into(),
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
            unbound_chats: Arc::new(Mutex::new(Vec::new())),
        };
        let policy = ClientNotificationPolicy::from(&ClientNotificationSettings {
            enabled: true,
            ..ClientNotificationSettings::default()
        });
        let template = NotificationTemplateContext {
            dashboard_url: "https://router.example.com".into(),
            sender: Some("Router <router@example.com>".into()),
            reply_to: None,
            delivery_configured: false,
            delivery_config_fingerprint: "test".into(),
            telegram: TelegramNotificationContext::default(),
        };

        run_notification_cycle(
            &store,
            &policy,
            &template,
            None,
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

    /// A Telegram row must not be handed to the Resend path, and must not be
    /// sent at all while the bot is unconfigured — a message with no token is
    /// a configuration fault, not something to retry twelve times.
    #[tokio::test]
    async fn malformed_telegram_batches_are_dead_lettered_without_unbinding() {
        let mut no_chat = test_telegram_batch();
        no_chat.id = "no-chat".into();
        no_chat.channel_target = Some("   ".into());
        let mut bad_chat = test_telegram_batch();
        bad_chat.id = "bad-chat".into();
        bad_chat.channel_target = Some("not-a-chat-id".into());

        let store = CycleStore {
            claims: Arc::new(Mutex::new(VecDeque::from([
                ClientNotificationClaim::Batch(no_chat),
                ClientNotificationClaim::Batch(bad_chat),
                ClientNotificationClaim::Empty,
            ]))),
            ..CycleStore::default()
        };
        let policy = ClientNotificationPolicy::from(&ClientNotificationSettings {
            enabled: true,
            ..ClientNotificationSettings::default()
        });
        let template = NotificationTemplateContext {
            dashboard_url: "https://router.example.com".into(),
            sender: Some("Router <router@example.com>".into()),
            reply_to: None,
            delivery_configured: true,
            delivery_config_fingerprint: "test".into(),
            telegram: TelegramNotificationContext::default(),
        };

        run_notification_cycle(
            &store,
            &policy,
            &template,
            Some("resend-key"),
            Some("bot-token"),
            &reqwest::Client::new(),
            "cycle-worker",
            false,
        )
        .await
        .expect("run notification cycle");

        assert_eq!(store.dead_letter_errors.lock().await.len(), 2);
        assert!(store.blocked_batches.lock().await.is_empty());
        assert!(store.retry_errors.lock().await.is_empty());
        assert!(
            store.unbound_chats.lock().await.is_empty(),
            "a malformed row must not cost the user their binding"
        );
    }

    /// The Resend key and the bot token are independent: one channel being
    /// unconfigured must not block the other, and neither may borrow the
    /// other's credential.
    #[tokio::test]
    async fn a_missing_bot_token_blocks_only_telegram_rows() {
        let store = CycleStore {
            claims: Arc::new(Mutex::new(VecDeque::from([
                ClientNotificationClaim::Batch(test_telegram_batch()),
                ClientNotificationClaim::Empty,
            ]))),
            ..CycleStore::default()
        };
        let policy = ClientNotificationPolicy::from(&ClientNotificationSettings {
            enabled: true,
            ..ClientNotificationSettings::default()
        });
        let template = NotificationTemplateContext {
            dashboard_url: "https://router.example.com".into(),
            sender: Some("Router <router@example.com>".into()),
            reply_to: None,
            delivery_configured: true,
            delivery_config_fingerprint: "test".into(),
            telegram: TelegramNotificationContext::default(),
        };

        run_notification_cycle(
            &store,
            &policy,
            &template,
            Some("resend-key"),
            None,
            &reqwest::Client::new(),
            "cycle-worker",
            false,
        )
        .await
        .expect("run notification cycle");

        assert_eq!(*store.blocked_batches.lock().await, vec!["t1".to_string()]);
    }

    #[test]
    fn telegram_envelope_validation_matches_the_bot_api_limits() {
        assert!(valid_telegram_batch(&test_telegram_batch()));

        let mut oversized = test_telegram_batch();
        oversized.text = "字".repeat(crate::telegram::MAX_MESSAGE_CHARS + 1);
        assert!(
            !valid_telegram_batch(&oversized),
            "length is counted in chars, not bytes"
        );
        let mut at_limit = test_telegram_batch();
        at_limit.text = "字".repeat(crate::telegram::MAX_MESSAGE_CHARS);
        assert!(valid_telegram_batch(&at_limit));

        for chat_id in ["", "   ", "abc", "12.5", &"9".repeat(33)] {
            let mut batch = test_telegram_batch();
            batch.channel_target = Some(chat_id.to_string());
            assert!(!valid_telegram_batch(&batch), "chat id {chat_id:?}");
        }
        // Group and channel chats are negative; supergroups are long.
        for chat_id in ["4242", "-1001234567890"] {
            let mut batch = test_telegram_batch();
            batch.channel_target = Some(chat_id.to_string());
            assert!(valid_telegram_batch(&batch), "chat id {chat_id:?}");
        }

        let mut empty_text = test_telegram_batch();
        empty_text.text = "  \n ".into();
        assert!(!valid_telegram_batch(&empty_text));

        // The recipient stays the owner email even on Telegram rows: it is
        // what the storm and ownership queries key on.
        let mut bad_recipient = test_telegram_batch();
        bad_recipient.recipient = "not-an-email".into();
        assert!(!valid_telegram_batch(&bad_recipient));
    }

    #[test]
    fn telegram_delivery_failures_keep_their_retry_classification() {
        let retry_at = Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap();
        let mapped = telegram_delivery_failure(crate::telegram::TelegramFailure {
            retryable: true,
            retry_at: Some(retry_at.timestamp()),
            http_status: Some(429),
            message: "Telegram sendMessage returned HTTP 429".into(),
            chat_unreachable: false,
        });
        assert!(mapped.retryable);
        assert_eq!(mapped.retry_at, Some(retry_at));

        let permanent = telegram_delivery_failure(crate::telegram::TelegramFailure::config(
            "Telegram bot token is not configured",
        ));
        assert!(!permanent.retryable);
        assert_eq!(permanent.retry_at, None);
    }

    #[test]
    fn telegram_message_reframes_the_email_without_changing_it() {
        let rendered = RenderedNotificationEmail {
            subject: "Client offline".into(),
            html: "<p>ignored</p>".into(),
            text: format!(
                "{EMAIL_TEXT_BRAND_LINE}

Client offline

Client: abcdef12



Open the dashboard: https://router.example.com

{EMAIL_TEXT_FOOTER}
"
            ),
        };

        let message = render_telegram_message(&rendered, "https://router.example.com/");

        assert!(message.starts_with(
            "Client offline

"
        ));
        assert!(!message.contains(EMAIL_TEXT_BRAND_LINE));
        assert!(
            !message.contains(EMAIL_TEXT_FOOTER),
            "the RFC footer is meaningless in a chat"
        );
        assert!(message.contains("Client: abcdef12"));
        assert!(
            !message.contains(
                "


"
            ),
            "blank runs are collapsed"
        );
        assert!(message.ends_with(
            "管理通知渠道 / Manage notifications: https://router.example.com/account/notifications"
        ));
        assert!(message.chars().count() <= crate::telegram::MAX_MESSAGE_CHARS);
    }

    #[test]
    fn telegram_message_is_truncated_to_what_the_bot_api_accepts() {
        let rendered = RenderedNotificationEmail {
            subject: "Client offline".into(),
            html: String::new(),
            text: "line
"
            .repeat(4_000),
        };
        let message = render_telegram_message(&rendered, "https://router.example.com");
        assert_eq!(
            message.chars().count(),
            crate::telegram::MAX_MESSAGE_CHARS,
            "a runaway digest must still be sendable"
        );
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
    fn policy_normalizes_unsafe_boot_values() {
        let settings = ClientNotificationSettings {
            enabled: true,
            offline_alert_secs: -1,
            recovery_stable_secs: i64::MAX,
            storm_percent: 101,
            registration_recipient_hourly_limit: i64::MAX,
            registration_global_hourly_limit: -1,
            ..ClientNotificationSettings::default()
        };
        let policy = ClientNotificationPolicy::from(&settings);
        assert!(policy.enabled);
        assert_eq!(policy.offline_alert_secs, MIN_OFFLINE_ALERT_SECS);
        assert_eq!(policy.recovery_stable_secs, 3_600);
        assert_eq!(policy.storm_percent, 100);
        assert_eq!(policy.registration_recipient_hourly_limit, 1_000);
        assert_eq!(policy.registration_global_hourly_limit, 1_000);

        let disabled = ClientNotificationPolicy::from(&ClientNotificationSettings {
            enabled: false,
            ..ClientNotificationSettings::default()
        });
        assert!(!disabled.enabled);
    }

    #[test]
    fn runtime_policy_preserves_normalized_registration_caps() {
        let mut config = Config::from_env();
        config.client_stale_secs = 3_600;
        config.cleanup_interval_secs = 300;
        let settings = ClientNotificationSettings {
            enabled: true,
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
    fn presence_reconciliation_waits_for_startup_grace_without_blocking_delivery_ticks() {
        let grace = Duration::from_secs(180);

        assert!(!should_reconcile_presence(0, Duration::ZERO, grace));
        assert!(!should_reconcile_presence(
            4,
            Duration::from_secs(179),
            grace
        ));
        assert!(should_reconcile_presence(4, grace, grace));
        assert!(!should_reconcile_presence(5, grace, grace));
    }

    #[test]
    fn templates_escape_values_and_never_include_full_installation_id() {
        let full_id = "12345678-secret-rest";
        let email = render_offline_email(&OfflineEmailData {
            installation_id: full_id.into(),
            tunnel_subdomain: Some("client<script>".into()),
            owner_email: Some("owner@example.com".into()),
            version: Some("1.2.3".into()),
            client_url: Some("https://client.example.com/".into()),
            last_heartbeat_at: "2026-07-15T12:00:00Z".into(),
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
            clients: (0..55)
                .map(|index| DigestEmailClient {
                    installation_id: format!("client-{index:04}"),
                    client_url: Some(format!("https://client-{index:04}.example.com/")),
                    password_hint: None,
                })
                .collect(),
            occurred_at: "2026-07-15T12:00:00Z".into(),
            dashboard_url: "https://router.example.com".into(),
        });
        assert_eq!(email.html.matches("<li").count(), MAX_DIGEST_CLIENTS + 1);
        assert!(email.html.contains("+5 more"));
        assert!(email.text.contains("https://client-0000.example.com/"));
        assert!(email.subject.chars().count() <= 150);
    }

    #[test]
    fn registration_template_includes_authoritative_url_and_partial_password_hint() {
        let email = render_registration_email(&RegistrationEmailData {
            installation_id: "12345678-secret-rest".into(),
            platform: "linux".into(),
            version: Some("1.2.3".into()),
            country_code: Some("US".into()),
            setup_completed_at: "2026-07-15T12:00:00Z".into(),
            owner_email: Some("owner@example.com".into()),
            client_url: Some("https://client.example.com/".into()),
            password_hint: Some("p******w".into()),
            dashboard_url: "https://router.example.com/".into(),
        });

        for body in [&email.html, &email.text] {
            assert!(body.contains("https://client.example.com/"));
            assert!(body.contains("p******w"));
            assert!(body.contains("not the complete password"));
            assert!(!body.contains("paraview"));
        }
    }

    #[test]
    fn registration_digest_lists_each_url_and_partial_password_hint() {
        let email = render_digest_email(&DigestEmailData {
            event_label: "registrations".into(),
            incident: false,
            clients: vec![
                DigestEmailClient {
                    installation_id: "client-a-long-id".into(),
                    client_url: Some("https://client-a.example.com/".into()),
                    password_hint: Some("p******w".into()),
                },
                DigestEmailClient {
                    installation_id: "client-b-long-id".into(),
                    client_url: Some("https://client-b.example.com/".into()),
                    password_hint: Some("s******t".into()),
                },
            ],
            occurred_at: "2026-07-15T12:00:00Z".into(),
            dashboard_url: "https://router.example.com/".into(),
        });

        for body in [&email.html, &email.text] {
            assert!(body.contains("https://client-a.example.com/"));
            assert!(body.contains("https://client-b.example.com/"));
            assert!(body.contains("p******w"));
            assert!(body.contains("s******t"));
            assert!(body.contains("partial"));
        }
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
        assert!(email.html.len() < 10_000);
        assert!(email.text.len() < 3_000);
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
        let (endpoint, requests, server) =
            start_mock_resend(StatusCode::OK, r#"{"id":"email_123"}"#, None).await;
        let result =
            send_resend_email_to(&reqwest::Client::new(), "re_test", &test_batch(), &endpoint)
                .await
                .unwrap();
        server.abort();
        assert_eq!(result, "email_123");
        let requests = requests.lock().await;
        let payload: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(
            payload.get("text").and_then(|value| value.as_str()),
            Some("offline")
        );
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
