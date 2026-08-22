use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use tokio::task::spawn_blocking;
use uuid::Uuid;

use crate::error::AppError;

use super::models::{
    AlertCondition, AlertDelivery, AlertDeliveryClaim, AlertDeliveryPolicy, AlertIncident,
    AlertOverviewCounts, AlertTransition, OperatorAlertSignal, severity_rank,
};

#[derive(Debug, Clone, Default)]
pub struct AlertChannelActivity {
    pub last_attempt_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub last_error: Option<String>,
    pub failure_code: Option<String>,
    pub failure_hint: Option<String>,
    pub failure_details: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct AlertStore {
    path: PathBuf,
    initialized: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
struct StoredIncident {
    id: String,
    fingerprint: String,
    scope: String,
    kind: String,
    entity_kind: String,
    entity_id: Option<String>,
    severity: String,
    status: String,
    title: String,
    message: String,
    details: serde_json::Value,
    occurrence_count: u64,
    started_at: i64,
    last_seen_at: i64,
    last_transition_at: i64,
    resolved_at: Option<i64>,
    acknowledged_at: Option<i64>,
    acknowledged_by: Option<String>,
    acknowledgement_note: Option<String>,
    silenced_at: Option<i64>,
    silenced_until: Option<i64>,
    silenced_by: Option<String>,
    silence_note: Option<String>,
}

impl From<StoredIncident> for AlertIncident {
    fn from(value: StoredIncident) -> Self {
        Self {
            id: value.id,
            fingerprint: value.fingerprint,
            scope: value.scope,
            kind: value.kind,
            entity_kind: value.entity_kind,
            entity_id: value.entity_id,
            severity: value.severity,
            status: value.status,
            title: value.title,
            message: value.message,
            details: value.details,
            occurrence_count: value.occurrence_count,
            started_at: value.started_at,
            last_seen_at: value.last_seen_at,
            last_transition_at: value.last_transition_at,
            resolved_at: value.resolved_at,
            acknowledged_at: value.acknowledged_at,
            acknowledged_by: value.acknowledged_by,
            acknowledgement_note: value.acknowledgement_note,
            silenced_at: value.silenced_at,
            silenced_until: value.silenced_until,
            silenced_by: value.silenced_by,
            silence_note: value.silence_note,
        }
    }
}

impl AlertStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            initialized: Arc::new(AtomicBool::new(false)),
        }
    }

    fn open(&self) -> Result<Connection, AppError> {
        crate::db::initialize_sqlite_runtime().map_err(|error| {
            AppError::Internal(format!("initialize alert database runtime failed: {error}"))
        })?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                AppError::Internal(format!("create alert database directory failed: {error}"))
            })?;
        }
        let conn = Connection::open(&self.path)
            .map_err(|error| AppError::Internal(format!("open alert database failed: {error}")))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| {
                AppError::Internal(format!("configure alert database failed: {error}"))
            })?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|error| {
                AppError::Internal(format!(
                    "enable alert database foreign keys failed: {error}"
                ))
            })?;
        if !self.initialized.load(Ordering::Acquire) {
            init_alert_db(&conn)?;
            self.initialized.store(true, Ordering::Release);
        }
        Ok(conn)
    }

    pub async fn init(&self) -> Result<(), AppError> {
        let store = self.clone();
        spawn_blocking(move || store.open().map(|_| ()))
            .await
            .map_err(|error| AppError::Internal(format!("alert init task failed: {error}")))?
    }

    pub async fn reconcile_conditions(
        &self,
        scope: String,
        conditions: Vec<AlertCondition>,
        now: i64,
        repeat_interval_secs: i64,
        delivery_policy: AlertDeliveryPolicy,
    ) -> Result<Vec<AlertTransition>, AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let mut conn = store.open()?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| {
                    AppError::Internal(format!("begin alert reconcile failed: {error}"))
                })?;
            let mut transitions = Vec::new();
            let mut observed = HashSet::new();
            for condition in conditions {
                if condition.scope != scope {
                    return Err(AppError::Internal(format!(
                        "alert scope mismatch: expected {scope}, got {}",
                        condition.scope
                    )));
                }
                observed.insert(condition.fingerprint.clone());
                if let Some(transition) = observe_condition_tx(
                    &tx,
                    &condition,
                    now,
                    repeat_interval_secs,
                    &delivery_policy,
                )? {
                    transitions.push(transition);
                }
            }

            let unresolved = load_active_incidents_for_scope_tx(&tx, &scope)?;
            for incident in unresolved {
                if observed.contains(&incident.fingerprint) {
                    continue;
                }
                if let Some(transition) = resolve_incident_tx(
                    &tx,
                    &incident,
                    None,
                    "Condition returned to normal",
                    serde_json::json!({ "reason": "condition_cleared" }),
                    now,
                    &delivery_policy,
                )? {
                    transitions.push(transition);
                }
            }
            tx.commit().map_err(|error| {
                AppError::Internal(format!("commit alert reconcile failed: {error}"))
            })?;
            Ok(transitions)
        })
        .await
        .map_err(|error| AppError::Internal(format!("alert reconcile task failed: {error}")))?
    }

    pub async fn ingest_signal(
        &self,
        signal: OperatorAlertSignal,
        delivery_policy: AlertDeliveryPolicy,
    ) -> Result<Option<AlertTransition>, AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let mut conn = store.open()?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| {
                    AppError::Internal(format!("begin alert signal ingest failed: {error}"))
                })?;
            let inserted = tx
                .execute(
                    "INSERT OR IGNORE INTO alert_source_events (
                        source_event_id, source_kind, received_at
                     ) VALUES (?1, ?2, ?3)",
                    params![signal.source_event_id, signal.kind, signal.occurred_at],
                )
                .map_err(|error| {
                    AppError::Internal(format!("dedupe alert source event failed: {error}"))
                })?;
            if inserted == 0 {
                tx.commit().map_err(|error| {
                    AppError::Internal(format!("commit duplicate alert signal failed: {error}"))
                })?;
                return Ok(None);
            }

            let transition = match signal.transition.as_str() {
                "firing" => observe_signal_tx(&tx, &signal, &delivery_policy)?,
                "resolved" => {
                    let incident =
                        load_active_incident_by_fingerprint_tx(&tx, &signal.fingerprint)?;
                    match incident {
                        Some(incident) => resolve_incident_tx(
                            &tx,
                            &incident,
                            Some(&signal.source_event_id),
                            &signal.message,
                            signal.details.clone(),
                            signal.occurred_at,
                            &delivery_policy,
                        )?,
                        None => None,
                    }
                }
                other => {
                    return Err(AppError::Internal(format!(
                        "unsupported alert signal transition: {other}"
                    )));
                }
            };
            tx.execute(
                "UPDATE alert_source_events
                 SET incident_id = ?2, transition_id = ?3, processed_at = ?4
                 WHERE source_event_id = ?1",
                params![
                    signal.source_event_id,
                    transition.as_ref().map(|value| value.incident_id.as_str()),
                    transition.as_ref().map(|value| value.id.as_str()),
                    signal.occurred_at,
                ],
            )
            .map_err(|error| {
                AppError::Internal(format!("complete alert source event failed: {error}"))
            })?;
            tx.commit().map_err(|error| {
                AppError::Internal(format!("commit alert signal ingest failed: {error}"))
            })?;
            Ok(transition)
        })
        .await
        .map_err(|error| AppError::Internal(format!("alert signal task failed: {error}")))?
    }

    pub async fn expire_silences(
        &self,
        now: i64,
        delivery_policy: AlertDeliveryPolicy,
    ) -> Result<Vec<AlertTransition>, AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let mut conn = store.open()?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| {
                    AppError::Internal(format!("begin silence expiry failed: {error}"))
                })?;
            let incidents = load_expired_silences_tx(&tx, now)?;
            let mut transitions = Vec::new();
            for incident in incidents {
                tx.execute(
                    "UPDATE alert_incidents
                     SET status = 'firing', silenced_at = NULL, silenced_until = NULL,
                         silenced_by = NULL, silence_note = NULL, last_transition_at = ?2
                     WHERE id = ?1 AND resolved_at IS NULL",
                    params![incident.id, now],
                )
                .map_err(|error| {
                    AppError::Internal(format!("expire incident silence failed: {error}"))
                })?;
                transitions.push(insert_transition_tx(
                    &tx,
                    &incident,
                    None,
                    "unsilenced",
                    "Silence expired while the condition is still firing",
                    incident.details.clone(),
                    None,
                    None,
                    now,
                    true,
                    &delivery_policy,
                )?);
            }
            tx.commit().map_err(|error| {
                AppError::Internal(format!("commit silence expiry failed: {error}"))
            })?;
            Ok(transitions)
        })
        .await
        .map_err(|error| AppError::Internal(format!("silence expiry task failed: {error}")))?
    }

    pub async fn list_incidents(
        &self,
        limit: usize,
        active_only: bool,
    ) -> Result<Vec<AlertIncident>, AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            load_incidents(&conn, limit.clamp(1, 500), active_only)
        })
        .await
        .map_err(|error| AppError::Internal(format!("list alert incidents task failed: {error}")))?
    }

    pub async fn list_deliveries(&self, limit: usize) -> Result<Vec<AlertDelivery>, AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            load_deliveries(&conn, limit.clamp(1, 500))
        })
        .await
        .map_err(|error| {
            AppError::Internal(format!("list alert deliveries task failed: {error}"))
        })?
    }

    pub async fn overview_counts(&self) -> Result<AlertOverviewCounts, AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            let (active, critical, resolved) = conn
                .query_row(
                    "SELECT
                        COALESCE(SUM(CASE WHEN resolved_at IS NULL THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN resolved_at IS NULL AND severity = 'critical' THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN resolved_at IS NOT NULL THEN 1 ELSE 0 END), 0)
                     FROM alert_incidents",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?.max(0) as u64,
                            row.get::<_, i64>(1)?.max(0) as u64,
                            row.get::<_, i64>(2)?.max(0) as u64,
                        ))
                    },
                )
                .map_err(|error| {
                    AppError::Internal(format!("count alert incidents failed: {error}"))
                })?;
            let failed_deliveries = conn
                .query_row(
                    "SELECT COUNT(*) FROM alert_deliveries
                     WHERE status IN ('retry', 'dead_letter', 'suppressed_disabled')",
                    [],
                    |row| Ok(row.get::<_, i64>(0)?.max(0) as u64),
                )
                .map_err(|error| {
                    AppError::Internal(format!("count failed alert deliveries failed: {error}"))
                })?;
            Ok(AlertOverviewCounts {
                active,
                critical,
                resolved,
                failed_deliveries,
            })
        })
        .await
        .map_err(|error| AppError::Internal(format!("alert count task failed: {error}")))?
    }

    pub async fn acknowledge(
        &self,
        incident_id: String,
        actor_email: String,
        note: Option<String>,
        now: i64,
    ) -> Result<AlertIncident, AppError> {
        self.change_incident_state(incident_id, actor_email, note, now, "acknowledged", None)
            .await
    }

    pub async fn silence(
        &self,
        incident_id: String,
        actor_email: String,
        note: Option<String>,
        now: i64,
        duration_secs: i64,
    ) -> Result<AlertIncident, AppError> {
        if !(60..=30 * 86_400).contains(&duration_secs) {
            return Err(AppError::BadRequest(
                "silence duration must be between 60 seconds and 30 days".into(),
            ));
        }
        self.change_incident_state(
            incident_id,
            actor_email,
            note,
            now,
            "silenced",
            Some(now.saturating_add(duration_secs)),
        )
        .await
    }

    pub async fn resume(
        &self,
        incident_id: String,
        actor_email: String,
        note: Option<String>,
        now: i64,
        delivery_policy: AlertDeliveryPolicy,
    ) -> Result<AlertIncident, AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let mut conn = store.open()?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| {
                    AppError::Internal(format!("begin alert resume failed: {error}"))
                })?;
            let incident = load_incident_by_id_tx(&tx, &incident_id)?
                .ok_or_else(|| AppError::NotFound("alert incident not found".into()))?;
            if incident.resolved_at.is_some() {
                return Err(AppError::Conflict(
                    "resolved incident cannot be resumed".into(),
                ));
            }
            tx.execute(
                "UPDATE alert_incidents
                 SET status = 'firing', acknowledged_at = NULL, acknowledged_by = NULL,
                     acknowledgement_note = NULL, silenced_at = NULL, silenced_until = NULL,
                     silenced_by = NULL, silence_note = NULL, last_transition_at = ?2
                 WHERE id = ?1",
                params![incident_id, now],
            )
            .map_err(|error| {
                AppError::Internal(format!("resume alert incident failed: {error}"))
            })?;
            insert_transition_tx(
                &tx,
                &incident,
                None,
                "resumed",
                "Operator resumed notifications for this incident",
                incident.details.clone(),
                Some(&actor_email),
                note.as_deref(),
                now,
                true,
                &delivery_policy,
            )?;
            let updated = load_incident_by_id_tx(&tx, &incident_id)?
                .ok_or_else(|| AppError::Internal("resumed incident disappeared".into()))?;
            tx.commit().map_err(|error| {
                AppError::Internal(format!("commit alert resume failed: {error}"))
            })?;
            Ok(updated.into())
        })
        .await
        .map_err(|error| AppError::Internal(format!("alert resume task failed: {error}")))?
    }

    async fn change_incident_state(
        &self,
        incident_id: String,
        actor_email: String,
        note: Option<String>,
        now: i64,
        next_status: &'static str,
        silenced_until: Option<i64>,
    ) -> Result<AlertIncident, AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let mut conn = store.open()?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| {
                    AppError::Internal(format!("begin alert state update failed: {error}"))
                })?;
            let incident = load_incident_by_id_tx(&tx, &incident_id)?
                .ok_or_else(|| AppError::NotFound("alert incident not found".into()))?;
            if incident.resolved_at.is_some() {
                return Err(AppError::Conflict(
                    "resolved incident cannot be acknowledged or silenced".into(),
                ));
            }
            let normalized_note = note
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.chars().take(500).collect::<String>());
            match next_status {
                "acknowledged" => {
                    tx.execute(
                        "UPDATE alert_incidents
                         SET status = 'acknowledged', acknowledged_at = ?2,
                             acknowledged_by = ?3, acknowledgement_note = ?4,
                             silenced_at = NULL, silenced_until = NULL,
                             silenced_by = NULL, silence_note = NULL,
                             last_transition_at = ?2
                         WHERE id = ?1",
                        params![incident_id, now, actor_email, normalized_note],
                    )
                    .map_err(|error| {
                        AppError::Internal(format!("acknowledge alert incident failed: {error}"))
                    })?;
                }
                "silenced" => {
                    tx.execute(
                        "UPDATE alert_incidents
                         SET status = 'silenced', silenced_at = ?2, silenced_until = ?3,
                             silenced_by = ?4, silence_note = ?5,
                             acknowledged_at = NULL, acknowledged_by = NULL,
                             acknowledgement_note = NULL, last_transition_at = ?2
                         WHERE id = ?1",
                        params![
                            incident_id,
                            now,
                            silenced_until,
                            actor_email,
                            normalized_note
                        ],
                    )
                    .map_err(|error| {
                        AppError::Internal(format!("silence alert incident failed: {error}"))
                    })?;
                }
                _ => return Err(AppError::Internal("invalid alert state update".into())),
            }
            insert_transition_tx(
                &tx,
                &incident,
                None,
                next_status,
                if next_status == "silenced" {
                    "Operator silenced this incident"
                } else {
                    "Operator acknowledged this incident"
                },
                serde_json::json!({ "silencedUntil": silenced_until }),
                Some(&actor_email),
                normalized_note.as_deref(),
                now,
                false,
                &AlertDeliveryPolicy {
                    enabled: false,
                    dashboard_url: String::new(),
                    channels: Vec::new(),
                },
            )?;
            let updated = load_incident_by_id_tx(&tx, &incident_id)?
                .ok_or_else(|| AppError::Internal("updated incident disappeared".into()))?;
            tx.commit().map_err(|error| {
                AppError::Internal(format!("commit alert state update failed: {error}"))
            })?;
            Ok(updated.into())
        })
        .await
        .map_err(|error| AppError::Internal(format!("alert state task failed: {error}")))?
    }

    pub async fn claim_delivery(
        &self,
        worker_id: String,
        now: i64,
        lease_secs: i64,
    ) -> Result<Option<AlertDeliveryClaim>, AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let mut conn = store.open()?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| {
                    AppError::Internal(format!("begin alert delivery claim failed: {error}"))
                })?;
            let delivery = tx
                .query_row(
                    "SELECT id, incident_id, transition_id, channel, payload_text, attempts
                     FROM alert_deliveries
                     WHERE (
                         status IN ('pending', 'retry')
                         AND COALESCE(next_attempt_at, 0) <= ?1
                     ) OR (
                         status = 'claimed' AND COALESCE(claim_expires_at, 0) <= ?1
                     )
                     ORDER BY created_at, id
                     LIMIT 1",
                    params![now],
                    |row| {
                        Ok(AlertDeliveryClaim {
                            id: row.get(0)?,
                            incident_id: row.get(1)?,
                            transition_id: row.get(2)?,
                            channel: row.get(3)?,
                            payload_text: row.get(4)?,
                            attempts: row.get::<_, i64>(5)?.max(0) as u32 + 1,
                        })
                    },
                )
                .optional()
                .map_err(|error| {
                    AppError::Internal(format!("select alert delivery failed: {error}"))
                })?;
            let Some(delivery) = delivery else {
                tx.commit().map_err(|error| {
                    AppError::Internal(format!("commit empty alert claim failed: {error}"))
                })?;
                return Ok(None);
            };
            let claimed = tx
                .execute(
                    "UPDATE alert_deliveries
                     SET status = 'claimed', attempts = ?2, claimed_by = ?3,
                         claim_expires_at = ?4, updated_at = ?5
                     WHERE id = ?1 AND (
                         (status IN ('pending', 'retry') AND COALESCE(next_attempt_at, 0) <= ?5)
                         OR (status = 'claimed' AND COALESCE(claim_expires_at, 0) <= ?5)
                     )",
                    params![
                        delivery.id,
                        delivery.attempts,
                        worker_id,
                        now.saturating_add(lease_secs.max(1)),
                        now
                    ],
                )
                .map_err(|error| {
                    AppError::Internal(format!("claim alert delivery failed: {error}"))
                })?;
            tx.commit().map_err(|error| {
                AppError::Internal(format!("commit alert delivery claim failed: {error}"))
            })?;
            Ok((claimed == 1).then_some(delivery))
        })
        .await
        .map_err(|error| AppError::Internal(format!("alert delivery claim task failed: {error}")))?
    }

    pub async fn finish_delivery(
        &self,
        delivery: AlertDeliveryClaim,
        result: AlertDeliveryResult,
        now: i64,
    ) -> Result<(), AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let mut conn = store.open()?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| {
                    AppError::Internal(format!("begin alert delivery finish failed: {error}"))
                })?;
            let (
                status,
                provider_message_id,
                next_attempt_at,
                last_error,
                failure_code,
                failure_hint,
                failure_details,
                http_status,
            ) = match &result {
                AlertDeliveryResult::Sent {
                    provider_message_id,
                    http_status,
                } => (
                    "sent",
                    provider_message_id.clone(),
                    None,
                    None,
                    None,
                    None,
                    None,
                    *http_status,
                ),
                AlertDeliveryResult::Retry {
                    error,
                    failure_code,
                    failure_hint,
                    failure_details,
                    next_attempt_at,
                    http_status,
                } => (
                    "retry",
                    None,
                    Some(*next_attempt_at),
                    Some(error.clone()),
                    failure_code.clone(),
                    failure_hint.clone(),
                    failure_details.clone(),
                    *http_status,
                ),
                AlertDeliveryResult::DeadLetter {
                    error,
                    failure_code,
                    failure_hint,
                    failure_details,
                    http_status,
                } => (
                    "dead_letter",
                    None,
                    None,
                    Some(error.clone()),
                    failure_code.clone(),
                    failure_hint.clone(),
                    failure_details.clone(),
                    *http_status,
                ),
                AlertDeliveryResult::Suppressed { reason } => (
                    "suppressed_disabled",
                    None,
                    None,
                    Some(reason.clone()),
                    None,
                    None,
                    None,
                    None,
                ),
            };
            let updated = tx
                .execute(
                    "UPDATE alert_deliveries
                 SET status = ?2, provider_message_id = ?3, next_attempt_at = ?4,
                     last_error = ?5, claimed_by = NULL, claim_expires_at = NULL,
                     sent_at = CASE WHEN ?2 = 'sent' THEN ?6 ELSE sent_at END,
                     updated_at = ?6
                 WHERE id = ?1 AND status = 'claimed' AND attempts = ?7",
                    params![
                        delivery.id,
                        status,
                        provider_message_id,
                        next_attempt_at,
                        last_error,
                        now,
                        delivery.attempts,
                    ],
                )
                .map_err(|error| {
                    AppError::Internal(format!("finish alert delivery failed: {error}"))
                })?;
            if updated == 0 {
                tx.commit().map_err(|error| {
                    AppError::Internal(format!("commit stale alert finish failed: {error}"))
                })?;
                return Ok(());
            }
            tx.execute(
                "INSERT INTO alert_delivery_attempts (
                    id, delivery_id, attempt_number, status, http_status,
                    provider_message_id, error_message, failure_code, failure_hint,
                    failure_details_json, attempted_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    Uuid::new_v4().to_string(),
                    delivery.id,
                    delivery.attempts,
                    status,
                    http_status,
                    provider_message_id,
                    last_error,
                    failure_code,
                    failure_hint,
                    failure_details.and_then(|value| serde_json::to_string(&value).ok()),
                    now
                ],
            )
            .map_err(|error| {
                AppError::Internal(format!("record alert delivery attempt failed: {error}"))
            })?;
            tx.commit().map_err(|error| {
                AppError::Internal(format!("commit alert delivery finish failed: {error}"))
            })?;
            Ok(())
        })
        .await
        .map_err(|error| {
            AppError::Internal(format!("alert delivery finish task failed: {error}"))
        })?
    }

    pub async fn retry_delivery(&self, delivery_id: String, now: i64) -> Result<(), AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            let updated = conn
                .execute(
                    "UPDATE alert_deliveries
                     SET status = 'pending', next_attempt_at = ?2, last_error = NULL,
                         claimed_by = NULL, claim_expires_at = NULL, updated_at = ?2
                     WHERE id = ?1 AND status IN (
                         'retry', 'dead_letter', 'suppressed_disabled'
                     )",
                    params![delivery_id, now],
                )
                .map_err(|error| {
                    AppError::Internal(format!("retry alert delivery failed: {error}"))
                })?;
            if updated == 0 {
                return Err(AppError::Conflict(
                    "only failed or suppressed alert deliveries can be retried".into(),
                ));
            }
            Ok(())
        })
        .await
        .map_err(|error| AppError::Internal(format!("alert retry task failed: {error}")))?
    }

    pub async fn suppress_disabled_deliveries(
        &self,
        enabled_channels: HashSet<String>,
        now: i64,
    ) -> Result<u64, AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            let mut statement = conn
                .prepare(
                    "SELECT id, channel FROM alert_deliveries
                     WHERE status IN ('pending', 'retry')",
                )
                .map_err(|error| {
                    AppError::Internal(format!("prepare disabled alert deliveries failed: {error}"))
                })?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|error| {
                    AppError::Internal(format!("query disabled alert deliveries failed: {error}"))
                })?;
            let mut ids = Vec::new();
            for row in rows {
                let (id, channel) = row.map_err(|error| {
                    AppError::Internal(format!("read disabled alert delivery failed: {error}"))
                })?;
                if !enabled_channels.contains(&channel) {
                    ids.push(id);
                }
            }
            drop(statement);
            let mut updated = 0_u64;
            for id in ids {
                updated += conn
                    .execute(
                        "UPDATE alert_deliveries
                         SET status = 'suppressed_disabled',
                             last_error = 'channel disabled before delivery', updated_at = ?2
                         WHERE id = ?1 AND status IN ('pending', 'retry')",
                        params![id, now],
                    )
                    .map_err(|error| {
                        AppError::Internal(format!("suppress alert delivery failed: {error}"))
                    })? as u64;
            }
            Ok(updated)
        })
        .await
        .map_err(|error| AppError::Internal(format!("suppress alert task failed: {error}")))?
    }

    pub async fn channel_activity(
        &self,
    ) -> Result<HashMap<String, AlertChannelActivity>, AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            let mut statement = conn
                .prepare(
                    "SELECT channel, attempted_at, success_at, error_message,
                            failure_code, failure_hint, failure_details_json
                     FROM (
                         SELECT d.channel AS channel, a.attempted_at AS attempted_at,
                                CASE WHEN a.status = 'sent' THEN a.attempted_at END AS success_at,
                                CASE WHEN a.status = 'sent' THEN NULL ELSE a.error_message END AS error_message,
                                CASE WHEN a.status = 'sent' THEN NULL ELSE a.failure_code END AS failure_code,
                                CASE WHEN a.status = 'sent' THEN NULL ELSE a.failure_hint END AS failure_hint,
                                CASE WHEN a.status = 'sent' THEN NULL ELSE a.failure_details_json END AS failure_details_json,
                                a.id AS activity_id
                         FROM alert_delivery_attempts a
                         INNER JOIN alert_deliveries d ON d.id = a.delivery_id
                         UNION ALL
                         SELECT channel, tested_at,
                                CASE WHEN status = 'success' THEN tested_at END,
                                CASE WHEN status = 'success' THEN NULL ELSE error_message END,
                                CASE WHEN status = 'success' THEN NULL ELSE failure_code END,
                                CASE WHEN status = 'success' THEN NULL ELSE failure_hint END,
                                CASE WHEN status = 'success' THEN NULL ELSE failure_details_json END,
                                id
                         FROM alert_channel_checks
                     )
                     ORDER BY attempted_at, activity_id",
                )
                .map_err(|error| {
                    AppError::Internal(format!("prepare alert channel activity failed: {error}"))
                })?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                })
                .map_err(|error| {
                    AppError::Internal(format!("query alert channel activity failed: {error}"))
                })?;
            let mut activity: HashMap<String, AlertChannelActivity> = HashMap::new();
            for row in rows {
                let (
                    channel,
                    attempted_at,
                    success_at,
                    error_message,
                    failure_code,
                    failure_hint,
                    failure_details_json,
                ) = row.map_err(|error| {
                    AppError::Internal(format!("read alert channel activity failed: {error}"))
                })?;
                let entry = activity.entry(channel).or_default();
                entry.last_attempt_at = Some(attempted_at);
                if let Some(success_at) = success_at {
                    entry.last_success_at = Some(
                        entry
                            .last_success_at
                            .map_or(success_at, |current| current.max(success_at)),
                    );
                }
                entry.last_error = error_message;
                entry.failure_code = failure_code;
                entry.failure_hint = failure_hint;
                entry.failure_details = failure_details_json
                    .as_deref()
                    .and_then(|value| serde_json::from_str(value).ok());
            }
            Ok(activity)
        })
        .await
        .map_err(|error| AppError::Internal(format!("alert channel task failed: {error}")))?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn record_channel_test(
        &self,
        channel: String,
        success: bool,
        provider_message_id: Option<String>,
        http_status: Option<u16>,
        error_message: Option<String>,
        failure_code: Option<String>,
        failure_hint: Option<String>,
        failure_details: Option<serde_json::Value>,
        tested_at: i64,
    ) -> Result<(), AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            conn.execute(
                "INSERT INTO alert_channel_checks (
                    id, channel, status, http_status, provider_message_id,
                    error_message, failure_code, failure_hint, failure_details_json,
                    tested_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    Uuid::new_v4().to_string(),
                    channel,
                    if success { "success" } else { "failed" },
                    http_status.map(i64::from),
                    provider_message_id,
                    error_message,
                    failure_code,
                    failure_hint,
                    failure_details.and_then(|value| serde_json::to_string(&value).ok()),
                    tested_at,
                ],
            )
            .map_err(|error| {
                AppError::Internal(format!("record alert channel test failed: {error}"))
            })?;
            Ok(())
        })
        .await
        .map_err(|error| AppError::Internal(format!("alert channel test task failed: {error}")))?
    }

    pub async fn prune(&self, now: i64, retention_days: u32) -> Result<(), AppError> {
        let store = self.clone();
        spawn_blocking(move || {
            let conn = store.open()?;
            let cutoff = now.saturating_sub(i64::from(retention_days.max(1)) * 86_400);
            conn.execute(
                "DELETE FROM alert_incidents
                 WHERE resolved_at IS NOT NULL AND resolved_at < ?1",
                params![cutoff],
            )
            .map_err(|error| {
                AppError::Internal(format!("prune resolved alert incidents failed: {error}"))
            })?;
            conn.execute(
                "DELETE FROM alert_source_events
                 WHERE processed_at IS NOT NULL AND processed_at < ?1",
                params![cutoff],
            )
            .map_err(|error| {
                AppError::Internal(format!("prune alert source events failed: {error}"))
            })?;
            conn.execute(
                "DELETE FROM alert_channel_checks WHERE tested_at < ?1",
                params![cutoff],
            )
            .map_err(|error| {
                AppError::Internal(format!("prune alert channel checks failed: {error}"))
            })?;
            Ok(())
        })
        .await
        .map_err(|error| AppError::Internal(format!("alert prune task failed: {error}")))?
    }
}

