//! Independent v1/v2 publication. A signed fetch is the installation's
//! explicit declaration that it understands resource tariffs.
use super::*;
use crate::share_value_pricing::{
    ContextInputBasis, ExecutionModelPrice, RESOURCE_TARIFF_POLICY, validate_resource_rates,
};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ResourcePriceSettings {
    pub models: BTreeMap<String, BTreeMap<String, String>>,
    pub exact_aliases: BTreeMap<String, String>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ResourcePriceEdit {
    pub expected_revision: u64,
    pub settings: ResourcePriceSettings,
}
pub(super) fn resource_settings(
    conn: &Connection,
) -> Result<(u64, ResourcePriceSettings), AppError> {
    let row: Option<(i64, String)> = conn
        .query_row(
            "SELECT revision,payload_json FROM resource_price_catalog WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db_error)?;
    match row {
        Some((revision, json)) => Ok((
            u64::try_from(revision).map_err(db_error)?,
            serde_json::from_str(&json).map_err(db_error)?,
        )),
        None => Ok((0, ResourcePriceSettings::default())),
    }
}
fn overlay(
    snapshot: &mut PricingSnapshot,
    settings: &ResourcePriceSettings,
    resources: bool,
) -> Result<(), AppError> {
    if settings.models.len() > 10_000 || settings.exact_aliases.len() > 40_000 {
        return Err(AppError::BadRequest(
            "too many model prices or mappings".into(),
        ));
    }
    if resources {
        snapshot.schema_version = 2;
        snapshot.tariff_policy_id = RESOURCE_TARIFF_POLICY.into();
        for (model, rates) in &settings.models {
            validate_resource_rates(rates).map_err(|e| AppError::BadRequest(e.to_string()))?;
            if rates.is_empty() {
                return Err(AppError::BadRequest(
                    "resource model requires an explicit price".into(),
                ));
            }
            snapshot
                .models
                .entry(model.clone())
                .or_insert_with(|| ExecutionModelPrice {
                    context_input_basis: ContextInputBasis::RawInput,
                    long_context_threshold: None,
                    long_context_inclusive: false,
                    rates: BTreeMap::new(),
                    resources: BTreeMap::new(),
                })
                .resources = rates.clone();
        }
    }
    for (alias, target) in &settings.exact_aliases {
        if !snapshot.models.contains_key(target) {
            if resources {
                return Err(AppError::BadRequest(format!(
                    "pricing mapping target is missing: {target}"
                )));
            }
            continue;
        }
        if snapshot.models.contains_key(alias) {
            return Err(AppError::BadRequest(
                "a canonical model cannot be replaced by an alias".into(),
            ));
        }
        snapshot.exact_aliases.insert(alias.clone(), target.clone());
    }
    snapshot
        .canonical_json()
        .map(|_| ())
        .map_err(|e| AppError::BadRequest(e.to_string()))
}
pub(super) fn overlay_text_mappings(
    snapshot: &mut PricingSnapshot,
    settings: &ResourcePriceSettings,
) -> Result<(), AppError> {
    overlay(snapshot, settings, false)
}

pub(super) fn publish(conn: &Connection, now: i64) -> Result<(), AppError> {
    let Some((_, _, base_effective, json)) = read_settings_raw(conn)? else {
        return Ok(());
    };
    let mut snapshot: PricingSnapshot = serde_json::from_str(&json).map_err(db_error)?;
    let (_, settings) = resource_settings(conn)?;
    overlay(&mut snapshot, &settings, true)?;
    let json = snapshot.canonical_json().map_err(db_error)?;
    let revision = PricingSnapshot::revision_of_canonical_json(&json);
    let current = read_resource_settings_meta(conn)?;
    if current.revision.as_deref() == Some(&revision) {
        return Ok(());
    }
    let effective = base_effective.max(if current.revision.is_some() {
        now + PUBLISH_MIN_LEAD_MS
    } else {
        now
    });
    conn.execute("INSERT INTO share_value_pricing_snapshots VALUES(?1,?2,?3,'resource-prices',?4)
        ON CONFLICT(revision) DO UPDATE SET payload_json=excluded.payload_json WHERE share_value_pricing_snapshots.payload_json='{}'",params![revision,json,effective,now]).map_err(db_error)?;
    conn.execute(
        "INSERT OR IGNORE INTO share_value_resource_pricing_revisions(revision) VALUES(?1)",
        params![revision],
    )
    .map_err(db_error)?;
    conn.execute(
        "INSERT INTO share_value_resource_pricing_config VALUES(1,?1,?2)
        ON CONFLICT(singleton) DO UPDATE SET revision=excluded.revision,version=excluded.version",
        params![
            revision,
            current
                .version
                .checked_add(1)
                .ok_or_else(|| db_error("resource price version exhausted"))?
        ],
    )
    .map_err(db_error)?;
    Ok(())
}
pub(super) fn read_resource_settings_meta(conn: &Connection) -> Result<SettingsMeta, AppError> {
    let row = read_resource_settings_raw(conn)?;
    Ok(match row {
        Some((version, revision, at, _)) => SettingsMeta {
            version,
            revision: Some(revision),
            effective_from_ms: Some(at),
        },
        None => SettingsMeta {
            version: 0,
            revision: None,
            effective_from_ms: None,
        },
    })
}
pub(super) fn read_resource_settings_raw(conn: &Connection) -> Result<RawSettings, AppError> {
    conn.query_row("SELECT c.version,s.revision,s.effective_from_ms,s.payload_json FROM share_value_resource_pricing_config c JOIN share_value_pricing_snapshots s ON s.revision=c.revision WHERE c.singleton=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(db_error)
}
pub(super) fn installation_supports_resources(
    conn: &Connection,
    id: &str,
) -> Result<bool, AppError> {
    Ok(conn
        .query_row(
            "SELECT resources FROM installation_value_capabilities WHERE installation_id=?1",
            params![id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?
        .unwrap_or(false))
}
pub(super) fn installation_meta(conn: &Connection, id: &str) -> Result<SettingsMeta, AppError> {
    if installation_supports_resources(conn, id)? {
        read_resource_settings_meta(conn)
    } else {
        read_settings_meta(conn)
    }
}
impl AppStore {
    pub(crate) async fn resource_price_settings(&self) -> Result<Value, AppError> {
        let conn = self.conn.lock().await;
        let (revision, settings) = resource_settings(&conn)?;
        let tariff = read_resource_settings_meta(&conn)?;
        Ok(
            serde_json::json!({"revision":revision,"settings":settings,"pricingRevision":tariff.revision,"effectiveFromMs":tariff.effective_from_ms}),
        )
    }
    pub(crate) async fn edit_resource_prices(
        &self,
        input: ResourcePriceEdit,
        actor: &str,
    ) -> Result<Value, AppError> {
        let json = serde_json::to_string(&input.settings).map_err(db_error)?;
        if json.len() > 1024 * 1024 {
            return Err(AppError::BadRequest(
                "resource price document is too large".into(),
            ));
        }
        // Derive from the authoritative catalog, never from a previous
        // overlay. Removing or retargeting an alias must be atomic in v1/v2.
        let catalog = self.pricing_catalog();
        if catalog.is_empty() {
            return Err(AppError::Conflict("publish text model prices first".into()));
        }
        let catalog_revision = catalog.revision().to_string();
        let now = Utc::now().timestamp_millis();
        let mut text = catalog.execution_snapshot(now / 1000).map_err(db_error)?;
        overlay_text_mappings(&mut text, &input.settings)?;
        let text_json = text.canonical_json().map_err(db_error)?;
        let text_revision = PricingSnapshot::revision_of_canonical_json(&text_json);
        let mut resources = text;
        overlay(&mut resources, &input.settings, true)?;
        {
            let conn = self.conn.lock().await;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(db_error)?;
            if self.pricing_catalog().revision() != catalog_revision {
                return Err(AppError::Conflict(
                    "model prices changed; reload before saving".into(),
                ));
            }
            let (revision, _) = resource_settings(&tx)?;
            if revision != input.expected_revision {
                return Err(AppError::Conflict(
                    "resource prices changed; reload before saving".into(),
                ));
            }
            let next = revision
                .checked_add(1)
                .ok_or_else(|| db_error("price revision exhausted"))?;
            let current = read_settings_meta(&tx)?;
            if current.revision.as_deref() != Some(text_revision.as_str()) {
                let effective = if current.revision.is_some() {
                    now + PUBLISH_MIN_LEAD_MS
                } else {
                    now
                };
                tx.execute("INSERT INTO share_value_pricing_snapshots VALUES(?1,?2,?3,'resource-mappings',?4) ON CONFLICT(revision) DO UPDATE SET payload_json=excluded.payload_json WHERE share_value_pricing_snapshots.payload_json='{}'",params![text_revision,text_json,effective,now]).map_err(db_error)?;
                tx.execute("INSERT INTO share_value_pricing_config VALUES(1,?1,?2) ON CONFLICT(singleton) DO UPDATE SET revision=excluded.revision,version=excluded.version",params![text_revision,current.version.checked_add(1).ok_or_else(|| db_error("text price version exhausted"))?]).map_err(db_error)?;
            }
            tx.execute("INSERT INTO resource_price_catalog VALUES(1,?1,?2,?3) ON CONFLICT(singleton) DO UPDATE SET revision=excluded.revision,payload_json=excluded.payload_json,updated_at_ms=excluded.updated_at_ms",params![next,json,now]).map_err(db_error)?;
            tx.execute(
                "INSERT INTO resource_price_history VALUES(?1,?2,?3,?4)",
                params![next, json, actor, now],
            )
            .map_err(db_error)?;
            publish(&tx, now)?;
            // Alias changes also affect text-only installations; force their
            // normal publication worker to derive v1 again.
            tx.execute("UPDATE share_value_pricing_publication SET published_catalog_revision=NULL WHERE singleton=1",[]).map_err(db_error)?;
            tx.commit().map_err(db_error)?;
        }
        self.sync_value_tariff_from_catalog().await?;
        self.resource_price_settings().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_fixture() -> PricingSnapshot {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/share-value-pricing-v1.json"
        ))
        .unwrap();
        let snapshot: PricingSnapshot =
            serde_json::from_value(fixture["snapshot"].clone()).unwrap();
        assert_eq!(
            snapshot.revision().unwrap(),
            fixture["revision"].as_str().unwrap()
        );
        assert!(!snapshot.canonical_json().unwrap().contains("\"resources\""));
        snapshot
    }

    #[test]
    fn signed_capability_selects_independent_tariffs_and_stale_fetch_cannot_downgrade() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::apply(&conn).unwrap();
        conn.pragma_update(None, "foreign_keys", "OFF").unwrap();
        let snapshot = text_fixture();
        let json = snapshot.canonical_json().unwrap();
        let revision = snapshot.revision().unwrap();
        conn.execute(
            "INSERT INTO share_value_pricing_snapshots VALUES(?1,?2,1,'fixture',1)",
            params![revision, json],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO share_value_pricing_config VALUES(1,?1,1)",
            params![revision],
        )
        .unwrap();
        let settings = ResourcePriceSettings {
            models: BTreeMap::from([(
                "text-model".into(),
                BTreeMap::from([("tool.web_search".into(), "1000000".into())]),
            )]),
            ..Default::default()
        };
        conn.execute(
            "INSERT INTO resource_price_catalog VALUES(1,1,?1,1)",
            params![serde_json::to_string(&settings).unwrap()],
        )
        .unwrap();
        publish(&conn, 1).unwrap();
        let fetch = |resources: bool, at| {
            let operation = ValueOperation::Fetch(FetchPayload {
                known_revision: None,
                supported_tariff_policies: if resources {
                    vec![RESOURCE_TARIFF_POLICY.into()]
                } else {
                    vec![]
                },
            });
            match apply_value_operation(&conn, "installation", operation, at).unwrap() {
                ValueOutcome::Manifest { raw, .. } => {
                    serde_json::from_str::<PricingSnapshot>(&raw.unwrap().3).unwrap()
                }
                _ => panic!("first fetch requires a complete manifest"),
            }
        };
        let resource = fetch(true, 100);
        assert_eq!(resource.schema_version, 2);
        assert_ne!(resource.revision().unwrap(), revision);
        let resource_revision = resource.revision().unwrap();
        for activate in [false, true] {
            let operation = |storage_schema| {
                let ack = AckPayload {
                    revision: resource_revision.clone(),
                    storage_schema,
                };
                if activate {
                    ValueOperation::Active(ack)
                } else {
                    ValueOperation::Ack(ack)
                }
            };
            assert!(matches!(
                apply_value_operation(&conn, "installation", operation(3), 101),
                Err(AppError::Conflict(_))
            ));
            apply_value_operation(&conn, "installation", operation(5), 101).unwrap();
        }
        assert_eq!(fetch(false, 90).schema_version, 2);
        assert_eq!(fetch(false, 110).revision().unwrap(), revision);
        assert!(matches!(
            apply_value_operation(
                &conn,
                "installation",
                ValueOperation::Ack(AckPayload {
                    revision: resource_revision,
                    storage_schema: 5,
                }),
                111
            ),
            Err(AppError::Conflict(_))
        ));
        // Older Servers continue acknowledging the independent v1 tariff.
        apply_value_operation(
            &conn,
            "installation",
            ValueOperation::Ack(AckPayload {
                revision: revision.clone(),
                storage_schema: 3,
            }),
            111,
        )
        .unwrap();
        assert_eq!(
            read_settings_meta(&conn).unwrap().revision.as_deref(),
            Some(revision.as_str())
        );
        assert!(
            resource.models["text-model"]
                .resources
                .contains_key("tool.web_search")
        );
        // Historical payload retention must not erase the v2 storage gate.
        let mut replacement = settings;
        replacement
            .models
            .get_mut("text-model")
            .unwrap()
            .insert("tool.web_search".into(), "2000000".into());
        conn.execute(
            "UPDATE resource_price_catalog SET payload_json=?1 WHERE singleton=1",
            params![serde_json::to_string(&replacement).unwrap()],
        )
        .unwrap();
        publish(&conn, 2).unwrap();
        assert_eq!(fetch(true, 120).schema_version, 2);
        let archived = resource.revision().unwrap();
        conn.execute(
            "UPDATE share_value_pricing_snapshots SET payload_json='{}' WHERE revision=?1",
            params![archived],
        )
        .unwrap();
        for active in [false, true] {
            let operation = |storage_schema| {
                let ack = AckPayload {
                    revision: archived.clone(),
                    storage_schema,
                };
                if active {
                    ValueOperation::Active(ack)
                } else {
                    ValueOperation::Ack(ack)
                }
            };
            assert!(matches!(
                apply_value_operation(&conn, "installation", operation(3), 121),
                Err(AppError::Conflict(_))
            ));
            apply_value_operation(&conn, "installation", operation(5), 121).unwrap();
        }
    }

    #[tokio::test]
    async fn resource_price_cas_keeps_drafts_and_alias_removal_publishes_both_policies() {
        let store = AppStore::new_in_memory_for_tests().unwrap();
        store.reload_pricing_catalog().await.unwrap();
        let initial = store.resource_price_settings().await.unwrap();
        let target = {
            let conn = store.conn.lock().await;
            let snapshot: PricingSnapshot =
                serde_json::from_str(&read_settings_raw(&conn).unwrap().unwrap().3).unwrap();
            snapshot.models.keys().next().unwrap().clone()
        };
        let settings = ResourcePriceSettings {
            models: BTreeMap::from([(
                target.clone(),
                BTreeMap::from([("tool.web_search".into(), "0".into())]),
            )]),
            exact_aliases: BTreeMap::from([("private-runtime-model".into(), target)]),
        };
        let first = store
            .edit_resource_prices(
                ResourcePriceEdit {
                    expected_revision: initial["revision"].as_u64().unwrap(),
                    settings: settings.clone(),
                },
                "owner",
            )
            .await
            .unwrap();
        assert!(matches!(
            store
                .edit_resource_prices(
                    ResourcePriceEdit {
                        expected_revision: 0,
                        settings: settings.clone()
                    },
                    "stale"
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        assert_eq!(
            store.resource_price_settings().await.unwrap()["settings"],
            first["settings"]
        );
        let mut removed = settings;
        removed.exact_aliases.clear();
        store
            .edit_resource_prices(
                ResourcePriceEdit {
                    expected_revision: 1,
                    settings: removed,
                },
                "owner",
            )
            .await
            .unwrap();
        let conn = store.conn.lock().await;
        for raw in [
            read_settings_raw(&conn).unwrap(),
            read_resource_settings_raw(&conn).unwrap(),
        ] {
            let snapshot: PricingSnapshot = serde_json::from_str(&raw.unwrap().3).unwrap();
            assert!(!snapshot.exact_aliases.contains_key("private-runtime-model"));
        }
        assert_eq!(
            conn.query_row("SELECT count(*) FROM resource_price_history", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
}
