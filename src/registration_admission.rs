use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

const ENV_SOURCE_RATE: &str = "CC_SWITCH_ROUTER_REGISTRATION_SOURCE_RATE_PER_MINUTE";
const ENV_SOURCE_BURST: &str = "CC_SWITCH_ROUTER_REGISTRATION_SOURCE_BURST";
const ENV_GLOBAL_RATE: &str = "CC_SWITCH_ROUTER_REGISTRATION_GLOBAL_RATE_PER_MINUTE";
const ENV_GLOBAL_BURST: &str = "CC_SWITCH_ROUTER_REGISTRATION_GLOBAL_BURST";
const ENV_KEY_RATE: &str = "CC_SWITCH_ROUTER_REGISTRATION_KEY_RATE_PER_MINUTE";
const ENV_KEY_BURST: &str = "CC_SWITCH_ROUTER_REGISTRATION_KEY_BURST";
const ENV_BUCKET_IDLE_SECS: &str = "CC_SWITCH_ROUTER_REGISTRATION_BUCKET_IDLE_SECS";
const ENV_MAX_SOURCE_BUCKETS: &str = "CC_SWITCH_ROUTER_REGISTRATION_MAX_SOURCE_BUCKETS";
const ENV_MAX_KEY_BUCKETS: &str = "CC_SWITCH_ROUTER_REGISTRATION_MAX_KEY_BUCKETS";
const ENV_SOURCE_NEW_IDENTITY_10M_LIMIT: &str =
    "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_10M_LIMIT";
const ENV_SOURCE_NEW_IDENTITY_HOURLY_LIMIT: &str =
    "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_HOURLY_LIMIT";
const ENV_SOURCE_NEW_IDENTITY_DAILY_LIMIT: &str =
    "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_SOURCE_DAILY_LIMIT";
const ENV_GLOBAL_NEW_IDENTITY_10M_LIMIT: &str =
    "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_10M_LIMIT";
const ENV_GLOBAL_NEW_IDENTITY_HOURLY_LIMIT: &str =
    "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_HOURLY_LIMIT";
const ENV_GLOBAL_NEW_IDENTITY_DAILY_LIMIT: &str =
    "CC_SWITCH_ROUTER_REGISTRATION_NEW_IDENTITY_GLOBAL_DAILY_LIMIT";
const ENV_UNOWNED_INSTALLATION_WATERMARK: &str =
    "CC_SWITCH_ROUTER_REGISTRATION_UNOWNED_INSTALLATION_WATERMARK";

const DEFAULT_SOURCE_RATE_PER_MINUTE: u32 = 60;
const DEFAULT_SOURCE_BURST: u32 = 20;
const DEFAULT_GLOBAL_RATE_PER_MINUTE: u32 = 600;
const DEFAULT_GLOBAL_BURST: u32 = 200;
const DEFAULT_KEY_RATE_PER_MINUTE: u32 = 10;
const DEFAULT_KEY_BURST: u32 = 3;
const DEFAULT_BUCKET_IDLE_SECS: u64 = 10 * 60;
const DEFAULT_MAX_SOURCE_BUCKETS: usize = 8_192;
const DEFAULT_MAX_KEY_BUCKETS: usize = 16_384;
const DEFAULT_SOURCE_NEW_IDENTITY_10M_LIMIT: u32 = 30;
const DEFAULT_SOURCE_NEW_IDENTITY_HOURLY_LIMIT: u32 = 100;
const DEFAULT_SOURCE_NEW_IDENTITY_DAILY_LIMIT: u32 = 300;
const DEFAULT_GLOBAL_NEW_IDENTITY_10M_LIMIT: u32 = 300;
const DEFAULT_GLOBAL_NEW_IDENTITY_HOURLY_LIMIT: u32 = 1_000;
const DEFAULT_GLOBAL_NEW_IDENTITY_DAILY_LIMIT: u32 = 5_000;
const DEFAULT_UNOWNED_INSTALLATION_WATERMARK: u32 = 50_000;

