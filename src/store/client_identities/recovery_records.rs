//! Plan every dependent row before the installation's public key changes hands.
use super::*;
use crate::db::types::Value as SqlValue;
use serde_json::{Map, Value, json};

pub(super) struct RecoveryRecords {
    pub(super) summary: Map<String, Value>,
    task_columns: Vec<String>,
    tasks: Vec<Vec<SqlValue>>,
    ack_count: usize,
}

// The current schema has both foreign keys and logical business references.
// Keep the latter explicit, including records that outlive an installation.
const OBLIGATIONS: &str = "
SELECT kind, n FROM (
 SELECT 'installation_fences' kind, count(*) n FROM installation_fences WHERE public_key=?3 OR installation_id IN (?1,?2)
 UNION ALL SELECT 'client_identities', count(*) FROM client_identities WHERE installation_id=?1
 UNION ALL SELECT 'client_identity_aliases', count(*) FROM client_identity_aliases WHERE previous_installation_id IN (?1,?2) OR installation_id IN (?1,?2)
 UNION ALL SELECT 'client_subdomain_takeovers', count(*) FROM client_subdomain_takeovers WHERE target_installation_id IN (?1,?2) OR source_installation_id IN (?1,?2)
 UNION ALL SELECT 'public_hosts', count(*) FROM public_hosts WHERE installation_id=?2
 UNION ALL SELECT 'installation_client_tunnels', count(*) FROM installation_client_tunnels WHERE installation_id=?2
 UNION ALL SELECT 'shares', count(*) FROM shares WHERE installation_id=?2
 UNION ALL SELECT 'online_client_listings', count(*) FROM online_client_listings WHERE installation_id IN (?1,?2)
 UNION ALL SELECT 'online_client_leases', count(*) FROM online_client_leases WHERE installation_id IN (?1,?2) AND released_at IS NULL
 UNION ALL SELECT 'client_market_terminations', count(*) FROM client_market_terminations WHERE host_id IN (?1,?2) AND (status='finance_pending' OR cleanup_confirmed_at IS NULL)
 UNION ALL SELECT 'online_client_operations', count(*) FROM online_client_operations WHERE installation_id IN (?1,?2)
 UNION ALL SELECT 'share_value_owner_model_prices', count(*) FROM share_value_owner_model_prices WHERE installation_id=?2
 UNION ALL SELECT 'share_value_owner_model_price_history', count(*) FROM share_value_owner_model_price_history WHERE installation_id=?2
 UNION ALL SELECT 'share_market_listings', count(*) FROM share_market_listings WHERE installation_id=?2
 UNION ALL SELECT 'share_request_logs', count(*) FROM share_request_logs WHERE installation_id=?2
 UNION ALL SELECT 'image_generation_jobs', count(*) FROM image_generation_jobs WHERE installation_id=?2
 UNION ALL SELECT 'image_generation_request_logs', count(*) FROM image_generation_request_logs WHERE installation_id=?2
 UNION ALL SELECT 'share_edit_requests', count(*) FROM share_edit_requests WHERE installation_id=?2
 UNION ALL SELECT 'leases', count(*) FROM leases WHERE installation_id IN (?1,?2)
 UNION ALL SELECT 'chat_rooms', count(*) FROM chat_rooms WHERE installation_id=?2 AND status='active'
) WHERE n>0 ORDER BY kind";

