use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

const INVALID_AUTH_WINDOW: Duration = Duration::from_secs(10 * 60);
const INVALID_AUTH_LIMIT: usize = 10;
const BAN_DURATION: Duration = Duration::from_secs(60 * 60);
const MAX_TRACKED_IPS: usize = 8_192;
const MAX_IDLE_DURATION: Duration = Duration::from_secs(70 * 60);

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
}
