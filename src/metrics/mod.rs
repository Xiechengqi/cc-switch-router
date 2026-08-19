pub mod collector;
pub mod models;
pub mod store;

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::clock_health::{ClockHealthService, ClockHealthStatus};
use crate::config::{Config, MetricsConfig};
use crate::error::AppError;
use crate::models::{GatewayRequestObservation, ShareRequestLogEntry};
use crate::proxy::ProxyRegistry;
use crate::store::AppStore;
use crate::{alerting::AlertingService, alerting::models::AlertCondition};

use self::collector::{HostSampler, host_info};
use self::models::{
    ClientMetricsSnapshot, HostMetricsInfo, HostMetricsStatus, LlmMetricsSnapshot,
    LlmRequestMetric, MetricEvent, MetricsHealth, MetricsSnapshot, RouterMetricsStatus,
};
use self::store::MetricsStore;

#[derive(Debug, Default)]
struct AlertRouterBaselineState {
    committed: Option<RouterMetricsStatus>,
    origin: Option<RouterMetricsStatus>,
}

impl AlertRouterBaselineState {
    fn previous_for(&mut self, current: &RouterMetricsStatus) -> Option<RouterMetricsStatus> {
        if let Some(committed) = &self.committed {
            return Some(committed.clone());
        }
        if let Some(origin) = &self.origin {
            return Some(origin.clone());
        }
        self.origin = Some(current.clone());
        None
    }

    fn commit(&mut self, current: RouterMetricsStatus) {
        self.committed = Some(current);
        self.origin = None;
    }
}

#[derive(Debug)]
pub struct MetricsRegistry {
    enabled: bool,
    sample_interval_secs: u64,
    store: MetricsStore,
    sampler: Mutex<HostSampler>,
    sample_cycle: Mutex<()>,
    last_host: Mutex<Option<HostMetricsStatus>>,
    alert_router_baseline: Mutex<AlertRouterBaselineState>,
    proxy_inflight: AtomicU64,
    proxy_requests_total: AtomicU64,
    proxy_upstream_errors_total: AtomicU64,
    proxy_5xx_total: AtomicU64,
    share_request_watchdog_forced_release_total: AtomicU64,
    share_request_manual_release_total: AtomicU64,
    proxy_request_body_timeout_total: AtomicU64,
    proxy_response_header_timeout_total: AtomicU64,
    proxy_downstream_stall_timeout_total: AtomicU64,
    proxy_request_hard_timeout_total: AtomicU64,
    proxy_stream_semantic_terminal_total: AtomicU64,
    proxy_stream_first_event_timeout_total: AtomicU64,
    proxy_stream_idle_timeout_total: AtomicU64,
    proxy_stream_parser_overflow_total: AtomicU64,
    proxy_stream_upstream_errors_total: AtomicU64,
    health_probe_failures_total: AtomicU64,
    health_probe_cached_failures_total: AtomicU64,
    db_errors_total: AtomicU64,
    ssh_active_sessions: AtomicU64,
    ssh_forward_listeners: AtomicU64,
    ssh_forward_listener_created_total: AtomicU64,
    ssh_forward_listener_shutdown_total: AtomicU64,
    ssh_forward_bind_errors_total: AtomicU64,
    ssh_forward_accept_errors_total: AtomicU64,
    ssh_forward_emfile_errors_total: AtomicU64,
    ssh_pending_channel_opens: AtomicU64,
    ssh_channel_open_started_total: AtomicU64,
    ssh_channel_open_succeeded_total: AtomicU64,
    ssh_channel_open_explicit_failures_total: AtomicU64,
    ssh_channel_open_timeout_total: AtomicU64,
    ssh_channel_open_session_errors_total: AtomicU64,
    ssh_channel_open_cancelled_total: AtomicU64,
    ssh_active_bridges: AtomicU64,
    ssh_bridge_created_total: AtomicU64,
    ssh_bridge_completed_total: AtomicU64,
    ssh_bridge_cancelled_total: AtomicU64,
    ssh_bridge_write_stall_total: AtomicU64,
    ssh_bridge_half_close_idle_total: AtomicU64,
    ssh_bridge_io_errors_total: AtomicU64,
    ssh_forward_capacity_rejected_total: AtomicU64,
}

impl MetricsRegistry {
    pub fn new(config: MetricsConfig) -> Arc<Self> {
        let sample_interval_secs = config.sample_interval_secs.max(1);
        Arc::new(Self {
            enabled: config.enabled,
            sample_interval_secs,
            store: MetricsStore::new(config.db_path, config.retention_days),
            sampler: Mutex::new(HostSampler::default()),
            sample_cycle: Mutex::new(()),
            last_host: Mutex::new(None),
            alert_router_baseline: Mutex::new(AlertRouterBaselineState::default()),
            proxy_inflight: AtomicU64::new(0),
            proxy_requests_total: AtomicU64::new(0),
            proxy_upstream_errors_total: AtomicU64::new(0),
            proxy_5xx_total: AtomicU64::new(0),
            share_request_watchdog_forced_release_total: AtomicU64::new(0),
            share_request_manual_release_total: AtomicU64::new(0),
            proxy_request_body_timeout_total: AtomicU64::new(0),
            proxy_response_header_timeout_total: AtomicU64::new(0),
            proxy_downstream_stall_timeout_total: AtomicU64::new(0),
            proxy_request_hard_timeout_total: AtomicU64::new(0),
            proxy_stream_semantic_terminal_total: AtomicU64::new(0),
            proxy_stream_first_event_timeout_total: AtomicU64::new(0),
            proxy_stream_idle_timeout_total: AtomicU64::new(0),
            proxy_stream_parser_overflow_total: AtomicU64::new(0),
            proxy_stream_upstream_errors_total: AtomicU64::new(0),
            health_probe_failures_total: AtomicU64::new(0),
            health_probe_cached_failures_total: AtomicU64::new(0),
            db_errors_total: AtomicU64::new(0),
            ssh_active_sessions: AtomicU64::new(0),
            ssh_forward_listeners: AtomicU64::new(0),
            ssh_forward_listener_created_total: AtomicU64::new(0),
            ssh_forward_listener_shutdown_total: AtomicU64::new(0),
            ssh_forward_bind_errors_total: AtomicU64::new(0),
            ssh_forward_accept_errors_total: AtomicU64::new(0),
            ssh_forward_emfile_errors_total: AtomicU64::new(0),
            ssh_pending_channel_opens: AtomicU64::new(0),
            ssh_channel_open_started_total: AtomicU64::new(0),
            ssh_channel_open_succeeded_total: AtomicU64::new(0),
            ssh_channel_open_explicit_failures_total: AtomicU64::new(0),
            ssh_channel_open_timeout_total: AtomicU64::new(0),
            ssh_channel_open_session_errors_total: AtomicU64::new(0),
            ssh_channel_open_cancelled_total: AtomicU64::new(0),
            ssh_active_bridges: AtomicU64::new(0),
            ssh_bridge_created_total: AtomicU64::new(0),
            ssh_bridge_completed_total: AtomicU64::new(0),
            ssh_bridge_cancelled_total: AtomicU64::new(0),
            ssh_bridge_write_stall_total: AtomicU64::new(0),
            ssh_bridge_half_close_idle_total: AtomicU64::new(0),
            ssh_bridge_io_errors_total: AtomicU64::new(0),
            ssh_forward_capacity_rejected_total: AtomicU64::new(0),
        })
    }

