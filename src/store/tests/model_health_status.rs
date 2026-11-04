use super::*;
use crate::models::{ShareModelHealthStatus, ShareProviderModelPolicy};

const HEALTH_FINGERPRINT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn health_result(status: &str, checked_at: i64) -> ShareModelHealthSlotResult {
    let mut result =
        model_health_slot_result(status, checked_at, "health-installation", "health-share");
    result.health_fingerprint = Some(HEALTH_FINGERPRINT.into());
    result
}

struct Fixture {
    store: AppStore,
    start: i64,
    epoch: String,
    runtimes: ShareAppRuntimes,
    support: ShareSupport,
    bindings: BTreeMap<String, String>,
}

impl Fixture {
    async fn new() -> Self {
        Self::new_at(Utc::now().timestamp() / 1800 * 1800 - 18000).await
    }

    async fn new_at(start: i64) -> Self {
        let store = AppStore::new_in_memory_for_tests().unwrap();
        insert_installation(&store, "health-installation").await;
        insert_share(
            &store,
            "health-installation",
            "health-share",
            "health-sub",
            "active",
        )
        .await;
        let input = model_probe_epoch_input("health-share", HEALTH_FINGERPRINT);
        let epoch = store
            .sync_share_model_probe_epoch("health-share", start, Some(&input))
            .await
            .unwrap()
            .unwrap();
        let mut probe = test_provider_model_probe("openai", "server-test-model");
        probe.requested_model = "server-test-model@low".into();
        probe.health_fingerprint = HEALTH_FINGERPRINT.into();
        let runtime = ShareUpstreamProvider {
            model_probe: Some(probe),
            model_policy: Some(ShareProviderModelPolicy::Single {
                upstream_model: "server-test-model".into(),
            }),
            ..Default::default()
        };
        let runtimes = ShareAppRuntimes {
            codex: Some(runtime),
            ..Default::default()
        };
        let support = ShareSupport {
            claude: true,
            codex: true,
            gemini: false,
        };
        let bindings = BTreeMap::from([
            ("claude".into(), "primary-provider".into()),
            ("codex".into(), "provider-test".into()),
        ]);
        set_share_bindings(&store, "health-share", &["claude", "codex"]).await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET app_type='claude', provider_id='primary-provider',
                    enabled_gemini=0, bindings_json=?2, app_runtimes_json=?1,
                    expires_at='2099-12-31T00:00:00Z' WHERE share_id='health-share'",
                params![
                    serde_json::to_string(&runtimes).unwrap(),
                    serde_json::to_string(&bindings).unwrap()
                ],
            )
            .unwrap();
            conn.execute(
                "UPDATE share_bindings SET provider_id='primary-provider'
                WHERE share_id='health-share' AND app_type='claude'",
                [],
            )
            .unwrap();
        }
        Self {
            store,
            start,
            epoch,
            runtimes,
            support,
            bindings,
        }
    }

    async fn claim(&self, offset: i64) -> String {
        let slot = self.start + offset * 1800;
        self.store
            .claim_share_model_health_slot(
                "health-share",
                slot,
                &self.epoch,
                "codex",
                "openai",
                "server-test-model@low",
                &format!("cc-switch-router-cycle:utc-{slot}"),
                slot,
            )
            .await
            .unwrap()
            .unwrap()
    }

    async fn push(&self, offset: i64, status: &str) {
        let slot = self.start + offset * 1800;
        let claim = self.claim(offset).await;
        let mut result = health_result(status, slot + 10);
        result.source = format!("cc-switch-router-cycle:utc-{slot}");
        if status == "failed" {
            result.status_code = Some(429);
            result.reason_code = Some("upstream_rate_limited".into());
            result.error_message =
                Some("fixture-secret: upstream account private@example.test".into());
        } else if status == "degraded" {
            result.reason_code = Some("probe_degraded".into());
        }
        self.store
            .finish_share_model_health_slot("health-share", slot, &claim, result)
            .await
            .unwrap();
    }

    async fn status(&self, offset: i64) -> ShareModelHealthStatus {
        let conn = self.store.conn.lock().await;
        model_health::current_status(
            &conn,
            "health-share",
            &self.support,
            &self.runtimes,
            &self.bindings,
            Utc.timestamp_opt(self.start + offset * 1800 + 11, 0)
                .unwrap(),
        )
        .unwrap()
    }
}

