//! In-memory tracker for recent proxy requests, keyed by user-origin country.
//!
//! Two views are exposed via [`RecentTraffic::snapshot`]:
//! - **`country_counts`** — ISO 3166-1 alpha-3 → request count over the last
//!   [`COUNTRY_WINDOW`]. Drives the dashboard's "demand" overlay.
//! - **`recent_events`** — fresh request events. Includes anything that started
//!   within [`TICKER_WINDOW_SECS`] seconds *and* anything still in-flight,
//!   regardless of age — so a long-running stream stays visible until it ends.
//!   Each event carries an `is_inflight` flag the frontend uses to drive
//!   row-level state transitions (insert / hold / retire).
//!
//! No persistence — the tracker lives inside `ServerState` and resets on restart.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::geo::iso2_to_iso3;

/// Sliding window for country count aggregation.
const COUNTRY_WINDOW_SECS: i64 = 5 * 60;
/// Freshness window for dashboard ticker request events.
const TICKER_WINDOW_SECS: i64 = 8;
/// Maximum number of events held in the ring buffer.
const MAX_EVENTS: usize = 64;
/// Hard cap on retained event records — protects memory under sustained spikes.
const MAX_RETAINED: usize = 4096;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentRequestEvent {
    pub request_id: String,
    pub share_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_subdomain: Option<String>,
    /// ISO 3166-1 alpha-2 country code of the end user (`cf-ipcountry`).
    /// `None` when the connecting peer was not Cloudflare-trusted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_country: Option<String>,
    /// Same value translated to alpha-3, ready to address SVG country paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_country_iso3: Option<String>,
    pub started_at: DateTime<Utc>,
    /// True while the underlying proxy request is still streaming. Stamped at
    /// snapshot time from the `inflight_request_ids` set, not stored on the
    /// event itself.
    pub is_inflight: bool,
}

#[derive(Default, Debug)]
struct State {
    events: VecDeque<RecentRequestEvent>,
    /// Request ids that have been recorded but not yet completed. Used both
    /// for snapshot-time stamping and to decide whether a stale-by-window
    /// event should still be retained.
    inflight_request_ids: HashSet<String>,
}

#[derive(Debug, Default, Clone)]
pub struct RecentTraffic {
    inner: Arc<RwLock<State>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecentTrafficSnapshot {
    pub country_counts: HashMap<String, usize>,
    pub events: Vec<RecentRequestEvent>,
    pub recent_events: Vec<RecentRequestEvent>,
}

impl RecentTraffic {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a request entering the proxy. Cheap: one write lock + a couple of
    /// VecDeque ops. The returned id is also stored as in-flight; call
    /// [`Self::complete`] when the upstream response stream ends so the ticker
    /// can retire the row.
    pub async fn record(
        &self,
        share_id: String,
        share_name: Option<String>,
        share_subdomain: Option<String>,
        user_country_iso2: Option<String>,
    ) -> String {
        let request_id = Uuid::new_v4().to_string();
        let user_country_iso3 = user_country_iso2
            .as_deref()
            .and_then(iso2_to_iso3)
            .map(str::to_string);
        let event = RecentRequestEvent {
            request_id: request_id.clone(),
            share_id,
            share_name,
            share_subdomain,
            user_country: user_country_iso2,
            user_country_iso3,
            started_at: Utc::now(),
            is_inflight: true,
        };
        let mut state = self.inner.write().await;
        state.inflight_request_ids.insert(request_id.clone());
        state.events.push_back(event);
        // Hard size cap; window-based pruning happens at snapshot time. If the
        // oldest event we drop here happens to still be in-flight (extreme
        // sustained load), forget the in-flight bit too — keeping it around
        // would leak memory once the matching `complete` arrives but the event
        // is already gone.
        while state.events.len() > MAX_RETAINED {
            if let Some(dropped) = state.events.pop_front() {
                state.inflight_request_ids.remove(&dropped.request_id);
            }
        }
        request_id
    }

    /// Mark a previously-recorded request as no longer in-flight. The matching
    /// event stays in the ring (so it can still appear in the ticker briefly
    /// during its `leaving` animation) until the freshness window evicts it.
    pub async fn complete(&self, request_id: &str) {
        let mut state = self.inner.write().await;
        state.inflight_request_ids.remove(request_id);
    }

