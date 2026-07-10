use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use resend_rs::Resend;
use tokio::sync::{Mutex, RwLock, broadcast};

use crate::abuse::AbuseTracker;
use crate::admin::upgrade::SharedUpgradeRegistry;
use crate::board_telegram::TelegramNotifier;
use crate::config::Config;
use crate::dynamic_settings::DynamicSettings;
use crate::ip_blacklist_stats::IpBlacklistStats;
use crate::metrics::MetricsRegistry;
use crate::models::{ResendUsageResponse, ShareEditAvailableEvent};
use crate::proxy::ProxyRegistry;
use crate::recent_traffic::RecentTraffic;
use crate::scheduling_signals::OverrideStore;
use crate::store::AppStore;

#[derive(Clone)]
pub struct ServerState {
    pub config: Config,
    pub server_geo: ServerGeo,
    pub store: AppStore,
    pub proxy: Arc<ProxyRegistry>,
    /// Shared HTTP client for proxied tunnel traffic. It keeps connection pools
    /// bounded and avoids allocating a new client for every request.
    pub proxy_http: reqwest::Client,
    pub resend: Option<Arc<Resend>>,
    pub resend_usage_cache: Arc<Mutex<Option<ResendUsageCache>>>,
    pub dynamic: Arc<RwLock<DynamicSettings>>,
    /// SSH host key 指纹（`SHA256:<base64-nopad>` 格式），在 /lease 响应中回传给客户端。
    pub ssh_host_fingerprint: Option<String>,
    /// In-memory rolling tracker of proxy traffic by user origin. Drives the dashboard
    /// "demand" overlay and burst-arc animation; not persisted across restarts.
    pub recent_traffic: RecentTraffic,
    /// In-memory temporary ban tracker for repeated invalid API authentication.
    pub abuse: Arc<AbuseTracker>,
    /// In-memory aggregation for blocked IP blacklist requests. Flushed to logs periodically.
    pub ip_blacklist_stats: Arc<IpBlacklistStats>,
    /// Optional Telegram bot client; populated when telegram env vars are set.
    pub telegram: Arc<RwLock<Option<Arc<TelegramNotifier>>>>,
    /// Single-flight upgrade orchestrator with SSE log fan-out.
    pub upgrade_registry: SharedUpgradeRegistry,
    /// Fan-out control channel for online cc-switch clients. Events are wake-ups only;
    /// clients still pull signed pending edits before applying anything.
    pub share_edit_events: broadcast::Sender<ShareEditAvailableEvent>,
    /// Path to the live env file (also the apply target).
    pub env_path: PathBuf,
    /// When the process started; powers the uptime value on /v1/admin/version.
    pub start_instant: Instant,
    /// Per-owner penalty multipliers seeded by market 429/rate-limit feedback.
    /// Decays via TTL; never persisted.
    pub scheduling_overrides: Arc<OverrideStore>,
    /// Separate metrics collector/store for host, router, and LLM observability.
    pub metrics: Arc<MetricsRegistry>,
    /// Per-IP limiter for the public payout-profile lookup endpoints.
    pub payout_profile_read_limiter: Arc<PublicPayoutProfileReadLimiter>,
}

const PUBLIC_PAYOUT_READS_PER_MINUTE: u32 = 300;

#[derive(Debug, Default)]
pub struct PublicPayoutProfileReadLimiter {
    buckets: Mutex<HashMap<IpAddr, (i64, u32)>>,
}

impl PublicPayoutProfileReadLimiter {
    pub async fn allow(&self, ip: IpAddr) -> bool {
        let minute = chrono::Utc::now().timestamp().div_euclid(60);
        let mut buckets = self.buckets.lock().await;
        if buckets.len() > 4096 {
            buckets.retain(|_, (bucket, _)| *bucket >= minute - 1);
        }
        let entry = buckets.entry(ip).or_insert((minute, 0));
        if entry.0 != minute {
            *entry = (minute, 0);
        }
        if entry.1 >= PUBLIC_PAYOUT_READS_PER_MINUTE {
            return false;
        }
        entry.1 += 1;
        true
    }
}

#[derive(Debug, Clone)]
pub struct ResendUsageCache {
    pub fetched_at_unix_secs: i64,
    pub value: ResendUsageResponse,
}

#[derive(Debug, Clone)]
pub struct ServerGeo {
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}
