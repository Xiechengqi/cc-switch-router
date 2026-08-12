use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::{Connection, OptionalExtension, params};
use tokio::task::spawn_blocking;

use crate::clock_health::ClockHealthStatus;
use crate::error::AppError;

use super::models::{
    ClearMetricsResponse, ClientMetricsPoint, ClientMetricsSnapshot, ClockMetricsPoint,
    HostMetricsPoint, HostMetricsStatus, LlmMetricsPoint, LlmRequestMetric, LlmTopItem,
    LlmTopResponse, MetricEvent, MetricsSeriesResponse, RouterMetricsPoint, RouterMetricsStatus,
};

#[derive(Debug, Clone)]
pub struct MetricsStore {
    path: PathBuf,
    retention_days: u32,
    initialized: Arc<AtomicBool>,
}

impl MetricsStore {
    pub fn new(path: PathBuf, retention_days: u32) -> Self {
        Self {
            path,
            retention_days,
            initialized: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Opens a connection, running the schema bootstrap only on the first call.
    fn open(&self) -> Result<Connection, AppError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                AppError::Internal(format!("create metrics db dir failed: {err}"))
            })?;
        }
        let conn = Connection::open(&self.path)
            .map_err(|err| AppError::Internal(format!("open metrics db failed: {err}")))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|err| AppError::Internal(format!("configure metrics db failed: {err}")))?;
        if !self.initialized.load(Ordering::Acquire) {
            init_metrics_db(&conn)?;
            self.initialized.store(true, Ordering::Release);
        }
        Ok(conn)
    }

    pub async fn init(&self) -> Result<(), AppError> {
        let store = self.clone();
        spawn_blocking(move || store.open().map(|_| ()))
            .await
            .map_err(|err| AppError::Internal(format!("metrics init task failed: {err}")))?
    }

    pub async fn insert_sample(
        &self,
        host: HostMetricsStatus,
        router: RouterMetricsStatus,
        clients: ClientMetricsSnapshot,
    ) -> Result<(), AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let mut conn = store.open()?;
            insert_metrics_sample(&mut conn, &host, &router, &clients)
        })
        .await
        .map_err(|err| AppError::Internal(format!("metrics sample task failed: {err}")))?
    }

    pub async fn insert_clock_sample(&self, sample: ClockHealthStatus) -> Result<(), AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let Some(timestamp) = sample.sampled_at else {
                return Ok(());
            };
            let conn = store.open()?;
            conn.execute(
                "INSERT INTO clock_metrics (
                    timestamp, offset_ms, uncertainty_ms, status, confidence,
                    valid_sources, total_sources, ntp_synchronized
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    timestamp,
                    sample.offset_ms,
                    sample.uncertainty_ms.map(|value| value as i64),
                    sample.status,
                    sample.confidence,
                    sample.valid_sources as i64,
                    sample.total_sources as i64,
                    sample.ntp_synchronized.map(i64::from),
                ],
            )
            .map_err(|error| AppError::Internal(format!("insert clock metrics failed: {error}")))?;
            Ok(())
        })
        .await
        .map_err(|error| AppError::Internal(format!("clock metrics task failed: {error}")))?
    }

    pub async fn prune(&self) -> Result<(), AppError> {
        let store = self.clone();
        let retention_days = self.retention_days;
        spawn_blocking(move || {
            let conn = store.open()?;
            prune_old_metrics(&conn, chrono::Utc::now().timestamp(), retention_days)
        })
        .await
        .map_err(|err| AppError::Internal(format!("metrics prune task failed: {err}")))?
    }

    pub async fn insert_llm_request(&self, metric: LlmRequestMetric) -> Result<(), AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            insert_llm_request_metric(&conn, &metric)
        })
        .await
        .map_err(|err| AppError::Internal(format!("llm metrics task failed: {err}")))?
    }

    pub async fn insert_event(&self, event: MetricEvent) -> Result<(), AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            conn.execute(
                "INSERT INTO metric_events (timestamp, severity, kind, message, details_json)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    event.timestamp,
                    event.severity,
                    event.kind,
                    event.message,
                    event.details.to_string(),
                ],
            )
            .map_err(|err| AppError::Internal(format!("insert metric event failed: {err}")))?;
            Ok(())
        })
        .await
        .map_err(|err| AppError::Internal(format!("metric event task failed: {err}")))?
    }

    /// Persists an alert only when the same `(kind, severity)` pair has either
    /// never been seen or was last seen long enough ago to count as a new
    /// incident. This is how the live alerts returned from `build_alerts` flow
    /// into the persisted `metric_events` table without spamming on every
    /// sample tick.
    pub async fn insert_event_deduped(&self, event: MetricEvent) -> Result<(), AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            let cooldown_secs: i64 = 5 * 60;
            let recent: Option<i64> = conn
                .query_row(
                    "SELECT MAX(timestamp) FROM metric_events
                      WHERE kind = ?1 AND severity = ?2 AND timestamp >= ?3",
                    params![event.kind, event.severity, event.timestamp - cooldown_secs],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|err| AppError::Internal(format!("dedupe metric event failed: {err}")))?
                .flatten();
            if recent.is_some() {
                return Ok(());
            }
            conn.execute(
                "INSERT INTO metric_events (timestamp, severity, kind, message, details_json)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    event.timestamp,
                    event.severity,
                    event.kind,
                    event.message,
                    event.details.to_string(),
                ],
            )
            .map_err(|err| AppError::Internal(format!("insert metric event failed: {err}")))?;
            Ok(())
        })
        .await
        .map_err(|err| AppError::Internal(format!("metric event task failed: {err}")))?
    }

    pub async fn latest_sample_timestamp(&self) -> Result<Option<i64>, AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            let value: Option<i64> = conn
                .query_row("SELECT MAX(timestamp) FROM host_metrics", [], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(|err| {
                    AppError::Internal(format!("load latest sample timestamp failed: {err}"))
                })?
                .flatten();
            Ok(value)
        })
        .await
        .map_err(|err| AppError::Internal(format!("latest sample task failed: {err}")))?
    }

    pub async fn latest_host_status(&self) -> Result<Option<HostMetricsStatus>, AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            latest_host_status(&conn)
        })
        .await
        .map_err(|err| AppError::Internal(format!("latest metrics task failed: {err}")))?
    }

    pub async fn series(
        &self,
        range_label: String,
        step_label: String,
    ) -> Result<MetricsSeriesResponse, AppError> {
        let range_secs = parse_duration_to_secs(&range_label)
            .ok_or_else(|| AppError::BadRequest("invalid metrics range".into()))?;
        let step_secs = parse_duration_to_secs(&step_label)
            .ok_or_else(|| AppError::BadRequest("invalid metrics step".into()))?;
        if step_secs <= 0 || range_secs <= 0 || step_secs > range_secs {
            return Err(AppError::BadRequest("invalid metrics range or step".into()));
        }
        let store = self.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            let end_ts = chrono::Utc::now().timestamp();
            let start_ts = end_ts - range_secs;
            Ok(MetricsSeriesResponse {
                range: range_label,
                step: step_label,
                clock: load_clock_series(&conn, start_ts, end_ts, step_secs)?,
                host: load_host_series(&conn, start_ts, end_ts, step_secs)?,
                router: load_router_series(&conn, start_ts, end_ts, step_secs)?,
                clients: load_client_series(&conn, start_ts, end_ts, step_secs)?,
                llm: load_llm_series(&conn, start_ts, end_ts, step_secs)?,
            })
        })
        .await
        .map_err(|err| AppError::Internal(format!("metrics series task failed: {err}")))?
    }

    pub async fn llm_snapshot(
        &self,
        range_secs: i64,
    ) -> Result<super::models::LlmMetricsSnapshot, AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            load_llm_snapshot(&conn, range_secs)
        })
        .await
        .map_err(|err| AppError::Internal(format!("llm snapshot task failed: {err}")))?
    }

    pub async fn llm_top(
        &self,
        range_label: String,
        by: String,
        limit: usize,
    ) -> Result<LlmTopResponse, AppError> {
        let range_secs = parse_duration_to_secs(&range_label)
            .ok_or_else(|| AppError::BadRequest("invalid metrics range".into()))?;
        let store = self.clone();
        let by_for_query = by.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            let end_ts = chrono::Utc::now().timestamp();
            let start_ts = end_ts - range_secs;
            Ok(LlmTopResponse {
                range: range_label,
                by: by_for_query.clone(),
                items: load_llm_top(&conn, start_ts, by_for_query.as_str(), limit)?,
            })
        })
        .await
        .map_err(|err| AppError::Internal(format!("llm top task failed: {err}")))?
    }

    pub async fn events(&self, limit: usize) -> Result<Vec<MetricEvent>, AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            load_events(&conn, limit)
        })
        .await
        .map_err(|err| AppError::Internal(format!("metric events task failed: {err}")))?
    }

    pub async fn llm_reliability(
        &self,
        range_label: String,
        limit: usize,
    ) -> Result<super::models::LlmReliabilityResponse, AppError> {
        let range_secs = parse_duration_to_secs(&range_label)
            .ok_or_else(|| AppError::BadRequest("invalid metrics range".into()))?;
        let store = self.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            let end_ts = chrono::Utc::now().timestamp();
            let start_ts = end_ts - range_secs;
            let (total, substituted, success_rate, items) =
                load_llm_reliability(&conn, start_ts, end_ts, limit)?;
            Ok(super::models::LlmReliabilityResponse {
                range: range_label,
                total_requests: total,
                substituted_requests: substituted,
                substitution_rate: if total > 0 {
                    substituted as f64 / total as f64
                } else {
                    0.0
                },
                substitution_success_rate: success_rate,
                items,
            })
        })
        .await
        .map_err(|err| AppError::Internal(format!("llm reliability task failed: {err}")))?
    }

    pub async fn clear(&self) -> Result<ClearMetricsResponse, AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            let mut deleted_rows = HashMap::new();
            for table in [
                "clock_metrics",
                "host_metrics",
                "router_metrics",
                "client_metrics",
                "llm_request_metrics",
                "metric_events",
            ] {
                let deleted = conn
                    .execute(&format!("DELETE FROM {table}"), [])
                    .map_err(|err| {
                        AppError::Internal(format!("clear metrics table {table} failed: {err}"))
                    })?;
                deleted_rows.insert(table.to_string(), deleted as u64);
            }
            Ok(ClearMetricsResponse {
                ok: true,
                deleted_rows,
            })
        })
        .await
        .map_err(|err| AppError::Internal(format!("clear metrics task failed: {err}")))?
    }
}