#[derive(Debug, Clone)]
pub enum AlertDeliveryResult {
    Sent {
        provider_message_id: Option<String>,
        http_status: Option<u16>,
    },
    Retry {
        error: String,
        failure_code: Option<String>,
        failure_hint: Option<String>,
        failure_details: Option<serde_json::Value>,
        next_attempt_at: i64,
        http_status: Option<u16>,
    },
    DeadLetter {
        error: String,
        failure_code: Option<String>,
        failure_hint: Option<String>,
        failure_details: Option<serde_json::Value>,
        http_status: Option<u16>,
    },
    Suppressed {
        reason: String,
    },
}

fn init_alert_db(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS alert_incidents (
            id TEXT PRIMARY KEY,
            fingerprint TEXT NOT NULL,
            scope TEXT NOT NULL,
            kind TEXT NOT NULL,
            entity_kind TEXT NOT NULL,
            entity_id TEXT,
            severity TEXT NOT NULL CHECK (severity IN ('info', 'warning', 'critical')),
            status TEXT NOT NULL CHECK (status IN ('firing', 'acknowledged', 'silenced', 'resolved')),
            title TEXT NOT NULL,
            message TEXT NOT NULL,
            details_json TEXT NOT NULL DEFAULT '{}',
            occurrence_count INTEGER NOT NULL DEFAULT 1,
            started_at INTEGER NOT NULL,
            last_seen_at INTEGER NOT NULL,
            last_transition_at INTEGER NOT NULL,
            resolved_at INTEGER,
            acknowledged_at INTEGER,
            acknowledged_by TEXT,
            acknowledgement_note TEXT,
            silenced_at INTEGER,
            silenced_until INTEGER,
            silenced_by TEXT,
            silence_note TEXT
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_alert_incidents_active_fingerprint
            ON alert_incidents(fingerprint) WHERE resolved_at IS NULL;
        CREATE INDEX IF NOT EXISTS idx_alert_incidents_status_seen
            ON alert_incidents(status, last_seen_at DESC);

        CREATE TABLE IF NOT EXISTS alert_transitions (
            id TEXT PRIMARY KEY,
            incident_id TEXT NOT NULL,
            source_event_id TEXT UNIQUE,
            transition TEXT NOT NULL,
            severity TEXT NOT NULL,
            message TEXT NOT NULL,
            details_json TEXT NOT NULL DEFAULT '{}',
            actor_email TEXT,
            note TEXT,
            occurred_at INTEGER NOT NULL,
            FOREIGN KEY (incident_id) REFERENCES alert_incidents(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_alert_transitions_incident_time
            ON alert_transitions(incident_id, occurred_at DESC);

        CREATE TABLE IF NOT EXISTS alert_deliveries (
            id TEXT PRIMARY KEY,
            incident_id TEXT NOT NULL,
            transition_id TEXT NOT NULL,
            channel TEXT NOT NULL CHECK (
                length(channel) BETWEEN 1 AND 64
                AND channel = lower(channel)
                AND channel NOT GLOB '*[^a-z0-9_-]*'
            ),
            status TEXT NOT NULL CHECK (status IN (
                'pending', 'claimed', 'retry', 'sent', 'dead_letter',
                'suppressed_disabled', 'superseded'
            )),
            payload_text TEXT NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0,
            provider_message_id TEXT,
            next_attempt_at INTEGER,
            claimed_by TEXT,
            claim_expires_at INTEGER,
            last_error TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            sent_at INTEGER,
            UNIQUE (transition_id, channel),
            FOREIGN KEY (incident_id) REFERENCES alert_incidents(id) ON DELETE CASCADE,
            FOREIGN KEY (transition_id) REFERENCES alert_transitions(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_alert_deliveries_pending
            ON alert_deliveries(status, next_attempt_at, created_at);

        CREATE TABLE IF NOT EXISTS alert_delivery_attempts (
            id TEXT PRIMARY KEY,
            delivery_id TEXT NOT NULL,
            attempt_number INTEGER NOT NULL,
            status TEXT NOT NULL,
            http_status INTEGER,
            provider_message_id TEXT,
            error_message TEXT,
            failure_code TEXT,
            failure_hint TEXT,
            failure_details_json TEXT,
            attempted_at INTEGER NOT NULL,
            FOREIGN KEY (delivery_id) REFERENCES alert_deliveries(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_alert_delivery_attempts_delivery
            ON alert_delivery_attempts(delivery_id, attempted_at DESC);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_alert_delivery_attempts_number
            ON alert_delivery_attempts(delivery_id, attempt_number);

        CREATE TABLE IF NOT EXISTS alert_channel_checks (
            id TEXT PRIMARY KEY,
            channel TEXT NOT NULL CHECK (
                length(channel) BETWEEN 1 AND 64
                AND channel = lower(channel)
                AND channel NOT GLOB '*[^a-z0-9_-]*'
            ),
            status TEXT NOT NULL CHECK (status IN ('success', 'failed')),
            http_status INTEGER,
            provider_message_id TEXT,
            error_message TEXT,
            failure_code TEXT,
            failure_hint TEXT,
            failure_details_json TEXT,
            tested_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_alert_channel_checks_channel_time
            ON alert_channel_checks(channel, tested_at DESC);

        CREATE TABLE IF NOT EXISTS alert_source_events (
            source_event_id TEXT PRIMARY KEY,
            source_kind TEXT NOT NULL,
            received_at INTEGER NOT NULL,
            incident_id TEXT,
            transition_id TEXT,
            processed_at INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_alert_source_events_processed
            ON alert_source_events(processed_at);
        ",
    )
    .map_err(|error| AppError::Internal(format!("init alert database failed: {error}")))?;
    for (table, column, definition) in [
        ("alert_delivery_attempts", "failure_code", "TEXT"),
        ("alert_delivery_attempts", "failure_hint", "TEXT"),
        ("alert_delivery_attempts", "failure_details_json", "TEXT"),
        ("alert_channel_checks", "failure_code", "TEXT"),
        ("alert_channel_checks", "failure_hint", "TEXT"),
        ("alert_channel_checks", "failure_details_json", "TEXT"),
    ] {
        ensure_alert_column(conn, table, column, definition)?;
    }
    Ok(())
}

fn ensure_alert_column(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), AppError> {
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|error| {
            AppError::Internal(format!("inspect alert table {table} failed: {error}"))
        })?;
    let mut rows = statement.query([]).map_err(|error| {
        AppError::Internal(format!("read alert table {table} columns failed: {error}"))
    })?;
    let mut exists = false;
    while let Some(row) = rows.next().map_err(|error| {
        AppError::Internal(format!(
            "iterate alert table {table} columns failed: {error}"
        ))
    })? {
        if row.get::<_, String>(1).map_err(|error| {
            AppError::Internal(format!("read alert table {table} column failed: {error}"))
        })? == column
        {
            exists = true;
            break;
        }
    }
    drop(rows);
    drop(statement);
    if !exists {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )
        .map_err(|error| {
            AppError::Internal(format!("add alert table {table}.{column} failed: {error}"))
        })?;
    }
    Ok(())
}

fn observe_condition_tx(
    tx: &Transaction<'_>,
    condition: &AlertCondition,
    now: i64,
    repeat_interval_secs: i64,
    policy: &AlertDeliveryPolicy,
) -> Result<Option<AlertTransition>, AppError> {
    let Some(mut incident) = load_active_incident_by_fingerprint_tx(tx, &condition.fingerprint)?
    else {
        return create_incident_tx(tx, condition, None, now, policy).map(Some);
    };

    let previous_severity = incident.severity.clone();
    let previous_status = incident.status.clone();
    let silence_expired =
        previous_status == "silenced" && incident.silenced_until.is_some_and(|until| until <= now);
    let escalated = severity_rank(&condition.severity) > severity_rank(&previous_severity);
    let next_status = if silence_expired || (escalated && previous_status == "acknowledged") {
        "firing"
    } else {
        previous_status.as_str()
    };
    tx.execute(
        "UPDATE alert_incidents
         SET severity = ?2, status = ?3, title = ?4, message = ?5,
             details_json = ?6, occurrence_count = occurrence_count + 1,
             last_seen_at = ?7,
             acknowledged_at = CASE WHEN ?8 = 1 THEN NULL ELSE acknowledged_at END,
             acknowledged_by = CASE WHEN ?8 = 1 THEN NULL ELSE acknowledged_by END,
             acknowledgement_note = CASE WHEN ?8 = 1 THEN NULL ELSE acknowledgement_note END,
             silenced_at = CASE WHEN ?9 = 1 THEN NULL ELSE silenced_at END,
             silenced_until = CASE WHEN ?9 = 1 THEN NULL ELSE silenced_until END,
             silenced_by = CASE WHEN ?9 = 1 THEN NULL ELSE silenced_by END,
             silence_note = CASE WHEN ?9 = 1 THEN NULL ELSE silence_note END
         WHERE id = ?1",
        params![
            incident.id,
            condition.severity,
            next_status,
            condition.title,
            condition.message,
            condition.details.to_string(),
            now,
            i64::from(escalated && previous_status == "acknowledged"),
            i64::from(silence_expired),
        ],
    )
    .map_err(|error| AppError::Internal(format!("update active alert incident failed: {error}")))?;

    incident.severity = condition.severity.clone();
    incident.status = next_status.into();
    incident.title = condition.title.clone();
    incident.message = condition.message.clone();
    incident.details = condition.details.clone();
    incident.last_seen_at = now;
    incident.occurrence_count = incident.occurrence_count.saturating_add(1);

    let transition_kind = if escalated {
        Some("escalated")
    } else if silence_expired {
        Some("unsilenced")
    } else if next_status == "firing"
        && now.saturating_sub(latest_notification_transition_at_tx(tx, &incident.id)?)
            >= repeat_interval_secs.max(60)
    {
        Some("reminder")
    } else {
        None
    };
    let Some(transition_kind) = transition_kind else {
        return Ok(None);
    };
    tx.execute(
        "UPDATE alert_incidents SET last_transition_at = ?2 WHERE id = ?1",
        params![incident.id, now],
    )
    .map_err(|error| AppError::Internal(format!("touch alert transition time failed: {error}")))?;
    insert_transition_tx(
        tx,
        &incident,
        None,
        transition_kind,
        &condition.message,
        condition.details.clone(),
        None,
        None,
        now,
        true,
        policy,
    )
    .map(Some)
}

fn observe_signal_tx(
    tx: &Transaction<'_>,
    signal: &OperatorAlertSignal,
    policy: &AlertDeliveryPolicy,
) -> Result<Option<AlertTransition>, AppError> {
    if let Some(mut incident) = load_active_incident_by_fingerprint_tx(tx, &signal.fingerprint)? {
        tx.execute(
            "UPDATE alert_incidents
             SET severity = ?2, title = ?3, message = ?4, details_json = ?5,
                 occurrence_count = occurrence_count + 1, last_seen_at = ?6
             WHERE id = ?1",
            params![
                incident.id,
                signal.severity,
                signal.title,
                signal.message,
                signal.details.to_string(),
                signal.occurred_at,
            ],
        )
        .map_err(|error| AppError::Internal(format!("observe alert signal failed: {error}")))?;
        incident.severity = signal.severity.clone();
        incident.message = signal.message.clone();
        incident.details = signal.details.clone();
        incident.last_seen_at = signal.occurred_at;
        return insert_transition_tx(
            tx,
            &incident,
            Some(&signal.source_event_id),
            "observed",
            &signal.message,
            signal.details.clone(),
            None,
            None,
            signal.occurred_at,
            false,
            policy,
        )
        .map(Some);
    }
    let condition = AlertCondition {
        fingerprint: signal.fingerprint.clone(),
        scope: "client".into(),
        kind: signal.kind.clone(),
        entity_kind: signal.entity_kind.clone(),
        entity_id: signal.entity_id.clone(),
        severity: signal.severity.clone(),
        title: signal.title.clone(),
        message: signal.message.clone(),
        details: signal.details.clone(),
    };
    create_incident_tx(
        tx,
        &condition,
        Some(&signal.source_event_id),
        signal.occurred_at,
        policy,
    )
    .map(Some)
}

fn create_incident_tx(
    tx: &Transaction<'_>,
    condition: &AlertCondition,
    source_event_id: Option<&str>,
    now: i64,
    policy: &AlertDeliveryPolicy,
) -> Result<AlertTransition, AppError> {
    let incident = StoredIncident {
        id: Uuid::new_v4().to_string(),
        fingerprint: condition.fingerprint.clone(),
        scope: condition.scope.clone(),
        kind: condition.kind.clone(),
        entity_kind: condition.entity_kind.clone(),
        entity_id: condition.entity_id.clone(),
        severity: normalize_severity(&condition.severity).into(),
        status: "firing".into(),
        title: condition.title.clone(),
        message: condition.message.clone(),
        details: condition.details.clone(),
        occurrence_count: 1,
        started_at: now,
        last_seen_at: now,
        last_transition_at: now,
        resolved_at: None,
        acknowledged_at: None,
        acknowledged_by: None,
        acknowledgement_note: None,
        silenced_at: None,
        silenced_until: None,
        silenced_by: None,
        silence_note: None,
    };
    tx.execute(
        "INSERT INTO alert_incidents (
            id, fingerprint, scope, kind, entity_kind, entity_id, severity, status,
            title, message, details_json, occurrence_count, started_at, last_seen_at,
            last_transition_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'firing', ?8, ?9, ?10, 1, ?11, ?11, ?11)",
        params![
            incident.id,
            incident.fingerprint,
            incident.scope,
            incident.kind,
            incident.entity_kind,
            incident.entity_id,
            incident.severity,
            incident.title,
            incident.message,
            incident.details.to_string(),
            now,
        ],
    )
    .map_err(|error| AppError::Internal(format!("create alert incident failed: {error}")))?;
    insert_transition_tx(
        tx,
        &incident,
        source_event_id,
        "firing",
        &condition.message,
        condition.details.clone(),
        None,
        None,
        now,
        true,
        policy,
    )
}

fn resolve_incident_tx(
    tx: &Transaction<'_>,
    incident: &StoredIncident,
    source_event_id: Option<&str>,
    message: &str,
    details: serde_json::Value,
    now: i64,
    policy: &AlertDeliveryPolicy,
) -> Result<Option<AlertTransition>, AppError> {
    if incident.resolved_at.is_some() {
        return Ok(None);
    }
    let updated = tx
        .execute(
            "UPDATE alert_incidents
             SET status = 'resolved', resolved_at = ?2, last_seen_at = ?2,
                 last_transition_at = ?2, message = ?3, details_json = ?4
             WHERE id = ?1 AND resolved_at IS NULL",
            params![incident.id, now, message, details.to_string()],
        )
        .map_err(|error| AppError::Internal(format!("resolve alert incident failed: {error}")))?;
    if updated == 0 {
        return Ok(None);
    }
    insert_transition_tx(
        tx,
        incident,
        source_event_id,
        "resolved",
        message,
        details,
        None,
        None,
        now,
        true,
        policy,
    )
    .map(Some)
}

#[allow(clippy::too_many_arguments)]
fn insert_transition_tx(
    tx: &Transaction<'_>,
    incident: &StoredIncident,
    source_event_id: Option<&str>,
    transition: &str,
    message: &str,
    details: serde_json::Value,
    actor_email: Option<&str>,
    note: Option<&str>,
    now: i64,
    notify: bool,
    policy: &AlertDeliveryPolicy,
) -> Result<AlertTransition, AppError> {
    let record = AlertTransition {
        id: Uuid::new_v4().to_string(),
        incident_id: incident.id.clone(),
        source_event_id: source_event_id.map(str::to_string),
        transition: transition.into(),
        severity: incident.severity.clone(),
        message: message.into(),
        details,
        actor_email: actor_email.map(str::to_string),
        note: note.map(str::to_string),
        occurred_at: now,
    };
    tx.execute(
        "INSERT INTO alert_transitions (
            id, incident_id, source_event_id, transition, severity, message,
            details_json, actor_email, note, occurred_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            record.id,
            record.incident_id,
            record.source_event_id,
            record.transition,
            record.severity,
            record.message,
            record.details.to_string(),
            record.actor_email,
            record.note,
            record.occurred_at,
        ],
    )
    .map_err(|error| AppError::Internal(format!("insert alert transition failed: {error}")))?;
    if notify || matches!(transition, "acknowledged" | "silenced") {
        supersede_stale_deliveries_tx(tx, incident, &record, now)?;
    }
    if notify && policy.enabled {
        enqueue_deliveries_tx(tx, incident, &record, policy)?;
    }
    Ok(record)
}

fn enqueue_deliveries_tx(
    tx: &Transaction<'_>,
    incident: &StoredIncident,
    transition: &AlertTransition,
    policy: &AlertDeliveryPolicy,
) -> Result<(), AppError> {
    let payload = render_delivery_payload(incident, transition, &policy.dashboard_url);
    for channel in &policy.channels {
        let recovery_for_previously_targeted_channel = transition.transition == "resolved"
            && tx
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM alert_deliveries
                        WHERE incident_id = ?1 AND channel = ?2
                     )",
                    params![incident.id, channel.channel],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| {
                    AppError::Internal(format!(
                        "check prior alert channel delivery failed: {error}"
                    ))
                })?
                != 0;
        if severity_rank(&transition.severity) < severity_rank(&channel.min_severity)
            && !recovery_for_previously_targeted_channel
        {
            continue;
        }
        tx.execute(
            "INSERT OR IGNORE INTO alert_deliveries (
                id, incident_id, transition_id, channel, status, payload_text,
                attempts, next_attempt_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, 'pending', ?5, 0, ?6, ?6, ?6)",
            params![
                Uuid::new_v4().to_string(),
                incident.id,
                transition.id,
                channel.channel,
                payload,
                transition.occurred_at,
            ],
        )
        .map_err(|error| AppError::Internal(format!("enqueue alert delivery failed: {error}")))?;
    }
    Ok(())
}

fn supersede_stale_deliveries_tx(
    tx: &Transaction<'_>,
    incident: &StoredIncident,
    transition: &AlertTransition,
    now: i64,
) -> Result<(), AppError> {
    tx.execute(
        "UPDATE alert_deliveries
         SET status = 'superseded', next_attempt_at = NULL,
             claimed_by = NULL, claim_expires_at = NULL,
             last_error = ?3, updated_at = ?4
         WHERE incident_id = ?1 AND transition_id != ?2
           AND status IN ('pending', 'retry', 'dead_letter', 'suppressed_disabled')",
        params![
            incident.id,
            transition.id,
            format!("superseded by {} transition", transition.transition),
            now,
        ],
    )
    .map_err(|error| AppError::Internal(format!("supersede alert deliveries failed: {error}")))?;
    Ok(())
}

fn render_delivery_payload(
    incident: &StoredIncident,
    transition: &AlertTransition,
    dashboard_url: &str,
) -> String {
    let state = transition.transition.to_ascii_uppercase();
    let severity = incident.severity.to_ascii_uppercase();
    let mut lines = vec![
        format!("[CC-Switch Router] {severity} · {state}"),
        incident.title.clone(),
        transition.message.clone(),
        format!("Kind: {}", incident.kind),
    ];
    if let Some(entity_id) = incident.entity_id.as_deref() {
        lines.push(format!("Entity: {} / {entity_id}", incident.entity_kind));
    }
    lines.push(format!(
        "Started: {}",
        format_timestamp(incident.started_at)
    ));
    lines.push(format!(
        "Updated: {}",
        format_timestamp(transition.occurred_at)
    ));
    if !dashboard_url.trim().is_empty() {
        lines.push(format!(
            "Dashboard: {}/metrics/",
            dashboard_url.trim_end_matches('/')
        ));
    }
    lines.join("\n")
}

fn format_timestamp(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|value| value.to_rfc3339())
        .unwrap_or_else(|| timestamp.to_string())
}

