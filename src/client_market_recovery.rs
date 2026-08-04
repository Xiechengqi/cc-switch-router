use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};
use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, Notify, OwnedMutexGuard, Semaphore};
use tokio::task::JoinSet;
use tracing::{info, warn};
use uuid::Uuid;

use crate::ServerState;
use crate::client_market::{
    ClientRecoveryRemoteOutcome, ProvisioningJobRecord, session_is_host_owner,
    ssh_recover_market_client_process,
};
use crate::db::{OptionalExtension, TransactionBehavior, params, params_from_iter};
use crate::error::AppError;
use crate::proxy::RouteAvailability;
use crate::store::AppStore;

pub(crate) const JOB_TYPE_RECOVER: &str = "recover";

const JOB_STATUS_SUCCEEDED: &str = "succeeded";
const JOB_STATUS_FAILED: &str = "failed";
const JOB_PHASE_CHECKING: &str = "recovery_checking";
const JOB_PHASE_WAITING: &str = "recovery_waiting";
const JOB_PHASE_COMPLETE: &str = "complete";

const RECOVERY_CYCLE_INTERVAL: Duration = Duration::from_secs(5);
const RECOVERY_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(60 * 60);
const INITIAL_OFFLINE_DELAY: ChronoDuration = ChronoDuration::seconds(30);
const STABLE_RESET_AFTER: ChronoDuration = ChronoDuration::minutes(10);
const TUNNEL_CONFIRM_TIMEOUT: Duration = Duration::from_secs(90);
const TUNNEL_CONFIRM_INTERVAL: Duration = Duration::from_secs(3);
const MAX_CONCURRENT_RECOVERIES: usize = 4;
const TERMINAL_JOB_RETENTION: ChronoDuration = ChronoDuration::days(30);

const RECOVERY_ELIGIBILITY_PREDICATE: &str = "
    h.status = 'allocated'
    AND h.installation_id = i.id
    AND i.provision_source = 'router_market'
    AND i.provision_host_id = h.id
    AND s.installation_id = i.id
    AND s.host_id = h.id
    AND s.status = 'active'
    AND t.installation_id = i.id
    AND t.enabled = 1
    AND NULLIF(TRIM(t.subdomain), '') IS NOT NULL";

pub struct ClientMarketRecoveryCoordinator {
    locks: Mutex<HashMap<String, Weak<Mutex<()>>>>,
    slots: Semaphore,
    wake: Notify,
}

impl Default for ClientMarketRecoveryCoordinator {
    fn default() -> Self {
        Self {
            locks: Mutex::default(),
            slots: Semaphore::new(MAX_CONCURRENT_RECOVERIES),
            wake: Notify::new(),
        }
    }
}

