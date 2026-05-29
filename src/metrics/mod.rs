pub mod collector;
pub mod models;
pub mod store;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::config::{Config, MetricsConfig};
use crate::error::AppError;
use crate::models::{MarketRequestLogEntry, ShareRequestLogEntry};
use crate::proxy::ProxyRegistry;

use self::collector::{HostSampler, host_info};
use self::models::{
    HostMetricsInfo, HostMetricsStatus, LlmMetricsSnapshot, LlmRequestMetric, MetricEvent,
    MetricsHealth, MetricsSnapshot, RouterMetricsStatus,
};
use self::store::MetricsStore;

#[derive(Debug)]
pub struct MetricsRegistry {
    enabled: bool,
    sample_interval_secs: u64,
    store: MetricsStore,
    sampler: Mutex<HostSampler>,
    last_host: Mutex<Option<HostMetricsStatus>>,
    proxy_inflight: AtomicU64,
    proxy_requests_total: AtomicU64,
    proxy_upstream_errors_total: AtomicU64,
    proxy_5xx_total: AtomicU64,
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
}

impl MetricsRegistry {
    pub fn new(config: MetricsConfig) -> Arc<Self> {
        let sample_interval_secs = config.sample_interval_secs.max(1);
        Arc::new(Self {
            enabled: config.enabled,
            sample_interval_secs,
            store: MetricsStore::new(config.db_path, config.retention_days),
            sampler: Mutex::new(HostSampler::default()),
            last_host: Mutex::new(None),
            proxy_inflight: AtomicU64::new(0),
            proxy_requests_total: AtomicU64::new(0),
            proxy_upstream_errors_total: AtomicU64::new(0),
            proxy_5xx_total: AtomicU64::new(0),
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
    ) -> Result<(), AppError> {
        let host = self.current_host_status(config).await;
        let router = self.router_status(proxy).await;
        let llm = if self.enabled {
            self.store.llm_snapshot(5 * 60).await.unwrap_or_default()
        } else {
            LlmMetricsSnapshot::default()
        };
        if self.enabled {
            self.store.insert_sample(host.clone(), router.clone()).await?;
            let alerts = build_alerts(&host, &router, &llm);
            for event in alerts {
                if let Err(err) = self.store.insert_event_deduped(event).await {
                    debug!("persist metric alert failed: {err}");
                }
            }
        }
        Ok(())
    }

    pub async fn snapshot(
        &self,
        config: &Config,
        proxy: &ProxyRegistry,
    ) -> Result<MetricsSnapshot, AppError> {
        let host = self.current_host_status(config).await;
        let router = self.router_status(proxy).await;
        let mut llm = if self.enabled {
            self.store.llm_snapshot(5 * 60).await.unwrap_or_default()
        } else {
            LlmMetricsSnapshot::default()
        };
        llm.inflight = self.proxy_inflight.load(Ordering::Relaxed);
        let alerts = build_alerts(&host, &router, &llm);
        let status = alerts
            .iter()
            .fold(MetricsHealth::Healthy, |current, event| {
                match (current, event.severity.as_str()) {
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
            host,
            router,
            llm,
            alerts,
        })
    }

    pub async fn router_status(&self, proxy: &ProxyRegistry) -> RouterMetricsStatus {
        let counts = proxy.counts().await;
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
            proxy_inflight: self.proxy_inflight.load(Ordering::Relaxed),
            proxy_requests_total: self.proxy_requests_total.load(Ordering::Relaxed),
            proxy_upstream_errors_total: self.proxy_upstream_errors_total.load(Ordering::Relaxed),
            proxy_5xx_total: self.proxy_5xx_total.load(Ordering::Relaxed),
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

    pub fn forward_listener_started(&self) {
        self.ssh_forward_listeners.fetch_add(1, Ordering::Relaxed);
        self.ssh_forward_listener_created_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn forward_listener_shutdown(&self) {
        decrement(&self.ssh_forward_listeners);
        self.ssh_forward_listener_shutdown_total
            .fetch_add(1, Ordering::Relaxed);
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

    pub fn record_market_request_logs(
        self: &Arc<Self>,
        market_email: &str,
        logs: &[MarketRequestLogEntry],
    ) {
        for log in logs {
            self.record_llm_request(LlmRequestMetric {
                timestamp: parse_rfc3339_timestamp(&log.created_at)
                    .unwrap_or_else(|| chrono::Utc::now().timestamp()),
                request_id: Some(log.request_id.clone()),
                route_type: "market".into(),
                market_email: Some(market_email.to_string()),
                share_id: log.share_id.clone(),
                subdomain: log.share_subdomain.clone(),
                app_type: Some(log.request_agent.clone()).filter(|value| !value.is_empty()),
                provider: None,
                requested_model: Some(log.requested_model.clone())
                    .filter(|value| !value.is_empty()),
                actual_model: Some(log.actual_model.clone()).filter(|value| !value.is_empty()),
                status: normalize_llm_status(&log.status, log.status_code),
                error_kind: error_kind_from_status(&log.status, log.status_code),
                http_status: log.status_code,
                latency_ms: log.latency_ms,
                ttft_ms: None,
                stream_started: false,
                stream_completed: log.status == "settled" || log.status == "success",
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
                estimated_cost_usd: log
                    .usage_amount_usd
                    .as_deref()
                    .and_then(|value| value.parse::<f64>().ok()),
            });
        }
    }

    pub fn record_share_request_logs(self: &Arc<Self>, logs: &[ShareRequestLogEntry]) {
        for log in logs {
            if log.is_health_check {
                continue;
            }
            self.record_llm_request(LlmRequestMetric {
                timestamp: log.created_at,
                request_id: Some(log.request_id.clone()),
                route_type: "direct".into(),
                market_email: None,
                share_id: Some(log.share_id.clone()),
                subdomain: None,
                app_type: Some(log.app_type.clone()).filter(|value| !value.is_empty()),
                provider: Some(log.provider_name.clone()).filter(|value| !value.is_empty()),
                requested_model: Some(log.requested_model.clone())
                    .filter(|value| !value.is_empty()),
                actual_model: Some(log.actual_model.clone()).filter(|value| !value.is_empty()),
                status: if log.status_code < 400 {
                    "success"
                } else {
                    "error"
                }
                .into(),
                error_kind: error_kind_from_status("", Some(log.status_code)),
                http_status: Some(log.status_code),
                latency_ms: Some(log.latency_ms),
                ttft_ms: log.first_token_ms,
                stream_started: log.is_streaming,
                stream_completed: log.status_code < 400,
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
        if let Err(err) = metrics.sample_and_store(&config, &proxy).await {
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

fn build_alerts(
    host: &HostMetricsStatus,
    router: &RouterMetricsStatus,
    llm: &LlmMetricsSnapshot,
) -> Vec<MetricEvent> {
    let now = host.timestamp;
    let mut events = Vec::new();
    if let Some(fd) = host.process.fd_usage_percent {
        if fd >= 85.0 {
            events.push(event(
                now,
                "critical",
                "fd_pressure",
                "FD usage is critical",
                serde_json::json!({ "fdUsagePercent": fd }),
            ));
        } else if fd >= 70.0 {
            events.push(event(
                now,
                "warning",
                "fd_pressure",
                "FD usage is elevated",
                serde_json::json!({ "fdUsagePercent": fd }),
            ));
        }
    }
    if router.ssh_forward_listeners > router.active_routes + 2 {
        events.push(event(
            now,
            "critical",
            "route_lifecycle",
            "Forward listeners exceed active routes",
            serde_json::json!({
                "forwardListeners": router.ssh_forward_listeners,
                "activeRoutes": router.active_routes,
            }),
        ));
    }
    if router.db_errors_total > 0 {
        events.push(event(
            now,
            "warning",
            "db_error",
            "Metrics observed DB errors",
            serde_json::json!({ "dbErrorsTotal": router.db_errors_total }),
        ));
    }
    if llm.error_rate >= 0.10 {
        events.push(event(
            now,
            "warning",
            "llm_error_rate",
            "LLM error rate is elevated",
            serde_json::json!({ "errorRate": llm.error_rate }),
        ));
    }
    if llm.rate_limit_per_minute >= 5.0 {
        events.push(event(
            now,
            "warning",
            "llm_rate_limit",
            "LLM rate limits increased",
            serde_json::json!({ "rateLimitPerMinute": llm.rate_limit_per_minute }),
        ));
    }
    events
}

fn event(
    timestamp: i64,
    severity: &str,
    kind: &str,
    message: &str,
    details: serde_json::Value,
) -> MetricEvent {
    MetricEvent {
        id: None,
        timestamp,
        severity: severity.into(),
        kind: kind.into(),
        message: message.into(),
        details,
    }
}

fn parse_rfc3339_timestamp(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.timestamp())
}

fn normalize_llm_status(status: &str, status_code: Option<u16>) -> String {
    if matches!(status, "settled" | "success") || status_code.is_some_and(|code| code < 400) {
        "success".into()
    } else {
        "error".into()
    }
}

fn error_kind_from_status(status: &str, status_code: Option<u16>) -> Option<String> {
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
