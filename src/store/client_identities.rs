//! Durable Client identity and explicit, audited repair of pre-registry orphans.
use super::*;

mod recovery_records;
use recovery_records::RecoveryRecords;

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
    /// Owner independently verified against the original backup by the preparation tool.
    pub expected_owner_email: Option<String>,
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
    records: RecoveryRecords,
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

fn recovery_db(error: crate::db::Error) -> AppError {
    AppError::Internal(format!("inspect or restore Client recovery data: {error}"))
}

fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>, AppError> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({})", quoted(table)))
        .map_err(recovery_db)?;
    collect_rows(stmt.query_map([], |row| row.get(1)).map_err(recovery_db)?)
}

fn insert_snapshot(
    conn: &Connection,
    table: &str,
    columns: &[String],
    values: Vec<crate::db::types::Value>,
) -> Result<(), AppError> {
    let names = columns
        .iter()
        .map(|column| quoted(column))
        .collect::<Vec<_>>()
        .join(",");
    let slots = vec!["?"; columns.len()].join(",");
    let changed = conn
        .execute(
            &format!("INSERT INTO {} ({names}) VALUES ({slots})", quoted(table)),
            params_from_iter(values),
        )
        .map_err(recovery_db)?;
    if changed != 1 {
        return Err(AppError::Internal(format!(
            "could not restore one {table} record"
        )));
    }
    Ok(())
}

// Aliases and their audit receipts are written in the same transaction, including
// by previous Router versions. Replay the committed receipt after a lost response.
fn applied_recovery(
    conn: &Connection,
    input: &ClientIdentityRecoveryRequest,
) -> Result<Option<serde_json::Value>, AppError> {
    let receipt = conn.query_row(
        "SELECT (SELECT l.payload_json FROM admin_audit_log l
            WHERE l.action='client_identity.recover' AND l.created_at=a.created_at
              AND json_extract(l.payload_json,'$.installationId')=a.installation_id
              AND json_extract(l.payload_json,'$.replacementInstallationId')=a.previous_installation_id)
         FROM client_identity_aliases a WHERE a.previous_installation_id=?1 AND a.installation_id=?2 AND a.public_key=?3",
        params![input.replacement_installation_id,input.original_installation_id,input.public_key],
        |row| row.get::<_, Option<String>>(0)
    ).optional().map_err(recovery_db)?;
    let Some(receipt) = receipt else {
        return Ok(None);
    };
    let raw = receipt.ok_or_else(|| {
        AppError::Internal("committed Client recovery is missing its audit receipt".into())
    })?;
    let mut result: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| AppError::Internal(format!("read Client recovery receipt: {e}")))?;
    if result["subdomain"] != input.subdomain
        || result["hostRevision"] != input.expected_host_revision
    {
        return Err(recovery_conflict(
            "CLIENT_IDENTITY_RECOVERY_HOST_CHANGED",
            "the previous recovery used a different reserved host or revision",
            serde_json::json!({}),
        ));
    }
    if input
        .expected_owner_email
        .as_ref()
        .is_some_and(|owner| result["ownerEmail"].as_str() != Some(owner.as_str()))
    {
        return Err(recovery_conflict(
            "CLIENT_IDENTITY_RECOVERY_ANCHOR_CHANGED",
            "the committed recovery used a different verified owner",
            serde_json::json!({}),
        ));
    }
    result["alreadyApplied"] = true.into();
    Ok(Some(result))
}

fn recovery_conflict(reason: &'static str, message: &str, details: serde_json::Value) -> AppError {
    AppError::coded_conflict(reason, message, details)
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
        || host.route_id != format!("client:{original}")
        || host.target_lane_id != *original
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
        || input
            .expected_owner_email
            .as_ref()
            .is_some_and(|owner| runtime.owner_email.as_ref() != Some(owner))
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
    let records = RecoveryRecords::inspect(conn, input)?;
    let offline_episode: i64 = conn.query_row(
        "SELECT max(0,
            COALESCE((SELECT offline_episode FROM client_identities WHERE installation_id=?2),0),
            COALESCE((SELECT max(offline_episode) FROM installation_notification_state WHERE installation_id IN (?1,?2)),0),
            COALESCE((SELECT max(episode) FROM client_notification_events WHERE installation_id IN (?1,?2)),0))",
        params![original,replacement], |row| row.get(0))
        .map_err(|error| AppError::Internal(format!("read recovered notification epoch: {error}")))?;
    let columns = table_columns(conn, "installations")?;
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
            "version":2,"original":original,"replacement":replacement,"publicKey":input.public_key,
            "subdomain":host.label,"revision":host.revision,"ownerEmail":runtime.owner_email,
            "ownerVerifiedAt":runtime.owner_verified_at,"runtimeHash":runtime_hash,"columns":columns,
            "hostLifecycle":host.lifecycle.as_str(),"hostCreatedAt":host.created_at,
            "offlineEpisode":offline_episode,
            "records":records.summary,"evidence":input.evidence,
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
    let mut result = serde_json::json!({"ok":true,"applied":input.apply,"installationId":original,
        "replacementInstallationId":replacement,"subdomain":host.label,"hostRevision":host.revision,
        "ownerEmail":runtime.owner_email,"planHash":hash,"offlineEpisode":offline_episode});
    result
        .as_object_mut()
        .expect("recovery result is an object")
        .extend(records.summary.clone());
    Ok(RecoveryPlan {
        records,
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
        mut input: ClientIdentityRecoveryRequest,
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
        input.subdomain = input.subdomain.trim().to_ascii_lowercase();
        let conn = self.conn.lock().await;
        let behavior = if input.apply {
            TransactionBehavior::Immediate
        } else {
            TransactionBehavior::Deferred
        };
        let tx = conn
            .transaction_with_behavior(behavior)
            .map_err(|e| AppError::Internal(format!("begin identity recovery: {e}")))?;
        if let Some(result) = applied_recovery(&tx, &input)? {
            return Ok(result);
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
        plan.records.retire_replacement(&tx, replacement)?;
        insert_snapshot(&tx, "installations", &plan.columns, plan.values)?;
        plan.records.restore_tasks(&tx)?;
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