#[tokio::test]
async fn canonical_health_is_yellow_then_red_and_recovers_without_using_primary_app() {
    let f = Fixture::new().await;
    assert_eq!(f.status(0).await.state, "unknown");
    for offset in 0..3 {
        f.push(offset, "failed").await;
        let status = f.status(offset).await;
        assert_eq!(status.app_type.as_deref(), Some("codex"));
        assert_eq!(status.status_code, Some(429));
        assert_eq!(
            status.state,
            if offset < 2 {
                "degraded"
            } else {
                "unavailable"
            }
        );
    }
    for offset in 3..6 {
        f.push(offset, "success").await;
        let status = f.status(offset).await;
        assert_eq!(
            status.state,
            if offset < 5 { "degraded" } else { "healthy" }
        );
        if offset < 5 {
            assert_eq!(status.reason_code, "probe_recovering");
        }
    }
    f.push(6, "degraded").await;
    let status = f.status(6).await;
    assert_eq!(status.state, "degraded");
    assert_eq!(status.reason_code, "probe_degraded");
}

#[tokio::test]
async fn canonical_health_is_unknown_when_stale_or_provider_model_fingerprint_changes() {
    let mut f = Fixture::new().await;
    f.push(0, "success").await;
    assert_eq!(f.status(0).await.state, "healthy");
    assert_eq!(f.status(3).await.reason_code, "probe_stale");
    f.runtimes
        .codex
        .as_mut()
        .unwrap()
        .model_probe
        .as_mut()
        .unwrap()
        .health_fingerprint = "new-profile".into();
    assert_eq!(f.status(0).await.reason_code, "awaiting_probe");
    f.runtimes
        .codex
        .as_mut()
        .unwrap()
        .model_probe
        .as_mut()
        .unwrap()
        .health_fingerprint = HEALTH_FINGERPRINT.into();
    let original = f.runtimes.codex.as_ref().unwrap().clone();
    f.runtimes
        .codex
        .as_mut()
        .unwrap()
        .model_probe
        .as_mut()
        .unwrap()
        .requested_model = "new-model".into();
    assert_eq!(f.status(0).await.state, "unknown");
    f.runtimes.codex = Some(original.clone());
    f.runtimes.codex.as_mut().unwrap().model_policy = Some(ShareProviderModelPolicy::Passthrough);
    assert_eq!(f.status(0).await.state, "unknown");
    f.runtimes.codex = Some(original);
    f.bindings.insert("codex".into(), "another-provider".into());
    assert_eq!(f.status(0).await.state, "unknown");
    f.support.codex = false;
    assert_eq!(f.status(0).await.reason_code, "not_monitored");
}