fn normalize_severity(value: &str) -> &'static str {
    match value {
        "critical" => "critical",
        "info" => "info",
        _ => "warning",
    }
}

fn latest_notification_transition_at_tx(
    tx: &Transaction<'_>,
    incident_id: &str,
) -> Result<i64, AppError> {
    tx.query_row(
        "SELECT COALESCE(MAX(occurred_at), 0) FROM alert_transitions
         WHERE incident_id = ?1
           AND transition IN ('firing', 'escalated', 'reminder', 'unsilenced', 'resumed')",
        params![incident_id],
        |row| row.get(0),
    )
    .map_err(|error| AppError::Internal(format!("load alert reminder time failed: {error}")))
}

fn load_active_incident_by_fingerprint_tx(
    tx: &Transaction<'_>,
    fingerprint: &str,
) -> Result<Option<StoredIncident>, AppError> {
    tx.query_row(
        &format!(
            "SELECT {INCIDENT_COLUMNS} FROM alert_incidents
             WHERE fingerprint = ?1 AND resolved_at IS NULL LIMIT 1"
        ),
        params![fingerprint],
        map_incident_row,
    )
    .optional()
    .map_err(|error| AppError::Internal(format!("load active alert incident failed: {error}")))
}

fn load_incident_by_id_tx(
    tx: &Transaction<'_>,
    incident_id: &str,
) -> Result<Option<StoredIncident>, AppError> {
    tx.query_row(
        &format!("SELECT {INCIDENT_COLUMNS} FROM alert_incidents WHERE id = ?1"),
        params![incident_id],
        map_incident_row,
    )
    .optional()
    .map_err(|error| AppError::Internal(format!("load alert incident failed: {error}")))
}