pub fn parse_duration_to_secs(input: &str) -> Option<i64> {
    let trimmed = input.trim();
    if trimmed.len() < 2 {
        return None;
    }
    let (value, unit) = trimmed.split_at(trimmed.len() - 1);
    let value = value.parse::<i64>().ok()?;
    match unit {
        "s" => Some(value),
        "m" => Some(value * 60),
        "h" => Some(value * 3600),
        "d" => Some(value * 86400),
        _ => None,
    }
}

pub fn default_step_label(range_secs: i64) -> String {
    if range_secs <= 15 * 60 {
        "15s".into()
    } else if range_secs <= 3600 {
        "30s".into()
    } else if range_secs <= 6 * 3600 {
        "1m".into()
    } else if range_secs <= 24 * 3600 {
        "5m".into()
    } else {
        "15m".into()
    }
}

fn init_metrics_db(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS host_metrics (
            timestamp INTEGER NOT NULL,
            cpu_percent REAL,
            load_1 REAL,
            load_5 REAL,
            load_15 REAL,
            memory_used_bytes INTEGER,
            memory_total_bytes INTEGER,
            memory_available_bytes INTEGER,
            swap_used_bytes INTEGER,
            swap_total_bytes INTEGER,
            disk_used_bytes INTEGER,
            disk_total_bytes INTEGER,
            rx_bytes_per_sec REAL,
            tx_bytes_per_sec REAL,
            tcp_established INTEGER,
            tcp_time_wait INTEGER,
            process_open_fds INTEGER,
            process_max_fds INTEGER,
            process_fd_usage_percent REAL,
            process_threads INTEGER,
            process_rss_bytes INTEGER,
            process_cpu_percent REAL,
            uptime_secs INTEGER,
            process_uptime_secs INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_host_metrics_ts ON host_metrics(timestamp);

        CREATE TABLE IF NOT EXISTS clock_metrics (
            timestamp INTEGER NOT NULL,
            offset_ms INTEGER,
            uncertainty_ms INTEGER,
            status TEXT NOT NULL,
            confidence TEXT NOT NULL,
            valid_sources INTEGER NOT NULL,
            total_sources INTEGER NOT NULL,
            ntp_synchronized INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_clock_metrics_ts ON clock_metrics(timestamp);

        CREATE TABLE IF NOT EXISTS router_metrics (
            timestamp INTEGER NOT NULL,
            active_routes INTEGER NOT NULL,
            pending_routes INTEGER NOT NULL,
            health_probe_failure_cache INTEGER NOT NULL,
            ssh_active_sessions INTEGER NOT NULL,
            ssh_forward_listeners INTEGER NOT NULL,
            ssh_forward_listener_created_total INTEGER NOT NULL,
            ssh_forward_listener_shutdown_total INTEGER NOT NULL,
            ssh_forward_bind_errors_total INTEGER NOT NULL,
            ssh_forward_accept_errors_total INTEGER NOT NULL,
            ssh_forward_emfile_errors_total INTEGER NOT NULL,
            ssh_pending_channel_opens INTEGER NOT NULL,
            ssh_channel_open_started_total INTEGER NOT NULL,
            ssh_channel_open_succeeded_total INTEGER NOT NULL,
            ssh_channel_open_explicit_failures_total INTEGER NOT NULL,
            ssh_channel_open_timeout_total INTEGER NOT NULL,
            ssh_channel_open_session_errors_total INTEGER NOT NULL,
            ssh_channel_open_cancelled_total INTEGER NOT NULL,
            ssh_active_bridges INTEGER NOT NULL,
            ssh_bridge_created_total INTEGER NOT NULL,
            ssh_bridge_completed_total INTEGER NOT NULL,
            ssh_bridge_cancelled_total INTEGER NOT NULL,
            ssh_bridge_write_stall_total INTEGER NOT NULL,
            ssh_bridge_half_close_idle_total INTEGER NOT NULL,
            ssh_bridge_io_errors_total INTEGER NOT NULL,
            ssh_forward_capacity_rejected_total INTEGER NOT NULL,
            proxy_inflight INTEGER NOT NULL,
            proxy_requests_total INTEGER NOT NULL,
            proxy_upstream_errors_total INTEGER NOT NULL,
            proxy_5xx_total INTEGER NOT NULL,
            health_probe_failures_total INTEGER NOT NULL,
            health_probe_cached_failures_total INTEGER NOT NULL,
            db_errors_total INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_router_metrics_ts ON router_metrics(timestamp);

        CREATE TABLE IF NOT EXISTS client_metrics (
            timestamp INTEGER NOT NULL,
            total INTEGER NOT NULL,
            monitored INTEGER NOT NULL,
            online INTEGER NOT NULL,
            recovering INTEGER NOT NULL,
            offline INTEGER NOT NULL,
            unknown_count INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_client_metrics_ts ON client_metrics(timestamp);

        CREATE TABLE IF NOT EXISTS llm_request_metrics (
            timestamp INTEGER NOT NULL,
            request_id TEXT,
            route_type TEXT NOT NULL,
            market_email TEXT,
            share_id TEXT,
            subdomain TEXT,
            app_type TEXT,
            provider TEXT,
            requested_model TEXT,
            actual_model TEXT,
            status TEXT NOT NULL,
            error_kind TEXT,
            http_status INTEGER,
            latency_ms INTEGER,
            ttft_ms INTEGER,
            stream_started INTEGER NOT NULL DEFAULT 0,
            stream_completed INTEGER NOT NULL DEFAULT 0,
            usage_state TEXT,
            stream_status TEXT,
            usage_revision INTEGER NOT NULL DEFAULT 0,
            input_tokens INTEGER,
            output_tokens INTEGER,
            total_tokens INTEGER,
            cache_read_tokens INTEGER,
            cache_write_tokens INTEGER,
            reasoning_tokens INTEGER,
            estimated_cost_usd REAL
        );
        CREATE INDEX IF NOT EXISTS idx_llm_request_metrics_ts ON llm_request_metrics(timestamp);
        CREATE INDEX IF NOT EXISTS idx_llm_request_metrics_share_ts ON llm_request_metrics(share_id, timestamp);
        CREATE INDEX IF NOT EXISTS idx_llm_request_metrics_market_ts ON llm_request_metrics(market_email, timestamp);
        CREATE INDEX IF NOT EXISTS idx_llm_request_metrics_model_ts ON llm_request_metrics(actual_model, timestamp);
        DELETE FROM llm_request_metrics
        WHERE request_id IS NOT NULL
          AND rowid NOT IN (
            SELECT MAX(rowid) FROM llm_request_metrics
            WHERE request_id IS NOT NULL
            GROUP BY request_id
          );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_llm_request_metrics_request_id
            ON llm_request_metrics(request_id);

        CREATE TABLE IF NOT EXISTS metric_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp INTEGER NOT NULL,
            severity TEXT NOT NULL,
            kind TEXT NOT NULL,
            message TEXT NOT NULL,
            details_json TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_metric_events_ts ON metric_events(timestamp);
        ",
    )
    .map_err(|err| AppError::Internal(format!("init metrics db failed: {err}")))?;
    Ok(())
}

fn insert_host_metrics(conn: &Connection, host: &HostMetricsStatus) -> Result<(), AppError> {
    let primary_disk = host.disks.first();
    conn.execute(
        "INSERT INTO host_metrics (
            timestamp, cpu_percent, load_1, load_5, load_15,
            memory_used_bytes, memory_total_bytes, memory_available_bytes,
            swap_used_bytes, swap_total_bytes, disk_used_bytes, disk_total_bytes,
            rx_bytes_per_sec, tx_bytes_per_sec, tcp_established, tcp_time_wait,
            process_open_fds, process_max_fds, process_fd_usage_percent,
            process_threads, process_rss_bytes, process_cpu_percent,
            uptime_secs, process_uptime_secs
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
        params![
            host.timestamp,
            host.cpu_percent,
            host.load_1,
            host.load_5,
            host.load_15,
            host.memory_used_bytes.map(|v| v as i64),
            host.memory_total_bytes.map(|v| v as i64),
            host.memory_available_bytes.map(|v| v as i64),
            host.swap_used_bytes.map(|v| v as i64),
            host.swap_total_bytes.map(|v| v as i64),
            primary_disk.map(|d| d.used_bytes as i64),
            primary_disk.map(|d| d.total_bytes as i64),
            host.network.rx_bytes_per_sec,
            host.network.tx_bytes_per_sec,
            host.network.tcp_established.map(|v| v as i64),
            host.network.tcp_time_wait.map(|v| v as i64),
            host.process.open_fds.map(|v| v as i64),
            host.process.max_fds.map(|v| v as i64),
            host.process.fd_usage_percent,
            host.process.threads.map(|v| v as i64),
            host.process.rss_bytes.map(|v| v as i64),
            host.process.cpu_percent,
            host.uptime_secs.map(|v| v as i64),
            host.process.uptime_secs.map(|v| v as i64),
        ],
    )
    .map_err(|err| AppError::Internal(format!("insert host metrics failed: {err}")))?;
    Ok(())
}

fn insert_router_metrics(
    conn: &Connection,
    timestamp: i64,
    router: &RouterMetricsStatus,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO router_metrics (
            timestamp, active_routes, pending_routes, health_probe_failure_cache,
            ssh_active_sessions, ssh_forward_listeners, ssh_forward_listener_created_total,
            ssh_forward_listener_shutdown_total, ssh_forward_bind_errors_total,
            ssh_forward_accept_errors_total, ssh_forward_emfile_errors_total,
            ssh_pending_channel_opens, ssh_channel_open_started_total,
            ssh_channel_open_succeeded_total, ssh_channel_open_explicit_failures_total,
            ssh_channel_open_timeout_total, ssh_channel_open_session_errors_total,
            ssh_channel_open_cancelled_total,
            ssh_active_bridges, ssh_bridge_created_total, ssh_bridge_completed_total,
            ssh_bridge_cancelled_total, ssh_bridge_write_stall_total,
            ssh_bridge_half_close_idle_total, ssh_bridge_io_errors_total,
            ssh_forward_capacity_rejected_total,
            proxy_inflight, proxy_requests_total, proxy_upstream_errors_total, proxy_5xx_total,
            health_probe_failures_total, health_probe_cached_failures_total, db_errors_total
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33)",
        params![
            timestamp,
            router.active_routes as i64,
            router.pending_routes as i64,
            router.health_probe_failure_cache as i64,
            router.ssh_active_sessions as i64,
            router.ssh_forward_listeners as i64,
            router.ssh_forward_listener_created_total as i64,
            router.ssh_forward_listener_shutdown_total as i64,
            router.ssh_forward_bind_errors_total as i64,
            router.ssh_forward_accept_errors_total as i64,
            router.ssh_forward_emfile_errors_total as i64,
            router.ssh_pending_channel_opens as i64,
            router.ssh_channel_open_started_total as i64,
            router.ssh_channel_open_succeeded_total as i64,
            router.ssh_channel_open_explicit_failures_total as i64,
            router.ssh_channel_open_timeout_total as i64,
            router.ssh_channel_open_session_errors_total as i64,
            router.ssh_channel_open_cancelled_total as i64,
            router.ssh_active_bridges as i64,
            router.ssh_bridge_created_total as i64,
            router.ssh_bridge_completed_total as i64,
            router.ssh_bridge_cancelled_total as i64,
            router.ssh_bridge_write_stall_total as i64,
            router.ssh_bridge_half_close_idle_total as i64,
            router.ssh_bridge_io_errors_total as i64,
            router.ssh_forward_capacity_rejected_total as i64,
            router.proxy_inflight as i64,
            router.proxy_requests_total as i64,
            router.proxy_upstream_errors_total as i64,
            router.proxy_5xx_total as i64,
            router.health_probe_failures_total as i64,
            router.health_probe_cached_failures_total as i64,
            router.db_errors_total as i64,
        ],
    )
    .map_err(|err| AppError::Internal(format!("insert router metrics failed: {err}")))?;
    Ok(())
}

fn insert_metrics_sample(
    conn: &mut Connection,
    host: &HostMetricsStatus,
    router: &RouterMetricsStatus,
    clients: &ClientMetricsSnapshot,
) -> Result<(), AppError> {
    let transaction = conn.transaction().map_err(|error| {
        AppError::Internal(format!("begin metrics sample transaction failed: {error}"))
    })?;
    insert_host_metrics(&transaction, host)?;
    insert_router_metrics(&transaction, host.timestamp, router)?;
    insert_client_metrics(&transaction, clients)?;
    transaction.commit().map_err(|error| {
        AppError::Internal(format!("commit metrics sample transaction failed: {error}"))
    })?;
    Ok(())
}

fn insert_client_metrics(
    conn: &Connection,
    clients: &ClientMetricsSnapshot,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO client_metrics (
            timestamp, total, monitored, online, recovering, offline, unknown_count
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            clients.timestamp,
            clients.total as i64,
            clients.monitored as i64,
            clients.online as i64,
            clients.recovering as i64,
            clients.offline as i64,
            clients.unknown as i64,
        ],
    )
    .map_err(|error| AppError::Internal(format!("insert Client metrics failed: {error}")))?;
    Ok(())
}

fn insert_llm_request_metric(conn: &Connection, metric: &LlmRequestMetric) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO llm_request_metrics (
            timestamp, request_id, route_type, market_email, share_id, subdomain,
            app_type, provider, requested_model, actual_model, status, error_kind,
            http_status, latency_ms, ttft_ms, stream_started, stream_completed,
            usage_state, stream_status, usage_revision,
            input_tokens, output_tokens, total_tokens, cache_read_tokens, cache_write_tokens,
            reasoning_tokens, estimated_cost_usd
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27)
        ON CONFLICT(request_id) DO UPDATE SET
            timestamp = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN excluded.timestamp ELSE llm_request_metrics.timestamp END,
            route_type = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN excluded.route_type ELSE llm_request_metrics.route_type END,
            market_email = COALESCE(llm_request_metrics.market_email, excluded.market_email),
            share_id = COALESCE(llm_request_metrics.share_id, excluded.share_id),
            subdomain = COALESCE(llm_request_metrics.subdomain, excluded.subdomain),
            app_type = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN COALESCE(excluded.app_type, llm_request_metrics.app_type) ELSE llm_request_metrics.app_type END,
            provider = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN COALESCE(excluded.provider, llm_request_metrics.provider) ELSE llm_request_metrics.provider END,
            requested_model = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN COALESCE(excluded.requested_model, llm_request_metrics.requested_model) ELSE llm_request_metrics.requested_model END,
            actual_model = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN COALESCE(excluded.actual_model, llm_request_metrics.actual_model) ELSE llm_request_metrics.actual_model END,
            status = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN excluded.status ELSE llm_request_metrics.status END,
            error_kind = CASE
                WHEN excluded.usage_revision < llm_request_metrics.usage_revision
                    THEN llm_request_metrics.error_kind
                WHEN excluded.error_kind = 'concurrency_limited'
                    THEN excluded.error_kind
                WHEN llm_request_metrics.error_kind = 'concurrency_limited'
                    THEN llm_request_metrics.error_kind
                ELSE COALESCE(excluded.error_kind, llm_request_metrics.error_kind)
            END,
            http_status = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN COALESCE(excluded.http_status, llm_request_metrics.http_status) ELSE llm_request_metrics.http_status END,
            latency_ms = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN COALESCE(excluded.latency_ms, llm_request_metrics.latency_ms) ELSE llm_request_metrics.latency_ms END,
            ttft_ms = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN COALESCE(excluded.ttft_ms, llm_request_metrics.ttft_ms) ELSE llm_request_metrics.ttft_ms END,
            stream_started = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN excluded.stream_started ELSE llm_request_metrics.stream_started END,
            stream_completed = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN excluded.stream_completed ELSE llm_request_metrics.stream_completed END,
            usage_state = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN COALESCE(excluded.usage_state, llm_request_metrics.usage_state) ELSE llm_request_metrics.usage_state END,
            stream_status = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN COALESCE(excluded.stream_status, llm_request_metrics.stream_status) ELSE llm_request_metrics.stream_status END,
            usage_revision = MAX(llm_request_metrics.usage_revision, excluded.usage_revision),
            input_tokens = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN COALESCE(excluded.input_tokens, llm_request_metrics.input_tokens) ELSE llm_request_metrics.input_tokens END,
            output_tokens = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN COALESCE(excluded.output_tokens, llm_request_metrics.output_tokens) ELSE llm_request_metrics.output_tokens END,
            total_tokens = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN COALESCE(excluded.total_tokens, llm_request_metrics.total_tokens) ELSE llm_request_metrics.total_tokens END,
            cache_read_tokens = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN COALESCE(excluded.cache_read_tokens, llm_request_metrics.cache_read_tokens) ELSE llm_request_metrics.cache_read_tokens END,
            cache_write_tokens = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN COALESCE(excluded.cache_write_tokens, llm_request_metrics.cache_write_tokens) ELSE llm_request_metrics.cache_write_tokens END,
            reasoning_tokens = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN COALESCE(excluded.reasoning_tokens, llm_request_metrics.reasoning_tokens) ELSE llm_request_metrics.reasoning_tokens END,
            estimated_cost_usd = CASE WHEN excluded.usage_revision >= llm_request_metrics.usage_revision THEN COALESCE(excluded.estimated_cost_usd, llm_request_metrics.estimated_cost_usd) ELSE llm_request_metrics.estimated_cost_usd END",
        params![
            metric.timestamp,
            metric.request_id,
            metric.route_type,
            metric.market_email,
            metric.share_id,
            metric.subdomain,
            metric.app_type,
            metric.provider,
            metric.requested_model,
            metric.actual_model,
            metric.status,
            metric.error_kind,
            metric.http_status.map(i64::from),
            metric.latency_ms.map(|v| v as i64),
            metric.ttft_ms.map(|v| v as i64),
            i64::from(metric.stream_started as u8),
            i64::from(metric.stream_completed as u8),
            metric.usage_state,
            metric.stream_status,
            metric.usage_revision.min(i64::MAX as u64) as i64,
            metric.input_tokens.map(|v| v as i64),
            metric.output_tokens.map(|v| v as i64),
            metric.total_tokens.map(|v| v as i64),
            metric.cache_read_tokens.map(|v| v as i64),
            metric.cache_write_tokens.map(|v| v as i64),
            metric.reasoning_tokens.map(|v| v as i64),
            metric.estimated_cost_usd,
        ],
    )
    .map_err(|err| AppError::Internal(format!("insert llm request metric failed: {err}")))?;
    Ok(())
}

