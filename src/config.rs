use std::collections::HashSet;
use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};

const APP_NAME: &str = "cc-switch-router";

#[derive(Debug, Clone)]
pub struct Config {
    pub api_addr: SocketAddr,
    pub ssh_addr: SocketAddr,
    pub tunnel_domain: String,
    pub ssh_public_addr: String,
    pub use_localhost: bool,
    pub lease_ttl_secs: i64,
    pub db_path: PathBuf,
    pub host_key_path: PathBuf,
    pub cleanup_interval_secs: u64,
    pub lease_retention_secs: i64,
    pub client_stale_secs: i64,
    pub resend_api_key: Option<String>,
    pub resend_from: Option<String>,
    pub resend_from_name: Option<String>,
    pub resend_reply_to: Option<String>,
    pub auth_code_ttl_secs: i64,
    pub auth_code_cooldown_secs: i64,
    pub auth_session_ttl_secs: i64,
    pub auth_refresh_ttl_secs: i64,
    pub auth_max_verify_attempts: i64,
    pub auth_email_hourly_limit: i64,
    pub auth_ip_hourly_limit: i64,
    pub auth_installation_hourly_limit: i64,
    pub ip_blacklist: String,
    pub free_share_ip_parallel_limit: i64,
    pub verification_service_base_url: String,
    pub verification_service_api_key: Option<String>,
    pub admin_emails: HashSet<String>,
    pub telegram_bot_token: Option<String>,
    pub telegram_chat_id: Option<String>,
    pub telegram_topic_id: Option<i64>,
    pub telegram_notify_all: bool,
    pub telegram_notify_admin: bool,
    pub board_max_len: usize,
    pub board_guest_per_hour: i64,
    pub board_user_per_hour: i64,
    pub board_pin_limit: i64,
    pub board_guest_self_delete_secs: i64,
}

