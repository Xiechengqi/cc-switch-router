//! Bounded, process-local diagnostics. No credentials, response bodies or schema.
use crate::models::ModelRouteRecentResult;
use std::collections::{HashMap, HashSet};
use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

const CAPACITY: usize = 4096;
const RECENT_TTL: Duration = Duration::from_secs(3600);
const RATE_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct Observation {
    user_id: String,
    revision: u64,
    sequence: u64,
    generation: u64,
    completed_at: Option<Instant>,
    started: Instant,
    result: Option<ModelRouteRecentResult>,
}
#[derive(Debug, Default)]
pub struct ModelRouteRuntime {
    sequence: AtomicU64,
    recent: Mutex<HashMap<String, Observation>>,
    last_error_warning: Mutex<Option<Instant>>,
    rates: Mutex<HashMap<String, (Instant, usize)>>,
}
impl ModelRouteRuntime {
    pub fn next_sequence(&self) -> Option<u64> {
        self.sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .ok()
            .map(|value| value + 1)
    }
    /// Called only while the existing Store snapshot lock is held.
    pub fn register(&self, user: &str, route: &str, revision: u64) -> Option<u64> {
        let mut entries = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = entries.get(route) {
            if entry.user_id == user && entry.revision == revision {
                return Some(entry.generation);
            }
        }
        if entries.len() >= CAPACITY && !entries.contains_key(route) {
            Self::sweep_entries(&mut entries, Instant::now());
            if entries.len() >= CAPACITY {
                let oldest = entries
                    .iter()
                    .min_by_key(|(_, entry)| entry.sequence)
                    .map(|(id, _)| id.clone());
                if let Some(id) = oldest {
                    entries.remove(&id);
                }
            }
        }
        let generation = self.next_sequence()?;
        entries.insert(
            route.into(),
            Observation {
                user_id: user.into(),
                revision,
                sequence: 0,
                generation,
                completed_at: None,
                started: Instant::now(),
                result: None,
            },
        );
        Some(generation)
    }
    pub fn begin(
        &self,
        user: &str,
        route: &str,
        revision: u64,
        generation: Option<u64>,
        sequence: u64,
    ) {
        let mut entries = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = entries.get_mut(route) {
            if entry.user_id == user
                && entry.revision == revision
                && Some(entry.generation) == generation
                && sequence > entry.sequence
            {
                entry.sequence = sequence;
                entry.started = Instant::now();
            }
        }
    }
    fn sweep_entries(entries: &mut HashMap<String, Observation>, now: Instant) {
        entries.retain(|_, entry| {
            let touched = entry
                .completed_at
                .map_or(entry.started, |at| at.max(entry.started));
            now.saturating_duration_since(touched) < RECENT_TTL
        });
    }
    pub fn sweep(&self) {
        Self::sweep_entries(
            &mut self.recent.lock().unwrap_or_else(|e| e.into_inner()),
            Instant::now(),
        );
    }
    pub fn finish(
        &self,
        user: &str,
        generation: Option<u64>,
        sequence: u64,
        result: ModelRouteRecentResult,
    ) {
        let mut entries = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = entries.get_mut(&result.route_id) {
            if entry.user_id == user
                && entry.revision == result.revision
                && Some(entry.generation) == generation
                && entry.sequence == sequence
            {
                entry.completed_at = Some(Instant::now());
                entry.result = Some(result);
            }
        }
    }
    pub fn results(
        &self,
        user: &str,
        revision: u64,
        routes: &HashSet<String>,
    ) -> Vec<ModelRouteRecentResult> {
        self.results_at(user, revision, routes, Instant::now())
    }
    fn results_at(
        &self,
        user: &str,
        revision: u64,
        routes: &HashSet<String>,
        now: Instant,
    ) -> Vec<ModelRouteRecentResult> {
        self.recent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(route, entry)| {
                routes.contains(*route)
                    && entry.user_id == user
                    && entry.revision == revision
                    && entry
                        .completed_at
                        .is_some_and(|at| now.saturating_duration_since(at) < RECENT_TTL)
            })
            .filter_map(|(_, entry)| entry.result.clone())
            .collect()
    }
    pub fn remove_routes(&self, routes: &HashSet<String>) {
        self.recent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|route, _| !routes.contains(route));
    }
    pub fn should_warn_error(&self) -> bool {
        let now = Instant::now();
        let mut last = self
            .last_error_warning
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|at| now.saturating_duration_since(at) < RATE_WINDOW) {
            return false;
        }
        *last = Some(now);
        true
    }
    pub fn admit_diagnostic(&self, user: &str) -> Result<(), u64> {
        self.admit_at(user, Instant::now())
    }
    fn admit_at(&self, user: &str, now: Instant) -> Result<(), u64> {
        let mut entries = self.rates.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|_, (started, _)| now.saturating_duration_since(*started) < RATE_WINDOW);
        if !entries.contains_key(user) && entries.len() >= CAPACITY {
            return Err(60);
        }
        let (started, count) = entries.entry(user.into()).or_insert((now, 0));
        if *count >= 3 {
            let remaining = RATE_WINDOW.saturating_sub(now.saturating_duration_since(*started));
            return Err(((remaining.as_millis() + 999) / 1000).max(1) as u64);
        }
        *count += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn result(route: &str, revision: u64, sequence: u64) -> ModelRouteRecentResult {
        ModelRouteRecentResult {
            route_id: route.into(),
            revision,
            request_sequence: sequence.to_string(),
            observed_at: chrono::Utc::now(),
            source: "request",
            phase: "response_headers",
            target_share_id: None,
            position: None,
            status_code: 200,
            continued_search: false,
            used_backup: false,
            body_state: None,
        }
    }
    impl ModelRouteRuntime {
        fn begin_at(&self, user: &str, route: &str, revision: u64, sequence: u64, now: Instant) {
            {
                let mut entries = self.recent.lock().unwrap();
                Self::sweep_entries(&mut entries, now);
            }
            let generation = self.register(user, route, revision);
            self.begin(user, route, revision, generation, sequence);
            self.recent.lock().unwrap().get_mut(route).unwrap().started = now;
        }
        fn test_finish(&self, user: &str, sequence: u64, result: ModelRouteRecentResult) {
            let generation = self
                .recent
                .lock()
                .unwrap()
                .get(&result.route_id)
                .map(|entry| entry.generation);
            self.finish(user, generation, sequence, result);
        }
    }
    #[test]
    fn model_route_runtime_limits_are_bounded_and_expire_without_sleeping() {
        let runtime = ModelRouteRuntime::default();
        let now = Instant::now();
        for _ in 0..3 {
            assert_eq!(runtime.admit_at("user", now), Ok(()));
        }
        assert_eq!(
            runtime.admit_at("user", now + Duration::from_millis(1)),
            Err(60)
        );
        assert_eq!(runtime.admit_at("user", now + RATE_WINDOW), Ok(()));
        for index in 0..CAPACITY {
            let _ = runtime.admit_at(&format!("u-{index}"), now + RATE_WINDOW);
        }
        assert_eq!(runtime.rates.lock().unwrap().len(), CAPACITY);
        assert!(runtime.admit_at("overflow", now + RATE_WINDOW).is_err());
        assert!(runtime.admit_at("overflow", now + RATE_WINDOW * 2).is_ok());
    }
    #[test]
    fn model_route_recent_reservations_prevent_old_completion_even_after_new_cancellation() {
        let runtime = ModelRouteRuntime::default();
        let now = Instant::now();
        runtime.begin_at("u", "r", 1, 1, now);
        runtime.begin_at("u", "r", 1, 2, now);
        runtime.begin_at("u", "r", 1, 1, now);
        assert_eq!(runtime.recent.lock().unwrap()["r"].sequence, 2);
        let result = ModelRouteRecentResult {
            route_id: "r".into(),
            revision: 1,
            request_sequence: "1".into(),
            observed_at: chrono::Utc::now(),
            source: "request",
            phase: "response_headers",
            target_share_id: None,
            position: None,
            status_code: 200,
            continued_search: false,
            used_backup: false,
            body_state: None,
        };
        runtime.test_finish("u", 1, result.clone());
        assert!(
            runtime
                .results("u", 1, &HashSet::from(["r".into()]))
                .is_empty()
        );
        runtime.test_finish("other-user", 2, result.clone());
        assert!(
            runtime
                .results("u", 1, &HashSet::from(["r".into()]))
                .is_empty()
        );
        runtime.test_finish("u", 2, result);
        assert_eq!(
            runtime.results("u", 1, &HashSet::from(["r".into()])).len(),
            1
        );
        assert!(
            runtime
                .results("u", 2, &HashSet::from(["r".into()]))
                .is_empty()
        );
        runtime.remove_routes(&HashSet::from(["r".into()]));
        assert!(runtime.recent.lock().unwrap().is_empty());
        for index in 0..=CAPACITY {
            runtime.begin_at("u", &format!("r-{index}"), 1, index as u64 + 3, now);
        }
        assert_eq!(runtime.recent.lock().unwrap().len(), CAPACITY);
        assert!(!runtime.recent.lock().unwrap().contains_key("r-0"));
        runtime.begin_at("u", "fresh", 1, 10000, now + RECENT_TTL);
        assert_eq!(runtime.recent.lock().unwrap().len(), 1);
    }
    #[test]
    fn model_route_generations_reject_evicted_and_removed_snapshot_reservations() {
        let runtime = ModelRouteRuntime::default();
        let old = runtime.register("u", "r", 1);
        runtime.remove_routes(&HashSet::from(["r".into()]));
        runtime.begin("u", "r", 1, old, 999);
        assert!(runtime.recent.lock().unwrap().is_empty());
        let new = runtime.register("u", "r", 1);
        assert_ne!(old, new);
        runtime.begin("u", "r", 1, new, 10);
        runtime.begin("u", "r", 1, old, 1000);
        assert_eq!(runtime.recent.lock().unwrap()["r"].sequence, 10);
    }
    #[test]
    fn model_route_begin_does_not_extend_completed_observation_ttl() {
        let runtime = ModelRouteRuntime::default();
        let generation = runtime.register("u", "r", 1);
        let now = Instant::now();
        runtime.begin("u", "r", 1, generation, 1);
        runtime.finish("u", generation, 1, result("r", 1, 1));
        runtime
            .recent
            .lock()
            .unwrap()
            .get_mut("r")
            .unwrap()
            .completed_at = Some(now - RECENT_TTL);
        runtime.begin("u", "r", 1, generation, 2);
        assert_eq!(
            runtime.recent.lock().unwrap()["r"].completed_at,
            Some(now - RECENT_TTL)
        );
        assert!(
            runtime
                .results_at("u", 1, &HashSet::from(["r".into()]), now)
                .is_empty()
        );
    }
    #[test]
    fn model_route_sweep_retains_completed_results_for_a_full_ttl() {
        let runtime = ModelRouteRuntime::default();
        let generation = runtime.register("u", "r", 1);
        let now = Instant::now();
        runtime.begin("u", "r", 1, generation, 1);
        runtime.finish("u", generation, 1, result("r", 1, 1));
        {
            let mut entries = runtime.recent.lock().unwrap();
            let entry = entries.get_mut("r").unwrap();
            entry.started = now;
            entry.completed_at = Some(now + Duration::from_secs(30));
            ModelRouteRuntime::sweep_entries(&mut entries, now + RECENT_TTL);
            assert!(entries.contains_key("r"));
            ModelRouteRuntime::sweep_entries(
                &mut entries,
                now + RECENT_TTL + Duration::from_secs(30),
            );
            assert!(entries.is_empty());
        }
    }
    #[test]
    fn model_route_evicted_generation_cannot_begin_or_finish_a_recreated_entry() {
        let runtime = ModelRouteRuntime::default();
        let old = runtime.register("u", "r", 1);
        runtime.begin("u", "r", 1, old, 1);
        for index in 0..CAPACITY {
            let route = format!("other-{index}");
            let generation = runtime.register("u", &route, 1);
            runtime.begin("u", &route, 1, generation, index as u64 + 10);
        }
        assert!(!runtime.recent.lock().unwrap().contains_key("r"));
        let new = runtime.register("u", "r", 1);
        assert_ne!(new, old);
        runtime.begin("u", "r", 1, new, 2);
        runtime.begin("u", "r", 1, old, 3);
        runtime.finish("u", old, 2, result("r", 1, 2));
        assert!(
            runtime
                .results("u", 1, &HashSet::from(["r".into()]))
                .is_empty()
        );
        runtime.finish("u", new, 2, result("r", 1, 2));
        assert_eq!(
            runtime.results("u", 1, &HashSet::from(["r".into()])).len(),
            1
        );
    }
}