    pub fn store(&self) -> &MetricsStore {
        &self.store
    }

    pub async fn init(&self) -> Result<(), AppError> {
        if self.enabled {
            self.store.init().await?;
        }
        Ok(())
    }

    pub async fn host_info(&self, config: &Config) -> HostMetricsInfo {
        host_info(config, self.store.path())
    }

    pub async fn current_host_status(&self, config: &Config) -> HostMetricsStatus {
        let host = self.sampler.lock().await.sample(config, self.store.path());
        *self.last_host.lock().await = Some(host.clone());
        host
    }

    pub async fn sample_and_store(
        &self,
        config: &Config,
        proxy: &ProxyRegistry,
        app_store: &AppStore,
        alerting: &AlertingService,
    ) -> Result<(), AppError> {
        let _cycle = self.sample_cycle.lock().await;
        let host = self.current_host_status(config).await;
        let router = self.router_status(proxy).await;
        if !self.enabled {
            return Ok(());
        }
        let previous_router = self
            .alert_router_baseline
            .lock()
            .await
            .previous_for(&router);
        let (clients, client_metrics_error) =
            match app_store.client_metrics_snapshot(chrono::Utc::now()).await {
                Ok(clients) => (clients, None),
                Err(error) => (
                    ClientMetricsSnapshot {
                        timestamp: host.timestamp,
                        ..Default::default()
                    },
                    Some(error.to_string()),
                ),
            };
        let llm = self.store.llm_snapshot(5 * 60).await.unwrap_or_default();
        let conditions = build_alert_conditions(
            &host,
            &router,
            previous_router.as_ref(),
            &llm,
            client_metrics_error.as_deref(),
        );
        let (persistence, reconciliation) = run_sample_sinks(
            || {
                self.store
                    .insert_sample(host.clone(), router.clone(), clients)
            },
            || alerting.reconcile_metrics(conditions, host.timestamp),
        )
        .await;
        if reconciliation.is_ok() {
            self.alert_router_baseline.lock().await.commit(router);
        }
        combine_sample_results(persistence, reconciliation)
    }

    pub async fn record_clock_sample(&self, sample: ClockHealthStatus) -> Result<(), AppError> {
        if !self.enabled {
            return Ok(());
        }
        self.store.insert_clock_sample(sample).await
    }

    pub async fn snapshot(
        &self,
        config: &Config,
        proxy: &ProxyRegistry,
        app_store: &AppStore,
        alerting: &AlertingService,
        clock_health: &ClockHealthService,
    ) -> Result<MetricsSnapshot, AppError> {
        let host = self.current_host_status(config).await;
        let router = self.router_status(proxy).await;
        let mut llm = if self.enabled {
            self.store.llm_snapshot(5 * 60).await.unwrap_or_default()
        } else {
            LlmMetricsSnapshot::default()
        };
        llm.inflight = self.proxy_inflight.load(Ordering::Relaxed);
        let clients = app_store
            .client_metrics_snapshot(chrono::Utc::now())
            .await
            .unwrap_or_else(|_| ClientMetricsSnapshot {
                timestamp: host.timestamp,
                ..Default::default()
            });
        let incidents = alerting.active_incidents().await.unwrap_or_default();
        let alerts = incidents
            .iter()
            .map(|incident| MetricEvent {
                id: None,
                timestamp: incident.last_transition_at,
                severity: incident.severity.clone(),
                kind: incident.kind.clone(),
                message: incident.message.clone(),
                details: serde_json::json!({
                    "incidentId": incident.id,
                    "status": incident.status,
                    "entityKind": incident.entity_kind,
                    "entityId": incident.entity_id,
                }),
            })
            .collect::<Vec<_>>();
        let status = incidents
            .iter()
            .fold(MetricsHealth::Healthy, |current, incident| {
                match (current, incident.severity.as_str()) {
                    (_, "critical") => MetricsHealth::Critical,
                    (MetricsHealth::Healthy, "warning") => MetricsHealth::Warning,
                    (other, _) => other,
                }
            });
        let last_persisted_at = if self.enabled {
            self.store.latest_sample_timestamp().await.unwrap_or(None)
        } else {
            None
        };
        Ok(MetricsSnapshot {
            status,
            sampled_at: host.timestamp,
            enabled: self.enabled,
            sample_interval_secs: self.sample_interval_secs,
            last_persisted_at,
            clock: clock_health.snapshot().await,
            host,
            router,
            clients,
            llm,
            alerts,
            incidents,
        })
    }

