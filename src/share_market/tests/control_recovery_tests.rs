use super::*;

async fn pending_revoke() -> (
    AppStore,
    AuthSession,
    AuthSession,
    String,
    String,
    DateTime<Utc>,
) {
    let store = AppStore::new_in_memory_for_tests().unwrap();
    let owner = session("recovery-owner", "recovery-owner@example.com");
    let buyer = session("recovery-buyer", "recovery-buyer@example.com");
    insert_share(
        &store,
        "recovery-share",
        &owner.email,
        &[ShareTokenPeriod::Day],
    )
    .await;
    let (_, seat) = create_listing(&store, &owner, "recovery-share", free_seat()).await;
    let sub = store
        .rent_fixture_share(&buyer, &seat, RentSeatRequest { offer_revision: 1 })
        .await
        .unwrap();
    let now = Utc::now();
    assert_eq!(
        store
            .share_market_reconcile_and_dispatch(now)
            .await
            .unwrap()
            .len(),
        1
    );
    // The initial grant reached the Server but has not been confirmed. An
    // empty descriptor must not make this return finish without a revoke.
    store
        .share_market_request_release(&buyer, &sub, false, false)
        .await
        .unwrap();
    assert_eq!(
        store
            .share_market_reconcile_and_dispatch(now + Duration::seconds(1))
            .await
            .unwrap()
            .len(),
        1
    );
    (store, owner, buyer, sub, seat, now)
}

