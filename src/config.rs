use std::collections::HashSet;
use std::env;
use std::fmt;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const APP_NAME: &str = "cc-switch-router";
pub const DEFAULT_FOOTER_TELEGRAM_URL: &str = "https://t.me/tokenswitchorg";
pub const DEFAULT_REQUEST_LOG_RETENTION_DAYS: u32 = 30;
pub const MIN_REQUEST_LOG_RETENTION_DAYS: u32 = 1;
pub const MAX_REQUEST_LOG_RETENTION_DAYS: u32 = 365;
/// Historical hardcoded IP-intelligence origins. Kept as the default so existing
/// deployments keep working; override with `CC_SWITCH_ROUTER_IP_INTEL_ENDPOINTS`.
const DEFAULT_IP_INTEL_ENDPOINTS: &[&str] = &["http://3.0.3.0", "http://3.0.2.1", "http://3.0.2.9"];

pub const DEFAULT_DB_SYNC_INTERVAL_SECS: u64 = 60;
pub const DEFAULT_CLOCK_PROBE_INTERVAL_SECS: u64 = 60;
pub const DEFAULT_CLOCK_PROBE_TIMEOUT_SECS: u64 = 4;
pub const DEFAULT_CLOCK_SOURCES: &[&str] = &[
    "https://www.cloudflare.com/cdn-cgi/trace",
    "https://www.apple.com/library/test/success.html",
    "https://checkip.amazonaws.com/",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseMode {
    Local,
    Turso,
}

impl DatabaseMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Turso => "turso",
        }
    }

    fn parse(value: &str) -> std::result::Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "local" => Ok(Self::Local),
            "turso" => Ok(Self::Turso),
            _ => Err(format!(
                "CC_SWITCH_ROUTER_DB_MODE must be local or turso, got: {value}"
            )),
        }
    }
}

#[derive(Clone)]
pub struct DatabaseConfig {
    pub mode: DatabaseMode,
    /// Local database file in local mode; embedded-replica file in Turso mode.
    pub path: PathBuf,
    pub turso_url: Option<String>,
    pub turso_auth_token: Option<String>,
    pub sync_interval_secs: u64,
}

impl DatabaseConfig {
    #[cfg(test)]
    pub fn local(path: PathBuf) -> Self {
        Self {
            mode: DatabaseMode::Local,
            path,
            turso_url: None,
            turso_auth_token: None,
            sync_interval_secs: DEFAULT_DB_SYNC_INTERVAL_SECS,
        }
    }

    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.path.as_os_str().is_empty() {
            return Err("CC_SWITCH_ROUTER_DB_PATH cannot be empty".into());
        }
        if !(1..=3_600).contains(&self.sync_interval_secs) {
            return Err(format!(
                "CC_SWITCH_ROUTER_DB_SYNC_INTERVAL_SECS must be between 1 and 3600, got: {}",
                self.sync_interval_secs
            ));
        }
        if self.mode == DatabaseMode::Turso {
            let url = self
                .turso_url
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    "CC_SWITCH_ROUTER_TURSO_URL is required in turso mode".to_string()
                })?;
            validate_turso_url(url)?;
            if self
                .turso_auth_token
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                return Err("CC_SWITCH_ROUTER_TURSO_AUTH_TOKEN is required in turso mode".into());
            }
        }
        Ok(())
    }
}

pub(crate) fn validate_turso_url(value: &str) -> std::result::Result<(), String> {
    let parsed = url::Url::parse(value.trim())
        .map_err(|_| "CC_SWITCH_ROUTER_TURSO_URL must be a valid URL".to_string())?;
    if !matches!(parsed.scheme(), "libsql" | "https") {
        return Err("CC_SWITCH_ROUTER_TURSO_URL must use libsql:// or https://".into());
    }
    if parsed.host_str().is_none() {
        return Err("CC_SWITCH_ROUTER_TURSO_URL must include a host".into());
    }
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(
            "CC_SWITCH_ROUTER_TURSO_URL must not contain credentials, query, or fragment; use CC_SWITCH_ROUTER_TURSO_AUTH_TOKEN for authentication"
                .into(),
        );
    }
    Ok(())
}

impl fmt::Debug for DatabaseConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DatabaseConfig")
            .field("mode", &self.mode)
            .field("path", &self.path)
            .field("turso_url", &self.turso_url)
            .field(
                "turso_auth_token",
                &self.turso_auth_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("sync_interval_secs", &self.sync_interval_secs)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct MetricsConfig {
    pub enabled: bool,
    pub db_path: PathBuf,
    pub retention_days: u32,
    pub sample_interval_secs: u64,
    pub alerting: AlertingSettings,
}

#[derive(Debug, Clone)]
pub struct ClockHealthConfig {
    pub enabled: bool,
    pub probe_interval_secs: u64,
    pub probe_timeout_secs: u64,
    pub sources: Vec<String>,
}

impl Default for ClockHealthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            probe_interval_secs: DEFAULT_CLOCK_PROBE_INTERVAL_SECS,
            probe_timeout_secs: DEFAULT_CLOCK_PROBE_TIMEOUT_SECS,
            sources: DEFAULT_CLOCK_SOURCES
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
        }
    }
}

