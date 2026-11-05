//! Rebuild display health from immutable observations, never from scheduling
//! availability or the mutable legacy runtime summary. Each indexed lookup is
//! bounded to the current epoch and three observations; history is not rewritten.
use super::*;
use crate::models::ShareManualModelHealthObservation;
use crate::models::ShareModelHealthStatus;

const FRESH_FOR_SECS: i64 = 65 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectionTestHealthContext {
    share_id: String,
    installation_id: String,
    epoch_id: String,
    config_revision: i64,
    pub(crate) degraded_threshold_ms: Option<u64>,
    evidence: ShareManualModelHealthObservation,
}

fn manual_context(
    conn: &Connection,
    share_id: &str,
    now: DateTime<Utc>,
) -> Result<Option<ConnectionTestHealthContext>, AppError> {
    let row = conn
        .query_row(
            "SELECT installation_id, capacity_pool_id, config_revision, share_status,
                enabled_claude, enabled_codex, enabled_gemini, app_runtimes_json,
                bindings_json, model_health_degraded_threshold_ms
         FROM shares WHERE share_id=?1",
            params![share_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    ShareSupport {
                        claude: row.get::<_, i64>(4)? != 0,
                        codex: row.get::<_, i64>(5)? != 0,
                        gemini: row.get::<_, i64>(6)? != 0,
                    },
                    parse_app_runtimes(row.get(7)?)?,
                    parse_share_bindings(row.get(8)?)?,
                    row.get::<_, Option<u64>>(9)?,
                ))
            },
        )
        .optional()
        .map_err(|e| AppError::Internal(format!("read manual model health context: {e}")))?;
    let Some((
        installation_id,
        capacity_pool_id,
        config_revision,
        state,
        support,
        runtimes,
        bindings,
        threshold,
    )) = row
    else {
        return Ok(None);
    };
    if state != "active" {
        return Ok(None);
    }
    let Some((app, api, runtime, probe)) = automatic_share_probe(&support, &runtimes, &bindings)
    else {
        return Ok(None);
    };
    let input = ShareModelProbeEpochInput {
        app_type: app.into(),
        api_type: api.into(),
        provider_id: bindings[app].clone(),
        provider_name: runtime.provider_name.clone(),
        capacity_pool_id,
        requested_model: probe.requested_model.clone(),
        wire_model: model_probe_actual_model(runtime, probe),
        policy_mode: runtime.model_policy.as_ref().map(|p| match p {
            crate::models::ShareProviderModelPolicy::Single { .. } => "single".into(),
            crate::models::ShareProviderModelPolicy::Passthrough => "passthrough".into(),
        }),
        health_fingerprint: probe.health_fingerprint.clone(),
    };
    // Configuration eligibility is synchronized without executing a probe.
    // A new epoch may start next slot; manual evidence need not wait for it.
    sync_share_model_probe_epoch_tx(
        conn,
        share_id,
        now.timestamp().div_euclid(1800) * 1800,
        Some(&input),
    )?;
    let Some(epoch) = read_open_share_model_probe_epoch(conn, share_id)? else {
        return Ok(None);
    };
    if !share_model_probe_epoch_matches(&epoch, &input) {
        return Ok(None);
    }
    Ok(Some(ConnectionTestHealthContext {
        share_id: share_id.into(),
        installation_id,
        epoch_id: epoch.epoch_id,
        config_revision,
        degraded_threshold_ms: threshold,
        evidence: ShareManualModelHealthObservation {
            observation_id: String::new(),
            app_type: app.into(),
            provider_id: input.provider_id,
            health_fingerprint: input.health_fingerprint,
            requested_model: input.requested_model,
            actual_model: input.wire_model,
            status: String::new(),
            status_code: None,
            latency_ms: 0,
            checked_at: 0,
        },
    }))
}