fn prune_old_metrics(conn: &Connection, now_ts: i64, retention_days: u32) -> Result<(), AppError> {
    if retention_days == 0 {
        return Ok(());
    }
    let cutoff = now_ts - retention_days as i64 * 86_400;
    for (table, column) in [
        ("clock_metrics", "timestamp"),
        ("host_metrics", "timestamp"),
        ("router_metrics", "timestamp"),
        ("client_metrics", "timestamp"),
        ("llm_request_metrics", "timestamp"),
        ("metric_events", "timestamp"),
    ] {
        conn.execute(
            &format!("DELETE FROM {table} WHERE {column} < ?1"),
            params![cutoff],
        )
        .map_err(|err| AppError::Internal(format!("prune {table} failed: {err}")))?;
    }
    Ok(())
}

fn latest_host_status(conn: &Connection) -> Result<Option<HostMetricsStatus>, AppError> {
    conn.query_row(
        "SELECT timestamp, cpu_percent, load_1, load_5, load_15,
                memory_used_bytes, memory_total_bytes, memory_available_bytes,
                swap_used_bytes, swap_total_bytes, disk_used_bytes, disk_total_bytes,
                rx_bytes_per_sec, tx_bytes_per_sec, tcp_established, tcp_time_wait,
                process_open_fds, process_max_fds, process_fd_usage_percent,
                process_threads, process_rss_bytes, process_cpu_percent,
                uptime_secs, process_uptime_secs
           FROM host_metrics ORDER BY timestamp DESC LIMIT 1",
        [],
        |row| {
            let disk_used = row.get::<_, Option<i64>>(10)?.unwrap_or_default() as u64;
            let disk_total = row.get::<_, Option<i64>>(11)?.unwrap_or_default() as u64;
            Ok(HostMetricsStatus {
                timestamp: row.get(0)?,
                uptime_secs: opt_i64_to_u64(row.get(22)?),
                cpu_percent: row.get(1)?,
                load_1: row.get(2)?,
                load_5: row.get(3)?,
                load_15: row.get(4)?,
                memory_used_bytes: opt_i64_to_u64(row.get(5)?),
                memory_total_bytes: opt_i64_to_u64(row.get(6)?),
                memory_available_bytes: opt_i64_to_u64(row.get(7)?),
                swap_used_bytes: opt_i64_to_u64(row.get(8)?),
                swap_total_bytes: opt_i64_to_u64(row.get(9)?),
                disks: vec![super::models::DiskUsage {
                    label: "root".into(),
                    mount_point: "/".into(),
                    used_bytes: disk_used,
                    total_bytes: disk_total,
                }],
                network: super::models::NetworkMetricsStatus {
                    rx_bytes_per_sec: row.get(12)?,
                    tx_bytes_per_sec: row.get(13)?,
                    tcp_established: opt_i64_to_u64(row.get(14)?),
                    tcp_time_wait: opt_i64_to_u64(row.get(15)?),
                },
                process: super::models::ProcessMetricsStatus {
                    open_fds: opt_i64_to_u64(row.get(16)?),
                    max_fds: opt_i64_to_u64(row.get(17)?),
                    fd_usage_percent: row.get(18)?,
                    threads: opt_i64_to_u64(row.get(19)?),
                    rss_bytes: opt_i64_to_u64(row.get(20)?),
                    cpu_percent: row.get(21)?,
                    uptime_secs: opt_i64_to_u64(row.get(23)?),
                },
            })
        },
    )
    .optional()
    .map_err(|err| AppError::Internal(format!("load latest host metrics failed: {err}")))
}