fn revoke_context(conn: &Connection, sub: &str) -> (String, String, String) {
    conn.query_row("SELECT operation.id, operation.edit_id, edit.patch_json
        FROM share_control_operations operation JOIN share_edit_requests edit ON edit.id=operation.edit_id
        WHERE operation.subscription_id=?1 AND operation.action='revoke'", params![sub],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap()
}

fn reject_timeout(conn: &Connection, edit: &str, now: DateTime<Utc>) {
    conn.execute("UPDATE share_edit_requests SET status='rejected',error_code='control_ack_timeout' WHERE id=?1", params![edit]).unwrap();
    handle_control_edit_ack_with_metadata(
        conn,
        edit,
        "rejected",
        Some("timeout"),
        Some("control_ack_timeout"),
        Some(true),
        &now.to_rfc3339(),
    )
    .unwrap();
}

#[tokio::test]
async fn verified_sync_wakes_the_last_revoke_attempt_without_releasing_early_or_resetting_budget() {
    let (store, _, _, sub, seat, now) = pending_revoke().await;
    let failed_at = now + Duration::seconds(CONTROL_EDIT_TTL_SECS + 2);
    let wake_at = failed_at + Duration::seconds(30);
    let (operation, edit, original) = {
        let conn = store.conn.lock().await;
        let context = revoke_context(&conn, &sub);
        conn.execute(
            "UPDATE share_control_operations SET attempts=7 WHERE id=?1",
            params![context.0],
        )
        .unwrap();
        context
    };
    // Exercise actual edit expiration, including clearing the active edit_id.
    assert!(
        store
            .share_market_reconcile_and_dispatch(failed_at)
            .await
            .unwrap()
            .is_empty()
    );
    {
        let conn = store.conn.lock().await;
        let context: (Option<String>, String, Option<String>, i64) = conn.query_row(
            "SELECT edit_id,next_attempt_at,last_attempt_edit_id,attempts FROM share_control_operations WHERE id=?1", params![operation],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        assert_eq!(context.0, None);
        assert_eq!(context.2.as_deref(), Some(edit.as_str()));
        assert_eq!(context.3, 7);
        assert_eq!(
            parse_time(&context.1).unwrap(),
            failed_at + Duration::seconds(CONTROL_RETRY_MAX_SECS)
        );
        conn.execute(
            "UPDATE shares SET config_revision=2 WHERE share_id='recovery-share'",
            [],
        )
        .unwrap();
        wake_recovered_revocations_tx(
            &conn,
            "installation-recovery-share",
            "recovery-share",
            2,
            wake_at,
        )
        .unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT attempts FROM share_control_operations WHERE id=?1",
                params![operation],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            7
        );
    }
    assert_eq!(subscription_status(&store, &sub).await, SUB_REVOKE_PENDING);
    assert_eq!(
        store
            .share_market_reconcile_and_dispatch(wake_at)
            .await
            .unwrap()
            .len(),
        1
    );
    {
        let conn = store.conn.lock().await;
        let (_, new_edit, replay) = revoke_context(&conn, &sub);
        assert_ne!(new_edit, edit);
        assert_eq!(
            replay, original,
            "uncertain results replay an identical payload"
        );
        assert_eq!(
            conn.query_row(
                "SELECT attempts FROM share_control_operations WHERE id=?1",
                params![operation],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            8
        );
        conn.execute(
            "UPDATE share_edit_requests SET status='applied' WHERE id=?1",
            params![new_edit],
        )
        .unwrap();
        handle_control_edit_ack(&conn, &new_edit, "applied", None, &wake_at.to_rfc3339()).unwrap();
        handle_control_edit_ack(&conn, &new_edit, "applied", None, &wake_at.to_rfc3339()).unwrap();
    }
    store
        .share_market_reconcile_and_dispatch(wake_at + Duration::seconds(1))
        .await
        .unwrap();
    assert_eq!(subscription_status(&store, &sub).await, SUB_RELEASED);
    let conn = store.conn.lock().await;
    assert_eq!(
        conn.query_row(
            "SELECT status FROM share_market_seats WHERE id=?1",
            params![seat],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        SEAT_AVAILABLE
    );
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM share_market_events WHERE subscription_id=?1 AND event_type='subscription_released'", params![sub], |r|r.get::<_,i64>(0)).unwrap(), 1);
}

#[tokio::test]
async fn unknown_replays_are_immutable_until_explicit_revision_conflict_allows_rebase() {
    let (store, _, _, sub, _, now) = pending_revoke().await;
    let (operation, original) = {
        let conn = store.conn.lock().await;
        let (operation, edit, original) = revoke_context(&conn, &sub);
        reject_timeout(&conn, &edit, now + Duration::seconds(2));
        conn.execute(
            "UPDATE shares SET config_revision=2 WHERE share_id='recovery-share'",
            [],
        )
        .unwrap();
        (operation, original)
    };
    assert_eq!(
        store
            .share_market_reconcile_and_dispatch(now + Duration::seconds(18))
            .await
            .unwrap()
            .len(),
        1
    );
    {
        let conn = store.conn.lock().await;
        let (_, edit, replay) = revoke_context(&conn, &sub);
        assert_eq!(replay, original);
        conn.execute(
            "UPDATE share_edit_requests SET status='rejected',error_code=?2 WHERE id=?1",
            params![edit, SHARE_REVISION_CONFLICT_CODE],
        )
        .unwrap();
        handle_control_edit_ack_with_metadata(
            &conn,
            &edit,
            "rejected",
            Some("revision conflict"),
            Some(SHARE_REVISION_CONFLICT_CODE),
            Some(true),
            &(now + Duration::seconds(19)).to_rfc3339(),
        )
        .unwrap();
    }
    assert_eq!(
        store
            .share_market_reconcile_and_dispatch(now + Duration::seconds(20))
            .await
            .unwrap()
            .len(),
        1
    );
    let conn = store.conn.lock().await;
    let (_, _, rebased) = revoke_context(&conn, &sub);
    let old: ShareSettingsPatch = serde_json::from_str(&original).unwrap();
    let mut expected = old.clone();
    expected
        .managed_grant
        .as_mut()
        .unwrap()
        .expected_config_revision = 2;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&rebased).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert_eq!(
        conn.query_row(
            "SELECT attempts FROM share_control_operations WHERE id=?1",
            params![operation],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        3
    );
}

#[tokio::test]
async fn recovery_is_bounded_scoped_and_does_not_resurrect_terminal_work() {
    let (store, _, _, sub, _, now) = pending_revoke().await;
    let conn = store.conn.lock().await;
    let (operation, edit, _) = revoke_context(&conn, &sub);
    reject_timeout(&conn, &edit, now + Duration::seconds(2));
    let long_retry = (now + Duration::minutes(15)).to_rfc3339();
    for (installation, share, revision, status, attempts, dead, code, rental_status) in [
        (
            "wrong",
            "recovery-share",
            2,
            "pending",
            1,
            None,
            "control_ack_timeout",
            SUB_REVOKE_PENDING,
        ),
        (
            "installation-recovery-share",
            "wrong",
            2,
            "pending",
            1,
            None,
            "control_ack_timeout",
            SUB_REVOKE_PENDING,
        ),
        (
            "installation-recovery-share",
            "recovery-share",
            1,
            "pending",
            1,
            None,
            "control_ack_timeout",
            SUB_REVOKE_PENDING,
        ),
        (
            "installation-recovery-share",
            "recovery-share",
            2,
            "applied",
            1,
            None,
            "control_ack_timeout",
            SUB_REVOKE_PENDING,
        ),
        (
            "installation-recovery-share",
            "recovery-share",
            2,
            "pending",
            8,
            None,
            "control_ack_timeout",
            SUB_REVOKE_PENDING,
        ),
        (
            "installation-recovery-share",
            "recovery-share",
            2,
            "pending",
            1,
            Some("dead"),
            "control_ack_timeout",
            SUB_REVOKE_PENDING,
        ),
        (
            "installation-recovery-share",
            "recovery-share",
            2,
            "pending",
            1,
            None,
            "contract_violation",
            SUB_REVOKE_PENDING,
        ),
        (
            "installation-recovery-share",
            "recovery-share",
            2,
            "pending",
            1,
            None,
            "control_ack_timeout",
            SUB_RELEASED,
        ),
    ] {
        conn.execute("UPDATE share_control_operations SET status=?2,attempts=?3,dead_lettered_at=?4,last_error_code=?5,next_attempt_at=?6,last_recovery_revision=NULL WHERE id=?1", params![operation,status,attempts,dead,code,long_retry]).unwrap();
        conn.execute(
            "UPDATE share_market_subscriptions SET status=?2 WHERE id=?1",
            params![sub, rental_status],
        )
        .unwrap();
        wake_recovered_revocations_tx(
            &conn,
            installation,
            share,
            revision,
            now + Duration::seconds(3),
        )
        .unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT next_attempt_at FROM share_control_operations WHERE id=?1",
                params![operation],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            long_retry
        );
    }
    conn.execute(
        "UPDATE share_market_subscriptions SET status='revoke_pending' WHERE id=?1",
        params![sub],
    )
    .unwrap();
    wake_recovered_revocations_tx(
        &conn,
        "installation-recovery-share",
        "recovery-share",
        2,
        now + Duration::seconds(3),
    )
    .unwrap();
    let earliest = now + Duration::seconds(1 + CONTROL_RETRY_BASE_SECS);
    assert_eq!(
        conn.query_row(
            "SELECT next_attempt_at FROM share_control_operations WHERE id=?1",
            params![operation],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        earliest.to_rfc3339()
    );
    conn.execute(
        "UPDATE share_control_operations SET next_attempt_at=?2 WHERE id=?1",
        params![operation, long_retry],
    )
    .unwrap();
    wake_recovered_revocations_tx(
        &conn,
        "installation-recovery-share",
        "recovery-share",
        2,
        now + Duration::seconds(4),
    )
    .unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT next_attempt_at FROM share_control_operations WHERE id=?1",
            params![operation],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        long_retry,
        "same evidence cannot wake twice, including after reopening the database"
    );
}

#[tokio::test]
async fn malformed_retry_times_do_not_reject_valid_recovery_evidence_or_reset_work() {
    let (store, _, _, sub, _, now) = pending_revoke().await;
    let conn = store.conn.lock().await;
    let (operation, edit, _) = revoke_context(&conn, &sub);
    reject_timeout(&conn, &edit, now + Duration::seconds(2));
    for (dispatched, retry) in [
        (
            "not-a-clock".into(),
            (now + Duration::minutes(15)).to_rfc3339(),
        ),
        (now.to_rfc3339(), "zz-invalid-retry".into()),
    ] {
        conn.execute(
            "UPDATE share_edit_requests SET created_at=?2 WHERE id=?1",
            params![edit, dispatched],
        )
        .unwrap();
        conn.execute(
            "UPDATE share_control_operations SET next_attempt_at=?2 WHERE id=?1",
            params![operation, retry],
        )
        .unwrap();
        wake_recovered_revocations_tx(
            &conn,
            "installation-recovery-share",
            "recovery-share",
            2,
            now + Duration::seconds(3),
        )
        .unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT attempts,next_attempt_at,last_recovery_revision FROM share_control_operations WHERE id=?1",
                params![operation],
                |r| Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<i64>>(2)?)),
            ).unwrap(),
            (1, retry, None),
        );
    }
}

