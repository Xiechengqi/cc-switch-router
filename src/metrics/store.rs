use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::{Connection, OptionalExtension, params};
use tokio::task::spawn_blocking;

use crate::error::AppError;

use super::models::{
    ClearMetricsResponse, HostMetricsPoint, HostMetricsStatus, LlmMetricsPoint, LlmRequestMetric,
    LlmTopItem, LlmTopResponse, MetricEvent, MetricsSeriesResponse, RouterMetricsPoint,
    RouterMetricsStatus,
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
            std::fs::create_dir_all(parent)
                .map_err(|err| AppError::Internal(format!("create metrics db dir failed: {err}")))?;
        }
        let conn = Connection::open(&self.path)
            .map_err(|err| AppError::Internal(format!("open metrics db failed: {err}")))?;
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
    ) -> Result<(), AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            insert_host_metrics(&conn, &host)?;
            insert_router_metrics(&conn, host.timestamp, &router)?;
            Ok(())
        })
        .await
        .map_err(|err| AppError::Internal(format!("metrics sample task failed: {err}")))?
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
                host: load_host_series(&conn, start_ts, end_ts, step_secs)?,
                router: load_router_series(&conn, start_ts, end_ts, step_secs)?,
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

    pub async fn clear(&self) -> Result<ClearMetricsResponse, AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            let mut deleted_rows = HashMap::new();
            for table in [
                "host_metrics",
                "router_metrics",
                "llm_request_metrics",
                "llm_route_attempt_metrics",
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
            process_cpu_percent REAL
        );
        CREATE INDEX IF NOT EXISTS idx_host_metrics_ts ON host_metrics(timestamp);

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
            proxy_inflight INTEGER NOT NULL,
            proxy_requests_total INTEGER NOT NULL,
            proxy_upstream_errors_total INTEGER NOT NULL,
            proxy_5xx_total INTEGER NOT NULL,
            health_probe_failures_total INTEGER NOT NULL,
            health_probe_cached_failures_total INTEGER NOT NULL,
            db_errors_total INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_router_metrics_ts ON router_metrics(timestamp);

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

        CREATE TABLE IF NOT EXISTS llm_route_attempt_metrics (
            timestamp INTEGER NOT NULL,
            request_id TEXT NOT NULL,
            attempt_no INTEGER NOT NULL,
            primary_subdomain TEXT,
            selected_subdomain TEXT,
            fallback_subdomain TEXT,
            status TEXT NOT NULL,
            failure_kind TEXT,
            retry_policy TEXT,
            latency_ms INTEGER,
            stream_started INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_llm_route_attempt_metrics_ts ON llm_route_attempt_metrics(timestamp);
        CREATE INDEX IF NOT EXISTS idx_llm_route_attempt_metrics_request ON llm_route_attempt_metrics(request_id);

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
            process_threads, process_rss_bytes, process_cpu_percent
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
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
            proxy_inflight, proxy_requests_total, proxy_upstream_errors_total, proxy_5xx_total,
            health_probe_failures_total, health_probe_cached_failures_total, db_errors_total
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
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

fn insert_llm_request_metric(conn: &Connection, metric: &LlmRequestMetric) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO llm_request_metrics (
            timestamp, request_id, route_type, market_email, share_id, subdomain,
            app_type, provider, requested_model, actual_model, status, error_kind,
            http_status, latency_ms, ttft_ms, stream_started, stream_completed,
            input_tokens, output_tokens, total_tokens, cache_read_tokens, cache_write_tokens,
            reasoning_tokens, estimated_cost_usd
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)
        ON CONFLICT(request_id) DO UPDATE SET
            timestamp = excluded.timestamp,
            route_type = excluded.route_type,
            market_email = excluded.market_email,
            share_id = excluded.share_id,
            subdomain = excluded.subdomain,
            app_type = excluded.app_type,
            provider = excluded.provider,
            requested_model = excluded.requested_model,
            actual_model = excluded.actual_model,
            status = excluded.status,
            error_kind = excluded.error_kind,
            http_status = excluded.http_status,
            latency_ms = excluded.latency_ms,
            ttft_ms = excluded.ttft_ms,
            stream_started = excluded.stream_started,
            stream_completed = excluded.stream_completed,
            input_tokens = excluded.input_tokens,
            output_tokens = excluded.output_tokens,
            total_tokens = excluded.total_tokens,
            cache_read_tokens = excluded.cache_read_tokens,
            cache_write_tokens = excluded.cache_write_tokens,
            reasoning_tokens = excluded.reasoning_tokens,
            estimated_cost_usd = excluded.estimated_cost_usd",
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
        ("host_metrics", "timestamp"),
        ("router_metrics", "timestamp"),
        ("llm_request_metrics", "timestamp"),
        ("llm_route_attempt_metrics", "timestamp"),
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
                process_threads, process_rss_bytes, process_cpu_percent
           FROM host_metrics ORDER BY timestamp DESC LIMIT 1",
        [],
        |row| {
            let disk_used = row.get::<_, Option<i64>>(10)?.unwrap_or_default() as u64;
            let disk_total = row.get::<_, Option<i64>>(11)?.unwrap_or_default() as u64;
            Ok(HostMetricsStatus {
                timestamp: row.get(0)?,
                uptime_secs: None,
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
                    uptime_secs: None,
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
                process_rss_bytes: opt_i64_to_u64(row.get(7)?),
            })
        })
        .map_err(|err| AppError::Internal(format!("query host metrics series failed: {err}")))?;
    collect_rows(rows, "host metrics series")
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
    collect_rows(rows, "router metrics series")
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
                latency_ms, ttft_ms
            FROM llm_request_metrics
            WHERE timestamp >= ?2 AND timestamp <= ?3
         ),
         agg AS (
            SELECT
                bucket_ts,
                COUNT(*) AS requests,
                SUM(CASE WHEN status != 'success' THEN 1 ELSE 0 END) AS errors,
                SUM(CASE WHEN error_kind = 'rate_limited' OR http_status = 429 THEN 1 ELSE 0 END) AS rate_limited,
                COALESCE(SUM(input_tokens), 0) AS input_tokens,
                COALESCE(SUM(output_tokens), 0) AS output_tokens,
                COALESCE(SUM(total_tokens), 0) AS total_tokens
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
            WHERE ttft_ms IS NOT NULL
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
            agg.input_tokens,
            agg.output_tokens,
            agg.total_tokens,
            latency_p95.value,
            ttft_p95.value
         FROM agg
         LEFT JOIN latency_p95 ON latency_p95.bucket_ts = agg.bucket_ts
         LEFT JOIN ttft_p95 ON ttft_p95.bucket_ts = agg.bucket_ts
         ORDER BY agg.bucket_ts ASC",
    ).map_err(|err| AppError::Internal(format!("prepare llm metrics series failed: {err}")))?;
    let rows = stmt
        .query_map(params![step_secs, start_ts, end_ts], |row| {
            let requests = row.get::<_, i64>(1)?.max(0) as f64;
            let errors = row.get::<_, i64>(2)?.max(0) as f64;
            let input_tokens = row.get::<_, i64>(4)?.max(0) as f64;
            let output_tokens = row.get::<_, i64>(5)?.max(0) as f64;
            let total_tokens = row.get::<_, i64>(6)?.max(0) as f64;
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
                p95_latency_ms: opt_i64_to_u64(row.get(7)?),
                p95_ttft_ms: opt_i64_to_u64(row.get(8)?),
            })
        })
        .map_err(|err| AppError::Internal(format!("query llm metrics series failed: {err}")))?;
    collect_rows(rows, "llm metrics series")
}