fn opt_i64_to_u64(value: Option<i64>) -> Option<u64> {
    value.and_then(|v| u64::try_from(v).ok())
}

fn opt_f64_to_u64(value: Option<f64>) -> Option<u64> {
    let value = value?;
    if !value.is_finite() || value < 0.0 || value > u64::MAX as f64 {
        return None;
    }
    Some(value.round() as u64)
}

fn load_clock_series(
    conn: &Connection,
    start_ts: i64,
    end_ts: i64,
    step_secs: i64,
) -> Result<Vec<ClockMetricsPoint>, AppError> {
    let mut statement = conn
        .prepare(
            "SELECT
                (timestamp / ?1) * ?1 AS bucket_ts,
                AVG(offset_ms),
                AVG(uncertainty_ms),
                AVG(valid_sources)
             FROM clock_metrics
             WHERE timestamp >= ?2 AND timestamp <= ?3
             GROUP BY bucket_ts
             ORDER BY bucket_ts ASC",
        )
        .map_err(|error| {
            AppError::Internal(format!("prepare clock metrics series failed: {error}"))
        })?;
    let rows = statement
        .query_map(params![step_secs, start_ts, end_ts], |row| {
            Ok(ClockMetricsPoint {
                timestamp: row.get(0)?,
                offset_ms: row.get(1)?,
                uncertainty_ms: row.get(2)?,
                valid_sources: row.get(3)?,
            })
        })
        .map_err(|error| {
            AppError::Internal(format!("query clock metrics series failed: {error}"))
        })?;
    let buckets = collect_rows(rows, "clock metrics series")?;
    Ok(fill_time_axis(
        start_ts,
        end_ts,
        step_secs,
        buckets,
        |point| point.timestamp,
        |timestamp| ClockMetricsPoint {
            timestamp,
            offset_ms: None,
            uncertainty_ms: None,
            valid_sources: None,
        },
    ))
}

fn load_host_series(
    conn: &Connection,
    start_ts: i64,
    end_ts: i64,
    step_secs: i64,
) -> Result<Vec<HostMetricsPoint>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT
            (timestamp / ?1) * ?1 AS bucket_ts,
            AVG(cpu_percent),
            AVG(CASE WHEN memory_total_bytes > 0 THEN memory_used_bytes * 100.0 / memory_total_bytes END),
            AVG(CASE WHEN disk_total_bytes > 0 THEN disk_used_bytes * 100.0 / disk_total_bytes END),
            AVG(process_fd_usage_percent),
            AVG(rx_bytes_per_sec),
            AVG(tx_bytes_per_sec),
            AVG(process_rss_bytes)
         FROM host_metrics
         WHERE timestamp >= ?2 AND timestamp <= ?3
         GROUP BY bucket_ts
         ORDER BY bucket_ts ASC",
    ).map_err(|err| AppError::Internal(format!("prepare host metrics series failed: {err}")))?;
    let rows = stmt
        .query_map(params![step_secs, start_ts, end_ts], |row| {
            Ok(HostMetricsPoint {
                timestamp: row.get(0)?,
                cpu_percent: row.get(1)?,
                memory_usage_percent: row.get(2)?,
                disk_usage_percent: row.get(3)?,
                fd_usage_percent: row.get(4)?,
                rx_bytes_per_sec: row.get(5)?,
                tx_bytes_per_sec: row.get(6)?,
                process_rss_bytes: opt_f64_to_u64(row.get(7)?),
            })
        })
        .map_err(|err| AppError::Internal(format!("query host metrics series failed: {err}")))?;
    let buckets = collect_rows(rows, "host metrics series")?;
    Ok(fill_time_axis(
        start_ts,
        end_ts,
        step_secs,
        buckets,
        |p| p.timestamp,
        |ts| HostMetricsPoint {
            timestamp: ts,
            cpu_percent: None,
            memory_usage_percent: None,
            disk_usage_percent: None,
            fd_usage_percent: None,
            rx_bytes_per_sec: None,
            tx_bytes_per_sec: None,
            process_rss_bytes: None,
        },
    ))
}

fn load_router_series(
    conn: &Connection,
    start_ts: i64,
    end_ts: i64,
    step_secs: i64,
) -> Result<Vec<RouterMetricsPoint>, AppError> {
    let mut stmt = conn
        .prepare(
            "WITH bucketed AS (
            SELECT *, (timestamp / ?1) * ?1 AS bucket_ts
              FROM router_metrics
             WHERE timestamp >= ?2 AND timestamp <= ?3
        ), latest AS (
            SELECT bucket_ts, MAX(timestamp) AS latest_ts FROM bucketed GROUP BY bucket_ts
        )
        SELECT b.bucket_ts, b.active_routes, b.ssh_forward_listeners, b.proxy_inflight,
               b.proxy_upstream_errors_total, b.health_probe_failures_total, b.db_errors_total
          FROM bucketed b
          JOIN latest l ON l.bucket_ts = b.bucket_ts AND l.latest_ts = b.timestamp
         ORDER BY b.bucket_ts ASC",
        )
        .map_err(|err| {
            AppError::Internal(format!("prepare router metrics series failed: {err}"))
        })?;
    let rows = stmt
        .query_map(params![step_secs, start_ts, end_ts], |row| {
            Ok(RouterMetricsPoint {
                timestamp: row.get(0)?,
                active_routes: row.get::<_, i64>(1)? as u64,
                forward_listeners: row.get::<_, i64>(2)? as u64,
                proxy_inflight: row.get::<_, i64>(3)? as u64,
                proxy_upstream_errors_total: row.get::<_, i64>(4)? as u64,
                health_probe_failures_total: row.get::<_, i64>(5)? as u64,
                db_errors_total: row.get::<_, i64>(6)? as u64,
            })
        })
        .map_err(|err| AppError::Internal(format!("query router metrics series failed: {err}")))?;
    let buckets = collect_rows(rows, "router metrics series")?;
    Ok(fill_time_axis(
        start_ts,
        end_ts,
        step_secs,
        buckets,
        |p| p.timestamp,
        |ts| RouterMetricsPoint {
            timestamp: ts,
            active_routes: 0,
            forward_listeners: 0,
            proxy_inflight: 0,
            proxy_upstream_errors_total: 0,
            health_probe_failures_total: 0,
            db_errors_total: 0,
        },
    ))
}

