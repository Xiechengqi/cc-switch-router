use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

const INVALID_AUTH_WINDOW: Duration = Duration::from_secs(10 * 60);
const INVALID_AUTH_LIMIT: usize = 10;
const BAN_DURATION: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, Copy)]
pub struct BanDecision {
    pub failures: usize,
    pub ban_duration: Duration,
}

#[derive(Debug, Default)]
struct IpAbuseState {
    invalid_auth_at: VecDeque<Instant>,
    banned_until: Option<Instant>,
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
        let state = by_ip.entry(ip.to_string()).or_default();
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
}