fn load_active_incidents_for_scope_tx(
    tx: &Transaction<'_>,
    scope: &str,
) -> Result<Vec<StoredIncident>, AppError> {
    let mut statement = tx
        .prepare(&format!(
            "SELECT {INCIDENT_COLUMNS} FROM alert_incidents
             WHERE scope = ?1 AND resolved_at IS NULL"
        ))
        .map_err(|error| AppError::Internal(format!("prepare active incidents failed: {error}")))?;
    let rows = statement
        .query_map(params![scope], map_incident_row)
        .map_err(|error| AppError::Internal(format!("query active incidents failed: {error}")))?;
    collect_rows(rows, "active alert incidents")
}

fn load_expired_silences_tx(
    tx: &Transaction<'_>,
    now: i64,
) -> Result<Vec<StoredIncident>, AppError> {
    let mut statement = tx
        .prepare(&format!(
            "SELECT {INCIDENT_COLUMNS} FROM alert_incidents
             WHERE status = 'silenced' AND resolved_at IS NULL
               AND silenced_until IS NOT NULL AND silenced_until <= ?1"
        ))
        .map_err(|error| AppError::Internal(format!("prepare expired silences failed: {error}")))?;
    let rows = statement
        .query_map(params![now], map_incident_row)
        .map_err(|error| AppError::Internal(format!("query expired silences failed: {error}")))?;
    collect_rows(rows, "expired alert silences")
}

