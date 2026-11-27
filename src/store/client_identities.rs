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

#[derive(Debug, Deserialize)]
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
}

impl AppStore {
    /// Preview is the default. Only the admin API may call this operation.
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
        let original = &input.original_installation_id;
        let replacement = &input.replacement_installation_id;
        let alias:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM client_identity_aliases WHERE previous_installation_id=?1 AND installation_id=?2 AND public_key=?3)",params![replacement,original,input.public_key],|r|r.get(0))
            .map_err(|e|AppError::Internal(e.to_string()))?;
        if alias {
            return Ok(
                serde_json::json!({"ok":true,"applied":true,"alreadyApplied":true,"installationId":original}),
            );
        }
        let host = crate::public_hosts::get_by_label(&tx, &input.subdomain)
            .map_err(map_public_host_error)?
            .ok_or_else(|| AppError::NotFound("reserved Client host not found".into()))?;
        if host.kind != PublicHostKind::Client
            || host.subject_id != *original
            || host.installation_id.as_deref() != Some(original.as_str())
            || host.revision != input.expected_host_revision
            || host.lifecycle == crate::public_hosts::PublicHostLifecycle::Tombstoned
        {
            return Err(AppError::Conflict(
                "reserved Client host changed or belongs to another identity".into(),
            ));
        }
        if get_installation(&tx, original)?.is_some() {
            return Err(AppError::Conflict(
                "original runtime already exists; manual conflict review required".into(),
            ));
        }
        let runtime = get_installation(&tx, replacement)?
            .ok_or_else(|| AppError::NotFound("replacement runtime not found".into()))?;
        if runtime.public_key != input.public_key || runtime.owner_verified_at.is_none() {
            return Err(AppError::Conflict(
                "replacement public key/verified owner does not match recovery evidence".into(),
            ));
        }
        let blocked:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM installation_fences WHERE public_key=?3 OR installation_id IN (?1,?2))
            OR EXISTS(SELECT 1 FROM client_identities WHERE installation_id=?1 AND (public_key!=?3 OR lifecycle!='active'))
            OR EXISTS(SELECT 1 FROM public_hosts WHERE installation_id=?2)
            OR EXISTS(SELECT 1 FROM shares WHERE installation_id=?2)
            OR EXISTS(SELECT 1 FROM online_client_listings WHERE installation_id IN (?1,?2))
            OR EXISTS(SELECT 1 FROM leases WHERE installation_id=?2)", params![original,replacement,input.public_key],|r|r.get(0))
            .map_err(|e|AppError::Internal(format!("check recovery obligations: {e}")))?;
        if blocked {
            return Err(AppError::Conflict(
                "identity recovery is blocked by fencing, claims or business references".into(),
            ));
        }
        let result = serde_json::json!({"ok":true,"applied":input.apply,"installationId":original,"replacementInstallationId":replacement,"subdomain":host.label,"hostRevision":host.revision,"ownerEmail":runtime.owner_email});
        if !input.apply {
            return Ok(result);
        }
        let now = Utc::now().to_rfc3339();
        let moved=tx.execute("UPDATE client_identities SET installation_id=?1,created_at=?4 WHERE installation_id=?2 AND public_key=?3 AND lifecycle='active'",params![original,replacement,input.public_key,host.created_at.to_rfc3339()])
            .map_err(|e|AppError::Internal(format!("restore identity anchor: {e}")))?;
        if moved != 1 {
            return Err(AppError::Conflict(
                "replacement identity anchor changed".into(),
            ));
        }
        // Copy every current column, including future optional metadata. No raw
        // PK update: dependent rows stay valid until their explicit retirement.
        let columns = {
            let mut s = tx
                .prepare("PRAGMA table_info(installations)")
                .map_err(|e| AppError::Internal(e.to_string()))?;
            collect_rows(
                s.query_map([], |r| r.get::<_, String>(1))
                    .map_err(|e| AppError::Internal(e.to_string()))?,
            )?
        };
        let names = columns
            .iter()
            .map(|name| format!("\"{}\"", name.replace('"', "\"\"")))
            .collect::<Vec<_>>();
        let values = columns
            .iter()
            .zip(&names)
            .map(|(name, quoted)| {
                if name == "id" {
                    "?1".to_string()
                } else if name == "client_activated_at" {
                    "COALESCE(client_activated_at,created_at)".to_string()
                } else {
                    quoted.clone()
                }
            })
            .collect::<Vec<_>>();
        tx.execute(
            &format!(
                "INSERT INTO installations ({}) SELECT {} FROM installations WHERE id=?2",
                names.join(","),
                values.join(",")
            ),
            params![original, replacement],
        )
        .map_err(|e| AppError::Internal(format!("restore original runtime: {e}")))?;
        purge_installation_data_tx(&tx, replacement)?;
        tx.execute(
            "UPDATE installations SET created_at=?2,client_activated_at=?2 WHERE id=?1",
            params![original, host.created_at.to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("restore original identity age: {e}")))?;
        tx.execute(
            "INSERT INTO client_identity_aliases VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                replacement,
                original,
                input.public_key,
                input.evidence,
                actor,
                now
            ],
        )
        .map_err(|e| AppError::Internal(format!("audit identity recovery: {e}")))?;
        tx.execute("INSERT INTO admin_audit_log(id,actor_email,action,payload_json,created_at) VALUES(?1,?2,'client_identity.recover',?3,?4)",params![Uuid::new_v4().to_string(),actor,result.to_string(),now])
            .map_err(|e|AppError::Internal(format!("record identity recovery audit: {e}")))?;
        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit identity recovery: {e}")))?;
        Ok(result)
    }
}
