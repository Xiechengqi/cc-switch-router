//! Router-side scheduling signal computation for market consumers.
//!
//! The market sorts shares using a base score that combines three signals:
//! [`QuotaHealth`], [`Stability`], and [`Headroom`]. The router owns ground
//! truth for all three (upstream quota fields, health-check history, live
//! concurrency counters) so it computes them once per `/v1/market/shares`
//! response. The market then applies its profile-specific sort on top.
//!
//! [`OverrideStore`] holds per-owner penalty multipliers seeded by 429
//! feedback from markets, so a transient upstream rate-limit decays without
//! requiring a DB write.
//!
//! See `docs/scheduling/router-signals.md` (TBD) for the full design.
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::{ShareSignals, ShareUpstreamQuota};

/// Minimum number of seconds a quota window must have left before we treat its
/// utilization as a meaningful health signal. Windows that reset within an hour
/// are ignored because momentary spikes near reset corrupt the score.
const QUOTA_WINDOW_MIN_TTL_S: i64 = 3_600;
/// Softmin temperature for combining multiple quota tiers; higher values pull
/// the result toward the worst tier.
const QUOTA_SOFTMIN_ALPHA: f64 = 10.0;
/// Reference horizon for the "near reset" urgency bonus on quota health.
const QUOTA_URGENCY_HORIZON_S: f64 = 18_000.0; // 5h
/// Weight applied to the 10-minute stability sample when we have full coverage.
const STABILITY_W10_MAX: f64 = 0.7;
/// Maximum healthy minutes inside the 10-minute window. Used as the confidence
/// denominator and as the normalizer for the 10m rate itself.
const STABILITY_W10_SAMPLES: f64 = 10.0;
/// Lower bound applied to headroom so a fully saturated share is still
/// schedulable (avoids deadlocks under hot traffic spikes).
const HEADROOM_FLOOR: f64 = 0.1;
/// TTL applied to a per-owner penalty entry when no explicit duration is sent.
const OVERRIDE_DEFAULT_TTL: Duration = Duration::from_secs(30 * 60);

/// Compute the rolled-up [`ShareSignals`] for one share.
///
/// `online_rate_24h` is the existing ratio (`online_minutes_24h / 1440`).
/// `samples_10m` is the count of distinct healthy minutes in the last
/// 10 minutes — also the numerator of `online_rate_10m` since one check
/// per minute is expected.
///
/// Currently only exercised via unit tests; the production path computes
/// each signal individually inside `list_market_shares` for SQL-row locality.
#[allow(dead_code)]
pub fn compute_share_signals(
    quota: Option<&ShareUpstreamQuota>,
    online_rate_24h: f64,
    samples_10m: usize,
    active_requests: usize,
    parallel_limit: i64,
    owner_email: Option<&str>,
    overrides: &OverrideStore,
    now: DateTime<Utc>,
) -> ShareSignals {
    let quota_health = compute_quota_health(quota, now);
    let stability = compute_stability(samples_10m, online_rate_24h);
    let headroom = compute_headroom(active_requests, parallel_limit);
    let owner_penalty = owner_email
        .map(|e| overrides.get(e).unwrap_or(1.0))
        .unwrap_or(1.0);

    ShareSignals {
        quota_health,
        stability,
        headroom,
        samples_10m: samples_10m as u32,
        owner_penalty,
    }
}

/// Softmin over all quota tiers whose reset window is far enough away to be
/// meaningful. Each tier contributes `1 - utilization` with an urgency bonus
/// proportional to how close the window is to resetting (so a near-empty
/// weekly bucket counts more than a near-empty 5h bucket).
///
/// Returns 0.5 (neutral) if `quota` is `None` or all tiers are short-window;
/// the missing-signal case should not punish, since cc-switch hasn't observed
/// the upstream yet.
pub fn compute_quota_health(quota: Option<&ShareUpstreamQuota>, now: DateTime<Utc>) -> f64 {
    let Some(q) = quota else {
        return 0.5;
    };

    let mut contributions: Vec<f64> = Vec::new();
    for tier in &q.tiers {
        let Some(ttl_s) = tier_ttl_secs(tier.resets_at.as_deref(), now) else {
            continue;
        };
        if ttl_s < QUOTA_WINDOW_MIN_TTL_S {
            continue;
        }
        // utilization is reported in 0..100. Clamp defensively.
        let util_ratio = (tier.utilization / 100.0).clamp(0.0, 1.0);
        let headroom = 1.0 - util_ratio;
        // Urgency bonus: shorter horizon → larger bonus.
        let urgency = (QUOTA_URGENCY_HORIZON_S / (QUOTA_URGENCY_HORIZON_S + ttl_s as f64))
            .clamp(0.0, 1.0)
            * 0.5;
        contributions.push((headroom + headroom * urgency).clamp(0.0, 1.5));
    }

    if contributions.is_empty() {
        return 0.5;
    }
    softmin(&contributions, QUOTA_SOFTMIN_ALPHA).clamp(0.0, 1.5)
}

