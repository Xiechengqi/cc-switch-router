//! Bounded, process-local record of each user's unified-API route executions
//! (plan P5-1). Facts the executor already computed: matched rule, attempts,
//! final target and stream phase. No credentials, bodies, IPs or emails, and no
//! schema: a restart or another Router instance starts empty.
use crate::models::ModelRouteAttempt;
use axum::body::{Body, Bytes};
use chrono::{DateTime, Utc};
use http_body::{Body as _, Frame, SizeHint};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const WINDOW: Duration = Duration::from_secs(15 * 60);
pub const PER_USER_CAPACITY: usize = 200;
const GLOBAL_CAPACITY: usize = 20_000;
const MAX_USERS: usize = 4096;
/// Longer than the proxy's absolute request lifetime: an unfinished event this
/// old lost its guard without a drop (it cannot happen normally) and is freed.
const STALE_INFLIGHT: Duration = Duration::from_secs(3 * 3600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRouteTrafficPhase {
    /// Trying candidates; no final response headers yet.
    Routing,
    /// Final headers returned; the body is still flowing.
    Streaming,
    /// The Router saw the body end normally. Not a statement about model output.
    Complete,
    /// The body ended with an error, or the caller left before it ended.
    Interrupted,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRouteTrafficEvent {
    pub sequence: String,
    pub app_type: String,
    pub requested_model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    pub matched_wildcard: bool,
    pub started_at: DateTime<Utc>,
    pub phase: ModelRouteTrafficPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_share_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<usize>,
    pub continued_search: bool,
    pub attempts: Vec<ModelRouteAttempt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRouteTrafficSnapshot {
    pub instance_id: String,
    pub cursor: String,
    /// True when the caller must replace, not merge: first read, another
    /// process, an unreadable cursor, or capacity evicted events it has not seen.
    pub reset: bool,
    pub window_seconds: u64,
    pub per_user_capacity: usize,
    pub events: Vec<ModelRouteTrafficEvent>,
}

/// What is known when a request starts routing.
pub struct ModelRouteTrafficStart {
    pub app_type: String,
    pub requested_model: String,
    pub route_id: Option<String>,
    pub revision: Option<u64>,
    pub matched_wildcard: bool,
}

/// Final-header facts from the candidate executor.
pub struct ModelRouteTrafficHeaders {
    pub status_code: u16,
    pub target_share_id: Option<String>,
    pub position: Option<usize>,
    pub continued_search: bool,
    pub attempts: Vec<ModelRouteAttempt>,
}

#[derive(Debug)]
struct Entry {
    event: ModelRouteTrafficEvent,
    sequence: u64,
    version: u64,
    started: Instant,
    ended: Option<Instant>,
}

#[derive(Debug, Default)]
struct UserLog {
    entries: VecDeque<Entry>,
    /// Highest version dropped by a capacity limit. A reader whose cursor is
    /// older missed it and must reset; time-window expiry needs no reset
    /// because readers apply the same window locally.
    evicted_version: u64,
}

#[derive(Debug, Default)]
struct State {
    users: HashMap<String, UserLog>,
    total: usize,
    version: u64,
}

#[derive(Debug)]
pub struct ModelRouteTraffic {
    instance_id: String,
    state: Mutex<State>,
}

impl Default for ModelRouteTraffic {
    fn default() -> Self {
        Self {
            instance_id: Uuid::new_v4().to_string(),
            state: Mutex::new(State::default()),
        }
    }
}

fn retained(entry: &Entry, now: Instant) -> bool {
    match entry.ended {
        Some(ended) => now.saturating_duration_since(ended) < WINDOW,
        None => now.saturating_duration_since(entry.started) < STALE_INFLIGHT,
    }
}

impl State {
    fn bump(&mut self) -> u64 {
        self.version += 1;
        self.version
    }
    fn sweep_user(&mut self, user: &str, now: Instant) {
        if let Some(log) = self.users.get_mut(user) {
            let before = log.entries.len();
            log.entries.retain(|entry| retained(entry, now));
            self.total -= before - log.entries.len();
        }
    }
    fn sweep(&mut self, now: Instant) {
        let mut total = 0;
        self.users.retain(|_, log| {
            log.entries.retain(|entry| retained(entry, now));
            total += log.entries.len();
            !log.entries.is_empty()
        });
        self.total = total;
    }
    fn evict_front(&mut self, user: &str) {
        if let Some(log) = self.users.get_mut(user) {
            if let Some(entry) = log.entries.pop_front() {
                log.evicted_version = log.evicted_version.max(entry.version);
                self.total -= 1;
            }
        }
    }
    fn find(&mut self, user: &str, sequence: u64) -> Option<&mut Entry> {
        self.users
            .get_mut(user)?
            .entries
            .iter_mut()
            .rev()
            .find(|entry| entry.sequence == sequence)
    }
}

impl ModelRouteTraffic {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn insert(
        &self,
        user: &str,
        sequence: u64,
        event: ModelRouteTrafficEvent,
        now: Instant,
        ended: bool,
    ) -> bool {
        let mut state = self.lock();
        if !state.users.contains_key(user) && state.users.len() >= MAX_USERS {
            state.sweep(now);
            if state.users.len() >= MAX_USERS {
                return false; // Never evict another user to admit a new one.
            }
        }
        state.sweep_user(user, now);
        if state
            .users
            .get(user)
            .is_some_and(|log| log.entries.len() >= PER_USER_CAPACITY)
        {
            state.evict_front(user);
        }
        if state.total >= GLOBAL_CAPACITY {
            state.sweep(now);
            if state.total >= GLOBAL_CAPACITY {
                let heaviest = state
                    .users
                    .iter()
                    .max_by_key(|(_, log)| log.entries.len())
                    .map(|(id, _)| id.clone());
                if let Some(id) = heaviest {
                    state.evict_front(&id);
                }
            }
        }
        let version = state.bump();
        state
            .users
            .entry(user.to_string())
            .or_default()
            .entries
            .push_back(Entry {
                event,
                sequence,
                version,
                started: now,
                ended: ended.then_some(now),
            });
        state.total += 1;
        true
    }

    fn event(
        sequence: u64,
        start: ModelRouteTrafficStart,
        phase: ModelRouteTrafficPhase,
    ) -> ModelRouteTrafficEvent {
        ModelRouteTrafficEvent {
            sequence: sequence.to_string(),
            app_type: start.app_type,
            requested_model: start.requested_model,
            route_id: start.route_id,
            revision: start.revision,
            matched_wildcard: start.matched_wildcard,
            started_at: Utc::now(),
            phase,
            status_code: None,
            error_code: None,
            target_share_id: None,
            position: None,
            continued_search: false,
            attempts: Vec::new(),
            headers_ms: None,
            duration_ms: None,
        }
    }

    /// Starts a routed request. The guard marks it interrupted if dropped early,
    /// e.g. when the caller disconnects while candidates are still being tried.
    pub fn begin(
        self: &Arc<Self>,
        user: &str,
        sequence: u64,
        start: ModelRouteTrafficStart,
    ) -> Option<ModelRouteTrafficGuard> {
        let event = Self::event(sequence, start, ModelRouteTrafficPhase::Routing);
        let now = Instant::now();
        self.insert(user, sequence, event, now, false)
            .then(|| ModelRouteTrafficGuard {
                traffic: self.clone(),
                user: user.to_string(),
                sequence,
                started: now,
                done: false,
            })
    }

    /// Records a request the Router answered before any candidate ran:
    /// no matching rule, or the rule's targets were rejected up front.
    pub fn rejected(
        &self,
        user: &str,
        sequence: u64,
        start: ModelRouteTrafficStart,
        status_code: u16,
        error_code: &'static str,
    ) {
        let mut event = Self::event(sequence, start, ModelRouteTrafficPhase::Complete);
        event.status_code = Some(status_code);
        event.error_code = Some(error_code);
        event.duration_ms = Some(0);
        self.insert(user, sequence, event, Instant::now(), true);
    }

    fn headers(
        &self,
        user: &str,
        sequence: u64,
        headers: ModelRouteTrafficHeaders,
        elapsed: Duration,
    ) {
        let mut state = self.lock();
        let version = state.version + 1;
        let Some(entry) = state.find(user, sequence) else {
            return;
        };
        if entry.event.phase != ModelRouteTrafficPhase::Routing {
            return;
        }
        let event = &mut entry.event;
        event.phase = ModelRouteTrafficPhase::Streaming;
        event.status_code = Some(headers.status_code);
        event.target_share_id = headers.target_share_id;
        event.position = headers.position;
        event.continued_search = headers.continued_search;
        event.attempts = headers.attempts;
        event.headers_ms = Some(elapsed.as_millis() as u64);
        entry.version = version;
        state.version = version;
    }

    fn finish(&self, user: &str, sequence: u64, phase: ModelRouteTrafficPhase, elapsed: Duration) {
        let mut state = self.lock();
        let version = state.version + 1;
        let Some(entry) = state.find(user, sequence) else {
            return;
        };
        // Phases only move forward; a late signal never reopens or rewrites an end.
        if entry.event.phase >= ModelRouteTrafficPhase::Complete || phase <= entry.event.phase {
            return;
        }
        entry.event.phase = phase;
        entry.event.duration_ms = Some(elapsed.as_millis() as u64);
        entry.ended = Some(Instant::now());
        entry.version = version;
        state.version = version;
    }

    pub fn snapshot(
        &self,
        user: &str,
        after: Option<&str>,
        instance: Option<&str>,
    ) -> ModelRouteTrafficSnapshot {
        let mut state = self.lock();
        state.sweep_user(user, Instant::now());
        let cursor = state.version;
        let after = after.and_then(|value| value.parse::<u64>().ok());
        let log = state.users.get(user);
        let reset = instance != Some(self.instance_id.as_str())
            || after.is_none_or(|after| {
                after > cursor || log.is_some_and(|log| after < log.evicted_version)
            });
        let events = log
            .map(|log| {
                log.entries
                    .iter()
                    .filter(|entry| reset || after.is_some_and(|after| entry.version > after))
                    .map(|entry| entry.event.clone())
                    .collect()
            })
            .unwrap_or_default();
        ModelRouteTrafficSnapshot {
            instance_id: self.instance_id.clone(),
            cursor: cursor.to_string(),
            reset,
            window_seconds: WINDOW.as_secs(),
            per_user_capacity: PER_USER_CAPACITY,
            events,
        }
    }
}

/// Owns one routed request's record from first candidate to body end.
#[derive(Debug)]
pub struct ModelRouteTrafficGuard {
    traffic: Arc<ModelRouteTraffic>,
    user: String,
    sequence: u64,
    started: Instant,
    done: bool,
}

impl ModelRouteTrafficGuard {
    pub fn headers(&self, headers: ModelRouteTrafficHeaders) {
        self.traffic
            .headers(&self.user, self.sequence, headers, self.started.elapsed());
    }

    fn finish(mut self, phase: ModelRouteTrafficPhase) {
        self.done = true;
        self.traffic
            .finish(&self.user, self.sequence, phase, self.started.elapsed());
    }

    /// Hands the guard to the response body so the record follows the stream.
    /// Frames, size hint and headers pass through untouched.
    pub fn attach(self, response: axum::response::Response) -> axum::response::Response {
        let (parts, body) = response.into_parts();
        if http_body::Body::is_end_stream(&body) {
            self.finish(ModelRouteTrafficPhase::Complete);
            return axum::response::Response::from_parts(parts, body);
        }
        axum::response::Response::from_parts(
            parts,
            Body::new(TrafficBody {
                inner: body,
                guard: Some(self),
            }),
        )
    }
}

impl Drop for ModelRouteTrafficGuard {
    fn drop(&mut self) {
        if !self.done {
            self.traffic.finish(
                &self.user,
                self.sequence,
                ModelRouteTrafficPhase::Interrupted,
                self.started.elapsed(),
            );
        }
    }
}

struct TrafficBody {
    inner: Body,
    guard: Option<ModelRouteTrafficGuard>,
}

impl TrafficBody {
    fn end(&mut self, phase: ModelRouteTrafficPhase) {
        if let Some(guard) = self.guard.take() {
            guard.finish(phase);
        }
    }
}

impl http_body::Body for TrafficBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let poll = Pin::new(&mut self.inner).poll_frame(cx);
        match &poll {
            Poll::Ready(None) => self.end(ModelRouteTrafficPhase::Complete),
            Poll::Ready(Some(Err(_))) => self.end(ModelRouteTrafficPhase::Interrupted),
            // A sized body can report its end without a trailing `None` poll.
            Poll::Ready(Some(Ok(_))) if self.inner.is_end_stream() => {
                self.end(ModelRouteTrafficPhase::Complete)
            }
            _ => {}
        }
        poll
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for TrafficBody {
    fn drop(&mut self) {
        let phase = if self.inner.is_end_stream() {
            ModelRouteTrafficPhase::Complete
        } else {
            ModelRouteTrafficPhase::Interrupted
        };
        self.end(phase);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    fn start(route: Option<&str>) -> ModelRouteTrafficStart {
        ModelRouteTrafficStart {
            app_type: "codex".into(),
            requested_model: "gpt-test".into(),
            route_id: route.map(Into::into),
            revision: route.map(|_| 1),
            matched_wildcard: false,
        }
    }
    fn headers(status: u16) -> ModelRouteTrafficHeaders {
        ModelRouteTrafficHeaders {
            status_code: status,
            target_share_id: Some("b".into()),
            position: Some(1),
            continued_search: true,
            attempts: vec![
                ModelRouteAttempt {
                    target_share_id: "a".into(),
                    position: 0,
                    outcome: "response",
                    status_code: Some(429),
                    duration_ms: 3,
                },
                ModelRouteAttempt {
                    target_share_id: "b".into(),
                    position: 1,
                    outcome: "response",
                    status_code: Some(status),
                    duration_ms: 5,
                },
            ],
        }
    }
    fn phase(traffic: &ModelRouteTraffic, user: &str, sequence: u64) -> ModelRouteTrafficPhase {
        traffic.lock().find(user, sequence).unwrap().event.phase
    }

    #[test]
    fn model_route_traffic_phases_only_move_forward_and_guard_drop_interrupts() {
        let traffic = Arc::new(ModelRouteTraffic::default());
        let guard = traffic.begin("u", 1, start(Some("r"))).unwrap();
        assert_eq!(phase(&traffic, "u", 1), ModelRouteTrafficPhase::Routing);
        guard.headers(headers(200));
        assert_eq!(phase(&traffic, "u", 1), ModelRouteTrafficPhase::Streaming);
        guard.finish(ModelRouteTrafficPhase::Complete);
        traffic.finish("u", 1, ModelRouteTrafficPhase::Interrupted, Duration::ZERO);
        traffic.headers("u", 1, headers(500), Duration::ZERO);
        let event = traffic.lock().find("u", 1).unwrap().event.clone();
        assert_eq!(event.phase, ModelRouteTrafficPhase::Complete);
        assert_eq!(
            event.status_code,
            Some(200),
            "a late header signal cannot rewrite an ended event"
        );
        assert_eq!(event.attempts.len(), 2);

        drop(traffic.begin("u", 2, start(Some("r"))).unwrap());
        assert_eq!(phase(&traffic, "u", 2), ModelRouteTrafficPhase::Interrupted);
    }

    #[test]
    fn model_route_traffic_rejections_are_recorded_as_ended_facts() {
        let traffic = Arc::new(ModelRouteTraffic::default());
        traffic.rejected("u", 7, start(None), 404, "user_model_route_not_configured");
        let snapshot = traffic.snapshot("u", None, None);
        assert_eq!(snapshot.events.len(), 1);
        let event = &snapshot.events[0];
        assert_eq!(event.phase, ModelRouteTrafficPhase::Complete);
        assert_eq!(
            (event.status_code, event.error_code),
            (Some(404), Some("user_model_route_not_configured"))
        );
        assert!(event.route_id.is_none());
    }

    #[test]
    fn model_route_traffic_cursor_returns_changes_and_resets_when_needed() {
        let traffic = Arc::new(ModelRouteTraffic::default());
        let first = traffic.begin("u", 1, start(Some("r"))).unwrap();
        let full = traffic.snapshot("u", None, None);
        assert!(full.reset);
        assert_eq!(full.events.len(), 1);
        let instance = Some(full.instance_id.as_str());
        let quiet = traffic.snapshot("u", Some(&full.cursor), instance);
        assert!(!quiet.reset);
        assert!(quiet.events.is_empty());
        first.headers(headers(200));
        let changed = traffic.snapshot("u", Some(&full.cursor), instance);
        assert!(!changed.reset);
        assert_eq!(
            changed.events.len(),
            1,
            "a phase change is returned to an incremental reader"
        );
        assert_eq!(changed.events[0].phase, ModelRouteTrafficPhase::Streaming);
        assert!(
            traffic
                .snapshot("u", Some(&full.cursor), Some("other-process"))
                .reset
        );
        assert!(traffic.snapshot("u", Some("not-a-number"), instance).reset);
        assert!(
            traffic.snapshot("u", Some("999999"), instance).reset,
            "a cursor from the future means a restart"
        );
        assert!(
            traffic.snapshot("other-user", None, None).events.is_empty(),
            "users never see each other"
        );
        drop(first);
    }

    #[test]
    fn model_route_traffic_capacity_evicts_oldest_and_forces_reset() {
        let traffic = Arc::new(ModelRouteTraffic::default());
        traffic.rejected("u", 1, start(None), 404, "user_model_route_not_configured");
        let snapshot = traffic.snapshot("u", None, None);
        for sequence in 2..=(PER_USER_CAPACITY as u64 + 1) {
            traffic.rejected(
                "u",
                sequence,
                start(None),
                404,
                "user_model_route_not_configured",
            );
        }
        let state = traffic.lock();
        assert_eq!(state.users["u"].entries.len(), PER_USER_CAPACITY);
        assert_eq!(state.total, PER_USER_CAPACITY);
        assert_eq!(state.users["u"].entries.front().unwrap().sequence, 2);
        drop(state);
        let next = traffic.snapshot("u", Some(&snapshot.cursor), Some(&snapshot.instance_id));
        assert!(next.reset, "the reader's cursor predates an evicted event");
        assert_eq!(next.events.len(), PER_USER_CAPACITY);
    }

    #[test]
    fn model_route_traffic_global_and_user_limits_are_bounded() {
        let traffic = Arc::new(ModelRouteTraffic::default());
        let mut sequence = 0;
        for user in 0..(GLOBAL_CAPACITY / PER_USER_CAPACITY) {
            for _ in 0..PER_USER_CAPACITY {
                sequence += 1;
                traffic.rejected(
                    &format!("u-{user}"),
                    sequence,
                    start(None),
                    404,
                    "user_model_route_not_configured",
                );
            }
        }
        assert_eq!(traffic.lock().total, GLOBAL_CAPACITY);
        traffic.rejected(
            "newcomer",
            sequence + 1,
            start(None),
            404,
            "user_model_route_not_configured",
        );
        let state = traffic.lock();
        assert_eq!(
            state.total, GLOBAL_CAPACITY,
            "the heaviest user's oldest event made room"
        );
        assert!(state.users.contains_key("newcomer"));
        drop(state);

        let crowded = Arc::new(ModelRouteTraffic::default());
        for user in 0..MAX_USERS {
            crowded.rejected(
                &format!("u-{user}"),
                user as u64,
                start(None),
                404,
                "user_model_route_not_configured",
            );
        }
        assert!(
            crowded.begin("late", 1, start(Some("r"))).is_none(),
            "a new user never evicts another user"
        );
        assert_eq!(crowded.lock().users.len(), MAX_USERS);
    }

    #[test]
    fn model_route_traffic_window_expires_ended_events_but_keeps_inflight() {
        let traffic = Arc::new(ModelRouteTraffic::default());
        let inflight = traffic.begin("u", 1, start(Some("r"))).unwrap();
        traffic.rejected("u", 2, start(None), 404, "user_model_route_not_configured");
        {
            let mut state = traffic.lock();
            let past = Instant::now() - WINDOW - Duration::from_secs(1);
            for entry in state.users.get_mut("u").unwrap().entries.iter_mut() {
                entry.started = past;
                if entry.ended.is_some() {
                    entry.ended = Some(past);
                }
            }
            state.sweep(Instant::now());
            assert_eq!(state.total, 1);
            assert_eq!(state.users["u"].entries[0].sequence, 1);
        }
        drop(inflight);
    }

    #[tokio::test]
    async fn model_route_traffic_body_wrapper_tracks_end_error_and_early_drop() {
        let traffic = Arc::new(ModelRouteTraffic::default());
        let guard = traffic.begin("u", 1, start(Some("r"))).unwrap();
        guard.headers(headers(200));
        let response = guard.attach(axum::response::Response::new(Body::from_stream(
            futures_util::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from_static(
                b"data: 1\n\n",
            ))]),
        )));
        assert_eq!(phase(&traffic, "u", 1), ModelRouteTrafficPhase::Streaming);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"data: 1\n\n", "frames pass through unchanged");
        assert_eq!(phase(&traffic, "u", 1), ModelRouteTrafficPhase::Complete);

        let guard = traffic.begin("u", 2, start(Some("r"))).unwrap();
        let response = guard.attach(axum::response::Response::new(Body::from_stream(
            futures_util::stream::iter(vec![Err::<Bytes, _>(std::io::Error::other(
                "upstream reset",
            ))]),
        )));
        assert!(response.into_body().collect().await.is_err());
        assert_eq!(phase(&traffic, "u", 2), ModelRouteTrafficPhase::Interrupted);

        let guard = traffic.begin("u", 3, start(Some("r"))).unwrap();
        let response = guard.attach(axum::response::Response::new(Body::from_stream(
            futures_util::stream::pending::<Result<Bytes, std::io::Error>>(),
        )));
        drop(response);
        assert_eq!(
            phase(&traffic, "u", 3),
            ModelRouteTrafficPhase::Interrupted,
            "caller left mid-stream"
        );

        let guard = traffic.begin("u", 4, start(Some("r"))).unwrap();
        let response = guard.attach(axum::response::Response::new(Body::from("{}")));
        drop(response);
        assert_eq!(
            phase(&traffic, "u", 4),
            ModelRouteTrafficPhase::Interrupted,
            "an unread sized body did not end"
        );
        let guard = traffic.begin("u", 5, start(Some("r"))).unwrap();
        let response = guard.attach(axum::response::Response::new(Body::from("{}")));
        let _ = response.into_body().collect().await.unwrap();
        assert_eq!(phase(&traffic, "u", 5), ModelRouteTrafficPhase::Complete);
        let guard = traffic.begin("u", 6, start(Some("r"))).unwrap();
        let _ = guard.attach(axum::response::Response::new(Body::empty()));
        assert_eq!(
            phase(&traffic, "u", 6),
            ModelRouteTrafficPhase::Complete,
            "an empty body ends immediately"
        );
    }
}
