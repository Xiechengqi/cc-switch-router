//! Publication convergence and signed installation-level activation receipts.
use super::*;

pub(super) fn mark_target(conn: &Connection, revision: &str, now: i64) -> Result<(), AppError> {
    conn.execute("INSERT INTO share_value_pricing_publication(singleton,target_catalog_revision,updated_at_ms) VALUES(1,?1,?2)
        ON CONFLICT(singleton) DO UPDATE SET target_catalog_revision=excluded.target_catalog_revision,updated_at_ms=excluded.updated_at_ms
        WHERE target_catalog_revision<>excluded.target_catalog_revision",params![revision,now]).map_err(db_error)?;
    Ok(())
}
pub(super) fn publication_status(
    conn: &Connection,
    catalog_revision: &str,
    now_unix: i64,
) -> Result<serde_json::Value, AppError> {
    let row: Option<(String,Option<String>,i64)> = conn.query_row("SELECT target_catalog_revision,published_catalog_revision,updated_at_ms FROM share_value_pricing_publication WHERE singleton=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(db_error)?;
    Ok(match row {
        Some((target, published, at)) => {
            serde_json::json!({"state":if published.as_deref()==Some(target.as_str()) && !needs_retry(conn,catalog_revision,now_unix)? {"published"} else {"pending"},"updatedAtMs":at})
        }
        None => serde_json::json!({"state":"unknown"}),
    })
}
pub(super) fn apply_activation(
    conn: &Connection,
    installation: &str,
    revision: &str,
    signed_at: i64,
) -> Result<serde_json::Value, AppError> {
    let now = Utc::now().timestamp_millis();
    if !revision_acknowledged(conn, installation, revision)? {
        return Err(AppError::Conflict(
            "pricing revision was not acknowledged".into(),
        ));
    }
    let effective: i64 = conn
        .query_row(
            "SELECT effective_from_ms FROM share_value_pricing_snapshots WHERE revision=?1",
            params![revision],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    if effective > now {
        return Err(AppError::Conflict(
            "pricing revision is not effective yet".into(),
        ));
    }
    conn.execute("INSERT INTO share_value_pricing_activations VALUES(?1,?2,?3,?4)
        ON CONFLICT(installation_id) DO UPDATE SET revision=excluded.revision,reported_at_ms=excluded.reported_at_ms,confirmed_at_ms=excluded.confirmed_at_ms
        WHERE excluded.reported_at_ms>share_value_pricing_activations.reported_at_ms",params![installation,revision,signed_at,now]).map_err(db_error)?;
    Ok(serde_json::json!({"ok":true,"revision":revision}))
}
pub(super) fn installation_activation(
    conn: &Connection,
    installation: &str,
    published: Option<&str>,
    now: i64,
) -> Result<serde_json::Value, AppError> {
    let receipt: Option<(String,i64)> = conn.query_row("SELECT revision,confirmed_at_ms FROM share_value_pricing_activations WHERE installation_id=?1",params![installation],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(db_error)?;
    Ok(match receipt {
        Some((revision, at)) => {
            serde_json::json!({"activePricingRevision":revision,"confirmedAtMs":at,"current":published==Some(revision.as_str()) && at>=now-PROJECTION_FRESH_MS})
        }
        None => serde_json::json!({"current":false}),
    })
}
pub(super) fn delivery_health(
    conn: &Connection,
    published: Option<&str>,
    now: i64,
) -> Result<serde_json::Value, AppError> {
    let (expected,acknowledged,activated):(i64,i64,i64)=conn.query_row(
        "WITH targets AS (
          SELECT i.id,i.last_seen_at,CASE WHEN coalesce(c.resources,0)=1
            THEN (SELECT revision FROM share_value_resource_pricing_config WHERE singleton=1) ELSE ?1 END AS revision
          FROM installations i LEFT JOIN installation_value_capabilities c ON c.installation_id=i.id
          WHERE i.lifecycle='active' AND EXISTS(SELECT 1 FROM shares s WHERE s.installation_id=i.id)
        )
        SELECT count(*),coalesce(sum(EXISTS(SELECT 1 FROM share_value_pricing_acks a WHERE a.installation_id=i.id AND a.revision=i.revision)),0),
        coalesce(sum(EXISTS(SELECT 1 FROM share_value_pricing_activations a WHERE a.installation_id=i.id AND a.revision=i.revision AND a.confirmed_at_ms>=?2 AND i.last_seen_at>=?3)),0)
        FROM targets i",
        params![published,now-PROJECTION_FRESH_MS,DateTime::<Utc>::from_timestamp_millis(now-PROJECTION_FRESH_MS).map(|v|v.to_rfc3339()).unwrap_or_default()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(db_error)?;
    Ok(
        serde_json::json!({"expectedInstallations":expected,"acknowledgedInstallations":acknowledged,"activatedInstallations":activated,"unconfirmedInstallations":expected-activated}),
    )
}

impl AppStore {
    pub(crate) async fn value_tariff_publication(&self) -> Result<serde_json::Value, AppError> {
        let conn = self.conn.lock().await;
        let mut status = publication_status(
            &conn,
            self.pricing_catalog().revision(),
            Utc::now().timestamp(),
        )?;
        let meta = read_settings_meta(&conn)?;
        status["revision"] = serde_json::json!(meta.revision);
        status["effectiveFromMs"] = serde_json::json!(meta.effective_from_ms);
        Ok(status)
    }
    async fn value_tariff_needs_retry(&self) -> Result<bool, AppError> {
        let conn = self.conn.lock().await;
        needs_retry(
            &conn,
            self.pricing_catalog().revision(),
            Utc::now().timestamp(),
        )
    }
    pub(crate) async fn run_value_tariff_delivery(self) -> anyhow::Result<()> {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let retry = match self.value_tariff_needs_retry().await {
                Ok(retry) => retry,
                Err(error) => {
                    tracing::warn!(%error,"USD tariff publication retry state unavailable");
                    continue;
                }
            };
            if retry && let Err(error) = self.sync_value_tariff_from_catalog().await {
                tracing::warn!(%error,"USD tariff publication pending; background delivery will retry");
            }
        }
    }
}

fn needs_retry(conn: &Connection, catalog_revision: &str, now_unix: i64) -> Result<bool, AppError> {
    conn.query_row("SELECT coalesce((SELECT target_catalog_revision<>?2 OR target_catalog_revision<>coalesce(published_catalog_revision,'') OR EXISTS(
        SELECT 1 FROM model_price_catalog m WHERE (m.effective_from>p.updated_at_ms/1000 AND m.effective_from<=?1)
        OR (m.effective_to>p.updated_at_ms/1000 AND m.effective_to<=?1))
        FROM share_value_pricing_publication p WHERE singleton=1),1)",params![now_unix,catalog_revision],|r|r.get(0)).map_err(db_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("delivery_tests.rs");
}