    pub async fn router_status(&self, proxy: &ProxyRegistry) -> RouterMetricsStatus {
        let counts = proxy.counts().await;
        let share_requests = proxy.share_request_registry_snapshot().await;
        RouterMetricsStatus {
            active_routes: counts.active_routes as u64,
            pending_routes: counts.pending_routes as u64,
            health_probe_failure_cache: counts.health_probe_failure_cache as u64,
            ssh_active_sessions: self.ssh_active_sessions.load(Ordering::Relaxed),
            ssh_forward_listeners: self.ssh_forward_listeners.load(Ordering::Relaxed),
            ssh_forward_listener_created_total: self
                .ssh_forward_listener_created_total
                .load(Ordering::Relaxed),
            ssh_forward_listener_shutdown_total: self
                .ssh_forward_listener_shutdown_total
                .load(Ordering::Relaxed),
            ssh_forward_bind_errors_total: self
                .ssh_forward_bind_errors_total
                .load(Ordering::Relaxed),
            ssh_forward_accept_errors_total: self
                .ssh_forward_accept_errors_total
                .load(Ordering::Relaxed),
            ssh_forward_emfile_errors_total: self
                .ssh_forward_emfile_errors_total
                .load(Ordering::Relaxed),
            ssh_pending_channel_opens: self.ssh_pending_channel_opens.load(Ordering::Relaxed),
            ssh_channel_open_started_total: self
                .ssh_channel_open_started_total
                .load(Ordering::Relaxed),
            ssh_channel_open_succeeded_total: self
                .ssh_channel_open_succeeded_total
                .load(Ordering::Relaxed),
            ssh_channel_open_explicit_failures_total: self
                .ssh_channel_open_explicit_failures_total
                .load(Ordering::Relaxed),
            ssh_channel_open_timeout_total: self
                .ssh_channel_open_timeout_total
                .load(Ordering::Relaxed),
            ssh_channel_open_session_errors_total: self
                .ssh_channel_open_session_errors_total
                .load(Ordering::Relaxed),
            ssh_channel_open_cancelled_total: self
                .ssh_channel_open_cancelled_total
                .load(Ordering::Relaxed),
            ssh_active_bridges: self.ssh_active_bridges.load(Ordering::Relaxed),
            ssh_bridge_created_total: self.ssh_bridge_created_total.load(Ordering::Relaxed),
            ssh_bridge_completed_total: self.ssh_bridge_completed_total.load(Ordering::Relaxed),
            ssh_bridge_cancelled_total: self.ssh_bridge_cancelled_total.load(Ordering::Relaxed),
            ssh_bridge_write_stall_total: self.ssh_bridge_write_stall_total.load(Ordering::Relaxed),
            ssh_bridge_half_close_idle_total: self
                .ssh_bridge_half_close_idle_total
                .load(Ordering::Relaxed),
            ssh_bridge_io_errors_total: self.ssh_bridge_io_errors_total.load(Ordering::Relaxed),
            ssh_forward_capacity_rejected_total: self
                .ssh_forward_capacity_rejected_total
                .load(Ordering::Relaxed),
            proxy_inflight: self.proxy_inflight.load(Ordering::Relaxed),
            proxy_requests_total: self.proxy_requests_total.load(Ordering::Relaxed),
            proxy_upstream_errors_total: self.proxy_upstream_errors_total.load(Ordering::Relaxed),
            proxy_5xx_total: self.proxy_5xx_total.load(Ordering::Relaxed),
            share_active_requests: share_requests.active_requests as u64,
            share_oldest_inflight_age_secs: share_requests.oldest_inflight_age_secs,
            share_oldest_progress_age_secs: share_requests.oldest_progress_age_secs,
            share_request_watchdog_forced_release_total: self
                .share_request_watchdog_forced_release_total
                .load(Ordering::Relaxed),
            share_request_manual_release_total: self
                .share_request_manual_release_total
                .load(Ordering::Relaxed),
            proxy_request_body_timeout_total: self
                .proxy_request_body_timeout_total
                .load(Ordering::Relaxed),
            proxy_response_header_timeout_total: self
                .proxy_response_header_timeout_total
                .load(Ordering::Relaxed),
            proxy_downstream_stall_timeout_total: self
                .proxy_downstream_stall_timeout_total
                .load(Ordering::Relaxed),
            proxy_request_hard_timeout_total: self
                .proxy_request_hard_timeout_total
                .load(Ordering::Relaxed),
            proxy_stream_semantic_terminal_total: self
                .proxy_stream_semantic_terminal_total
                .load(Ordering::Relaxed),
            proxy_stream_first_event_timeout_total: self
                .proxy_stream_first_event_timeout_total
                .load(Ordering::Relaxed),
            proxy_stream_idle_timeout_total: self
                .proxy_stream_idle_timeout_total
                .load(Ordering::Relaxed),
            proxy_stream_parser_overflow_total: self
                .proxy_stream_parser_overflow_total
                .load(Ordering::Relaxed),
            proxy_stream_upstream_errors_total: self
                .proxy_stream_upstream_errors_total
                .load(Ordering::Relaxed),
            health_probe_failures_total: self.health_probe_failures_total.load(Ordering::Relaxed),
            health_probe_cached_failures_total: self
                .health_probe_cached_failures_total
                .load(Ordering::Relaxed),
            db_errors_total: self.db_errors_total.load(Ordering::Relaxed),
        }
    }

    pub fn proxy_request_started(self: &Arc<Self>) -> MetricsPermit {
        self.proxy_requests_total.fetch_add(1, Ordering::Relaxed);
        self.proxy_inflight.fetch_add(1, Ordering::Relaxed);
        MetricsPermit {
            metrics: self.clone(),
            closed: false,
        }
    }

