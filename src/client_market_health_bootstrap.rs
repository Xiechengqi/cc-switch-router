//! First monitor enrollment for an existing rental. Never changes rental state.
use super::*;

#[derive(Clone, Debug)]
pub(super) struct Claim {
    pub host: String,
    pub installation: String,
    pub epoch: i64,
    pub generation: i64,
    pub sequence: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct BootstrapRequest {
    pub expected_installation_id: String,
    pub expected_machine_epoch: i64,
}

const ELIGIBLE: &str = "SELECT a.key_generation FROM router_ssh_hosts h JOIN client_host_agents a ON a.host_id=h.id
    WHERE h.id=?1 AND h.status='allocated' AND h.installation_id=?2 AND a.machine_epoch=?3 AND (?4 IS NULL OR h.provider_id=?4)
      AND a.legacy=1 AND a.admitted_hour IS NULL AND a.revoked=0 AND a.replacement_pending=0 AND a.readmission_pending=0
      AND COALESCE(a.last_error,'') NOT IN ('rotation_requested','hour_conflict')
      AND (a.public_key IS NULL OR EXISTS(SELECT 1 FROM client_host_agent_bootstraps b WHERE b.host_id=h.id
          AND b.installation_id=?2 AND b.machine_epoch=?3 AND b.key_generation=a.key_generation))
      AND EXISTS(SELECT 1 FROM client_market_subscriptions s WHERE s.host_id=h.id AND s.installation_id=?2 AND s.status IN ('active','billing_suspended'))
      AND NOT EXISTS(SELECT 1 FROM provisioning_jobs j WHERE j.host_id=h.id AND j.status IN ('pending','running'))";

pub(crate) async fn enqueue(
    store: &AppStore,
    host: &str,
    input: &BootstrapRequest,
    actor: &str,
) -> Result<(), AppError> {
    enqueue_as(store, host, input, actor, Some(actor)).await
}

pub(crate) async fn enqueue_operator(
    store: &AppStore,
    host: &str,
    input: &BootstrapRequest,
    actor: &str,
) -> Result<(), AppError> {
    enqueue_as(store, host, input, actor, None).await
}

async fn enqueue_as(
    store: &AppStore,
    host: &str,
    input: &BootstrapRequest,
    actor: &str,
    owner: Option<&str>,
) -> Result<(), AppError> {
    let conn = store.conn.lock().await;
    let tx = conn.transaction().map_err(database)?;
    let generation: i64 = tx
        .query_row(
            ELIGIBLE,
            params![
                host,
                input.expected_installation_id,
                input.expected_machine_epoch,
                owner
            ],
            |r| r.get(0),
        )
        .optional()
        .map_err(database)?
        .ok_or_else(|| {
            AppError::Conflict(
                "Host is not eligible for first monitor enrollment in this rental".into(),
            )
        })?;
    let pending: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM client_host_agent_bootstraps b JOIN client_host_agents a ON a.host_id=b.host_id
        WHERE b.host_id=?1 AND b.installation_id=?2 AND b.machine_epoch=?3 AND b.status IN ('pending','running') AND b.operation_sequence=a.operation_sequence)",
        params![host,input.expected_installation_id,input.expected_machine_epoch],|r|r.get(0)).map_err(database)?;
    if pending {
        return Ok(());
    }
    tx.execute(
        "UPDATE client_host_agents SET operation_sequence=operation_sequence+1 WHERE host_id=?1",
        params![host],
    )
    .map_err(database)?;
    let sequence: i64 = tx
        .query_row(
            "SELECT operation_sequence FROM client_host_agents WHERE host_id=?1",
            params![host],
            |r| r.get(0),
        )
        .map_err(database)?;
    let now = Utc::now().to_rfc3339();
    tx.execute("INSERT INTO client_host_agent_bootstraps(host_id,installation_id,machine_epoch,key_generation,operation_sequence,status,requested_by,requested_at,updated_at,owner_user_id)
        VALUES(?1,?2,?3,?4,?5,'pending',?6,?7,?7,?8)
        ON CONFLICT(host_id) DO UPDATE SET installation_id=excluded.installation_id,machine_epoch=excluded.machine_epoch,key_generation=excluded.key_generation,
        operation_sequence=excluded.operation_sequence,status='pending',attempts=0,requested_by=excluded.requested_by,owner_user_id=excluded.owner_user_id,requested_at=excluded.requested_at,updated_at=excluded.updated_at,last_error=NULL",
        params![host,input.expected_installation_id,input.expected_machine_epoch,generation,sequence,actor,now,owner]).map_err(database)?;
    tx.commit().map_err(database)
}