    /// Build the dashboard payload. Walks the ring once to compute country counts and
    /// pick the latest events; also drops anything older than the country window so
    /// the buffer cannot grow without bound under quiet periods. In-flight events
    /// are pinned: they survive past the window and are stamped `is_inflight=true`
    /// so the frontend keeps their ticker rows mounted until completion.
    pub async fn snapshot(&self) -> RecentTrafficSnapshot {
        let country_cutoff = Utc::now() - Duration::seconds(COUNTRY_WINDOW_SECS);
        let ticker_cutoff = Utc::now() - Duration::seconds(TICKER_WINDOW_SECS);
        let mut state = self.inner.write().await;
        // Compact: drop events that are both stale AND not currently in-flight.
        // Stop at the first event we want to keep so this stays O(evicted).
        while let Some(front) = state.events.front() {
            let is_inflight = state.inflight_request_ids.contains(&front.request_id);
            if front.started_at < country_cutoff && !is_inflight {
                state.events.pop_front();
            } else {
                break;
            }
        }
        let mut country_counts: HashMap<String, usize> = HashMap::new();
        for event in state.events.iter() {
            if let Some(iso3) = event.user_country_iso3.as_deref() {
                *country_counts.entry(iso3.to_string()).or_insert(0) += 1;
            }
        }
        let events: Vec<_> = state
            .events
            .iter()
            .map(|event| {
                let mut event = event.clone();
                event.is_inflight = state.inflight_request_ids.contains(&event.request_id);
                event
            })
            .collect();
        let recent_events: Vec<_> = events
            .iter()
            .filter_map(|event| {
                if !event.is_inflight && event.started_at < ticker_cutoff {
                    return None;
                }
                Some(event.clone())
            })
            .collect();
        let take_from = recent_events.len().saturating_sub(MAX_EVENTS);
        let recent_events = recent_events.into_iter().skip(take_from).collect();
        RecentTrafficSnapshot {
            country_counts,
            events,
            recent_events,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn record(traffic: &RecentTraffic, share: &str, country: Option<&str>) -> String {
        traffic
            .record(
                share.to_string(),
                None,
                None,
                country.map(|c| c.to_string()),
            )
            .await
    }

    #[tokio::test]
    async fn record_then_snapshot_aggregates_iso3() {
        let traffic = RecentTraffic::new();
        record(&traffic, "share-1", Some("US")).await;
        record(&traffic, "share-1", Some("us")).await; // case test
        record(&traffic, "share-2", Some("DE")).await;
        record(&traffic, "share-2", None).await;
        let snap = traffic.snapshot().await;
        // "us" lowercase won't hit iso2_to_iso3 (which expects uppercase).
        assert_eq!(snap.country_counts.get("USA"), Some(&1));
        assert_eq!(snap.country_counts.get("DEU"), Some(&1));
        assert_eq!(snap.recent_events.len(), 4);
        assert!(snap.recent_events.iter().all(|e| e.is_inflight));
    }

    #[tokio::test]
    async fn ring_caps_at_max_events() {
        let traffic = RecentTraffic::new();
        for _ in 0..100 {
            record(&traffic, "s", Some("US")).await;
        }
        let snap = traffic.snapshot().await;
        assert_eq!(snap.recent_events.len(), MAX_EVENTS);
    }

    #[tokio::test]
    async fn inflight_event_survives_ticker_window() {
        let traffic = RecentTraffic::new();
        let request_id = record(&traffic, "share-x", Some("US")).await;
        // Backdate the event to simulate a long-running request older than the
        // ticker freshness window.
        {
            let mut state = traffic.inner.write().await;
            for event in state.events.iter_mut() {
                if event.request_id == request_id {
                    event.started_at = Utc::now() - Duration::seconds(TICKER_WINDOW_SECS + 60);
                }
            }
        }
        let snap = traffic.snapshot().await;
        let event = snap
            .recent_events
            .iter()
            .find(|e| e.request_id == request_id)
            .expect("inflight event must remain in ticker output");
        assert!(event.is_inflight);
    }

    #[tokio::test]
    async fn complete_then_window_evicts_event() {
        let traffic = RecentTraffic::new();
        let request_id = record(&traffic, "share-x", Some("US")).await;
        traffic.complete(&request_id).await;
        // Right after complete, the event is still fresh — it should appear
        // once with is_inflight=false so the frontend can play its leaving
        // animation.
        let snap = traffic.snapshot().await;
        let event = snap
            .recent_events
            .iter()
            .find(|e| e.request_id == request_id)
            .expect("event should still be in ticker output during leaving window");
        assert!(!event.is_inflight);

        // Backdate past the ticker window — now the snapshot should drop it.
        {
            let mut state = traffic.inner.write().await;
            for event in state.events.iter_mut() {
                if event.request_id == request_id {
                    event.started_at = Utc::now() - Duration::seconds(TICKER_WINDOW_SECS + 60);
                }
            }
        }
        let snap = traffic.snapshot().await;
        assert!(
            !snap
                .recent_events
                .iter()
                .any(|e| e.request_id == request_id),
            "completed + stale event must be evicted"
        );
    }

    #[tokio::test]
    async fn complete_unknown_id_is_noop() {
        let traffic = RecentTraffic::new();
        // Should not panic or affect existing in-flight set.
        traffic.complete("nope").await;
        let id = record(&traffic, "share-x", Some("US")).await;
        traffic.complete("still-nope").await;
        let snap = traffic.snapshot().await;
        let event = snap
            .recent_events
            .iter()
            .find(|e| e.request_id == id)
            .unwrap();
        assert!(event.is_inflight);
    }
}