fn load_incidents(
    conn: &Connection,
    limit: usize,
    active_only: bool,
) -> Result<Vec<AlertIncident>, AppError> {
    let predicate = if active_only {
        "WHERE resolved_at IS NULL"
    } else {
        ""
    };
    let mut statement = conn
        .prepare(&format!(
            "SELECT {INCIDENT_COLUMNS} FROM alert_incidents {predicate}
             ORDER BY CASE WHEN resolved_at IS NULL THEN 0 ELSE 1 END,
                      last_transition_at DESC, started_at DESC
             LIMIT ?1"
        ))
        .map_err(|error| AppError::Internal(format!("prepare alert incidents failed: {error}")))?;
    let rows = statement
        .query_map(params![limit as i64], map_incident_row)
        .map_err(|error| AppError::Internal(format!("query alert incidents failed: {error}")))?;
    Ok(collect_rows(rows, "alert incidents")?
        .into_iter()
        .map(AlertIncident::from)
        .collect())
}

fn load_deliveries(conn: &Connection, limit: usize) -> Result<Vec<AlertDelivery>, AppError> {
    let mut statement = conn
        .prepare(
            "SELECT id, incident_id, transition_id, channel, status, attempts,
                    provider_message_id, next_attempt_at, last_error,
                    created_at, updated_at, sent_at
             FROM alert_deliveries
             ORDER BY updated_at DESC, created_at DESC LIMIT ?1",
        )
        .map_err(|error| AppError::Internal(format!("prepare alert deliveries failed: {error}")))?;
    let rows = statement
        .query_map(params![limit as i64], |row| {
            Ok(AlertDelivery {
                id: row.get(0)?,
                incident_id: row.get(1)?,
                transition_id: row.get(2)?,
                channel: row.get(3)?,
                status: row.get(4)?,
                attempts: row.get::<_, i64>(5)?.max(0) as u32,
                provider_message_id: row.get(6)?,
                next_attempt_at: row.get(7)?,
                last_error: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
                sent_at: row.get(11)?,
            })
        })
        .map_err(|error| AppError::Internal(format!("query alert deliveries failed: {error}")))?;
    collect_rows(rows, "alert deliveries")
}