#[tokio::test]
async fn recovery_progress_is_private_and_distinguishes_retry_drain_and_operator_work() {
    let (store, owner, buyer, sub, _, now) = pending_revoke().await;
    let conn = store.conn.lock().await;
    let (operation, edit, _) = revoke_context(&conn, &sub);
    reject_timeout(&conn, &edit, now + Duration::seconds(2));
    let record = subscription_record(&conn, &sub).unwrap().unwrap();
    for viewer in [Some(&owner), Some(&buyer)] {
        let view = subscription_view(&conn, record.clone(), viewer, &HashSet::new(), None).unwrap();
        assert_eq!(view.control_progress.unwrap().stage, "retry_scheduled");
    }
    let outsider = session("outsider", "outsider@example.com");
    let mut public = subscription_view(
        &conn,
        record.clone(),
        Some(&outsider),
        &HashSet::new(),
        None,
    )
    .unwrap();
    assert!(public.control_progress.is_none());
    let private =
        subscription_view(&conn, record.clone(), Some(&buyer), &HashSet::new(), None).unwrap();
    public.control_progress = private.control_progress;
    redact_public_subscription(&mut public);
    assert!(
        serde_json::to_value(public)
            .unwrap()
            .get("controlProgress")
            .is_none()
    );
    conn.execute("UPDATE share_control_operations SET status='dispatched',last_error='final consumption evidence pending' WHERE id=?1", params![operation]).unwrap();
    assert_eq!(
        control_recovery::progress_tx(&conn, &record)
            .unwrap()
            .unwrap()
            .stage,
        "awaiting_final_usage"
    );
    conn.execute(
        "UPDATE share_control_operations SET status='rejected',dead_lettered_at='dead' WHERE id=?1",
        params![operation],
    )
    .unwrap();
    assert_eq!(
        control_recovery::progress_tx(&conn, &record)
            .unwrap()
            .unwrap()
            .stage,
        "manual_intervention"
    );
}

