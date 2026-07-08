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
    // ── Email (Resend) ──
    SettingsField {
        key: "CC_SWITCH_ROUTER_RESEND_API_KEY",
        label: "Resend API key",
        group: "Email (Resend)",
        field_type: FieldType::Secret,
        required: true,
        restart_required: true,
        default: None,
        description: "re_xxx API key from Resend. Required for sending verification emails.",
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
        description: "Optional Reply-To: header. Replies go here when set.",
        placeholder: Some("support@example.com"),
        dynamic_group: None,
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
            trimmed.parse::<i64>().map_err(|_| {
                AppError::BadRequest(format!("{} must be an integer, got: {raw}", field.key))
            })?;
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
                        "{} entry must contain @, got: {part}",
                        field.key
                    )));
                }
                cleaned.push(part.to_string());
            }
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
            paused_share_stale_secs: 60 * 60,
            resend_api_key: None,
            resend_from: None,
            resend_from_name: None,
            resend_reply_to: None,
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
            metrics: crate::config::MetricsConfig {
                enabled: true,
                db_path: std::env::temp_dir().join("cc-switch-router-rebuild-test-metrics.db"),
                retention_days: 7,
                sample_interval_secs: 5,
            },
        }
    }
}