const INCIDENT_COLUMNS: &str = "id, fingerprint, scope, kind, entity_kind, entity_id,
    severity, status, title, message, details_json, occurrence_count,
    started_at, last_seen_at, last_transition_at, resolved_at,
    acknowledged_at, acknowledged_by, acknowledgement_note,
    silenced_at, silenced_until, silenced_by, silence_note";

fn map_incident_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredIncident> {
    let details_raw = row.get::<_, String>(10)?;
    Ok(StoredIncident {
        id: row.get(0)?,
        fingerprint: row.get(1)?,
        scope: row.get(2)?,
        kind: row.get(3)?,
        entity_kind: row.get(4)?,
        entity_id: row.get(5)?,
        severity: row.get(6)?,
        status: row.get(7)?,
        title: row.get(8)?,
        message: row.get(9)?,
        details: serde_json::from_str(&details_raw).unwrap_or_else(|_| serde_json::json!({})),
        occurrence_count: row.get::<_, i64>(11)?.max(0) as u64,
        started_at: row.get(12)?,
        last_seen_at: row.get(13)?,
        last_transition_at: row.get(14)?,
        resolved_at: row.get(15)?,
        acknowledged_at: row.get(16)?,
        acknowledged_by: row.get(17)?,
        acknowledgement_note: row.get(18)?,
        silenced_at: row.get(19)?,
        silenced_until: row.get(20)?,
        silenced_by: row.get(21)?,
        silence_note: row.get(22)?,
    })
}