/// Confidence-weighted blend of the 10-minute online rate and the 24-hour
/// rate. With full coverage (10 healthy samples in the last 10 minutes) the
/// 10m signal carries weight 0.7; with no samples the score collapses to the
/// 24h baseline.
pub fn compute_stability(samples_10m: usize, online_rate_24h: f64) -> f64 {
    let samples = (samples_10m as f64).min(STABILITY_W10_SAMPLES);
    let online_rate_10m = samples / STABILITY_W10_SAMPLES;
    let w10 = STABILITY_W10_MAX * (samples / STABILITY_W10_SAMPLES);
    let blended = w10 * online_rate_10m + (1.0 - w10) * online_rate_24h.clamp(0.0, 1.0);
    blended.clamp(0.0, 1.0)
}

/// Free-capacity ratio clamped to `[0.1, 1.0]`. `parallel_limit <= 0` is
/// interpreted as "unbounded" (router-side semantic), which maps to 1.0.
pub fn compute_headroom(active_requests: usize, parallel_limit: i64) -> f64 {
    if parallel_limit <= 0 {
        return 1.0;
    }
    let limit = parallel_limit as f64;
    let active = active_requests as f64;
    let ratio = 1.0 - (active / limit);
    ratio.clamp(HEADROOM_FLOOR, 1.0)
}

fn softmin(values: &[f64], alpha: f64) -> f64 {
    // Numerically-stable softmin: shift by the minimum so exponents are bounded.
    let min_v = values.iter().copied().fold(f64::INFINITY, f64::min);
    let mut num = 0.0;
    let mut den = 0.0;
    for v in values {
        let w = (-alpha * (v - min_v)).exp();
        num += v * w;
        den += w;
    }
    if den > 0.0 { num / den } else { min_v }
}

fn tier_ttl_secs(resets_at: Option<&str>, now: DateTime<Utc>) -> Option<i64> {
    let raw = resets_at?;
    let parsed = DateTime::parse_from_rfc3339(raw).ok()?.with_timezone(&Utc);
    Some((parsed - now).num_seconds())
}

/// In-memory store of per-owner penalty multipliers (range `(0.0, 1.0]`).
/// A value of `1.0` means "no penalty". Lower values down-rank the share.
///
/// Markets push 429/rate_limited feedback via
/// `POST /v1/market/shares/feedback`; the router scopes the penalty to the
/// owner email (shared upstream credentials → shared rate limit) and expires
/// it after `OVERRIDE_DEFAULT_TTL` unless renewed.
#[derive(Debug, Default)]
pub struct OverrideStore {
    inner: RwLock<HashMap<String, OverrideEntry>>,
}

#[derive(Debug, Clone, Copy)]
struct OverrideEntry {
    penalty: f64,
    expires_at: Instant,
}

impl OverrideStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Fetch the active penalty for an owner, or `None` if absent/expired.
    /// Expired entries are not removed here; [`Self::cleanup_expired`] handles
    /// reclamation lazily.
    pub fn get(&self, owner_email: &str) -> Option<f64> {
        let key = owner_email.to_ascii_lowercase();
        let guard = self.inner.read().ok()?;
        let entry = guard.get(&key)?;
        if entry.expires_at <= Instant::now() {
            None
        } else {
            Some(entry.penalty)
        }
    }

    /// Install or refresh a penalty for an owner. `ttl == None` falls back to
    /// `OVERRIDE_DEFAULT_TTL`. The lower of the new and existing penalty wins
    /// (more conservative); `expires_at` is always extended.
    pub fn set(&self, owner_email: &str, penalty: f64, ttl: Option<Duration>) {
        let key = owner_email.to_ascii_lowercase();
        let penalty = penalty.clamp(0.05, 1.0);
        let ttl = ttl.unwrap_or(OVERRIDE_DEFAULT_TTL);
        let expires_at = Instant::now() + ttl;
        let Ok(mut guard) = self.inner.write() else {
            return;
        };
        guard
            .entry(key)
            .and_modify(|e| {
                e.penalty = e.penalty.min(penalty);
                e.expires_at = e.expires_at.max(expires_at);
            })
            .or_insert(OverrideEntry {
                penalty,
                expires_at,
            });
    }

    /// Drop entries whose TTL has elapsed. Called from the existing background
    /// cleanup loop (no dedicated task).
    pub fn cleanup_expired(&self) {
        let Ok(mut guard) = self.inner.write() else {
            return;
        };
        let now = Instant::now();
        guard.retain(|_, e| e.expires_at > now);
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.read().map(|g| g.len()).unwrap_or(0)
    }
}