pub(super) async fn request(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(input): Json<BootstrapRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    let host = state
        .store
        .client_market_get_host(&id)
        .await?
        .ok_or_else(|| AppError::NotFound("Host not found".into()))?;
    if !session_is_host_owner(&session, host.provider_id.as_deref()) {
        return Err(AppError::Forbidden("Host owner required".into()));
    }
    enqueue(&state.store, &id, &input, &session.user_id).await?;
    spawn(&state, &id);
    Ok(Json(json!({"queued":true})))
}

pub(super) fn valid(conn: &Connection, claim: &Claim) -> Result<bool, AppError> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM client_host_agent_bootstraps b JOIN router_ssh_hosts h ON h.id=b.host_id JOIN client_host_agents a ON a.host_id=h.id
        WHERE b.host_id=?1 AND b.status='running' AND b.operation_sequence=?5 AND a.operation_sequence=?5
          AND (b.owner_user_id IS NULL OR h.provider_id=b.owner_user_id) AND h.status='allocated' AND h.installation_id=?2 AND b.installation_id=?2 AND a.machine_epoch=?3 AND b.machine_epoch=?3
          AND a.key_generation=?4 AND a.revoked=0 AND a.replacement_pending=0 AND a.readmission_pending=0
          AND EXISTS(SELECT 1 FROM client_market_subscriptions s WHERE s.host_id=h.id AND s.installation_id=?2 AND s.status IN ('active','billing_suspended'))
          AND NOT EXISTS(SELECT 1 FROM provisioning_jobs j WHERE j.host_id=h.id AND j.status IN ('pending','running')))",
        params![claim.host,claim.installation,claim.epoch,claim.generation,claim.sequence],|r|r.get(0)).map_err(database)
}