const MAX_SOURCE_RATE_PER_MINUTE: u32 = 6_000;
const MAX_SOURCE_BURST: u32 = 1_000;
const MAX_GLOBAL_RATE_PER_MINUTE: u32 = 60_000;
const MAX_GLOBAL_BURST: u32 = 10_000;
const MAX_KEY_RATE_PER_MINUTE: u32 = 600;
const MAX_KEY_BURST: u32 = 100;
const MIN_BUCKET_IDLE_SECS: u64 = 30;
const MAX_BUCKET_IDLE_SECS: u64 = 24 * 60 * 60;
const MIN_SOURCE_BUCKETS: usize = 128;
const MAX_SOURCE_BUCKETS: usize = 65_536;
const MIN_KEY_BUCKETS: usize = 256;
const MAX_KEY_BUCKETS: usize = 131_072;
const MAX_NEW_IDENTITY_WINDOW_LIMIT: u32 = 1_000_000;
const MIN_UNOWNED_INSTALLATION_WATERMARK: u32 = 1_000;
const MAX_UNOWNED_INSTALLATION_WATERMARK: u32 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistrationQuotaWindow {
    pub duration: Duration,
    pub limit: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistrationAdmissionPolicy {
    pub source_rate_per_minute: u32,
    pub source_burst: u32,
    pub global_rate_per_minute: u32,
    pub global_burst: u32,
    pub key_rate_per_minute: u32,
    pub key_burst: u32,
    pub bucket_idle_secs: u64,
    pub max_source_buckets: usize,
    pub max_key_buckets: usize,
    pub source_new_identity_10m_limit: u32,
    pub source_new_identity_hourly_limit: u32,
    pub source_new_identity_daily_limit: u32,
    pub global_new_identity_10m_limit: u32,
    pub global_new_identity_hourly_limit: u32,
    pub global_new_identity_daily_limit: u32,
    pub unowned_installation_watermark: u32,
}

impl Default for RegistrationAdmissionPolicy {
    fn default() -> Self {
        Self {
            source_rate_per_minute: DEFAULT_SOURCE_RATE_PER_MINUTE,
            source_burst: DEFAULT_SOURCE_BURST,
            global_rate_per_minute: DEFAULT_GLOBAL_RATE_PER_MINUTE,
            global_burst: DEFAULT_GLOBAL_BURST,
            key_rate_per_minute: DEFAULT_KEY_RATE_PER_MINUTE,
            key_burst: DEFAULT_KEY_BURST,
            bucket_idle_secs: DEFAULT_BUCKET_IDLE_SECS,
            max_source_buckets: DEFAULT_MAX_SOURCE_BUCKETS,
            max_key_buckets: DEFAULT_MAX_KEY_BUCKETS,
            source_new_identity_10m_limit: DEFAULT_SOURCE_NEW_IDENTITY_10M_LIMIT,
            source_new_identity_hourly_limit: DEFAULT_SOURCE_NEW_IDENTITY_HOURLY_LIMIT,
            source_new_identity_daily_limit: DEFAULT_SOURCE_NEW_IDENTITY_DAILY_LIMIT,
            global_new_identity_10m_limit: DEFAULT_GLOBAL_NEW_IDENTITY_10M_LIMIT,
            global_new_identity_hourly_limit: DEFAULT_GLOBAL_NEW_IDENTITY_HOURLY_LIMIT,
            global_new_identity_daily_limit: DEFAULT_GLOBAL_NEW_IDENTITY_DAILY_LIMIT,
            unowned_installation_watermark: DEFAULT_UNOWNED_INSTALLATION_WATERMARK,
        }
    }
}

impl RegistrationAdmissionPolicy {
    pub fn from_env() -> Self {
        Self::from_env_reader(|key| std::env::var(key).ok())
    }

    fn from_env_reader(mut read: impl FnMut(&str) -> Option<String>) -> Self {
        let defaults = Self::default();
        let source_new_identity_10m_limit = read_bounded_u32(
            &mut read,
            ENV_SOURCE_NEW_IDENTITY_10M_LIMIT,
            defaults.source_new_identity_10m_limit,
            1,
            MAX_NEW_IDENTITY_WINDOW_LIMIT,
        );
        let source_new_identity_hourly_limit = read_bounded_u32(
            &mut read,
            ENV_SOURCE_NEW_IDENTITY_HOURLY_LIMIT,
            defaults.source_new_identity_hourly_limit,
            1,
            MAX_NEW_IDENTITY_WINDOW_LIMIT,
        )
        .max(source_new_identity_10m_limit);
        let source_new_identity_daily_limit = read_bounded_u32(
            &mut read,
            ENV_SOURCE_NEW_IDENTITY_DAILY_LIMIT,
            defaults.source_new_identity_daily_limit,
            1,
            MAX_NEW_IDENTITY_WINDOW_LIMIT,
        )
        .max(source_new_identity_hourly_limit);
        let global_new_identity_10m_limit = read_bounded_u32(
            &mut read,
            ENV_GLOBAL_NEW_IDENTITY_10M_LIMIT,
            defaults.global_new_identity_10m_limit,
            1,
            MAX_NEW_IDENTITY_WINDOW_LIMIT,
        );
        let global_new_identity_hourly_limit = read_bounded_u32(
            &mut read,
            ENV_GLOBAL_NEW_IDENTITY_HOURLY_LIMIT,
            defaults.global_new_identity_hourly_limit,
            1,
            MAX_NEW_IDENTITY_WINDOW_LIMIT,
        )
        .max(global_new_identity_10m_limit);
        let global_new_identity_daily_limit = read_bounded_u32(
            &mut read,
            ENV_GLOBAL_NEW_IDENTITY_DAILY_LIMIT,
            defaults.global_new_identity_daily_limit,
            1,
            MAX_NEW_IDENTITY_WINDOW_LIMIT,
        )
        .max(global_new_identity_hourly_limit);
        Self {
            source_rate_per_minute: read_bounded_u32(
                &mut read,
                ENV_SOURCE_RATE,
                defaults.source_rate_per_minute,
                1,
                MAX_SOURCE_RATE_PER_MINUTE,
            ),
            source_burst: read_bounded_u32(
                &mut read,
                ENV_SOURCE_BURST,
                defaults.source_burst,
                1,
                MAX_SOURCE_BURST,
            ),
            global_rate_per_minute: read_bounded_u32(
                &mut read,
                ENV_GLOBAL_RATE,
                defaults.global_rate_per_minute,
                1,
                MAX_GLOBAL_RATE_PER_MINUTE,
            ),
            global_burst: read_bounded_u32(
                &mut read,
                ENV_GLOBAL_BURST,
                defaults.global_burst,
                1,
                MAX_GLOBAL_BURST,
            ),
            key_rate_per_minute: read_bounded_u32(
                &mut read,
                ENV_KEY_RATE,
                defaults.key_rate_per_minute,
                1,
                MAX_KEY_RATE_PER_MINUTE,
            ),
            key_burst: read_bounded_u32(
                &mut read,
                ENV_KEY_BURST,
                defaults.key_burst,
                1,
                MAX_KEY_BURST,
            ),
            bucket_idle_secs: read_bounded_u64(
                &mut read,
                ENV_BUCKET_IDLE_SECS,
                defaults.bucket_idle_secs,
                MIN_BUCKET_IDLE_SECS,
                MAX_BUCKET_IDLE_SECS,
            ),
            max_source_buckets: read_bounded_usize(
                &mut read,
                ENV_MAX_SOURCE_BUCKETS,
                defaults.max_source_buckets,
                MIN_SOURCE_BUCKETS,
                MAX_SOURCE_BUCKETS,
            ),
            max_key_buckets: read_bounded_usize(
                &mut read,
                ENV_MAX_KEY_BUCKETS,
                defaults.max_key_buckets,
                MIN_KEY_BUCKETS,
                MAX_KEY_BUCKETS,
            ),
            source_new_identity_10m_limit,
            source_new_identity_hourly_limit,
            source_new_identity_daily_limit,
            global_new_identity_10m_limit,
            global_new_identity_hourly_limit,
            global_new_identity_daily_limit,
            unowned_installation_watermark: read_bounded_u32(
                &mut read,
                ENV_UNOWNED_INSTALLATION_WATERMARK,
                defaults.unowned_installation_watermark,
                MIN_UNOWNED_INSTALLATION_WATERMARK,
                MAX_UNOWNED_INSTALLATION_WATERMARK,
            ),
        }
    }

    pub fn source_new_identity_quotas(self) -> [RegistrationQuotaWindow; 3] {
        [
            RegistrationQuotaWindow {
                duration: Duration::from_secs(10 * 60),
                limit: self.source_new_identity_10m_limit,
            },
            RegistrationQuotaWindow {
                duration: Duration::from_secs(60 * 60),
                limit: self.source_new_identity_hourly_limit,
            },
            RegistrationQuotaWindow {
                duration: Duration::from_secs(24 * 60 * 60),
                limit: self.source_new_identity_daily_limit,
            },
        ]
    }

    pub fn global_new_identity_quotas(self) -> [RegistrationQuotaWindow; 3] {
        [
            RegistrationQuotaWindow {
                duration: Duration::from_secs(10 * 60),
                limit: self.global_new_identity_10m_limit,
            },
            RegistrationQuotaWindow {
                duration: Duration::from_secs(60 * 60),
                limit: self.global_new_identity_hourly_limit,
            },
            RegistrationQuotaWindow {
                duration: Duration::from_secs(24 * 60 * 60),
                limit: self.global_new_identity_daily_limit,
            },
        ]
    }

    fn normalized_for_runtime(self) -> Self {
        let source_new_identity_10m_limit = self
            .source_new_identity_10m_limit
            .clamp(1, MAX_NEW_IDENTITY_WINDOW_LIMIT);
        let source_new_identity_hourly_limit = self
            .source_new_identity_hourly_limit
            .clamp(1, MAX_NEW_IDENTITY_WINDOW_LIMIT)
            .max(source_new_identity_10m_limit);
        let source_new_identity_daily_limit = self
            .source_new_identity_daily_limit
            .clamp(1, MAX_NEW_IDENTITY_WINDOW_LIMIT)
            .max(source_new_identity_hourly_limit);
        let global_new_identity_10m_limit = self
            .global_new_identity_10m_limit
            .clamp(1, MAX_NEW_IDENTITY_WINDOW_LIMIT);
        let global_new_identity_hourly_limit = self
            .global_new_identity_hourly_limit
            .clamp(1, MAX_NEW_IDENTITY_WINDOW_LIMIT)
            .max(global_new_identity_10m_limit);
        let global_new_identity_daily_limit = self
            .global_new_identity_daily_limit
            .clamp(1, MAX_NEW_IDENTITY_WINDOW_LIMIT)
            .max(global_new_identity_hourly_limit);
        Self {
            source_rate_per_minute: self
                .source_rate_per_minute
                .clamp(1, MAX_SOURCE_RATE_PER_MINUTE),
            source_burst: self.source_burst.clamp(1, MAX_SOURCE_BURST),
            global_rate_per_minute: self
                .global_rate_per_minute
                .clamp(1, MAX_GLOBAL_RATE_PER_MINUTE),
            global_burst: self.global_burst.clamp(1, MAX_GLOBAL_BURST),
            key_rate_per_minute: self.key_rate_per_minute.clamp(1, MAX_KEY_RATE_PER_MINUTE),
            key_burst: self.key_burst.clamp(1, MAX_KEY_BURST),
            bucket_idle_secs: self.bucket_idle_secs.clamp(1, MAX_BUCKET_IDLE_SECS),
            max_source_buckets: self.max_source_buckets.clamp(1, MAX_SOURCE_BUCKETS),
            max_key_buckets: self.max_key_buckets.clamp(1, MAX_KEY_BUCKETS),
            source_new_identity_10m_limit,
            source_new_identity_hourly_limit,
            source_new_identity_daily_limit,
            global_new_identity_10m_limit,
            global_new_identity_hourly_limit,
            global_new_identity_daily_limit,
            unowned_installation_watermark: self.unowned_installation_watermark.clamp(
                MIN_UNOWNED_INSTALLATION_WATERMARK,
                MAX_UNOWNED_INSTALLATION_WATERMARK,
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationAdmissionLimit {
    Global,
    Source,
    PublicKey,
    SourceBucketCapacity,
    PublicKeyBucketCapacity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistrationAdmissionRejection {
    pub limit: RegistrationAdmissionLimit,
    pub retry_after_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistrationAdmissionSnapshot {
    pub source_buckets: usize,
    pub key_buckets: usize,
}

pub struct RegistrationAdmissionLimiter {
    policy: RegistrationAdmissionPolicy,
    state: Mutex<LimiterState>,
}

impl RegistrationAdmissionLimiter {
    pub fn new(policy: RegistrationAdmissionPolicy) -> Self {
        Self::new_at(policy, Instant::now())
    }

    pub fn from_env() -> Self {
        Self::new(RegistrationAdmissionPolicy::from_env())
    }

    pub fn policy(&self) -> RegistrationAdmissionPolicy {
        self.policy
    }

    pub fn check_attempt(
        &self,
        source: Option<&str>,
        public_key: &str,
    ) -> Result<(), RegistrationAdmissionRejection> {
        self.check_at(source, public_key, Instant::now())
    }

    pub fn snapshot(&self) -> RegistrationAdmissionSnapshot {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        RegistrationAdmissionSnapshot {
            source_buckets: state.source_buckets.len(),
            key_buckets: state.key_buckets.len(),
        }
    }

    fn new_at(policy: RegistrationAdmissionPolicy, now: Instant) -> Self {
        let policy = policy.normalized_for_runtime();
        Self {
            policy,
            state: Mutex::new(LimiterState::new(policy, now)),
        }
    }

    fn check_at(
        &self,
        source: Option<&str>,
        public_key: &str,
        now: Instant,
    ) -> Result<(), RegistrationAdmissionRejection> {
        let source_scope = SourceScope::from_trusted_metadata(source);
        let key_fingerprint = public_key_fingerprint(public_key);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        state
            .global
            .take(now)
            .map_err(|retry_after_secs| RegistrationAdmissionRejection {
                limit: RegistrationAdmissionLimit::Global,
                retry_after_secs,
            })?;
        state.cleanup_if_due(self.policy, now);

        if !state.source_buckets.contains_key(&source_scope)
            && state.source_buckets.len() >= self.policy.max_source_buckets
        {
            return Err(RegistrationAdmissionRejection {
                limit: RegistrationAdmissionLimit::SourceBucketCapacity,
                retry_after_secs: state.capacity_retry_after(self.policy, now),
            });
        }
        let source_bucket = state.source_buckets.entry(source_scope).or_insert_with(|| {
            BucketEntry::new(
                self.policy.source_rate_per_minute,
                self.policy.source_burst,
                now,
            )
        });
        source_bucket.last_seen = now;
        source_bucket.bucket.take(now).map_err(|retry_after_secs| {
            RegistrationAdmissionRejection {
                limit: RegistrationAdmissionLimit::Source,
                retry_after_secs,
            }
        })?;

        if !state.key_buckets.contains_key(&key_fingerprint)
            && state.key_buckets.len() >= self.policy.max_key_buckets
        {
            return Err(RegistrationAdmissionRejection {
                limit: RegistrationAdmissionLimit::PublicKeyBucketCapacity,
                retry_after_secs: state.capacity_retry_after(self.policy, now),
            });
        }
        let key_bucket = state.key_buckets.entry(key_fingerprint).or_insert_with(|| {
            BucketEntry::new(self.policy.key_rate_per_minute, self.policy.key_burst, now)
        });
        key_bucket.last_seen = now;
        key_bucket
            .bucket
            .take(now)
            .map_err(|retry_after_secs| RegistrationAdmissionRejection {
                limit: RegistrationAdmissionLimit::PublicKey,
                retry_after_secs,
            })
    }
}

struct LimiterState {
    global: TokenBucket,
    source_buckets: HashMap<SourceScope, BucketEntry>,
    key_buckets: HashMap<[u8; 32], BucketEntry>,
    last_cleanup: Instant,
}

impl LimiterState {
    fn new(policy: RegistrationAdmissionPolicy, now: Instant) -> Self {
        Self {
            global: TokenBucket::new(policy.global_rate_per_minute, policy.global_burst, now),
            source_buckets: HashMap::new(),
            key_buckets: HashMap::new(),
            last_cleanup: now,
        }
    }

    fn cleanup_if_due(&mut self, policy: RegistrationAdmissionPolicy, now: Instant) {
        let cleanup_interval = cleanup_interval(policy);
        let elapsed = now
            .checked_duration_since(self.last_cleanup)
            .unwrap_or_default();
        if elapsed < cleanup_interval {
            return;
        }
        let idle = Duration::from_secs(policy.bucket_idle_secs);
        self.source_buckets.retain(|_, entry| {
            now.checked_duration_since(entry.last_seen)
                .unwrap_or_default()
                < idle
        });
        self.key_buckets.retain(|_, entry| {
            now.checked_duration_since(entry.last_seen)
                .unwrap_or_default()
                < idle
        });
        self.last_cleanup = now;
    }

    fn capacity_retry_after(&self, policy: RegistrationAdmissionPolicy, now: Instant) -> u64 {
        let elapsed = now
            .checked_duration_since(self.last_cleanup)
            .unwrap_or_default();
        cleanup_interval(policy)
            .saturating_sub(elapsed)
            .as_secs()
            .max(1)
    }
}

struct BucketEntry {
    bucket: TokenBucket,
    last_seen: Instant,
}

impl BucketEntry {
    fn new(rate_per_minute: u32, burst: u32, now: Instant) -> Self {
        Self {
            bucket: TokenBucket::new(rate_per_minute, burst, now),
            last_seen: now,
        }
    }
}

struct TokenBucket {
    tokens: f64,
    capacity: f64,
    refill_per_second: f64,
    last_refill: Instant,
}

impl TokenBucket {
    fn new(rate_per_minute: u32, burst: u32, now: Instant) -> Self {
        let capacity = f64::from(burst.max(1));
        Self {
            tokens: capacity,
            capacity,
            refill_per_second: f64::from(rate_per_minute.max(1)) / 60.0,
            last_refill: now,
        }
    }

    fn take(&mut self, now: Instant) -> Result<(), u64> {
        let elapsed = now
            .checked_duration_since(self.last_refill)
            .unwrap_or_default()
            .as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_per_second).min(self.capacity);
        self.last_refill = now;
        if self.tokens >= 1.0 - 1e-9 {
            self.tokens = (self.tokens - 1.0).max(0.0);
            return Ok(());
        }
        let missing = 1.0 - self.tokens;
        Err((missing / self.refill_per_second).ceil().max(1.0) as u64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SourceScope {
    Ipv4(Ipv4Addr),
    Ipv6Prefix64([u8; 8]),
    Opaque([u8; 32]),
    Unknown,
}

impl SourceScope {
    fn from_trusted_metadata(source: Option<&str>) -> Self {
        let Some(source) = source.map(str::trim).filter(|value| !value.is_empty()) else {
            return Self::Unknown;
        };
        match source.parse::<IpAddr>() {
            Ok(IpAddr::V4(ip)) => Self::Ipv4(ip),
            Ok(IpAddr::V6(ip)) => {
                if let Some(ipv4) = ip.to_ipv4_mapped() {
                    return Self::Ipv4(ipv4);
                }
                let octets = ip.octets();
                let mut prefix = [0_u8; 8];
                prefix.copy_from_slice(&octets[..8]);
                Self::Ipv6Prefix64(prefix)
            }
            Err(_) => {
                let normalized = source.to_ascii_lowercase();
                Self::Opaque(sha256(normalized.as_bytes()))
            }
        }
    }

    fn durable_scope(self) -> String {
        let digest = match self {
            Self::Ipv4(ip) => sha256_parts(b"registration-source-ipv4-v1\0", &ip.octets()),
            Self::Ipv6Prefix64(prefix) => {
                sha256_parts(b"registration-source-ipv6-64-v1\0", &prefix)
            }
            Self::Opaque(value) => sha256_parts(b"registration-source-opaque-v1\0", &value),
            Self::Unknown => return "registration-source:v1:unknown".to_string(),
        };
        format!("registration-source:v1:{}", hex::encode(digest))
    }
}

pub fn registration_source_scope(source: Option<&str>) -> String {
    SourceScope::from_trusted_metadata(source).durable_scope()
}

fn public_key_fingerprint(public_key: &str) -> [u8; 32] {
    sha256(public_key.trim().as_bytes())
}

fn sha256(value: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(value);
    let mut output = [0_u8; 32];
    output.copy_from_slice(&digest);
    output
}

fn sha256_parts(domain: &[u8], value: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(value);
    let digest = digest.finalize();
    let mut output = [0_u8; 32];
    output.copy_from_slice(&digest);
    output
}

fn cleanup_interval(policy: RegistrationAdmissionPolicy) -> Duration {
    Duration::from_secs((policy.bucket_idle_secs / 2).clamp(1, 60))
}

fn read_bounded_u32(
    read: &mut impl FnMut(&str) -> Option<String>,
    key: &str,
    default: u32,
    min: u32,
    max: u32,
) -> u32 {
    read(key)
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|value| value.clamp(u64::from(min), u64::from(max)) as u32)
        .unwrap_or(default)
}

fn read_bounded_u64(
    read: &mut impl FnMut(&str) -> Option<String>,
    key: &str,
    default: u64,
    min: u64,
    max: u64,
) -> u64 {
    read(key)
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|value| value.clamp(min, max))
        .unwrap_or(default)
}

fn read_bounded_usize(
    read: &mut impl FnMut(&str) -> Option<String>,
    key: &str,
    default: usize,
    min: usize,
    max: usize,
) -> usize {
    read(key)
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|value| value.clamp(min as u64, max as u64) as usize)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_policy() -> RegistrationAdmissionPolicy {
        RegistrationAdmissionPolicy {
            source_rate_per_minute: 600,
            source_burst: 100,
            global_rate_per_minute: 600,
            global_burst: 100,
            key_rate_per_minute: 600,
            key_burst: 10,
            bucket_idle_secs: 60,
            max_source_buckets: 10,
            max_key_buckets: 10,
            source_new_identity_10m_limit: 30,
            source_new_identity_hourly_limit: 100,
            source_new_identity_daily_limit: 300,
            global_new_identity_10m_limit: 300,
            global_new_identity_hourly_limit: 1_000,
            global_new_identity_daily_limit: 5_000,
            unowned_installation_watermark: 50_000,
        }
    }

    #[test]
    fn token_bucket_enforces_burst_and_refills() {
        let start = Instant::now();
        let mut bucket = TokenBucket::new(10, 3, start);
        assert_eq!(bucket.take(start), Ok(()));
        assert_eq!(bucket.take(start), Ok(()));
        assert_eq!(bucket.take(start), Ok(()));
        assert_eq!(bucket.take(start), Err(6));
        assert_eq!(bucket.take(start + Duration::from_secs(6)), Ok(()));
    }

    #[test]
    fn ipv6_addresses_share_a_64_bit_source_scope() {
        let first = SourceScope::from_trusted_metadata(Some("2001:db8:abcd:42::1"));
        let second = SourceScope::from_trusted_metadata(Some("2001:db8:abcd:42:ffff::99"));
        let different = SourceScope::from_trusted_metadata(Some("2001:db8:abcd:43::1"));
        assert_eq!(first, second);
        assert_ne!(first, different);
        assert_ne!(
            SourceScope::from_trusted_metadata(Some("192.0.2.1")),
            SourceScope::from_trusted_metadata(Some("192.0.2.2"))
        );
    }

    #[test]
    fn bucket_map_capacity_fails_closed_and_global_bucket_still_applies() {
        let start = Instant::now();
        let mut policy = test_policy();
        policy.global_rate_per_minute = 1;
        policy.global_burst = 4;
        policy.max_key_buckets = 2;
        let limiter = RegistrationAdmissionLimiter::new_at(policy, start);

        assert_eq!(limiter.check_at(Some("192.0.2.1"), "key-1", start), Ok(()));
        assert_eq!(limiter.check_at(Some("192.0.2.1"), "key-2", start), Ok(()));
        assert_eq!(
            limiter
                .check_at(Some("192.0.2.1"), "key-3", start)
                .unwrap_err()
                .limit,
            RegistrationAdmissionLimit::PublicKeyBucketCapacity
        );
        assert_eq!(
            limiter
                .check_at(Some("192.0.2.1"), "key-4", start)
                .unwrap_err()
                .limit,
            RegistrationAdmissionLimit::PublicKeyBucketCapacity
        );
        assert_eq!(
            limiter
                .check_at(Some("192.0.2.1"), "key-5", start)
                .unwrap_err()
                .limit,
            RegistrationAdmissionLimit::Global
        );
        assert_eq!(limiter.snapshot().key_buckets, 2);
    }

    #[test]
    fn idle_cleanup_releases_bucket_capacity() {
        let start = Instant::now();
        let mut policy = test_policy();
        policy.max_key_buckets = 1;
        let limiter = RegistrationAdmissionLimiter::new_at(policy, start);
        assert_eq!(limiter.check_at(Some("192.0.2.1"), "key-1", start), Ok(()));
        assert_eq!(
            limiter.check_at(Some("192.0.2.1"), "key-2", start + Duration::from_secs(61)),
            Ok(())
        );
        assert_eq!(limiter.snapshot().key_buckets, 1);
    }

    #[test]
    fn environment_values_are_parsed_and_safely_clamped() {
        let values = HashMap::from([
            (ENV_SOURCE_RATE, "0"),
            (ENV_SOURCE_BURST, "999999"),
            (ENV_GLOBAL_RATE, "invalid"),
            (ENV_KEY_RATE, "999999"),
            (ENV_BUCKET_IDLE_SECS, "1"),
            (ENV_MAX_SOURCE_BUCKETS, "1"),
            (ENV_MAX_KEY_BUCKETS, "999999999"),
            (ENV_SOURCE_NEW_IDENTITY_10M_LIMIT, "200"),
            (ENV_SOURCE_NEW_IDENTITY_HOURLY_LIMIT, "20"),
            (ENV_SOURCE_NEW_IDENTITY_DAILY_LIMIT, "2"),
            (ENV_GLOBAL_NEW_IDENTITY_10M_LIMIT, "5000"),
            (ENV_GLOBAL_NEW_IDENTITY_HOURLY_LIMIT, "500"),
            (ENV_GLOBAL_NEW_IDENTITY_DAILY_LIMIT, "50"),
            (ENV_UNOWNED_INSTALLATION_WATERMARK, "1"),
        ]);
        let policy = RegistrationAdmissionPolicy::from_env_reader(|key| {
            values.get(key).map(|value| (*value).to_string())
        });

        assert_eq!(policy.source_rate_per_minute, 1);
        assert_eq!(policy.source_burst, MAX_SOURCE_BURST);
        assert_eq!(
            policy.global_rate_per_minute,
            DEFAULT_GLOBAL_RATE_PER_MINUTE
        );
        assert_eq!(policy.key_rate_per_minute, MAX_KEY_RATE_PER_MINUTE);
        assert_eq!(policy.bucket_idle_secs, MIN_BUCKET_IDLE_SECS);
        assert_eq!(policy.max_source_buckets, MIN_SOURCE_BUCKETS);
        assert_eq!(policy.max_key_buckets, MAX_KEY_BUCKETS);
        assert_eq!(
            (
                policy.source_new_identity_10m_limit,
                policy.source_new_identity_hourly_limit,
                policy.source_new_identity_daily_limit,
            ),
            (200, 200, 200)
        );
        assert_eq!(
            (
                policy.global_new_identity_10m_limit,
                policy.global_new_identity_hourly_limit,
                policy.global_new_identity_daily_limit,
            ),
            (5_000, 5_000, 5_000)
        );
        assert_eq!(
            policy.unowned_installation_watermark,
            MIN_UNOWNED_INSTALLATION_WATERMARK
        );
    }

    #[test]
    fn persistent_quota_windows_are_exposed_in_store_order() {
        let policy = RegistrationAdmissionPolicy::default();
        assert_eq!(
            policy.source_new_identity_quotas(),
            [
                RegistrationQuotaWindow {
                    duration: Duration::from_secs(600),
                    limit: 30,
                },
                RegistrationQuotaWindow {
                    duration: Duration::from_secs(3_600),
                    limit: 100,
                },
                RegistrationQuotaWindow {
                    duration: Duration::from_secs(86_400),
                    limit: 300,
                },
            ]
        );
        assert_eq!(
            policy.global_new_identity_quotas(),
            [
                RegistrationQuotaWindow {
                    duration: Duration::from_secs(600),
                    limit: 300,
                },
                RegistrationQuotaWindow {
                    duration: Duration::from_secs(3_600),
                    limit: 1_000,
                },
                RegistrationQuotaWindow {
                    duration: Duration::from_secs(86_400),
                    limit: 5_000,
                },
            ]
        );
    }

    #[test]
    fn public_keys_are_stored_only_as_sha256_fingerprints() {
        let fingerprint = public_key_fingerprint("  secret-public-key  ");
        assert_eq!(
            hex::encode(fingerprint),
            "ba26d16e7385abe3e372fb1e73725149616a7a5253eb73bb88061a92f2b19a66"
        );

        let start = Instant::now();
        let limiter = RegistrationAdmissionLimiter::new_at(test_policy(), start);
        limiter
            .check_at(Some("opaque-source"), "secret-public-key", start)
            .unwrap();
        let state = limiter.state.lock().unwrap();
        assert!(state.key_buckets.contains_key(&fingerprint));
    }

    #[test]
    fn invalid_sources_are_normalized_then_hashed_and_empty_is_unknown() {
        assert_eq!(
            SourceScope::from_trusted_metadata(Some(" OPAQUE-SOURCE ")),
            SourceScope::Opaque([
                0x96, 0x03, 0xc7, 0x36, 0xd3, 0x2d, 0x1b, 0x66, 0x49, 0x79, 0xfb, 0xdd, 0x7d, 0x54,
                0xa7, 0x64, 0x14, 0x6c, 0xe1, 0x13, 0x02, 0x7a, 0xb2, 0x84, 0x3a, 0x02, 0xb0, 0x77,
                0xb7, 0xdf, 0xe2, 0xe7,
            ])
        );
        assert_eq!(
            SourceScope::from_trusted_metadata(Some("  ")),
            SourceScope::Unknown
        );
        assert_eq!(
            SourceScope::from_trusted_metadata(None),
            SourceScope::Unknown
        );
    }

    #[test]
    fn durable_source_scope_is_stable_private_and_ipv6_prefix_aware() {
        let first = registration_source_scope(Some("2001:db8:abcd:42::1"));
        let same_prefix = registration_source_scope(Some("2001:db8:abcd:42:ffff::99"));
        let different_prefix = registration_source_scope(Some("2001:db8:abcd:43::1"));
        assert_eq!(first, same_prefix);
        assert_ne!(first, different_prefix);
        assert!(!first.contains("2001:db8"));

        let ipv4 = registration_source_scope(Some("192.0.2.10"));
        assert!(!ipv4.contains("192.0.2.10"));
        assert_eq!(
            registration_source_scope(Some(" OPAQUE-SOURCE ")),
            registration_source_scope(Some("opaque-source"))
        );
        assert_eq!(
            registration_source_scope(None),
            "registration-source:v1:unknown"
        );
    }
}
