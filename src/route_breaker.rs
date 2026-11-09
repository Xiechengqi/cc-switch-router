//! Process-local, bounded circuit breakers for explicit model-route candidates.
//! Only routed attempts drive it: a real response, or a tunnel found offline at
//! dispatch time. Share health probes never do. Store serializes configuration
//! changes and result writes (Store connection -> registry, never the reverse).
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const FAILURE_THRESHOLD: u32 = 3;
const COOLDOWN: Duration = Duration::from_secs(60);
const MAX_COOLDOWN: Duration = Duration::from_secs(600);
const IDLE_TTL: Duration = Duration::from_secs(3600);
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(600);
const MAX_BREAKER_STATES: usize = 16_384;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BreakerPhase {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug)]
struct BreakerState {
    phase: BreakerPhase,
    consecutive_failures: u32,
    open_until: Option<Instant>,
    probe: Option<u64>,
    last_touched_at: Instant,
}
impl BreakerState {
    fn new(now: Instant) -> Self {
        Self {
            phase: BreakerPhase::Closed,
            consecutive_failures: 0,
            open_until: None,
            probe: None,
            last_touched_at: now,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BreakerKey {
    route_id: String,
    target_share_id: String,
}
impl BreakerKey {
    fn new(route: &str, target: &str) -> Self {
        Self {
            route_id: route.into(),
            target_share_id: target.into(),
        }
    }
}
#[derive(Debug, Default)]
struct RegistryInner {
    states: HashMap<BreakerKey, BreakerState>,
    next_probe: u64,
    capacity_dropped: u64,
    last_capacity_warning: Option<Instant>,
}
/// Not Clone/Copy: dropping an unfinished probe releases precisely its permit.
#[derive(Debug)]
pub struct ProbePermitGuard {
    inner: Arc<RwLock<RegistryInner>>,
    key: BreakerKey,
    generation: u64,
    armed: bool,
}
impl Drop for ProbePermitGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if let Some(state) = inner.states.get_mut(&self.key) {
            if state.probe == Some(self.generation) {
                state.probe = None;
                // Cancellation is neutral, not a success or a fresh cooldown.
                state.phase = BreakerPhase::HalfOpen;
            }
        }
    }
}
#[derive(Debug)]
pub enum BreakerPermit {
    Allowed {
        guard: Option<ProbePermitGuard>,
    },
    Rejected,
    /// The cooldown is over and the probe is free, but the caller declined to
    /// probe now. The probe stays free for a later acquire to take.
    ProbeDeferred,
}
#[derive(Debug, Clone, Copy)]
pub enum BreakerOutcome {
    Success,
    Failure { retry_after: Option<Duration> },
    Neutral,
}
#[derive(Debug)]
pub struct RouteBreakerRegistry {
    inner: Arc<RwLock<RegistryInner>>,
    capacity: usize,
}
impl Default for RouteBreakerRegistry {
    fn default() -> Self {
        Self {
            inner: Arc::new(RwLock::new(RegistryInner::default())),
            capacity: MAX_BREAKER_STATES,
        }
    }
}
impl RouteBreakerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// `allow_probe: false` returns `ProbeDeferred` instead of taking a probe.
    pub fn acquire(&self, route: &str, target: &str, allow_probe: bool) -> BreakerPermit {
        self.acquire_at(route, target, allow_probe, Instant::now())
    }
    fn acquire_at(
        &self,
        route: &str,
        target: &str,
        allow_probe: bool,
        now: Instant,
    ) -> BreakerPermit {
        let key = BreakerKey::new(route, target);
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let Some(state) = inner.states.get_mut(&key) else {
            return BreakerPermit::Allowed { guard: None };
        };
        state.last_touched_at = now;
        match state.phase {
            BreakerPhase::Closed => return BreakerPermit::Allowed { guard: None },
            BreakerPhase::Open if state.open_until.is_some_and(|until| now < until) => {
                return BreakerPermit::Rejected;
            }
            _ if state.probe.is_some() => return BreakerPermit::Rejected,
            _ if !allow_probe => return BreakerPermit::ProbeDeferred,
            _ => {}
        }
        // Exhaustion is fail-closed; never recycle an old permit number (ABA).
        let Some(generation) = inner.next_probe.checked_add(1) else {
            return BreakerPermit::Rejected;
        };
        inner.next_probe = generation;
        let state = inner
            .states
            .get_mut(&key)
            .expect("state under the same lock");
        state.phase = BreakerPhase::HalfOpen;
        state.probe = Some(generation);
        BreakerPermit::Allowed {
            guard: Some(ProbePermitGuard {
                inner: self.inner.clone(),
                key,
                generation,
                armed: true,
            }),
        }
    }

