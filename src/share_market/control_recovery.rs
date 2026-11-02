//! Durable, evidence-driven recovery. A timeout is an unknown result: replay
//! the original payload until an explicit conflict proves it was not applied.
use super::*;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlProgress {
    pub stage: &'static str,
    pub attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_attempt_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

pub(super) fn progress_tx(
    conn: &Connection,
    record: &SubscriptionRecord,
) -> Result<Option<ControlProgress>, AppError> {
    if !matches!(
        record.status.as_str(),
        SUB_REVOKE_PENDING
            | SUB_REVOKE_FAILED
            | SUB_BILLING_SUSPEND_PENDING
            | SUB_BILLING_CONTROL_FAILED
    ) {
        return Ok(None);
    }
    conn.query_row(
        "SELECT operation.status, operation.attempts, operation.next_attempt_at,
                COALESCE(operation.last_error_code, edit.error_code), operation.dead_lettered_at,
                operation.last_error
         FROM share_control_operations operation
         LEFT JOIN share_edit_requests edit ON edit.id = operation.last_attempt_edit_id
         WHERE operation.subscription_id = ?1 AND operation.action = 'revoke'
         ORDER BY operation.share_sequence DESC LIMIT 1",
        params![record.id],
        |row| {
            let status: String = row.get(0)?;
            let attempts: i64 = row.get(1)?;
            let retry: Option<String> = row.get(2)?;
            let error_code: Option<String> = row.get(3)?;
            let dead: Option<String> = row.get(4)?;
            let error: Option<String> = row.get(5)?;
            let stage = if dead.is_some()
                || status == "rejected"
                || (status == "pending" && attempts >= MAX_CONTROL_ATTEMPTS)
            {
                "manual_intervention"
            } else if error.as_deref() == Some("final consumption evidence pending")
                || error_code.as_deref() == Some("market_termination_pending")
            {
                "awaiting_final_usage"
            } else if status == "applied" {
                "awaiting_settlement"
            } else if status == "pending" && retry.is_some() {
                "retry_scheduled"
            } else {
                "awaiting_revocation"
            };
            Ok(ControlProgress {
                stage,
                attempts: attempts.max(0).min(u32::MAX as i64) as u32,
                next_attempt_at: (stage == "retry_scheduled").then_some(retry).flatten(),
                error_code,
            })
        },
    )
    .optional()
    .map_err(map_db("read private Share revoke progress"))
}

pub(crate) fn reject_invalid_reply_tx(
    conn: &Connection,
    edit_id: &str,
    message: &str,
    now: &str,
) -> Result<(), AppError> {
    let operation: Option<(String, String, String)> = conn
        .query_row(
            "SELECT id, subscription_id, action FROM share_control_operations
         WHERE edit_id = ?1 AND status = 'dispatched'",
            params![edit_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(map_db("read invalid Share control reply"))?;
    let Some((id, subscription_id, action)) = operation else {
        return Ok(());
    };
    if action == "upsert" {
        if let Some(record) = subscription_record(conn, &subscription_id)? {
            return quarantine_invalid_effective_grant_tx(
                conn,
                &record,
                &id,
                Some(edit_id),
                message,
                now,
            );
        }
    }
    handle_control_edit_ack_with_metadata(
        conn,
        edit_id,
        "rejected",
        Some(message),
        Some("control_reply_mismatch"),
        Some(false),
        now,
    )
}

pub(super) enum RetryPatch {
    Fresh,
    Replay(ShareSettingsPatch),
    MissingEvidence,
}

pub(super) fn retry_patch_tx(
    conn: &Connection,
    operation_id: &str,
    fresh: &ShareSettingsPatch,
) -> Result<RetryPatch, AppError> {
    let (attempts, json, error_code): (i64, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT operation.attempts, edit.patch_json, operation.last_error_code
         FROM share_control_operations operation
         JOIN share_market_subscriptions subscription ON subscription.id = operation.subscription_id
         LEFT JOIN share_edit_requests edit ON edit.id = operation.last_attempt_edit_id
           AND edit.share_id = operation.share_id AND edit.installation_id = subscription.installation_id
         WHERE operation.id = ?1",
            params![operation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(map_db("read original Share control retry"))?;
    let Some(json) = json else {
        return Ok(if attempts == 0 {
            RetryPatch::Fresh
        } else {
            RetryPatch::MissingEvidence
        });
    };
    let Ok(saved) = serde_json::from_str::<ShareSettingsPatch>(&json) else {
        return Ok(RetryPatch::MissingEvidence);
    };
    let valid = saved
        .managed_grant
        .as_ref()
        .zip(fresh.managed_grant.as_ref())
        .is_some_and(|(saved, fresh)| {
            saved.operation_id == operation_id
                && saved.operation_id == fresh.operation_id
                && saved.entitlement_id == fresh.entitlement_id
                && saved.share_sequence == fresh.share_sequence
                && saved.action == fresh.action
                && saved.email.eq_ignore_ascii_case(&fresh.email)
        });
    if !valid {
        return Ok(RetryPatch::MissingEvidence);
    }
    if error_code.as_deref() == Some(SHARE_REVISION_CONFLICT_CODE) {
        // Server checks its durable operation receipt before config revision.
        // A conflict therefore proves this operation did not apply there.
        return Ok(RetryPatch::Fresh);
    }
    Ok(RetryPatch::Replay(saved))
}

pub(super) fn block_unreplayable_tx(
    conn: &Connection,
    operation_id: &str,
    subscription_id: &str,
    action: &str,
    now: &str,
) -> Result<(), AppError> {
    const REASON: &str = "control_retry_evidence_missing";
    let record = subscription_record(conn, subscription_id)?
        .ok_or_else(|| AppError::Internal("unreplayable Share subscription is missing".into()))?;
    if action == "upsert" {
        quarantine_invalid_effective_grant_tx(
            conn,
            &record,
            operation_id,
            None,
            "original dispatched request unavailable; access must be reclaimed before retry",
            now,
        )?;
    } else {
        conn.execute("UPDATE share_control_operations SET status='rejected',dead_lettered_at=?2,
            last_error_code=?3,last_error='Original request evidence unavailable; operator review required',
            next_attempt_at=NULL,updated_at=?2 WHERE id=?1 AND status='pending'", params![operation_id,now,REASON])
            .map_err(map_db("isolate unreplayable Share revoke"))?;
        conn.execute(
            "UPDATE share_market_subscriptions SET status=CASE
            WHEN status='revoke_pending' THEN 'revoke_failed'
            WHEN status='billing_suspend_pending' THEN 'billing_control_failed' ELSE status END,
            updated_at=?2 WHERE id=?1",
            params![subscription_id, now],
        )
        .map_err(map_db("expose unreplayable Share cleanup"))?;
        enqueue_subscription_lifecycle_event_tx(
            conn,
            subscription_id,
            "revoke_failed",
            serde_json::json!({"failureCode":REASON}),
            &format!("share-control:{operation_id}:missing-evidence"),
            now,
        )?;
    }
    tracing::warn!(
        operation_id,
        subscription_id,
        action,
        reason = REASON,
        "Share control retry blocked because original request evidence is unavailable"
    );
    Ok(())
}

pub(crate) fn wake_recovered_revocations_tx(
    conn: &Connection,
    installation_id: &str,
    share_id: &str,
    revision: u64,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let Ok(revision) = i64::try_from(revision) else {
        return Ok(());
    };
    let candidates = {
        let mut stmt = conn.prepare(
            "SELECT operation.id, operation.attempts, edit.created_at,
                    operation.next_attempt_at, operation.last_attempt_config_revision
             FROM share_control_operations operation
             JOIN share_market_subscriptions subscription ON subscription.id = operation.subscription_id
             JOIN share_edit_requests edit ON edit.id = operation.last_attempt_edit_id
             WHERE operation.share_id = ?1 AND subscription.installation_id = ?2
               AND edit.share_id = operation.share_id AND edit.installation_id = ?2
               AND operation.action = 'revoke' AND operation.status = 'pending'
               AND subscription.status IN ('revoke_pending', 'billing_suspend_pending')
               AND operation.dead_lettered_at IS NULL AND operation.attempts < ?3
               AND operation.last_error_code IN ('control_ack_timeout', 'cc_switch_share_revision_conflict')
               AND operation.last_attempt_config_revision < ?4
               AND COALESCE(operation.last_recovery_revision, -1) < ?4
               AND operation.next_attempt_at > ?5",
        ).map_err(map_db("prepare recovered Share revocations"))?;
        stmt.query_map(
            params![
                share_id,
                installation_id,
                MAX_CONTROL_ATTEMPTS,
                revision,
                now.to_rfc3339()
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .map_err(map_db("query recovered Share revocations"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_db("read recovered Share revocations"))?
    };
    for (operation_id, attempts, dispatched_at, previous_retry, attempted_revision) in candidates {
        // Recovery is an optional acceleration, not another reason to reject
        // an otherwise valid Server sync. Keep corrupt historical diagnostics
        // isolated and leave the conservative retry schedule unchanged.
        let times = parse_time(&dispatched_at)
            .ok()
            .and_then(|at| at.checked_add_signed(Duration::seconds(CONTROL_RETRY_BASE_SECS)))
            .zip(parse_time(&previous_retry).ok());
        let Some((earliest, previous_retry_at)) = times else {
            tracing::warn!(
                operation_id,
                installation_id,
                share_id,
                reason = "invalid_retry_timestamp",
                "Skipping Share revoke acceleration with invalid historical retry times"
            );
            continue;
        };
        let retry_at = now.max(earliest);
        if retry_at >= previous_retry_at {
            continue;
        }
        let changed = conn
            .execute(
                "UPDATE share_control_operations
             SET next_attempt_at = ?2, last_recovery_revision = ?3
             WHERE id = ?1 AND status = 'pending' AND dead_lettered_at IS NULL
               AND attempts = ?4 AND next_attempt_at = ?5",
                params![
                    operation_id,
                    retry_at.to_rfc3339(),
                    revision,
                    attempts,
                    previous_retry
                ],
            )
            .map_err(map_db("wake recovered Share revoke"))?;
        if changed == 1 {
            tracing::info!(operation_id, installation_id, share_id, attempts,
                attempted_revision, recovered_revision = revision, next_attempt_at = %retry_at,
                reason = "verified_config_progress", "Share revoke retry accelerated after validated sync");
        }
    }
    Ok(())
}