impl ClientMarketRecoveryCoordinator {
    pub(crate) async fn lock(&self, installation_id: &str) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.locks.lock().await;
            locks.retain(|_, lock| lock.strong_count() > 0);
            match locks.get(installation_id).and_then(Weak::upgrade) {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(Mutex::new(()));
                    locks.insert(installation_id.to_string(), Arc::downgrade(&lock));
                    lock
                }
            }
        };
        lock.lock_owned().await
    }

    pub(crate) fn wake(&self) {
        self.wake.notify_one();
    }

    async fn notified(&self) {
        self.wake.notified().await;
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClientMarketRecoveryView {
    pub state: String,
    pub attempt_level: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_attempt_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_attempt_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
struct RecoveryCandidate {
    installation_id: String,
    host_id: String,
    subdomain: String,
}

#[derive(Debug, Clone)]
struct RecoveryJobContext {
    installation_id: String,
    host_id: String,
    subdomain: String,
    phase: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecoveryStateRecord {
    installation_id: String,
    host_id: String,
    observed_state: String,
    incident_started_at: Option<DateTime<Utc>>,
    stable_since: Option<DateTime<Utc>>,
    attempt_level: u32,
    next_attempt_at: Option<DateTime<Utc>>,
    last_attempt_at: Option<DateTime<Utc>>,
    last_outcome: Option<String>,
    blocked_reason: Option<String>,
    paused_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FinishKind {
    Succeeded,
    Failed,
    Blocked,
}

struct FinishAttempt<'a> {
    kind: FinishKind,
    outcome: &'a str,
    failure_code: Option<&'a str>,
    method: Option<&'a str>,
    event_type: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SshFailureClass {
    Transient(&'static str),
    Blocked(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryControlAction {
    Pause,
    Resume,
    Retry,
}

pub(crate) async fn control_recovery(
    state: &ServerState,
    session: &crate::models::AuthSession,
    host_id: &str,
    action: RecoveryControlAction,
) -> Result<ClientMarketRecoveryView, AppError> {
    if !state.config.client_market_recovery_enabled {
        return Err(AppError::Conflict(
            "Client Market automatic recovery is disabled".into(),
        ));
    }
    let is_admin = state.dynamic.read().await.is_admin(&session.email);
    let installation_id = state
        .store
        .client_market_recovery_installation_for_control(host_id, session, is_admin)
        .await?;
    let _guard = state.client_market_recovery.lock(&installation_id).await;
    let view = state
        .store
        .client_market_apply_recovery_control(host_id, session, is_admin, action, Utc::now())
        .await?;
    state.client_market_recovery.wake();
    Ok(view)
}

pub(crate) async fn run_service(state: ServerState) -> Result<(), AppError> {
    if !state.config.client_market_recovery_enabled {
        info!("Client Market automatic recovery is disabled");
        std::future::pending::<()>().await;
        return Ok(());
    }

    info!(
        initial_offline_secs = INITIAL_OFFLINE_DELAY.num_seconds(),
        stable_reset_secs = STABLE_RESET_AFTER.num_seconds(),
        max_concurrency = MAX_CONCURRENT_RECOVERIES,
        "Client Market automatic recovery started"
    );
    let mut interval = tokio::time::interval(RECOVERY_CYCLE_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut maintenance_interval = tokio::time::interval(RECOVERY_MAINTENANCE_INTERVAL);
    maintenance_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut tasks = JoinSet::new();

    loop {
        let maintenance_due = tokio::select! {
            _ = interval.tick() => false,
            _ = maintenance_interval.tick() => true,
            _ = state.client_market_recovery.notified() => false,
            result = tasks.join_next(), if !tasks.is_empty() => {
                report_recovery_task_result(result);
                false
            }
        };
        while let Some(result) = tasks.try_join_next() {
            report_recovery_task_result(Some(result));
        }
        let capacity = MAX_CONCURRENT_RECOVERIES.saturating_sub(tasks.len());
        match run_recovery_cycle(&state, capacity).await {
            Ok(job_ids) => {
                for job_id in job_ids {
                    let runner_state = state.clone();
                    tasks.spawn(run_recovery_job_task(runner_state, job_id, "scheduler"));
                }
            }
            Err(error) => warn!(error = %error, "Client Market recovery cycle failed"),
        }
        if maintenance_due
            && let Err(error) = state
                .store
                .client_market_prune_terminal_recovery_jobs(Utc::now())
                .await
        {
            warn!(error = %error, "failed to prune terminal Client recovery jobs");
        }
    }
}

fn report_recovery_task_result(result: Option<Result<(), tokio::task::JoinError>>) {
    if let Some(Err(error)) = result {
        warn!(error = %error, "Client Market recovery task stopped");
    }
}

async fn run_recovery_cycle(
    state: &ServerState,
    claim_limit: usize,
) -> Result<Vec<String>, AppError> {
    let candidates = state.store.client_market_recovery_candidates().await?;
    let snapshots = state
        .proxy
        .route_availability_snapshots(Duration::ZERO)
        .await;
    let observations = candidates
        .iter()
        .map(|candidate| {
            let online = snapshots
                .get(&candidate.subdomain)
                .is_some_and(|snapshot| snapshot.state == RouteAvailability::Active);
            (candidate.clone(), online)
        })
        .collect::<Vec<_>>();
    state
        .store
        .client_market_reconcile_recovery_states(&observations, Utc::now())
        .await?;
    if claim_limit == 0 {
        return Ok(Vec::new());
    }
    state
        .store
        .client_market_claim_due_recoveries(claim_limit, Utc::now())
        .await
}

pub(crate) async fn resume_interrupted_job(state: ServerState, job: ProvisioningJobRecord) {
    if job.job_type != JOB_TYPE_RECOVER {
        return;
    }
    if !state.config.client_market_recovery_enabled {
        if let Err(error) = state
            .store
            .client_market_cancel_recovery_job(&job.id, "recovery_disabled")
            .await
        {
            warn!(job_id = %job.id, error = %error, "failed to cancel disabled Client recovery");
        }
        return;
    }
    run_recovery_job_task(state, job.id, "interrupted").await;
}

async fn run_recovery_job_task(state: ServerState, job_id: String, source: &'static str) {
    let Ok(_slot) = state.client_market_recovery.slots.acquire().await else {
        warn!(job_id = %job_id, %source, "Client recovery concurrency limiter closed");
        return;
    };
    if let Err(error) = run_recovery_job(state.clone(), job_id.clone()).await {
        warn!(job_id = %job_id, %source, error = %error, "Client recovery job stopped unexpectedly");
        let mut retry_delay = Duration::from_secs(1);
        loop {
            match state
                .store
                .client_market_finish_recovery_attempt(
                    &job_id,
                    FinishAttempt {
                        kind: FinishKind::Failed,
                        outcome: "internal_error",
                        failure_code: Some("recovery_internal_error"),
                        method: None,
                        event_type: "client_recovery_attempt_failed",
                    },
                    Utc::now(),
                )
                .await
            {
                Ok(()) => break,
                Err(finalize_error) => {
                    warn!(
                        job_id = %job_id,
                        error = %finalize_error,
                        retry_secs = retry_delay.as_secs(),
                        "failed to finalize interrupted Client recovery"
                    );
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(Duration::from_secs(30));
                }
            }
        }
    }
}

async fn run_recovery_job(state: ServerState, job_id: String) -> Result<(), AppError> {
    let Some(initial) = state
        .store
        .client_market_recovery_job_context(&job_id)
        .await?
    else {
        state
            .store
            .client_market_cancel_recovery_job(&job_id, "recovery_ineligible")
            .await?;
        return Ok(());
    };

    if route_is_online(&state, &initial.subdomain).await {
        state
            .store
            .client_market_finish_recovery_attempt(
                &job_id,
                FinishAttempt {
                    kind: FinishKind::Succeeded,
                    outcome: "tunnel_returned",
                    failure_code: None,
                    method: None,
                    event_type: if initial.phase == JOB_PHASE_WAITING {
                        "client_recovery_succeeded"
                    } else {
                        "client_recovery_tunnel_returned"
                    },
                },
                Utc::now(),
            )
            .await?;
        return Ok(());
    }

    let remote_outcome = {
        let _action_guard = state
            .client_market_recovery
            .lock(&initial.installation_id)
            .await;
        let Some(context) = state
            .store
            .client_market_recovery_job_context(&job_id)
            .await?
        else {
            return Ok(());
        };
        if route_is_online(&state, &context.subdomain).await {
            state
                .store
                .client_market_finish_recovery_attempt(
                    &job_id,
                    FinishAttempt {
                        kind: FinishKind::Succeeded,
                        outcome: "tunnel_returned",
                        failure_code: None,
                        method: None,
                        event_type: "client_recovery_tunnel_returned",
                    },
                    Utc::now(),
                )
                .await?;
            return Ok(());
        }
        state
            .store
            .client_market_set_recovery_job_phase(&job_id, JOB_PHASE_CHECKING)
            .await?;
        let host = state
            .store
            .client_market_get_host(&context.host_id)
            .await?
            .ok_or_else(|| AppError::NotFound("recovery Host no longer exists".into()))?;
        if host.ssh_host_key_fingerprint.is_none() {
            Err(AppError::Conflict(
                "Host has no pinned SSH fingerprint".into(),
            ))
        } else {
            ssh_recover_market_client_process(&state, &host).await
        }
    };

    match remote_outcome {
        Ok(ClientRecoveryRemoteOutcome::AlreadyRunning) => {
            state
                .store
                .client_market_finish_recovery_attempt(
                    &job_id,
                    FinishAttempt {
                        kind: FinishKind::Succeeded,
                        outcome: "process_running",
                        failure_code: None,
                        method: None,
                        event_type: "client_recovery_process_present",
                    },
                    Utc::now(),
                )
                .await?;
        }
        Ok(ClientRecoveryRemoteOutcome::MissingBinary) => {
            finish_blocked(&state, &job_id, "missing_binary", None).await?;
        }
        Ok(ClientRecoveryRemoteOutcome::MissingConfig) => {
            finish_blocked(&state, &job_id, "missing_config", None).await?;
        }
        Ok(ClientRecoveryRemoteOutcome::StartFailed { method }) => {
            state
                .store
                .client_market_finish_recovery_attempt(
                    &job_id,
                    FinishAttempt {
                        kind: FinishKind::Failed,
                        outcome: "start_failed",
                        failure_code: Some("start_failed"),
                        method: Some(&method),
                        event_type: "client_recovery_failed",
                    },
                    Utc::now(),
                )
                .await?;
        }
        Ok(ClientRecoveryRemoteOutcome::Started { method }) => {
            state
                .store
                .client_market_set_recovery_job_phase(&job_id, JOB_PHASE_WAITING)
                .await?;
            match wait_for_recovered_tunnel(&state, &job_id, &initial.subdomain).await? {
                TunnelWaitOutcome::Online => {
                    state
                        .store
                        .client_market_finish_recovery_attempt(
                            &job_id,
                            FinishAttempt {
                                kind: FinishKind::Succeeded,
                                outcome: "restarted",
                                failure_code: None,
                                method: Some(&method),
                                event_type: "client_recovery_succeeded",
                            },
                            Utc::now(),
                        )
                        .await?;
                }
                TunnelWaitOutcome::TimedOut => {
                    state
                        .store
                        .client_market_finish_recovery_attempt(
                            &job_id,
                            FinishAttempt {
                                kind: FinishKind::Failed,
                                outcome: "tunnel_timeout",
                                failure_code: Some("tunnel_timeout"),
                                method: Some(&method),
                                event_type: "client_recovery_failed",
                            },
                            Utc::now(),
                        )
                        .await?;
                }
                TunnelWaitOutcome::Cancelled => {}
            }
        }
        Err(error) => match classify_ssh_failure(&error) {
            SshFailureClass::Blocked(reason) => {
                finish_blocked(&state, &job_id, reason, None).await?;
            }
            SshFailureClass::Transient(code) => {
                state
                    .store
                    .client_market_finish_recovery_attempt(
                        &job_id,
                        FinishAttempt {
                            kind: FinishKind::Failed,
                            outcome: code,
                            failure_code: Some(code),
                            method: None,
                            event_type: "client_recovery_attempt_failed",
                        },
                        Utc::now(),
                    )
                    .await?;
            }
        },
    }
    Ok(())
}

async fn finish_blocked(
    state: &ServerState,
    job_id: &str,
    reason: &str,
    method: Option<&str>,
) -> Result<(), AppError> {
    state
        .store
        .client_market_finish_recovery_attempt(
            job_id,
            FinishAttempt {
                kind: FinishKind::Blocked,
                outcome: "blocked",
                failure_code: Some(reason),
                method,
                event_type: "client_recovery_blocked",
            },
            Utc::now(),
        )
        .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TunnelWaitOutcome {
    Online,
    TimedOut,
    Cancelled,
}

async fn wait_for_recovered_tunnel(
    state: &ServerState,
    job_id: &str,
    subdomain: &str,
) -> Result<TunnelWaitOutcome, AppError> {
    let deadline = tokio::time::Instant::now() + TUNNEL_CONFIRM_TIMEOUT;
    loop {
        if state
            .store
            .client_market_recovery_job_context(job_id)
            .await?
            .is_none()
        {
            return Ok(TunnelWaitOutcome::Cancelled);
        }
        if route_is_online(state, subdomain).await {
            return Ok(TunnelWaitOutcome::Online);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(TunnelWaitOutcome::TimedOut);
        }
        tokio::time::sleep(TUNNEL_CONFIRM_INTERVAL).await;
    }
}

async fn route_is_online(state: &ServerState, subdomain: &str) -> bool {
    state
        .proxy
        .route_availability(subdomain, Duration::ZERO)
        .await
        .is_some_and(|snapshot| snapshot.state == RouteAvailability::Active)
}

fn classify_ssh_failure(error: &AppError) -> SshFailureClass {
    let detail = error.to_string().to_ascii_lowercase();
    if detail.contains("fingerprint")
        || detail.contains("host identification has changed")
        || detail.contains("host key verification failed")
        || detail.contains("known_hosts")
        || detail.contains("no pinned ssh")
    {
        SshFailureClass::Blocked("ssh_host_key_mismatch")
    } else if detail.contains("permission denied")
        || detail.contains("publickey")
        || detail.contains("identity file")
    {
        SshFailureClass::Blocked("ssh_authentication_failed")
    } else if detail.contains("timeout") || detail.contains("timed out") {
        SshFailureClass::Transient("ssh_timeout")
    } else {
        SshFailureClass::Transient("ssh_unavailable")
    }
}

fn backoff_after_attempt(attempt_level: u32, installation_id: &str) -> ChronoDuration {
    let capped_level = attempt_level.min(5);
    let base_seconds = match capped_level {
        0 | 1 => 10 * 60,
        2 => 30 * 60,
        3 => 60 * 60,
        4 => 3 * 60 * 60,
        _ => 6 * 60 * 60,
    };
    let digest = Sha256::digest(format!("{installation_id}:{capped_level}").as_bytes());
    let jitter_window = base_seconds / 10;
    let jitter = u64::from_be_bytes(digest[..8].try_into().expect("SHA-256 prefix"))
        % u64::try_from(jitter_window + 1).unwrap_or(1);
    ChronoDuration::seconds(base_seconds + i64::try_from(jitter).unwrap_or(0))
}

fn state_after_observation(
    existing: Option<RecoveryStateRecord>,
    candidate: &RecoveryCandidate,
    online: bool,
    now: DateTime<Utc>,
) -> RecoveryStateRecord {
    let Some(mut state) = existing else {
        return RecoveryStateRecord {
            installation_id: candidate.installation_id.clone(),
            host_id: candidate.host_id.clone(),
            observed_state: if online { "online" } else { "offline" }.into(),
            incident_started_at: (!online).then_some(now),
            stable_since: online.then_some(now),
            attempt_level: 0,
            next_attempt_at: (!online).then_some(now + INITIAL_OFFLINE_DELAY),
            last_attempt_at: None,
            last_outcome: None,
            blocked_reason: None,
            paused_at: None,
            updated_at: now,
        };
    };
    state.installation_id.clone_from(&candidate.installation_id);
    state.host_id.clone_from(&candidate.host_id);
    if state.observed_state == "paused" {
        return state;
    }
    if online {
        if state.observed_state == "online" {
            return state;
        }
        if state.observed_state != "stabilizing" {
            state.observed_state = "stabilizing".into();
            state.stable_since = Some(now);
            state.next_attempt_at = None;
            state.blocked_reason = None;
        }
        if state
            .stable_since
            .is_some_and(|stable_since| now - stable_since >= STABLE_RESET_AFTER)
        {
            state.observed_state = "online".into();
            state.incident_started_at = None;
            state.attempt_level = 0;
            state.next_attempt_at = None;
            state.blocked_reason = None;
            state.paused_at = None;
        }
        return state;
    }
    state.stable_since = None;
    if state.observed_state == "blocked" {
        return state;
    }
    if state.observed_state == "stabilizing" {
        state.observed_state = "offline".into();
        state.incident_started_at.get_or_insert(now);
        state.next_attempt_at = Some(match state.last_attempt_at {
            Some(last_attempt) => (last_attempt
                + backoff_after_attempt(state.attempt_level, &state.installation_id))
            .max(now),
            None => now + INITIAL_OFFLINE_DELAY,
        });
        state.last_outcome = Some("offline_again".into());
        return state;
    }
    if state.observed_state == "online" {
        state.observed_state = "offline".into();
        state.incident_started_at = Some(now);
        state.next_attempt_at = Some(now + INITIAL_OFFLINE_DELAY);
        return state;
    }
    state.incident_started_at.get_or_insert(now);
    if state.next_attempt_at.is_none() {
        state.next_attempt_at = Some(now + INITIAL_OFFLINE_DELAY);
    }
    state
}

impl AppStore {
    async fn client_market_recovery_candidates(&self) -> Result<Vec<RecoveryCandidate>, AppError> {
        let conn = self.conn.lock().await;
        let sql = format!(
            "SELECT i.id, h.id, t.subdomain
             FROM router_ssh_hosts h
             JOIN installations i ON i.id = h.installation_id
             JOIN client_market_subscriptions s ON s.installation_id = i.id
             JOIN installation_client_tunnels t ON t.installation_id = i.id
             WHERE {RECOVERY_ELIGIBILITY_PREDICATE}
             ORDER BY i.id"
        );
        let mut statement = conn.prepare(&sql).map_err(|error| {
            AppError::Internal(format!(
                "prepare Client recovery candidates failed: {error}"
            ))
        })?;
        let rows = statement
            .query_map([], |row| {
                Ok(RecoveryCandidate {
                    installation_id: row.get(0)?,
                    host_id: row.get(1)?,
                    subdomain: row.get(2)?,
                })
            })
            .map_err(|error| {
                AppError::Internal(format!("query Client recovery candidates failed: {error}"))
            })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|error| {
            AppError::Internal(format!("read Client recovery candidates failed: {error}"))
        })
    }

    async fn client_market_reconcile_recovery_states(
        &self,
        observations: &[(RecoveryCandidate, bool)],
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let ineligible = format!(
            "NOT EXISTS (
                SELECT 1
                FROM router_ssh_hosts h
                JOIN installations i ON i.id = h.installation_id
                JOIN client_market_subscriptions s ON s.installation_id = i.id
                JOIN installation_client_tunnels t ON t.installation_id = i.id
                WHERE i.id = client_market_recovery_state.installation_id
                  AND h.id = client_market_recovery_state.host_id
                  AND {RECOVERY_ELIGIBILITY_PREDICATE}
            )"
        );
        let has_ineligible =
            conn.query_row(
                &format!(
                    "SELECT EXISTS(
                        SELECT 1 FROM client_market_recovery_state WHERE {ineligible}
                     )"
                ),
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| {
                AppError::Internal(format!(
                    "check ineligible Client recovery state failed: {error}"
                ))
            })? != 0;
        let mut has_observation_change = false;
        for (candidate, online) in observations {
            if !recovery_binding_exists(&conn, candidate)? {
                continue;
            }
            let existing = read_recovery_state(&conn, &candidate.installation_id)?;
            let next = state_after_observation(existing.clone(), candidate, *online, now);
            if existing
                .as_ref()
                .is_none_or(|current| !same_recovery_state_ignoring_updated_at(current, &next))
            {
                has_observation_change = true;
                break;
            }
        }
        if !has_ineligible && !has_observation_change {
            return Ok(());
        }

        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!(
                    "begin Client recovery reconciliation failed: {error}"
                ))
            })?;
        let now_text = now.to_rfc3339();
        tx.execute(
            &format!(
                "UPDATE provisioning_jobs
                 SET status = 'failed', phase = 'complete', failure_code = 'recovery_ineligible',
                     log_blob = substr(COALESCE(log_blob, '') || 'recovery cancelled: installation is no longer eligible\n', -131072),
                     updated_at = ?1
                 WHERE type = 'recover' AND status IN ('pending', 'running')
                   AND installation_id IN (
                       SELECT installation_id FROM client_market_recovery_state WHERE {ineligible}
                   )"
            ),
            params![now_text],
        )
        .map_err(|error| {
            AppError::Internal(format!("cancel ineligible Client recoveries failed: {error}"))
        })?;
        tx.execute(
            &format!("DELETE FROM client_market_recovery_state WHERE {ineligible}"),
            [],
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "remove ineligible Client recovery state failed: {error}"
            ))
        })?;

        for (candidate, online) in observations {
            if !recovery_binding_exists(&tx, candidate)? {
                continue;
            }
            let existing = read_recovery_state(&tx, &candidate.installation_id)?;
            let mut next = state_after_observation(existing.clone(), candidate, *online, now);
            if existing
                .as_ref()
                .is_none_or(|current| !same_recovery_state_ignoring_updated_at(current, &next))
            {
                next.updated_at = now;
                upsert_recovery_state(&tx, &next)?;
            }
        }
        tx.commit().map_err(|error| {
            AppError::Internal(format!(
                "commit Client recovery reconciliation failed: {error}"
            ))
        })
    }

    async fn client_market_prune_terminal_recovery_jobs(
        &self,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        let cutoff = (now - TERMINAL_JOB_RETENTION).to_rfc3339();
        let conn = self.conn.lock().await;
        conn.execute(
            "DELETE FROM provisioning_jobs
             WHERE type = 'recover' AND status IN ('succeeded', 'failed') AND updated_at < ?1",
            params![cutoff],
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "prune terminal Client recovery jobs failed: {error}"
            ))
        })?;
        Ok(())
    }

    async fn client_market_claim_due_recoveries(
        &self,
        limit: usize,
        now: DateTime<Utc>,
    ) -> Result<Vec<String>, AppError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin Client recovery claims failed: {error}"))
            })?;
        let sql = format!(
            "SELECT r.installation_id, r.host_id, r.attempt_level, t.subdomain,
                    h.host_owner_email, s.client_owner_email, s.client_user_id
             FROM client_market_recovery_state r
             JOIN router_ssh_hosts h ON h.id = r.host_id
             JOIN installations i ON i.id = r.installation_id
             JOIN client_market_subscriptions s ON s.installation_id = i.id
             JOIN installation_client_tunnels t ON t.installation_id = i.id
             WHERE r.observed_state = 'offline' AND r.next_attempt_at <= ?1
               AND {RECOVERY_ELIGIBILITY_PREDICATE}
               AND NOT EXISTS (
                   SELECT 1 FROM provisioning_jobs j
                   WHERE j.host_id = r.host_id AND j.status IN ('pending', 'running')
               )
             ORDER BY r.next_attempt_at, r.installation_id
             LIMIT ?2"
        );
        type DueRow = (String, String, i64, String, String, String, String);
        let due = {
            let mut statement = tx.prepare(&sql).map_err(|error| {
                AppError::Internal(format!("prepare due Client recoveries failed: {error}"))
            })?;
            let rows = statement
                .query_map(params![now.to_rfc3339(), limit as i64], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                })
                .map_err(|error| {
                    AppError::Internal(format!("query due Client recoveries failed: {error}"))
                })?;
            rows.collect::<Result<Vec<DueRow>, _>>().map_err(|error| {
                AppError::Internal(format!("read due Client recoveries failed: {error}"))
            })?
        };
        let now_text = now.to_rfc3339();
        let mut claimed = Vec::new();
        for (
            installation_id,
            host_id,
            attempt_level,
            subdomain,
            host_owner_email,
            client_owner_email,
            client_owner_user_id,
        ) in due
        {
            let next_level = u32::try_from(attempt_level)
                .unwrap_or(0)
                .saturating_add(1)
                .min(5);
            let next_attempt_at =
                (now + backoff_after_attempt(next_level, &installation_id)).to_rfc3339();
            let job_id = Uuid::new_v4().to_string();
            let inserted = tx
                .execute(
                    "INSERT OR IGNORE INTO provisioning_jobs (
                        id, type, host_id, host_owner_email, client_owner_email,
                        selection_owners_json, selection_regions_json, subdomain, installation_id,
                        status, phase, log_blob, secret_ref, failure_code, created_at, updated_at,
                        client_owner_user_id
                     ) VALUES (?1, 'recover', ?2, ?3, ?4, '[]', '[]', ?5, ?6,
                               'running', 'recovery_checking',
                               'claimed persistent Client recovery attempt\n', NULL, NULL, ?7, ?7, ?8)",
                    params![
                        job_id,
                        host_id,
                        host_owner_email,
                        client_owner_email,
                        subdomain,
                        installation_id,
                        now_text,
                        client_owner_user_id,
                    ],
                )
                .map_err(|error| {
                    AppError::Internal(format!("insert Client recovery claim failed: {error}"))
                })?;
            if inserted != 1 {
                continue;
            }
            let state_changed = tx
                .execute(
                    "UPDATE client_market_recovery_state
                     SET attempt_level = ?3, last_attempt_at = ?4,
                         next_attempt_at = ?5, last_outcome = 'checking',
                         blocked_reason = NULL, updated_at = ?4
                     WHERE installation_id = ?1 AND host_id = ?2
                       AND observed_state = 'offline' AND next_attempt_at <= ?4",
                    params![
                        installation_id,
                        host_id,
                        i64::from(next_level),
                        now_text,
                        next_attempt_at,
                    ],
                )
                .map_err(|error| {
                    AppError::Internal(format!("advance Client recovery claim failed: {error}"))
                })?;
            if state_changed != 1 {
                return Err(AppError::Conflict(
                    "Client recovery state changed while it was being claimed".into(),
                ));
            }
            claimed.push(job_id);
        }
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit Client recovery claims failed: {error}"))
        })?;
        Ok(claimed)
    }

    async fn client_market_recovery_job_context(
        &self,
        job_id: &str,
    ) -> Result<Option<RecoveryJobContext>, AppError> {
        let conn = self.conn.lock().await;
        let sql = format!(
            "SELECT i.id, h.id, t.subdomain, j.phase
             FROM provisioning_jobs j
             JOIN router_ssh_hosts h ON h.id = j.host_id
             JOIN installations i ON i.id = j.installation_id
             JOIN client_market_subscriptions s ON s.installation_id = i.id
             JOIN installation_client_tunnels t ON t.installation_id = i.id
             WHERE j.id = ?1 AND j.type = 'recover' AND j.status = 'running'
               AND j.host_id = h.id AND j.installation_id = i.id
               AND j.subdomain = t.subdomain COLLATE NOCASE
               AND {RECOVERY_ELIGIBILITY_PREDICATE}"
        );
        conn.query_row(&sql, params![job_id], |row| {
            Ok(RecoveryJobContext {
                installation_id: row.get(0)?,
                host_id: row.get(1)?,
                subdomain: row.get(2)?,
                phase: row.get(3)?,
            })
        })
        .optional()
        .map_err(|error| {
            AppError::Internal(format!("read Client recovery job context failed: {error}"))
        })
    }

    async fn client_market_set_recovery_job_phase(
        &self,
        job_id: &str,
        phase: &str,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                "UPDATE provisioning_jobs SET phase = ?2, updated_at = ?3
                 WHERE id = ?1 AND type = 'recover' AND status = 'running'",
                params![job_id, phase, Utc::now().to_rfc3339()],
            )
            .map_err(|error| {
                AppError::Internal(format!("set Client recovery phase failed: {error}"))
            })?;
        if changed == 1 {
            Ok(())
        } else {
            Err(AppError::Conflict(
                "Client recovery job is no longer active".into(),
            ))
        }
    }

    async fn client_market_finish_recovery_attempt(
        &self,
        job_id: &str,
        finish: FinishAttempt<'_>,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin Client recovery completion failed: {error}"))
            })?;
        let job: Option<(String, String)> = tx
            .query_row(
                "SELECT installation_id, host_id FROM provisioning_jobs
                 WHERE id = ?1 AND type = 'recover' AND status = 'running'",
                params![job_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("read completing Client recovery failed: {error}"))
            })?;
        let Some((installation_id, host_id)) = job else {
            return Ok(());
        };
        let now_text = now.to_rfc3339();
        let job_status = if finish.kind == FinishKind::Succeeded {
            JOB_STATUS_SUCCEEDED
        } else {
            JOB_STATUS_FAILED
        };
        let log = format!("recovery result: {}\n", finish.outcome);
        let changed = tx
            .execute(
                "UPDATE provisioning_jobs
                 SET status = ?2, phase = ?3, failure_code = ?4,
                     log_blob = substr(COALESCE(log_blob, '') || ?5, -131072), updated_at = ?6
                 WHERE id = ?1 AND type = 'recover' AND status = 'running'",
                params![
                    job_id,
                    job_status,
                    JOB_PHASE_COMPLETE,
                    finish.failure_code,
                    log,
                    now_text,
                ],
            )
            .map_err(|error| {
                AppError::Internal(format!("complete Client recovery job failed: {error}"))
            })?;
        if changed != 1 {
            return Ok(());
        }
        let attempt_level = tx
            .query_row(
                "SELECT attempt_level FROM client_market_recovery_state
                 WHERE installation_id = ?1 AND host_id = ?2",
                params![installation_id, host_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!(
                    "read completed Client recovery level failed: {error}"
                ))
            })?
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0);
        let retry_at = (now + backoff_after_attempt(attempt_level, &installation_id)).to_rfc3339();
        match finish.kind {
            FinishKind::Succeeded if finish.outcome != "process_running" => tx.execute(
                "UPDATE client_market_recovery_state
                     SET observed_state = 'stabilizing', stable_since = ?3,
                         next_attempt_at = NULL, last_outcome = ?4,
                         blocked_reason = NULL, paused_at = NULL, updated_at = ?3
                     WHERE installation_id = ?1 AND host_id = ?2",
                params![installation_id, host_id, now_text, finish.outcome],
            ),
            FinishKind::Succeeded | FinishKind::Failed => tx.execute(
                "UPDATE client_market_recovery_state
                 SET observed_state = 'offline', stable_since = NULL,
                     next_attempt_at = ?3, last_outcome = ?4,
                     blocked_reason = NULL, updated_at = ?5
                 WHERE installation_id = ?1 AND host_id = ?2",
                params![installation_id, host_id, retry_at, finish.outcome, now_text],
            ),
            FinishKind::Blocked => tx.execute(
                "UPDATE client_market_recovery_state
                 SET observed_state = 'blocked', stable_since = NULL,
                     next_attempt_at = NULL, last_outcome = ?3,
                     blocked_reason = ?4, updated_at = ?5
                 WHERE installation_id = ?1 AND host_id = ?2",
                params![
                    installation_id,
                    host_id,
                    finish.outcome,
                    finish.failure_code,
                    now_text,
                ],
            ),
        }
        .map_err(|error| {
            AppError::Internal(format!(
                "update completed Client recovery state failed: {error}"
            ))
        })?;
        let state_snapshot: Option<(i64, Option<String>)> = tx
            .query_row(
                "SELECT attempt_level, next_attempt_at FROM client_market_recovery_state
                 WHERE installation_id = ?1 AND host_id = ?2",
                params![installation_id, host_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("read Client recovery event state failed: {error}"))
            })?;
        let (attempt_level, next_attempt_at) = state_snapshot.unwrap_or((0, None));
        crate::client_market_trade::insert_audit_tx(
            &tx,
            Some(&installation_id),
            Some(&host_id),
            None,
            Some("router-system@internal.invalid"),
            finish.event_type,
            serde_json::json!({
                "outcome": finish.outcome,
                "failureCode": finish.failure_code,
                "recoveryMethod": finish.method,
                "attemptLevel": attempt_level,
                "nextAttemptAt": next_attempt_at,
                "blockedReason": (finish.kind == FinishKind::Blocked).then_some(finish.failure_code).flatten(),
            }),
            now,
        )?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit Client recovery completion failed: {error}"))
        })
    }

    async fn client_market_cancel_recovery_job(
        &self,
        job_id: &str,
        failure_code: &str,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE provisioning_jobs
             SET status = 'failed', phase = 'complete', failure_code = ?2,
                 log_blob = substr(COALESCE(log_blob, '') || 'recovery cancelled\n', -131072),
                 updated_at = ?3
             WHERE id = ?1 AND type = 'recover' AND status IN ('pending', 'running')",
            params![job_id, failure_code, Utc::now().to_rfc3339()],
        )
        .map_err(|error| AppError::Internal(format!("cancel Client recovery failed: {error}")))?;
        Ok(())
    }

    pub(crate) async fn client_market_recovery_views_for_hosts(
        &self,
        host_ids: &[String],
    ) -> Result<HashMap<String, ClientMarketRecoveryView>, AppError> {
        if host_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let host_ids = host_ids.iter().cloned().collect::<HashSet<_>>();
        let placeholders = std::iter::repeat_n("?", host_ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let conn = self.conn.lock().await;
        let mut statement = conn
            .prepare(&format!(
                "SELECT host_id, observed_state, attempt_level, next_attempt_at,
                        last_attempt_at, last_outcome, blocked_reason, updated_at
                 FROM client_market_recovery_state WHERE host_id IN ({placeholders})"
            ))
            .map_err(|error| {
                AppError::Internal(format!("prepare Client recovery views failed: {error}"))
            })?;
        let rows = statement
            .query_map(params_from_iter(host_ids.iter()), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    ClientMarketRecoveryView {
                        state: row.get(1)?,
                        attempt_level: u32::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
                        next_attempt_at: row.get(3)?,
                        last_attempt_at: row.get(4)?,
                        last_outcome: row.get(5)?,
                        blocked_reason: row.get(6)?,
                        updated_at: row.get(7)?,
                    },
                ))
            })
            .map_err(|error| {
                AppError::Internal(format!("query Client recovery views failed: {error}"))
            })?;
        let mut output = HashMap::new();
        for row in rows {
            let (host_id, view) = row.map_err(|error| {
                AppError::Internal(format!("read Client recovery view failed: {error}"))
            })?;
            output.insert(host_id, view);
        }
        Ok(output)
    }

    async fn client_market_recovery_installation_for_control(
        &self,
        host_id: &str,
        session: &crate::models::AuthSession,
        is_admin: bool,
    ) -> Result<String, AppError> {
        let conn = self.conn.lock().await;
        let sql = format!(
            "SELECT i.id, h.provider_id, h.host_owner_email
             FROM router_ssh_hosts h
             JOIN installations i ON i.id = h.installation_id
             JOIN client_market_subscriptions s ON s.installation_id = i.id
             JOIN installation_client_tunnels t ON t.installation_id = i.id
             WHERE h.id = ?1 AND {RECOVERY_ELIGIBILITY_PREDICATE}"
        );
        let row: Option<(String, Option<String>, String)> = conn
            .query_row(&sql, params![host_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("read controlled Client recovery failed: {error}"))
            })?;
        let (installation_id, provider_id, host_owner_email) =
            row.ok_or_else(|| AppError::NotFound("eligible Client recovery not found".into()))?;
        if !is_admin && !session_is_host_owner(session, provider_id.as_deref(), &host_owner_email) {
            return Err(AppError::Forbidden(
                "only the Host Provider or Router administrator may control recovery".into(),
            ));
        }
        Ok(installation_id)
    }

    async fn client_market_apply_recovery_control(
        &self,
        host_id: &str,
        session: &crate::models::AuthSession,
        is_admin: bool,
        action: RecoveryControlAction,
        now: DateTime<Utc>,
    ) -> Result<ClientMarketRecoveryView, AppError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin Client recovery control failed: {error}"))
            })?;
        let sql = format!(
            "SELECT i.id, h.provider_id, h.host_owner_email, t.subdomain
             FROM router_ssh_hosts h
             JOIN installations i ON i.id = h.installation_id
             JOIN client_market_subscriptions s ON s.installation_id = i.id
             JOIN installation_client_tunnels t ON t.installation_id = i.id
             WHERE h.id = ?1 AND {RECOVERY_ELIGIBILITY_PREDICATE}"
        );
        let row: Option<(String, Option<String>, String, String)> = tx
            .query_row(&sql, params![host_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("validate Client recovery control failed: {error}"))
            })?;
        let (installation_id, provider_id, host_owner_email, subdomain) =
            row.ok_or_else(|| AppError::NotFound("eligible Client recovery not found".into()))?;
        if !is_admin && !session_is_host_owner(session, provider_id.as_deref(), &host_owner_email) {
            return Err(AppError::Forbidden(
                "only the Host Provider or Router administrator may control recovery".into(),
            ));
        }
        let active_jobs: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM provisioning_jobs
                 WHERE host_id = ?1 AND status IN ('pending', 'running')",
                params![host_id],
                |row| row.get(0),
            )
            .map_err(|error| {
                AppError::Internal(format!(
                    "check active recovery control jobs failed: {error}"
                ))
            })?;
        if action != RecoveryControlAction::Pause && active_jobs > 0 {
            return Err(AppError::Conflict(
                "the Host already has an active operation".into(),
            ));
        }
        let candidate = RecoveryCandidate {
            installation_id: installation_id.clone(),
            host_id: host_id.to_string(),
            subdomain,
        };
        let mut recovery = read_recovery_state(&tx, &installation_id)?
            .unwrap_or_else(|| state_after_observation(None, &candidate, false, now));
        match action {
            RecoveryControlAction::Pause => {
                tx.execute(
                    "UPDATE provisioning_jobs
                     SET status = 'failed', phase = 'complete', failure_code = 'recovery_paused',
                         log_blob = substr(COALESCE(log_blob, '') || 'recovery paused by operator\n', -131072),
                         updated_at = ?2
                     WHERE host_id = ?1 AND type = 'recover'
                       AND status IN ('pending', 'running')",
                    params![host_id, now.to_rfc3339()],
                )
                .map_err(|error| {
                    AppError::Internal(format!("pause active Client recovery failed: {error}"))
                })?;
                recovery.observed_state = "paused".into();
                recovery.next_attempt_at = None;
                recovery.stable_since = None;
                recovery.paused_at = Some(now);
                recovery.last_outcome = Some("paused".into());
            }
            RecoveryControlAction::Resume => {
                recovery.observed_state = "offline".into();
                recovery.incident_started_at = Some(now);
                recovery.stable_since = None;
                recovery.next_attempt_at = Some(now + INITIAL_OFFLINE_DELAY);
                recovery.blocked_reason = None;
                recovery.paused_at = None;
                recovery.last_outcome = Some("resumed".into());
            }
            RecoveryControlAction::Retry => {
                recovery.observed_state = "offline".into();
                recovery.incident_started_at.get_or_insert(now);
                recovery.stable_since = None;
                recovery.next_attempt_at = Some(now);
                recovery.blocked_reason = None;
                recovery.paused_at = None;
                recovery.last_outcome = Some("retry_requested".into());
            }
        }
        recovery.updated_at = now;
        upsert_recovery_state(&tx, &recovery)?;
        let (event_type, action_label) = match action {
            RecoveryControlAction::Pause => ("client_recovery_paused", "pause"),
            RecoveryControlAction::Resume => ("client_recovery_resumed", "resume"),
            RecoveryControlAction::Retry => ("client_recovery_retry_requested", "retry"),
        };
        crate::client_market_trade::insert_audit_tx(
            &tx,
            Some(&installation_id),
            Some(host_id),
            Some(&session.user_id),
            Some(&session.email),
            event_type,
            serde_json::json!({
                "action": action_label,
                "attemptLevel": recovery.attempt_level,
                "nextAttemptAt": recovery.next_attempt_at.map(|value| value.to_rfc3339()),
            }),
            now,
        )?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit Client recovery control failed: {error}"))
        })?;
        Ok(recovery_view(&recovery))
    }
}

