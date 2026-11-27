//! Durable Client identity and explicit, audited repair of pre-registry orphans.
use super::*;

pub(super) struct RememberedIdentity {
    pub id: String,
    pub owner_email: Option<String>,
    pub owner_verified_at: Option<DateTime<Utc>>,
    pub activated_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub fenced: bool,
}

pub(super) fn remembered_identity(
    conn: &Connection,
    key: &str,
) -> Result<Option<RememberedIdentity>, AppError> {
    let raw = conn.query_row(
        "SELECT installation_id,owner_email,owner_verified_at,client_activated_at,created_at,lifecycle FROM client_identities WHERE public_key=?1",
        params![key], |r| Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?))
    ).optional().map_err(|e| AppError::Internal(format!("read durable Client identity: {e}")))?;
    raw.map(|(id, owner_email, verified, activated, created, state)| {
        let parse = |value: String| {
            DateTime::parse_from_rfc3339(&value)
                .map(|at| at.with_timezone(&Utc))
                .map_err(|e| AppError::Internal(format!("invalid Client identity timestamp: {e}")))
        };
        Ok(RememberedIdentity {
            id,
            owner_email,
            owner_verified_at: verified.map(parse).transpose()?,
            activated_at: activated.map(parse).transpose()?,
            created_at: parse(created)?,
            fenced: state != "active",
        })
    })
    .transpose()
}

pub(super) fn registration_aliases(conn: &Connection, id: &str) -> Result<Vec<String>, AppError> {
    let mut stmt=conn.prepare("SELECT previous_installation_id FROM client_identity_aliases WHERE installation_id=?1 ORDER BY created_at,previous_installation_id")
        .map_err(|e| AppError::Internal(format!("prepare identity aliases: {e}")))?;
    let rows = stmt
        .query_map(params![id], |r| r.get::<_, String>(0))
        .map_err(|e| AppError::Internal(format!("read identity aliases: {e}")))?;
    collect_rows(rows)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientIdentityRecoveryRequest {
    pub original_installation_id: String,
    pub replacement_installation_id: String,
    pub public_key: String,
    pub subdomain: String,
    pub expected_host_revision: i64,
    /// Description/hash of independently verified operator evidence; never a private key.
    pub evidence: String,
    #[serde(default)]
    pub apply: bool,
    #[serde(default)]
    pub expected_plan_hash: Option<String>,
}

/// Snapshot every runtime column before deleting the replacement. Keeping
/// opaque SQL values avoids losing optional metadata or exposing control keys.
struct RecoveryPlan {
    columns: Vec<String>,
    values: Vec<crate::db::types::Value>,
    hash: String,
    result: serde_json::Value,
    created_at: String,
    offline_episode: i64,
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn recovery_conflict(reason: &'static str, message: &str, details: serde_json::Value) -> AppError {
    AppError::coded_conflict(reason, message, details)
}

fn referencing_rows(conn: &Connection, id: &str) -> Result<Vec<serde_json::Value>, AppError> {
    let tables = {
        let mut statement = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND sql LIKE '%installations%' AND name!='installations'")
            .map_err(|e| AppError::Internal(format!("inspect recovery references: {e}")))?;
        collect_rows(
            statement
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| AppError::Internal(e.to_string()))?,
        )?
    };
    let mut references = Vec::new();
    for table in tables {
        let foreign_keys = {
            let mut statement = conn
                .prepare(&format!("PRAGMA foreign_key_list({})", quoted(&table)))
                .map_err(|e| AppError::Internal(e.to_string()))?;
            collect_rows(
                statement
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, Option<String>>(4)?
                                .unwrap_or_else(|| "id".into()),
                            r.get::<_, String>(6)?,
                        ))
                    })
                    .map_err(|e| AppError::Internal(e.to_string()))?,
            )?
        };
        for (parent, column, parent_column, on_delete) in foreign_keys {
            if !parent.eq_ignore_ascii_case("installations") {
                continue;
            }
            let count: i64 = conn
                .query_row(
                    &format!(
                        "SELECT count(*) FROM {} AS child JOIN installations AS parent ON child.{}=parent.{} WHERE parent.id=?1",
                        quoted(&table),
                        quoted(&column),
                        quoted(&parent_column),
                    ),
                    params![id],
                    |r| r.get(0),
                )
                .map_err(|e| {
                    AppError::Internal(format!("inspect {table} recovery references: {e}"))
                })?;
            if count > 0 {
                references.push(serde_json::json!({"table":table,"column":column,"parentColumn":parent_column,"onDelete":on_delete,"count":count}));
            }
        }
    }
    references.sort_by_key(|item| {
        (
            item["table"].as_str().unwrap_or_default().to_string(),
            item["column"].as_str().unwrap_or_default().to_string(),
        )
    });
    Ok(references)
}