    pub fn needs_record(&self, route: &str, target: &str, outcome: BreakerOutcome) -> bool {
        match outcome {
            BreakerOutcome::Neutral => false,
            BreakerOutcome::Failure { .. } => true,
            BreakerOutcome::Success => self
                .inner
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .states
                .contains_key(&BreakerKey::new(route, target)),
        }
    }

    pub fn record(
        &self,
        route: &str,
        target: &str,
        guard: Option<ProbePermitGuard>,
        outcome: BreakerOutcome,
    ) {
        self.record_at(route, target, guard, outcome, Instant::now());
    }
    fn record_at(
        &self,
        route: &str,
        target: &str,
        mut guard: Option<ProbePermitGuard>,
        outcome: BreakerOutcome,
        now: Instant,
    ) {
        if matches!(outcome, BreakerOutcome::Neutral) {
            return;
        } // guard Drop
        let key = BreakerKey::new(route, target);
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if let Some(permit) = &guard {
            // Removal/recreation, or a stale result, must not consume a new probe.
            if permit.key != key
                || inner.states.get(&key).and_then(|s| s.probe) != Some(permit.generation)
            {
                return;
            }
        } else if inner
            .states
            .get(&key)
            .is_some_and(|s| s.phase != BreakerPhase::Closed)
        {
            // A Closed request that completed after the breaker opened cannot
            // release/overwrite its later probe or erase the new cooldown.
            return;
        }
        if let Some(permit) = &mut guard {
            permit.armed = false;
        }
        if matches!(outcome, BreakerOutcome::Success) {
            inner.states.remove(&key);
            return;
        }
        if !inner.states.contains_key(&key) && inner.states.len() >= self.capacity {
            Self::sweep_inner(&mut inner, now);
            if inner.states.len() >= self.capacity {
                inner.capacity_dropped = inner.capacity_dropped.saturating_add(1);
                if inner
                    .last_capacity_warning
                    .is_none_or(|last| now.saturating_duration_since(last) >= SWEEP_INTERVAL)
                {
                    tracing::warn!(
                        dropped = inner.capacity_dropped,
                        "model route breaker capacity reached; new keys are temporarily untracked"
                    );
                    inner.last_capacity_warning = Some(now);
                }
                return;
            }
        }
        let state = inner
            .states
            .entry(key)
            .or_insert_with(|| BreakerState::new(now));
        state.last_touched_at = now;
        state.probe = None;
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        if state.phase == BreakerPhase::HalfOpen || state.consecutive_failures >= FAILURE_THRESHOLD
        {
            let BreakerOutcome::Failure { retry_after } = outcome else {
                unreachable!()
            };
            state.phase = BreakerPhase::Open;
            state.open_until = Some(now + retry_after.unwrap_or(COOLDOWN).min(MAX_COOLDOWN));
        }
    }
    pub fn sweep(&self) {
        Self::sweep_inner(
            &mut self.inner.write().unwrap_or_else(|e| e.into_inner()),
            Instant::now(),
        );
    }
    fn sweep_inner(inner: &mut RegistryInner, now: Instant) {
        inner.states.retain(|_, state| {
            state.probe.is_some()
                || now.saturating_duration_since(state.last_touched_at) < IDLE_TTL
                || (state.phase != BreakerPhase::Closed
                    && state
                        .open_until
                        .is_none_or(|until| now.saturating_duration_since(until) < IDLE_TTL))
        });
    }
    /// Called only after a successful commit while the Store connection is held.
    pub fn retain_configuration(
        &self,
        old_routes: &HashSet<String>,
        current: &HashSet<(String, String)>,
    ) {
        self.inner
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .states
            .retain(|key, _| {
                !old_routes.contains(&key.route_id)
                    || current.contains(&(key.route_id.clone(), key.target_share_id.clone()))
            });
    }
    #[cfg(test)]
    pub fn is_open(&self, route: &str, target: &str) -> bool {
        self.inner
            .read()
            .unwrap()
            .states
            .get(&BreakerKey::new(route, target))
            .is_some_and(|s| s.phase == BreakerPhase::Open)
    }
    /// Ends the cooldown now, as if it had run out.
    #[cfg(test)]
    pub fn end_cooldown(&self, route: &str, target: &str) {
        if let Some(state) = self
            .inner
            .write()
            .unwrap()
            .states
            .get_mut(&BreakerKey::new(route, target))
        {
            state.open_until = Some(Instant::now());
        }
    }
    #[cfg(test)]
    pub fn record_failure(&self, route: &str, target: &str, retry_after: Option<Duration>) {
        self.record(route, target, None, BreakerOutcome::Failure { retry_after });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn failure(registry: &RouteBreakerRegistry, now: Instant) {
        registry.record_at(
            "r",
            "s",
            None,
            BreakerOutcome::Failure { retry_after: None },
            now,
        );
    }
    fn probe(registry: &RouteBreakerRegistry, now: Instant) -> ProbePermitGuard {
        match registry.acquire_at("r", "s", true, now) {
            BreakerPermit::Allowed { guard: Some(guard) } => guard,
            _ => panic!("expected probe"),
        }
    }
    #[test]
    fn route_breaker_success_only_leaves_no_state() {
        let r = RouteBreakerRegistry::new();
        assert!(matches!(
            r.acquire("r", "s", true),
            BreakerPermit::Allowed { guard: None }
        ));
        r.record("r", "s", None, BreakerOutcome::Success);
        assert!(r.inner.read().unwrap().states.is_empty());
        assert!(!r.needs_record("r", "s", BreakerOutcome::Success));
    }
    #[test]
    fn route_breaker_cancel_and_neutral_release_exact_probe() {
        let r = RouteBreakerRegistry::new();
        let now = Instant::now();
        for _ in 0..3 {
            failure(&r, now);
        }
        let guard = probe(&r, now + COOLDOWN);
        assert!(matches!(
            r.acquire_at("r", "s", true, now + COOLDOWN),
            BreakerPermit::Rejected
        ));
        drop(guard);
        let guard = probe(&r, now + COOLDOWN);
        r.record("r", "s", Some(guard), BreakerOutcome::Neutral);
        let guard = probe(&r, now + COOLDOWN);
        r.record("r", "s", Some(guard), BreakerOutcome::Success);
        assert!(r.inner.read().unwrap().states.is_empty());
    }
    #[test]
    fn route_breaker_declined_probe_changes_nothing() {
        let r = RouteBreakerRegistry::new();
        let now = Instant::now();
        assert!(matches!(
            r.acquire_at("r", "s", false, now),
            BreakerPermit::Allowed { guard: None }
        ));
        for _ in 0..3 {
            failure(&r, now);
        }
        assert!(matches!(
            r.acquire_at("r", "s", false, now),
            BreakerPermit::Rejected
        ));
        for _ in 0..2 {
            assert!(matches!(
                r.acquire_at("r", "s", false, now + COOLDOWN),
                BreakerPermit::ProbeDeferred
            ));
        }
        assert!(r.is_open("r", "s"));
        let guard = probe(&r, now + COOLDOWN);
        assert!(matches!(
            r.acquire_at("r", "s", false, now + COOLDOWN),
            BreakerPermit::Rejected
        ));
        drop(guard);
    }
    #[test]
    fn route_breaker_old_guard_and_ordinary_result_cannot_release_new_probe() {
        let r = RouteBreakerRegistry::new();
        let now = Instant::now();
        for _ in 0..3 {
            failure(&r, now);
        }
        let old = probe(&r, now + COOLDOWN);
        r.retain_configuration(&HashSet::from(["r".into()]), &HashSet::new());
        for _ in 0..3 {
            failure(&r, now);
        }
        let current = probe(&r, now + COOLDOWN);
        drop(old);
        r.record("r", "s", None, BreakerOutcome::Success);
        assert!(matches!(
            r.acquire_at("r", "s", true, now + COOLDOWN),
            BreakerPermit::Rejected
        ));
        drop(current);
        drop(probe(&r, now + COOLDOWN));
    }
    #[test]
    fn route_breaker_ttl_covers_partial_failures_and_protects_live_probes() {
        let r = RouteBreakerRegistry::new();
        let now = Instant::now();
        failure(&r, now);
        RouteBreakerRegistry::sweep_inner(&mut r.inner.write().unwrap(), now + IDLE_TTL);
        assert!(r.inner.read().unwrap().states.is_empty());
        for _ in 0..3 {
            failure(&r, now);
        }
        RouteBreakerRegistry::sweep_inner(&mut r.inner.write().unwrap(), now + IDLE_TTL);
        assert!(r.is_open("r", "s")); // cooldown ended less than an hour ago
        let guard = probe(&r, now + COOLDOWN);
        RouteBreakerRegistry::sweep_inner(&mut r.inner.write().unwrap(), now + IDLE_TTL * 3);
        assert_eq!(r.inner.read().unwrap().states.len(), 1);
        drop(guard);
        RouteBreakerRegistry::sweep_inner(&mut r.inner.write().unwrap(), now + IDLE_TTL * 3);
        assert!(r.inner.read().unwrap().states.is_empty());
    }
    #[test]
    fn route_breaker_capacity_does_not_evict_valid_state_and_reclaims_expiry() {
        let r = RouteBreakerRegistry {
            capacity: 1,
            ..Default::default()
        };
        let now = Instant::now();
        for _ in 0..3 {
            failure(&r, now);
        }
        r.record_at(
            "other",
            "target",
            None,
            BreakerOutcome::Failure { retry_after: None },
            now,
        );
        assert_eq!(r.inner.read().unwrap().states.len(), 1);
        assert_eq!(r.inner.read().unwrap().capacity_dropped, 1);
        assert!(r.is_open("r", "s"));
        r.record_at(
            "other",
            "target",
            None,
            BreakerOutcome::Failure { retry_after: None },
            now + IDLE_TTL * 2,
        );
        assert_eq!(r.inner.read().unwrap().states.len(), 1);
        assert!(!r.is_open("r", "s"));
    }
    #[test]
    fn route_breaker_retry_after_caps_and_targets_are_independent() {
        let r = RouteBreakerRegistry::new();
        let now = Instant::now();
        for _ in 0..3 {
            r.record_at(
                "r",
                "s",
                None,
                BreakerOutcome::Failure {
                    retry_after: Some(Duration::from_secs(3600)),
                },
                now,
            );
        }
        assert!(matches!(
            r.acquire_at("r", "s", true, now + MAX_COOLDOWN - Duration::from_secs(1)),
            BreakerPermit::Rejected
        ));
        assert!(matches!(
            r.acquire("other", "s", true),
            BreakerPermit::Allowed { guard: None }
        ));
        drop(probe(&r, now + MAX_COOLDOWN));
    }
}