    pub fn record_proxy_status(&self, status: axum::http::StatusCode) {
        if status.is_server_error() {
            self.proxy_5xx_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_proxy_upstream_error(&self, is_health_check: bool) {
        self.proxy_upstream_errors_total
            .fetch_add(1, Ordering::Relaxed);
        if is_health_check {
            self.health_probe_failures_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_proxy_stream_semantic_terminal(&self) {
        self.proxy_stream_semantic_terminal_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_proxy_request_body_timeout(&self) {
        self.proxy_request_body_timeout_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_proxy_response_header_timeout(&self) {
        self.proxy_response_header_timeout_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_proxy_downstream_stall_timeout(&self) {
        self.proxy_downstream_stall_timeout_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_proxy_request_hard_timeout(&self) {
        self.proxy_request_hard_timeout_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_share_request_watchdog_release(&self, reason: &str) {
        self.share_request_watchdog_forced_release_total
            .fetch_add(1, Ordering::Relaxed);
        match reason {
            "response_header_timeout" => self.record_proxy_response_header_timeout(),
            "first_event_timeout" => self.record_proxy_stream_first_event_timeout(),
            "business_idle_timeout" => self.record_proxy_stream_idle_timeout(),
            "hard_lifetime_timeout" => self.record_proxy_request_hard_timeout(),
            _ => {}
        }
    }

    pub fn record_share_request_manual_release(&self, count: usize) {
        self.share_request_manual_release_total
            .fetch_add(count as u64, Ordering::Relaxed);
    }

    pub fn record_proxy_stream_first_event_timeout(&self) {
        self.proxy_stream_first_event_timeout_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_proxy_stream_idle_timeout(&self) {
        self.proxy_stream_idle_timeout_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_proxy_stream_parser_overflow(&self) {
        self.proxy_stream_parser_overflow_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_proxy_stream_upstream_error(&self) {
        self.proxy_stream_upstream_errors_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_health_probe_cached_failure(&self) {
        self.health_probe_cached_failures_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_db_error(&self) {
        self.db_errors_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn ssh_session_started(self: &Arc<Self>) -> MetricsSessionGuard {
        self.ssh_active_sessions.fetch_add(1, Ordering::Relaxed);
        MetricsSessionGuard {
            metrics: self.clone(),
            closed: false,
        }
    }

    pub fn forward_listener_started(self: &Arc<Self>) -> MetricsForwardListenerGuard {
        self.ssh_forward_listeners.fetch_add(1, Ordering::Relaxed);
        self.ssh_forward_listener_created_total
            .fetch_add(1, Ordering::Relaxed);
        MetricsForwardListenerGuard {
            metrics: self.clone(),
        }
    }

    pub fn forward_bind_error(&self, message: &str) {
        self.ssh_forward_bind_errors_total
            .fetch_add(1, Ordering::Relaxed);
        if message.contains("Too many open files") || message.contains("os error 24") {
            self.ssh_forward_emfile_errors_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn forward_accept_error(&self, message: &str) {
        self.ssh_forward_accept_errors_total
            .fetch_add(1, Ordering::Relaxed);
        if message.contains("Too many open files") || message.contains("os error 24") {
            self.ssh_forward_emfile_errors_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn forward_channel_open_started(self: &Arc<Self>) -> MetricsChannelOpenGuard {
        self.ssh_pending_channel_opens
            .fetch_add(1, Ordering::Relaxed);
        self.ssh_channel_open_started_total
            .fetch_add(1, Ordering::Relaxed);
        MetricsChannelOpenGuard {
            metrics: self.clone(),
            closed: false,
        }
    }

    pub fn forward_bridge_started(self: &Arc<Self>) -> MetricsBridgeGuard {
        self.ssh_active_bridges.fetch_add(1, Ordering::Relaxed);
        self.ssh_bridge_created_total
            .fetch_add(1, Ordering::Relaxed);
        MetricsBridgeGuard {
            metrics: self.clone(),
            closed: false,
        }
    }

    pub fn forward_capacity_rejected(&self) {
        self.ssh_forward_capacity_rejected_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_llm_request(self: &Arc<Self>, metric: LlmRequestMetric) {
        if !self.enabled {
            return;
        }
        let metrics = self.clone();
        tokio::spawn(async move {
            if let Err(err) = metrics.store.insert_llm_request(metric).await {
                metrics.record_db_error();
                warn!("record llm metric failed: {err}");
            }
        });
    }

    pub fn record_gateway_request_observations(
        self: &Arc<Self>,
        gateway_id: &str,
        logs: &[GatewayRequestObservation],
    ) {
        for log in logs {
            let stream_status = log.status.trim().to_ascii_lowercase();
            let status = normalize_llm_status(&stream_status, log.status_code);
            self.record_llm_request(LlmRequestMetric {
                timestamp: parse_rfc3339_timestamp(&log.created_at)
                    .unwrap_or_else(|| chrono::Utc::now().timestamp()),
                request_id: Some(log.request_id.clone()),
                route_type: "gateway".into(),
                gateway_id: Some(gateway_id.to_string()),
                share_id: log.share_id.clone(),
                subdomain: log.share_subdomain.clone(),
                app_type: Some(log.request_agent.clone()).filter(|value| !value.is_empty()),
                provider: None,
                requested_model: Some(log.requested_model.clone())
                    .filter(|value| !value.is_empty()),
                actual_model: Some(log.actual_model.clone()).filter(|value| !value.is_empty()),
                status: status.clone(),
                error_kind: error_kind_from_status(&stream_status, log.status_code),
                http_status: log.status_code,
                latency_ms: log.latency_ms,
                ttft_ms: None,
                stream_started: false,
                stream_completed: status == "success",
                usage_state: Some(if status == "pending" {
                    "pending".into()
                } else {
                    "observed".into()
                }),
                stream_status: Some(stream_status).filter(|value| !value.is_empty()),
                usage_revision: 0,
                input_tokens: Some(log.input_tokens as u64),
                output_tokens: Some(log.output_tokens as u64),
                total_tokens: Some(
                    log.input_tokens as u64
                        + log.output_tokens as u64
                        + log.cache_read_tokens as u64
                        + log.cache_creation_tokens as u64,
                ),
                cache_read_tokens: Some(log.cache_read_tokens as u64),
                cache_write_tokens: Some(log.cache_creation_tokens as u64),
                reasoning_tokens: None,
                estimated_cost_usd: None,
            });
        }
    }

    pub fn record_share_request_logs(self: &Arc<Self>, logs: &[ShareRequestLogEntry]) {
        for log in logs {
            if log.is_health_check {
                continue;
            }
            let stream_status = log
                .stream_status
                .as_deref()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            let usage_state = log.usage_state.trim().to_ascii_lowercase();
            let interrupted = usage_state == "interrupted"
                || matches!(
                    stream_status.as_str(),
                    "client_cancelled"
                        | "interrupted"
                        | "timeout"
                        | "transport_error"
                        | "protocol_error"
                );
            let stream_completed = log.is_streaming
                && stream_status == "completed"
                && (200..300).contains(&log.status_code);
            let status = if log.status_code >= 400 || interrupted {
                "error"
            } else if log.is_streaming && !stream_completed {
                "pending"
            } else {
                "success"
            };
            self.record_llm_request(LlmRequestMetric {
                timestamp: log.created_at,
                request_id: Some(log.request_id.clone()),
                route_type: "direct".into(),
                gateway_id: None,
                share_id: Some(log.share_id.clone()),
                subdomain: None,
                app_type: Some(log.app_type.clone()).filter(|value| !value.is_empty()),
                provider: Some(log.provider_name.clone()).filter(|value| !value.is_empty()),
                requested_model: Some(log.requested_model.clone())
                    .filter(|value| !value.is_empty()),
                actual_model: Some(log.actual_model.clone()).filter(|value| !value.is_empty()),
                status: status.into(),
                error_kind: if interrupted {
                    Some("interrupted".into())
                } else {
                    error_kind_from_status("", Some(log.status_code))
                },
                http_status: Some(log.status_code),
                latency_ms: Some(log.latency_ms),
                ttft_ms: log.first_token_ms,
                stream_started: log.is_streaming,
                stream_completed,
                usage_state: Some(usage_state),
                stream_status: Some(stream_status).filter(|value| !value.is_empty()),
                usage_revision: log.usage_revision,
                input_tokens: Some(log.input_tokens as u64),
                output_tokens: Some(log.output_tokens as u64),
                total_tokens: Some(
                    log.input_tokens as u64
                        + log.output_tokens as u64
                        + log.cache_read_tokens as u64
                        + log.cache_creation_tokens as u64,
                ),
                cache_read_tokens: Some(log.cache_read_tokens as u64),
                cache_write_tokens: Some(log.cache_creation_tokens as u64),
                reasoning_tokens: None,
                estimated_cost_usd: None,
            });
        }
    }
}

#[derive(Debug)]
pub struct MetricsPermit {
    metrics: Arc<MetricsRegistry>,
    closed: bool,
}

impl Drop for MetricsPermit {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        decrement(&self.metrics.proxy_inflight);
    }
}

#[derive(Debug)]
pub struct MetricsSessionGuard {
    metrics: Arc<MetricsRegistry>,
    closed: bool,
}

#[derive(Debug)]
pub struct MetricsForwardListenerGuard {
    metrics: Arc<MetricsRegistry>,
}

impl Drop for MetricsForwardListenerGuard {
    fn drop(&mut self) {
        decrement(&self.metrics.ssh_forward_listeners);
        self.metrics
            .ssh_forward_listener_shutdown_total
            .fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardChannelOpenMetricOutcome {
    Succeeded,
    ExplicitFailure,
    TimedOut,
    SessionError,
    Cancelled,
}

#[derive(Debug)]
pub struct MetricsChannelOpenGuard {
    metrics: Arc<MetricsRegistry>,
    closed: bool,
}

impl MetricsChannelOpenGuard {
    pub fn finish(mut self, outcome: ForwardChannelOpenMetricOutcome) {
        self.close(outcome);
    }

    fn close(&mut self, outcome: ForwardChannelOpenMetricOutcome) {
        if self.closed {
            return;
        }
        self.closed = true;
        decrement(&self.metrics.ssh_pending_channel_opens);
        let counter = match outcome {
            ForwardChannelOpenMetricOutcome::Succeeded => {
                &self.metrics.ssh_channel_open_succeeded_total
            }
            ForwardChannelOpenMetricOutcome::ExplicitFailure => {
                &self.metrics.ssh_channel_open_explicit_failures_total
            }
            ForwardChannelOpenMetricOutcome::TimedOut => {
                &self.metrics.ssh_channel_open_timeout_total
            }
            ForwardChannelOpenMetricOutcome::SessionError => {
                &self.metrics.ssh_channel_open_session_errors_total
            }
            ForwardChannelOpenMetricOutcome::Cancelled => {
                &self.metrics.ssh_channel_open_cancelled_total
            }
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for MetricsChannelOpenGuard {
    fn drop(&mut self) {
        self.close(ForwardChannelOpenMetricOutcome::Cancelled);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardBridgeMetricOutcome {
    Completed,
    Cancelled,
    WriteStall,
    HalfCloseIdle,
    IoError,
}

#[derive(Debug)]
pub struct MetricsBridgeGuard {
    metrics: Arc<MetricsRegistry>,
    closed: bool,
}

impl MetricsBridgeGuard {
    pub fn finish(mut self, outcome: ForwardBridgeMetricOutcome) {
        self.close(outcome);
    }

    fn close(&mut self, outcome: ForwardBridgeMetricOutcome) {
        if self.closed {
            return;
        }
        self.closed = true;
        decrement(&self.metrics.ssh_active_bridges);
        let counter = match outcome {
            ForwardBridgeMetricOutcome::Completed => &self.metrics.ssh_bridge_completed_total,
            ForwardBridgeMetricOutcome::Cancelled => &self.metrics.ssh_bridge_cancelled_total,
            ForwardBridgeMetricOutcome::WriteStall => &self.metrics.ssh_bridge_write_stall_total,
            ForwardBridgeMetricOutcome::HalfCloseIdle => {
                &self.metrics.ssh_bridge_half_close_idle_total
            }
            ForwardBridgeMetricOutcome::IoError => &self.metrics.ssh_bridge_io_errors_total,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for MetricsBridgeGuard {
    fn drop(&mut self) {
        self.close(ForwardBridgeMetricOutcome::Cancelled);
    }
}

impl Drop for MetricsSessionGuard {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        decrement(&self.metrics.ssh_active_sessions);
    }
}

pub async fn run_collector(
    metrics: Arc<MetricsRegistry>,
    config: Config,
    proxy: Arc<ProxyRegistry>,
    app_store: AppStore,
    alerting: Arc<AlertingService>,
) {
    if let Err(err) = metrics.init().await {
        warn!("metrics init failed: {err}");
    }
    let mut interval = tokio::time::interval(Duration::from_secs(
        config.metrics.sample_interval_secs.max(1),
    ));
    let prune_every = Duration::from_secs(3600);
    let mut last_prune = Instant::now();
    loop {
        interval.tick().await;
        if !config.metrics.enabled {
            continue;
        }
        if let Err(err) = metrics
            .sample_and_store(&config, &proxy, &app_store, &alerting)
            .await
        {
            metrics.record_db_error();
            debug!("metrics sample failed: {err}");
        }
        if last_prune.elapsed() >= prune_every {
            if let Err(err) = metrics.store().prune().await {
                metrics.record_db_error();
                debug!("metrics prune failed: {err}");
            }
            last_prune = Instant::now();
        }
    }
}

fn decrement(value: &AtomicU64) {
    let mut current = value.load(Ordering::Relaxed);
    while current > 0 {
        match value.compare_exchange_weak(
            current,
            current - 1,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(next) => current = next,
        }
    }
}

async fn run_sample_sinks<P, PF, R, RF>(
    persist: P,
    reconcile: R,
) -> (Result<(), AppError>, Result<(), AppError>)
where
    P: FnOnce() -> PF,
    PF: Future<Output = Result<(), AppError>>,
    R: FnOnce() -> RF,
    RF: Future<Output = Result<(), AppError>>,
{
    let persistence = persist().await;
    let reconciliation = reconcile().await;
    (persistence, reconciliation)
}

fn combine_sample_results(
    persistence: Result<(), AppError>,
    reconciliation: Result<(), AppError>,
) -> Result<(), AppError> {
    match (persistence, reconciliation) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(persistence), Err(reconciliation)) => Err(AppError::Internal(format!(
            "metrics history persistence failed: {persistence}; alert reconciliation failed: {reconciliation}"
        ))),
    }
}

fn build_alert_conditions(
    host: &HostMetricsStatus,
    router: &RouterMetricsStatus,
    previous_router: Option<&RouterMetricsStatus>,
    llm: &LlmMetricsSnapshot,
    client_metrics_error: Option<&str>,
) -> Vec<AlertCondition> {
    let mut conditions = Vec::new();
    let mut push =
        |kind: &str, severity: &str, title: &str, message: &str, details: serde_json::Value| {
            conditions.push(AlertCondition {
                fingerprint: format!("{kind}:router:router"),
                scope: "metrics".into(),
                kind: kind.into(),
                entity_kind: "router".into(),
                entity_id: Some("router".into()),
                severity: severity.into(),
                title: title.into(),
                message: message.into(),
                details,
            });
        };
    if let Some(fd) = host.process.fd_usage_percent {
        if fd >= 85.0 {
            push(
                "fd_pressure",
                "critical",
                "Router file descriptor pressure",
                "FD usage is critical",
                serde_json::json!({ "fdUsagePercent": fd }),
            );
        } else if fd >= 70.0 {
            push(
                "fd_pressure",
                "warning",
                "Router file descriptor pressure",
                "FD usage is elevated",
                serde_json::json!({ "fdUsagePercent": fd }),
            );
        }
    }
    if let Some(cpu) = host.cpu_percent {
        if cpu >= 90.0 {
            push(
                "host_cpu_pressure",
                "critical",
                "Host CPU pressure",
                "Host CPU usage is critical",
                serde_json::json!({ "cpuPercent": cpu }),
            );
        } else if cpu >= 75.0 {
            push(
                "host_cpu_pressure",
                "warning",
                "Host CPU pressure",
                "Host CPU usage is elevated",
                serde_json::json!({ "cpuPercent": cpu }),
            );
        }
    }
    if let (Some(used), Some(total)) = (host.memory_used_bytes, host.memory_total_bytes) {
        if total > 0 {
            let percent = used as f64 * 100.0 / total as f64;
            if percent >= 92.0 {
                push(
                    "host_memory_pressure",
                    "critical",
                    "Host memory pressure",
                    "Host memory usage is critical",
                    serde_json::json!({ "memoryUsagePercent": percent }),
                );
            } else if percent >= 80.0 {
                push(
                    "host_memory_pressure",
                    "warning",
                    "Host memory pressure",
                    "Host memory usage is elevated",
                    serde_json::json!({ "memoryUsagePercent": percent }),
                );
            }
        }
    }
    if let Some(disk) = host.disks.first().filter(|disk| disk.total_bytes > 0) {
        let percent = disk.used_bytes as f64 * 100.0 / disk.total_bytes as f64;
        if percent >= 90.0 {
            push(
                "host_disk_pressure",
                "critical",
                "Host disk pressure",
                "Primary disk usage is critical",
                serde_json::json!({
                    "diskUsagePercent": percent,
                    "mountPoint": disk.mount_point,
                }),
            );
        } else if percent >= 80.0 {
            push(
                "host_disk_pressure",
                "warning",
                "Host disk pressure",
                "Primary disk usage is elevated",
                serde_json::json!({
                    "diskUsagePercent": percent,
                    "mountPoint": disk.mount_point,
                }),
            );
        }
    }
    if router.ssh_forward_listeners > router.active_routes + 2 {
        push(
            "route_lifecycle",
            "critical",
            "SSH route lifecycle mismatch",
            "Forward listeners exceed active routes",
            serde_json::json!({
                "forwardListeners": router.ssh_forward_listeners,
                "activeRoutes": router.active_routes,
            }),
        );
    }
    if let Some(previous) = previous_router {
        let db_errors = router
            .db_errors_total
            .saturating_sub(previous.db_errors_total);
        if db_errors > 0 {
            push(
                "db_error",
                "warning",
                "Router database errors",
                "Router observed new database errors",
                serde_json::json!({ "newErrors": db_errors, "total": router.db_errors_total }),
            );
        }
        let emfile_errors = router
            .ssh_forward_emfile_errors_total
            .saturating_sub(previous.ssh_forward_emfile_errors_total);
        if emfile_errors > 0 {
            push(
                "ssh_emfile",
                "critical",
                "SSH listener file descriptor exhaustion",
                "SSH forwarding observed too many open files",
                serde_json::json!({ "newErrors": emfile_errors }),
            );
        }
        let bridge_write_stalls = router
            .ssh_bridge_write_stall_total
            .saturating_sub(previous.ssh_bridge_write_stall_total);
        if bridge_write_stalls > 0 {
            push(
                "ssh_bridge_write_stall",
                "warning",
                "SSH bridge write progress stalled",
                "Forwarded TCP bridges reached the configured write-stall timeout",
                serde_json::json!({
                    "newStalls": bridge_write_stalls,
                    "total": router.ssh_bridge_write_stall_total,
                }),
            );
        }
        let channel_open_timeouts = router
            .ssh_channel_open_timeout_total
            .saturating_sub(previous.ssh_channel_open_timeout_total);
        if channel_open_timeouts > 0 {
            push(
                "ssh_channel_open_timeout",
                "warning",
                "SSH forwarded channel open timed out",
                "A Client did not confirm a forwarded TCP channel before the configured deadline",
                serde_json::json!({
                    "newTimeouts": channel_open_timeouts,
                    "total": router.ssh_channel_open_timeout_total,
                    "pendingChannelOpens": router.ssh_pending_channel_opens,
                }),
            );
        }
        let channel_session_errors = router
            .ssh_channel_open_session_errors_total
            .saturating_sub(previous.ssh_channel_open_session_errors_total);
        if channel_session_errors > 0 {
            push(
                "ssh_channel_open_session_error",
                "warning",
                "SSH session failed while opening a channel",
                "Forwarded TCP channel setup encountered a terminal SSH session error",
                serde_json::json!({
                    "newErrors": channel_session_errors,
                    "total": router.ssh_channel_open_session_errors_total,
                }),
            );
        }
        let capacity_rejections = router
            .ssh_forward_capacity_rejected_total
            .saturating_sub(previous.ssh_forward_capacity_rejected_total);
        if capacity_rejections > 0 {
            push(
                "ssh_forward_capacity",
                "warning",
                "SSH forwarding capacity exhausted",
                "Forwarded TCP connections were rejected at the configured connection limit",
                serde_json::json!({
                    "newRejections": capacity_rejections,
                    "total": router.ssh_forward_capacity_rejected_total,
                    "pendingChannelOpens": router.ssh_pending_channel_opens,
                    "activeBridges": router.ssh_active_bridges,
                }),
            );
        }
        let watchdog_releases = router
            .share_request_watchdog_forced_release_total
            .saturating_sub(previous.share_request_watchdog_forced_release_total);
        if watchdog_releases > 0 {
            push(
                "share_request_watchdog_release",
                "warning",
                "Share request leases were force-released",
                "The proxy watchdog reclaimed Share requests that exceeded a lifecycle deadline",
                serde_json::json!({
                    "newReleases": watchdog_releases,
                    "total": router.share_request_watchdog_forced_release_total,
                    "activeRequests": router.share_active_requests,
                }),
            );
        }
        let downstream_stalls = router
            .proxy_downstream_stall_timeout_total
            .saturating_sub(previous.proxy_downstream_stall_timeout_total);
        if downstream_stalls > 0 {
            push(
                "proxy_downstream_stall",
                "warning",
                "Proxy downstream delivery stalled",
                "Response pumps stopped after clients failed to consume buffered output",
                serde_json::json!({
                    "newTimeouts": downstream_stalls,
                    "total": router.proxy_downstream_stall_timeout_total,
                }),
            );
        }
    }
    if llm.rpm >= 1.0 && llm.error_rate >= 0.25 {
        push(
            "llm_error_rate",
            "critical",
            "LLM request error rate",
            "LLM error rate is critical",
            serde_json::json!({ "errorRate": llm.error_rate, "rpm": llm.rpm }),
        );
    } else if llm.rpm >= 1.0 && llm.error_rate >= 0.10 {
        push(
            "llm_error_rate",
            "warning",
            "LLM request error rate",
            "LLM error rate is elevated",
            serde_json::json!({ "errorRate": llm.error_rate, "rpm": llm.rpm }),
        );
    }
    if llm.rate_limit_per_minute >= 5.0 {
        push(
            "llm_rate_limit",
            "warning",
            "LLM rate limiting",
            "LLM rate limits increased",
            serde_json::json!({ "rateLimitPerMinute": llm.rate_limit_per_minute }),
        );
    }
    if let Some(error) = client_metrics_error {
        push(
            "client_metrics_unavailable",
            "warning",
            "Client status metrics unavailable",
            "Router could not read the Client presence state",
            serde_json::json!({ "error": error.chars().take(500).collect::<String>() }),
        );
    }
    conditions
}

fn parse_rfc3339_timestamp(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.timestamp())
}

fn normalize_llm_status(status: &str, status_code: Option<u16>) -> String {
    let status = status.trim().to_ascii_lowercase();
    if matches!(
        status.as_str(),
        "pending" | "in_progress" | "in-progress" | "processing"
    ) {
        "pending".into()
    } else if matches!(status.as_str(), "success" | "completed")
        || status_code.is_some_and(|code| code < 400)
    {
        "success".into()
    } else {
        "error".into()
    }
}

fn error_kind_from_status(status: &str, status_code: Option<u16>) -> Option<String> {
    const LOCAL_CONCURRENCY_CODES: &[&str] = &[
        "cc_switch_user_concurrency_limit_exceeded",
        "cc_switch_share_concurrency_limit_exceeded",
        "cc_switch_provider_account_concurrency_limit_exceeded",
        "cc_switch_free_share_ip_concurrency_limit_exceeded",
        "cc_switch_image_concurrency_limit_exceeded",
    ];
    if status == "concurrency_limited"
        || LOCAL_CONCURRENCY_CODES
            .iter()
            .any(|code| status.contains(code))
    {
        return Some("concurrency_limited".into());
    }
    if status_code == Some(429) || status.contains("rate_limit") || status.contains("rate_limited")
    {
        return Some("rate_limited".into());
    }
    match status_code {
        Some(401 | 403) => Some("auth_failed".into()),
        Some(404) => Some("model_unsupported".into()),
        Some(500..=599) => Some("upstream_error".into()),
        _ if status.contains("timeout") => Some("timeout".into()),
        _ if status.contains("error") || status.contains("failed") => Some("upstream_error".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::config::{AlertingSettings, MetricsConfig};
    use crate::error::AppError;
    use crate::metrics::models::{
        HostMetricsStatus, LlmMetricsSnapshot, NetworkMetricsStatus, ProcessMetricsStatus,
    };

    use super::{
        AlertRouterBaselineState, ForwardBridgeMetricOutcome, ForwardChannelOpenMetricOutcome,
        MetricsRegistry, RouterMetricsStatus, build_alert_conditions, error_kind_from_status,
        normalize_llm_status, run_sample_sinks,
    };

    #[test]
    fn alert_baseline_retains_deltas_until_reconciliation_commits() {
        let mut state = AlertRouterBaselineState::default();
        let first = RouterMetricsStatus {
            ssh_channel_open_timeout_total: 2,
            ..Default::default()
        };
        assert!(state.previous_for(&first).is_none());

        let recovered = RouterMetricsStatus {
            ssh_channel_open_timeout_total: 7,
            ..Default::default()
        };
        let previous = state
            .previous_for(&recovered)
            .expect("failed reconciliation should preserve the first observation");
        assert_eq!(previous.ssh_channel_open_timeout_total, 2);
        assert_eq!(
            recovered
                .ssh_channel_open_timeout_total
                .saturating_sub(previous.ssh_channel_open_timeout_total),
            5
        );

        state.commit(recovered.clone());
        let next = RouterMetricsStatus {
            ssh_channel_open_timeout_total: 9,
            ..Default::default()
        };
        let previous = state.previous_for(&next).unwrap();
        assert_eq!(previous.ssh_channel_open_timeout_total, 7);
    }

    #[test]
    fn share_watchdog_and_downstream_stall_deltas_raise_alerts() {
        let host = HostMetricsStatus {
            timestamp: 0,
            uptime_secs: None,
            cpu_percent: None,
            load_1: None,
            load_5: None,
            load_15: None,
            memory_used_bytes: None,
            memory_total_bytes: None,
            memory_available_bytes: None,
            swap_used_bytes: None,
            swap_total_bytes: None,
            disks: Vec::new(),
            network: NetworkMetricsStatus::default(),
            process: ProcessMetricsStatus::default(),
        };
        let previous = RouterMetricsStatus::default();
        let current = RouterMetricsStatus {
            share_request_watchdog_forced_release_total: 2,
            proxy_downstream_stall_timeout_total: 1,
            ..Default::default()
        };
        let conditions = build_alert_conditions(
            &host,
            &current,
            Some(&previous),
            &LlmMetricsSnapshot::default(),
            None,
        );
        let kinds = conditions
            .iter()
            .map(|condition| condition.kind.as_str())
            .collect::<Vec<_>>();
        assert!(kinds.contains(&"share_request_watchdog_release"));
        assert!(kinds.contains(&"proxy_downstream_stall"));
    }

    #[tokio::test]
    async fn alert_reconciliation_runs_when_metrics_history_persistence_fails() {
        let reconciled = Arc::new(AtomicBool::new(false));
        let reconcile_flag = reconciled.clone();
        let (persistence, reconciliation) = run_sample_sinks(
            || async { Err(AppError::Internal("injected metrics failure".into())) },
            move || async move {
                reconcile_flag.store(true, Ordering::Release);
                Ok(())
            },
        )
        .await;

        assert!(persistence.is_err());
        assert!(reconciliation.is_ok());
        assert!(reconciled.load(Ordering::Acquire));
    }

    #[test]
    fn concurrency_metrics_require_a_stable_local_code() {
        assert_eq!(
            error_kind_from_status("cc_switch_user_concurrency_limit_exceeded", Some(409))
                .as_deref(),
            Some("concurrency_limited")
        );
        assert_eq!(
            error_kind_from_status("concurrency_limited", Some(409)).as_deref(),
            Some("concurrency_limited")
        );
        assert_eq!(
            error_kind_from_status("provider concurrency limit exceeded", Some(409)).as_deref(),
            None
        );
        assert_eq!(
            error_kind_from_status("error", Some(409)).as_deref(),
            Some("upstream_error")
        );
    }

    #[test]
    fn gateway_status_normalization_preserves_pending_observations() {
        assert_eq!(normalize_llm_status("pending", Some(200)), "pending");
        assert_eq!(normalize_llm_status("completed", None), "success");
        assert_eq!(normalize_llm_status("failed", Some(500)), "error");
    }

    #[test]
    fn ssh_lifecycle_guards_keep_active_and_terminal_counters_consistent() {
        let metrics = MetricsRegistry::new(MetricsConfig {
            enabled: false,
            db_path: std::env::temp_dir().join("unused-router-metrics.db"),
            retention_days: 7,
            sample_interval_secs: 5,
            alerting: AlertingSettings::default(),
        });

        let listener = metrics.forward_listener_started();
        assert_eq!(metrics.ssh_forward_listeners.load(Ordering::Relaxed), 1);
        drop(listener);
        assert_eq!(metrics.ssh_forward_listeners.load(Ordering::Relaxed), 0);
        assert_eq!(
            metrics
                .ssh_forward_listener_shutdown_total
                .load(Ordering::Relaxed),
            1
        );

        metrics
            .forward_bridge_started()
            .finish(ForwardBridgeMetricOutcome::Completed);
        assert_eq!(metrics.ssh_active_bridges.load(Ordering::Relaxed), 0);
        assert_eq!(
            metrics.ssh_bridge_completed_total.load(Ordering::Relaxed),
            1
        );

        let cancelled = metrics.forward_bridge_started();
        assert_eq!(metrics.ssh_active_bridges.load(Ordering::Relaxed), 1);
        drop(cancelled);
        assert_eq!(metrics.ssh_active_bridges.load(Ordering::Relaxed), 0);
        assert_eq!(
            metrics.ssh_bridge_cancelled_total.load(Ordering::Relaxed),
            1
        );

        metrics
            .forward_channel_open_started()
            .finish(ForwardChannelOpenMetricOutcome::Succeeded);
        let cancelled_open = metrics.forward_channel_open_started();
        assert_eq!(metrics.ssh_pending_channel_opens.load(Ordering::Relaxed), 1);
        drop(cancelled_open);
        assert_eq!(metrics.ssh_pending_channel_opens.load(Ordering::Relaxed), 0);
        assert_eq!(
            metrics
                .ssh_channel_open_succeeded_total
                .load(Ordering::Relaxed),
            1
        );
        assert_eq!(
            metrics
                .ssh_channel_open_cancelled_total
                .load(Ordering::Relaxed),
            1
        );

        metrics.forward_capacity_rejected();
        assert_eq!(
            metrics
                .ssh_forward_capacity_rejected_total
                .load(Ordering::Relaxed),
            1
        );
    }
}
