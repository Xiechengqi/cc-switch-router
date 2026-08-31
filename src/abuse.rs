use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::Mutex;

const INVALID_AUTH_WINDOW: Duration = Duration::from_secs(10 * 60);
const INVALID_AUTH_LIMIT: usize = 10;
const BAN_DURATION: Duration = Duration::from_secs(60 * 60);
const MAX_TRACKED_IPS: usize = 8_192;
const MAX_IDLE_DURATION: Duration = Duration::from_secs(70 * 60);
const SHARE_ABUSE_WINDOW: Duration = Duration::from_secs(10 * 60);
const SHARE_ABUSE_LIMIT: usize = 10;
const SHARE_BAN_DURATION: chrono::Duration = chrono::Duration::hours(1);
const MAX_TRACKED_SHARE_IPS: usize = 32_768;

#[derive(Debug, Clone, Copy)]
pub struct BanDecision {
    pub failures: usize,
    pub ban_duration: Duration,
}

#[derive(Debug)]
struct IpAbuseState {
    invalid_auth_at: VecDeque<Instant>,
    banned_until: Option<Instant>,
    last_seen_at: Instant,
}

impl Default for IpAbuseState {
    fn default() -> Self {
        Self {
            invalid_auth_at: VecDeque::new(),
            banned_until: None,
            last_seen_at: Instant::now(),
        }
    }
}

