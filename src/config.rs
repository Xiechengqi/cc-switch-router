use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub api_addr: SocketAddr,
    pub ssh_addr: SocketAddr,
    pub tunnel_domain: String,
    pub use_localhost: bool,
    pub lease_ttl_secs: i64,
    pub db_path: PathBuf,
    pub admin_token: String,
    pub cleanup_interval_secs: u64,
    pub lease_retention_secs: i64,
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
            admin_token: env::var("PORTR_RS_ADMIN_TOKEN")
                .unwrap_or_else(|_| "change-me-admin-token".to_string()),
            cleanup_interval_secs: env::var("PORTR_RS_CLEANUP_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
            lease_retention_secs: env::var("PORTR_RS_LEASE_RETENTION_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(7 * 24 * 60 * 60),
        }
    }

    pub fn tunnel_url(&self, subdomain: &str) -> String {
        let scheme = if self.use_localhost { "http" } else { "https" };
        format!("{scheme}://{subdomain}.{}", self.tunnel_domain)
    }
}

fn default_db_path() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".config/portr-rs/portr-rs.db"))
        .unwrap_or_else(|| PathBuf::from("./data/portr-rs.db"))
}