impl RecoveryRecords {
    pub(super) fn inspect(
        conn: &Connection,
        input: &ClientIdentityRecoveryRequest,
    ) -> Result<Self, AppError> {
        let id = &input.replacement_installation_id;
        let mut stmt = conn.prepare(OBLIGATIONS).map_err(recovery_db)?;
        let rows = stmt.query_map(params![input.original_installation_id, id, input.public_key], |row| {
            let table: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok(json!({ "table": table, "count": count, "reason": "identity_or_business_reference" }))
        }).map_err(recovery_db)?;
        let mut blockers = collect_rows(rows)?;
        let mut records = Self {
            summary: Map::new(),
            task_columns: table_columns(conn, "installation_upgrade_tasks")?,
            tasks: Vec::new(),
            ack_count: 0,
        };
        records.inspect_tasks(conn, input, &mut blockers)?;
        records.inspect_pricing(conn, id, &mut blockers)?;

        let has_obligations = !blockers.is_empty();
        let mut retired = Vec::new();
        let mut blocking_refs = Vec::new();
        for reference in referencing_rows(conn, id)? {
            // Match the exact FK, not just its table. A new FK in a known table
            // must not silently inherit permission to cascade or be discarded.
            let table = reference["table"].as_str().unwrap_or_default();
            let typical = reference["parentTable"] == "installations"
                && reference["column"] == "installation_id"
                && reference["parentColumn"] == "id";
            match (typical, table, reference["onDelete"].as_str()) {
                (true, "installation_upgrade_tasks", Some("CASCADE"))
                | (true, "share_value_pricing_acks", Some("NO ACTION")) => {} // classified row by row
                (
                    true,
                    "installation_notification_state"
                    | "installation_setup_completions"
                    | "installation_value_capabilities",
                    Some("CASCADE"),
                ) => retired.push(reference),
                (true, "share_value_pricing_activations", Some("CASCADE")) => {
                    if !records.summary["pricingActivationToRefresh"].is_null() {
                        retired.push(reference);
                    }
                }
                _ => blocking_refs.push(reference),
            }
        }
        let retired_count: u64 = retired.iter().filter_map(|row| row["count"].as_u64()).sum();
        records.summary.insert(
            "recordCounts".into(),
            json!({
                "preservedUpgradeTasks": records.tasks.len(),
                "pricingReceiptsToRenew": records.ack_count,
                "retiredRuntimeReferences": retired_count
            }),
        );
        records
            .summary
            .insert("retiredRuntimeReferences".into(), json!(retired));
        if !blockers.is_empty() || !blocking_refs.is_empty() {
            let code = if has_obligations {
                "CLIENT_IDENTITY_RECOVERY_OBLIGATIONS"
            } else {
                "CLIENT_IDENTITY_RECOVERY_REFERENCES"
            };
            for reference in &blocking_refs {
                let mut blocker = reference.clone();
                blocker["reason"] = json!("protected_foreign_key");
                blockers.push(blocker);
            }
            records.summary.insert("blockers".into(), json!(blockers));
            records
                .summary
                .insert("references".into(), json!(blocking_refs));
            return Err(recovery_conflict(
                code,
                "Client identity recovery requires the listed records to be resolved",
                Value::Object(records.summary),
            ));
        }
        Ok(records)
    }

