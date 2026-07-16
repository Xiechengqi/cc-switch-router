use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

use crate::config::Config;
use crate::dynamic_settings::DynamicSettings;
use crate::error::AppError;

/// Field type informs the frontend how to render the control and how the
/// backend validates the value.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    Text,
    Int,
    Bool,
    Path,
    Url,
    Email,
    EmailList,
    IpList,
    Secret,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DynamicGroup {
    AdminEmails,
    Security,
    Telegram,
    Board,
    ClientNotifications,
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
    pub field_type: FieldType,
    pub required: bool,
    pub restart_required: bool,
    pub default: Option<String>,
    pub description: String,
    pub placeholder: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSchemaResponse {
    pub fields: Vec<SettingsFieldView>,
    pub groups: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingValueEntry {
    pub key: String,
    pub value: Option<String>,
    pub has_value: bool,
    pub is_secret: bool,
    pub source: ValueSource,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueSource {
    EnvFile,
    Default,
    Unset,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsValuesResponse {
    pub values: Vec<SettingValueEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUpdateRequest {
    pub updates: BTreeMap<String, Option<String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUpdateResponse {
    pub updated_keys: Vec<String>,
    pub unchanged_keys: Vec<String>,
    pub restart_required_keys: Vec<String>,
    pub dynamic_groups_refreshed: Vec<String>,
    pub env_path: String,
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
    // ── Persistence ──
    SettingsField {
        key: "CC_SWITCH_ROUTER_DB_PATH",
        label: "SQLite DB path",
        group: "Persistence",
        field_type: FieldType::Path,
        required: false,
        restart_required: true,
        default: None,
        description: "Filesystem path for the SQLite database. Created if missing.",
        placeholder: Some("/var/lib/cc-switch-router/router.db"),
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
        description: "Ed25519 SSH host key. Auto-generated on first start when missing.",
        placeholder: Some("/var/lib/cc-switch-router/ssh_host_ed25519_key"),
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
        default: Some("604800"),
        description: "Historical leases are kept this long before deletion. Default 7 days.",
        placeholder: Some("604800"),
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
        default: Some("86400"),
        description: "Offline installation records are deleted after this duration. Must be >= stale threshold.",
        placeholder: Some("86400"),
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
        description: "From: address used for outgoing mail. Defaults to noreply@<tunnel-domain-host>.",
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
        default: Some("false"),
        description: "Send registration and offline alerts to each client's verified owner with an active Router account.",
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
        label: "Recovery stability (seconds)",
        group: "Client notifications",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("120"),
        description: "Fresh authenticated heartbeats required before an offline client returns online.",
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
        key: "CC_SWITCH_ROUTER_AUTH_INSTALLATION_HOURLY_LIMIT",
        label: "Per-installation hourly limit",
        group: "Email verification & session",
        field_type: FieldType::Int,
        required: false,
        restart_required: true,
        default: Some("10"),
        description: "Maximum login code requests per installation per hour.",
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
    // ── Admin & message board ──
    SettingsField {
        key: "CC_SWITCH_ROUTER_ADMIN_EMAILS",
        label: "Extra admin emails",
        group: "Admin & message board",
        field_type: FieldType::EmailList,
        required: false,
        restart_required: false,
        default: None,
        description: "Comma-separated extra admin emails. router@<tunnel-host> is always admin.",
        placeholder: Some("ops@example.com, sre@example.com"),
        dynamic_group: Some(DynamicGroup::AdminEmails),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_BOARD_MAX_LEN",
        label: "Board: max message length",
        group: "Admin & message board",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("1000"),
        description: "Maximum characters per board message.",
        placeholder: Some("1000"),
        dynamic_group: Some(DynamicGroup::Board),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_BOARD_GUEST_PER_HOUR",
        label: "Board: guest messages / hour",
        group: "Admin & message board",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("5"),
        description: "Hourly cap for anonymous posts per guest id / IP.",
        placeholder: Some("5"),
        dynamic_group: Some(DynamicGroup::Board),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_BOARD_USER_PER_HOUR",
        label: "Board: user messages / hour",
        group: "Admin & message board",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("30"),
        description: "Hourly cap for logged-in non-admin user posts.",
        placeholder: Some("30"),
        dynamic_group: Some(DynamicGroup::Board),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_BOARD_PIN_LIMIT",
        label: "Board: simultaneous pin limit",
        group: "Admin & message board",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("3"),
        description: "Maximum number of messages that can be pinned at the same time.",
        placeholder: Some("3"),
        dynamic_group: Some(DynamicGroup::Board),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_BOARD_GUEST_SELF_DELETE_SECS",
        label: "Board: guest self-delete window (s)",
        group: "Admin & message board",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: Some("300"),
        description: "How long after posting a guest can still delete their own message.",
        placeholder: Some("300"),
        dynamic_group: Some(DynamicGroup::Board),
    },
    // ── Telegram ──
    SettingsField {
        key: "CC_SWITCH_ROUTER_TELEGRAM_BOT_TOKEN",
        label: "Telegram bot token",
        group: "Telegram",
        field_type: FieldType::Secret,
        required: false,
        restart_required: false,
        default: None,
        description: "@BotFather token. Leave empty to disable Telegram notifications.",
        placeholder: Some("123456:ABC-…"),
        dynamic_group: Some(DynamicGroup::Telegram),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_TELEGRAM_CHAT_ID",
        label: "Telegram chat id",
        group: "Telegram",
        field_type: FieldType::Text,
        required: false,
        restart_required: false,
        default: None,
        description: "Numeric chat id (group, channel, or user) for notifications.",
        placeholder: Some("-100123…"),
        dynamic_group: Some(DynamicGroup::Telegram),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_TELEGRAM_TOPIC_ID",
        label: "Telegram topic id (forum)",
        group: "Telegram",
        field_type: FieldType::Int,
        required: false,
        restart_required: false,
        default: None,
        description: "Optional supergroup forum topic id (message_thread_id).",
        placeholder: Some("42"),
        dynamic_group: Some(DynamicGroup::Telegram),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_TELEGRAM_NOTIFY_ALL",
        label: "Notify on every new board message",
        group: "Telegram",
        field_type: FieldType::Bool,
        required: false,
        restart_required: false,
        default: Some("true"),
        description: "Master switch for pushing new board messages to Telegram.",
        placeholder: None,
        dynamic_group: Some(DynamicGroup::Telegram),
    },
    SettingsField {
        key: "CC_SWITCH_ROUTER_TELEGRAM_NOTIFY_ADMIN",
        label: "Also notify when admin posts",
        group: "Telegram",
        field_type: FieldType::Bool,
        required: false,
        restart_required: false,
        default: Some("true"),
        description: "When false, posts from admin accounts are skipped.",
        placeholder: None,
        dynamic_group: Some(DynamicGroup::Telegram),
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
];

pub fn schema_response() -> SettingsSchemaResponse {
    let mut groups = Vec::new();
    let mut seen = HashSet::new();
    for field in SETTINGS_FIELDS {
        if seen.insert(field.group) {
            groups.push(field.group);
        }
    }
    SettingsSchemaResponse {
        fields: SETTINGS_FIELDS
            .iter()
            .map(|field| {
                let mut view = field_to_view(field);
                match field.key {
                    "CC_SWITCH_ROUTER_DB_PATH" => {
                        let path = crate::config::default_db_path().display().to_string();
                        view.placeholder = Some(path);
                    }
                    "CC_SWITCH_ROUTER_HOST_KEY_PATH" => {
                        let path = crate::config::default_host_key_path().display().to_string();
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
            .collect(),
        groups,
    }
}

fn field_to_view(field: &SettingsField) -> SettingsFieldView {
    SettingsFieldView {
        key: field.key.to_string(),
        label: field.label.to_string(),
        group: field.group.to_string(),
        field_type: field.field_type,
        required: field.required,
        restart_required: field.restart_required,
        default: field.default.map(str::to_string),
        description: field.description.to_string(),
        placeholder: field.placeholder.map(str::to_string),
    }
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
    for line in content.lines() {
        let line = line.trim_end_matches('\r').trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let value = value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        out.insert(key.to_string(), value);
    }
    Ok(out)
}

/// Read current values and produce the API response. Secrets surface only
/// `hasValue=true` plus a redacted display so admins can confirm presence
/// without leaking the secret over the wire.
pub fn values_response(env_path: &Path) -> Result<SettingsValuesResponse, AppError> {
    let file_kv = read_env_file(env_path)?;
    let mut entries = Vec::with_capacity(SETTINGS_FIELDS.len());
    for field in SETTINGS_FIELDS {
        let raw = file_kv.get(field.key).cloned();
        let (source, value) = match raw.as_deref() {
            Some(v) if !v.is_empty() => (ValueSource::EnvFile, Some(v.to_string())),
            _ => match field.default {
                Some(d) => (ValueSource::Default, Some(d.to_string())),
                None => (ValueSource::Unset, None),
            },
        };
        let has_value = value.as_deref().map(|v| !v.is_empty()).unwrap_or(false);
        let is_secret = matches!(field.field_type, FieldType::Secret);
        let display_value = if is_secret { None } else { value.clone() };
        entries.push(SettingValueEntry {
            key: field.key.to_string(),
            value: display_value,
            has_value,
            is_secret,
            source,
        });
    }
    Ok(SettingsValuesResponse { values: entries })
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
        let prev_normalized = prev.as_deref().map(str::trim).map(str::to_string);
        let next_normalized = next_value.as_deref().map(str::trim).map(str::to_string);

        if prev_normalized == next_normalized {
            unchanged.push(key.clone());
            continue;
        }
        if field.required && next_normalized.as_deref().unwrap_or("").is_empty() {
            return Err(AppError::BadRequest(format!(
                "{} is required and cannot be cleared",
                field.key
            )));
        }
        match &next_normalized {
            Some(v) if !v.is_empty() => {
                next.insert(key.clone(), v.clone());
            }
            _ => {
                next.remove(key);
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
    validate_registration_admission_relations(&next, updates)?;
    validate_client_notification_relations(&next, updates)?;

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
    fs::write(&tmp, body.as_bytes())
        .map_err(|e| AppError::Internal(format!("write env tmp failed: {e}: {}", tmp.display())))?;
    if path.exists() {
        let bak = path.with_extension("bak");
        let _ = fs::remove_file(&bak);
        if let Err(err) = fs::rename(path, &bak) {
            return Err(AppError::Internal(format!(
                "rotate env bak failed: {err}: {}",
                bak.display()
            )));
        }
    }
    fs::rename(&tmp, path).map_err(|e| {
        AppError::Internal(format!(
            "promote env file failed: {e}: {} -> {}",
            tmp.display(),
            path.display()
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
        let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
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
    match field.field_type {
        FieldType::Int => {
            let value = trimmed.parse::<i64>().map_err(|_| {
                AppError::BadRequest(format!("{} must be an integer, got: {raw}", field.key))
            })?;
            validate_client_notification_integer(field.key, value)?;
            validate_registration_admission_integer(field.key, value)?;
            Ok(Some(trimmed.to_string()))
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
            if !trimmed.contains('@') {
                return Err(AppError::BadRequest(format!(
                    "{} must contain @, got: {raw}",
                    field.key
                )));
            }
            Ok(Some(trimmed.to_string()))
        }
        FieldType::EmailList => {
            let mut cleaned = Vec::new();
            for piece in trimmed.split(',') {
                let part = piece.trim();
                if part.is_empty() {
                    continue;
                }
                if !part.contains('@') {
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
        FieldType::Url => {
            let url = trimmed.to_string();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(AppError::BadRequest(format!(
                    "{} must start with http:// or https://, got: {raw}",
                    field.key
                )));
            }
            Ok(Some(url))
        }
        FieldType::Path | FieldType::Text | FieldType::Secret => Ok(Some(trimmed.to_string())),
    }
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

fn validate_client_notification_relations(
    next: &BTreeMap<String, String>,
    updates: &BTreeMap<String, Option<String>>,
) -> Result<(), AppError> {
    const RECIPIENT_KEY: &str = "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_RECIPIENT_HOURLY_LIMIT";
    const GLOBAL_KEY: &str = "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_GLOBAL_HOURLY_LIMIT";
    if !updates.contains_key(RECIPIENT_KEY) && !updates.contains_key(GLOBAL_KEY) {
        return Ok(());
    }
    let recipient = effective_integer_setting(next, RECIPIENT_KEY)?;
    let global = effective_integer_setting(next, GLOBAL_KEY)?;
    if global < recipient {
        return Err(AppError::BadRequest(format!(
            "{GLOBAL_KEY} must be greater than or equal to {RECIPIENT_KEY}"
        )));
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
/// - Keys *not* mentioned in `updates` are left untouched — the boot
///   snapshot (including values supplied via process env / systemd
///   `Environment=`) survives an unrelated PATCH.
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
            "CC_SWITCH_ROUTER_TELEGRAM_BOT_TOKEN" => {
                current.telegram.bot_token = value.map(str::to_string);
            }
            "CC_SWITCH_ROUTER_IP_BLACKLIST" => {
                current.security.ip_blacklist = value
                    .map(crate::dynamic_settings::parse_ip_blacklist)
                    .unwrap_or_default();
            }
            "CC_SWITCH_ROUTER_TELEGRAM_CHAT_ID" => {
                current.telegram.chat_id = value.map(str::to_string);
            }
            "CC_SWITCH_ROUTER_TELEGRAM_TOPIC_ID" => {
                current.telegram.topic_id = value.and_then(|v| v.parse().ok());
            }
            "CC_SWITCH_ROUTER_TELEGRAM_NOTIFY_ALL" => {
                current.telegram.notify_all = value.map(parse_bool_truthy).unwrap_or(true);
            }
            "CC_SWITCH_ROUTER_TELEGRAM_NOTIFY_ADMIN" => {
                current.telegram.notify_admin = value.map(parse_bool_truthy).unwrap_or(true);
            }
            "CC_SWITCH_ROUTER_BOARD_MAX_LEN" => {
                current.board.max_len = value.and_then(|v| v.parse::<usize>().ok()).unwrap_or(1000);
            }
            "CC_SWITCH_ROUTER_BOARD_GUEST_PER_HOUR" => {
                current.board.guest_per_hour =
                    value.and_then(|v| v.parse::<i64>().ok()).unwrap_or(5);
            }
            "CC_SWITCH_ROUTER_BOARD_USER_PER_HOUR" => {
                current.board.user_per_hour =
                    value.and_then(|v| v.parse::<i64>().ok()).unwrap_or(30);
            }
            "CC_SWITCH_ROUTER_BOARD_PIN_LIMIT" => {
                current.board.pin_limit = value.and_then(|v| v.parse::<i64>().ok()).unwrap_or(3);
            }
            "CC_SWITCH_ROUTER_BOARD_GUEST_SELF_DELETE_SECS" => {
                current.board.guest_self_delete_secs =
                    value.and_then(|v| v.parse::<i64>().ok()).unwrap_or(300);
            }
            "CC_SWITCH_ROUTER_CLIENT_EMAIL_NOTIFICATIONS_ENABLED" => {
                current.client_notifications.enabled =
                    value.map(parse_bool_truthy).unwrap_or(false);
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
                "updates": {
                    "CC_SWITCH_ROUTER_CLIENT_EMAIL_NOTIFICATIONS_ENABLED": true
                }
            }))
            .is_err()
        );
    }

    #[test]
    fn normalize_int_rejects_garbage() {
        let field = field_by_key("CC_SWITCH_ROUTER_BOARD_MAX_LEN").unwrap();
        assert!(normalize_value(field, "abc").is_err());
        assert_eq!(
            normalize_value(field, " 1500 ").unwrap(),
            Some("1500".into())
        );
    }

    #[test]
    fn normalize_bool_canonicalizes() {
        let field = field_by_key("CC_SWITCH_ROUTER_TELEGRAM_NOTIFY_ALL").unwrap();
        assert_eq!(normalize_value(field, "ON").unwrap(), Some("true".into()));
        assert_eq!(normalize_value(field, "off").unwrap(), Some("false".into()));
        assert!(normalize_value(field, "maybe").is_err());
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
    fn validate_returns_diff_and_dynamic_groups() {
        let mut existing = HashMap::new();
        existing.insert("CC_SWITCH_ROUTER_BOARD_MAX_LEN".into(), "1000".into());
        let mut updates = BTreeMap::new();
        updates.insert("CC_SWITCH_ROUTER_BOARD_MAX_LEN".into(), Some("2000".into()));
        updates.insert("CC_SWITCH_ROUTER_BOARD_PIN_LIMIT".into(), Some("5".into()));
        let outcome = validate_and_diff(&existing, &updates).unwrap();
        assert_eq!(outcome.updated_keys.len(), 2);
        assert_eq!(outcome.restart_required_keys.len(), 0);
        assert_eq!(outcome.dynamic_groups.len(), 1);
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
            "Token Switch".into(),
        );
        write_env_file_atomic(&path, &kv).unwrap();
        let parsed = read_env_file(&path).unwrap();
        assert_eq!(
            parsed.get("CC_SWITCH_ROUTER_API_ADDR").unwrap(),
            "0.0.0.0:80"
        );
        assert_eq!(
            parsed.get("CC_SWITCH_ROUTER_RESEND_FROM_NAME").unwrap(),
            "Token Switch"
        );
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("bak"));
    }

    /// Codex's first round: process-env board limits must survive a PATCH
    /// that doesn't mention them. An admin saving an unrelated field
    /// must not silently reset every other dynamic value.
    #[test]
    fn unrelated_patch_preserves_boot_values() {
        let static_config = test_static_config_with_board_overrides();
        let mut current = DynamicSettings::from_config(&static_config);
        let mut updates: BTreeMap<String, Option<String>> = BTreeMap::new();
        updates.insert(
            "CC_SWITCH_ROUTER_ADMIN_EMAILS".into(),
            Some("alice@example.com".into()),
        );

        apply_updates_to_dynamic(&mut current, &updates, &static_config);

        // Board limits from the boot Config (process env) must survive.
        assert_eq!(current.board.max_len, 4242);
        assert_eq!(current.board.guest_per_hour, 17);
        assert_eq!(current.board.user_per_hour, 99);
        assert_eq!(current.board.pin_limit, 7);
        assert_eq!(current.board.guest_self_delete_secs, 600);

        // Telegram bool was set to a non-default at boot; must not snap to true.
        assert!(!current.telegram.notify_all);
        assert!(!current.telegram.notify_admin);
        assert_eq!(current.telegram.bot_token.as_deref(), Some("boot-token"));

        // ADMIN_EMAILS supersedes the boot set, but default admin
        // (derived from tunnel_domain) is still merged in.
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
        let static_config = test_static_config_with_board_overrides();
        // Start with a runtime that already has an extra admin loaded
        // (the "boot-extra@example.com" from process env).
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

    /// Clearing the Telegram bot token must disable notifications
    /// immediately (the API handler will then rebuild the notifier as
    /// None on the same flow).
    #[test]
    fn clearing_telegram_bot_token_disables_in_place() {
        let static_config = test_static_config_with_board_overrides();
        let mut current = DynamicSettings::from_config(&static_config);
        assert_eq!(current.telegram.bot_token.as_deref(), Some("boot-token"));

        let mut updates: BTreeMap<String, Option<String>> = BTreeMap::new();
        updates.insert("CC_SWITCH_ROUTER_TELEGRAM_BOT_TOKEN".into(), None);
        // Also explicit-empty form should behave the same as None.
        updates.insert("CC_SWITCH_ROUTER_TELEGRAM_CHAT_ID".into(), Some("".into()));
        apply_updates_to_dynamic(&mut current, &updates, &static_config);

        assert!(current.telegram.bot_token.is_none());
        assert!(current.telegram.chat_id.is_none());
    }

    /// Clearing a numeric board field falls back to the canonical
    /// default (matches `Config::from_env`'s `.unwrap_or(...)`).
    #[test]
    fn clearing_board_int_resets_to_canonical_default() {
        let static_config = test_static_config_with_board_overrides();
        let mut current = DynamicSettings::from_config(&static_config);
        assert_eq!(current.board.max_len, 4242);

        let mut updates: BTreeMap<String, Option<String>> = BTreeMap::new();
        updates.insert("CC_SWITCH_ROUTER_BOARD_MAX_LEN".into(), None);
        apply_updates_to_dynamic(&mut current, &updates, &static_config);
        assert_eq!(current.board.max_len, 1000, "schema default kicks in");
    }

    #[test]
    fn client_notification_kill_switch_applies_immediately() {
        let static_config = test_static_config_with_board_overrides();
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
        assert!(!current.client_notifications.enabled);
    }

    #[test]
    fn client_notification_offline_window_must_precede_cleanup() {
        let mut config = test_static_config_with_board_overrides();
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

    fn test_static_config_with_board_overrides() -> Config {
        use std::collections::HashSet;
        use std::net::{IpAddr, Ipv4Addr, SocketAddr};
        Config {
            api_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8787),
            ssh_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 2222),
            tunnel_domain: "router.example.com".into(),
            ssh_public_addr: String::new(),
            use_localhost: true,
            lease_ttl_secs: 60,
            db_path: std::env::temp_dir().join("cc-switch-router-rebuild-test.db"),
            host_key_path: std::env::temp_dir().join("cc-switch-router-rebuild-test.key"),
            cleanup_interval_secs: 300,
            lease_retention_secs: 7 * 24 * 60 * 60,
            client_stale_secs: 60 * 60,
            client_installation_retention_secs: 24 * 60 * 60,
            paused_share_stale_secs: 60 * 60,
            resend_api_key: None,
            resend_from: None,
            resend_from_name: None,
            resend_reply_to: None,
            client_notifications: crate::config::ClientNotificationSettings::default(),
            auth_code_ttl_secs: 300,
            auth_code_cooldown_secs: 60,
            auth_session_ttl_secs: 1800,
            auth_refresh_ttl_secs: 30 * 24 * 60 * 60,
            auth_max_verify_attempts: 5,
            auth_email_hourly_limit: 30,
            auth_ip_hourly_limit: 20,
            auth_installation_hourly_limit: 10,
            ip_blacklist: String::new(),
            free_share_ip_parallel_limit: 1,
            verification_service_base_url: "https://example.com".into(),
            verification_service_api_key: None,
            admin_emails: HashSet::from([
                "router@router.example.com".to_string(),
                "boot-extra@example.com".to_string(),
            ]),
            telegram_bot_token: Some("boot-token".into()),
            telegram_chat_id: Some("-100".into()),
            telegram_topic_id: Some(42),
            telegram_notify_all: false,
            telegram_notify_admin: false,
            board_max_len: 4242,
            board_guest_per_hour: 17,
            board_user_per_hour: 99,
            board_pin_limit: 7,
            board_guest_self_delete_secs: 600,
            ux_telemetry_enabled: false,
            ux_telemetry_retention_days: 7,
            metrics: crate::config::MetricsConfig {
                enabled: true,
                db_path: std::env::temp_dir().join("cc-switch-router-rebuild-test-metrics.db"),
                retention_days: 7,
                sample_interval_secs: 5,
            },
        }
    }
}