fn record_manual(
    conn: &Connection,
    context: &ConnectionTestHealthContext,
    source: &str,
    evidence: &ShareManualModelHealthObservation,
    now: DateTime<Utc>,
) -> Result<bool, AppError> {
    let expected = &context.evidence;
    if evidence.observation_id.is_empty()
        || evidence.observation_id.len() > 160
        || evidence.app_type != expected.app_type
        || evidence.provider_id != expected.provider_id
        || evidence.health_fingerprint != expected.health_fingerprint
        || evidence.requested_model != expected.requested_model
        || evidence.actual_model != expected.actual_model
        || !matches!(evidence.status.as_str(), "success" | "degraded" | "failed")
        || evidence.checked_at <= 0
        || evidence.checked_at > now.timestamp()
        || evidence.checked_at.saturating_add(FRESH_FOR_SECS) <= now.timestamp()
        || evidence.latency_ms > 7 * 60 * 1000
        || (evidence.status != "failed"
            && !evidence
                .status_code
                .is_some_and(|code| (200..300).contains(&code)))
    {
        return Ok(false);
    }
    // A cached result from before a pause / A→B→A configuration change must
    // not become evidence for the new epoch, even with the same fingerprint.
    let predates_reopened_epoch: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM share_model_probe_epochs e
         WHERE e.epoch_id=?1 AND e.created_at>?2 AND (EXISTS(
             SELECT 1 FROM share_model_probe_epochs old WHERE old.share_id=e.share_id
             AND old.epoch_id<>e.epoch_id AND old.health_fingerprint=e.health_fingerprint)
             OR EXISTS(SELECT 1 FROM share_model_live_observations old
                 WHERE old.share_id=e.share_id AND old.health_fingerprint=e.health_fingerprint
                 AND (old.probe_epoch_id IS NULL OR old.probe_epoch_id<>e.epoch_id))))",
            params![context.epoch_id, evidence.checked_at],
            |row| row.get(0),
        )
        .map_err(|e| AppError::Internal(format!("validate manual health epoch: {e}")))?;
    if predates_reopened_epoch {
        return Ok(false);
    }
    let changed = conn
        .execute(
            "INSERT INTO share_model_live_observations
         (share_id,source,observation_id,probe_epoch_id,installation_id,app_type,provider_id,
          health_fingerprint,requested_model,actual_model,status,status_code,latency_ms,checked_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
         ON CONFLICT(share_id,source,observation_id) DO NOTHING",
            params![
                context.share_id,
                source,
                evidence.observation_id,
                context.epoch_id,
                context.installation_id,
                evidence.app_type,
                evidence.provider_id,
                evidence.health_fingerprint,
                evidence.requested_model,
                evidence.actual_model,
                evidence.status,
                evidence.status_code,
                evidence.latency_ms,
                evidence.checked_at
            ],
        )
        .map_err(|e| AppError::Internal(format!("record manual model health: {e}")))?;
    Ok(changed > 0)
}

pub(super) fn record_server_manual(
    conn: &Connection,
    snapshot: &ShareRuntimeSnapshotResponse,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    if snapshot.manual_model_health.is_empty() {
        return Ok(());
    }
    let Some(context) = manual_context(conn, &snapshot.share_id, now)? else {
        return Ok(());
    };
    // At most one representative App. Bound intake even for a faulty Server.
    for evidence in snapshot.manual_model_health.iter().take(3) {
        record_manual(conn, &context, "server_manual", evidence, now)?;
    }
    Ok(())
}