fn collect_rows<T, F>(rows: rusqlite::MappedRows<'_, F>, label: &str) -> Result<Vec<T>, AppError>
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
{
    let mut values = Vec::new();
    for row in rows {
        values.push(
            row.map_err(|error| AppError::Internal(format!("read {label} failed: {error}")))?,
        );
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerting::models::AlertChannelPolicy;

    fn test_connection() -> Connection {
        crate::db::initialize_sqlite_runtime().expect("initialize SQLite test runtime");
        Connection::open_in_memory().expect("open alert test database")
    }

    fn test_store(label: &str) -> AlertStore {
        crate::db::initialize_sqlite_runtime().expect("initialize SQLite test runtime");
        AlertStore::new(std::env::temp_dir().join(format!(
            "cc-switch-router-alert-{label}-{}.db",
            Uuid::new_v4()
        )))
    }

    #[test]
    fn init_alert_db_adds_diagnostic_columns_to_an_existing_database() {
        let conn = test_connection();
        conn.execute_batch(
            "CREATE TABLE alert_delivery_attempts (
                id TEXT PRIMARY KEY,
                delivery_id TEXT NOT NULL,
                attempt_number INTEGER NOT NULL,
                status TEXT NOT NULL,
                http_status INTEGER,
                provider_message_id TEXT,
                error_message TEXT,
                attempted_at INTEGER NOT NULL
             );
             CREATE TABLE alert_channel_checks (
                id TEXT PRIMARY KEY,
                channel TEXT NOT NULL,
                status TEXT NOT NULL,
                http_status INTEGER,
                provider_message_id TEXT,
                error_message TEXT,
                tested_at INTEGER NOT NULL
             );",
        )
        .expect("seed legacy alert tables");

        init_alert_db(&conn).expect("upgrade alert tables");
        for (table, column) in [
            ("alert_delivery_attempts", "failure_code"),
            ("alert_delivery_attempts", "failure_hint"),
            ("alert_delivery_attempts", "failure_details_json"),
            ("alert_channel_checks", "failure_code"),
            ("alert_channel_checks", "failure_hint"),
            ("alert_channel_checks", "failure_details_json"),
        ] {
            let present = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1"),
                    params![column],
                    |row| row.get::<_, i64>(0),
                )
                .expect("inspect upgraded alert column");
            assert_eq!(present, 1, "missing {table}.{column}");
        }
    }

    fn condition() -> AlertCondition {
        AlertCondition {
            fingerprint: "fd_pressure:router:router".into(),
            scope: "metrics".into(),
            kind: "fd_pressure".into(),
            entity_kind: "router".into(),
            entity_id: Some("router".into()),
            severity: "warning".into(),
            title: "FD pressure".into(),
            message: "FD usage is elevated".into(),
            details: serde_json::json!({ "percent": 75 }),
        }
    }

    fn no_delivery() -> AlertDeliveryPolicy {
        AlertDeliveryPolicy {
            enabled: false,
            dashboard_url: "https://router.example.com".into(),
            channels: Vec::new(),
        }
    }

    #[tokio::test]
    async fn metric_condition_opens_and_resolves_one_incident() {
        let store = test_store("reconcile");
        store
            .reconcile_conditions("metrics".into(), vec![condition()], 100, 300, no_delivery())
            .await
            .expect("open incident");
        store
            .reconcile_conditions("metrics".into(), Vec::new(), 110, 300, no_delivery())
            .await
            .expect("resolve incident");
        let incidents = store
            .list_incidents(10, false)
            .await
            .expect("list incidents");
        assert_eq!(incidents.len(), 1);
        assert_eq!(incidents[0].status, "resolved");
        assert_eq!(incidents[0].resolved_at, Some(110));
    }

    #[tokio::test]
    async fn source_event_is_idempotent() {
        let store = test_store("signal-dedupe");
        let signal = OperatorAlertSignal {
            source_event_id: "client_presence:offline:one:1".into(),
            fingerprint: "client_offline:client:one".into(),
            transition: "firing".into(),
            kind: "client_offline".into(),
            entity_kind: "client".into(),
            entity_id: Some("one".into()),
            severity: "critical".into(),
            title: "Client offline".into(),
            message: "Client one is offline".into(),
            details: serde_json::json!({}),
            occurred_at: 100,
            attempts: 1,
        };
        assert!(
            store
                .ingest_signal(signal.clone(), no_delivery())
                .await
                .expect("first signal")
                .is_some()
        );
        assert!(
            store
                .ingest_signal(signal, no_delivery())
                .await
                .expect("duplicate signal")
                .is_none()
        );
        assert_eq!(store.list_incidents(10, true).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn acknowledgement_and_silence_do_not_resolve_active_incident() {
        let store = test_store("operator-state");
        store
            .reconcile_conditions("metrics".into(), vec![condition()], 100, 300, no_delivery())
            .await
            .unwrap();
        let incident = store.list_incidents(1, true).await.unwrap().remove(0);
        let acknowledged = store
            .acknowledge(
                incident.id.clone(),
                "admin@example.com".into(),
                Some("checking".into()),
                110,
            )
            .await
            .unwrap();
        assert_eq!(acknowledged.status, "acknowledged");
        let silenced = store
            .silence(incident.id, "admin@example.com".into(), None, 120, 600)
            .await
            .unwrap();
        assert_eq!(silenced.status, "silenced");
        assert!(silenced.resolved_at.is_none());
    }

    #[tokio::test]
    async fn delivery_is_frozen_and_claimed_once() {
        let store = test_store("delivery");
        let policy = AlertDeliveryPolicy {
            enabled: true,
            dashboard_url: "https://router.example.com".into(),
            channels: vec![AlertChannelPolicy {
                channel: "telegram".into(),
                min_severity: "warning".into(),
            }],
        };
        store
            .reconcile_conditions("metrics".into(), vec![condition()], 100, 300, policy)
            .await
            .unwrap();
        let claim = store
            .claim_delivery("worker".into(), 100, 30)
            .await
            .unwrap()
            .expect("claim delivery");
        assert_eq!(claim.channel, "telegram");
        assert!(claim.payload_text.contains("FD pressure"));
        assert!(
            store
                .claim_delivery("worker".into(), 101, 30)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn expired_claim_cannot_overwrite_reclaimed_delivery() {
        let store = test_store("delivery-reclaim");
        let policy = AlertDeliveryPolicy {
            enabled: true,
            dashboard_url: "https://router.example.com".into(),
            channels: vec![AlertChannelPolicy {
                channel: "telegram".into(),
                min_severity: "warning".into(),
            }],
        };
        store
            .reconcile_conditions("metrics".into(), vec![condition()], 100, 300, policy)
            .await
            .unwrap();
        let stale = store
            .claim_delivery("worker-one".into(), 100, 10)
            .await
            .unwrap()
            .expect("first claim");
        let current = store
            .claim_delivery("worker-two".into(), 111, 10)
            .await
            .unwrap()
            .expect("reclaimed delivery");

        store
            .finish_delivery(
                stale,
                AlertDeliveryResult::Sent {
                    provider_message_id: Some("stale".into()),
                    http_status: Some(200),
                },
                112,
            )
            .await
            .unwrap();
        let claimed = store.list_deliveries(1).await.unwrap().remove(0);
        assert_eq!(claimed.status, "claimed");
        assert_eq!(claimed.attempts, 2);

        store
            .finish_delivery(
                current,
                AlertDeliveryResult::Retry {
                    error: "temporary".into(),
                    failure_code: Some("api_timeout".into()),
                    failure_hint: Some("retry later".into()),
                    failure_details: None,
                    next_attempt_at: 130,
                    http_status: Some(503),
                },
                113,
            )
            .await
            .unwrap();
        let retried = store.list_deliveries(1).await.unwrap().remove(0);
        assert_eq!(retried.status, "retry");
        assert_eq!(retried.next_attempt_at, Some(130));
    }

    #[tokio::test]
    async fn channel_tests_drive_latest_channel_state() {
        let store = test_store("channel-activity");
        store
            .record_channel_test(
                "secondary".into(),
                false,
                None,
                Some(409),
                Some("temporary provider failure".into()),
                Some("api_endpoint_unreachable".into()),
                Some("check DNS".into()),
                Some(serde_json::json!({"resolvedAddresses": ["192.0.2.1"]})),
                100,
            )
            .await
            .unwrap();
        let failed = store.channel_activity().await.unwrap();
        let failed_activity = failed.get("secondary").expect("failed activity");
        assert_eq!(failed_activity.last_attempt_at, Some(100));
        assert_eq!(failed_activity.last_success_at, None);
        assert_eq!(
            failed_activity.last_error.as_deref(),
            Some("temporary provider failure")
        );
        assert_eq!(
            failed_activity.failure_code.as_deref(),
            Some("api_endpoint_unreachable")
        );
        assert_eq!(failed_activity.failure_hint.as_deref(), Some("check DNS"));
        assert_eq!(
            failed_activity.failure_details,
            Some(serde_json::json!({"resolvedAddresses": ["192.0.2.1"]}))
        );

        store
            .record_channel_test(
                "secondary".into(),
                true,
                Some("message-one".into()),
                Some(200),
                None,
                None,
                None,
                None,
                110,
            )
            .await
            .unwrap();
        let recovered = store.channel_activity().await.unwrap();
        let recovered_activity = recovered.get("secondary").expect("recovered activity");
        assert_eq!(recovered_activity.last_attempt_at, Some(110));
        assert_eq!(recovered_activity.last_success_at, Some(110));
        assert_eq!(recovered_activity.last_error, None);
        assert_eq!(recovered_activity.failure_code, None);
    }

    #[tokio::test]
    async fn newer_transition_supersedes_stale_delivery_and_preserves_recovery_channel() {
        let store = test_store("delivery-superseded");
        let policy = AlertDeliveryPolicy {
            enabled: true,
            dashboard_url: "https://router.example.com".into(),
            channels: vec![AlertChannelPolicy {
                channel: "telegram".into(),
                min_severity: "critical".into(),
            }],
        };
        let mut critical = condition();
        critical.severity = "critical".into();
        store
            .reconcile_conditions("metrics".into(), vec![critical], 100, 300, policy.clone())
            .await
            .unwrap();

        let mut warning = condition();
        warning.severity = "warning".into();
        store
            .reconcile_conditions("metrics".into(), vec![warning], 110, 300, policy.clone())
            .await
            .unwrap();
        store
            .reconcile_conditions("metrics".into(), Vec::new(), 120, 300, policy)
            .await
            .unwrap();

        let deliveries = store.list_deliveries(10).await.unwrap();
        assert_eq!(deliveries.len(), 2);
        let stale = deliveries
            .iter()
            .find(|delivery| delivery.status == "superseded")
            .expect("stale firing delivery must be superseded");
        assert!(store.retry_delivery(stale.id.clone(), 121).await.is_err());
        let recovery = store
            .claim_delivery("worker".into(), 120, 30)
            .await
            .unwrap()
            .expect("recovery delivery must retain the previously targeted channel");
        assert!(recovery.payload_text.contains("RESOLVED"));

        let counts = store.overview_counts().await.unwrap();
        assert_eq!(counts.active, 0);
        assert_eq!(counts.critical, 0);
        assert_eq!(counts.resolved, 1);
        assert_eq!(counts.failed_deliveries, 0);
    }
}
