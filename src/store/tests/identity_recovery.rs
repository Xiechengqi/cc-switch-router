use super::*;
use crate::share_value_pricing::RESOURCE_TARIFF_POLICY;
use crate::store::client_identities::ClientIdentityRecoveryRequest;
use crate::store::share_value_quotas::SignedValueRequest;
use serde_json::{Value, json};

struct Case {
    store: AppStore,
    config: Config,
    key: SigningKey,
    request: ClientIdentityRecoveryRequest,
    revision: String,
}

impl Case {
    async fn new(name: &str) -> Self {
        let (store, config) = setup_store(name).await;
        let original = Uuid::new_v4().to_string();
        let key = insert_signed_installation(&store, &original).await;
        claim_setup_client_tunnel(&store, &config, &key, &original, "historic").await;
        {
            let c = store.conn.lock().await;
            purge_installation_data_tx(&c, &original).unwrap();
            c.execute(
                "DELETE FROM client_identities WHERE installation_id=?1",
                params![original],
            )
            .unwrap();
            // The pre-132 collector kept the active host (revision 1), without a key.
            c.execute(
                "UPDATE public_hosts SET lifecycle='active',revision=1 WHERE label='historic'",
                [],
            )
            .unwrap();
        }
        let temp = register_with_key(&store, &key, "2.0.0")
            .await
            .installation_id;
        let mut tariff = share_value_test_snapshot();
        tariff.schema_version = 2;
        tariff.tariff_policy_id = RESOURCE_TARIFF_POLICY.into();
        let revision = tariff.revision().unwrap();
        {
            let c = store.conn.lock().await;
            c.execute("UPDATE installations SET owner_email='owner@example.com',owner_verified_at=?2 WHERE id=?1",params![temp,Utc::now().to_rfc3339()]).unwrap();
            c.execute("INSERT INTO installation_upgrade_tasks(installation_id,task_id,requested_by_email,status,target_commit_id,logs_json,restart_after,client_reported_at_ms,reported_at,created_at,updated_at,failure_json)
                VALUES(?1,'history','owner@example.com','failed','target','[\"log sentinel\"]',0,1,'first','created','first','{\"message\":\"failure sentinel\"}')",params![temp]).unwrap();
            c.execute(
                "INSERT INTO share_value_pricing_snapshots VALUES(?1,?2,1,'resource-prices',1)",
                params![revision, tariff.canonical_json().unwrap()],
            )
            .unwrap();
            c.execute(
                "INSERT INTO share_value_resource_pricing_config VALUES(1,?1,1)",
                params![revision],
            )
            .unwrap();
            c.execute(
                "INSERT INTO share_value_resource_pricing_revisions VALUES(?1)",
                params![revision],
            )
            .unwrap();
            c.execute(
                "INSERT INTO installation_value_capabilities VALUES(?1,1,10)",
                params![temp],
            )
            .unwrap();
            c.execute(
                "INSERT INTO share_value_pricing_acks VALUES(?1,?2,42)",
                params![temp, revision],
            )
            .unwrap();
            c.execute(
                "INSERT INTO share_value_pricing_activations VALUES(?1,?2,45,46)",
                params![temp, revision],
            )
            .unwrap();
        }
        let request = ClientIdentityRecoveryRequest {
            original_installation_id: original,
            replacement_installation_id: temp,
            public_key: public_key_b64(&key),
            subdomain: "historic".into(),
            expected_host_revision: 1,
            expected_owner_email: Some("owner@example.com".into()),
            evidence: "verified backup identity and public key".into(),
            apply: false,
            expected_plan_hash: None,
        };
        Self {
            store,
            config,
            key,
            request,
            revision,
        }
    }
    async fn preview(&self) -> Result<Value, AppError> {
        self.store
            .recover_client_identity("admin@example.com", self.request.clone())
            .await
    }
    async fn apply(&self, preview: &Value) -> Result<Value, AppError> {
        let mut input = self.request.clone();
        input.apply = true;
        input.expected_plan_hash = Some(preview["planHash"].as_str().unwrap().into());
        self.store
            .recover_client_identity("admin@example.com", input)
            .await
    }
    async fn pricing(&self, id: &str, action: &str, payload: Value) -> Result<Value, AppError> {
        let at = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(&self.key, id, action, &payload, at, &nonce);
        self.store
            .signed_value_operation(
                action,
                SignedValueRequest {
                    protocol_epoch: PROTOCOL_EPOCH.into(),
                    installation_id: id.into(),
                    timestamp_ms: at,
                    nonce,
                    signature,
                    payload,
                },
            )
            .await
    }
}

#[tokio::test]
async fn historical_identity_repair_preserves_history_and_requires_a_fresh_signed_tariff_ack() {
    let case = Case::new("recovery-with-deliveries").await;
    let old = &case.request.original_installation_id;
    let temp = &case.request.replacement_installation_id;
    let preview = case.preview().await.unwrap();
    assert_eq!(preview["preservedUpgradeTasks"][0]["taskId"], "history");
    assert_eq!(
        preview["pricingReceiptsToRenew"],
        json!([{"revision":case.revision,"tariffKind":"resource","publishedBy":"resource-prices","current":true,"acknowledgedAtMs":42}])
    );
    assert!(!preview.to_string().contains("sentinel"));
    {
        let c = case.store.conn.lock().await;
        c.execute("UPDATE installation_upgrade_tasks SET reported_at='second',updated_at='second',client_reported_at_ms=2 WHERE installation_id=?1",params![temp]).unwrap();
        c.execute("UPDATE share_value_pricing_activations SET confirmed_at_ms=50 WHERE installation_id=?1",params![temp]).unwrap();
    }
    assert_eq!(
        case.preview().await.unwrap()["planHash"],
        preview["planHash"]
    );
    case.store.conn.lock().await.execute_batch("CREATE TEMP TRIGGER reject_recovery_receipt BEFORE INSERT ON admin_audit_log WHEN NEW.action='client_identity.recover' BEGIN SELECT RAISE(ABORT,'receipt unavailable'); END;").unwrap();
    assert!(case.apply(&preview).await.is_err());
    {
        let c = case.store.conn.lock().await;
        for table in [
            "installation_upgrade_tasks",
            "share_value_pricing_acks",
            "share_value_pricing_activations",
        ] {
            assert_eq!(
                c.query_row(
                    &format!("SELECT count(*) FROM {table} WHERE installation_id=?1"),
                    params![temp],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
        }
        assert!(get_installation(&c, old).unwrap().is_none());
        c.execute_batch("DROP TRIGGER reject_recovery_receipt")
            .unwrap();
    }
    let applied = case.apply(&preview).await.unwrap();
    {
        let c = case.store.conn.lock().await;
        let preserved:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM installation_upgrade_tasks WHERE installation_id=?1 AND task_id='history'
            AND status='failed' AND requested_by_email='owner@example.com' AND lease_id IS NULL AND restart_after=0 AND restart_pending=0
            AND target_commit_id='target' AND created_at='created' AND reported_at='second' AND updated_at='second' AND client_reported_at_ms=2
            AND json_extract(logs_json,'$[0]')='log sentinel' AND json_extract(failure_json,'$.message')='failure sentinel')",params![old],|r|r.get(0)).unwrap();
        assert!(preserved);
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM share_value_pricing_acks WHERE installation_id IN (?1,?2)",
                params![old, temp],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        let violations: i64 = c
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(violations, 0);
    }
    let registration = register_with_key(&case.store, &case.key, "2.0.0").await;
    assert_eq!(&registration.installation_id, old);
    assert!(registration.previous_installation_ids.contains(temp));
    claim_setup_client_tunnel(&case.store, &case.config, &case.key, old, "historic").await;
    let fetch = case
        .pricing(
            old,
            "share_value_pricing_fetch",
            json!({"knownRevision": case.revision, "supportedTariffPolicies": [RESOURCE_TARIFF_POLICY]}),
        )
        .await
        .unwrap();
    assert_eq!(fetch["revision"], case.revision);
    assert!(fetch.get("snapshot").is_some());
    let ack = json!({"revision":case.revision,"storageSchema":5});
    assert!(
        case.pricing(temp, "share_value_pricing_ack", ack.clone())
            .await
            .is_err()
    );
    assert!(matches!(
        case.pricing(old, "share_value_pricing_active", ack.clone()).await,
        Err(AppError::Conflict(message)) if message.contains("not acknowledged")
    ));
    assert_eq!(
        case.pricing(old, "share_value_pricing_ack", ack.clone())
            .await
            .unwrap()["ok"],
        true
    );
    assert_eq!(
        case.pricing(old, "share_value_pricing_active", ack)
            .await
            .unwrap()["ok"],
        true
    );
    let mut replay = case.apply(&preview).await.unwrap();
    assert_eq!(
        replay.as_object_mut().unwrap().remove("alreadyApplied"),
        Some(json!(true))
    );
    assert_eq!(replay, applied);
    let mut equivalent = case.request.clone();
    equivalent.subdomain = " Historic ".into();
    assert_eq!(
        case.store
            .recover_client_identity("admin@example.com", equivalent)
            .await
            .unwrap()["planHash"],
        applied["planHash"]
    );
    let mut other_host = case.request.clone();
    other_host.subdomain = "different".into();
    assert_eq!(
        case.store
            .recover_client_identity("admin@example.com", other_host)
            .await
            .unwrap_err()
            .code(),
        Some("CLIENT_IDENTITY_RECOVERY_HOST_CHANGED")
    );
    let mut other_owner = case.request.clone();
    other_owner.expected_owner_email = Some("different@example.com".into());
    assert_eq!(
        case.store
            .recover_client_identity("admin@example.com", other_owner)
            .await
            .unwrap_err()
            .code(),
        Some("CLIENT_IDENTITY_RECOVERY_ANCHOR_CHANGED")
    );
    let c = case.store.conn.lock().await;
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM admin_audit_log WHERE action='client_identity.recover'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn historical_identity_repair_reports_all_blockers_and_revalidates_content() {
    let case = Case::new("recovery-blocking-dependencies").await;
    let temp = &case.request.replacement_installation_id;
    let preview = case.preview().await.unwrap();
    {
        let c = case.store.conn.lock().await;
        c.execute(
            "UPDATE installation_upgrade_tasks SET failure_json='{}' WHERE installation_id=?1",
            params![temp],
        )
        .unwrap();
    }
    assert_eq!(
        case.apply(&preview).await.unwrap_err().code(),
        Some("CLIENT_IDENTITY_RECOVERY_PREVIEW_REQUIRED")
    );
    // A receipt's source is part of the reviewed decision even with the same
    // number of ACKs and the same content revision.
    let before_source = case.preview().await.unwrap();
    case.store.conn.lock().await.execute("UPDATE share_value_pricing_snapshots SET published_by='new-publisher' WHERE revision=?1", params![case.revision]).unwrap();
    assert_eq!(
        case.apply(&before_source).await.unwrap_err().code(),
        Some("CLIENT_IDENTITY_RECOVERY_PREVIEW_REQUIRED")
    );
    let changed = case.preview().await.unwrap();
    {
        let c = case.store.conn.lock().await;
        c.execute("INSERT INTO installation_upgrade_tasks(installation_id,task_id,status,created_at,updated_at) VALUES(?1,'extra','success','created','now')",params![temp]).unwrap();
    }
    assert_eq!(
        case.apply(&changed).await.unwrap_err().code(),
        Some("CLIENT_IDENTITY_RECOVERY_PREVIEW_REQUIRED")
    );
    {
        let c = case.store.conn.lock().await;
        c.execute(
            "UPDATE installation_upgrade_tasks SET status='running' WHERE task_id='extra'",
            [],
        )
        .unwrap();
        for (task, pending) in [("restart", 1), ("lease-orphan-report", 0)] {
            c.execute("INSERT INTO installation_upgrade_tasks(installation_id,task_id,status,restart_pending,created_at,updated_at) VALUES(?1,?2,'success',?3,'created','now')",params![temp,task,pending]).unwrap();
        }
        c.execute("INSERT INTO online_client_listings(installation_id,seller_id,owner_email,state,revision,sequence,cycle_price_minor,updated_at)
            VALUES(?1,'seller','owner@example.com','listed',1,2,0,1)",params![temp]).unwrap();
        c.execute("INSERT INTO online_client_quotes(id,installation_id,buyer_id,revision,cycle_price_minor,projection_json,expires_at)
            VALUES('quote',?1,'buyer',1,0,'{}',100)",params![temp]).unwrap();
        c.execute("INSERT INTO online_client_leases(id,installation_id,quote_id,buyer_id,tenant_email,seller_id,alias,state,sequence,grant_until,created_at)
            VALUES('rental',?1,'quote','buyer','tenant@example.com','seller','lease-fixture','active',2,100,1)",params![temp]).unwrap();
        c.execute("INSERT INTO installation_upgrade_tasks(installation_id,task_id,status,created_at,updated_at,lease_id)
            VALUES(?1,'tenant-history','failed','created','now','rental')",params![temp]).unwrap();
        c.execute("INSERT INTO share_value_quota_projections VALUES(?1,'share','tenant@example.com','epoch','window',1,'{}',1)",params![temp]).unwrap();
        c.execute("INSERT INTO share_value_pricing_snapshots VALUES('private-rate','{}',1,'owner-prices',1)",[]).unwrap();
        c.execute(
            "INSERT INTO share_value_pricing_acks VALUES(?1,'private-rate',42)",
            params![temp],
        )
        .unwrap();
        c.execute(
            "INSERT INTO share_value_pricing_installation_revisions VALUES(?1,'private-rate',1,1)",
            params![temp],
        )
        .unwrap();
        c.execute_batch("CREATE TABLE future_recovery_reference (installation_id TEXT REFERENCES installations(id) ON DELETE CASCADE)").unwrap();
        c.execute(
            "INSERT INTO future_recovery_reference VALUES(?1)",
            params![temp],
        )
        .unwrap();
        c.execute_batch("CREATE TABLE future_task_notes(client TEXT, task TEXT, FOREIGN KEY(client,task) REFERENCES installation_upgrade_tasks(installation_id,task_id) ON DELETE CASCADE)").unwrap();
        c.execute(
            "INSERT INTO future_task_notes VALUES(?1,'history')",
            params![temp],
        )
        .unwrap();
    }
    let AppError::Coded { code, details, .. } = case.apply(&preview).await.unwrap_err() else {
        panic!("expected blocked recovery")
    };
    assert_eq!(code, "CLIENT_IDENTITY_RECOVERY_OBLIGATIONS");
    let blockers = details["blockers"].as_array().unwrap();
    for task in ["extra", "restart", "lease-orphan-report", "tenant-history"] {
        assert!(blockers.iter().any(|b| b["taskId"] == task));
    }
    for table in [
        "online_client_leases",
        "share_value_quota_projections",
        "share_value_pricing_acks",
        "share_value_pricing_installation_revisions",
        "future_recovery_reference",
        "future_task_notes",
    ] {
        assert!(blockers.iter().any(|b| b["table"] == table));
    }
    assert_eq!(details["preservedUpgradeTasks"][0]["taskId"], "history");
    assert_eq!(
        details["pricingReceiptsToRenew"][0]["revision"],
        case.revision
    );
    assert!(!details.to_string().contains("sentinel"));
    let mut wrong = case.request.clone();
    wrong.expected_host_revision += 1;
    assert_eq!(
        case.store
            .recover_client_identity("admin@example.com", wrong)
            .await
            .unwrap_err()
            .code(),
        Some("CLIENT_IDENTITY_RECOVERY_HOST_CHANGED")
    );
    let mut wrong_owner = case.request.clone();
    wrong_owner.expected_owner_email = Some("different@example.com".into());
    assert_eq!(
        case.store
            .recover_client_identity("admin@example.com", wrong_owner)
            .await
            .unwrap_err()
            .code(),
        Some("CLIENT_IDENTITY_RECOVERY_ANCHOR_CHANGED")
    );
    assert!(
        get_installation(&*case.store.conn.lock().await, temp)
            .unwrap()
            .is_some()
    );
}