fn prepare_recovery(
    conn: &Connection,
    input: &ClientIdentityRecoveryRequest,
) -> Result<RecoveryPlan, AppError> {
    let original = &input.original_installation_id;
    let replacement = &input.replacement_installation_id;
    let host = crate::public_hosts::get_by_label(conn, &input.subdomain)
        .map_err(map_public_host_error)?
        .ok_or_else(|| AppError::NotFound("reserved Client host not found".into()))?;
    if host.kind != PublicHostKind::Client
        || host.subject_id != *original
        || host.installation_id.as_deref() != Some(original.as_str())
        || host.revision != input.expected_host_revision
        || host.lifecycle == crate::public_hosts::PublicHostLifecycle::Tombstoned
    {
        return Err(recovery_conflict(
            "CLIENT_IDENTITY_RECOVERY_HOST_CHANGED",
            "reserved Client host changed or belongs to another identity",
            serde_json::json!({"hostRevision":host.revision}),
        ));
    }
    if get_installation(conn, original)?.is_some() {
        return Err(recovery_conflict(
            "CLIENT_IDENTITY_RECOVERY_RUNTIME_EXISTS",
            "original runtime already exists; manual conflict review required",
            serde_json::json!({}),
        ));
    }
    let runtime = get_installation(conn, replacement)?
        .ok_or_else(|| AppError::NotFound("replacement runtime not found".into()))?;
    let active: bool = conn
        .query_row(
            "SELECT lifecycle='active' FROM installations WHERE id=?1",
            params![replacement],
            |row| row.get(0),
        )
        .map_err(|error| AppError::Internal(error.to_string()))?;
    let identity = remembered_identity(conn, &input.public_key)?;
    if runtime.public_key != input.public_key
        || !active
        || runtime.owner_verified_at.is_none()
        || runtime.owner_email.as_deref().is_none_or(str::is_empty)
        || identity.as_ref().is_none_or(|anchor| {
            anchor.id != *replacement
                || anchor.fenced
                || anchor.owner_verified_at.is_none()
                || anchor.owner_email != runtime.owner_email
        })
    {
        return Err(recovery_conflict(
            "CLIENT_IDENTITY_RECOVERY_ANCHOR_CHANGED",
            "replacement public key, durable identity or verified owner does not match recovery evidence",
            serde_json::json!({}),
        ));
    }
    let blocked: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM installation_fences WHERE public_key=?3 OR installation_id IN (?1,?2))
        OR EXISTS(SELECT 1 FROM client_identities WHERE installation_id=?1)
        OR EXISTS(SELECT 1 FROM client_identity_aliases WHERE previous_installation_id IN (?1,?2) OR installation_id IN (?1,?2))
        OR EXISTS(SELECT 1 FROM client_subdomain_takeovers WHERE target_installation_id IN (?1,?2) OR source_installation_id IN (?1,?2))
        OR EXISTS(SELECT 1 FROM public_hosts WHERE installation_id=?2)
        OR EXISTS(SELECT 1 FROM installation_client_tunnels WHERE installation_id=?2)
        OR EXISTS(SELECT 1 FROM shares WHERE installation_id=?2)
        OR EXISTS(SELECT 1 FROM online_client_listings WHERE installation_id IN (?1,?2))
        OR EXISTS(SELECT 1 FROM online_client_leases WHERE installation_id IN (?1,?2) AND released_at IS NULL)
        OR EXISTS(SELECT 1 FROM client_market_terminations WHERE host_id IN (?1,?2) AND (status='finance_pending' OR cleanup_confirmed_at IS NULL))
        OR EXISTS(SELECT 1 FROM online_client_operations WHERE installation_id IN (?1,?2))
        OR EXISTS(SELECT 1 FROM installation_upgrade_tasks WHERE installation_id=?2)
        OR EXISTS(SELECT 1 FROM share_value_owner_model_prices WHERE installation_id=?2)
        OR EXISTS(SELECT 1 FROM provisioning_jobs WHERE installation_id IN (?1,?2))
        OR EXISTS(SELECT 1 FROM subdomain_reservations WHERE installation_id IN (?1,?2))
        OR EXISTS(SELECT 1 FROM share_market_listings WHERE installation_id=?2)
        OR EXISTS(SELECT 1 FROM share_request_logs WHERE installation_id=?2)
        OR EXISTS(SELECT 1 FROM image_generation_jobs WHERE installation_id=?2)
        OR EXISTS(SELECT 1 FROM image_generation_request_logs WHERE installation_id=?2)
        OR EXISTS(SELECT 1 FROM share_edit_requests WHERE installation_id=?2)
        OR EXISTS(SELECT 1 FROM leases WHERE installation_id IN (?1,?2))", params![original,replacement,input.public_key], |r| r.get(0))
        .map_err(|e| AppError::Internal(format!("check recovery obligations: {e}")))?;
    if blocked {
        return Err(recovery_conflict(
            "CLIENT_IDENTITY_RECOVERY_OBLIGATIONS",
            "identity recovery is blocked by fencing, claims or business references",
            serde_json::json!({}),
        ));
    }
    let references = referencing_rows(conn, replacement)?;
    let blocking = references
        .iter()
        .filter(|item| {
            item["onDelete"].as_str() != Some("CASCADE")
                || !matches!(
                    item["table"].as_str(),
                    Some(
                        "installation_notification_state"
                            | "installation_setup_completions"
                            | "installation_value_capabilities"
                            | "share_value_pricing_activations"
                            | "share_value_pricing_installation_tariffs"
                            | "share_value_pricing_installation_revisions"
                    )
                )
        })
        .cloned()
        .collect::<Vec<_>>();
    if !blocking.is_empty() {
        return Err(recovery_conflict(
            "CLIENT_IDENTITY_RECOVERY_REFERENCES",
            "replacement has references that cannot be safely retired",
            serde_json::json!({"references":blocking}),
        ));
    }
    let offline_episode: i64 = conn.query_row(
        "SELECT max(0,
            COALESCE((SELECT offline_episode FROM client_identities WHERE installation_id=?2),0),
            COALESCE((SELECT max(offline_episode) FROM installation_notification_state WHERE installation_id IN (?1,?2)),0),
            COALESCE((SELECT max(episode) FROM client_notification_events WHERE installation_id IN (?1,?2)),0))",
        params![original,replacement], |row| row.get(0))
        .map_err(|error| AppError::Internal(format!("read recovered notification epoch: {error}")))?;
    let columns = {
        let mut statement = conn
            .prepare("PRAGMA table_info(installations)")
            .map_err(|e| AppError::Internal(e.to_string()))?;
        collect_rows(
            statement
                .query_map([], |r| r.get::<_, String>(1))
                .map_err(|e| AppError::Internal(e.to_string()))?,
        )?
    };
    let mut values = conn
        .query_row(
            &format!(
                "SELECT {} FROM installations WHERE id=?1",
                columns
                    .iter()
                    .map(|column| quoted(column))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            params![replacement],
            |row| {
                (0..columns.len())
                    .map(|index| row.get::<_, crate::db::types::Value>(index))
                    .collect::<Result<Vec<_>, _>>()
            },
        )
        .map_err(|e| AppError::Internal(format!("snapshot replacement runtime: {e}")))?;
    let secret_exists = columns
        .iter()
        .position(|name| name == "control_secret_b64")
        .and_then(|index| match &values[index] {
            crate::db::types::Value::Text(secret) => Some(!secret.is_empty()),
            _ => None,
        })
        .unwrap_or(false);
    if !secret_exists {
        return Err(AppError::Internal(
            "replacement has no control credential".into(),
        ));
    }
    // Fingerprint stable metadata without returning credentials. Heartbeats,
    // geolocation and status polling may advance between preview and apply.
    // Unknown future columns are included by default.
    let mut runtime_digest = Sha256::new();
    for (column, value) in columns.iter().zip(&values) {
        if column.starts_with("geo_")
            || matches!(
                column.as_str(),
                "last_seen_at"
                    | "last_seen_ip"
                    | "country_code"
                    | "country"
                    | "region"
                    | "city"
                    | "latitude"
                    | "longitude"
                    | "public_ip"
                    | "status_reported_at"
                    | "log_collection_reported_at"
                    | "update_available"
            )
        {
            continue;
        }
        runtime_digest.update(column.as_bytes());
        runtime_digest.update([0]);
        runtime_digest.update(format!("{value:?}").as_bytes());
        runtime_digest.update([0]);
    }
    let runtime_hash = hex::encode(runtime_digest.finalize());
    let hash = hex::encode(Sha256::digest(
        serde_json::to_vec(&serde_json::json!({
            "version":1,"original":original,"replacement":replacement,"publicKey":input.public_key,
            "subdomain":host.label,"revision":host.revision,"ownerEmail":runtime.owner_email,
            "ownerVerifiedAt":runtime.owner_verified_at,"runtimeHash":runtime_hash,"columns":columns,
            "hostLifecycle":host.lifecycle.as_str(),"hostCreatedAt":host.created_at,
            "offlineEpisode":offline_episode,
            "references":references,"evidence":input.evidence,
        }))
        .map_err(|e| AppError::Internal(e.to_string()))?,
    ));
    for (column, value) in columns.iter().zip(values.iter_mut()) {
        if column == "id" {
            *value = crate::db::types::Value::Text(original.clone());
        }
        if matches!(column.as_str(), "created_at" | "client_activated_at") {
            *value = crate::db::types::Value::Text(host.created_at.to_rfc3339());
        }
    }
    let result = serde_json::json!({"ok":true,"applied":input.apply,"installationId":original,
        "replacementInstallationId":replacement,"subdomain":host.label,"hostRevision":host.revision,
        "ownerEmail":runtime.owner_email,"planHash":hash,"retiredRuntimeReferences":references,"offlineEpisode":offline_episode});
    Ok(RecoveryPlan {
        columns,
        values,
        hash,
        result,
        created_at: host.created_at.to_rfc3339(),
        offline_episode,
    })
}

