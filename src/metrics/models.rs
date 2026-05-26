use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSnapshot {
    pub status: MetricsHealth,
    pub sampled_at: i64,
    pub host: HostMetricsStatus,
    pub router: RouterMetricsStatus,
    pub llm: LlmMetricsSnapshot,
    pub alerts: Vec<MetricEvent>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MetricsHealth {
    Healthy,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostMetricsInfo {
    pub hostname: Option<String>,
    pub os_name: Option<String>,
    pub os_version: Option<String>,
    pub kernel_version: Option<String>,
    pub arch: String,
    pub cpu_brand: Option<String>,
    pub cpu_cores: usize,
    pub memory_total_bytes: Option<u64>,
    pub disks: Vec<DiskInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskInfo {
    pub name: String,
    pub mount_point: String,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostMetricsStatus {
    pub timestamp: i64,
    pub uptime_secs: Option<u64>,
    pub cpu_percent: Option<f64>,
    pub load_1: Option<f64>,
    pub load_5: Option<f64>,
    pub load_15: Option<f64>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
    pub memory_available_bytes: Option<u64>,
    pub swap_used_bytes: Option<u64>,
    pub swap_total_bytes: Option<u64>,
    pub disks: Vec<DiskUsage>,
    pub network: NetworkMetricsStatus,
    pub process: ProcessMetricsStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskUsage {
    pub label: String,
    pub mount_point: String,
    pub used_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkMetricsStatus {
    pub rx_bytes_per_sec: Option<f64>,
    pub tx_bytes_per_sec: Option<f64>,
    pub tcp_established: Option<u64>,
    pub tcp_time_wait: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessMetricsStatus {
    pub open_fds: Option<u64>,
    pub max_fds: Option<u64>,
    pub fd_usage_percent: Option<f64>,
    pub threads: Option<u64>,
    pub rss_bytes: Option<u64>,
    pub cpu_percent: Option<f64>,
    pub uptime_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterMetricsStatus {
    pub active_routes: u64,
    pub pending_routes: u64,
    pub health_probe_failure_cache: u64,
    pub ssh_active_sessions: u64,
    pub ssh_forward_listeners: u64,
    pub ssh_forward_listener_created_total: u64,
    pub ssh_forward_listener_shutdown_total: u64,
    pub ssh_forward_bind_errors_total: u64,
    pub ssh_forward_accept_errors_total: u64,
    pub ssh_forward_emfile_errors_total: u64,
    pub proxy_inflight: u64,
    pub proxy_requests_total: u64,
    pub proxy_upstream_errors_total: u64,
    pub proxy_5xx_total: u64,
    pub health_probe_failures_total: u64,
    pub health_probe_cached_failures_total: u64,
    pub db_errors_total: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmMetricsSnapshot {
    pub rpm: f64,
    pub tpm: f64,
    pub input_tpm: f64,
    pub output_tpm: f64,
    pub inflight: u64,
    pub error_rate: f64,
    pub rate_limit_per_minute: f64,
    pub p95_latency_ms: Option<u64>,
    pub p95_ttft_ms: Option<u64>,
    pub active_models: u64,
    pub active_shares: u64,
    pub failover_success_rate: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricEvent {
    pub id: Option<i64>,
    pub timestamp: i64,
    pub severity: String,
    pub kind: String,
    pub message: String,
    pub details: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSeriesResponse {
    pub range: String,
    pub step: String,
    pub host: Vec<HostMetricsPoint>,
    pub router: Vec<RouterMetricsPoint>,
    pub llm: Vec<LlmMetricsPoint>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostMetricsPoint {
    pub timestamp: i64,
    pub cpu_percent: Option<f64>,
    pub memory_usage_percent: Option<f64>,
    pub disk_usage_percent: Option<f64>,
    pub fd_usage_percent: Option<f64>,
    pub rx_bytes_per_sec: Option<f64>,
    pub tx_bytes_per_sec: Option<f64>,
    pub process_rss_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterMetricsPoint {
    pub timestamp: i64,
    pub active_routes: u64,
    pub forward_listeners: u64,
    pub proxy_inflight: u64,
    pub proxy_upstream_errors_total: u64,
    pub health_probe_failures_total: u64,
    pub db_errors_total: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmMetricsPoint {
    pub timestamp: i64,
    pub rpm: f64,
    pub tpm: f64,
    pub input_tpm: f64,
    pub output_tpm: f64,
    pub error_rate: f64,
    pub rate_limited: u64,
    pub p95_latency_ms: Option<u64>,
    pub p95_ttft_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmTopResponse {
    pub range: String,
    pub by: String,
    pub items: Vec<LlmTopItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmTopItem {
    pub key: String,
    pub requests: u64,
    pub total_tokens: u64,
    pub errors: u64,
    pub error_rate: f64,
    pub p95_latency_ms: Option<u64>,
    pub last_request_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClearMetricsResponse {
    pub ok: bool,
    pub deleted_rows: HashMap<String, u64>,
}

#[derive(Debug, Clone)]
pub struct LlmRequestMetric {
    pub timestamp: i64,
    pub request_id: Option<String>,
    pub route_type: String,
    pub market_email: Option<String>,
    pub share_id: Option<String>,
    pub subdomain: Option<String>,
    pub app_type: Option<String>,
    pub provider: Option<String>,
    pub requested_model: Option<String>,
    pub actual_model: Option<String>,
    pub status: String,
    pub error_kind: Option<String>,
    pub http_status: Option<u16>,
    pub latency_ms: Option<u64>,
    pub ttft_ms: Option<u64>,
    pub stream_started: bool,
    pub stream_completed: bool,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub estimated_cost_usd: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct MetricsRangeQuery {
    pub range: Option<String>,
    pub step: Option<String>,
    #[serde(rename = "groupBy")]
    pub group_by: Option<String>,
    pub by: Option<String>,
    pub limit: Option<usize>,
}