#[tokio::test]
async fn canonical_health_ignores_runtime_success_overwrite_and_repairs_historical_read_projection()
{
    let f = Fixture::new().await;
    for offset in 0..3 {
        f.push(offset, "failed").await;
    }
    let checked_at = f.start + 2 * 1800 + 10;
    let snapshot = ShareRuntimeSnapshotResponse {
        share_id: "health-share".into(),
        queried_at: checked_at + 20,
        token_limit: None,
        tokens_used: None,
        requests_count: None,
        share_status: None,
        support: f.support.clone(),
        app_runtimes: ShareAppRuntimes::default(),
        app_providers: ShareAppProviders::default(),
        model_health: ShareModelHealthSummary {
            codex: vec![ModelHealthSummary {
                app_type: "codex".into(),
                requested_model: "server-test-model@low".into(),
                actual_model: "server-test-model".into(),
                status: "success".into(),
                last_checked_at: Some(checked_at),
                recent_results: vec!["success".into(); 3],
                status_code: Some(429),
                source: Some(format!("cc-switch-router-cycle:utc-{}", checked_at - 10)),
                last_success_at: None,
                last_failed_at: None,
                error_message: None,
                latency_ms: 0,
                provider_id: None,
                provider_name: None,
            }],
            ..Default::default()
        },
    };
    {
        let conn = f.store.conn.lock().await;
        record_runtime_model_health_snapshot_conn(&conn, &snapshot).unwrap();
        let last: String = conn
            .query_row(
                "SELECT last_status FROM share_model_health_state WHERE share_id='health-share'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(last, "failed");
        // Simulate the already-deployed bug. The new read projection must use
        // the immutable observation without rewriting the historical evidence.
        conn.execute(
            "UPDATE share_model_health_checks SET status='success', source='cc-switch-scheduled'
             WHERE checked_at=?1",
            params![checked_at],
        )
        .unwrap();
        let checks = list_recent_share_model_health_checks(&conn, 10).unwrap();
        assert_eq!(checks[0].status, "failed");
        assert_eq!(checks[0].status_code, Some(429));
        assert_eq!(
            checks[0].source,
            format!("cc-switch-router-cycle:utc-{}", checked_at - 10)
        );
        let mut empty = snapshot.clone();
        empty.model_health = ShareModelHealthSummary::default();
        record_runtime_model_health_snapshot_conn(&conn, &empty).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM share_model_health_state WHERE share_id='health-share'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
    assert_eq!(f.status(2).await.state, "unavailable");
}

#[tokio::test]
async fn canonical_health_current_epoch_does_not_reuse_earlier_same_config_failures() {
    let f = Fixture::new().await;
    f.push(0, "failed").await;
    f.store
        .sync_share_model_probe_epoch("health-share", f.start + 1800, None)
        .await
        .unwrap();
    f.store
        .sync_share_model_probe_epoch(
            "health-share",
            f.start + 3600,
            Some(&model_probe_epoch_input("health-share", HEALTH_FINGERPRINT)),
        )
        .await
        .unwrap();
    assert_eq!(f.status(2).await.reason_code, "awaiting_probe");
}

#[tokio::test]
async fn canonical_health_expires_at_the_exact_freshness_boundary() {
    let f = Fixture::new().await;
    f.push(0, "success").await;
    let checked_at = f.start + 10;
    let conn = f.store.conn.lock().await;
    for (age, expected) in [(3899, "healthy"), (3900, "unknown"), (3901, "unknown")] {
        let status = model_health::current_status(
            &conn,
            "health-share",
            &f.support,
            &f.runtimes,
            &f.bindings,
            Utc.timestamp_opt(checked_at + age, 0).unwrap(),
        )
        .unwrap();
        assert_eq!(status.state, expected);
        assert_eq!(status.stale_at, Some(checked_at + 3900));
    }
}

#[tokio::test]
async fn canonical_health_does_not_count_monitoring_gaps_or_pending_claims() {
    let f = Fixture::new().await;
    f.push(0, "failed").await;
    f.push(1, "failed").await;
    let claim = f.claim(2).await;
    let mut gap = health_result("failed", f.start + 3610);
    gap.observation_id = None;
    gap.outcome = "unobserved".into();
    gap.failure_domain = Some("control_transport".into());
    gap.reason_code = Some("control_transport_failed".into());
    gap.evidence_scope = "share_projection".into();
    f.store
        .finish_share_model_health_slot("health-share", f.start + 3600, &claim, gap)
        .await
        .unwrap();
    let pending = f.claim(3).await;
    let status = f.status(3).await;
    assert_eq!(status.state, "degraded");
    assert_eq!(status.recent_results, ["failed", "failed"]);
    let mut failure = health_result("failed", f.start + 5410);
    failure.source = format!("cc-switch-router-cycle:utc-{}", f.start + 5400);
    f.store
        .finish_share_model_health_slot("health-share", f.start + 5400, &pending, failure)
        .await
        .unwrap();
    assert_eq!(f.status(3).await.state, "unavailable");
}

#[tokio::test]
async fn canonical_health_does_not_inherit_late_results_from_a_closed_epoch() {
    let mut f = Fixture::new().await;
    let old_claim = f.claim(0).await;
    let input = model_probe_epoch_input("health-share", "new-profile");
    f.store
        .sync_share_model_probe_epoch("health-share", f.start + 1800, Some(&input))
        .await
        .unwrap();
    f.runtimes
        .codex
        .as_mut()
        .unwrap()
        .model_probe
        .as_mut()
        .unwrap()
        .health_fingerprint = "new-profile".into();
    let mut late = health_result("failed", f.start + 1810);
    late.source = format!("cc-switch-router-cycle:utc-{}", f.start);
    f.store
        .finish_share_model_health_slot("health-share", f.start, &old_claim, late)
        .await
        .unwrap();
    assert_eq!(f.status(1).await.reason_code, "awaiting_probe");
    assert!(f.status(1).await.recent_results.is_empty());
}

#[tokio::test]
async fn canonical_history_and_snapshot_guard_respect_permitted_pre_slot_clock_skew() {
    let f = Fixture::new().await;
    let claim = f.claim(0).await;
    let mut result = health_result("failed", f.start - 120);
    result.source = format!("cc-switch-router-cycle:utc-{}", f.start);
    f.store
        .finish_share_model_health_slot("health-share", f.start, &claim, result)
        .await
        .unwrap();
    let conn = f.store.conn.lock().await;
    let check = list_recent_share_model_health_checks(&conn, 10)
        .unwrap()
        .remove(0);
    assert!(model_health::has_canonical_check(&conn, &check).unwrap());
    conn.execute("UPDATE share_model_health_checks SET status='success'", [])
        .unwrap();
    let restored = list_recent_share_model_health_checks(&conn, 10).unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].status, "failed");
}