fn load_client_series(
    conn: &Connection,
    start_ts: i64,
    end_ts: i64,
    step_secs: i64,
) -> Result<Vec<ClientMetricsPoint>, AppError> {
    let mut statement = conn
        .prepare(
            "WITH bucketed AS (
                SELECT *, (timestamp / ?1) * ?1 AS bucket_ts
                FROM client_metrics
                WHERE timestamp >= ?2 AND timestamp <= ?3
             ), latest AS (
                SELECT bucket_ts, MAX(timestamp) AS latest_ts
                FROM bucketed GROUP BY bucket_ts
             )
             SELECT b.bucket_ts, b.total, b.online, b.recovering,
                    b.offline, b.unknown_count
             FROM bucketed b
             JOIN latest l ON l.bucket_ts = b.bucket_ts AND l.latest_ts = b.timestamp
             ORDER BY b.bucket_ts ASC",
        )
        .map_err(|error| {
            AppError::Internal(format!("prepare Client metrics series failed: {error}"))
        })?;
    let rows = statement
        .query_map(params![step_secs, start_ts, end_ts], |row| {
            Ok(ClientMetricsPoint {
                timestamp: row.get(0)?,
                total: row.get::<_, i64>(1)?.max(0) as u64,
                online: row.get::<_, i64>(2)?.max(0) as u64,
                recovering: row.get::<_, i64>(3)?.max(0) as u64,
                offline: row.get::<_, i64>(4)?.max(0) as u64,
                unknown: row.get::<_, i64>(5)?.max(0) as u64,
            })
        })
        .map_err(|error| {
            AppError::Internal(format!("query Client metrics series failed: {error}"))
        })?;
    let buckets = collect_rows(rows, "Client metrics series")?;
    Ok(fill_time_axis(
        start_ts,
        end_ts,
        step_secs,
        buckets,
        |point| point.timestamp,
        |timestamp| ClientMetricsPoint {
            timestamp,
            total: 0,
            online: 0,
            recovering: 0,
            offline: 0,
            unknown: 0,
        },
    ))
}

fn load_llm_series(
    conn: &Connection,
    start_ts: i64,
    end_ts: i64,
    step_secs: i64,
) -> Result<Vec<LlmMetricsPoint>, AppError> {
    let mut stmt = conn.prepare(
         "WITH base AS (
            SELECT
                (timestamp / ?1) * ?1 AS bucket_ts,
                status, error_kind, http_status,
                input_tokens, output_tokens, total_tokens,
                latency_ms, ttft_ms, stream_started, stream_completed,
                usage_state, stream_status
            FROM llm_request_metrics
            WHERE timestamp >= ?2 AND timestamp <= ?3
         ),
         agg AS (
            SELECT
                bucket_ts,
                COUNT(*) AS requests,
                SUM(CASE WHEN status = 'error' THEN 1 ELSE 0 END) AS errors,
                SUM(CASE WHEN error_kind = 'rate_limited' OR http_status = 429 THEN 1 ELSE 0 END) AS rate_limited,
                SUM(CASE WHEN error_kind = 'concurrency_limited' THEN 1 ELSE 0 END) AS concurrency_limited,
                COALESCE(SUM(input_tokens), 0) AS input_tokens,
                COALESCE(SUM(output_tokens), 0) AS output_tokens,
                COALESCE(SUM(total_tokens), 0) AS total_tokens,
                AVG(CASE
                    WHEN status = 'success'
                     AND stream_started = 1
                     AND stream_completed = 1
                     AND stream_status = 'completed'
                     AND ttft_ms > 0
                     AND latency_ms > ttft_ms
                    THEN ttft_ms
                END) AS average_ttft_ms,
                AVG(CASE
                    WHEN status = 'success'
                     AND stream_started = 1
                     AND stream_completed = 1
                     AND stream_status = 'completed'
                     AND usage_state = 'observed'
                     AND output_tokens > 0
                     AND ttft_ms > 0
                     AND latency_ms > ttft_ms
                    THEN output_tokens * 1000.0 / (latency_ms - ttft_ms)
                END) AS average_tps,
                SUM(CASE
                    WHEN status = 'success'
                     AND stream_started = 1
                     AND stream_completed = 1
                     AND stream_status = 'completed'
                     AND ttft_ms > 0
                     AND latency_ms > ttft_ms
                    THEN 1 ELSE 0
                END) AS ttft_sample_count,
                SUM(CASE
                    WHEN status = 'success'
                     AND stream_started = 1
                     AND stream_completed = 1
                     AND stream_status = 'completed'
                     AND usage_state = 'observed'
                     AND output_tokens > 0
                     AND ttft_ms > 0
                     AND latency_ms > ttft_ms
                    THEN 1 ELSE 0
                END) AS tps_sample_count
            FROM base
            GROUP BY bucket_ts
         ),
         latency_ranked AS (
            SELECT
                bucket_ts,
                latency_ms AS value,
                ROW_NUMBER() OVER (PARTITION BY bucket_ts ORDER BY latency_ms) AS rn,
                COUNT(*) OVER (PARTITION BY bucket_ts) AS cnt
            FROM base
            WHERE latency_ms IS NOT NULL
         ),
         latency_p95 AS (
            SELECT bucket_ts, MIN(value) AS value
            FROM latency_ranked
            WHERE rn >= ((cnt * 95 + 99) / 100)
            GROUP BY bucket_ts
         ),
         ttft_ranked AS (
            SELECT
                bucket_ts,
                ttft_ms AS value,
                ROW_NUMBER() OVER (PARTITION BY bucket_ts ORDER BY ttft_ms) AS rn,
                COUNT(*) OVER (PARTITION BY bucket_ts) AS cnt
            FROM base
            WHERE status = 'success'
              AND stream_started = 1
              AND stream_completed = 1
              AND stream_status = 'completed'
              AND ttft_ms > 0
              AND latency_ms > ttft_ms
         ),
         ttft_p95 AS (
            SELECT bucket_ts, MIN(value) AS value
            FROM ttft_ranked
            WHERE rn >= ((cnt * 95 + 99) / 100)
            GROUP BY bucket_ts
         )
         SELECT
            agg.bucket_ts,
            agg.requests,
            agg.errors,
            agg.rate_limited,
            agg.concurrency_limited,
            agg.input_tokens,
            agg.output_tokens,
            agg.total_tokens,
            latency_p95.value,
            ttft_p95.value,
            agg.average_ttft_ms,
            agg.average_tps,
            agg.ttft_sample_count,
            agg.tps_sample_count
         FROM agg
         LEFT JOIN latency_p95 ON latency_p95.bucket_ts = agg.bucket_ts
         LEFT JOIN ttft_p95 ON ttft_p95.bucket_ts = agg.bucket_ts
         ORDER BY agg.bucket_ts ASC",
    ).map_err(|err| AppError::Internal(format!("prepare llm metrics series failed: {err}")))?;
    let rows = stmt
        .query_map(params![step_secs, start_ts, end_ts], |row| {
            let requests = row.get::<_, i64>(1)?.max(0) as f64;
            let errors = row.get::<_, i64>(2)?.max(0) as f64;
            let input_tokens = row.get::<_, i64>(5)?.max(0) as f64;
            let output_tokens = row.get::<_, i64>(6)?.max(0) as f64;
            let total_tokens = row.get::<_, i64>(7)?.max(0) as f64;
            let factor = 60.0 / step_secs as f64;
            Ok(LlmMetricsPoint {
                timestamp: row.get(0)?,
                rpm: requests * factor,
                tpm: total_tokens * factor,
                input_tpm: input_tokens * factor,
                output_tpm: output_tokens * factor,
                error_rate: if requests > 0.0 {
                    errors / requests
                } else {
                    0.0
                },
                rate_limited: row.get::<_, i64>(3)?.max(0) as u64,
                concurrency_limited: row.get::<_, i64>(4)?.max(0) as u64,
                p95_latency_ms: opt_i64_to_u64(row.get(8)?),
                p95_ttft_ms: opt_i64_to_u64(row.get(9)?),
                average_ttft_ms: row.get(10)?,
                average_tps: row.get(11)?,
                ttft_sample_count: row.get::<_, i64>(12)?.max(0) as u64,
                tps_sample_count: row.get::<_, i64>(13)?.max(0) as u64,
            })
        })
        .map_err(|err| AppError::Internal(format!("query llm metrics series failed: {err}")))?;
    let buckets = collect_rows(rows, "llm metrics series")?;
    Ok(fill_time_axis(
        start_ts,
        end_ts,
        step_secs,
        buckets,
        |p| p.timestamp,
        |ts| LlmMetricsPoint {
            timestamp: ts,
            rpm: 0.0,
            tpm: 0.0,
            input_tpm: 0.0,
            output_tpm: 0.0,
            error_rate: 0.0,
            rate_limited: 0,
            concurrency_limited: 0,
            p95_latency_ms: None,
            p95_ttft_ms: None,
            average_ttft_ms: None,
            average_tps: None,
            ttft_sample_count: 0,
            tps_sample_count: 0,
        },
    ))
}

/// Materializes empty buckets between sparse samples so the time axis is
/// continuous. Without this, the frontend cannot distinguish "no data" from
/// "no traffic" — both produce a sparse series, which the chart renders as
/// "sampling".
fn fill_time_axis<T, K, F>(
    start_ts: i64,
    end_ts: i64,
    step_secs: i64,
    buckets: Vec<T>,
    key: K,
    blank: F,
) -> Vec<T>
where
    K: Fn(&T) -> i64,
    F: Fn(i64) -> T,
{
    if step_secs <= 0 {
        return buckets;
    }
    let first_bucket = (start_ts / step_secs) * step_secs;
    let last_bucket = (end_ts / step_secs) * step_secs;
    let mut existing: HashMap<i64, T> =
        buckets.into_iter().map(|item| (key(&item), item)).collect();
    let mut out = Vec::new();
    let mut ts = first_bucket;
    while ts <= last_bucket {
        match existing.remove(&ts) {
            Some(item) => out.push(item),
            None => out.push(blank(ts)),
        }
        ts += step_secs;
    }
    out
}