impl AppStore {
    /// Preview and apply build the same plan. No evidence can be inferred from
    /// an IP/email; the administrator attests an independently checked backup.
    pub async fn recover_client_identity(
        &self,
        actor: &str,
        input: ClientIdentityRecoveryRequest,
    ) -> Result<serde_json::Value, AppError> {
        if input.evidence.trim().len() < 16
            || input.evidence.len() > 1024
            || input.public_key.len() > 128
            || Uuid::parse_str(&input.original_installation_id).is_err()
            || Uuid::parse_str(&input.replacement_installation_id).is_err()
            || input.original_installation_id == input.replacement_installation_id
        {
            return Err(AppError::BadRequest("valid distinct IDs, public key and independently verified backup evidence are required".into()));
        }
        validate_ed25519_public_key(&input.public_key)?;
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| AppError::Internal(format!("begin identity recovery: {e}")))?;
        let alias:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM client_identity_aliases WHERE previous_installation_id=?1 AND installation_id=?2 AND public_key=?3)",params![input.replacement_installation_id,input.original_installation_id,input.public_key],|r|r.get(0)).map_err(|e|AppError::Internal(e.to_string()))?;
        if alias {
            return Ok(
                serde_json::json!({"ok":true,"applied":true,"alreadyApplied":true,"installationId":input.original_installation_id}),
            );
        }
        let plan = prepare_recovery(&tx, &input)?;
        if !input.apply {
            return Ok(plan.result);
        }
        if input.expected_plan_hash.as_deref() != Some(plan.hash.as_str()) {
            return Err(recovery_conflict(
                "CLIENT_IDENTITY_RECOVERY_PREVIEW_REQUIRED",
                "obtain a current recovery preview before applying",
                serde_json::json!({"planHash":plan.hash}),
            ));
        }
        let original = &input.original_installation_id;
        let replacement = &input.replacement_installation_id;
        let now = Utc::now().to_rfc3339();
        // Retire the replacement BEFORE inserting the same public key under
        // the original ID. The public-key unique index remains in force.
        purge_installation_data_tx(&tx, replacement)?;
        let names = plan
            .columns
            .iter()
            .map(|name| quoted(name))
            .collect::<Vec<_>>()
            .join(",");
        let placeholders = (1..=plan.values.len())
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(",");
        tx.execute(
            &format!("INSERT INTO installations ({names}) VALUES ({placeholders})"),
            params_from_iter(plan.values),
        )
        .map_err(|e| AppError::Internal(format!("restore original runtime: {e}")))?;
        let moved=tx.execute("UPDATE client_identities SET installation_id=?1,created_at=?4,client_activated_at=?4,updated_at=?5,offline_episode=?6 WHERE installation_id=?2 AND public_key=?3 AND lifecycle='active'",params![original,replacement,input.public_key,plan.created_at,now,plan.offline_episode])
            .map_err(|e|AppError::Internal(format!("restore identity anchor: {e}")))?;
        if moved != 1 {
            return Err(recovery_conflict(
                "CLIENT_IDENTITY_RECOVERY_ANCHOR_CHANGED",
                "replacement identity anchor changed",
                serde_json::json!({}),
            ));
        }
        insert_installation_notification_baseline(&tx, original, Utc::now())?;
        tx.execute("UPDATE installation_notification_state SET offline_episode=?2 WHERE installation_id=?1",
            params![original,plan.offline_episode])
            .map_err(|error| AppError::Internal(format!("restore recovered notification epoch: {error}")))?;
        tx.execute("INSERT INTO client_identity_aliases(previous_installation_id,installation_id,public_key,evidence,actor_email,created_at) VALUES(?1,?2,?3,?4,?5,?6)",params![replacement,original,input.public_key,input.evidence,actor,now]).map_err(|e|AppError::Internal(format!("audit identity recovery: {e}")))?;
        tx.execute("INSERT INTO admin_audit_log(id,actor_email,action,payload_json,created_at) VALUES(?1,?2,'client_identity.recover',?3,?4)",params![Uuid::new_v4().to_string(),actor,plan.result.to_string(),now]).map_err(|e|AppError::Internal(format!("record identity recovery audit: {e}")))?;
        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit identity recovery: {e}")))?;
        Ok(plan.result)
    }
}