#[tokio::test]
async fn dashboard_market_and_share_url_expose_the_same_health_without_public_error_bodies() {
    let start = (Utc::now().timestamp() - 11) / 1800 * 1800 - 3600;
    let f = Fixture::new_at(start).await;
    let config = test_config("model-health-projections");
    insert_public_listing(
        &f.store,
        "health-listing",
        "health-share",
        "health-installation",
        "active",
    )
    .await;
    {
        let conn = f.store.conn.lock().await;
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO share_market_seats (id, listing_id, position, status, token_period_json, created_at, updated_at)
             VALUES ('health-seat', 'health-listing', 0, 'available', ?1, ?2, ?2)",
            params![serde_json::to_string(&ShareTokenPeriod::Day).unwrap(), now],
        ).unwrap();
    }
    let proxy = ProxyRegistry::default();
    proxy
        .set_route(
            "health-sub".into(),
            "http://127.0.0.1:1".into(),
            None,
            Some("health-share".into()),
            Some("Share".into()),
            false,
            -1,
            None,
        )
        .await;
    f.push(0, "failed").await;
    for offset in 1..3 {
        f.push(offset, "failed").await;
        let dashboard = f
            .store
            .dashboard_snapshot(
                &config,
                &ServerGeo {
                    lat: None,
                    lon: None,
                },
                &proxy,
                Some("owner@example.com"),
            )
            .await
            .unwrap();
        let share = &dashboard.shares[0];
        let catalog = f
            .store
            .share_market_catalog(None, &["health-sub".into()])
            .await
            .unwrap();
        assert_eq!(catalog.listings.len(), 1);
        assert_eq!(
            catalog.listings[0].model_health_status,
            share.model_health_status
        );
        let direct = f
            .store
            .share_view_for_share_url(
                "health-share",
                &HashSet::from(["health-sub".into()]),
                &HashMap::new(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(direct.model_health_status, share.model_health_status);
        let expected_reason = if offset < 2 {
            "provider_degraded"
        } else {
            "provider_unavailable"
        };
        let reason = share.operational_summary.primary_reason.as_ref().unwrap();
        assert_eq!(reason.code, expected_reason);
        assert_eq!(reason.entity_id.as_deref(), Some("provider-test"));
        assert_eq!(share.service_readiness.ready, offset < 2);
        let public = serde_json::to_string(&catalog.listings[0].model_health_status).unwrap();
        assert!(!public.contains("fixture-secret"));
        assert!(!public.contains("private@example.test"));
    }
}