fn load_llm_snapshot(
    conn: &Connection,
    range_secs: i64,
) -> Result<super::models::LlmMetricsSnapshot, AppError> {
    let end_ts = chrono::Utc::now().timestamp();
    let start_ts = end_ts - range_secs;
    let p95_latency_ms = load_llm_percentile(conn, "latency_ms", start_ts, end_ts)?;
    let p95_ttft_ms = load_llm_percentile(conn, "ttft_ms", start_ts, end_ts)?;
    let failover_success_rate = load_substitution_success_rate(conn, start_ts, end_ts)?;
    conn.query_row(
        "SELECT
            COUNT(*),
            COALESCE(SUM(CASE WHEN status = 'error' THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN error_kind = 'rate_limited' OR http_status = 429 THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(input_tokens), 0),
            COALESCE(SUM(output_tokens), 0),
            COALESCE(SUM(total_tokens), 0),
            COUNT(DISTINCT COALESCE(NULLIF(actual_model, ''), requested_model)),
            COUNT(DISTINCT share_id),
            COALESCE(SUM(cache_read_tokens), 0),
            AVG(CASE
                WHEN status = 'success'
                 AND stream_started = 1
                 AND stream_completed = 1
                 AND stream_status = 'completed'
                 AND ttft_ms > 0
                 AND latency_ms > ttft_ms
                THEN ttft_ms
            END),
            AVG(CASE
                WHEN status = 'success'
                 AND stream_started = 1
                 AND stream_completed = 1
                 AND stream_status = 'completed'
                 AND usage_state = 'observed'
                 AND output_tokens > 0
                 AND ttft_ms > 0
                 AND latency_ms > ttft_ms
                THEN output_tokens * 1000.0 / (latency_ms - ttft_ms)
            END),
            COALESCE(SUM(CASE
                WHEN status = 'success'
                 AND stream_started = 1
                 AND stream_completed = 1
                 AND stream_status = 'completed'
                 AND ttft_ms > 0
                 AND latency_ms > ttft_ms
                THEN 1 ELSE 0
            END), 0),
            COALESCE(SUM(CASE
                WHEN status = 'success'
                 AND stream_started = 1
                 AND stream_completed = 1
                 AND stream_status = 'completed'
                 AND usage_state = 'observed'
                 AND output_tokens > 0
                 AND ttft_ms > 0
                 AND latency_ms > ttft_ms
                THEN 1 ELSE 0
            END), 0)
         FROM llm_request_metrics WHERE timestamp >= ?1 AND timestamp <= ?2",
        params![start_ts, end_ts],
        |row| {
            let requests = row.get::<_, i64>(0)?.max(0) as f64;
            let errors = row.get::<_, i64>(1)?.max(0) as f64;
            let input_tokens = row.get::<_, i64>(3)?.max(0) as f64;
            let cache_read_tokens = row.get::<_, i64>(8)?.max(0) as f64;
            let factor = 60.0 / range_secs.max(1) as f64;
            Ok(super::models::LlmMetricsSnapshot {
                rpm: requests * factor,
                tpm: row.get::<_, i64>(5)?.max(0) as f64 * factor,
                input_tpm: input_tokens * factor,
                output_tpm: row.get::<_, i64>(4)?.max(0) as f64 * factor,
                inflight: 0,
                error_rate: if requests > 0.0 {
                    errors / requests
                } else {
                    0.0
                },
                rate_limit_per_minute: row.get::<_, i64>(2)?.max(0) as f64 * factor,
                p95_latency_ms,
                p95_ttft_ms,
                average_ttft_ms: row.get(9)?,
                average_tps: row.get(10)?,
                ttft_sample_count: row.get::<_, i64>(11)?.max(0) as u64,
                tps_sample_count: row.get::<_, i64>(12)?.max(0) as u64,
                active_models: row.get::<_, i64>(6)?.max(0) as u64,
                active_shares: row.get::<_, i64>(7)?.max(0) as u64,
                failover_success_rate,
                cache_hit_rate: if cache_read_tokens + input_tokens > 0.0 {
                    Some(cache_read_tokens / (cache_read_tokens + input_tokens))
                } else {
                    None
                },
            })
        },
    )
    .map_err(|err| AppError::Internal(format!("load llm snapshot failed: {err}")))
}

/// A model substitution is a request served by a model other than the one the
/// caller asked for — the router's effective "failover". This returns the
/// success rate among those substituted requests, or `None` when no
/// substitution happened in the window.
fn load_substitution_success_rate(
    conn: &Connection,
    start_ts: i64,
    end_ts: i64,
) -> Result<Option<f64>, AppError> {
    conn.query_row(
        "SELECT
            COUNT(*),
            COALESCE(SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END), 0)
         FROM llm_request_metrics
         WHERE timestamp >= ?1 AND timestamp <= ?2
           AND status IN ('success', 'error')
           AND actual_model IS NOT NULL AND actual_model != ''
           AND requested_model IS NOT NULL AND requested_model != ''
           AND actual_model != requested_model",
        params![start_ts, end_ts],
        |row| {
            let total = row.get::<_, i64>(0)?.max(0);
            let success = row.get::<_, i64>(1)?.max(0);
            Ok(if total > 0 {
                Some(success as f64 / total as f64)
            } else {
                None
            })
        },
    )
    .map_err(|err| AppError::Internal(format!("load substitution success rate failed: {err}")))
}

/// Aggregates model-substitution pairs (`requested_model` → `actual_model`)
/// over a window so the dashboard can show which models the router is
/// silently rerouting and how reliable each substitution is.
fn load_llm_reliability(
    conn: &Connection,
    start_ts: i64,
    end_ts: i64,
    limit: usize,
) -> Result<
    (
        u64,
        u64,
        Option<f64>,
        Vec<super::models::LlmSubstitutionItem>,
    ),
    AppError,
> {
    let (total, substituted, sub_success_total, sub_success_ok): (i64, i64, i64, i64) = conn
        .query_row(
            "SELECT
                COUNT(*),
                COALESCE(SUM(CASE WHEN actual_model IS NOT NULL AND actual_model != ''
                    AND requested_model IS NOT NULL AND requested_model != ''
                    AND actual_model != requested_model THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN actual_model IS NOT NULL AND actual_model != ''
                    AND requested_model IS NOT NULL AND requested_model != ''
                    AND actual_model != requested_model THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN actual_model IS NOT NULL AND actual_model != ''
                    AND requested_model IS NOT NULL AND requested_model != ''
                    AND actual_model != requested_model AND status = 'success' THEN 1 ELSE 0 END), 0)
             FROM llm_request_metrics
             WHERE timestamp >= ?1 AND timestamp <= ?2
               AND status IN ('success', 'error')",
            params![start_ts, end_ts],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|err| AppError::Internal(format!("load reliability totals failed: {err}")))?;

    let mut stmt = conn
        .prepare(
            "SELECT requested_model, actual_model,
                    COUNT(*) AS requests,
                    SUM(CASE WHEN status = 'error' THEN 1 ELSE 0 END) AS errors
             FROM llm_request_metrics
             WHERE timestamp >= ?1 AND timestamp <= ?2
               AND status IN ('success', 'error')
               AND actual_model IS NOT NULL AND actual_model != ''
               AND requested_model IS NOT NULL AND requested_model != ''
               AND actual_model != requested_model
             GROUP BY requested_model, actual_model
             ORDER BY requests DESC
             LIMIT ?3",
        )
        .map_err(|err| AppError::Internal(format!("prepare reliability items failed: {err}")))?;
    let rows = stmt
        .query_map(params![start_ts, end_ts, limit as i64], |row| {
            let requests = row.get::<_, i64>(2)?.max(0) as u64;
            let errors = row.get::<_, i64>(3)?.max(0) as u64;
            Ok(super::models::LlmSubstitutionItem {
                requested_model: row.get(0)?,
                actual_model: row.get(1)?,
                requests,
                errors,
                error_rate: if requests > 0 {
                    errors as f64 / requests as f64
                } else {
                    0.0
                },
            })
        })
        .map_err(|err| AppError::Internal(format!("query reliability items failed: {err}")))?;
    let items = collect_rows(rows, "reliability items")?;

    let substitution_success_rate = if sub_success_total > 0 {
        Some(sub_success_ok as f64 / sub_success_total as f64)
    } else {
        None
    };
    Ok((
        total.max(0) as u64,
        substituted.max(0) as u64,
        substitution_success_rate,
        items,
    ))
}