impl ClockHealthConfig {
    pub fn validate(&self) -> std::result::Result<(), String> {
        if !(15..=3_600).contains(&self.probe_interval_secs) {
            return Err(format!(
                "CC_SWITCH_ROUTER_CLOCK_PROBE_INTERVAL_SECS must be between 15 and 3600, got: {}",
                self.probe_interval_secs
            ));
        }
        if !(1..=15).contains(&self.probe_timeout_secs) {
            return Err(format!(
                "CC_SWITCH_ROUTER_CLOCK_PROBE_TIMEOUT_SECS must be between 1 and 15, got: {}",
                self.probe_timeout_secs
            ));
        }
        if self.sources.len() < 3 || self.sources.len() > 5 {
            return Err(
                "CC_SWITCH_ROUTER_CLOCK_SOURCES must contain between 3 and 5 HTTPS URLs".into(),
            );
        }
        let mut hosts = HashSet::new();
        for source in &self.sources {
            let parsed = url::Url::parse(source)
                .map_err(|_| format!("invalid clock source URL: {source}"))?;
            if parsed.scheme() != "https" || parsed.host_str().is_none() {
                return Err(format!("clock source must be an HTTPS URL: {source}"));
            }
            if !parsed.username().is_empty() || parsed.password().is_some() {
                return Err(format!(
                    "clock source must not contain credentials: {source}"
                ));
            }
            if parsed.query().is_some() || parsed.fragment().is_some() {
                return Err(format!(
                    "clock source must not contain a query or fragment: {source}"
                ));
            }
            let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
            if !hosts.insert(host) {
                return Err("clock sources must use distinct hosts".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct AlertingSettings {
    pub enabled: bool,
    pub repeat_interval_secs: i64,
    pub history_retention_days: u32,
    pub telegram_enabled: bool,
    pub telegram_bot_token: Option<String>,
    pub telegram_chat_id: Option<String>,
    pub telegram_topic_id: Option<i64>,
    pub telegram_min_severity: String,
}

impl Default for AlertingSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            repeat_interval_secs: 30 * 60,
            history_retention_days: 90,
            telegram_enabled: false,
            telegram_bot_token: None,
            telegram_chat_id: None,
            telegram_topic_id: None,
            telegram_min_severity: "warning".into(),
        }
    }
}

impl fmt::Debug for AlertingSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AlertingSettings")
            .field("enabled", &self.enabled)
            .field("repeat_interval_secs", &self.repeat_interval_secs)
            .field("history_retention_days", &self.history_retention_days)
            .field("telegram_enabled", &self.telegram_enabled)
            .field(
                "telegram_bot_token",
                &self.telegram_bot_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("telegram_chat_id", &self.telegram_chat_id)
            .field("telegram_topic_id", &self.telegram_topic_id)
            .field("telegram_min_severity", &self.telegram_min_severity)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientNotificationSettings {
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

impl Default for ClientNotificationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            offline_alert_secs: 180,
            recovery_stable_secs: 120,
            cooldown_secs: 30 * 60,
            batch_window_secs: 60,
            storm_window_secs: 5 * 60,
            storm_min_clients: 5,
            storm_percent: 20,
            storm_reminder_secs: 30 * 60,
            recipient_hourly_limit: 10,
            global_hourly_limit: 50,
            registration_recipient_hourly_limit: 3,
            registration_global_hourly_limit: 10,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub api_addr: SocketAddr,
    pub ssh_addr: SocketAddr,
    pub tunnel_domain: String,
    pub ssh_public_addr: String,
    pub use_localhost: bool,
    pub lease_ttl_secs: i64,
    pub data_dir: PathBuf,
    pub database: DatabaseConfig,
    pub host_key_path: PathBuf,
    /// Outbound Client Market SSH private key (default `$HOME/.ssh/cc-switch-router-provision`).
    pub provision_ssh_private_key_path: PathBuf,
    /// Matching OpenSSH public key (defaults to `<private key path>.pub`).
    pub provision_ssh_public_key_path: PathBuf,
    pub cleanup_interval_secs: u64,
    pub lease_retention_secs: i64,
    pub request_log_retention_days: u32,
    pub client_stale_secs: i64,
    pub client_installation_retention_secs: i64,
    pub paused_share_stale_secs: i64,
    /// Router-side SSH fallback for Client Market installations whose tunnel went offline.
    pub client_market_recovery_enabled: bool,
    pub resend_api_key: Option<String>,
    pub resend_from: Option<String>,
    pub resend_from_name: Option<String>,
    pub resend_reply_to: Option<String>,
    pub client_notifications: ClientNotificationSettings,
    pub auth_code_ttl_secs: i64,
    pub auth_code_cooldown_secs: i64,
    pub auth_session_ttl_secs: i64,
    pub auth_refresh_ttl_secs: i64,
    pub auth_max_verify_attempts: i64,
    pub auth_email_hourly_limit: i64,
    pub auth_ip_hourly_limit: i64,
    pub auth_source_hourly_limit: i64,
    pub ip_blacklist: String,
    pub free_share_ip_parallel_limit: i64,
    pub market_usd_cny_rate_micros: i64,
    /// Base URLs of the IP-intelligence service, tried in order. Every registered
    /// Client Market Host IP is sent to these endpoints, so they should be operated
    /// by the Router operator or a party trusted with the full Host inventory.
    /// Prefer `https://` origins; plaintext entries are logged as a warning at boot.
    pub ip_intel_endpoints: Vec<String>,
    pub verification_service_base_url: String,
    pub verification_service_api_key: Option<String>,
    /// The official Client Market Provider. This is intentionally independent
    /// from the broader administrator set.
    pub router_owner_email: Option<String>,
    pub admin_emails: HashSet<String>,
    pub ux_telemetry_enabled: bool,
    pub ux_telemetry_retention_days: u32,
    pub footer_telegram_url: String,
    pub metrics: MetricsConfig,
    pub clock_health: ClockHealthConfig,
}

impl Config {
    pub fn from_env() -> Self {
        let tunnel_domain = env_var("CC_SWITCH_ROUTER_TUNNEL_DOMAIN").unwrap_or_default();
        let mut admin_emails =
            parse_admin_emails(env_var("CC_SWITCH_ROUTER_ADMIN_EMAILS").as_deref());
        if let Some(default_admin) = derive_default_admin_email(&tunnel_domain) {
            admin_emails.insert(default_admin);
        }
        let router_owner_email = resolve_router_owner_email(
            env_var("CC_SWITCH_ROUTER_OWNER_EMAIL").as_deref(),
            &tunnel_domain,
        );
        let data_dir = env_var("CC_SWITCH_ROUTER_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(default_data_dir);
        let database = DatabaseConfig {
            mode: DatabaseMode::parse(
                env_var("CC_SWITCH_ROUTER_DB_MODE")
                    .as_deref()
                    .unwrap_or("local"),
            )
            .unwrap_or_else(|message| panic!("{message}")),
            path: env_var("CC_SWITCH_ROUTER_DB_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|| default_db_path_in(&data_dir)),
            turso_url: env_var("CC_SWITCH_ROUTER_TURSO_URL"),
            turso_auth_token: env_var("CC_SWITCH_ROUTER_TURSO_AUTH_TOKEN"),
            sync_interval_secs: env_var("CC_SWITCH_ROUTER_DB_SYNC_INTERVAL_SECS")
                .map(|value| {
                    value.parse::<u64>().unwrap_or_else(|_| {
                        panic!("invalid CC_SWITCH_ROUTER_DB_SYNC_INTERVAL_SECS: {value}")
                    })
                })
                .unwrap_or(DEFAULT_DB_SYNC_INTERVAL_SECS),
        };
        let provision_ssh_private_key_path =
            env_var("CC_SWITCH_ROUTER_PROVISION_SSH_PRIVATE_KEY_PATH")
                .or_else(|| env_var("CC_SWITCH_ROUTER_PROVISION_SSH_KEY_PATH"))
                .map(PathBuf::from)
                .unwrap_or_else(default_provision_ssh_private_key_path);
        let provision_ssh_public_key_path =
            env_var("CC_SWITCH_ROUTER_PROVISION_SSH_PUBLIC_KEY_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    provision_ssh_public_path_for_private(&provision_ssh_private_key_path)
                });
        Self {
            api_addr: env_var("CC_SWITCH_ROUTER_API_ADDR")
                .unwrap_or_else(|| "0.0.0.0:80".to_string())
                .parse()
                .expect("invalid CC_SWITCH_ROUTER_API_ADDR"),
            ssh_addr: env_var("CC_SWITCH_ROUTER_SSH_ADDR")
                .unwrap_or_else(|| "0.0.0.0:2222".to_string())
                .parse()
                .expect("invalid CC_SWITCH_ROUTER_SSH_ADDR"),
            tunnel_domain: tunnel_domain.clone(),
            ssh_public_addr: env_var("CC_SWITCH_ROUTER_SSH_PUBLIC_ADDR").unwrap_or_default(),
            use_localhost: env_var("CC_SWITCH_ROUTER_USE_LOCALHOST")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
            lease_ttl_secs: env_var("CC_SWITCH_ROUTER_LEASE_TTL_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
            data_dir: data_dir.clone(),
            database,
            host_key_path: env_var("CC_SWITCH_ROUTER_HOST_KEY_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|| data_dir.join("ssh_host_ed25519_key")),
            provision_ssh_private_key_path,
            provision_ssh_public_key_path,
            cleanup_interval_secs: env_var("CC_SWITCH_ROUTER_CLEANUP_INTERVAL_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
            lease_retention_secs: env_var("CC_SWITCH_ROUTER_LEASE_RETENTION_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(24 * 60 * 60),
            request_log_retention_days: parse_request_log_retention_days(
                env_var("CC_SWITCH_ROUTER_REQUEST_LOG_RETENTION_DAYS").as_deref(),
            )
            .unwrap_or_else(|message| panic!("{message}")),
            client_stale_secs: env_var("CC_SWITCH_ROUTER_CLIENT_STALE_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(60 * 60),
            client_installation_retention_secs: {
                let stale = env_var("CC_SWITCH_ROUTER_CLIENT_STALE_SECS")
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(60 * 60);
                let retention = env_var("CC_SWITCH_ROUTER_CLIENT_INSTALLATION_RETENTION_SECS")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(6 * 60 * 60);
                retention.max(stale)
            },
            paused_share_stale_secs: env_var("CC_SWITCH_ROUTER_PAUSED_SHARE_STALE_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(60 * 60),
            client_market_recovery_enabled: env_bool(
                "CC_SWITCH_ROUTER_CLIENT_MARKET_RECOVERY_ENABLED",
                true,
            ),
            resend_api_key: env_var("CC_SWITCH_ROUTER_RESEND_API_KEY"),
            resend_from: env_var("CC_SWITCH_ROUTER_RESEND_FROM")
                .or_else(|| crate::startup_config::default_resend_from(&tunnel_domain)),
            resend_from_name: env_var("CC_SWITCH_ROUTER_RESEND_FROM_NAME"),
            resend_reply_to: env_var("CC_SWITCH_ROUTER_RESEND_REPLY_TO"),
            client_notifications: ClientNotificationSettings {
                enabled: env_bool("CC_SWITCH_ROUTER_CLIENT_EMAIL_NOTIFICATIONS_ENABLED", true),
                offline_alert_secs: env_i64("CC_SWITCH_ROUTER_CLIENT_OFFLINE_ALERT_SECS", 180),
                recovery_stable_secs: env_i64("CC_SWITCH_ROUTER_CLIENT_RECOVERY_STABLE_SECS", 120),
                cooldown_secs: env_i64("CC_SWITCH_ROUTER_CLIENT_ALERT_COOLDOWN_SECS", 30 * 60),
                batch_window_secs: env_i64("CC_SWITCH_ROUTER_CLIENT_ALERT_BATCH_WINDOW_SECS", 60),
                storm_window_secs: env_i64(
                    "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_WINDOW_SECS",
                    5 * 60,
                ),
                storm_min_clients: env_i64("CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_MIN_CLIENTS", 5),
                storm_percent: env_i64("CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_PERCENT", 20),
                storm_reminder_secs: env_i64(
                    "CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_REMINDER_SECS",
                    30 * 60,
                ),
                recipient_hourly_limit: env_i64(
                    "CC_SWITCH_ROUTER_CLIENT_ALERT_RECIPIENT_HOURLY_LIMIT",
                    10,
                ),
                global_hourly_limit: env_i64(
                    "CC_SWITCH_ROUTER_CLIENT_ALERT_GLOBAL_HOURLY_LIMIT",
                    50,
                ),
                registration_recipient_hourly_limit: env_i64(
                    "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_RECIPIENT_HOURLY_LIMIT",
                    3,
                ),
                registration_global_hourly_limit: env_i64(
                    "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_GLOBAL_HOURLY_LIMIT",
                    10,
                ),
            },
            auth_code_ttl_secs: env_var("CC_SWITCH_ROUTER_AUTH_CODE_TTL_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(5 * 60),
            auth_code_cooldown_secs: env_var("CC_SWITCH_ROUTER_AUTH_CODE_COOLDOWN_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
            auth_session_ttl_secs: env_var("CC_SWITCH_ROUTER_AUTH_SESSION_TTL_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(30 * 60),
            auth_refresh_ttl_secs: env_var("CC_SWITCH_ROUTER_AUTH_REFRESH_TTL_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(30 * 24 * 60 * 60),
            auth_max_verify_attempts: env_var("CC_SWITCH_ROUTER_AUTH_MAX_VERIFY_ATTEMPTS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            auth_email_hourly_limit: env_var("CC_SWITCH_ROUTER_AUTH_EMAIL_HOURLY_LIMIT")
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            auth_ip_hourly_limit: env_var("CC_SWITCH_ROUTER_AUTH_IP_HOURLY_LIMIT")
                .and_then(|v| v.parse().ok())
                .unwrap_or(20),
            auth_source_hourly_limit: env_var("CC_SWITCH_ROUTER_AUTH_SOURCE_HOURLY_LIMIT")
                .and_then(|v| v.parse().ok())
                .unwrap_or(10),
            ip_blacklist: env_var("CC_SWITCH_ROUTER_IP_BLACKLIST").unwrap_or_default(),
            free_share_ip_parallel_limit: env_var("CC_SWITCH_ROUTER_FREE_SHARE_IP_PARALLEL_LIMIT")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1),
            market_usd_cny_rate_micros: env_var("CC_SWITCH_ROUTER_MARKET_USD_CNY_RATE")
                .and_then(|value| crate::market_billing::parse_usd_cny_rate_micros(&value).ok())
                .unwrap_or(crate::market_billing::DEFAULT_USD_CNY_RATE_MICROS),
            ip_intel_endpoints: parse_ip_intel_endpoints(
                env_var("CC_SWITCH_ROUTER_IP_INTEL_ENDPOINTS").as_deref(),
            ),
            verification_service_base_url: env_var(
                "CC_SWITCH_ROUTER_VERIFICATION_SERVICE_BASE_URL",
            )
            .unwrap_or_else(|| "https://tokenswitch.org".to_string()),
            verification_service_api_key: env_var("CC_SWITCH_ROUTER_VERIFICATION_SERVICE_API_KEY"),
            router_owner_email,
            admin_emails,
            ux_telemetry_enabled: env_bool("CC_SWITCH_ROUTER_UX_TELEMETRY_ENABLED", false),
            ux_telemetry_retention_days: env_var("CC_SWITCH_ROUTER_UX_TELEMETRY_RETENTION_DAYS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(7),
            footer_telegram_url: env_var("CC_SWITCH_ROUTER_FOOTER_TELEGRAM_URL")
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| DEFAULT_FOOTER_TELEGRAM_URL.to_string()),
            metrics: MetricsConfig {
                enabled: env_bool("CC_SWITCH_ROUTER_METRICS_ENABLED", true),
                db_path: env_var("CC_SWITCH_ROUTER_METRICS_DB_PATH")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| default_metrics_db_path_in(&data_dir)),
                retention_days: env_var("CC_SWITCH_ROUTER_METRICS_RETENTION_DAYS")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(7),
                sample_interval_secs: env_var("CC_SWITCH_ROUTER_METRICS_SAMPLE_INTERVAL_SECS")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(5),
                alerting: AlertingSettings {
                    enabled: env_bool("CC_SWITCH_ROUTER_ALERTING_ENABLED", true),
                    repeat_interval_secs: env_i64(
                        "CC_SWITCH_ROUTER_ALERT_REPEAT_INTERVAL_SECS",
                        30 * 60,
                    ),
                    history_retention_days: env_var(
                        "CC_SWITCH_ROUTER_ALERT_HISTORY_RETENTION_DAYS",
                    )
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(90),
                    telegram_enabled: env_bool("CC_SWITCH_ROUTER_ALERT_TELEGRAM_ENABLED", false),
                    telegram_bot_token: env_var("CC_SWITCH_ROUTER_ALERT_TELEGRAM_BOT_TOKEN"),
                    telegram_chat_id: env_var("CC_SWITCH_ROUTER_ALERT_TELEGRAM_CHAT_ID"),
                    telegram_topic_id: env_var("CC_SWITCH_ROUTER_ALERT_TELEGRAM_TOPIC_ID")
                        .and_then(|value| value.parse().ok()),
                    telegram_min_severity: env_var("CC_SWITCH_ROUTER_ALERT_TELEGRAM_MIN_SEVERITY")
                        .unwrap_or_else(|| "warning".into()),
                },
            },
            clock_health: ClockHealthConfig {
                enabled: env_bool("CC_SWITCH_ROUTER_CLOCK_MONITOR_ENABLED", true),
                probe_interval_secs: env_var("CC_SWITCH_ROUTER_CLOCK_PROBE_INTERVAL_SECS")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(DEFAULT_CLOCK_PROBE_INTERVAL_SECS),
                probe_timeout_secs: env_var("CC_SWITCH_ROUTER_CLOCK_PROBE_TIMEOUT_SECS")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(DEFAULT_CLOCK_PROBE_TIMEOUT_SECS),
                sources: env_var("CC_SWITCH_ROUTER_CLOCK_SOURCES")
                    .map(|value| {
                        value
                            .split(',')
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(ToOwned::to_owned)
                            .collect()
                    })
                    .unwrap_or_else(|| {
                        DEFAULT_CLOCK_SOURCES
                            .iter()
                            .map(|value| (*value).to_string())
                            .collect()
                    }),
            },
        }
    }

    pub fn is_admin(&self, email: &str) -> bool {
        let normalized = email.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return false;
        }
        self.admin_emails.contains(&normalized)
    }

    /// The built-in admin email derived from `tunnel_domain` (e.g.
    /// `router.example.com` → `router@router.example.com`). Always returned
    /// in lowercase. Returns `None` if the tunnel domain has no usable host.
    pub fn default_admin_email(&self) -> Option<String> {
        derive_default_admin_email(&self.tunnel_domain)
    }

    pub fn official_provider_email(&self) -> Option<&str> {
        self.router_owner_email.as_deref()
    }

    pub fn validate_official_provider_config(&self) -> std::result::Result<(), String> {
        validate_official_provider_config(self.use_localhost, self.router_owner_email.as_deref())
    }

    pub fn validate_database_config(&self) -> std::result::Result<(), String> {
        self.database.validate()?;
        if self.data_dir.as_os_str().is_empty() {
            return Err("CC_SWITCH_ROUTER_DATA_DIR cannot be empty".into());
        }
        if self.database.path == self.metrics.db_path {
            return Err("business database and metrics database must use different paths".into());
        }
        Ok(())
    }

    pub fn validate_clock_health_config(&self) -> std::result::Result<(), String> {
        self.clock_health.validate()
    }

    pub fn tunnel_url(&self, subdomain: &str) -> String {
        let scheme = if self.use_localhost { "http" } else { "https" };
        format!("{scheme}://{subdomain}.{}", self.tunnel_domain)
    }

    pub fn router_id(&self) -> String {
        tunnel_domain_host(&self.tunnel_domain).unwrap_or_else(|| "router".to_string())
    }

    pub fn effective_ssh_public_addr(&self) -> String {
        if !self.ssh_public_addr.is_empty() {
            return self.ssh_public_addr.clone();
        }
        let port = self.ssh_addr.port();
        format!("{}:{}", self.tunnel_domain, port)
    }

    pub fn free_share_ip_limit_enabled(&self) -> bool {
        self.free_share_ip_parallel_limit > 0
    }
}

pub fn default_env_path() -> PathBuf {
    path_in_home(".env").unwrap_or_else(|| PathBuf::from("./.env"))
}

/// Default data directory: `$HOME/.cc-switch-router` (or `./data` when `HOME` is unset).
pub fn default_data_dir() -> PathBuf {
    data_dir_in_home().unwrap_or_else(|| PathBuf::from("./data"))
}

pub fn default_db_path() -> PathBuf {
    default_db_path_in(&default_data_dir())
}

pub fn default_metrics_db_path() -> PathBuf {
    default_metrics_db_path_in(&default_data_dir())
}

pub fn default_host_key_path() -> PathBuf {
    default_data_dir().join("ssh_host_ed25519_key")
}

fn default_db_path_in(data_dir: &Path) -> PathBuf {
    data_dir.join(format!("{APP_NAME}.db"))
}

fn default_metrics_db_path_in(data_dir: &Path) -> PathBuf {
    data_dir.join(format!("{APP_NAME}-metrics.db"))
}

pub fn default_provision_ssh_private_key_path() -> PathBuf {
    path_in_user_ssh("cc-switch-router-provision")
        .unwrap_or_else(|| default_data_dir().join("cc-switch-router-provision"))
}

pub fn default_provision_ssh_public_key_path() -> PathBuf {
    provision_ssh_public_path_for_private(&default_provision_ssh_private_key_path())
}

fn provision_ssh_public_path_for_private(private_key_path: &Path) -> PathBuf {
    let mut value = private_key_path.as_os_str().to_os_string();
    value.push(".pub");
    PathBuf::from(value)
}

fn path_in_user_ssh(leaf: &str) -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".ssh").join(leaf))
}

pub fn ensure_default_env_file() -> Result<PathBuf> {
    let env_path = existing_env_path().unwrap_or_else(default_env_path);
    if let Some(parent) = env_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create env dir failed: {}", parent.display()))?;
    }

    if !env_path.exists() {
        fs::write(&env_path, default_env_contents())
            .with_context(|| format!("write default env failed: {}", env_path.display()))?;
    }

    Ok(env_path)
}

pub fn load_env_file(path: &PathBuf) -> Result<()> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("read env file failed: {}", path.display()))?;

    for (index, raw_line) in content.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            anyhow::bail!("invalid env line {} in {}", index + 1, path.display());
        };

        let key = key.trim();
        if key.is_empty() {
            anyhow::bail!("empty env key on line {} in {}", index + 1, path.display());
        }

        if env::var_os(key).is_none() {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            unsafe {
                env::set_var(key, value);
            }
        }
    }

    Ok(())
}

fn default_env_contents() -> String {
    format!(
        "\
CC_SWITCH_ROUTER_API_ADDR=0.0.0.0:80
CC_SWITCH_ROUTER_SSH_ADDR=0.0.0.0:2222
CC_SWITCH_ROUTER_TUNNEL_DOMAIN=
CC_SWITCH_ROUTER_SSH_PUBLIC_ADDR=
CC_SWITCH_ROUTER_OWNER_EMAIL=
CC_SWITCH_ROUTER_USE_LOCALHOST=false
CC_SWITCH_ROUTER_LEASE_TTL_SECS=60
CC_SWITCH_ROUTER_DATA_DIR={}
CC_SWITCH_ROUTER_DB_MODE=local
CC_SWITCH_ROUTER_DB_PATH={}
# Required only when CC_SWITCH_ROUTER_DB_MODE=turso.
CC_SWITCH_ROUTER_TURSO_URL=
CC_SWITCH_ROUTER_TURSO_AUTH_TOKEN=
CC_SWITCH_ROUTER_DB_SYNC_INTERVAL_SECS=60
CC_SWITCH_ROUTER_CLEANUP_INTERVAL_SECS=300
CC_SWITCH_ROUTER_LEASE_RETENTION_SECS=86400
CC_SWITCH_ROUTER_REQUEST_LOG_RETENTION_DAYS=30
CC_SWITCH_ROUTER_CLIENT_STALE_SECS=3600
CC_SWITCH_ROUTER_CLIENT_INSTALLATION_RETENTION_SECS=21600
CC_SWITCH_ROUTER_REGISTRATION_SOURCE_RATE_PER_MINUTE=60
CC_SWITCH_ROUTER_REGISTRATION_SOURCE_BURST=20
CC_SWITCH_ROUTER_REGISTRATION_GLOBAL_RATE_PER_MINUTE=600
CC_SWITCH_ROUTER_REGISTRATION_GLOBAL_BURST=200
CC_SWITCH_ROUTER_REGISTRATION_KEY_RATE_PER_MINUTE=10
CC_SWITCH_ROUTER_REGISTRATION_KEY_BURST=3
CC_SWITCH_ROUTER_REGISTRATION_BUCKET_IDLE_SECS=600
CC_SWITCH_ROUTER_REGISTRATION_MAX_SOURCE_BUCKETS=8192
CC_SWITCH_ROUTER_REGISTRATION_MAX_KEY_BUCKETS=16384
CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_10M_LIMIT=30
CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_HOURLY_LIMIT=100
CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_DAILY_LIMIT=300
CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_10M_LIMIT=300
CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_HOURLY_LIMIT=1000
CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_DAILY_LIMIT=5000
CC_SWITCH_ROUTER_REGISTRATION_UNOWNED_INSTALLATION_WATERMARK=50000
CC_SWITCH_ROUTER_PAUSED_SHARE_STALE_SECS=3600
# Router waits 30 seconds before recovering an offline Client Market installation.
CC_SWITCH_ROUTER_CLIENT_MARKET_RECOVERY_ENABLED=true
CC_SWITCH_ROUTER_AUTH_CODE_TTL_SECS=300
CC_SWITCH_ROUTER_AUTH_CODE_COOLDOWN_SECS=60
CC_SWITCH_ROUTER_AUTH_SESSION_TTL_SECS=1800
CC_SWITCH_ROUTER_AUTH_REFRESH_TTL_SECS=2592000
CC_SWITCH_ROUTER_AUTH_MAX_VERIFY_ATTEMPTS=5
CC_SWITCH_ROUTER_AUTH_EMAIL_HOURLY_LIMIT=30
CC_SWITCH_ROUTER_AUTH_IP_HOURLY_LIMIT=20
CC_SWITCH_ROUTER_AUTH_SOURCE_HOURLY_LIMIT=10
CC_SWITCH_ROUTER_IP_BLACKLIST=
CC_SWITCH_ROUTER_FREE_SHARE_IP_PARALLEL_LIMIT=1
CC_SWITCH_ROUTER_MARKET_USD_CNY_RATE=7
CC_SWITCH_ROUTER_RESEND_API_KEY=
# CC_SWITCH_ROUTER_RESEND_FROM defaults to noreply@[CC_SWITCH_ROUTER_TUNNEL_DOMAIN]
CC_SWITCH_ROUTER_RESEND_FROM=
# Client lifecycle email notifications default to enabled and go to each currently verified owner.
CC_SWITCH_ROUTER_CLIENT_EMAIL_NOTIFICATIONS_ENABLED=true
CC_SWITCH_ROUTER_CLIENT_OFFLINE_ALERT_SECS=180
CC_SWITCH_ROUTER_CLIENT_RECOVERY_STABLE_SECS=120
CC_SWITCH_ROUTER_CLIENT_ALERT_COOLDOWN_SECS=1800
CC_SWITCH_ROUTER_CLIENT_ALERT_BATCH_WINDOW_SECS=60
CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_WINDOW_SECS=300
CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_MIN_CLIENTS=5
CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_PERCENT=20
CC_SWITCH_ROUTER_CLIENT_ALERT_STORM_REMINDER_SECS=1800
CC_SWITCH_ROUTER_CLIENT_ALERT_RECIPIENT_HOURLY_LIMIT=10
CC_SWITCH_ROUTER_CLIENT_ALERT_GLOBAL_HOURLY_LIMIT=50
CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_RECIPIENT_HOURLY_LIMIT=3
CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_GLOBAL_HOURLY_LIMIT=10
# router@<tunnel_domain-host> is always treated as admin. Use this variable
# to add additional admin emails (comma-separated, case-insensitive).
CC_SWITCH_ROUTER_ADMIN_EMAILS=
CC_SWITCH_ROUTER_UX_TELEMETRY_ENABLED=false
CC_SWITCH_ROUTER_UX_TELEMETRY_RETENTION_DAYS=7
CC_SWITCH_ROUTER_FOOTER_TELEGRAM_URL=https://t.me/tokenswitchorg
CC_SWITCH_ROUTER_METRICS_ENABLED=true
CC_SWITCH_ROUTER_METRICS_DB_PATH={}
CC_SWITCH_ROUTER_METRICS_RETENTION_DAYS=7
CC_SWITCH_ROUTER_METRICS_SAMPLE_INTERVAL_SECS=5
CC_SWITCH_ROUTER_CLOCK_MONITOR_ENABLED=true
CC_SWITCH_ROUTER_CLOCK_PROBE_INTERVAL_SECS=60
CC_SWITCH_ROUTER_CLOCK_PROBE_TIMEOUT_SECS=4
CC_SWITCH_ROUTER_CLOCK_SOURCES=https://www.cloudflare.com/cdn-cgi/trace,https://www.apple.com/library/test/success.html,https://checkip.amazonaws.com/
CC_SWITCH_ROUTER_ALERTING_ENABLED=true
CC_SWITCH_ROUTER_ALERT_REPEAT_INTERVAL_SECS=1800
CC_SWITCH_ROUTER_ALERT_HISTORY_RETENTION_DAYS=90
CC_SWITCH_ROUTER_ALERT_TELEGRAM_ENABLED=false
CC_SWITCH_ROUTER_ALERT_TELEGRAM_BOT_TOKEN=
CC_SWITCH_ROUTER_ALERT_TELEGRAM_CHAT_ID=
CC_SWITCH_ROUTER_ALERT_TELEGRAM_TOPIC_ID=
CC_SWITCH_ROUTER_ALERT_TELEGRAM_MIN_SEVERITY=warning
",
        default_data_dir().display(),
        default_db_path().display(),
        default_metrics_db_path().display()
    )
}

fn env_var(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_bool(key: &str, default: bool) -> bool {
    let Some(value) = env_var(key) else {
        return default;
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => default,
    }
}

fn env_i64(key: &str, default: i64) -> i64 {
    env_var(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn parse_request_log_retention_days(value: Option<&str>) -> std::result::Result<u32, String> {
    let Some(value) = value else {
        return Ok(DEFAULT_REQUEST_LOG_RETENTION_DAYS);
    };
    let days = value.parse::<u32>().map_err(|_| {
        format!(
            "invalid CC_SWITCH_ROUTER_REQUEST_LOG_RETENTION_DAYS: expected an integer between {} and {}",
            MIN_REQUEST_LOG_RETENTION_DAYS, MAX_REQUEST_LOG_RETENTION_DAYS
        )
    })?;
    if !(MIN_REQUEST_LOG_RETENTION_DAYS..=MAX_REQUEST_LOG_RETENTION_DAYS).contains(&days) {
        return Err(format!(
            "invalid CC_SWITCH_ROUTER_REQUEST_LOG_RETENTION_DAYS: expected an integer between {} and {}, got {days}",
            MIN_REQUEST_LOG_RETENTION_DAYS, MAX_REQUEST_LOG_RETENTION_DAYS
        ));
    }
    Ok(days)
}

fn parse_admin_emails(value: Option<&str>) -> HashSet<String> {
    let mut set = HashSet::new();
    let Some(raw) = value else { return set };
    for piece in raw.split(',') {
        let trimmed = piece.trim().to_ascii_lowercase();
        if !trimmed.is_empty() {
            set.insert(trimmed);
        }
    }
    set
}

/// Endpoints receive every registered Host IP, so the default is kept identical to
/// the historical hardcoded list — changing the scheme blindly would break Host
/// registration if those origins do not terminate TLS. Operators who front the
/// service with HTTPS should set `CC_SWITCH_ROUTER_IP_INTEL_ENDPOINTS`.
fn parse_ip_intel_endpoints(value: Option<&str>) -> Vec<String> {
    let parsed: Vec<String> = value
        .unwrap_or_default()
        .split(',')
        .map(|piece| piece.trim().trim_end_matches('/'))
        .filter(|piece| !piece.is_empty())
        .map(|piece| {
            if piece.starts_with("http://") || piece.starts_with("https://") {
                piece.to_string()
            } else {
                format!("https://{piece}")
            }
        })
        .collect();
    if parsed.is_empty() {
        return DEFAULT_IP_INTEL_ENDPOINTS
            .iter()
            .map(|value| value.to_string())
            .collect();
    }
    parsed
}

pub fn tunnel_domain_host(tunnel_domain: &str) -> Option<String> {
    let raw = tunnel_domain.trim();
    if raw.is_empty() {
        return None;
    }
    // Strip the optional :port suffix, including bracketed IPv6 literals.
    let host = if let Some(rest) = raw.strip_prefix('[') {
        let end = rest.find(']')?;
        &rest[..end]
    } else {
        raw.rsplit_once(':').map(|(host, _)| host).unwrap_or(raw)
    };
    let host = host.trim().trim_matches('.');
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

fn derive_default_admin_email(tunnel_domain: &str) -> Option<String> {
    tunnel_domain_host(tunnel_domain).map(|host| format!("router@{host}"))
}

fn resolve_router_owner_email(configured: Option<&str>, tunnel_domain: &str) -> Option<String> {
    let configured = configured
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    match configured {
        Some(value) => crate::notifications::is_basic_email(&value).then_some(value),
        None => derive_default_admin_email(tunnel_domain)
            .filter(|value| crate::notifications::is_basic_email(value)),
    }
}

fn validate_official_provider_config(
    use_localhost: bool,
    router_owner_email: Option<&str>,
) -> std::result::Result<(), String> {
    if use_localhost || router_owner_email.is_some() {
        return Ok(());
    }
    Err(
        "production Client Market requires a valid CC_SWITCH_ROUTER_OWNER_EMAIL or a tunnel domain that resolves router@<tunnel-domain>"
            .into(),
    )
}

fn data_dir_in_home() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(format!(".{APP_NAME}")))
}

fn path_in_home(leaf: &str) -> Option<PathBuf> {
    data_dir_in_home().map(|dir| dir.join(leaf))
}

fn existing_env_path() -> Option<PathBuf> {
    let default_path = default_env_path();
    default_path.exists().then_some(default_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_share_limit_obeys_parallel_limit_setting() {
        let config = Config {
            api_addr: "127.0.0.1:8787".parse().expect("api addr"),
            ssh_addr: "127.0.0.1:2222".parse().expect("ssh addr"),
            tunnel_domain: "example.com".into(),
            ssh_public_addr: String::new(),
            use_localhost: true,
            lease_ttl_secs: 60,
            data_dir: PathBuf::from("/tmp"),
            database: DatabaseConfig::local(PathBuf::from("/tmp/test.db")),
            host_key_path: PathBuf::from("/tmp/test.key"),
            provision_ssh_private_key_path: PathBuf::from("/tmp/test_id_rsa"),
            provision_ssh_public_key_path: PathBuf::from("/tmp/test_id_rsa.pub"),
            cleanup_interval_secs: 300,
            lease_retention_secs: 60,
            request_log_retention_days: DEFAULT_REQUEST_LOG_RETENTION_DAYS,
            client_stale_secs: 60,
            client_installation_retention_secs: 6 * 60 * 60,
            paused_share_stale_secs: 60,
            client_market_recovery_enabled: true,
            resend_api_key: None,
            resend_from: None,
            resend_from_name: None,
            resend_reply_to: None,
            client_notifications: ClientNotificationSettings::default(),
            auth_code_ttl_secs: 300,
            auth_code_cooldown_secs: 60,
            auth_session_ttl_secs: 300,
            auth_refresh_ttl_secs: 300,
            auth_max_verify_attempts: 5,
            auth_email_hourly_limit: 30,
            auth_ip_hourly_limit: 5,
            auth_source_hourly_limit: 5,
            ip_blacklist: String::new(),
            free_share_ip_parallel_limit: 1,
            market_usd_cny_rate_micros: crate::market_billing::DEFAULT_USD_CNY_RATE_MICROS,
            ip_intel_endpoints: Vec::new(),
            verification_service_base_url: "https://example.com".into(),
            verification_service_api_key: None,
            router_owner_email: None,
            admin_emails: HashSet::new(),
            ux_telemetry_enabled: false,
            ux_telemetry_retention_days: 7,
            footer_telegram_url: DEFAULT_FOOTER_TELEGRAM_URL.to_string(),
            metrics: MetricsConfig {
                enabled: true,
                db_path: PathBuf::from("/tmp/test-metrics.db"),
                retention_days: 7,
                sample_interval_secs: 5,
                alerting: AlertingSettings::default(),
            },
            clock_health: ClockHealthConfig::default(),
        };

        assert!(config.free_share_ip_limit_enabled());

        let disabled = Config {
            free_share_ip_parallel_limit: 0,
            ip_intel_endpoints: Vec::new(),
            ..config
        };
        assert!(!disabled.free_share_ip_limit_enabled());
    }

    #[test]
    fn parse_admin_emails_normalizes_and_dedupes() {
        let parsed = parse_admin_emails(Some(" Alice@Example.com, bob@x.org ,alice@example.com,"));
        assert!(parsed.contains("alice@example.com"));
        assert!(parsed.contains("bob@x.org"));
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn client_notification_defaults_are_enabled_and_strictly_capped() {
        let settings = ClientNotificationSettings::default();
        assert!(settings.enabled);
        assert_eq!(settings.recipient_hourly_limit, 10);
        assert_eq!(settings.global_hourly_limit, 50);
        assert_eq!(settings.registration_recipient_hourly_limit, 3);
        assert_eq!(settings.registration_global_hourly_limit, 10);
    }

    #[test]
    fn default_env_excludes_legacy_client_notification_recipient_switches() {
        let contents = default_env_contents();
        assert!(!contents.contains("CC_SWITCH_ROUTER_CLIENT_ALERT_EMAILS"));
        assert!(!contents.contains("CC_SWITCH_ROUTER_CLIENT_OFFLINE_NOTIFY_OWNER"));
    }

    #[test]
    fn default_env_includes_request_log_retention_default() {
        assert!(default_env_contents().contains("CC_SWITCH_ROUTER_REQUEST_LOG_RETENTION_DAYS=30"));
    }

    #[test]
    fn default_env_enables_client_market_recovery() {
        assert!(
            default_env_contents().contains("CC_SWITCH_ROUTER_CLIENT_MARKET_RECOVERY_ENABLED=true")
        );
    }

    #[test]
    fn database_defaults_to_local_mode() {
        let contents = default_env_contents();
        assert!(contents.contains("CC_SWITCH_ROUTER_DB_MODE=local"));
        assert!(contents.contains("CC_SWITCH_ROUTER_DB_SYNC_INTERVAL_SECS=60"));
        assert_eq!(DatabaseMode::parse("LOCAL"), Ok(DatabaseMode::Local));
        assert_eq!(DatabaseMode::parse("turso"), Ok(DatabaseMode::Turso));
        assert!(DatabaseMode::parse("remote").is_err());
    }

    #[test]
    fn turso_database_config_requires_valid_url_and_token() {
        let mut database = DatabaseConfig {
            mode: DatabaseMode::Turso,
            path: PathBuf::from("/tmp/router-replica.db"),
            turso_url: None,
            turso_auth_token: None,
            sync_interval_secs: DEFAULT_DB_SYNC_INTERVAL_SECS,
        };
        assert!(database.validate().unwrap_err().contains("TURSO_URL"));

        database.turso_url = Some("libsql://router-example.turso.io".into());
        assert!(database.validate().unwrap_err().contains("AUTH_TOKEN"));

        database.turso_auth_token = Some("secret-token".into());
        assert!(database.validate().is_ok());
        database.turso_url = Some("https://router-example.turso.io".into());
        assert!(database.validate().is_ok());

        for invalid in [
            "http://router-example.turso.io",
            "libsql://",
            "libsql://user:password@router-example.turso.io",
            "libsql://router-example.turso.io?authToken=secret",
            "not-a-url",
        ] {
            database.turso_url = Some(invalid.into());
            assert!(
                database.validate().is_err(),
                "accepted invalid URL: {invalid}"
            );
        }
    }

    #[test]
    fn database_sync_interval_is_bounded() {
        let mut database = DatabaseConfig::local(PathBuf::from("/tmp/router.db"));
        database.sync_interval_secs = 0;
        assert!(database.validate().is_err());
        database.sync_interval_secs = 1;
        assert!(database.validate().is_ok());
        database.sync_interval_secs = 3_600;
        assert!(database.validate().is_ok());
        database.sync_interval_secs = 3_601;
        assert!(database.validate().is_err());
    }

    #[test]
    fn database_debug_redacts_turso_token() {
        let database = DatabaseConfig {
            mode: DatabaseMode::Turso,
            path: PathBuf::from("/tmp/router-replica.db"),
            turso_url: Some("libsql://router-example.turso.io".into()),
            turso_auth_token: Some("do-not-log-this-token".into()),
            sync_interval_secs: DEFAULT_DB_SYNC_INTERVAL_SECS,
        };
        let debug = format!("{database:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("do-not-log-this-token"));
    }

    #[test]
    fn clock_health_sources_require_distinct_credential_free_https_hosts() {
        assert!(ClockHealthConfig::default().validate().is_ok());

        let mut config = ClockHealthConfig::default();
        config.sources[0] = "http://time.example.test/".into();
        assert!(config.validate().is_err());

        let mut config = ClockHealthConfig::default();
        config.sources[1] = "https://www.cloudflare.com/other".into();
        assert!(config.validate().is_err());

        let mut config = ClockHealthConfig::default();
        config.sources[2] = "https://time.example.test/?token=secret".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn request_log_retention_parser_is_strict_and_bounded() {
        assert_eq!(
            parse_request_log_retention_days(None),
            Ok(DEFAULT_REQUEST_LOG_RETENTION_DAYS)
        );
        assert_eq!(parse_request_log_retention_days(Some("1")), Ok(1));
        assert_eq!(parse_request_log_retention_days(Some("365")), Ok(365));
        assert!(parse_request_log_retention_days(Some("0")).is_err());
        assert!(parse_request_log_retention_days(Some("366")).is_err());
        assert!(parse_request_log_retention_days(Some("invalid")).is_err());
    }

    #[test]
    fn legacy_recipient_env_vars_do_not_gate_owner_notifications() {
        let keys = [
            "CC_SWITCH_ROUTER_CLIENT_EMAIL_NOTIFICATIONS_ENABLED",
            "CC_SWITCH_ROUTER_CLIENT_ALERT_EMAILS",
            "CC_SWITCH_ROUTER_CLIENT_OFFLINE_NOTIFY_OWNER",
        ];
        let previous = keys.map(|key| (key, env::var_os(key)));
        unsafe {
            env::set_var(keys[0], "true");
            env::set_var(keys[1], "");
            env::set_var(keys[2], "false");
        }

        let policy = crate::notifications::ClientNotificationPolicy::from(
            &Config::from_env().client_notifications,
        );

        for (key, value) in previous {
            unsafe {
                if let Some(value) = value {
                    env::set_var(key, value);
                } else {
                    env::remove_var(key);
                }
            }
        }
        assert!(policy.enabled);
    }

    #[test]
    fn client_notification_registration_caps_load_from_env() {
        unsafe {
            env::set_var(
                "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_RECIPIENT_HOURLY_LIMIT",
                "7",
            );
            env::set_var(
                "CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_GLOBAL_HOURLY_LIMIT",
                "19",
            );
        }

        let config = Config::from_env();
        assert_eq!(
            config
                .client_notifications
                .registration_recipient_hourly_limit,
            7
        );
        assert_eq!(
            config.client_notifications.registration_global_hourly_limit,
            19
        );

        unsafe {
            env::remove_var("CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_RECIPIENT_HOURLY_LIMIT");
            env::remove_var("CC_SWITCH_ROUTER_CLIENT_ALERT_REGISTRATION_GLOBAL_HOURLY_LIMIT");
        }
    }

    #[test]
    fn default_env_includes_registration_admission_defaults() {
        let contents = default_env_contents();
        let expected = vec![
            "CC_SWITCH_ROUTER_REGISTRATION_SOURCE_RATE_PER_MINUTE=60",
            "CC_SWITCH_ROUTER_REGISTRATION_SOURCE_BURST=20",
            "CC_SWITCH_ROUTER_REGISTRATION_GLOBAL_RATE_PER_MINUTE=600",
            "CC_SWITCH_ROUTER_REGISTRATION_GLOBAL_BURST=200",
            "CC_SWITCH_ROUTER_REGISTRATION_KEY_RATE_PER_MINUTE=10",
            "CC_SWITCH_ROUTER_REGISTRATION_KEY_BURST=3",
            "CC_SWITCH_ROUTER_REGISTRATION_BUCKET_IDLE_SECS=600",
            "CC_SWITCH_ROUTER_REGISTRATION_MAX_SOURCE_BUCKETS=8192",
            "CC_SWITCH_ROUTER_REGISTRATION_MAX_KEY_BUCKETS=16384",
            "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_10M_LIMIT=30",
            "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_HOURLY_LIMIT=100",
            "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_DAILY_LIMIT=300",
            "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_10M_LIMIT=300",
            "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_HOURLY_LIMIT=1000",
            "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_DAILY_LIMIT=5000",
            "CC_SWITCH_ROUTER_REGISTRATION_UNOWNED_INSTALLATION_WATERMARK=50000",
        ];
        let actual = contents
            .lines()
            .filter(|line| line.starts_with("CC_SWITCH_ROUTER_REGISTRATION_"))
            .collect::<Vec<_>>();

        assert_eq!(actual, expected);
    }

    #[test]
    fn derive_default_admin_email_strips_port_and_lowercases() {
        assert_eq!(
            derive_default_admin_email("Router.Example.COM:8443"),
            Some("router@router.example.com".into())
        );
        assert_eq!(
            derive_default_admin_email("router.example.com"),
            Some("router@router.example.com".into())
        );
        assert_eq!(
            derive_default_admin_email("127.0.0.1:8787"),
            Some("router@127.0.0.1".into())
        );
        assert_eq!(
            derive_default_admin_email("[::1]:8787"),
            Some("router@::1".into())
        );
        assert_eq!(derive_default_admin_email(":8787"), None);
        assert_eq!(derive_default_admin_email("   "), None);
    }

    #[test]
    fn router_owner_email_prefers_valid_override_and_rejects_unresolvable_production() {
        assert_eq!(
            resolve_router_owner_email(Some(" Owner@Example.COM "), "router.invalid"),
            Some("owner@example.com".into())
        );
        assert_eq!(
            resolve_router_owner_email(Some("not-an-email"), "router.example.com"),
            None
        );

        assert!(validate_official_provider_config(false, None).is_err());
        assert!(validate_official_provider_config(true, None).is_ok());
        assert!(validate_official_provider_config(false, Some("owner@example.com")).is_ok());
    }

    #[test]
    fn default_admin_email_is_always_in_admin_set() {
        unsafe {
            env::set_var("CC_SWITCH_ROUTER_TUNNEL_DOMAIN", "router.example.com");
            env::remove_var("CC_SWITCH_ROUTER_ADMIN_EMAILS");
        }
        let config = Config::from_env();
        assert!(config.is_admin("router@router.example.com"));
        assert!(config.is_admin("Router@Router.Example.com"));
        assert!(!config.is_admin("eve@router.example.com"));
        unsafe {
            env::remove_var("CC_SWITCH_ROUTER_TUNNEL_DOMAIN");
        }
    }

    #[test]
    fn resend_from_defaults_to_tunnel_domain_host() {
        unsafe {
            env::set_var("CC_SWITCH_ROUTER_TUNNEL_DOMAIN", "router.example.com:8787");
            env::set_var("CC_SWITCH_ROUTER_RESEND_FROM", "");
        }
        let config = Config::from_env();
        assert_eq!(
            config.resend_from.as_deref(),
            Some("noreply@router.example.com")
        );
        unsafe {
            env::remove_var("CC_SWITCH_ROUTER_TUNNEL_DOMAIN");
            env::remove_var("CC_SWITCH_ROUTER_RESEND_FROM");
        }
    }

    #[test]
    fn default_data_dir_uses_home_dot_prefix() {
        let dir = default_data_dir();
        if let Some(home) = env::var_os("HOME") {
            assert_eq!(dir, PathBuf::from(home).join(".cc-switch-router"));
        } else {
            assert_eq!(dir, PathBuf::from("./data"));
        }
    }

    #[test]
    fn env_bool_falls_back_to_default_for_garbage() {
        unsafe {
            env::set_var("CC_SWITCH_ROUTER_TEST_BOOL_GARBAGE", "maybe");
        }
        assert!(env_bool("CC_SWITCH_ROUTER_TEST_BOOL_GARBAGE", true));
        assert!(!env_bool("CC_SWITCH_ROUTER_TEST_BOOL_GARBAGE", false));
        unsafe {
            env::remove_var("CC_SWITCH_ROUTER_TEST_BOOL_GARBAGE");
        }
    }
}