async fn claim(store: &AppStore, host: &str) -> Result<Option<Claim>, AppError> {
    let conn = store.conn.lock().await;
    let tx = conn.transaction().map_err(database)?;
    let row:Option<(String,i64,i64,i64)>=tx.query_row("SELECT b.installation_id,b.machine_epoch,b.key_generation,b.operation_sequence FROM client_host_agent_bootstraps b
        JOIN router_ssh_hosts h ON h.id=b.host_id JOIN client_host_agents a ON a.host_id=h.id
        WHERE b.host_id=?1 AND b.attempts<3 AND b.operation_sequence=a.operation_sequence
        AND h.status='allocated' AND h.installation_id=b.installation_id AND a.machine_epoch=b.machine_epoch AND a.key_generation=b.key_generation AND a.revoked=0
        AND (b.status='pending' OR (b.status IN ('running','failed') AND julianday(b.updated_at)<julianday(?2)-15.0/1440))",
        params![host,Utc::now().to_rfc3339()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(database)?;
    let Some((installation, epoch, generation, old_sequence)) = row else {
        return Ok(None);
    };
    let sequence = old_sequence + 1;
    tx.execute("UPDATE client_host_agents SET operation_sequence=?2 WHERE host_id=?1 AND operation_sequence=?3",params![host,sequence,old_sequence]).map_err(database)?;
    tx.execute("UPDATE client_host_agent_bootstraps SET status='running',attempts=attempts+1,operation_sequence=?2,updated_at=?3 WHERE host_id=?1",
        params![host,sequence,Utc::now().to_rfc3339()]).map_err(database)?;
    let claim = Claim {
        host: host.into(),
        installation,
        epoch,
        generation,
        sequence,
    };
    if !valid(&tx, &claim)? {
        return Ok(None);
    }
    tx.commit().map_err(database)?;
    Ok(Some(claim))
}

pub(super) async fn refresh(store: &AppStore, claim: &Claim) -> Result<(), AppError> {
    let conn = store.conn.lock().await;
    if !valid(&conn, claim)? {
        return Err(AppError::Conflict(
            "Monitor bootstrap rental context changed".into(),
        ));
    }
    conn.execute("UPDATE client_host_agent_bootstraps SET updated_at=?3 WHERE host_id=?1 AND operation_sequence=?2 AND status='running'",
        params![claim.host,claim.sequence,Utc::now().to_rfc3339()]).map_err(database)?;
    Ok(())
}

fn spawn(state: &ServerState, host: &str) {
    let state = state.clone();
    let host = host.to_owned();
    tokio::spawn(async move {
        let Ok(_slot) = binary::PREPARATION_SLOTS.try_acquire() else {
            return;
        };
        let _lock = state
            .client_market_actions
            .lock(&format!("binary:{host}"))
            .await;
        let result=async {
            let Some(claim)=claim(&state.store,&host).await? else{return Ok(())};
            let result=async {
                let record=state.store.client_market_get_host(&host).await?.ok_or_else(||AppError::NotFound("Host not found".into()))?;
                require_pinned_host_fingerprint(&record,&known_hosts_path(&state.config)).await?;
                let selector=state.dynamic.read().await.client_server_release.clone();
                let release=state.client_server_release_validator.resolve_installation(&selector).await.map_err(AppError::ServiceUnavailable)?;
                deployment::enroll_rental(&state,&record,&release,&claim).await
            }.await;
            if let Err(error)=&result {
                let conn=state.store.conn.lock().await;
                let status=if valid(&conn,&claim)? {"failed"}else{"cancelled"};
                conn.execute("UPDATE client_host_agent_bootstraps SET status=?3,last_error=?4,updated_at=?5 WHERE host_id=?1 AND operation_sequence=?2 AND status='running'",
                    params![host,claim.sequence,status,error.to_string().chars().take(500).collect::<String>(),Utc::now().to_rfc3339()]).map_err(database)?;
            }
            result
        }.await;
        if let Err(error) = result {
            warn!(host_id=%host,%error,"Rental monitor bootstrap deferred")
        }
    });
}

fn recover_tasks(conn: &Connection, now: &str) -> Result<(), AppError> {
    conn.execute("UPDATE client_host_agent_bootstraps AS b SET status='cancelled',last_error='operation_superseded',updated_at=?1
        WHERE b.status IN ('pending','running','failed') AND NOT EXISTS(
          SELECT 1 FROM client_host_agents a JOIN router_ssh_hosts h ON h.id=a.host_id
          WHERE a.host_id=b.host_id AND a.operation_sequence=b.operation_sequence AND a.revoked=0
            AND a.machine_epoch=b.machine_epoch AND a.key_generation=b.key_generation
            AND a.replacement_pending=0 AND a.readmission_pending=0
            AND h.status='allocated' AND h.installation_id=b.installation_id
            AND (b.owner_user_id IS NULL OR h.provider_id=b.owner_user_id)
            AND EXISTS(SELECT 1 FROM client_market_subscriptions s WHERE s.host_id=h.id AND s.installation_id=b.installation_id AND s.status IN ('active','billing_suspended'))
            AND NOT EXISTS(SELECT 1 FROM provisioning_jobs j WHERE j.host_id=h.id AND j.status IN ('pending','running')))", params![now]).map_err(database)?;
    // A crash during the last allowed attempt must leave a retryable failure,
    // rather than a task that the UI shows as installing forever.
    conn.execute("UPDATE client_host_agent_bootstraps SET status='failed',last_error='monitor_bootstrap_interrupted',updated_at=?1
        WHERE status='running' AND attempts>=3 AND julianday(updated_at)<julianday(?1)-15.0/1440", params![now]).map_err(database)?;
    Ok(())
}

pub(crate) async fn reconcile(state: &ServerState) -> Result<(), AppError> {
    let ids = {
        let conn = state.store.conn.lock().await;
        recover_tasks(&conn, &Utc::now().to_rfc3339())?;
        let mut q=conn.prepare("SELECT host_id FROM client_host_agent_bootstraps WHERE attempts<3 AND
            (status='pending' OR (status IN ('running','failed') AND julianday(updated_at)<julianday(?1)-15.0/1440)) ORDER BY updated_at LIMIT 4").map_err(database)?;
        q.query_map(params![Utc::now().to_rfc3339()], |r| r.get::<_, String>(0))
            .map_err(database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database)?
    };
    for id in ids {
        spawn(state, &id)
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> (AppStore, crate::models::AuthSession, BootstrapRequest) {
        let store = AppStore::new_in_memory_for_tests().unwrap();
        let owner = {
            let conn = store.conn.lock().await;
            let owner = crate::market_finance_auth::test_session(&conn, "bootstrap-owner");
            let buyer = crate::market_finance_auth::test_session(&conn, "bootstrap-buyer");
            let now = Utc::now().to_rfc3339();
            conn.execute("INSERT INTO router_ssh_hosts(id,ip,port,host_owner_email,provider_id,status,created_at,updated_at) VALUES('bootstrap-host','192.0.2.40',22,?1,?2,'onboarding',?3,?3)",params![owner.email,owner.user_id,now]).unwrap();
            conn.execute("UPDATE client_host_agents SET legacy=1,maintenance_pending=1 WHERE host_id='bootstrap-host'",[]).unwrap();
            conn.execute("UPDATE router_ssh_hosts SET status='allocated',installation_id='bootstrap-lease' WHERE id='bootstrap-host'",[]).unwrap();
            conn.execute("INSERT INTO client_market_subscriptions(installation_id,host_id,provider_id,host_owner_email,client_user_id,client_owner_email,status,offer_revision,created_at,updated_at)
                VALUES('bootstrap-lease','bootstrap-host',?1,?2,?3,?4,'active',7,?5,?5)",params![owner.user_id,owner.email,buyer.user_id,buyer.email,now]).unwrap();
            owner
        };
        (
            store,
            owner,
            BootstrapRequest {
                expected_installation_id: "bootstrap-lease".into(),
                expected_machine_epoch: 1,
            },
        )
    }

    #[tokio::test]
    async fn recovery_finalizes_the_last_interrupted_attempt_and_cancels_obsolete_failures() {
        let (store, owner, input) = fixture().await;
        enqueue(&store, "bootstrap-host", &input, &owner.user_id)
            .await
            .unwrap();
        for attempt in 1..=3 {
            assert!(claim(&store, "bootstrap-host").await.unwrap().is_some());
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE client_host_agent_bootstraps SET updated_at='2020-01-01T00:00:00Z'",
                [],
            )
            .unwrap();
            recover_tasks(&conn, &Utc::now().to_rfc3339()).unwrap();
            let state: (String, i64) = conn
                .query_row(
                    "SELECT status,attempts FROM client_host_agent_bootstraps",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(
                state,
                (
                    (if attempt == 3 { "failed" } else { "running" }).into(),
                    attempt
                )
            );
        }
        assert!(claim(&store, "bootstrap-host").await.unwrap().is_none());
        {
            let conn = store.conn.lock().await;
            let owner_view = project_views(&conn, Some(&owner), Some("bootstrap-host"), false)
                .unwrap()
                .remove("bootstrap-host")
                .unwrap();
            assert_eq!(owner_view.deployment.state, "failed");
            assert!(owner_view.deployment.can_bootstrap);
        }
        enqueue(&store, "bootstrap-host", &input, &owner.user_id)
            .await
            .unwrap();
        assert!(claim(&store, "bootstrap-host").await.unwrap().is_some());
        let conn = store.conn.lock().await;
        conn.execute("UPDATE client_host_agent_bootstraps SET status='failed',updated_at='2020-01-01T00:00:00Z'", []).unwrap();
        conn.execute(
            "UPDATE router_ssh_hosts SET provider_id='another-owner' WHERE id='bootstrap-host'",
            [],
        )
        .unwrap();
        recover_tasks(&conn, &Utc::now().to_rfc3339()).unwrap();
        assert_eq!(
            conn.query_row("SELECT status FROM client_host_agent_bootstraps", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "cancelled"
        );
    }

    #[tokio::test]
    async fn rental_bootstrap_is_single_flight_and_preserves_the_active_rental() {
        let (store, owner, input) = fixture().await;
        enqueue(&store, "bootstrap-host", &input, &owner.user_id)
            .await
            .unwrap();
        let first = claim(&store, "bootstrap-host").await.unwrap().unwrap();
        enqueue(&store, "bootstrap-host", &input, &owner.user_id)
            .await
            .unwrap();
        assert!(claim(&store, "bootstrap-host").await.unwrap().is_none());
        let conn = store.conn.lock().await;
        assert!(valid(&conn, &first).unwrap());
        let state:(String,String,String,i64)=conn.query_row("SELECT h.status,h.installation_id,s.status,s.offer_revision FROM router_ssh_hosts h JOIN client_market_subscriptions s ON s.host_id=h.id WHERE h.id='bootstrap-host'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        assert_eq!(
            state,
            (
                "allocated".into(),
                "bootstrap-lease".into(),
                "active".into(),
                7
            )
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM client_host_hour_reports", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn rental_end_binding_change_and_revocation_cancel_old_execution_rights() {
        for mutation in [
            "UPDATE client_market_subscriptions SET status='releasing' WHERE installation_id='bootstrap-lease'",
            "UPDATE router_ssh_hosts SET installation_id='new-lease' WHERE id='bootstrap-host'",
            "UPDATE client_host_agents SET revoked=1,operation_sequence=operation_sequence+1 WHERE host_id='bootstrap-host'",
            "UPDATE router_ssh_hosts SET provider_id='new-owner' WHERE id='bootstrap-host'",
        ] {
            let (store, owner, input) = fixture().await;
            enqueue(&store, "bootstrap-host", &input, &owner.user_id)
                .await
                .unwrap();
            let first = claim(&store, "bootstrap-host").await.unwrap().unwrap();
            let conn = store.conn.lock().await;
            conn.execute(mutation, []).unwrap();
            assert!(!valid(&conn, &first).unwrap());
            assert_eq!(
                conn.query_row("SELECT status FROM client_host_agent_bootstraps", [], |r| r
                    .get::<_, String>(0))
                    .unwrap(),
                "cancelled"
            );
        }
    }

    #[tokio::test]
    async fn retry_preserves_key_generation_and_first_schedule_and_rejects_wrong_context() {
        let (store, owner, mut input) = fixture().await;
        input.expected_machine_epoch = 2;
        assert!(
            enqueue(&store, "bootstrap-host", &input, &owner.user_id)
                .await
                .is_err()
        );
        input.expected_machine_epoch = 1;
        enqueue(&store, "bootstrap-host", &input, &owner.user_id)
            .await
            .unwrap();
        let first = claim(&store, "bootstrap-host").await.unwrap().unwrap();
        {
            let conn = store.conn.lock().await;
            conn.execute("UPDATE client_host_agents SET public_key='same-key',first_due_at=123456 WHERE host_id='bootstrap-host'",[]).unwrap();
            conn.execute("UPDATE client_host_agent_bootstraps SET status='failed',updated_at='2020-01-01T00:00:00Z'",[]).unwrap();
        }
        let retry = claim(&store, "bootstrap-host").await.unwrap().unwrap();
        assert!(retry.sequence > first.sequence);
        assert_eq!(retry.generation, first.generation);
        let conn = store.conn.lock().await;
        assert!(!valid(&conn, &first).unwrap());
        assert!(valid(&conn, &retry).unwrap());
        assert_eq!(
            conn.query_row("SELECT first_due_at FROM client_host_agents", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            123456
        );
    }

    #[tokio::test]
    async fn enqueue_rechecks_owner_inside_the_write_transaction() {
        let (store, owner, input) = fixture().await;
        assert!(
            enqueue(&store, "bootstrap-host", &input, "not-the-owner")
                .await
                .is_err()
        );
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE router_ssh_hosts SET provider_id='new-owner' WHERE id='bootstrap-host'",
                [],
            )
            .unwrap();
        }
        assert!(
            enqueue(&store, "bootstrap-host", &input, &owner.user_id)
                .await
                .is_err()
        );
        enqueue_operator(&store, "bootstrap-host", &input, "uid:0")
            .await
            .unwrap();
        let claim = claim(&store, "bootstrap-host").await.unwrap().unwrap();
        assert!(valid(&*store.conn.lock().await, &claim).unwrap());
    }

    #[tokio::test]
    async fn bootstrap_capability_and_state_ignore_tasks_for_another_identity() {
        for mutation in [
            "UPDATE client_host_agent_bootstraps SET installation_id='old-lease'",
            "UPDATE client_host_agent_bootstraps SET machine_epoch=2",
            "UPDATE client_host_agent_bootstraps SET key_generation=2",
        ] {
            let (store, owner, input) = fixture().await;
            enqueue(&store, "bootstrap-host", &input, &owner.user_id)
                .await
                .unwrap();
            {
                let conn = store.conn.lock().await;
                conn.execute("UPDATE client_host_agents SET public_key='current-key' WHERE host_id='bootstrap-host'", []).unwrap();
                conn.execute(
                    "UPDATE client_host_agent_bootstraps SET status='failed'",
                    [],
                )
                .unwrap();
                let current = project_views(&conn, Some(&owner), Some("bootstrap-host"), false)
                    .unwrap()
                    .remove("bootstrap-host")
                    .unwrap();
                assert!(current.deployment.can_bootstrap);
                assert_eq!(current.deployment.state, "failed");
                conn.execute(mutation, []).unwrap();
                let changed = project_views(&conn, Some(&owner), Some("bootstrap-host"), false)
                    .unwrap()
                    .remove("bootstrap-host")
                    .unwrap();
                assert!(!changed.deployment.can_bootstrap);
                assert_eq!(changed.deployment.state, "not_enabled");
            }
            assert!(
                enqueue(&store, "bootstrap-host", &input, &owner.user_id)
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn unregistered_rentals_do_not_claim_to_be_awaiting_the_first_report() {
        let (store, owner, input) = fixture().await;
        {
            let conn = store.conn.lock().await;
            let public = public_summary(&conn, "bootstrap-host").unwrap().unwrap();
            assert_eq!(public.state, "not_enabled");
            assert!(!public.deployment.can_bootstrap);
            let own = project_views(&conn, Some(&owner), Some("bootstrap-host"), false)
                .unwrap()
                .remove("bootstrap-host")
                .unwrap();
            assert!(own.deployment.can_bootstrap);
            assert_eq!(
                serde_json::to_value(&public).unwrap(),
                serde_json::to_value(
                    project_views(&conn, None, Some("bootstrap-host"), false)
                        .unwrap()
                        .remove("bootstrap-host")
                        .unwrap()
                )
                .unwrap()
            );
        }
        enqueue(&store, "bootstrap-host", &input, &owner.user_id)
            .await
            .unwrap();
        let conn = store.conn.lock().await;
        let queued = public_summary(&conn, "bootstrap-host").unwrap().unwrap();
        assert_eq!(queued.state, "deploying");
        assert_eq!(queued.deployment.state, "queued");
        assert!(queued.next_due_at.is_none());
        assert!(queued.snapshot.is_none());
    }
}
