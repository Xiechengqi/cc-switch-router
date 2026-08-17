use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use crate::config::{
    Config, MAX_REQUEST_LOG_RETENTION_DAYS, MIN_REQUEST_LOG_RETENTION_DAYS, SshTransportConfig,
};
use crate::dynamic_settings::DynamicSettings;
use crate::error::AppError;

/// Field type informs the frontend how to render the control and how the
/// backend validates the value.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    Text,
    Select,
    Int,
    Decimal,
    Bool,
    Path,
    Url,
    Email,
    EmailList,
    IpList,
    UrlList,
    Secret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsCategory {
    GeneralDisplay,
    Connectivity,
    DataLifecycle,
    IdentitySecurity,
    Notifications,
    Observability,
    Marketplace,
}

impl SettingsCategory {
    pub const ALL: [Self; 7] = [
        Self::GeneralDisplay,
        Self::Connectivity,
        Self::DataLifecycle,
        Self::IdentitySecurity,
        Self::Notifications,
        Self::Observability,
        Self::Marketplace,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::GeneralDisplay => "General & Display",
            Self::Connectivity => "Connectivity",
            Self::DataLifecycle => "Data & Lifecycle",
            Self::IdentitySecurity => "Identity & Security",
            Self::Notifications => "Notifications",
            Self::Observability => "Observability",
            Self::Marketplace => "Marketplace",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::GeneralDisplay => "Dashboard presentation and operator-facing links.",
            Self::Connectivity => "Public listeners, SSH tunnels, and proxy transport limits.",
            Self::DataLifecycle => "Database, file paths, leases, retention, and cleanup.",
            Self::IdentitySecurity => {
                "Login, registration admission, administrators, and access controls."
            }
            Self::Notifications => {
                "Email, user notifications, operator alerts, and delivery channels."
            }
            Self::Observability => "Metrics, clock health, and Server audit-log collection.",
            Self::Marketplace => "Share and Client Market policy, billing, and host intelligence.",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Normal,
    Caution,
    Critical,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsCategoryView {
    pub id: SettingsCategory,
    pub label: &'static str,
    pub description: &'static str,
    pub field_count: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldConstraints {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_items: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_items: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldDependency {
    pub key: &'static str,
    pub equals: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DynamicGroup {
    DashboardUx,
    AdminEmails,
    Security,
    Alerting,
    ClientNotifications,
    TelegramBot,
    MarketBilling,
    ServerLogs,
}

#[derive(Debug, Clone)]
pub struct SettingsField {
    pub key: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub field_type: FieldType,
    pub required: bool,
    pub restart_required: bool,
    pub default: Option<&'static str>,
    pub description: &'static str,
    pub placeholder: Option<&'static str>,
    pub dynamic_group: Option<DynamicGroup>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsFieldView {
    pub key: String,
    pub label: String,
    pub group: String,
    pub category: SettingsCategory,
    pub field_type: FieldType,
    pub required: bool,
    pub restart_required: bool,
    pub risk: RiskLevel,
    pub default: Option<String>,
    pub description: String,
    pub placeholder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<&'static str>,
    pub constraints: FieldConstraints,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<FieldDependency>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSchemaResponse {
    pub fields: Vec<SettingsFieldView>,
    pub groups: Vec<&'static str>,
    pub categories: Vec<SettingsCategoryView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingValueEntry {
    pub key: String,
    pub value: Option<String>,
    pub has_value: bool,
    pub is_secret: bool,
    pub source: ValueSource,
    pub effective_value: Option<String>,
    pub effective_has_value: bool,
    pub effective_source: ValueSource,
    pub pending_restart: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueSource {
    EnvFile,
    Default,
    Runtime,
    Unset,
}

#[cfg(test)]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsValuesResponse {
    pub values: Vec<SettingValueEntry>,
}

#[derive(Debug, Clone)]
pub struct SettingsRuntimeSnapshot {
    startup_effective: Arc<HashMap<String, String>>,
    startup_file_keys: Arc<HashSet<String>>,
}

impl SettingsRuntimeSnapshot {
    pub fn capture(env_path: &Path) -> Result<Self, AppError> {
        let startup_file_keys = read_env_file(env_path)?.into_keys().collect();
        let startup_effective = SETTINGS_FIELDS
            .iter()
            .filter_map(|field| {
                std::env::var(field.key)
                    .ok()
                    .map(|value| (field.key.to_string(), value))
            })
            .collect();
        Ok(Self {
            startup_effective: Arc::new(startup_effective),
            startup_file_keys: Arc::new(startup_file_keys),
        })
    }

    pub fn for_tests() -> Self {
        Self {
            startup_effective: Arc::new(HashMap::new()),
            startup_file_keys: Arc::new(HashSet::new()),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSnapshotResponse {
    pub revision: String,
    pub generated_at: String,
    pub env_path: String,
    pub schema: SettingsSchemaResponse,
    pub values: Vec<SettingValueEntry>,
    pub pending_restart_keys: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUpdateRequest {
    pub expected_revision: String,
    pub updates: BTreeMap<String, Option<String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsValidationResponse {
    pub valid: bool,
    pub field_errors: BTreeMap<String, Vec<String>>,
    pub form_errors: Vec<String>,
    pub restart_required_keys: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUpdateResponse {
    pub updated_keys: Vec<String>,
    pub unchanged_keys: Vec<String>,
    pub restart_required_keys: Vec<String>,
    pub dynamic_groups_refreshed: Vec<String>,
    pub env_path: String,
    pub revision: String,
}

/// The single source of truth for the entire env surface of the router.
pub const SETTINGS_FIELDS: &[SettingsField] = &[
    // ── Network & public address ──
    SettingsField {
        key: "CC_SWITCH_ROUTER_API_ADDR",
        label: "HTTP listen address",
        group: "Network",
        field_type: FieldType::Text,
        required: false,
        restart_required: true,
        default: Some("0.0.0.0:80"),
        description: "axum HTTP server bind address. Must be host:port.",
        placeholder: Some("0.0.0.0:80"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_SSH_ADDR",
        label: "SSH listen address",
        group: "Network",
        field_type: FieldType::Text,
        required: false,
        restart_required: true,
        default: Some("0.0.0.0:2222"),
        description: "russh server bind address for tunnel reverse forwarding.",
        placeholder: Some("0.0.0.0:2222"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_TUNNEL_DOMAIN",
        label: "Tunnel domain",
        group: "Network",
        field_type: FieldType::Text,
        required: true,
        restart_required: true,
        default: None,
        description: "Public host[:port]. Derives router@<host> as the built-in admin and \
             is sent to clients in lease responses.",
        placeholder: Some("router.example.com"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_SSH_PUBLIC_ADDR",
        label: "SSH public address",
        group: "Network",
        field_type: FieldType::Text,
        required: true,
        restart_required: true,
        default: None,
        description: "Public SSH host:port returned to clients. Must be reachable from clients.",
        placeholder: Some("router.example.com:2222"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_USE_LOCALHOST",
        label: "Use localhost (HTTP)",
        group: "Network",
        field_type: FieldType::Bool,
        required: false,
        restart_required: true,
        default: Some("false"),
        description: "When true, generated tunnel URLs use http://. Set false for HTTPS in production.",
        placeholder: None,
        dynamic_group: None,
    },
    // ── SSH transport lifecycle ──
    SettingsField {
        key: "CC_SWITCH_ROUTER_SSH_INACTIVITY_TIMEOUT_SECS",
        label: "SSH inactivity timeout (seconds)",
        group: "SSH transport",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("300"),
        description: "Close an inbound SSH session that has received no traffic within this interval (30-3600 seconds).",
        placeholder: Some("300"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_SSH_KEEPALIVE_INTERVAL_SECS",
        label: "SSH keepalive interval (seconds)",
        group: "SSH transport",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("30"),
        description: "Send a keepalive after this period without inbound SSH traffic (5-300 seconds).",
        placeholder: Some("30"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_SSH_KEEPALIVE_MAX",
        label: "Unanswered SSH keepalives",
        group: "SSH transport",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("3"),
        description: "Maximum unanswered keepalive probes before the SSH session is closed (1-10).",
        placeholder: Some("3"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_SSH_CHANNEL_OPEN_TIMEOUT_SECS",
        label: "Forward channel open timeout (seconds)",
        group: "SSH transport",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("15"),
        description: "Maximum time to wait for a client to confirm a forwarded TCP channel (1-120 seconds).",
        placeholder: Some("15"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_SSH_BRIDGE_WRITE_STALL_TIMEOUT_SECS",
        label: "Bridge write stall timeout (seconds)",
        group: "SSH transport",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("300"),
        description: "Close a bridge only when pending bytes make no write progress for this long (30-3600 seconds).",
        placeholder: Some("300"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_SSH_BRIDGE_HALF_CLOSE_IDLE_TIMEOUT_SECS",
        label: "Bridge half-close idle timeout (seconds)",
        group: "SSH transport",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("300"),
        description: "After one direction reaches EOF, wait this long without remaining-direction progress (30-3600 seconds).",
        placeholder: Some("300"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_SSH_MAX_FORWARD_CONNECTIONS",
        label: "Global forward connection limit",
        group: "SSH transport",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("2048"),
        description: "Maximum forwarded TCP connections waiting for a channel or actively bridging across all SSH tunnels (1-65536).",
        placeholder: Some("2048"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_SSH_MAX_FORWARD_CONNECTIONS_PER_TUNNEL",
        label: "Per-tunnel forward connection limit",
        group: "SSH transport",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("256"),
        description: "Maximum forwarded TCP connections waiting for a channel or actively bridging on one SSH tunnel (1-4096 and no more than the global limit).",
        placeholder: Some("256"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_PROXY_REQUEST_BODY_TIMEOUT_SECS",
        label: "Request body timeout (seconds)",
        group: "Proxy streaming",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("30"),
        description: "Stop reading a downstream request body after this interval (5-300 seconds). Share concurrency is acquired only after the body is complete.",
        placeholder: Some("30"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_PROXY_RESPONSE_HEADER_TIMEOUT_SECS",
        label: "Response header timeout (seconds)",
        group: "Proxy streaming",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("120"),
        description: "Cancel an upstream Share request when response headers do not arrive within 5-600 seconds.",
        placeholder: Some("120"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_PROXY_STREAM_FIRST_EVENT_TIMEOUT_SECS",
        label: "First stream event timeout (seconds)",
        group: "Proxy streaming",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("120"),
        description: "Close a Share stream when no protocol-level business event arrives within this interval (5-600 seconds). SSE comments and keepalives do not reset it.",
        placeholder: Some("120"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_PROXY_STREAM_IDLE_TIMEOUT_SECS",
        label: "Stream business idle timeout (seconds)",
        group: "Proxy streaming",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("900"),
        description: "Close a Share stream after this period without a protocol-level business event (30-3600 seconds). SSE comments and keepalives do not reset it.",
        placeholder: Some("900"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_PROXY_DOWNSTREAM_STALL_TIMEOUT_SECS",
        label: "Downstream stall timeout (seconds)",
        group: "Proxy streaming",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("120"),
        description: "Cancel a response pump when the downstream client stops consuming the bounded response buffer for 5-600 seconds.",
        placeholder: Some("120"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_PROXY_MAX_REQUEST_LIFETIME_SECS",
        label: "Request hard lifetime (seconds)",
        group: "Proxy streaming",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("7200"),
        description: "Absolute Share request lifetime (60-86400 seconds). It must exceed every phase timeout.",
        placeholder: Some("7200"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_PROXY_REQUEST_BODY_LIMIT_MB",
        label: "Request body limit (MB)",
        group: "Proxy streaming",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("10"),
        description: "Buffered request body ceiling for ordinary API paths such as /v1/responses and /v1/messages (1-64 MB). Oversized requests get 413 before they take a Share concurrency slot. Bodies are held in memory, so plan for this value times the in-flight request count. The client enforces its own ceiling too, so raising this alone will not admit larger bodies.",
        placeholder: Some("10"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_PROXY_MEDIA_REQUEST_BODY_LIMIT_MB",
        label: "Video request body limit (MB)",
        group: "Proxy streaming",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("32"),
        description: "Buffered request body ceiling for /v1/videos/generations (1-256 MB). Must not be lower than the ordinary request body limit.",
        placeholder: Some("32"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_PROXY_IMAGE_REQUEST_BODY_LIMIT_MB",
        label: "Image request body limit (MB)",
        group: "Proxy streaming",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("48"),
        description: "Buffered request body ceiling for /v1/images/generations and /v1/images/edits, which carry inline base64 attachments (1-256 MB). Must not be lower than the ordinary request body limit.",
        placeholder: Some("48"),
        dynamic_group: None,
    },
    // ── Persistence ──
    SettingsField {
        key: "CC_SWITCH_ROUTER_DATA_DIR",
        label: "Router data directory",
        group: "Persistence",
        field_type: FieldType::Path,
        required: false,
        restart_required: true,
        default: None,
        description: "Directory for Router-owned local files such as image results and Client Market SSH known_hosts.",
        placeholder: Some("/var/lib/cc-switch-router"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_DB_MODE",
        label: "Business database mode",
        group: "Persistence",
        field_type: FieldType::Select,
        required: false,
        restart_required: true,
        default: Some("local"),
        description: "Use local for a standalone libSQL file or turso for a Turso Cloud Embedded Replica.",
        placeholder: None,
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_DB_PATH",
        label: "Business database file",
        group: "Persistence",
        field_type: FieldType::Path,
        required: false,
        restart_required: true,
        default: None,
        description: "Local libSQL file in local mode, or the on-disk Embedded Replica file in Turso mode.",
        placeholder: Some("/var/lib/cc-switch-router/router.db"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_TURSO_URL",
        label: "Turso database URL",
        group: "Persistence",
        field_type: FieldType::Text,
        required: false,
        restart_required: true,
        default: None,
        description: "Turso Cloud database URL. Required in turso mode; must use libsql:// or https:// and contain no credentials, query, or fragment.",
        placeholder: Some("libsql://database-organization.turso.io"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_TURSO_AUTH_TOKEN",
        label: "Turso auth token",
        group: "Persistence",
        field_type: FieldType::Secret,
        required: false,
        restart_required: true,
        default: None,
        description: "Secret token used by the Embedded Replica for synchronization and delegated writes.",
        placeholder: Some("eyJhbGciOiJFZERTQSIs..."),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_DB_SYNC_INTERVAL_SECS",
        label: "Replica sync interval (seconds)",
        group: "Persistence",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("60"),
        description: "How often a Turso Embedded Replica pulls committed Cloud frames (1-3600 seconds).",
        placeholder: Some("60"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_HOST_KEY_PATH",
        label: "SSH host key path",
        group: "Persistence",
        field_type: FieldType::Path,
        required: false,
        restart_required: true,
        default: None,
        description: "Ed25519 SSH host key for inbound client tunnels. Auto-generated on first start when missing.",
        placeholder: Some("/var/lib/cc-switch-router/ssh_host_ed25519_key"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_PROVISION_SSH_PRIVATE_KEY_PATH",
        label: "Client Market SSH private key",
        group: "Persistence",
        field_type: FieldType::Path,
        required: false,
        restart_required: true,
        default: None,
        description: "Dedicated outbound SSH private key used to verify and provision Client Market hosts. Generated as Ed25519 on first start when missing.",
        placeholder: Some("~/.ssh/cc-switch-router-provision"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_PROVISION_SSH_PUBLIC_KEY_PATH",
        label: "Client Market SSH public key",
        group: "Persistence",
        field_type: FieldType::Path,
        required: false,
        restart_required: true,
        default: None,
        description: "Matching OpenSSH public key shown on Client Market for authorized_keys. Derived from the private key when missing.",
        placeholder: Some("~/.ssh/cc-switch-router-provision.pub"),
        dynamic_group: None,
    },
    // ── Lease / cleanup ──
    SettingsField {
        key: "CC_SWITCH_ROUTER_LEASE_TTL_SECS",
        label: "Lease TTL (seconds)",
        group: "Lease & Cleanup",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("60"),
        description: "How long a tunnel lease is valid before the client must renew.",
        placeholder: Some("60"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_CLEANUP_INTERVAL_SECS",
        label: "Cleanup interval (seconds)",
        group: "Lease & Cleanup",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("300"),
        description: "How often the background task purges expired leases / stale clients.",
        placeholder: Some("300"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_LEASE_RETENTION_SECS",
        label: "Lease retention (seconds)",
        group: "Lease & Cleanup",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("86400"),
        description: "Historical leases are kept this long before deletion. Default 1 day.",
        placeholder: Some("86400"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_PAUSED_SHARE_STALE_SECS",
        label: "Paused Share retention (seconds)",
        group: "Lease & Cleanup",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("3600"),
        description: "Delete paused Share records that have not been refreshed for this long.",
        placeholder: Some("3600"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_REQUEST_LOG_RETENTION_DAYS",
        label: "Request history retention (days)",
        group: "Lease & Cleanup",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("30"),
        description: "Share and image request history is kept for this many days (1-365).",
        placeholder: Some("30"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_SERVER_LOG_INGEST_ENABLED",
        label: "Server log collection",
        group: "Server logs",
        field_type: FieldType::Bool,
        required: false,
        restart_required: true,
        default: Some("true"),
        description: "Accept signed structured audit batches from registered Server installations.",
        placeholder: None,
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_SERVER_LOG_PUBLIC_ENABLED",
        label: "Public Server logs",
        group: "Server logs",
        field_type: FieldType::Bool,
        required: false,
        restart_required: false,
        default: Some("true"),
        description: "Allow anonymous visitors to view the public projection of events that occurred in the last five minutes.",
        placeholder: None,
        dynamic_group: Some(DynamicGroup::ServerLogs),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_SERVER_LOG_DATA_DIR",
        label: "Server log directory",
        group: "Server logs",
        field_type: FieldType::Path,
        required: false,
        restart_required: true,
        default: None,
        description: "Router-owned file directory for per-Client audit events and upload cursors; event bodies are never stored in the business database.",
        placeholder: Some("/var/lib/cc-switch-router/server-logs"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_SERVER_LOG_RETENTION_DAYS",
        label: "Server log retention (days)",
        group: "Server logs",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("7"),
        description: "Keep uploaded Server audit files for this many days (1-90).",
        placeholder: Some("7"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_SERVER_LOG_MAX_TOTAL_MIB",
        label: "Server log capacity (MiB)",
        group: "Server logs",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("1024"),
        description: "Maximum aggregate size of uploaded Server audit files before oldest segments are removed (16-1048576 MiB).",
        placeholder: Some("1024"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_CLIENT_STALE_SECS",
        label: "Client stale threshold (seconds)",
        group: "Lease & Cleanup",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("3600"),
        description: "Clients that have not heartbeat for this duration are marked offline.",
        placeholder: Some("3600"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_CLIENT_INSTALLATION_RETENTION_SECS",
        label: "Client installation retention (seconds)",
        group: "Lease & Cleanup",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("21600"),
        description: "Offline installation records are deleted after this duration. Must be >= stale threshold.",
        placeholder: Some("21600"),
        dynamic_group: None,
    },
    // ── Registration admission ──
    SettingsField {
        key: "CC_SWITCH_ROUTER_REGISTRATION_SOURCE_RATE_PER_MINUTE",
        label: "Source attempt rate / minute",
        group: "Registration admission",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("60"),
        description: "Sustained registration attempts allowed per trusted source each minute.",
        placeholder: Some("60"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_REGISTRATION_SOURCE_BURST",
        label: "Source attempt burst",
        group: "Registration admission",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("20"),
        description: "Short registration attempt burst allowed per trusted source.",
        placeholder: Some("20"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_REGISTRATION_GLOBAL_RATE_PER_MINUTE",
        label: "Global attempt rate / minute",
        group: "Registration admission",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("600"),
        description: "Sustained registration attempts allowed across the router each minute.",
        placeholder: Some("600"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_REGISTRATION_GLOBAL_BURST",
        label: "Global attempt burst",
        group: "Registration admission",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("200"),
        description: "Short registration attempt burst allowed across the router.",
        placeholder: Some("200"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_REGISTRATION_KEY_RATE_PER_MINUTE",
        label: "Key attempt rate / minute",
        group: "Registration admission",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("10"),
        description: "Sustained registration attempts allowed per public key each minute.",
        placeholder: Some("10"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_REGISTRATION_KEY_BURST",
        label: "Key attempt burst",
        group: "Registration admission",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("3"),
        description: "Short registration attempt burst allowed per public key.",
        placeholder: Some("3"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_REGISTRATION_BUCKET_IDLE_SECS",
        label: "Attempt counter idle time (seconds)",
        group: "Registration admission",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("600"),
        description: "Idle time before per-source and per-key attempt counters are released.",
        placeholder: Some("600"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_REGISTRATION_MAX_SOURCE_BUCKETS",
        label: "Maximum source counters",
        group: "Registration admission",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("8192"),
        description: "Maximum in-memory source attempt counters retained at once.",
        placeholder: Some("8192"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_REGISTRATION_MAX_KEY_BUCKETS",
        label: "Maximum key counters",
        group: "Registration admission",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("16384"),
        description: "Maximum in-memory public-key attempt counters retained at once.",
        placeholder: Some("16384"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_10M_LIMIT",
        label: "Source new identities / 10 minutes",
        group: "Registration admission",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("30"),
        description: "New installation identities allowed per source in ten minutes.",
        placeholder: Some("30"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_HOURLY_LIMIT",
        label: "Source new identities / hour",
        group: "Registration admission",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("100"),
        description: "New installation identities allowed per source in one hour.",
        placeholder: Some("100"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_DAILY_LIMIT",
        label: "Source new identities / day",
        group: "Registration admission",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("300"),
        description: "New installation identities allowed per source in one day.",
        placeholder: Some("300"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_10M_LIMIT",
        label: "Global new identities / 10 minutes",
        group: "Registration admission",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("300"),
        description: "New installation identities allowed across the router in ten minutes.",
        placeholder: Some("300"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_HOURLY_LIMIT",
        label: "Global new identities / hour",
        group: "Registration admission",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("1000"),
        description: "New installation identities allowed across the router in one hour.",
        placeholder: Some("1000"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_DAILY_LIMIT",
        label: "Global new identities / day",
        group: "Registration admission",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("5000"),
        description: "New installation identities allowed across the router in one day.",
        placeholder: Some("5000"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_REGISTRATION_UNOWNED_INSTALLATION_WATERMARK",
        label: "Unowned installation watermark",
        group: "Registration admission",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("50000"),
        description: "Maximum unowned installation records allowed before new identity admission is paused.",
        placeholder: Some("50000"),
        dynamic_group: None,
    },
    // ── Email (Resend) ──
    SettingsField {
        key: "CC_SWITCH_ROUTER_RESEND_API_KEY",
        label: "Resend API key",
        group: "Email (Resend)",
        field_type: FieldType::Secret,
        required: true,
        restart_required: true,
        default: None,
        description: "re_xxx API key from Resend. Required for verification and client lifecycle emails.",
        placeholder: Some("re_…"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_RESEND_FROM",
        label: "Sender address",
        group: "Email (Resend)",
        field_type: FieldType::Email,
        required: false,
        restart_required: true,
        default: Some("noreply@[CC_SWITCH_ROUTER_TUNNEL_DOMAIN]"),
        description: "From: address used for outgoing mail. Use a Resend-verified domain with aligned SPF, DKIM, and DMARC; defaults to noreply@<tunnel-domain-host>.",
        placeholder: Some("noreply@example.com"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_RESEND_FROM_NAME",
        label: "Sender display name",
        group: "Email (Resend)",
        field_type: FieldType::Text,
        required: false,
        restart_required: true,
        default: None,
        description: "Display name attached to the From: address.",
        placeholder: Some("CC-Switch Router"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_RESEND_REPLY_TO",
        label: "Reply-To address",
        group: "Email (Resend)",
        field_type: FieldType::Email,
        required: false,
        restart_required: true,
        default: None,
        description: "Optional Reply-To header for verification and client lifecycle emails.",
        placeholder: Some("support@example.com"),
        dynamic_group: None,
    },
    // ── Client lifecycle notifications ──
    SettingsField {
        key: "CC_SWITCH_ROUTER_CLIENT_EMAIL_NOTIFICATIONS_ENABLED",
        label: "Client email notifications",
        group: "Client notifications",
        field_type: FieldType::Bool,
        required: false,
        restart_required: false,
        default: Some("true"),
        description: "Send registration and offline alerts to each client's currently verified owner email.",
        placeholder: None,
        dynamic_group: Some(DynamicGroup::ClientNotifications),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_CLIENT_OFFLINE_ALERT_SECS",
        label: "Offline confirmation (seconds)",
        group: "Client notifications",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("180"),
        description: "Authenticated heartbeat silence required before a client is confirmed offline (minimum 180 seconds).",
        placeholder: Some("180"),
        dynamic_group: Some(DynamicGroup::ClientNotifications),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_CLIENT_RECOVERY_STABLE_SECS",
        label: "Offline episode close (seconds)",
        group: "Client notifications",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("120"),
        description: "Continuous authenticated heartbeats required before an offline episode is closed. This does not start or restart the Client process.",
        placeholder: Some("120"),
        dynamic_group: Some(DynamicGroup::ClientNotifications),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_CLIENT_ALERT_COOLDOWN_SECS",
        label: "Per-client cooldown (seconds)",
        group: "Client notifications",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("1800"),
        description: "Minimum interval between offline alerts for the same client.",
        placeholder: Some("1800"),
        dynamic_group: Some(DynamicGroup::ClientNotifications),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_CLIENT_ALERT_BATCH_WINDOW_SECS",
        label: "Batch window (seconds)",
        group: "Client notifications",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("60"),
        description: "Offline events for the same recipient are combined within this window; authenticated registrations use a five-second debounce.",
        placeholder: Some("60"),
        dynamic_group: Some(DynamicGroup::ClientNotifications),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_WINDOW_SECS",
        label: "Storm detection window (seconds)",
        group: "Client notifications",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("300"),
        description: "Window used to detect a correlated multi-client outage.",
        placeholder: Some("300"),
        dynamic_group: Some(DynamicGroup::ClientNotifications),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_MIN_CLIENTS",
        label: "Storm minimum clients",
        group: "Client notifications",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("5"),
        description: "Absolute registration or offline event count that can trigger incident digest mode.",
        placeholder: Some("5"),
        dynamic_group: Some(DynamicGroup::ClientNotifications),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_PERCENT",
        label: "Storm monitored-client percentage",
        group: "Client notifications",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("20"),
        description: "Percentage of monitored clients offline that triggers incident digest mode; registration bursts use the absolute threshold.",
        placeholder: Some("20"),
        dynamic_group: Some(DynamicGroup::ClientNotifications),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_REMINDER_SECS",
        label: "Storm digest interval (seconds)",
        group: "Client notifications",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("1800"),
        description: "Minimum interval between digest updates for the same active incident.",
        placeholder: Some("1800"),
        dynamic_group: Some(DynamicGroup::ClientNotifications),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_CLIENT_ALERT_RECIPIENT_HOURLY_LIMIT",
        label: "Offline per-recipient hourly cap",
        group: "Client notifications",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("10"),
        description: "Maximum offline notifications sent to one recipient per hour.",
        placeholder: Some("10"),
        dynamic_group: Some(DynamicGroup::ClientNotifications),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_CLIENT_ALERT_GLOBAL_HOURLY_LIMIT",
        label: "Offline global hourly cap",
        group: "Client notifications",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("50"),
        description: "Maximum offline notifications sent by this router per hour.",
        placeholder: Some("50"),
        dynamic_group: Some(DynamicGroup::ClientNotifications),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_RECIPIENT_HOURLY_LIMIT",
        label: "Registration per-recipient hourly cap",
        group: "Client notifications",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("3"),
        description: "Maximum registration notifications sent to one recipient per hour.",
        placeholder: Some("3"),
        dynamic_group: Some(DynamicGroup::ClientNotifications),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_GLOBAL_HOURLY_LIMIT",
        label: "Registration global hourly cap",
        group: "Client notifications",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("10"),
        description: "Maximum registration notifications sent by this router per hour.",
        placeholder: Some("10"),
        dynamic_group: Some(DynamicGroup::ClientNotifications),
    },
    // ── Auth code / session ──
    SettingsField {
        key: "CC_SWITCH_ROUTER_AUTH_CODE_TTL_SECS",
        label: "Verification code TTL (seconds)",
        group: "Email verification & session",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("300"),
        description: "How long an emailed login code stays valid.",
        placeholder: Some("300"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_AUTH_CODE_COOLDOWN_SECS",
        label: "Resend cooldown (seconds)",
        group: "Email verification & session",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("60"),
        description: "Minimum interval between consecutive code requests for the same email.",
        placeholder: Some("60"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_AUTH_SESSION_TTL_SECS",
        label: "Access token TTL (seconds)",
        group: "Email verification & session",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("1800"),
        description: "Lifetime of an access token before refresh is required.",
        placeholder: Some("1800"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_AUTH_REFRESH_TTL_SECS",
        label: "Refresh token TTL (seconds)",
        group: "Email verification & session",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("2592000"),
        description: "How long a refresh token can be used before requiring login again. Default 30 days.",
        placeholder: Some("2592000"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_AUTH_MAX_VERIFY_ATTEMPTS",
        label: "Verify attempts cap",
        group: "Email verification & session",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("5"),
        description: "Maximum wrong code attempts per challenge before lockout.",
        placeholder: Some("5"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_AUTH_EMAIL_HOURLY_LIMIT",
        label: "Per-email hourly limit",
        group: "Email verification & session",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("30"),
        description: "Maximum login code requests per email per hour.",
        placeholder: Some("30"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_AUTH_IP_HOURLY_LIMIT",
        label: "Per-IP hourly limit",
        group: "Email verification & session",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("20"),
        description: "Maximum login code requests per source IP per hour.",
        placeholder: Some("20"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_AUTH_SOURCE_HOURLY_LIMIT",
        label: "Per-auth-source hourly limit",
        group: "Email verification & session",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("10"),
        description: "Maximum login code requests per authentication source per hour.",
        placeholder: Some("10"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_IP_BLACKLIST",
        label: "IP blacklist",
        group: "Security",
        field_type: FieldType::IpList,
        required: false,
        restart_required: false,
        default: None,
        description: "Comma, whitespace, or newline-separated source IP/CIDR entries blocked at the HTTP edge. Applies immediately.",
        placeholder: Some("203.0.113.10\n198.51.100.0/24\n2001:db8::/32"),
        dynamic_group: Some(DynamicGroup::Security),
    },
    // ── Free share ──
    SettingsField {
        key: "CC_SWITCH_ROUTER_FREE_SHARE_IP_PARALLEL_LIMIT",
        label: "Free share parallel limit / IP",
        group: "Free share",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("1"),
        description: "Concurrent free share requests allowed per source IP. Set 0 to disable the limit.",
        placeholder: Some("1"),
        dynamic_group: None,
    },
    // ── Market billing ──
    SettingsField {
        key: "CC_SWITCH_ROUTER_MARKET_USD_CNY_RATE",
        label: "USD/CNY exchange rate",
        group: "Market billing",
        field_type: FieldType::Decimal,
        required: false,
        restart_required: false,
        default: Some("7"),
        description: "CNY received for 1 USD. Applies immediately to unbilled estimates and is frozen when each invoice opens (0.01-100, up to 6 decimal places).",
        placeholder: Some("7"),
        dynamic_group: Some(DynamicGroup::MarketBilling),
    },
    // ── External verification ──
    SettingsField {
        key: "CC_SWITCH_ROUTER_VERIFICATION_SERVICE_BASE_URL",
        label: "Verification service URL",
        group: "External verification",
        field_type: FieldType::Url,
        required: false,
        restart_required: true,
        default: Some("https://tokenswitch.org"),
        description: "External service used to redeem owner-email verification tokens.",
        placeholder: Some("https://tokenswitch.org"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_VERIFICATION_SERVICE_API_KEY",
        label: "Verification service API key",
        group: "External verification",
        field_type: FieldType::Secret,
        required: false,
        restart_required: true,
        default: None,
        description: "Optional shared secret for the verification service.",
        placeholder: Some("…"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_IP_INTEL_ENDPOINTS",
        label: "Host IP intelligence endpoints",
        group: "Client Market",
        field_type: FieldType::UrlList,
        required: false,
        restart_required: true,
        default: Some("http://3.0.3.0,http://3.0.2.1,http://3.0.2.9"),
        description: "Ordered base URLs used to enrich Client Market Host IPs. Every registered Host IP is disclosed to these services; use trusted endpoints.",
        placeholder: Some("https://ip-intel-1.example.com, https://ip-intel-2.example.com"),
        dynamic_group: None,
    },
    // ── Client Market ──
    SettingsField {
        key: "CC_SWITCH_ROUTER_OWNER_EMAIL",
        label: "Official Client Market Provider",
        group: "Client Market",
        field_type: FieldType::Email,
        required: false,
        restart_required: true,
        default: None,
        description: "Email used as the official, default-selected Client Market Host Provider. Defaults to router@<tunnel-host> when unset.",
        placeholder: Some("router@example.com"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_ADMIN_EMAILS",
        label: "Extra admin emails",
        group: "Administrators",
        field_type: FieldType::EmailList,
        required: false,
        restart_required: false,
        default: None,
        description: "Comma-separated extra admin emails. router@<tunnel-host> is always admin.",
        placeholder: Some("ops@example.com, sre@example.com"),
        dynamic_group: Some(DynamicGroup::AdminEmails),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_UX_TELEMETRY_ENABLED",
        label: "Enable local UX telemetry",
        group: "Dashboard UX",
        field_type: FieldType::Bool,
        required: false,
        restart_required: true,
        default: Some("false"),
        description: "Store privacy-minimized dashboard interaction events locally. No entity ids, emails, URLs, addresses, tokens, or request content are recorded.",
        placeholder: None,
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_UX_TELEMETRY_RETENTION_DAYS",
        label: "UX telemetry retention (days)",
        group: "Dashboard UX",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("7"),
        description: "How long local dashboard UX events are retained.",
        placeholder: Some("7"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_FOOTER_TELEGRAM_URL",
        label: "Footer Telegram URL",
        group: "Dashboard UX",
        field_type: FieldType::Url,
        required: false,
        restart_required: false,
        default: None,
        description: "Telegram link shown in the dashboard footer next to GitHub. Clear to hide.",
        placeholder: Some("https://t.me/tokenswitchorg"),
        dynamic_group: Some(DynamicGroup::DashboardUx),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_METRICS_ENABLED",
        label: "Enable metrics",
        group: "Metrics",
        field_type: FieldType::Bool,
        required: false,
        restart_required: true,
        default: Some("true"),
        description: "Collect host, router, and LLM metrics into a separate metrics database.",
        placeholder: None,
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_METRICS_DB_PATH",
        label: "Metrics DB path",
        group: "Metrics",
        field_type: FieldType::Path,
        required: false,
        restart_required: true,
        default: None,
        description: "SQLite file used only for metrics history. This is separate from the business database.",
        placeholder: Some("$HOME/.cc-switch-router/cc-switch-router-metrics.db"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_METRICS_RETENTION_DAYS",
        label: "Metrics retention days",
        group: "Metrics",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("7"),
        description: "Number of days to keep metrics samples before automatic pruning.",
        placeholder: Some("7"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_METRICS_SAMPLE_INTERVAL_SECS",
        label: "Metrics sample interval",
        group: "Metrics",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("5"),
        description: "Sampling interval in seconds for host and router metrics.",
        placeholder: Some("5"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_CLOCK_MONITOR_ENABLED",
        label: "Enable clock monitor",
        group: "Clock health",
        field_type: FieldType::Bool,
        required: false,
        restart_required: true,
        default: Some("true"),
        description: "Observe Router clock drift using independent HTTPS Date sources. The monitor never changes the system clock.",
        placeholder: None,
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_CLOCK_PROBE_INTERVAL_SECS",
        label: "Clock probe interval (seconds)",
        group: "Clock health",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("60"),
        description: "Interval between clock-health probe cycles (15-3600 seconds).",
        placeholder: Some("60"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_CLOCK_PROBE_TIMEOUT_SECS",
        label: "Clock probe timeout (seconds)",
        group: "Clock health",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("4"),
        description: "Complete request timeout for each clock source (1-15 seconds).",
        placeholder: Some("4"),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_CLOCK_SOURCES",
        label: "Clock reference sources",
        group: "Clock health",
        field_type: FieldType::UrlList,
        required: false,
        restart_required: true,
        default: Some(
            "https://www.cloudflare.com/cdn-cgi/trace,https://www.apple.com/library/test/success.html,https://checkip.amazonaws.com/",
        ),
        description: "Three to five HTTPS URLs on distinct hosts. At least two agreeing sources are required for a trusted offset sample.",
        placeholder: Some(
            "https://source-1.example.com, https://source-2.example.com, https://source-3.example.com",
        ),
        dynamic_group: None,
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_ALERTING_ENABLED",
        label: "Enable operator alerts",
        group: "Alerting",
        field_type: FieldType::Bool,
        required: false,
        restart_required: false,
        default: Some("true"),
        description: "Persist incidents at all times, and enqueue enabled IM channel deliveries while this switch is on.",
        placeholder: None,
        dynamic_group: Some(DynamicGroup::Alerting),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_ALERT_REPEAT_INTERVAL_SECS",
        label: "Firing reminder interval",
        group: "Alerting",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("1800"),
        description: "Minimum seconds between reminders for an unacknowledged incident that remains active.",
        placeholder: Some("1800"),
        dynamic_group: Some(DynamicGroup::Alerting),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_ALERT_HISTORY_RETENTION_DAYS",
        label: "Resolved incident retention",
        group: "Alerting",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("90"),
        description: "Days to retain resolved incidents and their delivery audit history. Active incidents are never pruned.",
        placeholder: Some("90"),
        dynamic_group: Some(DynamicGroup::Alerting),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_ALERT_TELEGRAM_ENABLED",
        label: "Enable Telegram alerts",
        group: "Telegram alerts",
        field_type: FieldType::Bool,
        required: false,
        restart_required: false,
        default: Some("false"),
        description: "Deliver new, escalated, reminder, resumed, and recovery transitions through Telegram.",
        placeholder: None,
        dynamic_group: Some(DynamicGroup::Alerting),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_ALERT_TELEGRAM_BOT_TOKEN",
        label: "Telegram bot token",
        group: "Telegram alerts",
        field_type: FieldType::Secret,
        required: false,
        restart_required: false,
        default: None,
        description: "Bot token issued by @BotFather. It is never returned by the Settings API or written to logs.",
        placeholder: Some("123456:ABC..."),
        dynamic_group: Some(DynamicGroup::Alerting),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_ALERT_TELEGRAM_CHAT_ID",
        label: "Telegram chat id",
        group: "Telegram alerts",
        field_type: FieldType::Text,
        required: false,
        restart_required: false,
        default: None,
        description: "Target user, group, or channel chat id.",
        placeholder: Some("-1001234567890"),
        dynamic_group: Some(DynamicGroup::Alerting),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_ALERT_TELEGRAM_TOPIC_ID",
        label: "Telegram forum topic id",
        group: "Telegram alerts",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: None,
        description: "Optional message_thread_id for a forum topic in a supergroup.",
        placeholder: Some("42"),
        dynamic_group: Some(DynamicGroup::Alerting),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_ALERT_TELEGRAM_MIN_SEVERITY",
        label: "Telegram minimum severity",
        group: "Telegram alerts",
        field_type: FieldType::Select,
        required: false,
        restart_required: false,
        default: Some("warning"),
        description: "Lowest incident severity delivered to Telegram.",
        placeholder: None,
        dynamic_group: Some(DynamicGroup::Alerting),
    },
    // ---- User-facing Telegram notification bot -----------------------------
    // Deliberately separate from "Telegram alerts" above: that bot is a
    // send-only operator channel pinned to one chat; this one receives
    // `/start <token>` deep links and fans notifications out to per-user
    // chats. The same token may be pasted into both, but rotating one must
    // never silently break the other.
    SettingsField {
        key: "CC_SWITCH_ROUTER_TELEGRAM_BOT_ENABLED",
        label: "Enable Telegram notification bot",
        group: "Telegram bot",
        field_type: FieldType::Bool,
        required: false,
        restart_required: false,
        default: Some("false"),
        description: "Let users bind a Telegram account on the Account page and receive notifications there. Turning this off hides the binding UI and stops Telegram delivery; existing bindings are preserved.",
        placeholder: None,
        dynamic_group: Some(DynamicGroup::TelegramBot),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_TELEGRAM_BOT_TOKEN",
        label: "Telegram bot token",
        group: "Telegram bot",
        field_type: FieldType::Secret,
        required: false,
        restart_required: false,
        default: None,
        description: "Bot token issued by @BotFather. Never returned by the Settings API or written to logs. Changing it invalidates every pending bind link.",
        placeholder: Some("123456:ABC..."),
        dynamic_group: Some(DynamicGroup::TelegramBot),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_TELEGRAM_BOT_MODE",
        label: "Telegram update mode",
        group: "Telegram bot",
        field_type: FieldType::Select,
        required: false,
        restart_required: false,
        default: Some("polling"),
        description: "polling: the Router long-polls getUpdates (no inbound connectivity needed, single Router process only). webhook: Telegram pushes to /v1/integrations/telegram/webhook and a webhook secret is required.",
        placeholder: None,
        dynamic_group: Some(DynamicGroup::TelegramBot),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_TELEGRAM_WEBHOOK_SECRET",
        label: "Telegram webhook secret",
        group: "Telegram bot",
        field_type: FieldType::Secret,
        required: false,
        restart_required: false,
        default: None,
        description: "Shared secret sent back as X-Telegram-Bot-Api-Secret-Token. Required in webhook mode; 16-256 chars of A-Z a-z 0-9 _ -.",
        placeholder: Some("32+ random characters"),
        dynamic_group: Some(DynamicGroup::TelegramBot),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_TELEGRAM_BIND_TOKEN_TTL_SECS",
        label: "Bind link lifetime (seconds)",
        group: "Telegram bot",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("900"),
        description: "How long a t.me bind link stays valid. Between 60 and 86400 seconds.",
        placeholder: Some("900"),
        dynamic_group: Some(DynamicGroup::TelegramBot),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_TELEGRAM_RECIPIENT_HOURLY_LIMIT",
        label: "Telegram per-user hourly cap",
        group: "Telegram bot",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("10"),
        description: "Maximum Telegram notifications delivered to one user per hour. Counted independently from the email caps.",
        placeholder: Some("10"),
        dynamic_group: Some(DynamicGroup::TelegramBot),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_TELEGRAM_GLOBAL_HOURLY_LIMIT",
        label: "Telegram global hourly cap",
        group: "Telegram bot",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("50"),
        description: "Maximum Telegram notifications delivered across all users per hour.",
        placeholder: Some("50"),
        dynamic_group: Some(DynamicGroup::TelegramBot),
    },
];

pub fn schema_response() -> SettingsSchemaResponse {
    let mut groups = Vec::new();
    let mut seen = HashSet::new();
    for field in SETTINGS_FIELDS {
        if seen.insert(field.group) {
            groups.push(field.group);
        }
    }
    let fields = SETTINGS_FIELDS
        .iter()
        .map(|field| {
            let mut view = field_to_view(field);
            match field.key {
                "CC_SWITCH_ROUTER_DATA_DIR" => {
                    let path = crate::config::default_data_dir().display().to_string();
                    view.placeholder = Some(path);
                }
                "CC_SWITCH_ROUTER_DB_PATH" => {
                    let path = crate::config::default_db_path().display().to_string();
                    view.placeholder = Some(path);
                }
                "CC_SWITCH_ROUTER_HOST_KEY_PATH" => {
                    let path = crate::config::default_host_key_path().display().to_string();
                    view.placeholder = Some(path);
                }
                "CC_SWITCH_ROUTER_PROVISION_SSH_PRIVATE_KEY_PATH" => {
                    let path = crate::config::default_provision_ssh_private_key_path()
                        .display()
                        .to_string();
                    view.default = Some(path.clone());
                    view.placeholder = Some(path);
                }
                "CC_SWITCH_ROUTER_PROVISION_SSH_PUBLIC_KEY_PATH" => {
                    let path = crate::config::default_provision_ssh_public_key_path()
                        .display()
                        .to_string();
                    view.default = Some(path.clone());
                    view.placeholder = Some(path);
                }
                "CC_SWITCH_ROUTER_METRICS_DB_PATH" => {
                    let path = crate::config::default_metrics_db_path()
                        .display()
                        .to_string();
                    view.default = Some(path.clone());
                    view.placeholder = Some(path);
                }
                _ => {}
            }
            view
        })
        .collect::<Vec<_>>();
    let categories = SettingsCategory::ALL
        .into_iter()
        .map(|category| SettingsCategoryView {
            id: category,
            label: category.label(),
            description: category.description(),
            field_count: fields
                .iter()
                .filter(|field| field.category == category)
                .count(),
        })
        .collect();
    SettingsSchemaResponse {
        fields,
        groups,
        categories,
    }
}

fn field_to_view(field: &SettingsField) -> SettingsFieldView {
    SettingsFieldView {
        key: field.key.to_string(),
        label: field.label.to_string(),
        group: field.group.to_string(),
        category: category_for_group(field.group),
        field_type: field.field_type,
        required: field.required,
        restart_required: field.restart_required,
        risk: risk_for_field(field),
        default: field.default.map(str::to_string),
        description: field.description.to_string(),
        placeholder: field.placeholder.map(str::to_string),
        unit: unit_for_field(field),
        constraints: constraints_for_field(field.key),
        dependencies: dependencies_for_field(field.key),
        options: match field.key {
            "CC_SWITCH_ROUTER_DB_MODE" => vec!["local".into(), "turso".into()],
            "CC_SWITCH_ROUTER_ALERT_TELEGRAM_MIN_SEVERITY" => {
                vec!["info".into(), "warning".into(), "critical".into()]
            }
            "CC_SWITCH_ROUTER_TELEGRAM_BOT_MODE" => {
                vec!["polling".into(), "webhook".into()]
            }
            _ => Vec::new(),
        },
    }
}

fn category_for_group(group: &str) -> SettingsCategory {
    match group {
        "Dashboard UX" => SettingsCategory::GeneralDisplay,
        "Network" | "SSH transport" | "Proxy streaming" => SettingsCategory::Connectivity,
        "Persistence" | "Lease & Cleanup" => SettingsCategory::DataLifecycle,
        "Registration admission"
        | "Email verification & session"
        | "Security"
        | "External verification"
        | "Administrators" => SettingsCategory::IdentitySecurity,
        "Email (Resend)"
        | "Client notifications"
        | "Alerting"
        | "Telegram alerts"
        | "Telegram bot" => SettingsCategory::Notifications,
        "Metrics" | "Clock health" | "Server logs" => SettingsCategory::Observability,
        "Free share" | "Market billing" | "Client Market" => SettingsCategory::Marketplace,
        _ => SettingsCategory::GeneralDisplay,
    }
}

fn risk_for_field(field: &SettingsField) -> RiskLevel {
    match field.key {
        "CC_SWITCH_ROUTER_API_ADDR"
        | "CC_SWITCH_ROUTER_SSH_ADDR"
        | "CC_SWITCH_ROUTER_TUNNEL_DOMAIN"
        | "CC_SWITCH_ROUTER_DB_MODE"
        | "CC_SWITCH_ROUTER_DB_PATH"
        | "CC_SWITCH_ROUTER_TURSO_URL"
        | "CC_SWITCH_ROUTER_TURSO_AUTH_TOKEN"
        | "CC_SWITCH_ROUTER_HOST_KEY_PATH"
        | "CC_SWITCH_ROUTER_PROVISION_SSH_PRIVATE_KEY_PATH"
        | "CC_SWITCH_ROUTER_PROVISION_SSH_PUBLIC_KEY_PATH"
        | "CC_SWITCH_ROUTER_ADMIN_EMAILS"
        | "CC_SWITCH_ROUTER_IP_BLACKLIST" => RiskLevel::Critical,
        _ if field.restart_required || matches!(field.field_type, FieldType::Secret) => {
            RiskLevel::Caution
        }
        _ => RiskLevel::Normal,
    }
}

fn unit_for_field(field: &SettingsField) -> Option<&'static str> {
    if field.key.ends_with("_SECS") {
        Some("seconds")
    } else if field.key.ends_with("_DAYS") {
        Some("days")
    } else if field.key.ends_with("_MB") || field.key.ends_with("_MIB") {
        Some("MiB")
    } else if field.key.ends_with("_PERCENT") {
        Some("percent")
    } else {
        None
    }
}

fn number_constraints(min: f64, max: f64) -> FieldConstraints {
    FieldConstraints {
        min: Some(min),
        max: Some(max),
        step: Some(1.0),
        ..FieldConstraints::default()
    }
}

fn constraints_for_field(key: &str) -> FieldConstraints {
    let range = match key {
        "CC_SWITCH_ROUTER_SSH_INACTIVITY_TIMEOUT_SECS"
        | "CC_SWITCH_ROUTER_SSH_BRIDGE_WRITE_STALL_TIMEOUT_SECS"
        | "CC_SWITCH_ROUTER_SSH_BRIDGE_HALF_CLOSE_IDLE_TIMEOUT_SECS" => Some((30.0, 3_600.0)),
        "CC_SWITCH_ROUTER_SSH_KEEPALIVE_INTERVAL_SECS" => Some((5.0, 300.0)),
        "CC_SWITCH_ROUTER_SSH_KEEPALIVE_MAX" => Some((1.0, 10.0)),
        "CC_SWITCH_ROUTER_SSH_CHANNEL_OPEN_TIMEOUT_SECS" => Some((1.0, 120.0)),
        "CC_SWITCH_ROUTER_SSH_MAX_FORWARD_CONNECTIONS" => Some((1.0, 65_536.0)),
        "CC_SWITCH_ROUTER_SSH_MAX_FORWARD_CONNECTIONS_PER_TUNNEL" => Some((1.0, 4_096.0)),
        "CC_SWITCH_ROUTER_PROXY_REQUEST_BODY_TIMEOUT_SECS"
        | "CC_SWITCH_ROUTER_PROXY_DOWNSTREAM_STALL_TIMEOUT_SECS" => Some((5.0, 300.0)),
        "CC_SWITCH_ROUTER_PROXY_RESPONSE_HEADER_TIMEOUT_SECS"
        | "CC_SWITCH_ROUTER_PROXY_STREAM_FIRST_EVENT_TIMEOUT_SECS" => Some((5.0, 600.0)),
        "CC_SWITCH_ROUTER_PROXY_STREAM_IDLE_TIMEOUT_SECS" => Some((30.0, 3_600.0)),
        "CC_SWITCH_ROUTER_PROXY_MAX_REQUEST_LIFETIME_SECS" => Some((60.0, 86_400.0)),
        "CC_SWITCH_ROUTER_PROXY_REQUEST_BODY_LIMIT_MB" => Some((1.0, 64.0)),
        "CC_SWITCH_ROUTER_PROXY_MEDIA_REQUEST_BODY_LIMIT_MB"
        | "CC_SWITCH_ROUTER_PROXY_IMAGE_REQUEST_BODY_LIMIT_MB" => Some((1.0, 256.0)),
        "CC_SWITCH_ROUTER_DB_SYNC_INTERVAL_SECS" => Some((1.0, 3_600.0)),
        "CC_SWITCH_ROUTER_LEASE_TTL_SECS" => Some((10.0, 3_600.0)),
        "CC_SWITCH_ROUTER_CLEANUP_INTERVAL_SECS" => Some((10.0, 86_400.0)),
        "CC_SWITCH_ROUTER_LEASE_RETENTION_SECS"
        | "CC_SWITCH_ROUTER_CLIENT_INSTALLATION_RETENTION_SECS" => Some((60.0, 31_536_000.0)),
        "CC_SWITCH_ROUTER_PAUSED_SHARE_STALE_SECS" => Some((60.0, 2_592_000.0)),
        "CC_SWITCH_ROUTER_REQUEST_LOG_RETENTION_DAYS" => Some((1.0, 365.0)),
        "CC_SWITCH_ROUTER_SERVER_LOG_RETENTION_DAYS" => Some((1.0, 90.0)),
        "CC_SWITCH_ROUTER_SERVER_LOG_MAX_TOTAL_MIB" => Some((16.0, 1_048_576.0)),
        "CC_SWITCH_ROUTER_CLIENT_STALE_SECS" => Some((60.0, 604_800.0)),
        "CC_SWITCH_ROUTER_REGISTRATION_SOURCE_RATE_PER_MINUTE" => Some((1.0, 6_000.0)),
        "CC_SWITCH_ROUTER_REGISTRATION_SOURCE_BURST" => Some((1.0, 1_000.0)),
        "CC_SWITCH_ROUTER_REGISTRATION_GLOBAL_RATE_PER_MINUTE" => Some((1.0, 60_000.0)),
        "CC_SWITCH_ROUTER_REGISTRATION_GLOBAL_BURST" => Some((1.0, 10_000.0)),
        "CC_SWITCH_ROUTER_REGISTRATION_KEY_RATE_PER_MINUTE" => Some((1.0, 600.0)),
        "CC_SWITCH_ROUTER_REGISTRATION_KEY_BURST" => Some((1.0, 100.0)),
        "CC_SWITCH_ROUTER_REGISTRATION_BUCKET_IDLE_SECS" => Some((30.0, 86_400.0)),
        "CC_SWITCH_ROUTER_REGISTRATION_MAX_SOURCE_BUCKETS" => Some((128.0, 65_536.0)),
        "CC_SWITCH_ROUTER_REGISTRATION_MAX_KEY_BUCKETS" => Some((256.0, 131_072.0)),
        "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_10M_LIMIT"
        | "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_HOURLY_LIMIT"
        | "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_DAILY_LIMIT"
        | "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_10M_LIMIT"
        | "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_HOURLY_LIMIT"
        | "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_DAILY_LIMIT" => {
            Some((1.0, 1_000_000.0))
        }
        "CC_SWITCH_ROUTER_REGISTRATION_UNOWNED_INSTALLATION_WATERMARK" => {
            Some((1_000.0, 1_000_000.0))
        }
        "CC_SWITCH_ROUTER_CLIENT_OFFLINE_ALERT_SECS" => Some((180.0, 86_400.0)),
        "CC_SWITCH_ROUTER_CLIENT_RECOVERY_STABLE_SECS" => Some((30.0, 3_600.0)),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_COOLDOWN_SECS" => Some((60.0, 604_800.0)),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_BATCH_WINDOW_SECS" => Some((1.0, 600.0)),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_WINDOW_SECS" => Some((60.0, 3_600.0)),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_MIN_CLIENTS" => Some((2.0, 10_000.0)),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_PERCENT" => Some((1.0, 100.0)),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_REMINDER_SECS" => Some((300.0, 86_400.0)),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_RECIPIENT_HOURLY_LIMIT" => Some((1.0, 10_000.0)),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_GLOBAL_HOURLY_LIMIT" => Some((1.0, 100_000.0)),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_RECIPIENT_HOURLY_LIMIT" => Some((1.0, 1_000.0)),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_GLOBAL_HOURLY_LIMIT" => Some((1.0, 10_000.0)),
        "CC_SWITCH_ROUTER_AUTH_CODE_TTL_SECS" => Some((60.0, 3_600.0)),
        "CC_SWITCH_ROUTER_AUTH_CODE_COOLDOWN_SECS" => Some((1.0, 3_600.0)),
        "CC_SWITCH_ROUTER_AUTH_SESSION_TTL_SECS" => Some((60.0, 86_400.0)),
        "CC_SWITCH_ROUTER_AUTH_REFRESH_TTL_SECS" => Some((300.0, 31_536_000.0)),
        "CC_SWITCH_ROUTER_AUTH_MAX_VERIFY_ATTEMPTS" => Some((1.0, 20.0)),
        "CC_SWITCH_ROUTER_AUTH_EMAIL_HOURLY_LIMIT"
        | "CC_SWITCH_ROUTER_AUTH_IP_HOURLY_LIMIT"
        | "CC_SWITCH_ROUTER_AUTH_SOURCE_HOURLY_LIMIT" => Some((1.0, 10_000.0)),
        "CC_SWITCH_ROUTER_FREE_SHARE_IP_PARALLEL_LIMIT" => Some((0.0, 10_000.0)),
        "CC_SWITCH_ROUTER_UX_TELEMETRY_RETENTION_DAYS" => Some((1.0, 365.0)),
        "CC_SWITCH_ROUTER_METRICS_RETENTION_DAYS" => Some((1.0, 3_650.0)),
        "CC_SWITCH_ROUTER_METRICS_SAMPLE_INTERVAL_SECS" => Some((1.0, 300.0)),
        "CC_SWITCH_ROUTER_CLOCK_PROBE_INTERVAL_SECS" => Some((15.0, 3_600.0)),
        "CC_SWITCH_ROUTER_CLOCK_PROBE_TIMEOUT_SECS" => Some((1.0, 15.0)),
        "CC_SWITCH_ROUTER_ALERT_REPEAT_INTERVAL_SECS" => Some((60.0, 604_800.0)),
        "CC_SWITCH_ROUTER_ALERT_HISTORY_RETENTION_DAYS" => Some((1.0, 3_650.0)),
        "CC_SWITCH_ROUTER_ALERT_TELEGRAM_TOPIC_ID" => Some((1.0, i32::MAX as f64)),
        "CC_SWITCH_ROUTER_TELEGRAM_BIND_TOKEN_TTL_SECS" => Some((60.0, 86_400.0)),
        "CC_SWITCH_ROUTER_TELEGRAM_RECIPIENT_HOURLY_LIMIT" => Some((1.0, 1_000.0)),
        "CC_SWITCH_ROUTER_TELEGRAM_GLOBAL_HOURLY_LIMIT" => Some((1.0, 10_000.0)),
        _ => None,
    };
    if key == "CC_SWITCH_ROUTER_MARKET_USD_CNY_RATE" {
        return FieldConstraints {
            min: Some(0.01),
            max: Some(100.0),
            step: Some(0.000001),
            ..FieldConstraints::default()
        };
    }
    if key == "CC_SWITCH_ROUTER_CLOCK_SOURCES" {
        return FieldConstraints {
            min_items: Some(3),
            max_items: Some(5),
            ..FieldConstraints::default()
        };
    }
    range
        .map(|(min, max)| number_constraints(min, max))
        .unwrap_or_default()
}

fn dependencies_for_field(key: &str) -> Vec<FieldDependency> {
    let dependency = match key {
        "CC_SWITCH_ROUTER_TURSO_URL"
        | "CC_SWITCH_ROUTER_TURSO_AUTH_TOKEN"
        | "CC_SWITCH_ROUTER_DB_SYNC_INTERVAL_SECS" => Some(("CC_SWITCH_ROUTER_DB_MODE", "turso")),
        "CC_SWITCH_ROUTER_CLIENT_OFFLINE_ALERT_SECS"
        | "CC_SWITCH_ROUTER_CLIENT_RECOVERY_STABLE_SECS"
        | "CC_SWITCH_ROUTER_CLIENT_ALERT_COOLDOWN_SECS"
        | "CC_SWITCH_ROUTER_CLIENT_ALERT_BATCH_WINDOW_SECS"
        | "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_WINDOW_SECS"
        | "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_MIN_CLIENTS"
        | "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_PERCENT"
        | "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_REMINDER_SECS"
        | "CC_SWITCH_ROUTER_CLIENT_ALERT_RECIPIENT_HOURLY_LIMIT"
        | "CC_SWITCH_ROUTER_CLIENT_ALERT_GLOBAL_HOURLY_LIMIT"
        | "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_RECIPIENT_HOURLY_LIMIT"
        | "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_GLOBAL_HOURLY_LIMIT" => Some((
            "CC_SWITCH_ROUTER_CLIENT_EMAIL_NOTIFICATIONS_ENABLED",
            "true",
        )),
        "CC_SWITCH_ROUTER_ALERT_REPEAT_INTERVAL_SECS"
        | "CC_SWITCH_ROUTER_ALERT_HISTORY_RETENTION_DAYS" => {
            Some(("CC_SWITCH_ROUTER_ALERTING_ENABLED", "true"))
        }
        "CC_SWITCH_ROUTER_ALERT_TELEGRAM_BOT_TOKEN"
        | "CC_SWITCH_ROUTER_ALERT_TELEGRAM_CHAT_ID"
        | "CC_SWITCH_ROUTER_ALERT_TELEGRAM_TOPIC_ID"
        | "CC_SWITCH_ROUTER_ALERT_TELEGRAM_MIN_SEVERITY" => {
            Some(("CC_SWITCH_ROUTER_ALERT_TELEGRAM_ENABLED", "true"))
        }
        "CC_SWITCH_ROUTER_TELEGRAM_BOT_TOKEN"
        | "CC_SWITCH_ROUTER_TELEGRAM_BOT_MODE"
        | "CC_SWITCH_ROUTER_TELEGRAM_BIND_TOKEN_TTL_SECS"
        | "CC_SWITCH_ROUTER_TELEGRAM_RECIPIENT_HOURLY_LIMIT"
        | "CC_SWITCH_ROUTER_TELEGRAM_GLOBAL_HOURLY_LIMIT" => {
            Some(("CC_SWITCH_ROUTER_TELEGRAM_BOT_ENABLED", "true"))
        }
        "CC_SWITCH_ROUTER_TELEGRAM_WEBHOOK_SECRET" => {
            Some(("CC_SWITCH_ROUTER_TELEGRAM_BOT_MODE", "webhook"))
        }
        "CC_SWITCH_ROUTER_METRICS_DB_PATH"
        | "CC_SWITCH_ROUTER_METRICS_RETENTION_DAYS"
        | "CC_SWITCH_ROUTER_METRICS_SAMPLE_INTERVAL_SECS" => {
            Some(("CC_SWITCH_ROUTER_METRICS_ENABLED", "true"))
        }
        "CC_SWITCH_ROUTER_CLOCK_PROBE_INTERVAL_SECS"
        | "CC_SWITCH_ROUTER_CLOCK_PROBE_TIMEOUT_SECS"
        | "CC_SWITCH_ROUTER_CLOCK_SOURCES" => {
            Some(("CC_SWITCH_ROUTER_CLOCK_MONITOR_ENABLED", "true"))
        }
        _ => None,
    };
    dependency
        .map(|(key, equals)| vec![FieldDependency { key, equals }])
        .unwrap_or_default()
}

pub fn field_by_key(key: &str) -> Option<&'static SettingsField> {
    SETTINGS_FIELDS.iter().find(|f| f.key == key)
}

/// Parse an existing `.env` file into key→value, preserving only assignment
/// lines (comments + blank lines are dropped on read). The atomic writer
/// re-emits a clean canonical file.
pub fn read_env_file(path: &Path) -> Result<HashMap<String, String>, AppError> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return Err(AppError::Internal(format!(
                "read env file failed: {err}: {}",
                path.display()
            )));
        }
    };
    let mut out = HashMap::new();
    for (index, line) in content.lines().enumerate() {
        let line = line.trim_end_matches('\r').trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(AppError::Internal(format!(
                "invalid env line {} in {}",
                index + 1,
                path.display()
            )));
        };
        let key = key.trim();
        if key.is_empty() {
            return Err(AppError::Internal(format!(
                "empty env key on line {} in {}",
                index + 1,
                path.display()
            )));
        }
        let value = crate::config::decode_env_value(value).map_err(|message| {
            AppError::Internal(format!(
                "invalid env value on line {} in {}: {message}",
                index + 1,
                path.display()
            ))
        })?;
        out.insert(key.to_string(), value);
    }
    Ok(out)
}

/// Read current values and produce the API response. Secrets surface only
/// `hasValue=true` plus a redacted display so admins can confirm presence
/// without leaking the secret over the wire.
#[cfg(test)]
pub fn values_response(env_path: &Path) -> Result<SettingsValuesResponse, AppError> {
    let file_kv = read_env_file(env_path)?;
    let runtime = SettingsRuntimeSnapshot::for_tests();
    Ok(SettingsValuesResponse {
        values: value_entries(&file_kv, &runtime, None)?,
    })
}

pub fn snapshot_response(
    env_path: &Path,
    runtime: &SettingsRuntimeSnapshot,
    dynamic: &DynamicSettings,
    config: &Config,
) -> Result<SettingsSnapshotResponse, AppError> {
    let file_kv = read_env_file(env_path)?;
    let values = value_entries(&file_kv, runtime, Some((dynamic, config)))?;
    let pending_restart_keys = values
        .iter()
        .filter(|entry| entry.pending_restart)
        .map(|entry| entry.key.clone())
        .collect();
    Ok(SettingsSnapshotResponse {
        revision: settings_revision(&file_kv),
        generated_at: chrono::Utc::now().to_rfc3339(),
        env_path: env_path.display().to_string(),
        schema: schema_response(),
        values,
        pending_restart_keys,
    })
}

pub fn settings_revision(values: &HashMap<String, String>) -> String {
    let ordered = values.iter().collect::<BTreeMap<_, _>>();
    let mut digest = Sha256::new();
    digest.update(b"cc-switch-router-settings-v1\0");
    for (key, value) in ordered {
        digest.update(key.as_bytes());
        digest.update(b"\0");
        digest.update(value.as_bytes());
        digest.update(b"\0");
    }
    hex::encode(digest.finalize())
}

fn value_entries(
    file_kv: &HashMap<String, String>,
    runtime: &SettingsRuntimeSnapshot,
    dynamic_state: Option<(&DynamicSettings, &Config)>,
) -> Result<Vec<SettingValueEntry>, AppError> {
    SETTINGS_FIELDS
        .iter()
        .map(|field| {
            let (source, configured) = configured_value(field, file_kv);
            let live_dynamic = if field.dynamic_group.is_some() {
                dynamic_state
                    .map(|(dynamic, config)| dynamic_effective_value(field, dynamic, config))
                    .transpose()?
            } else {
                None
            };
            let (effective_source, effective) = if let Some(effective) = live_dynamic {
                (
                    dynamic_effective_source(
                        field,
                        source,
                        configured.as_deref(),
                        effective.as_deref(),
                        runtime,
                    ),
                    effective,
                )
            } else if let Some(value) = runtime
                .startup_effective
                .get(field.key)
                .filter(|value| !value.trim().is_empty())
            {
                let source = if runtime.startup_file_keys.contains(field.key) {
                    ValueSource::EnvFile
                } else {
                    ValueSource::Default
                };
                (source, Some(value.clone()))
            } else {
                let default = resolved_default_value(field);
                let source = if default.is_some() {
                    ValueSource::Default
                } else {
                    ValueSource::Unset
                };
                (source, default)
            };
            let has_value = configured.as_deref().is_some_and(|value| !value.is_empty());
            let effective_has_value = effective.as_deref().is_some_and(|value| !value.is_empty());
            let is_secret = matches!(field.field_type, FieldType::Secret);
            let values_differ = normalized_field_comparison(field, configured.as_deref())
                != normalized_field_comparison(field, effective.as_deref());
            let pending_restart =
                values_differ && (field.restart_required || field.dynamic_group.is_some());
            Ok(SettingValueEntry {
                key: field.key.to_string(),
                value: (!is_secret).then_some(configured).flatten(),
                has_value,
                is_secret,
                source,
                effective_value: (!is_secret).then_some(effective).flatten(),
                effective_has_value,
                effective_source,
                pending_restart,
            })
        })
        .collect()
}

fn dynamic_effective_value(
    field: &SettingsField,
    dynamic: &DynamicSettings,
    config: &Config,
) -> Result<Option<String>, AppError> {
    let value = match field.key {
        "CC_SWITCH_ROUTER_SERVER_LOG_PUBLIC_ENABLED" => {
            Some(dynamic.server_log_public_enabled.to_string())
        }
        "CC_SWITCH_ROUTER_CLIENT_EMAIL_NOTIFICATIONS_ENABLED" => {
            Some(dynamic.client_notifications.enabled.to_string())
        }
        "CC_SWITCH_ROUTER_CLIENT_OFFLINE_ALERT_SECS" => {
            Some(dynamic.client_notifications.offline_alert_secs.to_string())
        }
        "CC_SWITCH_ROUTER_CLIENT_RECOVERY_STABLE_SECS" => Some(
            dynamic
                .client_notifications
                .recovery_stable_secs
                .to_string(),
        ),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_COOLDOWN_SECS" => {
            Some(dynamic.client_notifications.cooldown_secs.to_string())
        }
        "CC_SWITCH_ROUTER_CLIENT_ALERT_BATCH_WINDOW_SECS" => {
            Some(dynamic.client_notifications.batch_window_secs.to_string())
        }
        "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_WINDOW_SECS" => {
            Some(dynamic.client_notifications.storm_window_secs.to_string())
        }
        "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_MIN_CLIENTS" => {
            Some(dynamic.client_notifications.storm_min_clients.to_string())
        }
        "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_PERCENT" => {
            Some(dynamic.client_notifications.storm_percent.to_string())
        }
        "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_REMINDER_SECS" => {
            Some(dynamic.client_notifications.storm_reminder_secs.to_string())
        }
        "CC_SWITCH_ROUTER_CLIENT_ALERT_RECIPIENT_HOURLY_LIMIT" => Some(
            dynamic
                .client_notifications
                .recipient_hourly_limit
                .to_string(),
        ),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_GLOBAL_HOURLY_LIMIT" => {
            Some(dynamic.client_notifications.global_hourly_limit.to_string())
        }
        "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_RECIPIENT_HOURLY_LIMIT" => Some(
            dynamic
                .client_notifications
                .registration_recipient_hourly_limit
                .to_string(),
        ),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_GLOBAL_HOURLY_LIMIT" => Some(
            dynamic
                .client_notifications
                .registration_global_hourly_limit
                .to_string(),
        ),
        "CC_SWITCH_ROUTER_IP_BLACKLIST" => non_empty_value(
            dynamic
                .security
                .ip_blacklist
                .iter()
                .map(|block| block.canonical())
                .collect::<Vec<_>>()
                .join(","),
        ),
        "CC_SWITCH_ROUTER_MARKET_USD_CNY_RATE" => Some(crate::market_billing::format_usd_cny_rate(
            dynamic.market_usd_cny_rate_micros,
        )),
        "CC_SWITCH_ROUTER_ADMIN_EMAILS" => {
            let default_admin = config.default_admin_email();
            let mut emails = dynamic
                .admin_emails
                .iter()
                .filter(|email| default_admin.as_deref() != Some(email.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            emails.sort_unstable();
            non_empty_value(emails.join(","))
        }
        "CC_SWITCH_ROUTER_FOOTER_TELEGRAM_URL" => {
            non_empty_value(dynamic.footer_telegram_url.clone())
        }
        "CC_SWITCH_ROUTER_ALERTING_ENABLED" => Some(dynamic.alerting.enabled.to_string()),
        "CC_SWITCH_ROUTER_ALERT_REPEAT_INTERVAL_SECS" => {
            Some(dynamic.alerting.repeat_interval_secs.to_string())
        }
        "CC_SWITCH_ROUTER_ALERT_HISTORY_RETENTION_DAYS" => {
            Some(dynamic.alerting.history_retention_days.to_string())
        }
        "CC_SWITCH_ROUTER_ALERT_TELEGRAM_ENABLED" => {
            Some(dynamic.alerting.telegram_enabled.to_string())
        }
        "CC_SWITCH_ROUTER_ALERT_TELEGRAM_BOT_TOKEN" => dynamic.alerting.telegram_bot_token.clone(),
        "CC_SWITCH_ROUTER_ALERT_TELEGRAM_CHAT_ID" => dynamic.alerting.telegram_chat_id.clone(),
        "CC_SWITCH_ROUTER_ALERT_TELEGRAM_TOPIC_ID" => dynamic
            .alerting
            .telegram_topic_id
            .map(|value| value.to_string()),
        "CC_SWITCH_ROUTER_ALERT_TELEGRAM_MIN_SEVERITY" => {
            non_empty_value(dynamic.alerting.telegram_min_severity.clone())
        }
        "CC_SWITCH_ROUTER_TELEGRAM_BOT_ENABLED" => Some(dynamic.telegram_bot.enabled.to_string()),
        "CC_SWITCH_ROUTER_TELEGRAM_BOT_TOKEN" => dynamic.telegram_bot.bot_token.clone(),
        "CC_SWITCH_ROUTER_TELEGRAM_BOT_MODE" => {
            Some(dynamic.telegram_bot.mode.as_str().to_string())
        }
        "CC_SWITCH_ROUTER_TELEGRAM_WEBHOOK_SECRET" => dynamic.telegram_bot.webhook_secret.clone(),
        "CC_SWITCH_ROUTER_TELEGRAM_BIND_TOKEN_TTL_SECS" => {
            Some(dynamic.telegram_bot.bind_token_ttl_secs.to_string())
        }
        "CC_SWITCH_ROUTER_TELEGRAM_RECIPIENT_HOURLY_LIMIT" => {
            Some(dynamic.telegram_bot.recipient_hourly_limit.to_string())
        }
        "CC_SWITCH_ROUTER_TELEGRAM_GLOBAL_HOURLY_LIMIT" => {
            Some(dynamic.telegram_bot.global_hourly_limit.to_string())
        }
        _ => {
            return Err(AppError::Internal(format!(
                "missing live value mapping for dynamic setting: {}",
                field.key
            )));
        }
    };
    Ok(value)
}

fn non_empty_value(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn dynamic_effective_source(
    field: &SettingsField,
    configured_source: ValueSource,
    configured: Option<&str>,
    effective: Option<&str>,
    runtime: &SettingsRuntimeSnapshot,
) -> ValueSource {
    if normalized_field_comparison(field, configured)
        == normalized_field_comparison(field, effective)
    {
        return configured_source;
    }
    if runtime
        .startup_effective
        .get(field.key)
        .is_some_and(|startup| {
            normalized_field_comparison(field, Some(startup.as_str()))
                == normalized_field_comparison(field, effective)
        })
    {
        return if runtime.startup_file_keys.contains(field.key) {
            ValueSource::EnvFile
        } else {
            ValueSource::Runtime
        };
    }
    if normalized_field_comparison(field, resolved_default_value(field).as_deref())
        == normalized_field_comparison(field, effective)
    {
        return if effective.is_some() {
            ValueSource::Default
        } else {
            ValueSource::Unset
        };
    }
    if effective.is_some() {
        ValueSource::Runtime
    } else {
        ValueSource::Unset
    }
}

fn configured_value(
    field: &SettingsField,
    file_kv: &HashMap<String, String>,
) -> (ValueSource, Option<String>) {
    if let Some(value) = file_kv
        .get(field.key)
        .filter(|value| !value.trim().is_empty())
    {
        return (ValueSource::EnvFile, Some(value.clone()));
    }
    match resolved_default_value(field) {
        Some(value) => (ValueSource::Default, Some(value)),
        None => (ValueSource::Unset, None),
    }
}

fn resolved_default_value(field: &SettingsField) -> Option<String> {
    match field.key {
        "CC_SWITCH_ROUTER_DATA_DIR" => {
            Some(crate::config::default_data_dir().display().to_string())
        }
        "CC_SWITCH_ROUTER_DB_PATH" => Some(crate::config::default_db_path().display().to_string()),
        "CC_SWITCH_ROUTER_HOST_KEY_PATH" => {
            Some(crate::config::default_host_key_path().display().to_string())
        }
        "CC_SWITCH_ROUTER_PROVISION_SSH_PRIVATE_KEY_PATH" => Some(
            crate::config::default_provision_ssh_private_key_path()
                .display()
                .to_string(),
        ),
        "CC_SWITCH_ROUTER_PROVISION_SSH_PUBLIC_KEY_PATH" => Some(
            crate::config::default_provision_ssh_public_key_path()
                .display()
                .to_string(),
        ),
        "CC_SWITCH_ROUTER_METRICS_DB_PATH" => Some(
            crate::config::default_metrics_db_path()
                .display()
                .to_string(),
        ),
        "CC_SWITCH_ROUTER_SERVER_LOG_DATA_DIR" => Some(
            crate::config::default_data_dir()
                .join("server-logs")
                .display()
                .to_string(),
        ),
        _ => field.default.map(str::to_string),
    }
}

fn normalized_comparison(value: Option<&str>) -> Option<String> {
    value.map(str::trim).map(str::to_string)
}

fn normalized_field_comparison(field: &SettingsField, value: Option<&str>) -> Option<String> {
    let value = value?;
    normalize_value(field, value)
        .ok()
        .flatten()
        .or_else(|| normalized_comparison(Some(value)))
}

pub fn validation_response(
    existing: &HashMap<String, String>,
    updates: &BTreeMap<String, Option<String>>,
) -> SettingsValidationResponse {
    match validate_and_diff(existing, updates) {
        Ok(outcome) => SettingsValidationResponse {
            valid: true,
            field_errors: BTreeMap::new(),
            form_errors: Vec::new(),
            restart_required_keys: outcome.restart_required_keys,
        },
        Err(error) => {
            let message = error.to_string();
            let field = SETTINGS_FIELDS
                .iter()
                .find(|field| message.contains(field.key))
                .map(|field| field.key.to_string())
                .or_else(|| {
                    (updates.len() == 1)
                        .then(|| updates.keys().next().cloned())
                        .flatten()
                });
            let mut field_errors = BTreeMap::new();
            let mut form_errors = Vec::new();
            if let Some(field) = field {
                field_errors.insert(field, vec![message]);
            } else {
                form_errors.push(message);
            }
            SettingsValidationResponse {
                valid: false,
                field_errors,
                form_errors,
                restart_required_keys: Vec::new(),
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ApplyOutcome {
    pub updated_keys: Vec<String>,
    pub unchanged_keys: Vec<String>,
    pub restart_required_keys: Vec<String>,
    pub dynamic_groups: Vec<DynamicGroup>,
    pub new_env_kv: BTreeMap<String, String>,
}

/// Validate updates against the schema and compute the new in-memory env
/// state. Does not touch disk — the caller writes the file under the same
/// lock that protects DynamicSettings.
pub fn validate_and_diff(
    existing: &HashMap<String, String>,
    updates: &BTreeMap<String, Option<String>>,
) -> Result<ApplyOutcome, AppError> {
    let mut updated = Vec::new();
    let mut unchanged = Vec::new();
    let mut restart_keys = Vec::new();
    let mut groups: Vec<DynamicGroup> = Vec::new();
    let mut next: BTreeMap<String, String> = existing
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    for (key, raw_value) in updates {
        let field = field_by_key(key)
            .ok_or_else(|| AppError::BadRequest(format!("unknown setting: {key}")))?;
        let next_value = match raw_value {
            Some(v) => normalize_value(field, v)?,
            None => None,
        };
        let prev = existing.get(key).cloned();
        let prev_normalized = prev
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let next_normalized = next_value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);

        if field.required && next_normalized.as_deref().unwrap_or("").is_empty() {
            return Err(AppError::BadRequest(format!(
                "{} is required and cannot be cleared",
                field.key
            )));
        }
        if prev_normalized == next_normalized
            && (next_normalized.is_some() || existing.contains_key(key))
        {
            unchanged.push(key.clone());
            continue;
        }
        match &next_normalized {
            Some(v) if !v.is_empty() => {
                next.insert(key.clone(), v.clone());
            }
            _ => {
                next.insert(key.clone(), String::new());
            }
        }
        updated.push(key.clone());
        if field.restart_required {
            restart_keys.push(key.clone());
        }
        if let Some(group) = field.dynamic_group {
            if !groups
                .iter()
                .any(|g| std::mem::discriminant(g) == std::mem::discriminant(&group))
            {
                groups.push(group);
            }
        }
    }
    let effective_next = next
        .iter()
        .filter(|(_, value)| !value.trim().is_empty())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    validate_registration_admission_relations(&effective_next, updates)?;
    validate_client_notification_relations(&effective_next, updates)?;
    validate_alerting_relations(&effective_next, updates)?;
    validate_telegram_bot_relations(&effective_next, updates)?;
    validate_database_relations(&effective_next, updates)?;
    validate_ssh_transport_relations(&effective_next, updates)?;
    validate_proxy_stream_relations(&effective_next, updates)?;
    validate_clock_relations(&effective_next, updates)?;
    validate_lifecycle_relations(&effective_next, updates)?;
    validate_auth_relations(&effective_next, updates)?;

    Ok(ApplyOutcome {
        updated_keys: updated,
        unchanged_keys: unchanged,
        restart_required_keys: restart_keys,
        dynamic_groups: groups,
        new_env_kv: next,
    })
}

/// Write the env file atomically: stage to `<path>.new`, fsync, rename over
/// the live file, and keep a `<path>.bak` of the previous version.
pub fn write_env_file_atomic(path: &Path, kv: &BTreeMap<String, String>) -> Result<(), AppError> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Internal(format!("env path has no parent: {}", path.display())))?;
    fs::create_dir_all(parent).map_err(|e| {
        AppError::Internal(format!(
            "create env parent failed: {e}: {}",
            parent.display()
        ))
    })?;

    let mut body = String::new();
    body.push_str(
        "# cc-switch-router env file. Managed by the admin UI; manual edits remain valid.\n\n",
    );
    let mut emitted = HashSet::new();
    for field in SETTINGS_FIELDS {
        if let Some(value) = kv.get(field.key) {
            body.push_str(field.key);
            body.push('=');
            body.push_str(&escape_env_value(value));
            body.push('\n');
            emitted.insert(field.key.to_string());
        }
    }
    // Preserve any keys the user has set outside the known schema, so an
    // admin who hand-edited the file doesn't lose context.
    for (key, value) in kv {
        if !emitted.contains(key) {
            body.push_str(key);
            body.push('=');
            body.push_str(&escape_env_value(value));
            body.push('\n');
        }
    }

    let tmp = path.with_extension("new");
    let mut tmp_file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp)
        .map_err(|e| AppError::Internal(format!("open env tmp failed: {e}: {}", tmp.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tmp_file
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|e| {
                AppError::Internal(format!(
                    "set env tmp permissions failed: {e}: {}",
                    tmp.display()
                ))
            })?;
    }
    tmp_file
        .write_all(body.as_bytes())
        .and_then(|_| tmp_file.sync_all())
        .map_err(|e| AppError::Internal(format!("sync env tmp failed: {e}: {}", tmp.display())))?;
    drop(tmp_file);
    if path.exists() {
        let bak = path.with_extension("bak");
        let _ = fs::remove_file(&bak);
        fs::copy(path, &bak).map_err(|err| {
            AppError::Internal(format!("write env backup failed: {err}: {}", bak.display()))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&bak, fs::Permissions::from_mode(0o600)).map_err(|err| {
                AppError::Internal(format!(
                    "set env backup permissions failed: {err}: {}",
                    bak.display()
                ))
            })?;
        }
        fs::File::open(&bak)
            .and_then(|backup| backup.sync_all())
            .map_err(|err| {
                AppError::Internal(format!("sync env backup failed: {err}: {}", bak.display()))
            })?;
    }
    fs::rename(&tmp, path).map_err(|e| {
        AppError::Internal(format!(
            "promote env file failed: {e}: {} -> {}",
            tmp.display(),
            path.display()
        ))
    })?;
    #[cfg(unix)]
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|e| {
            AppError::Internal(format!(
                "sync env directory failed: {e}: {}",
                parent.display()
            ))
        })?;
    Ok(())
}

fn escape_env_value(value: &str) -> String {
    let needs_quotes = value.is_empty()
        || value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '#' | '"' | '\'' | '\\'));
    if needs_quotes {
        let escaped = value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t");
        format!("\"{escaped}\"")
    } else {
        value.to_string()
    }
}

fn normalize_value(field: &SettingsField, raw: &str) -> Result<Option<String>, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if raw.contains('\0')
        || (!matches!(field.field_type, FieldType::UrlList)
            && raw
                .chars()
                .any(|character| matches!(character, '\n' | '\r')))
    {
        return Err(AppError::BadRequest(format!(
            "{} must be a single-line value without NUL characters",
            field.key
        )));
    }
    match field.field_type {
        FieldType::Int => {
            let value = trimmed.parse::<i64>().map_err(|_| {
                AppError::BadRequest(format!("{} must be an integer, got: {raw}", field.key))
            })?;
            let constraints = constraints_for_field(field.key);
            if constraints
                .min
                .is_some_and(|minimum| (value as f64) < minimum)
                || constraints
                    .max
                    .is_some_and(|maximum| (value as f64) > maximum)
            {
                return Err(AppError::BadRequest(format!(
                    "{} must be between {} and {}, got: {value}",
                    field.key,
                    constraints.min.unwrap_or(f64::MIN),
                    constraints.max.unwrap_or(f64::MAX),
                )));
            }
            if field.key == "CC_SWITCH_ROUTER_REQUEST_LOG_RETENTION_DAYS"
                && !(i64::from(MIN_REQUEST_LOG_RETENTION_DAYS)
                    ..=i64::from(MAX_REQUEST_LOG_RETENTION_DAYS))
                    .contains(&value)
            {
                return Err(AppError::BadRequest(format!(
                    "{} must be between {} and {}, got: {value}",
                    field.key, MIN_REQUEST_LOG_RETENTION_DAYS, MAX_REQUEST_LOG_RETENTION_DAYS
                )));
            }
            validate_client_notification_integer(field.key, value)?;
            validate_registration_admission_integer(field.key, value)?;
            validate_alerting_integer(field.key, value)?;
            if field.key == "CC_SWITCH_ROUTER_DB_SYNC_INTERVAL_SECS"
                && !(1..=3_600).contains(&value)
            {
                return Err(AppError::BadRequest(format!(
                    "{} must be between 1 and 3600, got: {value}",
                    field.key
                )));
            }
            Ok(Some(trimmed.to_string()))
        }
        FieldType::Select => match field.key {
            "CC_SWITCH_ROUTER_DB_MODE" => match trimmed.to_ascii_lowercase().as_str() {
                "local" | "turso" => Ok(Some(trimmed.to_ascii_lowercase())),
                _ => Err(AppError::BadRequest(format!(
                    "{} must be local or turso, got: {raw}",
                    field.key
                ))),
            },
            "CC_SWITCH_ROUTER_ALERT_TELEGRAM_MIN_SEVERITY" => {
                match trimmed.to_ascii_lowercase().as_str() {
                    "info" | "warning" | "critical" => Ok(Some(trimmed.to_ascii_lowercase())),
                    _ => Err(AppError::BadRequest(format!(
                        "{} must be info, warning, or critical, got: {raw}",
                        field.key
                    ))),
                }
            }
            "CC_SWITCH_ROUTER_TELEGRAM_BOT_MODE" => {
                match crate::config::TelegramBotMode::parse(trimmed) {
                    Some(mode) => Ok(Some(mode.as_str().to_string())),
                    None => Err(AppError::BadRequest(format!(
                        "{} must be polling or webhook, got: {raw}",
                        field.key
                    ))),
                }
            }
            _ => Err(AppError::Internal(format!(
                "unsupported select settings field: {}",
                field.key
            ))),
        },
        FieldType::Decimal => {
            if field.key != "CC_SWITCH_ROUTER_MARKET_USD_CNY_RATE" {
                return Err(AppError::Internal(format!(
                    "unsupported decimal settings field: {}",
                    field.key
                )));
            }
            let rate_micros = crate::market_billing::parse_usd_cny_rate_micros(trimmed)
                .map_err(|message| AppError::BadRequest(format!("{}: {message}", field.key)))?;
            Ok(Some(crate::market_billing::format_usd_cny_rate(
                rate_micros,
            )))
        }
        FieldType::Bool => match trimmed.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(Some("true".to_string())),
            "0" | "false" | "no" | "off" => Ok(Some("false".to_string())),
            _ => Err(AppError::BadRequest(format!(
                "{} must be true/false, got: {raw}",
                field.key
            ))),
        },
        FieldType::Email => {
            if !crate::notifications::is_basic_email(trimmed) {
                return Err(AppError::BadRequest(format!(
                    "{} must be a valid email address, got: {raw}",
                    field.key
                )));
            }
            Ok(Some(trimmed.to_ascii_lowercase()))
        }
        FieldType::EmailList => {
            let mut cleaned = Vec::new();
            for piece in trimmed.split(',') {
                let part = piece.trim();
                if part.is_empty() {
                    continue;
                }
                if !crate::notifications::is_basic_email(part) {
                    return Err(AppError::BadRequest(format!(
                        "{} contains an invalid email address: {part}",
                        field.key
                    )));
                }
                cleaned.push(part.to_ascii_lowercase());
            }
            cleaned.sort_unstable();
            cleaned.dedup();
            if cleaned.is_empty() {
                Ok(None)
            } else {
                Ok(Some(cleaned.join(",")))
            }
        }
        FieldType::IpList => crate::dynamic_settings::normalize_ip_blacklist(trimmed)
            .map(Some)
            .ok_or_else(|| {
                AppError::BadRequest(format!(
                    "{} must contain only IP or CIDR entries, got: {raw}",
                    field.key
                ))
            }),
        FieldType::UrlList => normalize_url_list(field.key, trimmed).map(Some),
        FieldType::Url => {
            let parsed = url::Url::parse(trimmed).map_err(|_| {
                AppError::BadRequest(format!("{} must be a valid URL, got: {raw}", field.key))
            })?;
            if !matches!(parsed.scheme(), "http" | "https")
                || parsed.host_str().is_none()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
            {
                return Err(AppError::BadRequest(format!(
                    "{} must be an HTTP(S) URL without credentials, got: {raw}",
                    field.key
                )));
            }
            Ok(Some(trimmed.to_string()))
        }
        FieldType::Text if field.key == "CC_SWITCH_ROUTER_TURSO_URL" => {
            crate::config::validate_turso_url(trimmed).map_err(AppError::BadRequest)?;
            Ok(Some(trimmed.to_string()))
        }
        FieldType::Text
            if matches!(
                field.key,
                "CC_SWITCH_ROUTER_API_ADDR" | "CC_SWITCH_ROUTER_SSH_ADDR"
            ) =>
        {
            trimmed.parse::<std::net::SocketAddr>().map_err(|_| {
                AppError::BadRequest(format!(
                    "{} must be a numeric IP address and port, got: {raw}",
                    field.key
                ))
            })?;
            Ok(Some(trimmed.to_string()))
        }
        FieldType::Text if field.key == "CC_SWITCH_ROUTER_TUNNEL_DOMAIN" => {
            if trimmed.contains("://")
                || trimmed.contains('/')
                || crate::config::tunnel_domain_host(trimmed).is_none()
            {
                return Err(AppError::BadRequest(format!(
                    "{} must be a host with an optional port, got: {raw}",
                    field.key
                )));
            }
            Ok(Some(trimmed.to_ascii_lowercase()))
        }
        FieldType::Text if field.key == "CC_SWITCH_ROUTER_SSH_PUBLIC_ADDR" => {
            let parsed = url::Url::parse(&format!("ssh://{trimmed}")).map_err(|_| {
                AppError::BadRequest(format!(
                    "{} must be a reachable host and port, got: {raw}",
                    field.key
                ))
            })?;
            if parsed.host_str().is_none() || parsed.port().is_none() {
                return Err(AppError::BadRequest(format!(
                    "{} must include a host and port, got: {raw}",
                    field.key
                )));
            }
            Ok(Some(trimmed.to_ascii_lowercase()))
        }
        FieldType::Secret if field.key == "CC_SWITCH_ROUTER_RESEND_API_KEY" => {
            if !trimmed.starts_with("re_") {
                return Err(AppError::BadRequest(format!(
                    "{} must start with re_",
                    field.key
                )));
            }
            Ok(Some(trimmed.to_string()))
        }
        FieldType::Path | FieldType::Text | FieldType::Secret => Ok(Some(trimmed.to_string())),
    }
}

fn normalize_url_list(key: &str, raw: &str) -> Result<String, AppError> {
    let mut values = Vec::new();
    let mut hosts = HashSet::new();
    for piece in raw.split([',', '\n', '\r']) {
        let piece = piece.trim().trim_end_matches('/');
        if piece.is_empty() {
            continue;
        }
        let normalized = if key == "CC_SWITCH_ROUTER_IP_INTEL_ENDPOINTS"
            && !piece.starts_with("http://")
            && !piece.starts_with("https://")
        {
            format!("https://{piece}")
        } else {
            piece.to_string()
        };
        let parsed = url::Url::parse(&normalized)
            .map_err(|_| AppError::BadRequest(format!("{key} contains an invalid URL: {piece}")))?;
        let required_scheme = if key == "CC_SWITCH_ROUTER_CLOCK_SOURCES" {
            "https"
        } else {
            parsed.scheme()
        };
        if parsed.scheme() != required_scheme
            || !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(AppError::BadRequest(format!(
                "{key} contains an unsupported URL: {piece}"
            )));
        }
        if key == "CC_SWITCH_ROUTER_CLOCK_SOURCES"
            && (parsed.query().is_some() || parsed.fragment().is_some())
        {
            return Err(AppError::BadRequest(format!(
                "{key} sources must not contain a query or fragment: {piece}"
            )));
        }
        if key == "CC_SWITCH_ROUTER_CLOCK_SOURCES" {
            let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
            if !hosts.insert(host) {
                return Err(AppError::BadRequest(format!(
                    "{key} must use distinct hosts"
                )));
            }
        }
        values.push(normalized);
    }
    let constraints = constraints_for_field(key);
    if constraints
        .min_items
        .is_some_and(|minimum| values.len() < minimum)
        || constraints
            .max_items
            .is_some_and(|maximum| values.len() > maximum)
    {
        return Err(AppError::BadRequest(format!(
            "{key} must contain between {} and {} URLs",
            constraints.min_items.unwrap_or(1),
            constraints.max_items.unwrap_or(usize::MAX),
        )));
    }
    if values.is_empty() {
        return Err(AppError::BadRequest(format!(
            "{key} must contain at least one URL"
        )));
    }
    Ok(values.join(","))
}

fn validate_database_relations(
    next: &BTreeMap<String, String>,
    updates: &BTreeMap<String, Option<String>>,
) -> Result<(), AppError> {
    const DATABASE_KEYS: &[&str] = &[
        "CC_SWITCH_ROUTER_DB_MODE",
        "CC_SWITCH_ROUTER_DB_PATH",
        "CC_SWITCH_ROUTER_TURSO_URL",
        "CC_SWITCH_ROUTER_TURSO_AUTH_TOKEN",
        "CC_SWITCH_ROUTER_DB_SYNC_INTERVAL_SECS",
        "CC_SWITCH_ROUTER_METRICS_DB_PATH",
    ];
    if !updates
        .keys()
        .any(|key| DATABASE_KEYS.contains(&key.as_str()))
    {
        return Ok(());
    }

    let configured = |key: &str| {
        next.get(key)
            .cloned()
            .filter(|value| !value.trim().is_empty())
    };
    let mode = configured("CC_SWITCH_ROUTER_DB_MODE").unwrap_or_else(|| "local".into());
    if mode.eq_ignore_ascii_case("turso") {
        let url = configured("CC_SWITCH_ROUTER_TURSO_URL").ok_or_else(|| {
            AppError::BadRequest(
                "CC_SWITCH_ROUTER_TURSO_URL is required when database mode is turso".into(),
            )
        })?;
        crate::config::validate_turso_url(&url).map_err(AppError::BadRequest)?;
        if configured("CC_SWITCH_ROUTER_TURSO_AUTH_TOKEN").is_none() {
            return Err(AppError::BadRequest(
                "CC_SWITCH_ROUTER_TURSO_AUTH_TOKEN is required when database mode is turso".into(),
            ));
        }
    }
    let business_path = configured("CC_SWITCH_ROUTER_DB_PATH")
        .unwrap_or_else(|| crate::config::default_db_path().display().to_string());
    let metrics_path = configured("CC_SWITCH_ROUTER_METRICS_DB_PATH").unwrap_or_else(|| {
        crate::config::default_metrics_db_path()
            .display()
            .to_string()
    });
    if Path::new(&business_path) == Path::new(&metrics_path) {
        return Err(AppError::BadRequest(
            "CC_SWITCH_ROUTER_METRICS_DB_PATH must differ from CC_SWITCH_ROUTER_DB_PATH".into(),
        ));
    }
    Ok(())
}

fn validate_ssh_transport_relations(
    next: &BTreeMap<String, String>,
    updates: &BTreeMap<String, Option<String>>,
) -> Result<(), AppError> {
    const SSH_TRANSPORT_KEYS: &[&str] = &[
        "CC_SWITCH_ROUTER_SSH_INACTIVITY_TIMEOUT_SECS",
        "CC_SWITCH_ROUTER_SSH_KEEPALIVE_INTERVAL_SECS",
        "CC_SWITCH_ROUTER_SSH_KEEPALIVE_MAX",
        "CC_SWITCH_ROUTER_SSH_CHANNEL_OPEN_TIMEOUT_SECS",
        "CC_SWITCH_ROUTER_SSH_BRIDGE_WRITE_STALL_TIMEOUT_SECS",
        "CC_SWITCH_ROUTER_SSH_BRIDGE_HALF_CLOSE_IDLE_TIMEOUT_SECS",
        "CC_SWITCH_ROUTER_SSH_MAX_FORWARD_CONNECTIONS",
        "CC_SWITCH_ROUTER_SSH_MAX_FORWARD_CONNECTIONS_PER_TUNNEL",
    ];
    if !updates
        .keys()
        .any(|key| SSH_TRANSPORT_KEYS.contains(&key.as_str()))
    {
        return Ok(());
    }

    let defaults = SshTransportConfig::default();
    let config = SshTransportConfig {
        inactivity_timeout_secs: resolved_ssh_u64(
            next,
            updates,
            "CC_SWITCH_ROUTER_SSH_INACTIVITY_TIMEOUT_SECS",
            defaults.inactivity_timeout_secs,
        )?,
        keepalive_interval_secs: resolved_ssh_u64(
            next,
            updates,
            "CC_SWITCH_ROUTER_SSH_KEEPALIVE_INTERVAL_SECS",
            defaults.keepalive_interval_secs,
        )?,
        keepalive_max: resolved_ssh_usize(
            next,
            updates,
            "CC_SWITCH_ROUTER_SSH_KEEPALIVE_MAX",
            defaults.keepalive_max,
        )?,
        channel_open_timeout_secs: resolved_ssh_u64(
            next,
            updates,
            "CC_SWITCH_ROUTER_SSH_CHANNEL_OPEN_TIMEOUT_SECS",
            defaults.channel_open_timeout_secs,
        )?,
        bridge_write_stall_timeout_secs: resolved_ssh_u64(
            next,
            updates,
            "CC_SWITCH_ROUTER_SSH_BRIDGE_WRITE_STALL_TIMEOUT_SECS",
            defaults.bridge_write_stall_timeout_secs,
        )?,
        bridge_half_close_idle_timeout_secs: resolved_ssh_u64(
            next,
            updates,
            "CC_SWITCH_ROUTER_SSH_BRIDGE_HALF_CLOSE_IDLE_TIMEOUT_SECS",
            defaults.bridge_half_close_idle_timeout_secs,
        )?,
        max_forward_connections: resolved_ssh_usize(
            next,
            updates,
            "CC_SWITCH_ROUTER_SSH_MAX_FORWARD_CONNECTIONS",
            defaults.max_forward_connections,
        )?,
        max_forward_connections_per_tunnel: resolved_ssh_usize(
            next,
            updates,
            "CC_SWITCH_ROUTER_SSH_MAX_FORWARD_CONNECTIONS_PER_TUNNEL",
            defaults.max_forward_connections_per_tunnel,
        )?,
    };
    config.validate().map_err(AppError::BadRequest)
}

fn validate_proxy_stream_relations(
    next: &BTreeMap<String, String>,
    updates: &BTreeMap<String, Option<String>>,
) -> Result<(), AppError> {
    let keys = [
        "CC_SWITCH_ROUTER_PROXY_REQUEST_BODY_TIMEOUT_SECS",
        "CC_SWITCH_ROUTER_PROXY_RESPONSE_HEADER_TIMEOUT_SECS",
        "CC_SWITCH_ROUTER_PROXY_STREAM_FIRST_EVENT_TIMEOUT_SECS",
        "CC_SWITCH_ROUTER_PROXY_STREAM_IDLE_TIMEOUT_SECS",
        "CC_SWITCH_ROUTER_PROXY_DOWNSTREAM_STALL_TIMEOUT_SECS",
        "CC_SWITCH_ROUTER_PROXY_MAX_REQUEST_LIFETIME_SECS",
        "CC_SWITCH_ROUTER_PROXY_REQUEST_BODY_LIMIT_MB",
        "CC_SWITCH_ROUTER_PROXY_MEDIA_REQUEST_BODY_LIMIT_MB",
        "CC_SWITCH_ROUTER_PROXY_IMAGE_REQUEST_BODY_LIMIT_MB",
    ];
    if !keys.iter().any(|key| updates.contains_key(*key)) {
        return Ok(());
    }
    let config = crate::config::ProxyStreamConfig {
        request_body_timeout_secs: resolved_ssh_u64(
            next,
            updates,
            keys[0],
            crate::config::DEFAULT_PROXY_REQUEST_BODY_TIMEOUT_SECS,
        )?,
        response_header_timeout_secs: resolved_ssh_u64(
            next,
            updates,
            keys[1],
            crate::config::DEFAULT_PROXY_RESPONSE_HEADER_TIMEOUT_SECS,
        )?,
        first_event_timeout_secs: resolved_ssh_u64(
            next,
            updates,
            keys[2],
            crate::config::DEFAULT_PROXY_STREAM_FIRST_EVENT_TIMEOUT_SECS,
        )?,
        idle_timeout_secs: resolved_ssh_u64(
            next,
            updates,
            keys[3],
            crate::config::DEFAULT_PROXY_STREAM_IDLE_TIMEOUT_SECS,
        )?,
        downstream_stall_timeout_secs: resolved_ssh_u64(
            next,
            updates,
            keys[4],
            crate::config::DEFAULT_PROXY_DOWNSTREAM_STALL_TIMEOUT_SECS,
        )?,
        max_request_lifetime_secs: resolved_ssh_u64(
            next,
            updates,
            keys[5],
            crate::config::DEFAULT_PROXY_MAX_REQUEST_LIFETIME_SECS,
        )?,
        request_body_limit_mb: resolved_ssh_u64(
            next,
            updates,
            keys[6],
            crate::config::DEFAULT_PROXY_REQUEST_BODY_LIMIT_MB,
        )?,
        media_request_body_limit_mb: resolved_ssh_u64(
            next,
            updates,
            keys[7],
            crate::config::DEFAULT_PROXY_MEDIA_REQUEST_BODY_LIMIT_MB,
        )?,
        image_request_body_limit_mb: resolved_ssh_u64(
            next,
            updates,
            keys[8],
            crate::config::DEFAULT_PROXY_IMAGE_REQUEST_BODY_LIMIT_MB,
        )?,
    };
    config.validate().map_err(AppError::BadRequest)
}

fn validate_clock_relations(
    next: &BTreeMap<String, String>,
    updates: &BTreeMap<String, Option<String>>,
) -> Result<(), AppError> {
    const KEYS: [&str; 4] = [
        "CC_SWITCH_ROUTER_CLOCK_MONITOR_ENABLED",
        "CC_SWITCH_ROUTER_CLOCK_PROBE_INTERVAL_SECS",
        "CC_SWITCH_ROUTER_CLOCK_PROBE_TIMEOUT_SECS",
        "CC_SWITCH_ROUTER_CLOCK_SOURCES",
    ];
    if !KEYS.iter().any(|key| updates.contains_key(*key)) {
        return Ok(());
    }
    let defaults = crate::config::ClockHealthConfig::default();
    let sources = resolved_ssh_value(next, updates, KEYS[3])
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or(defaults.sources);
    crate::config::ClockHealthConfig {
        enabled: resolved_ssh_value(next, updates, KEYS[0])
            .map(|value| parse_bool_truthy(&value))
            .unwrap_or(defaults.enabled),
        probe_interval_secs: resolved_ssh_u64(
            next,
            updates,
            KEYS[1],
            defaults.probe_interval_secs,
        )?,
        probe_timeout_secs: resolved_ssh_u64(next, updates, KEYS[2], defaults.probe_timeout_secs)?,
        sources,
    }
    .validate()
    .map_err(AppError::BadRequest)
}

fn validate_lifecycle_relations(
    next: &BTreeMap<String, String>,
    updates: &BTreeMap<String, Option<String>>,
) -> Result<(), AppError> {
    const KEYS: [&str; 6] = [
        "CC_SWITCH_ROUTER_LEASE_TTL_SECS",
        "CC_SWITCH_ROUTER_CLEANUP_INTERVAL_SECS",
        "CC_SWITCH_ROUTER_LEASE_RETENTION_SECS",
        "CC_SWITCH_ROUTER_CLIENT_STALE_SECS",
        "CC_SWITCH_ROUTER_CLIENT_INSTALLATION_RETENTION_SECS",
        "CC_SWITCH_ROUTER_PAUSED_SHARE_STALE_SECS",
    ];
    if !KEYS.iter().any(|key| updates.contains_key(*key)) {
        return Ok(());
    }
    let value = |key: &str| effective_relation_i64(next, updates, key);
    let lease_ttl = value(KEYS[0])?;
    let cleanup = value(KEYS[1])?;
    let lease_retention = value(KEYS[2])?;
    let client_stale = value(KEYS[3])?;
    let installation_retention = value(KEYS[4])?;
    let paused_share_stale = value(KEYS[5])?;
    if lease_retention < lease_ttl {
        return Err(AppError::BadRequest(format!(
            "{} must be greater than or equal to {}",
            KEYS[2], KEYS[0]
        )));
    }
    if installation_retention < client_stale {
        return Err(AppError::BadRequest(format!(
            "{} must be greater than or equal to {}",
            KEYS[4], KEYS[3]
        )));
    }
    if paused_share_stale < cleanup {
        return Err(AppError::BadRequest(format!(
            "{} must be greater than or equal to {}",
            KEYS[5], KEYS[1]
        )));
    }
    Ok(())
}

fn validate_auth_relations(
    next: &BTreeMap<String, String>,
    updates: &BTreeMap<String, Option<String>>,
) -> Result<(), AppError> {
    const KEYS: [&str; 4] = [
        "CC_SWITCH_ROUTER_AUTH_CODE_TTL_SECS",
        "CC_SWITCH_ROUTER_AUTH_CODE_COOLDOWN_SECS",
        "CC_SWITCH_ROUTER_AUTH_SESSION_TTL_SECS",
        "CC_SWITCH_ROUTER_AUTH_REFRESH_TTL_SECS",
    ];
    if !KEYS.iter().any(|key| updates.contains_key(*key)) {
        return Ok(());
    }
    let code_ttl = effective_relation_i64(next, updates, KEYS[0])?;
    let code_cooldown = effective_relation_i64(next, updates, KEYS[1])?;
    let session_ttl = effective_relation_i64(next, updates, KEYS[2])?;
    let refresh_ttl = effective_relation_i64(next, updates, KEYS[3])?;
    if code_cooldown >= code_ttl {
        return Err(AppError::BadRequest(format!(
            "{} must be less than {}",
            KEYS[1], KEYS[0]
        )));
    }
    if refresh_ttl <= session_ttl {
        return Err(AppError::BadRequest(format!(
            "{} must be greater than {}",
            KEYS[3], KEYS[2]
        )));
    }
    Ok(())
}

fn effective_relation_i64(
    next: &BTreeMap<String, String>,
    updates: &BTreeMap<String, Option<String>>,
    key: &str,
) -> Result<i64, AppError> {
    let field = field_by_key(key)
        .ok_or_else(|| AppError::Internal(format!("missing settings schema field: {key}")))?;
    let value = resolved_ssh_value(next, updates, key)
        .or_else(|| field.default.map(str::to_string))
        .ok_or_else(|| AppError::Internal(format!("missing settings default: {key}")))?;
    value
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest(format!("{key} must be an integer, got: {value}")))
}

fn resolved_ssh_value(
    next: &BTreeMap<String, String>,
    _updates: &BTreeMap<String, Option<String>>,
    key: &str,
) -> Option<String> {
    next.get(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn resolved_ssh_u64(
    next: &BTreeMap<String, String>,
    updates: &BTreeMap<String, Option<String>>,
    key: &str,
    default: u64,
) -> Result<u64, AppError> {
    resolved_ssh_value(next, updates, key)
        .map(|value| {
            value.parse::<u64>().map_err(|_| {
                AppError::BadRequest(format!(
                    "{key} must be a non-negative integer, got: {value}"
                ))
            })
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn resolved_ssh_usize(
    next: &BTreeMap<String, String>,
    updates: &BTreeMap<String, Option<String>>,
    key: &str,
    default: usize,
) -> Result<usize, AppError> {
    resolved_ssh_value(next, updates, key)
        .map(|value| {
            value.parse::<usize>().map_err(|_| {
                AppError::BadRequest(format!(
                    "{key} must be a non-negative integer, got: {value}"
                ))
            })
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn validate_client_notification_integer(key: &str, value: i64) -> Result<(), AppError> {
    let range = match key {
        "CC_SWITCH_ROUTER_CLIENT_OFFLINE_ALERT_SECS" => {
            Some((crate::notifications::MIN_OFFLINE_ALERT_SECS, 86_400))
        }
        "CC_SWITCH_ROUTER_CLIENT_RECOVERY_STABLE_SECS" => Some((30, 3_600)),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_COOLDOWN_SECS" => Some((60, 604_800)),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_BATCH_WINDOW_SECS" => Some((1, 600)),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_WINDOW_SECS" => Some((60, 3_600)),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_MIN_CLIENTS" => Some((2, 10_000)),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_PERCENT" => Some((1, 100)),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_REMINDER_SECS" => Some((300, 86_400)),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_RECIPIENT_HOURLY_LIMIT" => Some((1, 10_000)),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_GLOBAL_HOURLY_LIMIT" => Some((1, 100_000)),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_RECIPIENT_HOURLY_LIMIT" => Some((1, 1_000)),
        "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_GLOBAL_HOURLY_LIMIT" => Some((1, 10_000)),
        _ => None,
    };
    if let Some((min, max)) = range {
        if !(min..=max).contains(&value) {
            return Err(AppError::BadRequest(format!(
                "{key} must be between {min} and {max}, got: {value}"
            )));
        }
    }
    Ok(())
}

fn validate_alerting_integer(key: &str, value: i64) -> Result<(), AppError> {
    let range = match key {
        "CC_SWITCH_ROUTER_ALERT_REPEAT_INTERVAL_SECS" => Some((60, 7 * 86_400)),
        "CC_SWITCH_ROUTER_ALERT_HISTORY_RETENTION_DAYS" => Some((1, 3_650)),
        "CC_SWITCH_ROUTER_ALERT_TELEGRAM_TOPIC_ID" => Some((1, i64::from(i32::MAX))),
        _ => None,
    };
    if let Some((min, max)) = range {
        if !(min..=max).contains(&value) {
            return Err(AppError::BadRequest(format!(
                "{key} must be between {min} and {max}, got: {value}"
            )));
        }
    }
    Ok(())
}

fn validate_alerting_relations(
    next: &BTreeMap<String, String>,
    updates: &BTreeMap<String, Option<String>>,
) -> Result<(), AppError> {
    const ALERT_KEYS: &[&str] = &[
        "CC_SWITCH_ROUTER_ALERTING_ENABLED",
        "CC_SWITCH_ROUTER_ALERT_TELEGRAM_ENABLED",
        "CC_SWITCH_ROUTER_ALERT_TELEGRAM_BOT_TOKEN",
        "CC_SWITCH_ROUTER_ALERT_TELEGRAM_CHAT_ID",
        "CC_SWITCH_ROUTER_ALERT_TELEGRAM_TOPIC_ID",
        "CC_SWITCH_ROUTER_ALERT_TELEGRAM_MIN_SEVERITY",
    ];
    if !updates.keys().any(|key| ALERT_KEYS.contains(&key.as_str())) {
        return Ok(());
    }
    let configured = |key: &str| {
        next.get(key)
            .cloned()
            .filter(|value| !value.trim().is_empty())
    };
    let enabled = |key: &str| {
        configured(key)
            .map(|value| parse_bool_truthy(&value))
            .unwrap_or(false)
    };
    if enabled("CC_SWITCH_ROUTER_ALERT_TELEGRAM_ENABLED") {
        for key in [
            "CC_SWITCH_ROUTER_ALERT_TELEGRAM_BOT_TOKEN",
            "CC_SWITCH_ROUTER_ALERT_TELEGRAM_CHAT_ID",
        ] {
            if configured(key).is_none() {
                return Err(AppError::BadRequest(format!(
                    "{key} is required when Telegram alerts are enabled"
                )));
            }
        }
    }
    Ok(())
}

/// The user-facing bot has three interlocking requirements that are cheap to
/// get wrong and expensive to debug: enabling it without a token, selecting
/// webhook mode without a secret, and out-of-range TTL/caps that silently
/// disable delivery.
fn validate_telegram_bot_relations(
    next: &BTreeMap<String, String>,
    updates: &BTreeMap<String, Option<String>>,
) -> Result<(), AppError> {
    const BOT_KEYS: &[&str] = &[
        "CC_SWITCH_ROUTER_TELEGRAM_BOT_ENABLED",
        "CC_SWITCH_ROUTER_TELEGRAM_BOT_TOKEN",
        "CC_SWITCH_ROUTER_TELEGRAM_BOT_MODE",
        "CC_SWITCH_ROUTER_TELEGRAM_WEBHOOK_SECRET",
        "CC_SWITCH_ROUTER_TELEGRAM_BIND_TOKEN_TTL_SECS",
        "CC_SWITCH_ROUTER_TELEGRAM_RECIPIENT_HOURLY_LIMIT",
        "CC_SWITCH_ROUTER_TELEGRAM_GLOBAL_HOURLY_LIMIT",
    ];
    if !updates.keys().any(|key| BOT_KEYS.contains(&key.as_str())) {
        return Ok(());
    }
    let configured = |key: &str| {
        next.get(key)
            .cloned()
            .filter(|value| !value.trim().is_empty())
    };
    let enabled = configured("CC_SWITCH_ROUTER_TELEGRAM_BOT_ENABLED")
        .map(|value| parse_bool_truthy(&value))
        .unwrap_or(false);

    let bot_token = configured("CC_SWITCH_ROUTER_TELEGRAM_BOT_TOKEN");
    if enabled && bot_token.is_none() {
        return Err(AppError::BadRequest(
            "CC_SWITCH_ROUTER_TELEGRAM_BOT_TOKEN is required when the Telegram notification bot is enabled".into(),
        ));
    }
    if let Some(token) = bot_token {
        let token = token.trim();
        let valid = token.split_once(':').is_some_and(|(bot_id, secret)| {
            !bot_id.is_empty()
                && bot_id.bytes().all(|byte| byte.is_ascii_digit())
                && !secret.is_empty()
                && secret
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        });
        if !valid {
            return Err(AppError::BadRequest(
                "CC_SWITCH_ROUTER_TELEGRAM_BOT_TOKEN must use the BotFather format <numeric bot id>:<token>".into(),
            ));
        }
    }

    let webhook_mode = configured("CC_SWITCH_ROUTER_TELEGRAM_BOT_MODE")
        .and_then(|value| crate::config::TelegramBotMode::parse(&value))
        .unwrap_or(crate::config::TelegramBotMode::Polling)
        == crate::config::TelegramBotMode::Webhook;
    if enabled && webhook_mode {
        let secret = configured("CC_SWITCH_ROUTER_TELEGRAM_WEBHOOK_SECRET").ok_or_else(|| {
            AppError::BadRequest(
                "CC_SWITCH_ROUTER_TELEGRAM_WEBHOOK_SECRET is required in webhook mode".into(),
            )
        })?;
        let secret = secret.trim();
        if !(16..=256).contains(&secret.chars().count()) {
            return Err(AppError::BadRequest(
                "CC_SWITCH_ROUTER_TELEGRAM_WEBHOOK_SECRET must be 16 to 256 characters".into(),
            ));
        }
        // Telegram only echoes back this exact alphabet.
        if !secret
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(AppError::BadRequest(
                "CC_SWITCH_ROUTER_TELEGRAM_WEBHOOK_SECRET may only contain A-Z, a-z, 0-9, underscore, and hyphen".into(),
            ));
        }
    }

    for (key, min, max) in [
        ("CC_SWITCH_ROUTER_TELEGRAM_BIND_TOKEN_TTL_SECS", 60, 86_400),
        ("CC_SWITCH_ROUTER_TELEGRAM_RECIPIENT_HOURLY_LIMIT", 1, 1_000),
        ("CC_SWITCH_ROUTER_TELEGRAM_GLOBAL_HOURLY_LIMIT", 1, 10_000),
    ] {
        if let Some(raw) = configured(key) {
            let parsed = raw.trim().parse::<i64>().map_err(|_| {
                AppError::BadRequest(format!("{key} must be an integer, got: {raw}"))
            })?;
            if !(min..=max).contains(&parsed) {
                return Err(AppError::BadRequest(format!(
                    "{key} must be between {min} and {max}, got: {parsed}"
                )));
            }
        }
    }

    let recipient_cap = configured("CC_SWITCH_ROUTER_TELEGRAM_RECIPIENT_HOURLY_LIMIT")
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(10);
    let global_cap = configured("CC_SWITCH_ROUTER_TELEGRAM_GLOBAL_HOURLY_LIMIT")
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(50);
    if recipient_cap > global_cap {
        return Err(AppError::BadRequest(format!(
            "CC_SWITCH_ROUTER_TELEGRAM_RECIPIENT_HOURLY_LIMIT ({recipient_cap}) cannot exceed CC_SWITCH_ROUTER_TELEGRAM_GLOBAL_HOURLY_LIMIT ({global_cap})"
        )));
    }
    Ok(())
}

fn validate_client_notification_relations(
    next: &BTreeMap<String, String>,
    updates: &BTreeMap<String, Option<String>>,
) -> Result<(), AppError> {
    const PAIRS: [(&str, &str); 2] = [
        (
            "CC_SWITCH_ROUTER_CLIENT_ALERT_RECIPIENT_HOURLY_LIMIT",
            "CC_SWITCH_ROUTER_CLIENT_ALERT_GLOBAL_HOURLY_LIMIT",
        ),
        (
            "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_RECIPIENT_HOURLY_LIMIT",
            "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_GLOBAL_HOURLY_LIMIT",
        ),
    ];
    if !PAIRS.iter().any(|(recipient, global)| {
        updates.contains_key(*recipient) || updates.contains_key(*global)
    }) {
        return Ok(());
    }
    for (recipient_key, global_key) in PAIRS {
        let recipient = effective_integer_setting(next, recipient_key)?;
        let global = effective_integer_setting(next, global_key)?;
        if global < recipient {
            return Err(AppError::BadRequest(format!(
                "{global_key} must be greater than or equal to {recipient_key}"
            )));
        }
    }
    Ok(())
}

fn validate_registration_admission_integer(key: &str, value: i64) -> Result<(), AppError> {
    let range = match key {
        "CC_SWITCH_ROUTER_REGISTRATION_SOURCE_RATE_PER_MINUTE" => Some((1, 6_000)),
        "CC_SWITCH_ROUTER_REGISTRATION_SOURCE_BURST" => Some((1, 1_000)),
        "CC_SWITCH_ROUTER_REGISTRATION_GLOBAL_RATE_PER_MINUTE" => Some((1, 60_000)),
        "CC_SWITCH_ROUTER_REGISTRATION_GLOBAL_BURST" => Some((1, 10_000)),
        "CC_SWITCH_ROUTER_REGISTRATION_KEY_RATE_PER_MINUTE" => Some((1, 600)),
        "CC_SWITCH_ROUTER_REGISTRATION_KEY_BURST" => Some((1, 100)),
        "CC_SWITCH_ROUTER_REGISTRATION_BUCKET_IDLE_SECS" => Some((30, 86_400)),
        "CC_SWITCH_ROUTER_REGISTRATION_MAX_SOURCE_BUCKETS" => Some((128, 65_536)),
        "CC_SWITCH_ROUTER_REGISTRATION_MAX_KEY_BUCKETS" => Some((256, 131_072)),
        "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_10M_LIMIT"
        | "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_HOURLY_LIMIT"
        | "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_DAILY_LIMIT"
        | "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_10M_LIMIT"
        | "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_HOURLY_LIMIT"
        | "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_DAILY_LIMIT" => Some((1, 1_000_000)),
        "CC_SWITCH_ROUTER_REGISTRATION_UNOWNED_INSTALLATION_WATERMARK" => Some((1_000, 1_000_000)),
        _ => None,
    };
    if let Some((min, max)) = range {
        if !(min..=max).contains(&value) {
            return Err(AppError::BadRequest(format!(
                "{key} must be between {min} and {max}, got: {value}"
            )));
        }
    }
    Ok(())
}

fn validate_registration_admission_relations(
    next: &BTreeMap<String, String>,
    updates: &BTreeMap<String, Option<String>>,
) -> Result<(), AppError> {
    const SOURCE_KEYS: [&str; 3] = [
        "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_10M_LIMIT",
        "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_HOURLY_LIMIT",
        "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_DAILY_LIMIT",
    ];
    const GLOBAL_KEYS: [&str; 3] = [
        "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_10M_LIMIT",
        "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_HOURLY_LIMIT",
        "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_DAILY_LIMIT",
    ];
    if !updates
        .keys()
        .any(|key| SOURCE_KEYS.contains(&key.as_str()) || GLOBAL_KEYS.contains(&key.as_str()))
    {
        return Ok(());
    }
    for keys in [SOURCE_KEYS, GLOBAL_KEYS] {
        let values = keys
            .map(|key| effective_integer_setting(next, key))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        for index in 1..values.len() {
            if values[index] < values[index - 1] {
                return Err(AppError::BadRequest(format!(
                    "{} must be greater than or equal to {}",
                    keys[index],
                    keys[index - 1]
                )));
            }
        }
    }
    Ok(())
}

fn effective_integer_setting(next: &BTreeMap<String, String>, key: &str) -> Result<i64, AppError> {
    let field = field_by_key(key)
        .ok_or_else(|| AppError::Internal(format!("missing settings schema field: {key}")))?;
    let value = next
        .get(key)
        .map(String::as_str)
        .or(field.default)
        .ok_or_else(|| AppError::Internal(format!("missing settings default: {key}")))?;
    value
        .trim()
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest(format!("{key} must be an integer, got: {value}")))
}

/// Apply the admin's settings updates to the live `DynamicSettings` in
/// place.
///
/// This is the only path that mutates the in-memory dynamic state at
/// runtime. It is intentionally diff-based:
///
/// - Keys *not* mentioned in `updates` are left untouched, so unrelated
///   settings keep their current live value.
/// - `Some(non_empty)` sets the field.
/// - `Some(empty)` / `None` clears the field. Clearing means resetting
///   to its canonical "no override" state (which is the same default
///   `Config::from_env` would pick). This is what makes admin revocation
///   actually take effect — emptying `CC_SWITCH_ROUTER_ADMIN_EMAILS` drops
///   the extras immediately, not "next restart".
///
/// The static `Config` is used solely to look up the built-in default
/// admin (`router@<tunnel-host>`), which is always preserved.
pub fn apply_updates_to_dynamic(
    current: &mut DynamicSettings,
    updates: &BTreeMap<String, Option<String>>,
    static_config: &Config,
) {
    for (key, raw) in updates {
        let value = raw.as_deref().map(str::trim).filter(|s| !s.is_empty());
        match key.as_str() {
            "CC_SWITCH_ROUTER_ADMIN_EMAILS" => {
                let mut set = std::collections::HashSet::new();
                if let Some(list) = value {
                    for piece in list.split(',') {
                        let trimmed = piece.trim().to_ascii_lowercase();
                        if !trimmed.is_empty() {
                            set.insert(trimmed);
                        }
                    }
                }
                // The built-in admin is always present, even when the admin
                // explicitly clears the extras list.
                if let Some(default_admin) = static_config.default_admin_email() {
                    set.insert(default_admin);
                }
                current.admin_emails = set;
            }
            "CC_SWITCH_ROUTER_IP_BLACKLIST" => {
                current.security.ip_blacklist = value
                    .map(crate::dynamic_settings::parse_ip_blacklist)
                    .unwrap_or_default();
            }
            "CC_SWITCH_ROUTER_FOOTER_TELEGRAM_URL" => {
                current.footer_telegram_url = value.map(str::to_string).unwrap_or_default();
            }
            "CC_SWITCH_ROUTER_ALERTING_ENABLED" => {
                current.alerting.enabled = value.map(parse_bool_truthy).unwrap_or(true);
            }
            "CC_SWITCH_ROUTER_ALERT_REPEAT_INTERVAL_SECS" => {
                current.alerting.repeat_interval_secs = value
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(30 * 60);
            }
            "CC_SWITCH_ROUTER_ALERT_HISTORY_RETENTION_DAYS" => {
                current.alerting.history_retention_days =
                    value.and_then(|value| value.parse().ok()).unwrap_or(90);
            }
            "CC_SWITCH_ROUTER_ALERT_TELEGRAM_ENABLED" => {
                current.alerting.telegram_enabled = value.map(parse_bool_truthy).unwrap_or(false);
            }
            "CC_SWITCH_ROUTER_ALERT_TELEGRAM_BOT_TOKEN" => {
                current.alerting.telegram_bot_token = value.map(str::to_string);
            }
            "CC_SWITCH_ROUTER_ALERT_TELEGRAM_CHAT_ID" => {
                current.alerting.telegram_chat_id = value.map(str::to_string);
            }
            "CC_SWITCH_ROUTER_ALERT_TELEGRAM_TOPIC_ID" => {
                current.alerting.telegram_topic_id = value.and_then(|value| value.parse().ok());
            }
            "CC_SWITCH_ROUTER_ALERT_TELEGRAM_MIN_SEVERITY" => {
                current.alerting.telegram_min_severity =
                    value.unwrap_or("warning").to_ascii_lowercase();
            }
            "CC_SWITCH_ROUTER_CLIENT_EMAIL_NOTIFICATIONS_ENABLED" => {
                current.client_notifications.enabled = value.map(parse_bool_truthy).unwrap_or(true);
            }
            "CC_SWITCH_ROUTER_CLIENT_OFFLINE_ALERT_SECS" => {
                current.client_notifications.offline_alert_secs =
                    value.and_then(|v| v.parse().ok()).unwrap_or(180);
            }
            "CC_SWITCH_ROUTER_CLIENT_RECOVERY_STABLE_SECS" => {
                current.client_notifications.recovery_stable_secs =
                    value.and_then(|v| v.parse().ok()).unwrap_or(120);
            }
            "CC_SWITCH_ROUTER_CLIENT_ALERT_COOLDOWN_SECS" => {
                current.client_notifications.cooldown_secs =
                    value.and_then(|v| v.parse().ok()).unwrap_or(30 * 60);
            }
            "CC_SWITCH_ROUTER_CLIENT_ALERT_BATCH_WINDOW_SECS" => {
                current.client_notifications.batch_window_secs =
                    value.and_then(|v| v.parse().ok()).unwrap_or(60);
            }
            "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_WINDOW_SECS" => {
                current.client_notifications.storm_window_secs =
                    value.and_then(|v| v.parse().ok()).unwrap_or(5 * 60);
            }
            "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_MIN_CLIENTS" => {
                current.client_notifications.storm_min_clients =
                    value.and_then(|v| v.parse().ok()).unwrap_or(5);
            }
            "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_PERCENT" => {
                current.client_notifications.storm_percent =
                    value.and_then(|v| v.parse().ok()).unwrap_or(20);
            }
            "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_REMINDER_SECS" => {
                current.client_notifications.storm_reminder_secs =
                    value.and_then(|v| v.parse().ok()).unwrap_or(30 * 60);
            }
            "CC_SWITCH_ROUTER_CLIENT_ALERT_RECIPIENT_HOURLY_LIMIT" => {
                current.client_notifications.recipient_hourly_limit =
                    value.and_then(|v| v.parse().ok()).unwrap_or(10);
            }
            "CC_SWITCH_ROUTER_CLIENT_ALERT_GLOBAL_HOURLY_LIMIT" => {
                current.client_notifications.global_hourly_limit =
                    value.and_then(|v| v.parse().ok()).unwrap_or(50);
            }
            "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_RECIPIENT_HOURLY_LIMIT" => {
                current
                    .client_notifications
                    .registration_recipient_hourly_limit =
                    value.and_then(|v| v.parse().ok()).unwrap_or(3);
            }
            "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_GLOBAL_HOURLY_LIMIT" => {
                current
                    .client_notifications
                    .registration_global_hourly_limit =
                    value.and_then(|v| v.parse().ok()).unwrap_or(10);
            }
            "CC_SWITCH_ROUTER_TELEGRAM_BOT_ENABLED" => {
                current.telegram_bot.enabled = value.map(parse_bool_truthy).unwrap_or(false);
            }
            "CC_SWITCH_ROUTER_TELEGRAM_BOT_TOKEN" => {
                current.telegram_bot.bot_token = value.map(str::to_string);
            }
            "CC_SWITCH_ROUTER_TELEGRAM_BOT_MODE" => {
                current.telegram_bot.mode = value
                    .and_then(crate::config::TelegramBotMode::parse)
                    .unwrap_or(crate::config::TelegramBotMode::Polling);
            }
            "CC_SWITCH_ROUTER_TELEGRAM_WEBHOOK_SECRET" => {
                current.telegram_bot.webhook_secret = value.map(str::to_string);
            }
            "CC_SWITCH_ROUTER_TELEGRAM_BIND_TOKEN_TTL_SECS" => {
                current.telegram_bot.bind_token_ttl_secs =
                    value.and_then(|v| v.parse().ok()).unwrap_or(900);
            }
            "CC_SWITCH_ROUTER_TELEGRAM_RECIPIENT_HOURLY_LIMIT" => {
                current.telegram_bot.recipient_hourly_limit =
                    value.and_then(|v| v.parse().ok()).unwrap_or(10);
            }
            "CC_SWITCH_ROUTER_TELEGRAM_GLOBAL_HOURLY_LIMIT" => {
                current.telegram_bot.global_hourly_limit =
                    value.and_then(|v| v.parse().ok()).unwrap_or(50);
            }
            "CC_SWITCH_ROUTER_MARKET_USD_CNY_RATE" => {
                current.market_usd_cny_rate_micros = value
                    .and_then(|value| crate::market_billing::parse_usd_cny_rate_micros(value).ok())
                    .unwrap_or(crate::market_billing::DEFAULT_USD_CNY_RATE_MICROS);
            }
            "CC_SWITCH_ROUTER_SERVER_LOG_PUBLIC_ENABLED" => {
                current.server_log_public_enabled = value.map(parse_bool_truthy).unwrap_or(true);
            }
            // Restart-required fields (paths, addresses, TTLs, Resend API
            // key, auth limits, verification URLs, email From/Reply-To):
            // these have already been written to the .env file by the
            // caller and will be picked up at the next start. We do not
            // shadow them into DynamicSettings.
            _ => {}
        }
    }
}

fn parse_bool_truthy(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_update_json_contract_uses_strings_for_boolean_fields() {
        let parsed: SettingsUpdateRequest = serde_json::from_value(serde_json::json!({
            "expectedRevision": "revision-1",
            "updates": {
                "CC_SWITCH_ROUTER_CLIENT_EMAIL_NOTIFICATIONS_ENABLED": "true"
            }
        }))
        .expect("string boolean settings should deserialize");
        assert_eq!(
            parsed
                .updates
                .get("CC_SWITCH_ROUTER_CLIENT_EMAIL_NOTIFICATIONS_ENABLED"),
            Some(&Some("true".to_string()))
        );
        assert!(
            serde_json::from_value::<SettingsUpdateRequest>(serde_json::json!({
                "expectedRevision": "revision-1",
                "updates": {
                    "CC_SWITCH_ROUTER_CLIENT_EMAIL_NOTIFICATIONS_ENABLED": true
                }
            }))
            .is_err()
        );
    }

    #[test]
    fn normalize_int_rejects_garbage() {
        let field = field_by_key("CC_SWITCH_ROUTER_METRICS_RETENTION_DAYS").unwrap();
        assert!(normalize_value(field, "abc").is_err());
        assert_eq!(normalize_value(field, " 30 ").unwrap(), Some("30".into()));
    }

    #[test]
    fn market_exchange_rate_is_dynamic_and_canonical() {
        let field = field_by_key("CC_SWITCH_ROUTER_MARKET_USD_CNY_RATE").unwrap();
        assert_eq!(
            normalize_value(field, " 7.120000 ").unwrap(),
            Some("7.12".into())
        );
        assert!(normalize_value(field, "0").is_err());
        assert!(normalize_value(field, "7.1234567").is_err());
        assert!(!field.restart_required);
        assert!(matches!(
            field.dynamic_group,
            Some(DynamicGroup::MarketBilling)
        ));

        let config = test_static_config();
        let mut dynamic = DynamicSettings::from_config(&config);
        let updates = BTreeMap::from([(
            "CC_SWITCH_ROUTER_MARKET_USD_CNY_RATE".into(),
            Some("7.25".into()),
        )]);
        apply_updates_to_dynamic(&mut dynamic, &updates, &config);
        assert_eq!(dynamic.market_usd_cny_rate_micros, 7_250_000);
    }

    #[test]
    fn request_history_retention_enforces_supported_range() {
        let field = field_by_key("CC_SWITCH_ROUTER_REQUEST_LOG_RETENTION_DAYS").unwrap();
        assert!(normalize_value(field, "0").is_err());
        assert_eq!(normalize_value(field, "1").unwrap(), Some("1".into()));
        assert_eq!(normalize_value(field, "365").unwrap(), Some("365".into()));
        assert!(normalize_value(field, "366").is_err());
    }

    #[test]
    fn normalize_bool_canonicalizes() {
        let field = field_by_key("CC_SWITCH_ROUTER_METRICS_ENABLED").unwrap();
        assert_eq!(normalize_value(field, "ON").unwrap(), Some("true".into()));
        assert_eq!(normalize_value(field, "off").unwrap(), Some("false".into()));
        assert!(normalize_value(field, "maybe").is_err());
    }

    #[test]
    fn alert_channels_require_complete_credentials() {
        let existing = HashMap::new();
        let mut telegram = BTreeMap::from([
            (
                "CC_SWITCH_ROUTER_ALERT_TELEGRAM_ENABLED".into(),
                Some("true".into()),
            ),
            ("CC_SWITCH_ROUTER_ALERT_TELEGRAM_BOT_TOKEN".into(), None),
            ("CC_SWITCH_ROUTER_ALERT_TELEGRAM_CHAT_ID".into(), None),
        ]);
        assert!(validate_and_diff(&existing, &telegram).is_err());
        telegram.insert(
            "CC_SWITCH_ROUTER_ALERT_TELEGRAM_BOT_TOKEN".into(),
            Some("bot-token".into()),
        );
        telegram.insert(
            "CC_SWITCH_ROUTER_ALERT_TELEGRAM_CHAT_ID".into(),
            Some("-100123".into()),
        );
        assert!(validate_and_diff(&existing, &telegram).is_ok());
    }

    #[test]
    fn alert_channel_secrets_are_never_returned_by_settings_values_api() {
        let path = std::env::temp_dir().join(format!(
            "cc-switch-router-alert-settings-secret-{}.env",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &path,
            "CC_SWITCH_ROUTER_ALERT_TELEGRAM_BOT_TOKEN=telegram-secret\n",
        )
        .expect("write alert settings fixture");

        let response = values_response(&path).expect("read alert settings values");
        let entry = response
            .values
            .iter()
            .find(|entry| entry.key == "CC_SWITCH_ROUTER_ALERT_TELEGRAM_BOT_TOKEN")
            .expect("alert secret entry");
        assert!(entry.is_secret);
        assert!(entry.has_value);
        assert!(entry.value.is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn database_mode_schema_and_validation_are_strict() {
        let schema = schema_response();
        let mode = schema
            .fields
            .iter()
            .find(|field| field.key == "CC_SWITCH_ROUTER_DB_MODE")
            .expect("database mode field");
        assert!(matches!(mode.field_type, FieldType::Select));
        assert_eq!(mode.options, vec!["local", "turso"]);

        let mode_field = field_by_key("CC_SWITCH_ROUTER_DB_MODE").unwrap();
        assert_eq!(
            normalize_value(mode_field, "TURSO").unwrap(),
            Some("turso".into())
        );
        assert!(normalize_value(mode_field, "remote").is_err());

        let url_field = field_by_key("CC_SWITCH_ROUTER_TURSO_URL").unwrap();
        assert!(normalize_value(url_field, "libsql://router-example.turso.io").is_ok());
        assert!(normalize_value(url_field, "http://router-example.turso.io").is_err());
        assert!(normalize_value(url_field, "libsql://router.turso.io?token=secret").is_err());
    }

    #[test]
    fn ssh_transport_settings_use_runtime_validation_contract() {
        let existing = HashMap::new();
        let valid = BTreeMap::from([
            (
                "CC_SWITCH_ROUTER_SSH_MAX_FORWARD_CONNECTIONS".into(),
                Some("128".into()),
            ),
            (
                "CC_SWITCH_ROUTER_SSH_MAX_FORWARD_CONNECTIONS_PER_TUNNEL".into(),
                Some("64".into()),
            ),
        ]);
        let outcome = validate_and_diff(&existing, &valid).expect("valid SSH transport settings");
        assert_eq!(outcome.restart_required_keys.len(), 2);

        let invalid_capacity = BTreeMap::from([
            (
                "CC_SWITCH_ROUTER_SSH_MAX_FORWARD_CONNECTIONS".into(),
                Some("32".into()),
            ),
            (
                "CC_SWITCH_ROUTER_SSH_MAX_FORWARD_CONNECTIONS_PER_TUNNEL".into(),
                Some("64".into()),
            ),
        ]);
        assert!(validate_and_diff(&existing, &invalid_capacity).is_err());

        let invalid_keepalive = BTreeMap::from([
            (
                "CC_SWITCH_ROUTER_SSH_INACTIVITY_TIMEOUT_SECS".into(),
                Some("100".into()),
            ),
            (
                "CC_SWITCH_ROUTER_SSH_KEEPALIVE_INTERVAL_SECS".into(),
                Some("30".into()),
            ),
            (
                "CC_SWITCH_ROUTER_SSH_KEEPALIVE_MAX".into(),
                Some("3".into()),
            ),
        ]);
        assert!(validate_and_diff(&existing, &invalid_keepalive).is_err());

        let out_of_range = BTreeMap::from([(
            "CC_SWITCH_ROUTER_SSH_BRIDGE_WRITE_STALL_TIMEOUT_SECS".into(),
            Some("29".into()),
        )]);
        assert!(validate_and_diff(&existing, &out_of_range).is_err());
    }

    #[test]
    fn proxy_stream_settings_use_runtime_validation_contract() {
        let request_body_field = SETTINGS_FIELDS
            .iter()
            .find(|field| field.key == "CC_SWITCH_ROUTER_PROXY_REQUEST_BODY_LIMIT_MB")
            .expect("request body limit settings field");
        assert_eq!(
            request_body_field
                .default
                .expect("request body limit default")
                .parse::<u64>()
                .expect("numeric request body limit default"),
            crate::config::DEFAULT_PROXY_REQUEST_BODY_LIMIT_MB,
        );
        assert_eq!(request_body_field.placeholder, request_body_field.default);

        let existing = HashMap::new();
        let valid = BTreeMap::from([
            (
                "CC_SWITCH_ROUTER_PROXY_STREAM_FIRST_EVENT_TIMEOUT_SECS".into(),
                Some("5".into()),
            ),
            (
                "CC_SWITCH_ROUTER_PROXY_STREAM_IDLE_TIMEOUT_SECS".into(),
                Some("3600".into()),
            ),
        ]);
        let outcome = validate_and_diff(&existing, &valid).expect("valid proxy stream settings");
        assert_eq!(outcome.restart_required_keys.len(), 2);

        for (key, value) in [
            (
                "CC_SWITCH_ROUTER_PROXY_STREAM_FIRST_EVENT_TIMEOUT_SECS",
                "4",
            ),
            (
                "CC_SWITCH_ROUTER_PROXY_STREAM_FIRST_EVENT_TIMEOUT_SECS",
                "601",
            ),
            ("CC_SWITCH_ROUTER_PROXY_STREAM_IDLE_TIMEOUT_SECS", "29"),
            ("CC_SWITCH_ROUTER_PROXY_STREAM_IDLE_TIMEOUT_SECS", "3601"),
        ] {
            let updates = BTreeMap::from([(key.into(), Some(value.into()))]);
            assert!(
                validate_and_diff(&existing, &updates).is_err(),
                "accepted invalid {key}={value}"
            );
        }
    }

    #[test]
    fn turso_mode_requires_url_and_secret_token() {
        let existing = HashMap::new();
        let mut updates =
            BTreeMap::from([("CC_SWITCH_ROUTER_DB_MODE".into(), Some("turso".into()))]);
        assert!(validate_and_diff(&existing, &updates).is_err());

        updates.insert(
            "CC_SWITCH_ROUTER_TURSO_URL".into(),
            Some("libsql://router-example.turso.io".into()),
        );
        assert!(validate_and_diff(&existing, &updates).is_err());

        updates.insert(
            "CC_SWITCH_ROUTER_TURSO_AUTH_TOKEN".into(),
            Some("secret-token".into()),
        );
        let outcome = validate_and_diff(&existing, &updates).expect("complete Turso settings");
        assert_eq!(outcome.restart_required_keys.len(), 3);

        let existing = HashMap::from([
            ("CC_SWITCH_ROUTER_DB_MODE".into(), "turso".into()),
            (
                "CC_SWITCH_ROUTER_TURSO_URL".into(),
                "libsql://router-example.turso.io".into(),
            ),
            (
                "CC_SWITCH_ROUTER_TURSO_AUTH_TOKEN".into(),
                "existing-token".into(),
            ),
        ]);
        let updates = BTreeMap::from([("CC_SWITCH_ROUTER_TURSO_AUTH_TOKEN".into(), None)]);
        assert!(validate_and_diff(&existing, &updates).is_err());
    }

    #[test]
    fn turso_token_is_never_returned_by_settings_values_api() {
        let path = std::env::temp_dir().join(format!(
            "cc-switch-router-settings-secret-{}.env",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &path,
            "CC_SWITCH_ROUTER_TURSO_AUTH_TOKEN=do-not-return-this-token\n",
        )
        .expect("write settings fixture");

        let response = values_response(&path).expect("read settings values");
        let token = response
            .values
            .iter()
            .find(|entry| entry.key == "CC_SWITCH_ROUTER_TURSO_AUTH_TOKEN")
            .expect("Turso token entry");
        assert!(token.is_secret);
        assert!(token.has_value);
        assert!(token.value.is_none());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn normalize_email_list_cleans_spaces_and_validates() {
        let field = field_by_key("CC_SWITCH_ROUTER_ADMIN_EMAILS").unwrap();
        assert_eq!(
            normalize_value(field, " a@b.com ,, c@d.io ").unwrap(),
            Some("a@b.com,c@d.io".into())
        );
        assert!(normalize_value(field, "not-an-email").is_err());
    }

    #[test]
    fn legacy_client_notification_recipient_settings_are_not_exposed() {
        assert!(field_by_key("CC_SWITCH_ROUTER_CLIENT_ALERT_EMAILS").is_none());
        assert!(field_by_key("CC_SWITCH_ROUTER_CLIENT_OFFLINE_NOTIFY_OWNER").is_none());
    }

    #[test]
    fn client_notification_thresholds_enforce_safe_ranges() {
        let offline = field_by_key("CC_SWITCH_ROUTER_CLIENT_OFFLINE_ALERT_SECS").unwrap();
        let storm_percent = field_by_key("CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_PERCENT").unwrap();
        let registration_recipient =
            field_by_key("CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_RECIPIENT_HOURLY_LIMIT")
                .unwrap();
        let registration_global =
            field_by_key("CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_GLOBAL_HOURLY_LIMIT").unwrap();
        assert!(normalize_value(offline, "179").is_err());
        assert_eq!(normalize_value(offline, "180").unwrap(), Some("180".into()));
        assert!(normalize_value(storm_percent, "0").is_err());
        assert!(normalize_value(storm_percent, "101").is_err());
        assert!(normalize_value(registration_recipient, "0").is_err());
        assert!(normalize_value(registration_recipient, "1001").is_err());
        assert!(normalize_value(registration_global, "0").is_err());
        assert!(normalize_value(registration_global, "10001").is_err());
    }

    #[test]
    fn registration_notification_global_cap_covers_recipient_cap() {
        let existing = HashMap::new();
        let mut updates = BTreeMap::new();
        updates.insert(
            "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_RECIPIENT_HOURLY_LIMIT".into(),
            Some("11".into()),
        );
        assert!(validate_and_diff(&existing, &updates).is_err());

        updates.insert(
            "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_GLOBAL_HOURLY_LIMIT".into(),
            Some("11".into()),
        );
        let outcome = validate_and_diff(&existing, &updates).expect("compatible lane caps");
        assert!(outcome.restart_required_keys.is_empty());
        assert_eq!(outcome.dynamic_groups.len(), 1);
    }

    #[test]
    fn client_notification_settings_are_dynamic() {
        let fields = SETTINGS_FIELDS
            .iter()
            .filter(|field| field.group == "Client notifications")
            .collect::<Vec<_>>();
        assert!(!fields.is_empty());
        assert!(fields.iter().all(|field| !field.restart_required));
        assert!(
            fields.iter().all(|field| matches!(
                field.dynamic_group,
                Some(DynamicGroup::ClientNotifications)
            ))
        );
    }

    #[test]
    fn registration_admission_settings_restart_and_enforce_runtime_bounds() {
        let fields = SETTINGS_FIELDS
            .iter()
            .filter(|field| field.group == "Registration admission")
            .collect::<Vec<_>>();
        assert_eq!(fields.len(), 16);
        assert!(fields.iter().all(|field| field.restart_required));
        assert!(fields.iter().all(|field| field.dynamic_group.is_none()));

        for (key, min, max) in [
            (
                "CC_SWITCH_ROUTER_REGISTRATION_SOURCE_RATE_PER_MINUTE",
                1,
                6_000,
            ),
            ("CC_SWITCH_ROUTER_REGISTRATION_SOURCE_BURST", 1, 1_000),
            (
                "CC_SWITCH_ROUTER_REGISTRATION_GLOBAL_RATE_PER_MINUTE",
                1,
                60_000,
            ),
            ("CC_SWITCH_ROUTER_REGISTRATION_GLOBAL_BURST", 1, 10_000),
            ("CC_SWITCH_ROUTER_REGISTRATION_KEY_RATE_PER_MINUTE", 1, 600),
            ("CC_SWITCH_ROUTER_REGISTRATION_KEY_BURST", 1, 100),
            ("CC_SWITCH_ROUTER_REGISTRATION_BUCKET_IDLE_SECS", 30, 86_400),
            (
                "CC_SWITCH_ROUTER_REGISTRATION_MAX_SOURCE_BUCKETS",
                128,
                65_536,
            ),
            (
                "CC_SWITCH_ROUTER_REGISTRATION_MAX_KEY_BUCKETS",
                256,
                131_072,
            ),
            (
                "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_10M_LIMIT",
                1,
                1_000_000,
            ),
            (
                "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_HOURLY_LIMIT",
                1,
                1_000_000,
            ),
            (
                "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_DAILY_LIMIT",
                1,
                1_000_000,
            ),
            (
                "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_10M_LIMIT",
                1,
                1_000_000,
            ),
            (
                "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_HOURLY_LIMIT",
                1,
                1_000_000,
            ),
            (
                "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_DAILY_LIMIT",
                1,
                1_000_000,
            ),
            (
                "CC_SWITCH_ROUTER_REGISTRATION_UNOWNED_INSTALLATION_WATERMARK",
                1_000,
                1_000_000,
            ),
        ] {
            let field = field_by_key(key).expect("registration admission setting");
            assert_eq!(
                normalize_value(field, &min.to_string()).expect("minimum value"),
                Some(min.to_string())
            );
            assert_eq!(
                normalize_value(field, &max.to_string()).expect("maximum value"),
                Some(max.to_string())
            );
            assert!(normalize_value(field, &(min - 1).to_string()).is_err());
            assert!(normalize_value(field, &(max + 1).to_string()).is_err());
        }
    }

    #[test]
    fn registration_identity_windows_must_be_monotonic() {
        let existing = HashMap::new();
        let mut updates = BTreeMap::new();
        updates.insert(
            "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_10M_LIMIT".into(),
            Some("101".into()),
        );
        assert!(validate_and_diff(&existing, &updates).is_err());

        updates.clear();
        updates.insert(
            "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_HOURLY_LIMIT".into(),
            Some("6000".into()),
        );
        assert!(validate_and_diff(&existing, &updates).is_err());

        updates.insert(
            "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_DAILY_LIMIT".into(),
            Some("6000".into()),
        );
        let outcome = validate_and_diff(&existing, &updates).expect("monotonic windows");
        assert_eq!(outcome.restart_required_keys.len(), 2);
        assert!(outcome.dynamic_groups.is_empty());
    }

    #[test]
    fn validate_required_field_rejects_clear() {
        let mut existing = HashMap::new();
        existing.insert(
            "CC_SWITCH_ROUTER_TUNNEL_DOMAIN".to_string(),
            "router.example.com".to_string(),
        );
        let mut updates = BTreeMap::new();
        updates.insert("CC_SWITCH_ROUTER_TUNNEL_DOMAIN".into(), Some("".into()));
        assert!(validate_and_diff(&existing, &updates).is_err());
    }

    #[test]
    fn clearing_optional_field_persists_an_empty_assignment() {
        let existing = HashMap::from([(
            "CC_SWITCH_ROUTER_FOOTER_TELEGRAM_URL".into(),
            "https://t.me/example".into(),
        )]);
        let updates = BTreeMap::from([("CC_SWITCH_ROUTER_FOOTER_TELEGRAM_URL".into(), None)]);

        let outcome = validate_and_diff(&existing, &updates).expect("clear optional setting");

        assert_eq!(
            outcome
                .new_env_kv
                .get("CC_SWITCH_ROUTER_FOOTER_TELEGRAM_URL")
                .map(String::as_str),
            Some("")
        );

        let missing = HashMap::new();
        let materialized =
            validate_and_diff(&missing, &updates).expect("materialize empty assignment");
        assert_eq!(
            materialized
                .new_env_kv
                .get("CC_SWITCH_ROUTER_FOOTER_TELEGRAM_URL")
                .map(String::as_str),
            Some("")
        );

        let already_empty =
            HashMap::from([("CC_SWITCH_ROUTER_FOOTER_TELEGRAM_URL".into(), String::new())]);
        let unchanged =
            validate_and_diff(&already_empty, &updates).expect("keep existing empty assignment");
        assert!(unchanged.updated_keys.is_empty());
        assert_eq!(
            unchanged.unchanged_keys,
            vec!["CC_SWITCH_ROUTER_FOOTER_TELEGRAM_URL"]
        );
    }

    #[test]
    fn validate_returns_diff_and_dynamic_groups() {
        let mut existing = HashMap::new();
        existing.insert("CC_SWITCH_ROUTER_MARKET_USD_CNY_RATE".into(), "7".into());
        let mut updates = BTreeMap::new();
        updates.insert(
            "CC_SWITCH_ROUTER_MARKET_USD_CNY_RATE".into(),
            Some("7.2".into()),
        );
        updates.insert(
            "CC_SWITCH_ROUTER_ADMIN_EMAILS".into(),
            Some("ops@example.com".into()),
        );
        let outcome = validate_and_diff(&existing, &updates).unwrap();
        assert_eq!(outcome.updated_keys.len(), 2);
        assert_eq!(outcome.restart_required_keys.len(), 0);
        assert_eq!(outcome.dynamic_groups.len(), 2);
    }

    #[test]
    fn write_and_read_env_roundtrip() {
        use std::env;
        let dir = env::temp_dir();
        let path = dir.join(format!(
            "cc-switch-router-test-{}.env",
            uuid::Uuid::new_v4()
        ));
        let mut kv = BTreeMap::new();
        kv.insert("CC_SWITCH_ROUTER_API_ADDR".into(), "0.0.0.0:80".into());
        kv.insert(
            "CC_SWITCH_ROUTER_RESEND_FROM_NAME".into(),
            "Token \"Switch\" \\ Core #1".into(),
        );
        kv.insert("CUSTOM_EQUALS".into(), "left=right".into());
        write_env_file_atomic(&path, &kv).unwrap();
        let parsed = read_env_file(&path).unwrap();
        assert_eq!(
            parsed.get("CC_SWITCH_ROUTER_API_ADDR").unwrap(),
            "0.0.0.0:80"
        );
        assert_eq!(
            parsed.get("CC_SWITCH_ROUTER_RESEND_FROM_NAME").unwrap(),
            "Token \"Switch\" \\ Core #1"
        );
        assert_eq!(parsed.get("CUSTOM_EQUALS").unwrap(), "left=right");
        kv.insert("CC_SWITCH_ROUTER_API_ADDR".into(), "0.0.0.0:81".into());
        write_env_file_atomic(&path, &kv).unwrap();
        let backup = read_env_file(&path.with_extension("bak")).unwrap();
        assert_eq!(
            backup.get("CC_SWITCH_ROUTER_API_ADDR").unwrap(),
            "0.0.0.0:80"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(path.with_extension("bak"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("bak"));
    }

    #[test]
    fn unrelated_patch_preserves_boot_values() {
        let static_config = test_static_config();
        let mut current = DynamicSettings::from_config(&static_config);
        let mut updates: BTreeMap<String, Option<String>> = BTreeMap::new();
        updates.insert(
            "CC_SWITCH_ROUTER_ADMIN_EMAILS".into(),
            Some("alice@example.com".into()),
        );

        apply_updates_to_dynamic(&mut current, &updates, &static_config);

        assert!(current.admin_emails.contains("alice@example.com"));
        assert!(
            current.admin_emails.contains("router@router.example.com"),
            "default admin must always be present"
        );
    }

    /// Codex's second round: clearing a dynamic field must apply
    /// immediately, not "next restart". This is the admin-revocation
    /// case — emptying the extras list drops them right now, while the
    /// built-in `router@<tunnel-host>` admin is always kept.
    #[test]
    fn clearing_admin_emails_revokes_extras_immediately() {
        let static_config = test_static_config();
        // Start with a runtime that already has an extra admin loaded at boot.
        let mut current = DynamicSettings::from_config(&static_config);
        assert!(current.admin_emails.contains("boot-extra@example.com"));

        // Admin opens settings UI and clears the extras field.
        let mut updates: BTreeMap<String, Option<String>> = BTreeMap::new();
        updates.insert("CC_SWITCH_ROUTER_ADMIN_EMAILS".into(), None);
        apply_updates_to_dynamic(&mut current, &updates, &static_config);

        assert!(
            !current.admin_emails.contains("boot-extra@example.com"),
            "extra admin must be revoked the moment the UI clears the field"
        );
        assert!(
            current.admin_emails.contains("router@router.example.com"),
            "default admin (router@host) is always kept"
        );
    }

    #[test]
    fn client_notification_kill_switch_applies_immediately() {
        let static_config = test_static_config();
        let mut current = DynamicSettings::from_config(&static_config);
        let mut updates = BTreeMap::new();
        updates.insert(
            "CC_SWITCH_ROUTER_CLIENT_EMAIL_NOTIFICATIONS_ENABLED".into(),
            Some("true".into()),
        );
        updates.insert(
            "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_RECIPIENT_HOURLY_LIMIT".into(),
            Some("7".into()),
        );
        updates.insert(
            "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_GLOBAL_HOURLY_LIMIT".into(),
            Some("19".into()),
        );
        apply_updates_to_dynamic(&mut current, &updates, &static_config);
        assert!(current.client_notifications.enabled);
        assert_eq!(
            current
                .client_notifications
                .registration_recipient_hourly_limit,
            7
        );
        assert_eq!(
            current
                .client_notifications
                .registration_global_hourly_limit,
            19
        );

        updates.insert(
            "CC_SWITCH_ROUTER_CLIENT_EMAIL_NOTIFICATIONS_ENABLED".into(),
            None,
        );
        apply_updates_to_dynamic(&mut current, &updates, &static_config);
        assert!(current.client_notifications.enabled);
    }

    #[test]
    fn security_and_dashboard_settings_apply_immediately() {
        let static_config = test_static_config();
        let mut current = DynamicSettings::from_config(&static_config);
        let updates = BTreeMap::from([
            (
                "CC_SWITCH_ROUTER_IP_BLACKLIST".into(),
                Some("203.0.113.0/24".into()),
            ),
            ("CC_SWITCH_ROUTER_FOOTER_TELEGRAM_URL".into(), None),
        ]);

        apply_updates_to_dynamic(&mut current, &updates, &static_config);

        assert!(current.is_ip_blacklisted("203.0.113.7".parse().unwrap()));
        assert!(!current.is_ip_blacklisted("198.51.100.7".parse().unwrap()));
        assert!(current.footer_telegram_link().is_none());
    }

    #[test]
    fn client_notification_offline_window_must_precede_cleanup() {
        let mut config = test_static_config();
        config.cleanup_interval_secs = 300;
        config.client_stale_secs = 3_600;
        let mut settings = crate::config::ClientNotificationSettings::default();
        settings.enabled = true;
        assert!(
            crate::notifications::validate_notification_cleanup_window(&settings, &config).is_ok()
        );
        settings.offline_alert_secs = 3_301;
        assert!(
            crate::notifications::validate_notification_cleanup_window(&settings, &config).is_err()
        );

        config.client_stale_secs = 300;
        let (policy, warning) =
            crate::notifications::ClientNotificationPolicy::for_runtime(&settings, &config);
        assert!(!policy.enabled);
        assert!(warning.is_some());

        settings.enabled = false;
        assert!(
            crate::notifications::validate_notification_cleanup_window(&settings, &config).is_ok(),
            "an invalid active policy must never prevent using the kill switch"
        );
    }

    #[test]
    fn settings_contract_exposes_all_fields_in_seven_domains() {
        let schema = schema_response();
        assert_eq!(SETTINGS_FIELDS.len(), 118);
        assert_eq!(schema.fields.len(), 118);
        assert_eq!(schema.categories.len(), 7);
        assert!(
            SETTINGS_FIELDS
                .iter()
                .all(|field| field.restart_required || field.dynamic_group.is_some()),
            "every no-restart field must publish a live dynamic update"
        );
        assert_eq!(
            schema
                .categories
                .iter()
                .map(|category| category.field_count)
                .sum::<usize>(),
            118
        );
        assert!(schema.fields.iter().all(|field| !field.group.is_empty()));
        let webhook = schema
            .fields
            .iter()
            .find(|field| field.key == "CC_SWITCH_ROUTER_TELEGRAM_WEBHOOK_SECRET")
            .expect("webhook secret field");
        assert_eq!(webhook.dependencies.len(), 1);
        assert_eq!(
            webhook.dependencies[0].key,
            "CC_SWITCH_ROUTER_TELEGRAM_BOT_MODE"
        );
        assert_eq!(webhook.dependencies[0].equals, "webhook");
        let operator_telegram = schema
            .fields
            .iter()
            .find(|field| field.key == "CC_SWITCH_ROUTER_ALERT_TELEGRAM_ENABLED")
            .expect("operator Telegram channel field");
        assert!(
            operator_telegram.dependencies.is_empty(),
            "operator channel configuration must remain available while alert delivery is disabled"
        );
        let clock_sources = schema
            .fields
            .iter()
            .find(|field| field.key == "CC_SWITCH_ROUTER_CLOCK_SOURCES")
            .expect("clock sources field");
        assert_eq!(clock_sources.constraints.min_items, Some(3));
        assert_eq!(clock_sources.constraints.max_items, Some(5));
    }

    #[test]
    fn settings_revision_is_stable_and_changes_with_content() {
        let first = HashMap::from([
            ("B".to_string(), "2".to_string()),
            ("A".to_string(), "1".to_string()),
        ]);
        let reordered = HashMap::from([
            ("A".to_string(), "1".to_string()),
            ("B".to_string(), "2".to_string()),
        ]);
        assert_eq!(settings_revision(&first), settings_revision(&reordered));
        let changed = HashMap::from([
            ("A".to_string(), "1".to_string()),
            ("B".to_string(), "3".to_string()),
        ]);
        assert_ne!(settings_revision(&first), settings_revision(&changed));
    }

    #[test]
    fn settings_snapshot_reports_persisted_restart_boundary() {
        let path = std::env::temp_dir().join(format!(
            "cc-switch-router-settings-snapshot-{}.env",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, "CC_SWITCH_ROUTER_API_ADDR=0.0.0.0:81\n")
            .expect("write settings fixture");
        let startup = SettingsRuntimeSnapshot {
            startup_effective: Arc::new(HashMap::from([(
                "CC_SWITCH_ROUTER_API_ADDR".into(),
                "0.0.0.0:80".into(),
            )])),
            startup_file_keys: Arc::new(HashSet::from(["CC_SWITCH_ROUTER_API_ADDR".into()])),
        };
        let config = test_static_config();
        let dynamic = DynamicSettings::from_config(&config);
        let snapshot =
            snapshot_response(&path, &startup, &dynamic, &config).expect("settings snapshot");
        let api_addr = snapshot
            .values
            .iter()
            .find(|entry| entry.key == "CC_SWITCH_ROUTER_API_ADDR")
            .expect("API address entry");
        assert!(api_addr.pending_restart);
        assert_eq!(api_addr.value.as_deref(), Some("0.0.0.0:81"));
        assert_eq!(api_addr.effective_value.as_deref(), Some("0.0.0.0:80"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn settings_snapshot_keeps_manual_dynamic_file_edits_pending() {
        let path = std::env::temp_dir().join(format!(
            "cc-switch-router-settings-dynamic-snapshot-{}.env",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, "CC_SWITCH_ROUTER_ALERTING_ENABLED=false\n")
            .expect("write settings fixture");
        let runtime = SettingsRuntimeSnapshot::for_tests();
        let config = test_static_config();
        let dynamic = DynamicSettings::from_config(&config);

        let snapshot = snapshot_response(&path, &runtime, &dynamic, &config)
            .expect("dynamic settings snapshot");
        let alerting = snapshot
            .values
            .iter()
            .find(|entry| entry.key == "CC_SWITCH_ROUTER_ALERTING_ENABLED")
            .expect("alerting entry");

        assert_eq!(alerting.value.as_deref(), Some("false"));
        assert_eq!(alerting.effective_value.as_deref(), Some("true"));
        assert_eq!(alerting.effective_source, ValueSource::Default);
        assert!(alerting.pending_restart);
        assert!(
            snapshot
                .pending_restart_keys
                .contains(&"CC_SWITCH_ROUTER_ALERTING_ENABLED".to_string())
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn settings_snapshot_redacts_dynamic_telegram_secret() {
        let path = std::env::temp_dir().join(format!(
            "cc-switch-router-settings-dynamic-secret-{}.env",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &path,
            "CC_SWITCH_ROUTER_TELEGRAM_BOT_TOKEN=telegram-secret\n",
        )
        .expect("write settings fixture");
        let runtime = SettingsRuntimeSnapshot::for_tests();
        let mut config = test_static_config();
        config.telegram_bot.bot_token = Some("telegram-secret".into());
        let dynamic = DynamicSettings::from_config(&config);

        let snapshot = snapshot_response(&path, &runtime, &dynamic, &config)
            .expect("dynamic secret settings snapshot");
        let token = snapshot
            .values
            .iter()
            .find(|entry| entry.key == "CC_SWITCH_ROUTER_TELEGRAM_BOT_TOKEN")
            .expect("Telegram token entry");
        assert!(token.is_secret);
        assert!(token.has_value);
        assert!(token.effective_has_value);
        assert!(token.value.is_none());
        assert!(token.effective_value.is_none());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cross_field_validation_uses_persisted_settings() {
        let existing = HashMap::from([
            (
                "CC_SWITCH_ROUTER_CLIENT_ALERT_RECIPIENT_HOURLY_LIMIT".into(),
                "80".into(),
            ),
            (
                "CC_SWITCH_ROUTER_CLIENT_ALERT_GLOBAL_HOURLY_LIMIT".into(),
                "50".into(),
            ),
            (
                "CC_SWITCH_ROUTER_TELEGRAM_BOT_ENABLED".into(),
                "false".into(),
            ),
            ("CC_SWITCH_ROUTER_TELEGRAM_BOT_TOKEN".into(), String::new()),
        ]);

        let too_low = BTreeMap::from([(
            "CC_SWITCH_ROUTER_CLIENT_ALERT_GLOBAL_HOURLY_LIMIT".into(),
            Some("60".into()),
        )]);
        assert!(validate_and_diff(&existing, &too_low).is_err());

        let valid_cap = BTreeMap::from([(
            "CC_SWITCH_ROUTER_CLIENT_ALERT_GLOBAL_HOURLY_LIMIT".into(),
            Some("100".into()),
        )]);
        assert!(validate_and_diff(&existing, &valid_cap).is_ok());

        let enable_bot = BTreeMap::from([(
            "CC_SWITCH_ROUTER_TELEGRAM_BOT_ENABLED".into(),
            Some("true".into()),
        )]);
        assert!(validate_and_diff(&existing, &enable_bot).is_err());

        let enable_bot_with_token = BTreeMap::from([
            (
                "CC_SWITCH_ROUTER_TELEGRAM_BOT_ENABLED".into(),
                Some("true".into()),
            ),
            (
                "CC_SWITCH_ROUTER_TELEGRAM_BOT_TOKEN".into(),
                Some("123456789:ABCDEFGHIJKLMNOPQRSTUVWXYZ_abcdefghi".into()),
            ),
        ]);
        assert!(validate_and_diff(&existing, &enable_bot_with_token).is_ok());

        for invalid_token in [
            "missing-colon",
            "bot-id:validSecret",
            "123456789:contains/slash",
            "123456789:",
        ] {
            let invalid = BTreeMap::from([(
                "CC_SWITCH_ROUTER_TELEGRAM_BOT_TOKEN".into(),
                Some(invalid_token.into()),
            )]);
            assert!(validate_and_diff(&existing, &invalid).is_err());
        }
    }

    #[test]
    fn clock_and_ip_intelligence_url_lists_are_normalized_and_validated() {
        let clock = field_by_key("CC_SWITCH_ROUTER_CLOCK_SOURCES").expect("clock sources");
        assert!(normalize_value(clock, "https://a.example,https://b.example").is_err());
        assert!(
            normalize_value(
                clock,
                "https://a.example,https://a.example/path,https://c.example"
            )
            .is_err()
        );
        assert!(
            normalize_value(
                clock,
                "https://a.example,https://b.example,https://c.example"
            )
            .is_ok()
        );

        let intel = field_by_key("CC_SWITCH_ROUTER_IP_INTEL_ENDPOINTS").expect("IP intel");
        assert_eq!(
            normalize_value(intel, "intel.example.com/, http://trusted.example/")
                .expect("valid IP intel list")
                .as_deref(),
            Some("https://intel.example.com,http://trusted.example")
        );
    }

    #[test]
    fn lifecycle_and_auth_relations_reject_unsafe_windows() {
        let existing = HashMap::new();
        let lifecycle = BTreeMap::from([(
            "CC_SWITCH_ROUTER_PAUSED_SHARE_STALE_SECS".into(),
            Some("120".into()),
        )]);
        assert!(validate_and_diff(&existing, &lifecycle).is_err());

        let auth = BTreeMap::from([(
            "CC_SWITCH_ROUTER_AUTH_CODE_COOLDOWN_SECS".into(),
            Some("300".into()),
        )]);
        assert!(validate_and_diff(&existing, &auth).is_err());
        let auth = BTreeMap::from([(
            "CC_SWITCH_ROUTER_AUTH_REFRESH_TTL_SECS".into(),
            Some("1800".into()),
        )]);
        assert!(validate_and_diff(&existing, &auth).is_err());
    }

    #[test]
    fn validation_response_targets_the_related_field() {
        let response = validation_response(
            &HashMap::new(),
            &BTreeMap::from([(
                "CC_SWITCH_ROUTER_CLOCK_PROBE_TIMEOUT_SECS".into(),
                Some("20".into()),
            )]),
        );
        assert!(!response.valid);
        assert!(
            response
                .field_errors
                .contains_key("CC_SWITCH_ROUTER_CLOCK_PROBE_TIMEOUT_SECS")
        );
        assert!(response.form_errors.is_empty());
    }

    fn test_static_config() -> Config {
        use std::collections::HashSet;
        use std::net::{IpAddr, Ipv4Addr, SocketAddr};
        Config {
            api_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8787),
            ssh_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 2222),
            tunnel_domain: "router.example.com".into(),
            ssh_public_addr: String::new(),
            ssh_transport: crate::config::SshTransportConfig::default(),
            proxy_stream: crate::config::ProxyStreamConfig::default(),
            use_localhost: true,
            lease_ttl_secs: 60,
            data_dir: std::env::temp_dir(),
            database: crate::config::DatabaseConfig::local(
                std::env::temp_dir().join("cc-switch-router-rebuild-test.db"),
            ),
            host_key_path: std::env::temp_dir().join("cc-switch-router-rebuild-test.key"),
            provision_ssh_private_key_path: std::env::temp_dir()
                .join("cc-switch-router-rebuild-test-id_rsa"),
            provision_ssh_public_key_path: std::env::temp_dir()
                .join("cc-switch-router-rebuild-test-id_rsa.pub"),
            cleanup_interval_secs: 300,
            lease_retention_secs: 24 * 60 * 60,
            request_log_retention_days: 30,
            client_stale_secs: 60 * 60,
            client_installation_retention_secs: 6 * 60 * 60,
            paused_share_stale_secs: 60 * 60,
            resend_api_key: None,
            resend_from: None,
            resend_from_name: None,
            resend_reply_to: None,
            client_notifications: crate::config::ClientNotificationSettings::default(),
            telegram_bot: crate::config::TelegramBotSettings::default(),
            auth_code_ttl_secs: 300,
            auth_code_cooldown_secs: 60,
            auth_session_ttl_secs: 1800,
            auth_refresh_ttl_secs: 30 * 24 * 60 * 60,
            auth_max_verify_attempts: 5,
            auth_email_hourly_limit: 30,
            auth_ip_hourly_limit: 20,
            auth_source_hourly_limit: 10,
            ip_blacklist: String::new(),
            free_share_ip_parallel_limit: 1,
            market_usd_cny_rate_micros: crate::market_billing::DEFAULT_USD_CNY_RATE_MICROS,
            ip_intel_endpoints: Vec::new(),
            verification_service_base_url: "https://example.com".into(),
            verification_service_api_key: None,
            router_owner_email: Some("router@router.example.com".into()),
            admin_emails: HashSet::from([
                "router@router.example.com".to_string(),
                "boot-extra@example.com".to_string(),
            ]),
            ux_telemetry_enabled: false,
            ux_telemetry_retention_days: 7,
            footer_telegram_url: crate::config::DEFAULT_FOOTER_TELEGRAM_URL.to_string(),
            metrics: crate::config::MetricsConfig {
                enabled: true,
                db_path: std::env::temp_dir().join("cc-switch-router-rebuild-test-metrics.db"),
                retention_days: 7,
                sample_interval_secs: 5,
                alerting: crate::config::AlertingSettings::default(),
            },
            clock_health: crate::config::ClockHealthConfig::default(),
        }
    }
}