    fn inspect_tasks(
        &mut self,
        conn: &Connection,
        input: &ClientIdentityRecoveryRequest,
        blockers: &mut Vec<Value>,
    ) -> Result<(), AppError> {
        let columns = &self.task_columns;
        let selected = columns
            .iter()
            .map(|col| quoted(col))
            .collect::<Vec<_>>()
            .join(",");
        let id_column = columns
            .iter()
            .position(|col| col == "installation_id")
            .ok_or_else(|| AppError::Internal("upgrade history has no installation_id".into()))?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT task_id,status,restart_pending,lease_id,target_commit_id,{selected}
            FROM installation_upgrade_tasks WHERE installation_id=?1 ORDER BY task_id"
            ))
            .map_err(recovery_db)?;
        let rows = stmt
            .query_map(params![input.replacement_installation_id], |r| {
                let values = (0..columns.len())
                    .map(|n| r.get::<_, SqlValue>(5 + n))
                    .collect::<crate::db::Result<Vec<_>>>()?;
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    values,
                ))
            })
            .map_err(recovery_db)?;
        let mut preserved = Vec::new();
        for row in rows {
            let (task, status, restart_pending, lease, target, mut values) =
                row.map_err(recovery_db)?;
            let lease_scoped = lease.is_some() || task.starts_with("lease-");
            if !matches!(status.as_str(), "success" | "failed") || restart_pending || lease_scoped {
                blockers.push(json!({
                    "table": "installation_upgrade_tasks",
                    "taskId": task, "status": status,
                    "restartPending": restart_pending, "leaseScoped": lease_scoped,
                    "reason": "upgrade_not_portable"
                }));
                continue;
            }
            // Use every column (including future columns) except the three
            // timestamps advanced by re-reporting an identical terminal task.
            let mut digest = Sha256::new();
            for (name, value) in columns.iter().zip(&values) {
                if !matches!(
                    name.as_str(),
                    "reported_at" | "updated_at" | "client_reported_at_ms"
                ) {
                    digest.update(name.as_bytes());
                    digest.update([0]);
                    digest.update(format!("{value:?}").as_bytes());
                    digest.update([0]);
                }
            }
            preserved.push(json!({
                "taskId": task, "status": status, "targetCommitId": target,
                "contentHash": hex::encode(digest.finalize())
            }));
            values[id_column] = SqlValue::Text(input.original_installation_id.clone());
            self.tasks.push(values);
        }
        self.summary
            .insert("preservedUpgradeTasks".into(), json!(preserved));
        Ok(())
    }

    fn inspect_pricing(
        &mut self,
        conn: &Connection,
        id: &str,
        blockers: &mut Vec<Value>,
    ) -> Result<(), AppError> {
        // The same content revision can appear in both global and owner pricing.
        // The installation-specific issuance/tariff tables are independently
        // protected FKs. Use the same global provenance as revision_issued_to.
        let mut stmt = conn.prepare("WITH delivery(kind,revision,ack_ms) AS (
               SELECT 'ack',revision,acknowledged_at_ms FROM share_value_pricing_acks WHERE installation_id=?1
               UNION ALL SELECT 'activation',revision,NULL FROM share_value_pricing_activations WHERE installation_id=?1
             )
             SELECT d.kind,d.revision,d.ack_ms,s.published_by,
               EXISTS(SELECT 1 FROM share_value_pricing_config WHERE revision=d.revision),
               EXISTS(SELECT 1 FROM share_value_resource_pricing_config WHERE revision=d.revision),
               EXISTS(SELECT 1 FROM share_value_resource_pricing_revisions WHERE revision=d.revision)
                 OR coalesce(json_extract(s.payload_json,'$.schemaVersion')=2,0)
             FROM delivery d LEFT JOIN share_value_pricing_snapshots s ON s.revision=d.revision
             ORDER BY d.kind,d.revision").map_err(recovery_db)?;
        let mut acks = Vec::new();
        let mut activation = Value::Null;
        for row in stmt
            .query_map(params![id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, bool>(4)?,
                    r.get::<_, bool>(5)?,
                    r.get::<_, bool>(6)?,
                ))
            })
            .map_err(recovery_db)?
        {
            let (kind, revision, at, publisher, text_current, resource_current, resource) =
                row.map_err(recovery_db)?;
            if publisher.is_none()
                || (publisher.as_deref() == Some("owner-prices")
                    && !text_current
                    && !resource_current)
            {
                let table = if kind == "ack" {
                    "share_value_pricing_acks"
                } else {
                    "share_value_pricing_activations"
                };
                blockers.push(json!({ "table": table, "revision": revision, "reason": "pricing_is_not_global" }));
                continue;
            }
            let tariff_kind = if resource || resource_current {
                "resource"
            } else {
                "text"
            };
            let mut item = json!({
                "revision": revision, "tariffKind": tariff_kind, "publishedBy": publisher,
                "current": text_current || resource_current
            });
            if let Some(at) = at {
                item["acknowledgedAtMs"] = at.into();
                acks.push(item);
            } else {
                activation = item;
            }
        }
        self.ack_count = acks.len();
        self.summary
            .insert("pricingReceiptsToRenew".into(), json!(acks));
        self.summary
            .insert("pricingActivationToRefresh".into(), activation);
        Ok(())
    }

    pub(super) fn retire_replacement(
        &self,
        conn: &Connection,
        replacement: &str,
    ) -> Result<(), AppError> {
        // The remaining cascades were enumerated in this same write transaction.
        // Ordinary offline GC also archives chat and deletes Share/business data.
        let deleted = conn
            .execute(
                "DELETE FROM share_value_pricing_acks WHERE installation_id=?1",
                params![replacement],
            )
            .map_err(recovery_db)?;
        if deleted != self.ack_count {
            return Err(AppError::Internal(
                "unexpected pricing receipt count during identity recovery".into(),
            ));
        }
        if conn
            .execute(
                "DELETE FROM installations WHERE id=?1",
                params![replacement],
            )
            .map_err(recovery_db)?
            != 1
        {
            return Err(AppError::Internal(
                "replacement installation disappeared during recovery".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn restore_tasks(self, conn: &Connection) -> Result<(), AppError> {
        for values in self.tasks {
            insert_snapshot(
                conn,
                "installation_upgrade_tasks",
                &self.task_columns,
                values,
            )?;
        }
        Ok(())
    }
}

// A future table can reference a terminal task (or an ACK) rather than the
// installation itself. The parent row will be removed while restoring its ID.
// Detect populated child FKs before either an explicit delete or a cascade.
fn referencing_rows(conn: &Connection, id: &str) -> Result<Vec<Value>, AppError> {
    let mut stmt = conn.prepare(r#"SELECT s.name, f.id, lower(f."table"), f.on_delete, f."from",
           coalesce(f."to", (SELECT p.name FROM pragma_table_info(f."table") AS p WHERE p.pk=f.seq+1))
        FROM sqlite_master s JOIN pragma_foreign_key_list(s.name) AS f
        WHERE s.type='table' AND lower(f."table") IN (
          'installations','installation_upgrade_tasks','share_value_pricing_acks',
          'installation_notification_state','installation_setup_completions',
          'installation_value_capabilities','share_value_pricing_activations')
        ORDER BY s.name,f.id,f.seq"#).map_err(recovery_db)?;
    // Group column pairs: two individual matches on a composite FK do not
    // establish that the child references the same parent row.
    let mut keys = std::collections::BTreeMap::<_, Vec<(String, String)>>::new();
    for row in stmt
        .query_map([], |r| {
            Ok((
                (
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ),
                (r.get::<_, String>(4)?, r.get::<_, String>(5)?),
            ))
        })
        .map_err(recovery_db)?
    {
        let (fk, columns) = row.map_err(recovery_db)?;
        keys.entry(fk).or_default().push(columns);
    }
    let mut references = Vec::new();
    for ((child, _, parent, action), columns) in keys {
        let predicate = columns
            .iter()
            .map(|(c, p)| format!("pa.{}=ch.{}", quoted(p), quoted(c)))
            .collect::<Vec<_>>()
            .join(" AND ");
        let scope = if parent == "installations" {
            "id"
        } else {
            "installation_id"
        };
        let sql = format!(
            "SELECT count(*) FROM {} pa JOIN {} ch ON {predicate} WHERE pa.{scope}=?1",
            quoted(&parent),
            quoted(&child)
        );
        let count: i64 = conn
            .query_row(&sql, params![id], |r| r.get(0))
            .map_err(recovery_db)?;
        if count > 0 {
            let (from, to) = columns.into_iter().unzip::<_, _, Vec<_>, Vec<_>>();
            references.push(json!({
                "table": child, "column": from.join(","), "parentTable": parent,
                "parentColumn": to.join(","), "onDelete": action, "count": count
            }));
        }
    }
    Ok(references)
}