/// Request body for `POST /v1/market/shares/feedback`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareFeedbackRequest {
    /// Share whose request triggered the feedback. The router resolves the
    /// owner email and applies the penalty to all shares of that owner.
    pub share_id: String,
    /// `rate_limited` is the only kind handled today. Future kinds (e.g.
    /// `auth_failed`) are reserved.
    pub kind: ShareFeedbackKind,
    /// Optional explicit penalty override; clamped to `(0.05, 1.0]`. Defaults
    /// to `0.5` for `rate_limited`.
    #[serde(default)]
    pub penalty: Option<f64>,
    /// Optional TTL in seconds; defaults vary by kind.
    #[serde(default)]
    pub ttl_secs: Option<u64>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ShareFeedbackKind {
    RateLimited,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareFeedbackResponse {
    pub ok: bool,
    pub owner_scope: Option<String>,
    pub applied_penalty: f64,
    pub expires_in_secs: u64,
}

/// Request body for `POST /v1/market/shares/headroom`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareHeadroomRequest {
    pub share_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareHeadroomEntry {
    pub share_id: String,
    pub active_requests: usize,
    pub parallel_limit: i64,
    pub headroom: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareHeadroomResponse {
    pub queried_at: String,
    pub entries: Vec<ShareHeadroomEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(secs_from_now: i64) -> String {
        let now = Utc::now();
        let t = now + chrono::Duration::seconds(secs_from_now);
        t.to_rfc3339()
    }

    #[test]
    fn quota_health_skips_short_windows() {
        let quota = ShareUpstreamQuota {
            status: "ok".into(),
            queried_at: None,
            tiers: vec![crate::models::ShareUpstreamQuotaTier {
                label: "5h".into(),
                utilization: 90.0,
                resets_at: Some(ts(300)), // 5 minutes out, below 1h threshold
            }],
        };
        // Only short window → fall back to neutral.
        assert_eq!(compute_quota_health(Some(&quota), Utc::now()), 0.5);
    }

    #[test]
    fn quota_health_softmin_picks_worst_tier() {
        let quota = ShareUpstreamQuota {
            status: "ok".into(),
            queried_at: None,
            tiers: vec![
                crate::models::ShareUpstreamQuotaTier {
                    label: "weekly".into(),
                    utilization: 10.0,
                    resets_at: Some(ts(7 * 86400)),
                },
                crate::models::ShareUpstreamQuotaTier {
                    label: "daily".into(),
                    utilization: 95.0,
                    resets_at: Some(ts(86400)),
                },
            ],
        };
        let v = compute_quota_health(Some(&quota), Utc::now());
        // The 95%-utilized daily tier dominates softmin, so result is near 0.05..0.1.
        assert!(v < 0.25, "expected near-zero quota health, got {v}");
    }

    #[test]
    fn quota_health_missing_quota_is_neutral() {
        assert_eq!(compute_quota_health(None, Utc::now()), 0.5);
    }

    #[test]
    fn stability_zero_samples_collapses_to_24h() {
        let s = compute_stability(0, 0.83);
        assert!((s - 0.83).abs() < 1e-9);
    }

    #[test]
    fn stability_full_samples_blends_with_max_weight() {
        // 10 healthy minutes → online_rate_10m = 1.0, w10 = 0.7
        // blended = 0.7*1.0 + 0.3*0.5 = 0.85
        let s = compute_stability(10, 0.5);
        assert!((s - 0.85).abs() < 1e-9, "got {s}");
    }

    #[test]
    fn headroom_unlimited_returns_one() {
        assert_eq!(compute_headroom(50, -1), 1.0);
        assert_eq!(compute_headroom(0, 0), 1.0);
    }

    #[test]
    fn headroom_saturated_clamps_to_floor() {
        assert_eq!(compute_headroom(10, 10), HEADROOM_FLOOR);
        assert_eq!(compute_headroom(20, 10), HEADROOM_FLOOR);
    }

    #[test]
    fn headroom_partial_is_proportional() {
        let h = compute_headroom(3, 10);
        assert!((h - 0.7).abs() < 1e-9, "got {h}");
    }

    #[test]
    fn override_set_lowest_penalty_wins() {
        let store = OverrideStore::new();
        store.set("Alice@example.com", 0.7, None);
        store.set("alice@example.com", 0.3, None);
        let p = store.get("ALICE@example.com").unwrap();
        assert!((p - 0.3).abs() < 1e-9);
    }

    #[test]
    fn override_expires_after_ttl() {
        let store = OverrideStore::new();
        store.set("bob@example.com", 0.5, Some(Duration::from_millis(1)));
        std::thread::sleep(Duration::from_millis(5));
        assert!(store.get("bob@example.com").is_none());
    }

    #[test]
    fn override_cleanup_removes_expired() {
        let store = OverrideStore::new();
        store.set("carol@example.com", 0.5, Some(Duration::from_millis(1)));
        std::thread::sleep(Duration::from_millis(5));
        store.cleanup_expired();
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn compute_share_signals_applies_owner_penalty() {
        let store = OverrideStore::new();
        store.set("dave@example.com", 0.4, None);
        let s = compute_share_signals(
            None,
            0.9,
            8,
            2,
            10,
            Some("dave@example.com"),
            &store,
            Utc.timestamp_opt(0, 0).unwrap(),
        );
        assert!((s.owner_penalty - 0.4).abs() < 1e-9);
    }
}