#[derive(Debug, Default)]
pub struct AbuseTracker {
    by_ip: Mutex<HashMap<String, IpAbuseState>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareClientBan {
    pub id: String,
    pub share_id: String,
    pub client_ip: String,
    pub reason_code: String,
    pub failure_count: usize,
    pub first_failure_at: DateTime<Utc>,
    pub last_failure_at: DateTime<Utc>,
    pub banned_at: DateTime<Utc>,
    pub banned_until: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareClientBanPage {
    pub items: Vec<ShareClientBan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareClientUnbanResponse {
    pub ok: bool,
    pub ban_id: String,
    pub already_unbanned: bool,
}

#[derive(Debug, Clone)]
pub struct ShareBanDecision {
    pub share_id: String,
    pub client_ip: String,
    pub reason_code: String,
    pub failure_count: usize,
    pub first_failure_at: DateTime<Utc>,
    pub last_failure_at: DateTime<Utc>,
    pub banned_at: DateTime<Utc>,
    pub banned_until: DateTime<Utc>,
}

#[derive(Debug)]
struct ShareFailureState {
    failures: VecDeque<(Instant, DateTime<Utc>)>,
    last_seen_at: Instant,
}

impl Default for ShareFailureState {
    fn default() -> Self {
        Self {
            failures: VecDeque::new(),
            last_seen_at: Instant::now(),
        }
    }
}

/// Share-scoped abuse state. Failure windows stay in memory, while active bans
/// are persisted by `AppStore` and loaded back into this cache at startup.
#[derive(Debug, Default)]
pub struct ShareAbuseTracker {
    failures: Mutex<HashMap<(String, String), ShareFailureState>>,
    active_bans: Mutex<HashMap<(String, String), ShareClientBan>>,
}

impl AbuseTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn ban_remaining(&self, ip: &str) -> Option<Duration> {
        let now = Instant::now();
        let mut by_ip = self.by_ip.lock().await;
        let state = by_ip.get_mut(ip)?;
        state.last_seen_at = now;
        match state.banned_until {
            Some(until) if until > now => Some(until.saturating_duration_since(now)),
            Some(_) => {
                state.banned_until = None;
                None
            }
            None => None,
        }
    }

    pub async fn record_invalid_auth(&self, ip: &str) -> Option<BanDecision> {
        let now = Instant::now();
        let mut by_ip = self.by_ip.lock().await;
        if !by_ip.contains_key(ip) && by_ip.len() >= MAX_TRACKED_IPS {
            prune_idle_states(&mut by_ip, now);
            if by_ip.len() >= MAX_TRACKED_IPS {
                evict_oldest_state(&mut by_ip);
            }
        }
        let state = by_ip.entry(ip.to_string()).or_default();
        state.last_seen_at = now;
        prune_old_failures(&mut state.invalid_auth_at, now);
        state.invalid_auth_at.push_back(now);
        let failures = state.invalid_auth_at.len();
        if failures >= INVALID_AUTH_LIMIT {
            state.banned_until = Some(now + BAN_DURATION);
            state.invalid_auth_at.clear();
            return Some(BanDecision {
                failures,
                ban_duration: BAN_DURATION,
            });
        }
        None
    }
}

impl ShareAbuseTracker {
    pub fn new(active_bans: impl IntoIterator<Item = ShareClientBan>) -> Self {
        Self {
            failures: Mutex::new(HashMap::new()),
            active_bans: Mutex::new(
                active_bans
                    .into_iter()
                    .map(|ban| ((ban.share_id.clone(), ban.client_ip.clone()), ban))
                    .collect(),
            ),
        }
    }

    pub async fn ban_remaining(&self, share_id: &str, client_ip: &str) -> Option<Duration> {
        let now = Utc::now();
        let key = (share_id.to_string(), client_ip.to_string());
        let mut active_bans = self.active_bans.lock().await;
        let ban = active_bans.get(&key)?;
        if ban.banned_until <= now {
            active_bans.remove(&key);
            return None;
        }
        (ban.banned_until - now).to_std().ok()
    }

    pub async fn record_violation(
        &self,
        share_id: &str,
        client_ip: &str,
        reason_code: &str,
    ) -> Option<ShareBanDecision> {
        if self.ban_remaining(share_id, client_ip).await.is_some() {
            return None;
        }
        let now_instant = Instant::now();
        let now = Utc::now();
        let key = (share_id.to_string(), client_ip.to_string());
        let mut failures = self.failures.lock().await;
        if !failures.contains_key(&key) && failures.len() >= MAX_TRACKED_SHARE_IPS {
            prune_share_failure_states(&mut failures, now_instant);
            if failures.len() >= MAX_TRACKED_SHARE_IPS {
                if let Some(oldest) = failures
                    .iter()
                    .min_by_key(|(_, state)| state.last_seen_at)
                    .map(|(key, _)| key.clone())
                {
                    failures.remove(&oldest);
                }
            }
        }
        let state = failures.entry(key).or_default();
        state.last_seen_at = now_instant;
        while state
            .failures
            .front()
            .is_some_and(|(seen_at, _)| now_instant.duration_since(*seen_at) > SHARE_ABUSE_WINDOW)
        {
            state.failures.pop_front();
        }
        state.failures.push_back((now_instant, now));
        if state.failures.len() < SHARE_ABUSE_LIMIT {
            return None;
        }
        let first_failure_at = state
            .failures
            .front()
            .map(|(_, seen_at)| *seen_at)
            .unwrap_or(now);
        let failure_count = state.failures.len();
        state.failures.clear();
        Some(ShareBanDecision {
            share_id: share_id.to_string(),
            client_ip: client_ip.to_string(),
            reason_code: reason_code.to_string(),
            failure_count,
            first_failure_at,
            last_failure_at: now,
            banned_at: now,
            banned_until: now + SHARE_BAN_DURATION,
        })
    }

    pub async fn activate(&self, ban: ShareClientBan) {
        self.active_bans
            .lock()
            .await
            .insert((ban.share_id.clone(), ban.client_ip.clone()), ban);
    }

    pub async fn unban(&self, share_id: &str, client_ip: &str) {
        let key = (share_id.to_string(), client_ip.to_string());
        self.active_bans.lock().await.remove(&key);
        self.failures.lock().await.remove(&key);
    }
}

fn prune_share_failure_states(
    failures: &mut HashMap<(String, String), ShareFailureState>,
    now: Instant,
) {
    failures.retain(|_, state| now.duration_since(state.last_seen_at) <= MAX_IDLE_DURATION);
}

fn prune_idle_states(by_ip: &mut HashMap<String, IpAbuseState>, now: Instant) {
    by_ip.retain(|_, state| {
        let actively_banned = state.banned_until.is_some_and(|until| until > now);
        actively_banned || now.duration_since(state.last_seen_at) <= MAX_IDLE_DURATION
    });
}

fn evict_oldest_state(by_ip: &mut HashMap<String, IpAbuseState>) {
    let oldest = by_ip
        .iter()
        .min_by_key(|(_, state)| state.last_seen_at)
        .map(|(ip, _)| ip.clone());
    if let Some(oldest) = oldest {
        by_ip.remove(&oldest);
    }
}

fn prune_old_failures(failures: &mut VecDeque<Instant>, now: Instant) {
    while failures
        .front()
        .is_some_and(|seen_at| now.duration_since(*seen_at) > INVALID_AUTH_WINDOW)
    {
        failures.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bans_on_tenth_invalid_auth() {
        let tracker = AbuseTracker::new();
        for _ in 0..(INVALID_AUTH_LIMIT - 1) {
            assert!(tracker.record_invalid_auth("203.0.113.10").await.is_none());
        }

        let decision = tracker
            .record_invalid_auth("203.0.113.10")
            .await
            .expect("tenth invalid auth should ban");
        assert_eq!(decision.failures, INVALID_AUTH_LIMIT);
        assert_eq!(decision.ban_duration, BAN_DURATION);
        assert!(tracker.ban_remaining("203.0.113.10").await.is_some());
    }

    #[tokio::test]
    async fn tracks_ips_independently() {
        let tracker = AbuseTracker::new();
        for _ in 0..(INVALID_AUTH_LIMIT - 1) {
            tracker.record_invalid_auth("203.0.113.10").await;
        }

        assert!(tracker.record_invalid_auth("203.0.113.11").await.is_none());
        assert!(tracker.ban_remaining("203.0.113.10").await.is_none());
        assert!(tracker.ban_remaining("203.0.113.11").await.is_none());
    }

    #[tokio::test]
    async fn tracked_ip_count_is_bounded() {
        let tracker = AbuseTracker::new();
        for index in 0..(MAX_TRACKED_IPS + 32) {
            tracker
                .record_invalid_auth(&format!("2001:db8::{index:x}"))
                .await;
        }
        assert!(tracker.by_ip.lock().await.len() <= MAX_TRACKED_IPS);
    }

    #[tokio::test]
    async fn share_bans_are_scoped_by_share_and_ip() {
        let tracker = ShareAbuseTracker::new([]);
        for _ in 0..(SHARE_ABUSE_LIMIT - 1) {
            assert!(
                tracker
                    .record_violation("share-a", "203.0.113.10", "share_client_abuse")
                    .await
                    .is_none()
            );
        }
        let decision = tracker
            .record_violation("share-a", "203.0.113.10", "share_client_abuse")
            .await
            .expect("tenth Share violation should produce a ban decision");
        let ban = ShareClientBan {
            id: "ban-a".into(),
            share_id: decision.share_id,
            client_ip: decision.client_ip,
            reason_code: decision.reason_code,
            failure_count: decision.failure_count,
            first_failure_at: decision.first_failure_at,
            last_failure_at: decision.last_failure_at,
            banned_at: decision.banned_at,
            banned_until: decision.banned_until,
        };
        tracker.activate(ban).await;

        assert!(
            tracker
                .ban_remaining("share-a", "203.0.113.10")
                .await
                .is_some()
        );
        assert!(
            tracker
                .ban_remaining("share-b", "203.0.113.10")
                .await
                .is_none()
        );
        assert!(
            tracker
                .ban_remaining("share-a", "203.0.113.11")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn unban_clears_active_ban_and_failure_window() {
        let now = Utc::now();
        let tracker = ShareAbuseTracker::new([ShareClientBan {
            id: "ban-a".into(),
            share_id: "share-a".into(),
            client_ip: "2001:db8::1".into(),
            reason_code: "share_client_abuse".into(),
            failure_count: SHARE_ABUSE_LIMIT,
            first_failure_at: now,
            last_failure_at: now,
            banned_at: now,
            banned_until: now + SHARE_BAN_DURATION,
        }]);

        tracker.unban("share-a", "2001:db8::1").await;

        assert!(
            tracker
                .ban_remaining("share-a", "2001:db8::1")
                .await
                .is_none()
        );
        assert!(
            tracker
                .record_violation("share-a", "2001:db8::1", "share_client_abuse")
                .await
                .is_none()
        );
    }
}
