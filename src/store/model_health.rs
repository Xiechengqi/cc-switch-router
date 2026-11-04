//! Rebuild display health from immutable observations, never from scheduling
//! availability or the mutable legacy runtime summary. Each indexed lookup is
//! bounded to the current epoch and three observations; history is not rewritten.
use super::*;
use crate::models::ShareModelHealthStatus;

const FRESH_FOR_SECS: i64 = 65 * 60;

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
            "SELECT o.status, o.outcome, o.status_code, o.checked_at, o.reason_code
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