fn load_llm_top(
    conn: &Connection,
    start_ts: i64,
    by: &str,
    limit: usize,
) -> Result<Vec<LlmTopItem>, AppError> {
    let key_expr = match by {
        "share" | "shares" | "share-performance" => {
            "COALESCE(NULLIF(subdomain, ''), share_id, '-')"
        }
        "market" | "markets" => "COALESCE(market_email, '-')",
        _ => "COALESCE(NULLIF(actual_model, ''), requested_model, '-')",
    };
    let order_expr = match by {
        "errors" => "errors DESC",
        "latency" => "p95_latency_ms DESC",
        "requests" => "requests DESC",
        "share-performance" => {
            "CASE WHEN ttft_sample_count > 0 THEN 0 ELSE 1 END, average_ttft_ms DESC, requests DESC"
        }
        _ => "total_tokens DESC",
    };
    let base_filter = if matches!(by, "share" | "shares" | "share-performance") {
        "AND route_type = 'direct' AND share_id IS NOT NULL AND share_id != ''"
    } else {
        ""
    };
    let sql = format!(
        "WITH base AS (
            SELECT
                {key_expr} AS key,
                status,
                total_tokens,
                latency_ms,
                ttft_ms,
                stream_started,
                stream_completed,
                usage_state,
                stream_status,
                output_tokens,
                timestamp
            FROM llm_request_metrics
            WHERE timestamp >= ?1
              {base_filter}
         ),
         agg AS (
            SELECT key,
                COUNT(*) AS requests,
                COALESCE(SUM(total_tokens), 0) AS total_tokens,
                SUM(CASE WHEN status = 'error' THEN 1 ELSE 0 END) AS errors,
                AVG(CASE
                    WHEN status = 'success'
                     AND stream_started = 1
                     AND stream_completed = 1
                     AND stream_status = 'completed'
                     AND ttft_ms > 0
                     AND latency_ms > ttft_ms
                    THEN ttft_ms
                END) AS average_ttft_ms,
                AVG(CASE
                    WHEN status = 'success'
                     AND stream_started = 1
                     AND stream_completed = 1
                     AND stream_status = 'completed'
                     AND usage_state = 'observed'
                     AND output_tokens > 0
                     AND ttft_ms > 0
                     AND latency_ms > ttft_ms
                    THEN output_tokens * 1000.0 / (latency_ms - ttft_ms)
                END) AS average_tps,
                SUM(CASE
                    WHEN status = 'success'
                     AND stream_started = 1
                     AND stream_completed = 1
                     AND stream_status = 'completed'
                     AND ttft_ms > 0
                     AND latency_ms > ttft_ms
                    THEN 1 ELSE 0
                END) AS ttft_sample_count,
                SUM(CASE
                    WHEN status = 'success'
                     AND stream_started = 1
                     AND stream_completed = 1
                     AND stream_status = 'completed'
                     AND usage_state = 'observed'
                     AND output_tokens > 0
                     AND ttft_ms > 0
                     AND latency_ms > ttft_ms
                    THEN 1 ELSE 0
                END) AS tps_sample_count,
                MAX(timestamp) AS last_request_at
            FROM base
            GROUP BY key
         ),
         latency_ranked AS (
            SELECT
                key,
                latency_ms AS value,
                ROW_NUMBER() OVER (PARTITION BY key ORDER BY latency_ms) AS rn,
                COUNT(*) OVER (PARTITION BY key) AS cnt
            FROM base
            WHERE latency_ms IS NOT NULL
         ),
         latency_p95 AS (
            SELECT key, MIN(value) AS p95_latency_ms
            FROM latency_ranked
            WHERE rn >= ((cnt * 95 + 99) / 100)
            GROUP BY key
         )
         SELECT
            agg.key,
            agg.requests,
            agg.total_tokens,
            agg.errors,
            latency_p95.p95_latency_ms,
            agg.average_ttft_ms,
            agg.average_tps,
            agg.ttft_sample_count,
            agg.tps_sample_count,
            agg.last_request_at
         FROM agg
         LEFT JOIN latency_p95 ON latency_p95.key = agg.key
         ORDER BY {order_expr}
         LIMIT ?2"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|err| AppError::Internal(format!("prepare llm top failed: {err}")))?;
    let rows = stmt
        .query_map(params![start_ts, limit as i64], |row| {
            let requests = row.get::<_, i64>(1)?.max(0) as u64;
            let errors = row.get::<_, i64>(3)?.max(0) as u64;
            Ok(LlmTopItem {
                key: row.get(0)?,
                requests,
                total_tokens: row.get::<_, i64>(2)?.max(0) as u64,
                errors,
                error_rate: if requests > 0 {
                    errors as f64 / requests as f64
                } else {
                    0.0
                },
                p95_latency_ms: opt_i64_to_u64(row.get(4)?),
                average_ttft_ms: row.get(5)?,
                average_tps: row.get(6)?,
                ttft_sample_count: row.get::<_, i64>(7)?.max(0) as u64,
                tps_sample_count: row.get::<_, i64>(8)?.max(0) as u64,
                last_request_at: row.get(9)?,
            })
        })
        .map_err(|err| AppError::Internal(format!("query llm top failed: {err}")))?;
    collect_rows(rows, "llm top")
}

fn load_llm_percentile(
    conn: &Connection,
    column: &str,
    start_ts: i64,
    end_ts: i64,
) -> Result<Option<u64>, AppError> {
    let column = match column {
        "latency_ms" => "latency_ms",
        "ttft_ms" => "ttft_ms",
        _ => return Err(AppError::Internal("invalid llm percentile column".into())),
    };
    let quality_filter = if column == "ttft_ms" {
        "AND status = 'success'\n             AND stream_started = 1\n             AND stream_completed = 1\n             AND stream_status = 'completed'\n             AND ttft_ms > 0\n             AND latency_ms > ttft_ms"
    } else {
        ""
    };
    let sql = format!(
        "WITH ranked AS (
            SELECT
                {column} AS value,
                ROW_NUMBER() OVER (ORDER BY {column}) AS rn,
                COUNT(*) OVER () AS cnt
            FROM llm_request_metrics
            WHERE timestamp >= ?1 AND timestamp <= ?2 AND {column} IS NOT NULL
              {quality_filter}
         )
         SELECT value
         FROM ranked
         WHERE rn >= ((cnt * 95 + 99) / 100)
         ORDER BY rn ASC
         LIMIT 1"
    );
    let value = conn
        .query_row(&sql, params![start_ts, end_ts], |row| row.get::<_, i64>(0))
        .optional()
        .map_err(|err| AppError::Internal(format!("load llm percentile failed: {err}")))?;
    Ok(opt_i64_to_u64(value))
}