fn load_llm_snapshot(
    conn: &Connection,
    range_secs: i64,
) -> Result<super::models::LlmMetricsSnapshot, AppError> {
    let end_ts = chrono::Utc::now().timestamp();
    let start_ts = end_ts - range_secs;
    let p95_latency_ms = load_llm_percentile(conn, "latency_ms", start_ts, end_ts)?;
    let p95_ttft_ms = load_llm_percentile(conn, "ttft_ms", start_ts, end_ts)?;
    conn.query_row(
        "SELECT
            COUNT(*),
            COALESCE(SUM(CASE WHEN status != 'success' THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN error_kind = 'rate_limited' OR http_status = 429 THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(input_tokens), 0),
            COALESCE(SUM(output_tokens), 0),
            COALESCE(SUM(total_tokens), 0),
            COUNT(DISTINCT COALESCE(NULLIF(actual_model, ''), requested_model)),
            COUNT(DISTINCT share_id)
         FROM llm_request_metrics WHERE timestamp >= ?1 AND timestamp <= ?2",
        params![start_ts, end_ts],
        |row| {
            let requests = row.get::<_, i64>(0)?.max(0) as f64;
            let errors = row.get::<_, i64>(1)?.max(0) as f64;
            let factor = 60.0 / range_secs.max(1) as f64;
            Ok(super::models::LlmMetricsSnapshot {
                rpm: requests * factor,
                tpm: row.get::<_, i64>(5)?.max(0) as f64 * factor,
                input_tpm: row.get::<_, i64>(3)?.max(0) as f64 * factor,
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
                active_models: row.get::<_, i64>(6)?.max(0) as u64,
                active_shares: row.get::<_, i64>(7)?.max(0) as u64,
                failover_success_rate: None,
            })
        },
    )
    .map_err(|err| AppError::Internal(format!("load llm snapshot failed: {err}")))
}

fn load_llm_top(
    conn: &Connection,
    start_ts: i64,
    by: &str,
    limit: usize,
) -> Result<Vec<LlmTopItem>, AppError> {
    let key_expr = match by {
        "share" | "shares" => "COALESCE(share_id, '-')",
        "market" | "markets" => "COALESCE(market_email, '-')",
        _ => "COALESCE(NULLIF(actual_model, ''), requested_model, '-')",
    };
    let order_expr = match by {
        "errors" => "errors DESC",
        "latency" => "p95_latency_ms DESC",
        "requests" => "requests DESC",
        _ => "total_tokens DESC",
    };
    let sql = format!(
        "WITH base AS (
            SELECT
                {key_expr} AS key,
                status,
                total_tokens,
                latency_ms,
                timestamp
            FROM llm_request_metrics
            WHERE timestamp >= ?1
         ),
         agg AS (
            SELECT key,
                COUNT(*) AS requests,
                COALESCE(SUM(total_tokens), 0) AS total_tokens,
                SUM(CASE WHEN status != 'success' THEN 1 ELSE 0 END) AS errors,
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
                last_request_at: row.get(5)?,
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
    let sql = format!(
        "WITH ranked AS (
            SELECT
                {column} AS value,
                ROW_NUMBER() OVER (ORDER BY {column}) AS rn,
                COUNT(*) OVER () AS cnt
            FROM llm_request_metrics
            WHERE timestamp >= ?1 AND timestamp <= ?2 AND {column} IS NOT NULL
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