#[tokio::test]
async fn scheduled_value_policy_never_confirms_initial_access() {
    let store = AppStore::new_in_memory_for_tests().unwrap();
    let owner = session("schedule-owner", "schedule-owner@example.com");
    let buyer = session("schedule-buyer", "schedule-buyer@example.com");
    insert_share(
        &store,
        "schedule-share",
        &owner.email,
        &[ShareTokenPeriod::Day],
    )
    .await;
    let (_, seat) = create_listing(&store, &owner, "schedule-share", free_seat()).await;
    let sub = store
        .rent_fixture_share(&buyer, &seat, RentSeatRequest { offer_revision: 1 })
        .await
        .unwrap();
    let now = Utc::now();
    store
        .share_market_reconcile_and_dispatch(now)
        .await
        .unwrap();
    set_entitlement(&store, &sub, now).await;
    {
        let conn = store.conn.lock().await;
        let json: String = conn
            .query_row(
                "SELECT user_grants_json FROM shares WHERE share_id='schedule-share'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut grants: BTreeMap<String, ShareUserGrant> = serde_json::from_str(&json).unwrap();
        let grant = grants.get_mut(&buyer.email).unwrap();
        grant.pending_value_quota = Some(crate::models::PendingValueQuota {
            policy: grant.policy.clone(),
            effective_from_ms: (now + Duration::minutes(1)).timestamp_millis(),
            retain_access: false,
        });
        conn.execute(
            "UPDATE shares SET user_grants_json=?1 WHERE share_id='schedule-share'",
            params![serde_json::to_string(&grants).unwrap()],
        )
        .unwrap();
    }
    store
        .share_market_reconcile_and_dispatch(now)
        .await
        .unwrap();
    assert_eq!(subscription_status(&store, &sub).await, SUB_GRANT_PENDING);
}

#[tokio::test]
async fn missing_original_request_requires_operator_review_instead_of_guessing_a_payload() {
    let (store, _, _, sub, _, now) = pending_revoke().await;
    {
        let conn = store.conn.lock().await;
        let (operation, edit, _) = revoke_context(&conn, &sub);
        reject_timeout(&conn, &edit, now + Duration::seconds(2));
        conn.execute(
            "UPDATE share_control_operations SET last_attempt_edit_id=NULL WHERE id=?1",
            params![operation],
        )
        .unwrap();
    }
    assert!(
        store
            .share_market_reconcile_and_dispatch(now + Duration::seconds(18))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(subscription_status(&store, &sub).await, SUB_REVOKE_FAILED);
    let conn = store.conn.lock().await;
    let state: (String, i64, String) = conn.query_row(
        "SELECT status,attempts,last_error_code FROM share_control_operations WHERE subscription_id=?1 AND action='revoke'",params![sub],
        |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(
        state,
        (
            "rejected".into(),
            1,
            "control_retry_evidence_missing".into()
        )
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM share_edit_requests WHERE share_id='recovery-share'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
}

#[tokio::test]
async fn paid_activation_hold_survives_recovery_and_is_released_only_after_confirmed_revoke() {
    let store = AppStore::new_in_memory_for_tests().unwrap();
    let owner = session("paid-recovery-owner", "paid-recovery-owner@example.com");
    let buyer = session("paid-recovery-buyer", "paid-recovery-buyer@example.com");
    insert_share(
        &store,
        "paid-recovery",
        &owner.email,
        &[ShareTokenPeriod::Day],
    )
    .await;
    let (_, seat) = create_listing(
        &store,
        &owner,
        "paid-recovery",
        SeatInput {
            trial_hours: Some(0),
            ..paid_seat()
        },
    )
    .await;
    let sub = store
        .rent_fixture_share(&buyer, &seat, RentSeatRequest { offer_revision: 1 })
        .await
        .unwrap();
    let now = Utc::now();
    store
        .share_market_reconcile_and_dispatch(now)
        .await
        .unwrap();
    store
        .share_market_request_release(&buyer, &sub, false, false)
        .await
        .unwrap();
    store
        .share_market_reconcile_and_dispatch(now + Duration::seconds(1))
        .await
        .unwrap();
    let (contract, hold, before) = {
        let conn = store.conn.lock().await;
        let contract =
            crate::market_platform_contracts::contract_for_product_tx(&conn, "share", &sub)
                .unwrap()
                .unwrap();
        let hold: String = conn
            .query_row(
                "SELECT activation_hold_id FROM market_platform_contracts WHERE id=?1",
                params![contract],
                |r| r.get(0),
            )
            .unwrap();
        let before = crate::market_wallet::wallet_snapshot_tx(&conn, &buyer.user_id).unwrap();
        assert!(before.held_units > 0);
        (contract, hold, serde_json::to_value(before).unwrap())
    };
    let retry_at = now + Duration::minutes(6);
    store
        .share_market_reconcile_and_dispatch(retry_at)
        .await
        .unwrap();
    {
        let conn = store.conn.lock().await;
        conn.execute(
            "UPDATE shares SET config_revision=2 WHERE share_id='paid-recovery'",
            [],
        )
        .unwrap();
        wake_recovered_revocations_tx(
            &conn,
            "installation-paid-recovery",
            "paid-recovery",
            2,
            retry_at,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(
                crate::market_wallet::wallet_snapshot_tx(&conn, &buyer.user_id).unwrap()
            )
            .unwrap(),
            before
        );
    }
    store
        .share_market_reconcile_and_dispatch(retry_at)
        .await
        .unwrap();
    {
        let conn = store.conn.lock().await;
        let (_, edit, _) = revoke_context(&conn, &sub);
        conn.execute(
            "UPDATE share_edit_requests SET status='applied' WHERE id=?1",
            params![edit],
        )
        .unwrap();
        handle_control_edit_ack(&conn, &edit, "applied", None, &retry_at.to_rfc3339()).unwrap();
    }
    store
        .share_market_reconcile_and_dispatch(retry_at + Duration::seconds(1))
        .await
        .unwrap();
    store
        .share_market_reconcile_and_dispatch(retry_at + Duration::seconds(2))
        .await
        .unwrap();
    let conn = store.conn.lock().await;
    assert_eq!(
        conn.query_row(
            "SELECT status FROM market_wallet_holds WHERE id=?1",
            params![hold],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "released"
    );
    assert_eq!(
        conn.query_row(
            "SELECT status FROM market_platform_contracts WHERE id=?1",
            params![contract],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "activation_failed"
    );
    assert_eq!(
        crate::market_wallet::wallet_snapshot_tx(&conn, &buyer.user_id)
            .unwrap()
            .held_units,
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM market_platform_periods WHERE contract_id=?1",
            params![contract],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    crate::market_wallet::reconcile_wallet_tx(&conn, &buyer.user_id).unwrap();
}

#[tokio::test]
async fn an_ack_for_the_old_revoke_never_revokes_a_new_renters_entitlement() {
    let (store, _, buyer, sub, seat, now) = pending_revoke().await;
    let edit = {
        let conn = store.conn.lock().await;
        let (_, edit, _) = revoke_context(&conn, &sub);
        conn.execute(
            "UPDATE share_edit_requests SET status='applied' WHERE id=?1",
            params![edit],
        )
        .unwrap();
        handle_control_edit_ack(&conn, &edit, "applied", None, &now.to_rfc3339()).unwrap();
        edit
    };
    store
        .share_market_reconcile_and_dispatch(now + Duration::seconds(2))
        .await
        .unwrap();
    let new_sub = store
        .rent_fixture_share(&buyer, &seat, RentSeatRequest { offer_revision: 1 })
        .await
        .unwrap();
    activate_subscription(&store, &new_sub, now + Duration::seconds(3)).await;
    let conn = store.conn.lock().await;
    handle_control_edit_ack(
        &conn,
        &edit,
        "applied",
        None,
        &(now + Duration::seconds(4)).to_rfc3339(),
    )
    .unwrap();
    let entitlement: String = conn
        .query_row(
            "SELECT entitlement_id FROM share_market_subscriptions WHERE id=?1",
            params![new_sub],
            |r| r.get(0),
        )
        .unwrap();
    let json: String = conn
        .query_row(
            "SELECT user_grants_json FROM shares WHERE share_id='recovery-share'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(active_entitlement(Some(&json), &entitlement));
    assert_eq!(
        conn.query_row(
            "SELECT current_subscription_id FROM share_market_seats WHERE id=?1",
            params![seat],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        new_sub
    );
}