fn recovery_binding_exists(
    conn: &crate::db::Connection,
    candidate: &RecoveryCandidate,
) -> Result<bool, AppError> {
    let sql = format!(
        "SELECT 1
         FROM router_ssh_hosts h
         JOIN installations i ON i.id = h.installation_id
         JOIN client_market_subscriptions s ON s.installation_id = i.id
         JOIN installation_client_tunnels t ON t.installation_id = i.id
         WHERE i.id = ?1 AND h.id = ?2 AND t.subdomain = ?3 COLLATE NOCASE
           AND {RECOVERY_ELIGIBILITY_PREDICATE}"
    );
    conn.query_row(
        &sql,
        params![
            candidate.installation_id,
            candidate.host_id,
            candidate.subdomain,
        ],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map(|value| value.is_some())
    .map_err(|error| {
        AppError::Internal(format!("validate Client recovery binding failed: {error}"))
    })
}

fn read_recovery_state(
    conn: &crate::db::Connection,
    installation_id: &str,
) -> Result<Option<RecoveryStateRecord>, AppError> {
    conn.query_row(
        "SELECT installation_id, host_id, observed_state, incident_started_at,
                stable_since, attempt_level, next_attempt_at, last_attempt_at,
                last_outcome, blocked_reason, paused_at, updated_at
         FROM client_market_recovery_state WHERE installation_id = ?1",
        params![installation_id],
        |row| {
            Ok(RecoveryStateRecord {
                installation_id: row.get(0)?,
                host_id: row.get(1)?,
                observed_state: row.get(2)?,
                incident_started_at: parse_optional_time(row.get(3)?, 3)?,
                stable_since: parse_optional_time(row.get(4)?, 4)?,
                attempt_level: u32::try_from(row.get::<_, i64>(5)?).unwrap_or(0),
                next_attempt_at: parse_optional_time(row.get(6)?, 6)?,
                last_attempt_at: parse_optional_time(row.get(7)?, 7)?,
                last_outcome: row.get(8)?,
                blocked_reason: row.get(9)?,
                paused_at: parse_optional_time(row.get(10)?, 10)?,
                updated_at: parse_required_time(row.get(11)?, 11)?,
            })
        },
    )
    .optional()
    .map_err(|error| AppError::Internal(format!("read Client recovery state failed: {error}")))
}

fn upsert_recovery_state(
    conn: &crate::db::Connection,
    state: &RecoveryStateRecord,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO client_market_recovery_state (
            installation_id, host_id, observed_state, incident_started_at, stable_since,
            attempt_level, next_attempt_at, last_attempt_at, last_outcome,
            blocked_reason, paused_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(installation_id) DO UPDATE SET
            host_id = excluded.host_id,
            observed_state = excluded.observed_state,
            incident_started_at = excluded.incident_started_at,
            stable_since = excluded.stable_since,
            attempt_level = excluded.attempt_level,
            next_attempt_at = excluded.next_attempt_at,
            last_attempt_at = excluded.last_attempt_at,
            last_outcome = excluded.last_outcome,
            blocked_reason = excluded.blocked_reason,
            paused_at = excluded.paused_at,
            updated_at = excluded.updated_at",
        params![
            state.installation_id,
            state.host_id,
            state.observed_state,
            state.incident_started_at.map(|value| value.to_rfc3339()),
            state.stable_since.map(|value| value.to_rfc3339()),
            i64::from(state.attempt_level),
            state.next_attempt_at.map(|value| value.to_rfc3339()),
            state.last_attempt_at.map(|value| value.to_rfc3339()),
            state.last_outcome,
            state.blocked_reason,
            state.paused_at.map(|value| value.to_rfc3339()),
            state.updated_at.to_rfc3339(),
        ],
    )
    .map_err(|error| AppError::Internal(format!("write Client recovery state failed: {error}")))?;
    Ok(())
}

fn recovery_view(state: &RecoveryStateRecord) -> ClientMarketRecoveryView {
    ClientMarketRecoveryView {
        state: state.observed_state.clone(),
        attempt_level: state.attempt_level,
        next_attempt_at: state.next_attempt_at.map(|value| value.to_rfc3339()),
        last_attempt_at: state.last_attempt_at.map(|value| value.to_rfc3339()),
        last_outcome: state.last_outcome.clone(),
        blocked_reason: state.blocked_reason.clone(),
        updated_at: state.updated_at.to_rfc3339(),
    }
}

fn same_recovery_state_ignoring_updated_at(
    left: &RecoveryStateRecord,
    right: &RecoveryStateRecord,
) -> bool {
    let mut right = right.clone();
    right.updated_at = left.updated_at;
    left == &right
}

fn parse_optional_time(
    value: Option<String>,
    column: usize,
) -> crate::db::Result<Option<DateTime<Utc>>> {
    value
        .map(|value| parse_required_time(value, column))
        .transpose()
}

fn parse_required_time(value: String, column: usize) -> crate::db::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| {
            crate::db::Error::FromSqlConversionFailure(
                column,
                crate::db::types::Type::Text,
                Box::new(error),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate() -> RecoveryCandidate {
        RecoveryCandidate {
            installation_id: "installation-1".into(),
            host_id: "host-1".into(),
            subdomain: "client-1".into(),
        }
    }

    fn offline_state(now: DateTime<Utc>) -> RecoveryStateRecord {
        state_after_observation(None, &candidate(), false, now)
    }

    async fn recovery_store() -> AppStore {
        let store = AppStore::new_in_memory_for_tests().expect("create recovery test store");
        let now = Utc::now().to_rfc3339();
        let conn = store.conn.lock().await;
        conn.execute(
            "INSERT INTO installations (
                id, public_key, platform, app_version, owner_email,
                created_at, last_seen_at, provision_source, provision_host_id
             ) VALUES ('installation-1', 'recovery-public-key', 'linux', 'test',
                       'client@example.com', ?1, ?1, 'router_market', 'host-1')",
            params![now],
        )
        .expect("insert recovery installation");
        conn.execute(
            "INSERT INTO router_ssh_hosts (
                id, ip, port, host_owner_email, ssh_host_key_fingerprint,
                status, installation_id, created_at, updated_at, provider_id
             ) VALUES ('host-1', '198.51.100.10', 22, 'provider@example.com',
                       'SHA256:test', 'allocated', 'installation-1', ?1, ?1, 'provider-1')",
            params![now],
        )
        .expect("insert recovery Host");
        conn.execute(
            "INSERT INTO installation_client_tunnels (
                installation_id, owner_email, subdomain, enabled, created_at, updated_at
             ) VALUES ('installation-1', 'client@example.com', 'client-1', 1, ?1, ?1)",
            params![now],
        )
        .expect("insert recovery tunnel");
        conn.execute(
            "INSERT INTO client_market_subscriptions (
                installation_id, host_id, provider_id, host_owner_email,
                client_user_id, client_owner_email, status, offer_revision,
                created_at, updated_at
             ) VALUES ('installation-1', 'host-1', 'provider-1', 'provider@example.com',
                       'client-1', 'client@example.com', 'active', 1, ?1, ?1)",
            params![now],
        )
        .expect("insert recovery subscription");
        drop(conn);
        store
    }

    fn session(user_id: &str, email: &str) -> crate::models::AuthSession {
        let now = Utc::now();
        crate::models::AuthSession {
            session_id: format!("session-{user_id}"),
            user_id: user_id.into(),
            email: email.into(),
            installation_id: "browser-installation".into(),
            access_token_hash: "access".into(),
            refresh_token_hash: "refresh".into(),
            access_expires_at: now + ChronoDuration::hours(1),
            refresh_expires_at: now + ChronoDuration::days(1),
            created_at: now,
            last_used_at: now,
        }
    }

    #[test]
    fn initial_offline_observation_waits_thirty_seconds() {
        let now = Utc::now();
        let state = offline_state(now);
        assert_eq!(state.observed_state, "offline");
        assert_eq!(
            state.next_attempt_at,
            Some(now + ChronoDuration::seconds(30))
        );
        assert!(state.next_attempt_at.unwrap() > now + ChronoDuration::seconds(29));
    }

    #[test]
    fn coordinator_limits_all_recovery_sources_to_four_slots() {
        let coordinator = ClientMarketRecoveryCoordinator::default();
        let permits = (0..MAX_CONCURRENT_RECOVERIES)
            .map(|_| coordinator.slots.try_acquire().expect("recovery slot"))
            .collect::<Vec<_>>();
        assert!(coordinator.slots.try_acquire().is_err());
        drop(permits);
        assert!(coordinator.slots.try_acquire().is_ok());
    }

    #[test]
    fn stable_recovery_resets_backoff_only_after_ten_minutes() {
        let now = Utc::now();
        let mut state = offline_state(now);
        state.attempt_level = 3;
        state.last_attempt_at = Some(now);
        let stabilizing = state_after_observation(
            Some(state),
            &candidate(),
            true,
            now + ChronoDuration::seconds(1),
        );
        assert_eq!(stabilizing.observed_state, "stabilizing");
        assert_eq!(stabilizing.attempt_level, 3);
        let still_stabilizing = state_after_observation(
            Some(stabilizing.clone()),
            &candidate(),
            true,
            now + ChronoDuration::minutes(10),
        );
        assert_eq!(still_stabilizing.observed_state, "stabilizing");
        let stable = state_after_observation(
            Some(stabilizing),
            &candidate(),
            true,
            now + ChronoDuration::minutes(10) + ChronoDuration::seconds(1),
        );
        assert_eq!(stable.observed_state, "online");
        assert_eq!(stable.attempt_level, 0);
        assert!(stable.incident_started_at.is_none());
    }

    #[test]
    fn dropping_during_stabilization_preserves_escalated_backoff() {
        let now = Utc::now();
        let mut state = offline_state(now);
        state.attempt_level = 2;
        state.last_attempt_at = Some(now);
        state.observed_state = "stabilizing".into();
        state.stable_since = Some(now + ChronoDuration::minutes(1));
        state.next_attempt_at = None;
        let dropped = state_after_observation(
            Some(state),
            &candidate(),
            false,
            now + ChronoDuration::minutes(2),
        );
        assert_eq!(dropped.observed_state, "offline");
        assert_eq!(dropped.attempt_level, 2);
        assert!(
            dropped.next_attempt_at.unwrap() >= now + ChronoDuration::minutes(30),
            "second recovery must retain at least the 30 minute backoff"
        );
    }

    #[test]
    fn backoff_schedule_escalates_and_caps_at_six_hours() {
        let id = "installation-stable-jitter";
        let delays = (1..=6)
            .map(|level| backoff_after_attempt(level, id).num_seconds())
            .collect::<Vec<_>>();
        assert!((600..=660).contains(&delays[0]));
        assert!((1800..=1980).contains(&delays[1]));
        assert!((3600..=3960).contains(&delays[2]));
        assert!((10_800..=11_880).contains(&delays[3]));
        assert!((21_600..=23_760).contains(&delays[4]));
        assert_eq!(delays[4], delays[5]);
    }

    #[test]
    fn permanent_ssh_failures_are_blocked() {
        for error in [
            AppError::Conflict("SSH host key fingerprint mismatch".into()),
            AppError::BadRequest("Permission denied (publickey)".into()),
        ] {
            assert!(matches!(
                classify_ssh_failure(&error),
                SshFailureClass::Blocked(_)
            ));
        }
        assert_eq!(
            classify_ssh_failure(&AppError::ServiceUnavailable("ssh timeout".into())),
            SshFailureClass::Transient("ssh_timeout")
        );
    }

    #[tokio::test]
    async fn persistent_claim_waits_thirty_seconds_and_serializes_the_host() {
        let store = recovery_store().await;
        let observed_at = Utc::now();
        store
            .client_market_reconcile_recovery_states(&[(candidate(), false)], observed_at)
            .await
            .expect("record offline incident");
        assert!(
            store
                .client_market_claim_due_recoveries(4, observed_at + ChronoDuration::seconds(29))
                .await
                .expect("claim before threshold")
                .is_empty()
        );
        let claims = store
            .client_market_claim_due_recoveries(4, observed_at + ChronoDuration::seconds(30))
            .await
            .expect("claim at threshold");
        assert_eq!(claims.len(), 1);
        assert!(
            store
                .client_market_claim_due_recoveries(4, observed_at + ChronoDuration::hours(1))
                .await
                .expect("second claim")
                .is_empty(),
            "the active provisioning_jobs row must serialize the Host"
        );
        let conn = store.conn.lock().await;
        let (job_type, status, attempt_level): (String, String, i64) = conn
            .query_row(
                "SELECT j.type, j.status, r.attempt_level
                 FROM provisioning_jobs j
                 JOIN client_market_recovery_state r ON r.installation_id = j.installation_id
                 WHERE j.id = ?1",
                params![claims[0]],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read persistent recovery claim");
        assert_eq!(job_type, JOB_TYPE_RECOVER);
        assert_eq!(status, "running");
        assert_eq!(attempt_level, 1);
    }

    #[tokio::test]
    async fn disabled_recovery_claim_can_be_cancelled_without_remote_work() {
        let store = recovery_store().await;
        let observed_at = Utc::now();
        store
            .client_market_reconcile_recovery_states(&[(candidate(), false)], observed_at)
            .await
            .unwrap();
        let job_id = store
            .client_market_claim_due_recoveries(1, observed_at + ChronoDuration::seconds(30))
            .await
            .unwrap()
            .pop()
            .expect("claim recovery");
        store
            .client_market_cancel_recovery_job(&job_id, "recovery_disabled")
            .await
            .expect("cancel disabled recovery");

        assert!(
            store
                .client_market_recovery_job_context(&job_id)
                .await
                .unwrap()
                .is_none()
        );
        let conn = store.conn.lock().await;
        let (status, failure_code): (String, Option<String>) = conn
            .query_row(
                "SELECT status, failure_code FROM provisioning_jobs WHERE id = ?1",
                params![job_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "failed");
        assert_eq!(failure_code.as_deref(), Some("recovery_disabled"));
    }

    #[tokio::test]
    async fn retry_backoff_starts_after_the_intervention_finishes() {
        let store = recovery_store().await;
        let observed_at = Utc::now();
        store
            .client_market_reconcile_recovery_states(&[(candidate(), false)], observed_at)
            .await
            .unwrap();
        let job_id = store
            .client_market_claim_due_recoveries(1, observed_at + ChronoDuration::seconds(30))
            .await
            .unwrap()
            .pop()
            .expect("claim recovery");
        let finished_at = observed_at + ChronoDuration::minutes(5);
        store
            .client_market_finish_recovery_attempt(
                &job_id,
                FinishAttempt {
                    kind: FinishKind::Failed,
                    outcome: "ssh_timeout",
                    failure_code: Some("ssh_timeout"),
                    method: None,
                    event_type: "client_recovery_attempt_failed",
                },
                finished_at,
            )
            .await
            .expect("finish recovery");

        let state = {
            let conn = store.conn.lock().await;
            read_recovery_state(&conn, "installation-1")
                .unwrap()
                .expect("read recovery state")
        };
        let retry_at = state.next_attempt_at.expect("next retry");
        assert!(retry_at >= finished_at + ChronoDuration::minutes(10));
        assert!(retry_at <= finished_at + ChronoDuration::minutes(11));
    }

    #[tokio::test]
    async fn blocked_recovery_reason_stays_out_of_shared_client_chat() {
        let store = recovery_store().await;
        let observed_at = Utc::now();
        store
            .client_market_reconcile_recovery_states(&[(candidate(), false)], observed_at)
            .await
            .unwrap();
        let job_id = store
            .client_market_claim_due_recoveries(1, observed_at + ChronoDuration::seconds(30))
            .await
            .unwrap()
            .pop()
            .expect("claim recovery");
        store
            .client_market_finish_recovery_attempt(
                &job_id,
                FinishAttempt {
                    kind: FinishKind::Blocked,
                    outcome: "blocked",
                    failure_code: Some("ssh_authentication_failed"),
                    method: None,
                    event_type: "client_recovery_blocked",
                },
                observed_at + ChronoDuration::seconds(31),
            )
            .await
            .expect("block recovery");

        let conn = store.conn.lock().await;
        let chat_payload: String = conn
            .query_row(
                "SELECT payload_json FROM client_chat_system_outbox
                 WHERE event_type = 'client_recovery_blocked'",
                [],
                |row| row.get(0),
            )
            .expect("read recovery chat payload");
        assert!(!chat_payload.contains("ssh_authentication_failed"));
        assert!(!chat_payload.contains("blockedReason"));
        let audit_detail: String = conn
            .query_row(
                "SELECT detail_json FROM client_market_subscription_events
                 WHERE event_type = 'client_recovery_blocked'",
                [],
                |row| row.get(0),
            )
            .expect("read recovery audit detail");
        assert!(audit_detail.contains("ssh_authentication_failed"));
    }

    #[tokio::test]
    async fn cleanup_atomically_preempts_an_active_recovery() {
        let store = recovery_store().await;
        let observed_at = Utc::now();
        store
            .client_market_reconcile_recovery_states(&[(candidate(), false)], observed_at)
            .await
            .unwrap();
        let recovery_job = store
            .client_market_claim_due_recoveries(1, observed_at + ChronoDuration::seconds(30))
            .await
            .unwrap()
            .pop()
            .expect("claim recovery");
        let cleanup_job = store
            .client_market_begin_system_cleanup_job("installation-1", "service_terminated")
            .await
            .expect("cleanup preempts recovery");
        let conn = store.conn.lock().await;
        let recovery_status: (String, Option<String>) = conn
            .query_row(
                "SELECT status, failure_code FROM provisioning_jobs WHERE id = ?1",
                params![recovery_job],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(recovery_status.0, "failed");
        assert_eq!(recovery_status.1.as_deref(), Some("recovery_cancelled"));
        let cleanup_status: (String, String) = conn
            .query_row(
                "SELECT type, status FROM provisioning_jobs WHERE id = ?1",
                params![cleanup_job],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(cleanup_status, ("cleanup".into(), "pending".into()));
        let state_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM client_market_recovery_state WHERE installation_id = 'installation-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state_count, 0);
    }

    #[tokio::test]
    async fn billing_suspension_atomically_preempts_an_active_recovery() {
        let store = recovery_store().await;
        let observed_at = Utc::now();
        store
            .client_market_reconcile_recovery_states(&[(candidate(), false)], observed_at)
            .await
            .unwrap();
        let recovery_job = store
            .client_market_claim_due_recoveries(1, observed_at + ChronoDuration::seconds(30))
            .await
            .unwrap()
            .pop()
            .expect("claim recovery");
        store
            .client_market_set_billing_suspended("installation-1", true)
            .await
            .expect("billing suspension preempts recovery");

        let conn = store.conn.lock().await;
        let recovery_status: (String, Option<String>) = conn
            .query_row(
                "SELECT status, failure_code FROM provisioning_jobs WHERE id = ?1",
                params![recovery_job],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(recovery_status.0, "failed");
        assert_eq!(
            recovery_status.1.as_deref(),
            Some("recovery_suspended_for_billing")
        );
        let recovery_states: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM client_market_recovery_state
                 WHERE installation_id = 'installation-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(recovery_states, 0);
    }

    #[tokio::test]
    async fn only_provider_or_admin_can_control_recovery() {
        let store = recovery_store().await;
        let now = Utc::now();
        store
            .client_market_reconcile_recovery_states(&[(candidate(), false)], now)
            .await
            .unwrap();
        let provider = session("provider-1", "provider@example.com");
        let paused = store
            .client_market_apply_recovery_control(
                "host-1",
                &provider,
                false,
                RecoveryControlAction::Pause,
                now,
            )
            .await
            .expect("Provider pauses recovery");
        assert_eq!(paused.state, "paused");
        let renter = session("client-1", "client@example.com");
        assert!(matches!(
            store
                .client_market_apply_recovery_control(
                    "host-1",
                    &renter,
                    false,
                    RecoveryControlAction::Retry,
                    now,
                )
                .await,
            Err(AppError::Forbidden(_))
        ));
        let retried = store
            .client_market_apply_recovery_control(
                "host-1",
                &provider,
                false,
                RecoveryControlAction::Retry,
                now,
            )
            .await
            .expect("Provider requests immediate retry");
        assert_eq!(retried.state, "offline");
        assert_eq!(
            retried.next_attempt_at.as_deref(),
            Some(now.to_rfc3339().as_str())
        );
        let admin = session("admin-1", "admin@example.com");
        let admin_paused = store
            .client_market_apply_recovery_control(
                "host-1",
                &admin,
                true,
                RecoveryControlAction::Pause,
                now,
            )
            .await
            .expect("Router administrator pauses recovery");
        assert_eq!(admin_paused.state, "paused");
    }
}