impl AppStore {
    pub(crate) async fn prepare_connection_test_health(
        &self,
        share_id: &str,
        app: &str,
        expected_probe: &crate::models::ProviderModelProbe,
    ) -> Result<Option<ConnectionTestHealthContext>, AppError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| AppError::Internal(format!("begin connection health context: {e}")))?;
        let context = manual_context(&tx, share_id, Utc::now())?;
        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit connection health context: {e}")))?;
        Ok(context.filter(|c| {
            c.evidence.app_type == app
                && c.evidence.health_fingerprint == expected_probe.health_fingerprint
                && c.evidence.requested_model == expected_probe.requested_model
        }))
    }

    /// Only trusted text-test outcomes reach here. Caller/quota/control errors
    /// are deliberately not converted into a shared Provider failure.
    pub(crate) async fn finish_connection_test_health(
        &self,
        before: &ConnectionTestHealthContext,
        status: &str,
        status_code: u16,
        latency_ms: u64,
    ) -> Result<bool, AppError> {
        let conn = self.conn.lock().await;
        let now = Utc::now();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| AppError::Internal(format!("begin connection live health: {e}")))?;
        let Some(after) = manual_context(&tx, &before.share_id, now)? else {
            return Ok(false);
        };
        if &after != before {
            return Ok(false);
        }
        let mut evidence = before.evidence.clone();
        evidence.observation_id = Uuid::new_v4().to_string();
        evidence.checked_at = now.timestamp();
        evidence.status = status.into();
        evidence.status_code = Some(status_code);
        evidence.latency_ms = latency_ms;
        let recorded = record_manual(&tx, before, "router_connection_test", &evidence, now)?;
        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit connection live health: {e}")))?;
        Ok(recorded)
    }
}

/// Live display may acknowledge a current manual test, but it never changes
/// cycle streaks, SLA/history, readiness or scheduling. Repeated clicks cannot
/// clear the recovery window, and newer cycle evidence takes precedence.
pub(crate) fn live_status(
    conn: &Connection,
    share_id: &str,
    support: &ShareSupport,
    runtimes: &ShareAppRuntimes,
    bindings: &BTreeMap<String, String>,
    now: DateTime<Utc>,
    canonical: &ShareModelHealthStatus,
) -> Result<ShareModelHealthStatus, AppError> {
    let mut status = canonical.clone();
    status.source = Some("periodic".into());
    let Some((app, _, runtime, probe)) = automatic_share_probe(support, runtimes, bindings) else {
        return Ok(status);
    };
    let policy_mode = runtime.model_policy.as_ref().map(|policy| match policy {
        crate::models::ShareProviderModelPolicy::Single { .. } => "single",
        crate::models::ShareProviderModelPolicy::Passthrough => "passthrough",
    });
    let row = conn.query_row(
        "SELECT m.status,m.status_code,m.latency_ms,m.checked_at,m.source
         FROM share_model_live_observations m
         JOIN share_model_probe_epochs e ON e.epoch_id=m.probe_epoch_id
         JOIN shares sh ON sh.share_id=m.share_id
         WHERE m.share_id=?1 AND e.ends_at_slot IS NULL AND sh.share_status='active'
           AND m.installation_id=sh.installation_id AND e.capacity_pool_id=sh.capacity_pool_id
           AND m.app_type=?2 AND m.provider_id=?3 AND m.health_fingerprint=?4
           AND m.requested_model=?5 AND m.actual_model=?6
           AND e.app_type=m.app_type AND e.provider_id=m.provider_id
           AND e.health_fingerprint=m.health_fingerprint AND e.requested_model=m.requested_model
           AND e.wire_model=m.actual_model AND e.policy_mode IS ?8
           AND m.checked_at<=?7 AND m.checked_at>?7-3900
         ORDER BY m.checked_at DESC, CASE m.status WHEN 'failed' THEN 0 WHEN 'degraded' THEN 1 ELSE 2 END,
                  m.source, m.observation_id LIMIT 1",
        params![share_id, app, bindings[app], probe.health_fingerprint, probe.requested_model,
            model_probe_actual_model(runtime, probe), now.timestamp(), policy_mode],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<u16>>(1)?, row.get::<_, u64>(2)?,
            row.get::<_, i64>(3)?, row.get::<_, String>(4)?))
    ).optional().map_err(|e| AppError::Internal(format!("read live model health: {e}")))?;
    let Some((result, code, latency, checked_at, source)) = row else {
        return Ok(status);
    };
    if canonical.checked_at.is_some_and(|time| time >= checked_at) {
        return Ok(status);
    }
    let current_cycle_failures = canonical
        .stale_at
        .is_some_and(|time| time > now.timestamp())
        && canonical.recent_results.iter().any(|s| s != "success");
    let verified_through = if canonical.state == "healthy" {
        canonical.checked_at.unwrap_or(0)
    } else {
        0
    };
    let recent_manual_failure: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM share_model_live_observations m
         JOIN share_model_probe_epochs e ON e.epoch_id=m.probe_epoch_id
         WHERE m.share_id=?1 AND e.ends_at_slot IS NULL
           AND m.app_type=?2 AND m.provider_id=?3 AND m.health_fingerprint=?4
           AND m.requested_model=?5 AND m.actual_model=?6
           AND m.checked_at>?7-3900 AND m.checked_at<=?8 AND m.checked_at>?9
           AND m.status<>'success')",
            params![
                share_id,
                app,
                bindings[app],
                probe.health_fingerprint,
                probe.requested_model,
                model_probe_actual_model(runtime, probe),
                now.timestamp(),
                checked_at,
                verified_through
            ],
            |row| row.get(0),
        )
        .map_err(|e| AppError::Internal(format!("read live health recovery guard: {e}")))?;
    status.state = if result == "success" && !current_cycle_failures && !recent_manual_failure {
        "healthy"
    } else if result == "failed" && canonical.state == "unavailable" {
        "unavailable"
    } else {
        "degraded"
    }
    .into();
    status.reason_code = if result == "success" && status.state == "degraded" {
        "probe_recovering"
    } else if result == "degraded" {
        "probe_slow"
    } else if result == "failed" && code == Some(429) {
        "upstream_rate_limited"
    } else if result == "failed" {
        "manual_probe_failed"
    } else {
        "manual_probe_verified"
    }
    .into();
    status.checked_at = Some(checked_at);
    status.stale_at = Some(checked_at.saturating_add(FRESH_FOR_SECS));
    status.status_code = code;
    status.latency_ms = Some(latency);
    status.source = Some(source);
    Ok(status)
}