impl Config {
    pub fn from_env() -> Self {
        let tunnel_domain = env_var("CC_SWITCH_ROUTER_TUNNEL_DOMAIN").unwrap_or_default();
        let mut admin_emails =
            parse_admin_emails(env_var("CC_SWITCH_ROUTER_ADMIN_EMAILS").as_deref());
        if let Some(default_admin) = derive_default_admin_email(&tunnel_domain) {
            admin_emails.insert(default_admin);
        }
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
            db_path: env_var("CC_SWITCH_ROUTER_DB_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(default_db_path),
            host_key_path: env_var("CC_SWITCH_ROUTER_HOST_KEY_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(default_host_key_path),
            cleanup_interval_secs: env_var("CC_SWITCH_ROUTER_CLEANUP_INTERVAL_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
            lease_retention_secs: env_var("CC_SWITCH_ROUTER_LEASE_RETENTION_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(7 * 24 * 60 * 60),
            client_stale_secs: env_var("CC_SWITCH_ROUTER_CLIENT_STALE_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(60 * 60),
            resend_api_key: env_var("CC_SWITCH_ROUTER_RESEND_API_KEY"),
            resend_from: env_var("CC_SWITCH_ROUTER_RESEND_FROM")
                .or_else(|| crate::startup_config::default_resend_from(&tunnel_domain)),
            resend_from_name: env_var("CC_SWITCH_ROUTER_RESEND_FROM_NAME"),
            resend_reply_to: env_var("CC_SWITCH_ROUTER_RESEND_REPLY_TO"),
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
            auth_installation_hourly_limit: env_var(
                "CC_SWITCH_ROUTER_AUTH_INSTALLATION_HOURLY_LIMIT",
            )
            .and_then(|v| v.parse().ok())
            .unwrap_or(10),
            ip_blacklist: env_var("CC_SWITCH_ROUTER_IP_BLACKLIST").unwrap_or_default(),
            free_share_ip_parallel_limit: env_var("CC_SWITCH_ROUTER_FREE_SHARE_IP_PARALLEL_LIMIT")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1),
            verification_service_base_url: env_var(
                "CC_SWITCH_ROUTER_VERIFICATION_SERVICE_BASE_URL",
            )
            .unwrap_or_else(|| "https://tokenswitch.org".to_string()),
            verification_service_api_key: env_var("CC_SWITCH_ROUTER_VERIFICATION_SERVICE_API_KEY"),
            admin_emails,
            telegram_bot_token: env_var("CC_SWITCH_ROUTER_TELEGRAM_BOT_TOKEN")
                .filter(|v| !v.trim().is_empty()),
            telegram_chat_id: env_var("CC_SWITCH_ROUTER_TELEGRAM_CHAT_ID")
                .filter(|v| !v.trim().is_empty()),
            telegram_topic_id: env_var("CC_SWITCH_ROUTER_TELEGRAM_TOPIC_ID")
                .and_then(|v| v.trim().parse().ok()),
            telegram_notify_all: env_bool("CC_SWITCH_ROUTER_TELEGRAM_NOTIFY_ALL", true),
            telegram_notify_admin: env_bool("CC_SWITCH_ROUTER_TELEGRAM_NOTIFY_ADMIN", true),
            board_max_len: env_var("CC_SWITCH_ROUTER_BOARD_MAX_LEN")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1000),
            board_guest_per_hour: env_var("CC_SWITCH_ROUTER_BOARD_GUEST_PER_HOUR")
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            board_user_per_hour: env_var("CC_SWITCH_ROUTER_BOARD_USER_PER_HOUR")
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            board_pin_limit: env_var("CC_SWITCH_ROUTER_BOARD_PIN_LIMIT")
                .and_then(|v| v.parse().ok())
                .unwrap_or(3),
            board_guest_self_delete_secs: env_var("CC_SWITCH_ROUTER_BOARD_GUEST_SELF_DELETE_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
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

    pub fn tunnel_url(&self, subdomain: &str) -> String {
        let scheme = if self.use_localhost { "http" } else { "https" };
        format!("{scheme}://{subdomain}.{}", self.tunnel_domain)
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

    pub fn is_market_subdomain(&self, subdomain: &str) -> bool {
        let subdomain = subdomain.trim().to_ascii_lowercase();
        subdomain == "market" || subdomain.starts_with("market-")
    }
}

pub fn default_env_path() -> PathBuf {
    path_in_home(APP_NAME, ".env").unwrap_or_else(|| PathBuf::from("./.env"))
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

fn default_db_path() -> PathBuf {
    path_in_home(APP_NAME, &format!("{APP_NAME}.db"))
        .unwrap_or_else(|| PathBuf::from(format!("./data/{APP_NAME}.db")))
}

fn default_host_key_path() -> PathBuf {
    path_in_home(APP_NAME, "ssh_host_ed25519_key")
        .unwrap_or_else(|| PathBuf::from("./data/ssh_host_ed25519_key"))
}

fn default_env_contents() -> String {
    format!(
        "\
CC_SWITCH_ROUTER_API_ADDR=0.0.0.0:80
CC_SWITCH_ROUTER_SSH_ADDR=0.0.0.0:2222
CC_SWITCH_ROUTER_TUNNEL_DOMAIN=
CC_SWITCH_ROUTER_SSH_PUBLIC_ADDR=
CC_SWITCH_ROUTER_USE_LOCALHOST=false
CC_SWITCH_ROUTER_LEASE_TTL_SECS=60
CC_SWITCH_ROUTER_DB_PATH={}
CC_SWITCH_ROUTER_CLEANUP_INTERVAL_SECS=300
CC_SWITCH_ROUTER_LEASE_RETENTION_SECS=604800
CC_SWITCH_ROUTER_CLIENT_STALE_SECS=3600
CC_SWITCH_ROUTER_AUTH_CODE_TTL_SECS=300
CC_SWITCH_ROUTER_AUTH_CODE_COOLDOWN_SECS=60
CC_SWITCH_ROUTER_AUTH_SESSION_TTL_SECS=1800
CC_SWITCH_ROUTER_AUTH_REFRESH_TTL_SECS=2592000
CC_SWITCH_ROUTER_AUTH_MAX_VERIFY_ATTEMPTS=5
CC_SWITCH_ROUTER_AUTH_EMAIL_HOURLY_LIMIT=30
CC_SWITCH_ROUTER_AUTH_IP_HOURLY_LIMIT=20
CC_SWITCH_ROUTER_AUTH_INSTALLATION_HOURLY_LIMIT=10
CC_SWITCH_ROUTER_IP_BLACKLIST=
CC_SWITCH_ROUTER_FREE_SHARE_IP_PARALLEL_LIMIT=1
CC_SWITCH_ROUTER_RESEND_API_KEY=
# CC_SWITCH_ROUTER_RESEND_FROM defaults to noreply@[CC_SWITCH_ROUTER_TUNNEL_DOMAIN]
CC_SWITCH_ROUTER_RESEND_FROM=
# router@<tunnel_domain-host> is always treated as admin. Use this variable
# to add additional admin emails (comma-separated, case-insensitive).
# Admins post with an OFFICIAL badge and can pin/feature/delete any message.
CC_SWITCH_ROUTER_ADMIN_EMAILS=
# Telegram bot push for new board messages. Leave the token empty to disable.
CC_SWITCH_ROUTER_TELEGRAM_BOT_TOKEN=
CC_SWITCH_ROUTER_TELEGRAM_CHAT_ID=
# CC_SWITCH_ROUTER_TELEGRAM_TOPIC_ID=
CC_SWITCH_ROUTER_TELEGRAM_NOTIFY_ALL=true
CC_SWITCH_ROUTER_TELEGRAM_NOTIFY_ADMIN=true
CC_SWITCH_ROUTER_BOARD_MAX_LEN=1000
CC_SWITCH_ROUTER_BOARD_GUEST_PER_HOUR=5
CC_SWITCH_ROUTER_BOARD_USER_PER_HOUR=30
CC_SWITCH_ROUTER_BOARD_PIN_LIMIT=3
CC_SWITCH_ROUTER_BOARD_GUEST_SELF_DELETE_SECS=300
",
        default_db_path().display()
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

fn path_in_home(app_name: &str, leaf: &str) -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".config").join(app_name).join(leaf))
}

fn existing_env_path() -> Option<PathBuf> {
    let default_path = default_env_path();
    if default_path.exists() {
        return Some(default_path);
    }
    None
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
            db_path: PathBuf::from("/tmp/test.db"),
            host_key_path: PathBuf::from("/tmp/test.key"),
            cleanup_interval_secs: 300,
            lease_retention_secs: 60,
            client_stale_secs: 60,
            resend_api_key: None,
            resend_from: None,
            resend_from_name: None,
            resend_reply_to: None,
            auth_code_ttl_secs: 300,
            auth_code_cooldown_secs: 60,
            auth_session_ttl_secs: 300,
            auth_refresh_ttl_secs: 300,
            auth_max_verify_attempts: 5,
            auth_email_hourly_limit: 30,
            auth_ip_hourly_limit: 5,
            auth_installation_hourly_limit: 5,
            ip_blacklist: String::new(),
            free_share_ip_parallel_limit: 1,
            verification_service_base_url: "https://example.com".into(),
            verification_service_api_key: None,
            admin_emails: HashSet::new(),
            telegram_bot_token: None,
            telegram_chat_id: None,
            telegram_topic_id: None,
            telegram_notify_all: true,
            telegram_notify_admin: true,
            board_max_len: 1000,
            board_guest_per_hour: 5,
            board_user_per_hour: 30,
            board_pin_limit: 3,
            board_guest_self_delete_secs: 300,
        };

        assert!(config.free_share_ip_limit_enabled());

        let disabled = Config {
            free_share_ip_parallel_limit: 0,
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