fn load_events(conn: &Connection, limit: usize) -> Result<Vec<MetricEvent>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, timestamp, severity, kind, message, details_json
               FROM metric_events
              ORDER BY timestamp DESC, id DESC
              LIMIT ?1",
        )
        .map_err(|err| AppError::Internal(format!("prepare metric events failed: {err}")))?;
    let rows = stmt
        .query_map(params![limit as i64], |row| {
            let details_raw = row
                .get::<_, Option<String>>(5)?
                .unwrap_or_else(|| "{}".into());
            Ok(MetricEvent {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                severity: row.get(2)?,
                kind: row.get(3)?,
                message: row.get(4)?,
                details: serde_json::from_str(&details_raw)
                    .unwrap_or_else(|_| serde_json::json!({})),
            })
        })
        .map_err(|err| AppError::Internal(format!("query metric events failed: {err}")))?;
    collect_rows(rows, "metric events")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::models::{
        ClientMetricsSnapshot, DiskUsage, NetworkMetricsStatus, ProcessMetricsStatus,
    };

    #[test]
    fn host_series_reads_avg_rss_as_real() {
        let conn = Connection::open_in_memory().expect("open in-memory metrics db");
        init_metrics_db(&conn).expect("init metrics db");

        insert_host_metrics(&conn, &host_sample(900, 100)).expect("insert first host sample");
        insert_host_metrics(&conn, &host_sample(910, 102)).expect("insert second host sample");

        let series = load_host_series(&conn, 900, 929, 30).expect("load host series");

        assert_eq!(series.len(), 1);
        assert_eq!(series[0].timestamp, 900);
        assert_eq!(series[0].process_rss_bytes, Some(101));
    }

    #[test]
    fn client_series_uses_latest_sample_in_each_bucket() {
        let conn = Connection::open_in_memory().expect("open in-memory metrics db");
        init_metrics_db(&conn).expect("init metrics db");
        insert_client_metrics(
            &conn,
            &ClientMetricsSnapshot {
                timestamp: 900,
                total: 4,
                monitored: 4,
                online: 3,
                recovering: 0,
                offline: 1,
                unknown: 0,
                items: Vec::new(),
            },
        )
        .expect("insert first Client sample");
        insert_client_metrics(
            &conn,
            &ClientMetricsSnapshot {
                timestamp: 910,
                total: 4,
                monitored: 4,
                online: 2,
                recovering: 1,
                offline: 1,
                unknown: 0,
                items: Vec::new(),
            },
        )
        .expect("insert latest Client sample");

        let series = load_client_series(&conn, 900, 929, 30).expect("load Client series");
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].timestamp, 900);
        assert_eq!(series[0].online, 2);
        assert_eq!(series[0].recovering, 1);
        assert_eq!(series[0].offline, 1);
    }

    #[test]
    fn router_metrics_persist_all_ssh_forwarding_counters() {
        let conn = Connection::open_in_memory().expect("open in-memory metrics db");
        init_metrics_db(&conn).expect("init metrics db");
        let router = RouterMetricsStatus {
            ssh_pending_channel_opens: 11,
            ssh_channel_open_started_total: 12,
            ssh_channel_open_succeeded_total: 13,
            ssh_channel_open_explicit_failures_total: 14,
            ssh_channel_open_timeout_total: 15,
            ssh_channel_open_session_errors_total: 16,
            ssh_channel_open_cancelled_total: 17,
            ssh_active_bridges: 18,
            ssh_bridge_created_total: 19,
            ssh_bridge_completed_total: 20,
            ssh_bridge_cancelled_total: 21,
            ssh_bridge_write_stall_total: 22,
            ssh_bridge_half_close_idle_total: 23,
            ssh_bridge_io_errors_total: 24,
            ssh_forward_capacity_rejected_total: 25,
            ..Default::default()
        };
        insert_router_metrics(&conn, 900, &router).expect("insert router metrics");

        let stored: Vec<i64> = conn
            .query_row(
                "SELECT ssh_pending_channel_opens, ssh_channel_open_started_total,
                        ssh_channel_open_succeeded_total,
                        ssh_channel_open_explicit_failures_total,
                        ssh_channel_open_timeout_total,
                        ssh_channel_open_session_errors_total,
                        ssh_channel_open_cancelled_total,
                        ssh_active_bridges, ssh_bridge_created_total,
                        ssh_bridge_completed_total, ssh_bridge_cancelled_total,
                        ssh_bridge_write_stall_total, ssh_bridge_half_close_idle_total,
                        ssh_bridge_io_errors_total, ssh_forward_capacity_rejected_total
                   FROM router_metrics WHERE timestamp = 900",
                [],
                |row| (0..15).map(|index| row.get(index)).collect(),
            )
            .expect("read SSH forwarding metrics");
        assert_eq!(stored, (11..=25).collect::<Vec<_>>());
    }

    #[test]
    fn metrics_sample_transaction_rolls_back_partial_history() {
        let mut conn = Connection::open_in_memory().expect("open in-memory metrics db");
        init_metrics_db(&conn).expect("init metrics db");
        conn.execute("DROP TABLE router_metrics", [])
            .expect("remove router metrics table");
        let host = host_sample(900, 512);
        let router = RouterMetricsStatus::default();
        let clients = ClientMetricsSnapshot {
            timestamp: 900,
            ..Default::default()
        };

        assert!(insert_metrics_sample(&mut conn, &host, &router, &clients).is_err());
        let host_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM host_metrics", [], |row| row.get(0))
            .unwrap();
        let client_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM client_metrics", [], |row| row.get(0))
            .unwrap();
        assert_eq!(host_rows, 0);
        assert_eq!(client_rows, 0);
    }

    #[test]
    fn concurrency_classification_survives_metric_enrichment_in_any_order() {
        let conn = Connection::open_in_memory().expect("open in-memory metrics db");
        init_metrics_db(&conn).expect("init metrics db");

        for (request_id, first_kind, second_kind) in [
            (
                "generic-then-concurrency",
                "upstream_error",
                "concurrency_limited",
            ),
            (
                "concurrency-then-generic",
                "concurrency_limited",
                "upstream_error",
            ),
        ] {
            insert_llm_request_metric(&conn, &llm_metric(request_id, first_kind))
                .expect("insert first metric");
            insert_llm_request_metric(&conn, &llm_metric(request_id, second_kind))
                .expect("enrich metric");

            let stored: String = conn
                .query_row(
                    "SELECT error_kind FROM llm_request_metrics WHERE request_id = ?1",
                    params![request_id],
                    |row| row.get(0),
                )
                .expect("read merged classification");
            assert_eq!(stored, "concurrency_limited");
        }
    }

    #[test]
    fn llm_performance_uses_completed_streams_and_output_tokens_only() {
        let conn = Connection::open_in_memory().expect("open in-memory metrics db");
        init_metrics_db(&conn).expect("init metrics db");

        let mut observed = llm_metric("observed", "");
        observed.timestamp = chrono::Utc::now().timestamp();
        observed.status = "success".into();
        observed.error_kind = None;
        observed.http_status = Some(200);
        observed.latency_ms = Some(5_000);
        observed.ttft_ms = Some(1_000);
        observed.stream_started = true;
        observed.stream_completed = true;
        observed.usage_state = Some("observed".into());
        observed.stream_status = Some("completed".into());
        observed.usage_revision = 2;
        observed.input_tokens = Some(9_000);
        observed.output_tokens = Some(40);
        observed.total_tokens = Some(9_040);
        insert_llm_request_metric(&conn, &observed).expect("insert observed stream");

        let mut missing = observed.clone();
        missing.request_id = Some("missing".into());
        missing.ttft_ms = Some(2_000);
        missing.usage_state = Some("missing".into());
        missing.output_tokens = Some(500);
        insert_llm_request_metric(&conn, &missing).expect("insert missing usage stream");

        let mut interrupted = observed.clone();
        interrupted.request_id = Some("interrupted".into());
        interrupted.status = "error".into();
        interrupted.stream_completed = false;
        interrupted.usage_state = Some("interrupted".into());
        interrupted.stream_status = Some("interrupted".into());
        insert_llm_request_metric(&conn, &interrupted).expect("insert interrupted stream");

        let snapshot = load_llm_snapshot(&conn, 300).expect("load llm snapshot");
        assert_eq!(snapshot.ttft_sample_count, 2);
        assert_eq!(snapshot.tps_sample_count, 1);
        assert_eq!(snapshot.average_ttft_ms, Some(1_500.0));
        assert_eq!(snapshot.average_tps, Some(10.0));

        let top = load_llm_top(
            &conn,
            chrono::Utc::now().timestamp() - 300,
            "share-performance",
            10,
        )
        .expect("load share performance");
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].ttft_sample_count, 2);
        assert_eq!(top[0].tps_sample_count, 1);
        assert_eq!(top[0].average_tps, Some(10.0));
    }

    #[test]
    fn older_usage_revision_cannot_replace_completed_metric() {
        let conn = Connection::open_in_memory().expect("open in-memory metrics db");
        init_metrics_db(&conn).expect("init metrics db");

        let mut completed = llm_metric("revisioned", "");
        completed.status = "success".into();
        completed.error_kind = None;
        completed.http_status = Some(200);
        completed.latency_ms = Some(4_000);
        completed.ttft_ms = Some(500);
        completed.stream_started = true;
        completed.stream_completed = true;
        completed.usage_state = Some("observed".into());
        completed.stream_status = Some("completed".into());
        completed.usage_revision = 2;
        completed.output_tokens = Some(35);
        insert_llm_request_metric(&conn, &completed).expect("insert completed metric");

        let mut stale = completed.clone();
        stale.status = "pending".into();
        stale.ttft_ms = None;
        stale.stream_completed = false;
        stale.usage_state = Some("pending".into());
        stale.stream_status = Some("pending".into());
        stale.usage_revision = 1;
        stale.output_tokens = Some(0);
        insert_llm_request_metric(&conn, &stale).expect("attempt stale metric update");

        let stored: (String, i64, String, String, i64, i64) = conn
            .query_row(
                "SELECT status, stream_completed, usage_state, stream_status, usage_revision, output_tokens
                   FROM llm_request_metrics WHERE request_id = 'revisioned'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("read revisioned metric");
        assert_eq!(
            stored,
            (
                "success".into(),
                1,
                "observed".into(),
                "completed".into(),
                2,
                35
            )
        );
    }

    #[test]
    fn share_terminal_and_market_context_merge_independently_of_arrival_order() {
        for market_first in [true, false] {
            let conn = Connection::open_in_memory().expect("open in-memory metrics db");
            init_metrics_db(&conn).expect("init metrics db");

            let mut terminal = llm_metric("cross-source", "");
            terminal.timestamp = 902;
            terminal.subdomain = None;
            terminal.status = "success".into();
            terminal.error_kind = None;
            terminal.http_status = Some(200);
            terminal.latency_ms = Some(5_000);
            terminal.ttft_ms = Some(1_000);
            terminal.stream_started = true;
            terminal.stream_completed = true;
            terminal.usage_state = Some("observed".into());
            terminal.stream_status = Some("completed".into());
            terminal.usage_revision = 2;
            terminal.requested_model = Some("requested-model".into());
            terminal.actual_model = Some("actual-model".into());
            terminal.output_tokens = Some(40);

            let mut market = terminal.clone();
            market.timestamp = 901;
            market.route_type = "market".into();
            market.market_email = Some("market@example.com".into());
            market.subdomain = Some("share-subdomain".into());
            market.stream_started = false;
            market.stream_completed = false;
            market.ttft_ms = None;
            market.stream_status = Some("settled".into());
            market.usage_revision = 0;

            let ordered = if market_first {
                [&market, &terminal]
            } else {
                [&terminal, &market]
            };
            for metric in ordered {
                insert_llm_request_metric(&conn, metric).expect("merge cross-source metric");
            }

            let stored: (
                String,
                String,
                String,
                String,
                i64,
                String,
                String,
                i64,
                i64,
            ) = conn
                .query_row(
                    "SELECT route_type, market_email, subdomain, status, stream_completed,
                            usage_state, stream_status, usage_revision, output_tokens
                       FROM llm_request_metrics WHERE request_id = 'cross-source'",
                    [],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                            row.get(7)?,
                            row.get(8)?,
                        ))
                    },
                )
                .expect("read cross-source metric");
            assert_eq!(
                stored,
                (
                    "direct".into(),
                    "market@example.com".into(),
                    "share-subdomain".into(),
                    "success".into(),
                    1,
                    "observed".into(),
                    "completed".into(),
                    2,
                    40,
                ),
                "market_first={market_first}"
            );
        }
    }

    #[test]
    fn substitution_success_rate_ignores_pending_requests() {
        let conn = Connection::open_in_memory().expect("open in-memory metrics db");
        init_metrics_db(&conn).expect("init metrics db");

        for (request_id, status) in [
            ("substitution-success", "success"),
            ("substitution-error", "error"),
            ("substitution-pending", "pending"),
        ] {
            let mut metric = llm_metric(request_id, "upstream_error");
            metric.status = status.into();
            metric.requested_model = Some("requested-model".into());
            metric.actual_model = Some("actual-model".into());
            insert_llm_request_metric(&conn, &metric).expect("insert substitution metric");
        }

        assert_eq!(
            load_substitution_success_rate(&conn, 0, 1_000)
                .expect("load substitution success rate"),
            Some(0.5)
        );
    }

    fn llm_metric(request_id: &str, error_kind: &str) -> LlmRequestMetric {
        LlmRequestMetric {
            timestamp: 900,
            request_id: Some(request_id.to_string()),
            route_type: "direct".into(),
            market_email: None,
            share_id: Some("share-1".into()),
            subdomain: Some("share-1".into()),
            app_type: Some("codex".into()),
            provider: None,
            requested_model: None,
            actual_model: None,
            status: "error".into(),
            error_kind: Some(error_kind.to_string()),
            http_status: Some(409),
            latency_ms: Some(0),
            ttft_ms: None,
            stream_started: false,
            stream_completed: false,
            usage_state: None,
            stream_status: None,
            usage_revision: 0,
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
            estimated_cost_usd: None,
        }
    }

    fn host_sample(timestamp: i64, rss_bytes: u64) -> HostMetricsStatus {
        HostMetricsStatus {
            timestamp,
            uptime_secs: Some(120),
            cpu_percent: Some(25.0),
            load_1: Some(0.2),
            load_5: Some(0.3),
            load_15: Some(0.4),
            memory_used_bytes: Some(1_000),
            memory_total_bytes: Some(2_000),
            memory_available_bytes: Some(1_000),
            swap_used_bytes: Some(0),
            swap_total_bytes: Some(0),
            disks: vec![DiskUsage {
                label: "root".into(),
                mount_point: "/".into(),
                used_bytes: 2_000,
                total_bytes: 4_000,
            }],
            network: NetworkMetricsStatus {
                rx_bytes_per_sec: Some(10.0),
                tx_bytes_per_sec: Some(20.0),
                tcp_established: Some(1),
                tcp_time_wait: Some(0),
            },
            process: ProcessMetricsStatus {
                open_fds: Some(10),
                max_fds: Some(100),
                fd_usage_percent: Some(10.0),
                threads: Some(4),
                rss_bytes: Some(rss_bytes),
                cpu_percent: Some(1.0),
                uptime_secs: Some(60),
            },
        }
    }
}

fn collect_rows<T, F>(rows: rusqlite::MappedRows<'_, F>, label: &str) -> Result<Vec<T>, AppError>
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
{
    let mut values = Vec::new();
    for row in rows {
        values.push(row.map_err(|err| AppError::Internal(format!("read {label} failed: {err}")))?);
    }
    Ok(values)
}