pub(crate) fn current_status(
    conn: &Connection,
    share_id: &str,
    support: &ShareSupport,
    runtimes: &ShareAppRuntimes,
    bindings: &BTreeMap<String, String>,
    now: DateTime<Utc>,
) -> Result<ShareModelHealthStatus, AppError> {
    let Some((app, api, runtime, probe)) = automatic_share_probe(support, runtimes, bindings)
    else {
        return Ok(ShareModelHealthStatus::default());
    };
    let actual_model = model_probe_actual_model(runtime, probe);
    let policy_mode = runtime.model_policy.as_ref().map(|policy| match policy {
        crate::models::ShareProviderModelPolicy::Passthrough => "passthrough",
        crate::models::ShareProviderModelPolicy::Single { .. } => "single",
    });
    let mut status = ShareModelHealthStatus {
        app_type: Some(app.into()),
        model: Some(actual_model.clone()),
        health_fingerprint: Some(probe.health_fingerprint.clone()),
        reason_code: "awaiting_probe".into(),
        ..Default::default()
    };
    let epoch: Option<String> = conn
        .query_row(
            "SELECT e.epoch_id FROM share_model_probe_epochs e
         JOIN shares sh ON sh.share_id = e.share_id
         WHERE e.share_id = ?1 AND e.ends_at_slot IS NULL AND e.evidence_version = 2
           AND sh.share_status = 'active' AND e.capacity_pool_id = sh.capacity_pool_id
           AND e.app_type = ?2 AND e.api_type = ?3 AND e.provider_id = ?4
           AND e.health_fingerprint = ?5 AND e.requested_model = ?6 AND e.wire_model = ?7
           AND e.starts_at_slot <= ?8 AND e.policy_mode IS ?9",
            params![
                share_id,
                app,
                api,
                bindings[app],
                probe.health_fingerprint,
                probe.requested_model,
                actual_model,
                now.timestamp(),
                policy_mode
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| AppError::Internal(format!("read current model health epoch: {e}")))?;
    let Some(epoch) = epoch else {
        return Ok(status);
    };
    let mut statement = conn
        .prepare(
            "SELECT o.status, o.outcome, o.status_code, o.checked_at, o.reason_code, o.latency_ms
         FROM share_model_health_slots s
         JOIN share_model_probe_observations o ON o.observation_id = s.observation_id
         JOIN share_model_probe_epochs e ON e.epoch_id = s.probe_epoch_id
         JOIN shares sh ON sh.share_id = e.share_id
         WHERE s.probe_epoch_id = ?1 AND s.share_id = ?2
           AND o.evidence_version = 2 AND o.evidence_scope = 'provider_runtime'
           AND o.installation_id = sh.installation_id
           AND o.capacity_pool_id = e.capacity_pool_id
           AND o.app_type = e.app_type AND o.api_type = e.api_type
           AND o.provider_id = e.provider_id AND o.health_fingerprint = e.health_fingerprint
           AND o.requested_model = e.requested_model AND o.actual_model = e.wire_model
           AND o.slot_start = s.slot_start AND s.slot_start >= e.starts_at_slot
           AND o.checked_at <= ?3 AND s.slot_start <= ?3
         ORDER BY s.slot_start DESC LIMIT 3",
        )
        .map_err(|e| AppError::Internal(format!("prepare current model health: {e}")))?;
    let observations = statement
        .query_map(params![epoch, share_id, now.timestamp()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<u16>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, u64>(5)?,
            ))
        })
        .map_err(|e| AppError::Internal(format!("query current model health: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Internal(format!("read current model health: {e}")))?;
    let Some(latest) = observations.first() else {
        return Ok(status);
    };
    status.checked_at = Some(latest.3);
    status.stale_at = Some(latest.3.saturating_add(FRESH_FOR_SECS));
    status.status_code = latest.2;
    status.latency_ms = Some(latest.5);
    status.recent_results = observations
        .iter()
        .map(|row| {
            if row.1 == "failure" {
                "failed".into()
            } else {
                row.0.clone()
            }
        })
        .collect();
    if now.timestamp() >= latest.3.saturating_add(FRESH_FOR_SECS) {
        status.reason_code = "probe_stale".into();
        return Ok(status);
    }
    status.state = if status.recent_results.len() == 3
        && status
            .recent_results
            .iter()
            .all(|result| result == "failed")
    {
        "unavailable"
    } else if status
        .recent_results
        .iter()
        .any(|result| result != "success")
    {
        "degraded"
    } else {
        "healthy"
    }
    .into();
    status.reason_code = if latest.0 == "success" && status.state == "degraded" {
        "probe_recovering".into()
    } else {
        latest.4.clone().unwrap_or_else(|| "probe_observed".into())
    };
    Ok(status)
}

/// Runtime snapshots are advisory and cannot replace a fixed cycle observation
/// (including legacy snapshots from Servers with transient-failure tolerance).
pub(super) fn has_canonical_check(
    conn: &Connection,
    check: &ShareModelHealthCheckEntry,
) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM share_model_health_slots s
         JOIN share_model_probe_observations o ON o.observation_id = s.observation_id
         WHERE s.share_id = ?1 AND s.slot_start BETWEEN ?4 - 2400 AND ?4 + 300
           AND o.app_type = ?2 AND o.requested_model = ?3 AND o.checked_at = ?4)",
        params![
            check.share_id,
            check.app_type,
            check.requested_model,
            check.checked_at
        ],
        |row| row.get(0),
    )
    .map_err(|e| AppError::Internal(format!("read canonical model health check: {e}")))
}
