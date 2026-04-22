use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};

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
    pub resend_reply_to: Option<String>,
    pub auth_code_ttl_secs: i64,
    pub auth_code_cooldown_secs: i64,
    pub auth_session_ttl_secs: i64,
    pub auth_refresh_ttl_secs: i64,
    pub auth_max_verify_attempts: i64,
    pub auth_email_hourly_limit: i64,
    pub auth_ip_hourly_limit: i64,
    pub auth_installation_hourly_limit: i64,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            api_addr: env::var("PORTR_RS_API_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:8787".to_string())
                .parse()
                .expect("invalid PORTR_RS_API_ADDR"),
            ssh_addr: env::var("PORTR_RS_SSH_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:2222".to_string())
                .parse()
                .expect("invalid PORTR_RS_SSH_ADDR"),
            tunnel_domain: env::var("PORTR_RS_TUNNEL_DOMAIN")
                .unwrap_or_else(|_| "0.0.0.0:8787".to_string()),
            ssh_public_addr: env::var("PORTR_RS_SSH_PUBLIC_ADDR").unwrap_or_default(),
            use_localhost: env::var("PORTR_RS_USE_LOCALHOST")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(true),
            lease_ttl_secs: env::var("PORTR_RS_LEASE_TTL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
            db_path: env::var("PORTR_RS_DB_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|_| default_db_path()),
            host_key_path: env::var("PORTR_RS_HOST_KEY_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|_| default_host_key_path()),
            cleanup_interval_secs: env::var("PORTR_RS_CLEANUP_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
            lease_retention_secs: env::var("PORTR_RS_LEASE_RETENTION_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(7 * 24 * 60 * 60),
            client_stale_secs: env::var("PORTR_RS_CLIENT_STALE_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60 * 60),
            resend_api_key: env::var("PORTR_RS_RESEND_API_KEY").ok(),
            resend_from: env::var("PORTR_RS_RESEND_FROM").ok(),
            resend_reply_to: env::var("PORTR_RS_RESEND_REPLY_TO").ok(),
            auth_code_ttl_secs: env::var("PORTR_RS_AUTH_CODE_TTL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5 * 60),
            auth_code_cooldown_secs: env::var("PORTR_RS_AUTH_CODE_COOLDOWN_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
            auth_session_ttl_secs: env::var("PORTR_RS_AUTH_SESSION_TTL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30 * 60),
            auth_refresh_ttl_secs: env::var("PORTR_RS_AUTH_REFRESH_TTL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30 * 24 * 60 * 60),
            auth_max_verify_attempts: env::var("PORTR_RS_AUTH_MAX_VERIFY_ATTEMPTS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            auth_email_hourly_limit: env::var("PORTR_RS_AUTH_EMAIL_HOURLY_LIMIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            auth_ip_hourly_limit: env::var("PORTR_RS_AUTH_IP_HOURLY_LIMIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20),
            auth_installation_hourly_limit: env::var("PORTR_RS_AUTH_INSTALLATION_HOURLY_LIMIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(10),
        }
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
}

pub fn default_env_path() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".config/portr-rs/.env"))
        .unwrap_or_else(|| PathBuf::from("./.env"))
}

pub fn ensure_default_env_file() -> Result<PathBuf> {
    let env_path = default_env_path();
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
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".config/portr-rs/portr-rs.db"))
        .unwrap_or_else(|| PathBuf::from("./data/portr-rs.db"))
}

fn default_host_key_path() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".config/portr-rs/ssh_host_ed25519_key"))
        .unwrap_or_else(|| PathBuf::from("./data/ssh_host_ed25519_key"))
}

fn default_env_contents() -> String {
    format!(
        "\
PORTR_RS_API_ADDR=0.0.0.0:8787
PORTR_RS_SSH_ADDR=0.0.0.0:2222
PORTR_RS_TUNNEL_DOMAIN=0.0.0.0:8787
PORTR_RS_USE_LOCALHOST=true
PORTR_RS_LEASE_TTL_SECS=60
PORTR_RS_DB_PATH={}
PORTR_RS_CLEANUP_INTERVAL_SECS=300
PORTR_RS_LEASE_RETENTION_SECS=604800
PORTR_RS_CLIENT_STALE_SECS=3600
PORTR_RS_AUTH_CODE_TTL_SECS=300
PORTR_RS_AUTH_CODE_COOLDOWN_SECS=60
PORTR_RS_AUTH_SESSION_TTL_SECS=1800
PORTR_RS_AUTH_REFRESH_TTL_SECS=2592000
PORTR_RS_AUTH_MAX_VERIFY_ATTEMPTS=5
PORTR_RS_AUTH_EMAIL_HOURLY_LIMIT=5
PORTR_RS_AUTH_IP_HOURLY_LIMIT=20
PORTR_RS_AUTH_INSTALLATION_HOURLY_LIMIT=10
",
        default_db_path().display()
    )
}
