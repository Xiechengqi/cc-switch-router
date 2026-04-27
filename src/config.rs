use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const APP_NAME: &str = "cc-switch-router";
const LEGACY_APP_NAME: &str = "portr-rs";

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
    pub free_share_ip_parallel_limit: i64,
    pub verification_service_base_url: String,
    pub verification_service_api_key: Option<String>,
    pub markets: Vec<MarketConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketConfig {
    pub id: String,
    pub display_name: String,
    pub email: String,
    pub subdomain: String,
    pub public_base_url: String,
    #[serde(default)]
    pub token_hashes: Vec<MarketTokenHash>,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketTokenHash {
    pub hash: String,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicMarketConfig {
    pub id: String,
    pub display_name: String,
    pub email: String,
    pub subdomain: String,
    pub public_base_url: String,
    pub status: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            api_addr: env_var("CC_SWITCH_ROUTER_API_ADDR", "PORTR_RS_API_ADDR")
                .unwrap_or_else(|| "0.0.0.0:8787".to_string())
                .parse()
                .expect("invalid CC_SWITCH_ROUTER_API_ADDR/PORTR_RS_API_ADDR"),
            ssh_addr: env_var("CC_SWITCH_ROUTER_SSH_ADDR", "PORTR_RS_SSH_ADDR")
                .unwrap_or_else(|| "0.0.0.0:2222".to_string())
                .parse()
                .expect("invalid CC_SWITCH_ROUTER_SSH_ADDR/PORTR_RS_SSH_ADDR"),
            tunnel_domain: env_var("CC_SWITCH_ROUTER_TUNNEL_DOMAIN", "PORTR_RS_TUNNEL_DOMAIN")
                .unwrap_or_else(|| "0.0.0.0:8787".to_string()),
            ssh_public_addr: env_var(
                "CC_SWITCH_ROUTER_SSH_PUBLIC_ADDR",
                "PORTR_RS_SSH_PUBLIC_ADDR",
            )
            .unwrap_or_default(),
            use_localhost: env_var("CC_SWITCH_ROUTER_USE_LOCALHOST", "PORTR_RS_USE_LOCALHOST")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(true),
            lease_ttl_secs: env_var("CC_SWITCH_ROUTER_LEASE_TTL_SECS", "PORTR_RS_LEASE_TTL_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
            db_path: env_var("CC_SWITCH_ROUTER_DB_PATH", "PORTR_RS_DB_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(default_db_path),
            host_key_path: env_var("CC_SWITCH_ROUTER_HOST_KEY_PATH", "PORTR_RS_HOST_KEY_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(default_host_key_path),
            cleanup_interval_secs: env_var(
                "CC_SWITCH_ROUTER_CLEANUP_INTERVAL_SECS",
                "PORTR_RS_CLEANUP_INTERVAL_SECS",
            )
            .and_then(|v| v.parse().ok())
            .unwrap_or(300),
            lease_retention_secs: env_var(
                "CC_SWITCH_ROUTER_LEASE_RETENTION_SECS",
                "PORTR_RS_LEASE_RETENTION_SECS",
            )
            .and_then(|v| v.parse().ok())
            .unwrap_or(7 * 24 * 60 * 60),
            client_stale_secs: env_var(
                "CC_SWITCH_ROUTER_CLIENT_STALE_SECS",
                "PORTR_RS_CLIENT_STALE_SECS",
            )
            .and_then(|v| v.parse().ok())
            .unwrap_or(60 * 60),
            resend_api_key: env_var("CC_SWITCH_ROUTER_RESEND_API_KEY", "PORTR_RS_RESEND_API_KEY"),
            resend_from: env_var("CC_SWITCH_ROUTER_RESEND_FROM", "PORTR_RS_RESEND_FROM"),
            resend_reply_to: env_var(
                "CC_SWITCH_ROUTER_RESEND_REPLY_TO",
                "PORTR_RS_RESEND_REPLY_TO",
            ),
            auth_code_ttl_secs: env_var(
                "CC_SWITCH_ROUTER_AUTH_CODE_TTL_SECS",
                "PORTR_RS_AUTH_CODE_TTL_SECS",
            )
            .and_then(|v| v.parse().ok())
            .unwrap_or(5 * 60),
            auth_code_cooldown_secs: env_var(
                "CC_SWITCH_ROUTER_AUTH_CODE_COOLDOWN_SECS",
                "PORTR_RS_AUTH_CODE_COOLDOWN_SECS",
            )
            .and_then(|v| v.parse().ok())
            .unwrap_or(60),
            auth_session_ttl_secs: env_var(
                "CC_SWITCH_ROUTER_AUTH_SESSION_TTL_SECS",
                "PORTR_RS_AUTH_SESSION_TTL_SECS",
            )
            .and_then(|v| v.parse().ok())
            .unwrap_or(30 * 60),
            auth_refresh_ttl_secs: env_var(
                "CC_SWITCH_ROUTER_AUTH_REFRESH_TTL_SECS",
                "PORTR_RS_AUTH_REFRESH_TTL_SECS",
            )
            .and_then(|v| v.parse().ok())
            .unwrap_or(30 * 24 * 60 * 60),
            auth_max_verify_attempts: env_var(
                "CC_SWITCH_ROUTER_AUTH_MAX_VERIFY_ATTEMPTS",
                "PORTR_RS_AUTH_MAX_VERIFY_ATTEMPTS",
            )
            .and_then(|v| v.parse().ok())
            .unwrap_or(5),
            auth_email_hourly_limit: env_var(
                "CC_SWITCH_ROUTER_AUTH_EMAIL_HOURLY_LIMIT",
                "PORTR_RS_AUTH_EMAIL_HOURLY_LIMIT",
            )
            .and_then(|v| v.parse().ok())
            .unwrap_or(5),
            auth_ip_hourly_limit: env_var(
                "CC_SWITCH_ROUTER_AUTH_IP_HOURLY_LIMIT",
                "PORTR_RS_AUTH_IP_HOURLY_LIMIT",
            )
            .and_then(|v| v.parse().ok())
            .unwrap_or(20),
            auth_installation_hourly_limit: env_var(
                "CC_SWITCH_ROUTER_AUTH_INSTALLATION_HOURLY_LIMIT",
                "PORTR_RS_AUTH_INSTALLATION_HOURLY_LIMIT",
            )
            .and_then(|v| v.parse().ok())
            .unwrap_or(10),
            free_share_ip_parallel_limit: env_var(
                "CC_SWITCH_ROUTER_FREE_SHARE_IP_PARALLEL_LIMIT",
                "PORTR_RS_FREE_SHARE_IP_PARALLEL_LIMIT",
            )
            .and_then(|v| v.parse().ok())
            .unwrap_or(1),
            verification_service_base_url: env_var(
                "CC_SWITCH_ROUTER_VERIFICATION_SERVICE_BASE_URL",
                "PORTR_RS_VERIFICATION_SERVICE_BASE_URL",
            )
            .unwrap_or_else(|| "https://tokenswitch.org".to_string()),
            verification_service_api_key: env_var(
                "CC_SWITCH_ROUTER_VERIFICATION_SERVICE_API_KEY",
                "PORTR_RS_VERIFICATION_SERVICE_API_KEY",
            ),
            markets: parse_markets_env(),
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

    pub fn free_share_ip_limit_enabled(&self) -> bool {
        self.free_share_ip_parallel_limit > 0
    }

    pub fn public_markets(&self) -> Vec<PublicMarketConfig> {
        self.markets
            .iter()
            .filter(|market| market.status.eq_ignore_ascii_case("active"))
            .map(|market| PublicMarketConfig {
                id: market.id.clone(),
                display_name: market.display_name.clone(),
                email: market.email.clone(),
                subdomain: market.subdomain.clone(),
                public_base_url: market.public_base_url.clone(),
                status: market.status.clone(),
            })
            .collect()
    }

    pub fn market_by_token(&self, token: &str, required_scope: &str) -> Option<&MarketConfig> {
        if token.trim().is_empty() {
            return None;
        }
        let digest = Sha256::digest(token.as_bytes());
        let token_hash_hex = format!("sha256:{}", bytes_hex(&digest));
        let token_hash_b64 = format!("sha256:{}", general_purpose::STANDARD.encode(digest));
        let now = Utc::now();
        self.markets.iter().find(|market| {
            market.status.eq_ignore_ascii_case("active")
                && market.scopes.iter().any(|scope| scope == required_scope)
                && market.token_hashes.iter().any(|candidate| {
                    let configured_hash = candidate.hash.trim();
                    (constant_time_eq(configured_hash, &token_hash_hex)
                        || constant_time_eq(configured_hash, &token_hash_b64))
                        && candidate
                            .expires_at
                            .map(|expires| expires > now)
                            .unwrap_or(true)
                })
        })
    }

    pub fn is_market_subdomain(&self, subdomain: &str) -> bool {
        self.markets.iter().any(|market| {
            market.status.eq_ignore_ascii_case("active") && market.subdomain == subdomain
        })
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
    let preferred = path_in_home(APP_NAME, &format!("{APP_NAME}.db"));
    if let Some(path) = preferred.as_ref().filter(|path| path.exists()).cloned() {
        return path;
    }
    legacy_path_in_home(&format!("{LEGACY_APP_NAME}.db"))
        .or(preferred)
        .unwrap_or_else(|| PathBuf::from(format!("./data/{APP_NAME}.db")))
}

fn default_host_key_path() -> PathBuf {
    let preferred = path_in_home(APP_NAME, "ssh_host_ed25519_key");
    if let Some(path) = preferred.as_ref().filter(|path| path.exists()).cloned() {
        return path;
    }
    legacy_path_in_home("ssh_host_ed25519_key")
        .or(preferred)
        .unwrap_or_else(|| PathBuf::from("./data/ssh_host_ed25519_key"))
}

fn default_env_contents() -> String {
    format!(
        "\
CC_SWITCH_ROUTER_API_ADDR=0.0.0.0:8787
CC_SWITCH_ROUTER_SSH_ADDR=0.0.0.0:2222
CC_SWITCH_ROUTER_TUNNEL_DOMAIN=0.0.0.0:8787
CC_SWITCH_ROUTER_USE_LOCALHOST=true
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
CC_SWITCH_ROUTER_AUTH_EMAIL_HOURLY_LIMIT=5
CC_SWITCH_ROUTER_AUTH_IP_HOURLY_LIMIT=20
CC_SWITCH_ROUTER_AUTH_INSTALLATION_HOURLY_LIMIT=10
CC_SWITCH_ROUTER_FREE_SHARE_IP_PARALLEL_LIMIT=1
CC_SWITCH_ROUTER_MARKETS=[]
",
        default_db_path().display()
    )
}

fn env_var(primary: &str, legacy: &str) -> Option<String> {
    env::var(primary).ok().or_else(|| env::var(legacy).ok())
}

fn path_in_home(app_name: &str, leaf: &str) -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".config").join(app_name).join(leaf))
}

fn legacy_path_in_home(leaf: &str) -> Option<PathBuf> {
    path_in_home(LEGACY_APP_NAME, leaf).filter(|path| path.exists())
}

fn existing_env_path() -> Option<PathBuf> {
    let default_path = default_env_path();
    if default_path.exists() {
        return Some(default_path);
    }
    legacy_path_in_home(".env")
}

fn parse_markets_env() -> Vec<MarketConfig> {
    let raw = env::var("CC_SWITCH_ROUTER_MARKETS").unwrap_or_else(|_| "[]".to_string());
    serde_json::from_str(&raw).expect("invalid CC_SWITCH_ROUTER_MARKETS JSON")
}

fn sha256_hex(input: &[u8]) -> String {
    let digest = Sha256::digest(input);
    bytes_hex(&digest)
}

fn bytes_hex(input: &[u8]) -> String {
    let mut output = String::with_capacity(input.len() * 2);
    for byte in input {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right.iter())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
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
            resend_reply_to: None,
            auth_code_ttl_secs: 300,
            auth_code_cooldown_secs: 60,
            auth_session_ttl_secs: 300,
            auth_refresh_ttl_secs: 300,
            auth_max_verify_attempts: 5,
            auth_email_hourly_limit: 5,
            auth_ip_hourly_limit: 5,
            auth_installation_hourly_limit: 5,
            free_share_ip_parallel_limit: 1,
            verification_service_base_url: "https://example.com".into(),
            verification_service_api_key: None,
            markets: Vec::new(),
        };

        assert!(config.free_share_ip_limit_enabled());

        let disabled = Config {
            free_share_ip_parallel_limit: 0,
            ..config
        };
        assert!(!disabled.free_share_ip_limit_enabled());
    }

    #[test]
    fn market_by_token_requires_hash_scope_and_active_status() {
        let mut config = Config {
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
            resend_reply_to: None,
            auth_code_ttl_secs: 300,
            auth_code_cooldown_secs: 60,
            auth_session_ttl_secs: 300,
            auth_refresh_ttl_secs: 300,
            auth_max_verify_attempts: 5,
            auth_email_hourly_limit: 5,
            auth_ip_hourly_limit: 5,
            auth_installation_hourly_limit: 5,
            free_share_ip_parallel_limit: 1,
            verification_service_base_url: "https://example.com".into(),
            verification_service_api_key: None,
            markets: vec![MarketConfig {
                id: "main-market".into(),
                display_name: "Main Market".into(),
                email: "market@example.com".into(),
                subdomain: "market-a".into(),
                public_base_url: "https://market-a.example.com".into(),
                token_hashes: vec![
                    MarketTokenHash {
                        hash: format!("sha256:{}", sha256_hex(b"csmr_live_test")),
                        created_at: None,
                        expires_at: None,
                    },
                    MarketTokenHash {
                        hash: format!(
                            "sha256:{}",
                            general_purpose::STANDARD.encode(Sha256::digest(b"csmr_live_b64"))
                        ),
                        created_at: None,
                        expires_at: None,
                    },
                ],
                scopes: vec!["market:shares:read".into()],
                status: "active".into(),
            }],
        };

        assert!(
            config
                .market_by_token("csmr_live_test", "market:shares:read")
                .is_some()
        );
        assert!(
            config
                .market_by_token("csmr_live_b64", "market:shares:read")
                .is_some()
        );
        assert!(
            config
                .market_by_token("csmr_live_test", "market:proxy:use")
                .is_none()
        );
        config.markets[0].status = "disabled".into();
        assert!(
            config
                .market_by_token("csmr_live_test", "market:shares:read")
                .is_none()
        );
    }
}
