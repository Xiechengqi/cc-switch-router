//! Installation-bound executable tariffs and private quota projections.
use super::*;
use crate::share_value_pricing::{
    PricingManifest, PricingSnapshot, ValueQuotaStatus, unsigned_decimal,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SignedValueRequest {
    pub protocol_epoch: String,
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    #[serde(flatten)]
    pub payload: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_value_projection_rejects_old_windows_epochs_and_fabricated_totals() {
        let now = DateTime::parse_from_rfc3339("2026-10-30T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let (start, end) = token_period_window(
            &ShareUserPolicy {
                token_period: ShareTokenPeriod::Day,
                ..Default::default()
            },
            now,
        )
        .unwrap();
        let start = start.unwrap().timestamp_millis();
        let end = end.unwrap().timestamp_millis();
        let grant = ShareUserGrant {
            email: "user@example.test".into(),
            active: true,
            policy: ShareUserPolicy {
                token_period: ShareTokenPeriod::Day,
                value_quota: Some(crate::share_value_pricing::ValueQuota {
                    limit_usd_micros: "1000000".into(),
                    tariff_policy_id: crate::share_value_pricing::TARIFF_POLICY.into(),
                }),
                ..Default::default()
            },
            value_metering: Some(crate::share_value_pricing::ValueMeteringBinding {
                quota_policy_epoch: "epoch".into(),
                value_metered_from_ms: 0,
                effective_from_ms: 0,
                pricing_revision: "revision".into(),
            }),
            ..Default::default()
        };
        let status = ValueQuotaStatus {
            email: grant.email.clone(),
            policy_epoch: "epoch".into(),
            window_id: format!("Day:{start}:{end}"),
            window_starts_at_ms: start,
            resets_at_ms: Some(end),
            metered_from_ms: 0,
            active_pricing_revision: "revision".into(),
            usage_pricing_revisions_digest: "digest".into(),
            usage_watermark: 1,
            limit_usd_micros: Some("1000000".into()),
            used_picos: "150".into(),
            observed_picos: "100".into(),
            manual_offset_picos: "50".into(),
            pending_risk_picos: "0".into(),
            metering_status: "observed".into(),
            synced_at_ms: now.timestamp_millis(),
        };
        assert!(current_projection(&grant, &status, now).is_ok());
        assert!(current_projection(&grant, &status, now + Duration::days(1)).is_err());
        assert!(
            current_projection(
                &grant,
                &ValueQuotaStatus {
                    policy_epoch: "old".into(),
                    ..status.clone()
                },
                now
            )
            .is_err()
        );
        assert!(
            current_projection(
                &grant,
                &ValueQuotaStatus {
                    used_picos: "0".into(),
                    ..status.clone()
                },
                now
            )
            .is_err()
        );
        assert!(
            current_projection(
                &ShareUserGrant {
                    active: false,
                    ..grant
                },
                &status,
                now
            )
            .is_err()
        );
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PricingSettings {
    pub version: i64,
    pub revision: Option<String>,
    pub effective_from_ms: Option<i64>,
    pub snapshot: Option<PricingSnapshot>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PublishPricing {
    pub expected_version: i64,
    pub effective_from_ms: i64,
    pub snapshot: PricingSnapshot,
}
fn db_error(error: impl std::fmt::Display) -> AppError {
    AppError::Internal(format!("Share value quota storage: {error}"))
}
fn read_settings(conn: &Connection) -> Result<PricingSettings, AppError> {
    let row: Option<(i64,String,i64,String)> = conn.query_row(
        "SELECT c.version,s.revision,s.effective_from_ms,s.payload_json FROM share_value_pricing_config c
         JOIN share_value_pricing_snapshots s ON s.revision=c.revision WHERE c.singleton=1", [],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(db_error)?;
    match row {
        None => Ok(PricingSettings {
            version: 0,
            revision: None,
            effective_from_ms: None,
            snapshot: None,
        }),
        Some((version, revision, at, json)) => {
            let snapshot: PricingSnapshot = serde_json::from_str(&json).map_err(db_error)?;
            if snapshot.revision().map_err(db_error)? != revision {
                return Err(db_error("snapshot hash mismatch"));
            }
            Ok(PricingSettings {
                version,
                revision: Some(revision),
                effective_from_ms: Some(at),
                snapshot: Some(snapshot),
            })
        }
    }
}
fn current_projection(
    grant: &ShareUserGrant,
    status: &ValueQuotaStatus,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let binding = grant
        .value_metering
        .as_ref()
        .ok_or_else(|| AppError::Conflict("quota binding unavailable".into()))?;
    let used =
        unsigned_decimal(&status.used_picos).map_err(|e| AppError::BadRequest(e.to_string()))?;
    let observed = unsigned_decimal(&status.observed_picos)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    let offset = unsigned_decimal(&status.manual_offset_picos)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    unsigned_decimal(&status.pending_risk_picos)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    let (start, end, id) = if grant.policy.token_period == ShareTokenPeriod::AccountReset {
        let cycle = grant
            .account_quota_cycle
            .as_ref()
            .ok_or_else(|| AppError::Conflict("quota cycle unavailable".into()))?;
        if cycle.starts_at_ms > now.timestamp_millis()
            || !cycle
                .ends_at_ms
                .is_some_and(|end| now.timestamp_millis() < end)
        {
            return Err(AppError::Conflict("quota cycle is not current".into()));
        }
        (
            cycle.starts_at_ms,
            cycle.ends_at_ms,
            format!(
                "account:{}:{}:{}:{}",
                cycle.source_ref, cycle.source_epoch, cycle.bucket_id, cycle.cycle_id
            ),
        )
    } else {
        let (start, end) = token_period_window(&grant.policy, now)?;
        let start = start.map_or(0, |v| v.timestamp_millis());
        let end = end.map(|v| v.timestamp_millis());
        (
            start,
            end,
            format!(
                "{:?}:{}:{}",
                grant.policy.token_period,
                start,
                end.unwrap_or(0)
            ),
        )
    };
    if !grant.active
        || status.email != grant.email.trim().to_ascii_lowercase()
        || binding.quota_policy_epoch != status.policy_epoch
        || binding.value_metered_from_ms != status.metered_from_ms
        || status.limit_usd_micros.as_ref()
            != grant
                .policy
                .value_quota
                .as_ref()
                .map(|q| &q.limit_usd_micros)
        || status.window_starts_at_ms != start
        || status.resets_at_ms != end
        || status.window_id != id
        || observed.checked_add(offset) != Some(used)
    {
        return Err(AppError::Conflict(
            "quota policy or current window changed".into(),
        ));
    }
    Ok(())
}
impl AppStore {
    /// Latest publication is reported separately from the Server's active tariff.
    /// A future publication must not invalidate an already acknowledged active one.
    pub(crate) async fn value_pricing_readiness(
        &self,
        installation: &str,
    ) -> Result<serde_json::Value, AppError> {
        let conn = self.conn.lock().await;
        let settings = read_settings(&conn)?;
        let acknowledged_at: Option<i64> = conn.query_row(
            "SELECT acknowledged_at_ms FROM share_value_pricing_acks WHERE installation_id=?1 AND revision=?2",
            params![installation, settings.revision], |row| row.get(0),
        ).optional().map_err(db_error)?;
        Ok(serde_json::json!({"publishedRevision":settings.revision,
            "effectiveFromMs":settings.effective_from_ms,"acknowledgedAtMs":acknowledged_at,
            "version":settings.version}))
    }
    pub(crate) async fn value_quota_health(&self) -> Result<serde_json::Value, AppError> {
        let conn = self.conn.lock().await;
        let mut shares = conn
            .prepare("SELECT share_id,user_grants_json FROM shares")
            .map_err(db_error)?;
        let now = Utc::now();
        let (mut enabled, mut scheduled, mut shadow, mut stale) = (0, 0, 0, 0);
        for row in shares
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })
            .map_err(db_error)?
        {
            let (id, json) = row.map_err(db_error)?;
            for (email, grant) in parse_share_user_grants(json)
                .map_err(db_error)?
                .into_iter()
                .filter(|(_, g)| g.active)
            {
                if grant.pending_value_quota.is_some() {
                    scheduled += 1;
                }
                if grant.policy.value_quota.is_some() {
                    enabled += 1;
                } else if grant.value_metering.is_some() {
                    shadow += 1;
                }
                if grant.policy.value_quota.is_none() && grant.pending_value_quota.is_none() {
                    continue;
                }
                let mut projections=conn.prepare("SELECT payload_json FROM share_value_quota_projections WHERE share_id=?1 AND email=?2 ORDER BY synced_at_ms DESC").map_err(db_error)?;
                let mut fresh = false;
                for json in projections
                    .query_map(params![id, email], |r| r.get::<_, String>(0))
                    .map_err(db_error)?
                {
                    let status: ValueQuotaStatus =
                        serde_json::from_str(&json.map_err(db_error)?).map_err(db_error)?;
                    if current_projection(&grant, &status, now).is_ok()
                        && now.timestamp_millis().saturating_sub(status.synced_at_ms) <= 90_000
                    {
                        fresh = true;
                        break;
                    }
                }
                if !fresh {
                    stale += 1;
                }
            }
        }
        let settings = read_settings(&conn)?;
        let acknowledgements: i64 = conn
            .query_row(
                "SELECT count(*) FROM share_value_pricing_acks WHERE revision=?1",
                params![settings.revision],
                |r| r.get(0),
            )
            .map_err(db_error)?;
        Ok(
            serde_json::json!({"activeUsdGrants":enabled,"scheduledGrants":scheduled,"shadowGrants":shadow,"staleGrants":stale,"acknowledgedInstallations":acknowledgements,
                "publishedRevision":settings.revision,"effectiveFromMs":settings.effective_from_ms}),
        )
    }
    pub(crate) async fn value_pricing_settings(&self) -> Result<PricingSettings, AppError> {
        let conn = self.conn.lock().await;
        read_settings(&conn)
    }
    pub(crate) async fn publish_value_pricing(
        &self,
        input: PublishPricing,
        actor: &str,
    ) -> Result<PricingSettings, AppError> {
        let revision = input
            .snapshot
            .revision()
            .map_err(|e| AppError::BadRequest(e.to_string()))?;
        let now = Utc::now().timestamp_millis();
        if input.effective_from_ms < now || input.effective_from_ms > now + 366 * 86400000 {
            return Err(AppError::BadRequest(
                "effectiveFromMs must be within the next year".into(),
            ));
        }
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let current = read_settings(&tx)?;
        if current.version != input.expected_version {
            return Err(AppError::Conflict(
                "pricing configuration changed; reload before publishing".into(),
            ));
        }
        // A content revision is immutable, including its initial activation time.
        tx.execute(
            "INSERT OR IGNORE INTO share_value_pricing_snapshots VALUES(?1,?2,?3,?4,?5)",
            params![
                revision,
                serde_json::to_string(&input.snapshot).map_err(db_error)?,
                input.effective_from_ms,
                actor,
                now
            ],
        )
        .map_err(db_error)?;
        tx.execute("INSERT INTO share_value_pricing_config VALUES(1,?1,?2)
            ON CONFLICT(singleton) DO UPDATE SET revision=excluded.revision,version=excluded.version",
            params![revision,current.version.checked_add(1).ok_or_else(|| db_error("configuration version overflow"))?]).map_err(db_error)?;
        let result = read_settings(&tx)?;
        tx.commit().map_err(db_error)?;
        Ok(result)
    }
    pub(crate) async fn signed_value_operation(
        &self,
        action: &str,
        input: SignedValueRequest,
    ) -> Result<serde_json::Value, AppError> {
        if input.protocol_epoch != PROTOCOL_EPOCH {
            return Err(AppError::BadRequest("unsupported protocol epoch".into()));
        }
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let installation = get_installation(&tx, &input.installation_id)?
            .ok_or_else(|| AppError::Unauthorized("installation not found".into()))?;
        verify_signed_share_request(
            &tx,
            &installation.public_key,
            &input.installation_id,
            action,
            &input.payload,
            input.timestamp_ms,
            &input.nonce,
            &input.signature,
        )?;
        let response = match action {
            "share_value_pricing_fetch" => {
                if input.payload != serde_json::json!({}) {
                    return Err(AppError::BadRequest("fetch payload must be empty".into()));
                }
                let settings = read_settings(&tx)?;
                match (
                    settings.revision,
                    settings.snapshot,
                    settings.effective_from_ms,
                ) {
                    (Some(revision), Some(snapshot), Some(effective_from_ms)) => {
                        serde_json::to_value(PricingManifest {
                            installation_id: input.installation_id.clone(),
                            revision,
                            effective_from_ms,
                            snapshot,
                        })
                        .map_err(db_error)?
                    }
                    _ => serde_json::Value::Null,
                }
            }
            "share_value_pricing_ack" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct Ack {
                    revision: String,
                    storage_schema: u16,
                }
                let ack: Ack = serde_json::from_value(input.payload)
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                if ack.storage_schema < 3 {
                    return Err(AppError::BadRequest(
                        "committed value quota storage is required".into(),
                    ));
                }
                let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM share_value_pricing_snapshots WHERE revision=?1)",params![ack.revision],|r|r.get(0)).map_err(db_error)?;
                if !exists {
                    return Err(AppError::NotFound("pricing revision not found".into()));
                }
                tx.execute("INSERT INTO share_value_pricing_acks VALUES(?1,?2,?3)
                    ON CONFLICT(installation_id,revision) DO UPDATE SET acknowledged_at_ms=excluded.acknowledged_at_ms",
                    params![input.installation_id,ack.revision,Utc::now().timestamp_millis()]).map_err(db_error)?;
                serde_json::json!({"ok":true,"revision":ack.revision})
            }
            "share_value_quota_watermark" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct Report {
                    share_id: String,
                    status: ValueQuotaStatus,
                }
                let report: Report = serde_json::from_value(input.payload)
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                let mut status = report.status;
                // This is the Router's confirmation time, not an unbounded
                // Server-clock promise that a stale projection is fresh.
                status.synced_at_ms = Utc::now().timestamp_millis();
                unsigned_decimal(&status.used_picos)
                    .and_then(|_| unsigned_decimal(&status.pending_risk_picos))
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                if !matches!(
                    status.metering_status.as_str(),
                    "observed" | "pending" | "unavailable" | "conservative" | "scheduled"
                ) || status.email != status.email.trim().to_ascii_lowercase()
                    || status.window_id.is_empty()
                    || status.window_starts_at_ms < 0
                    || status
                        .resets_at_ms
                        .is_some_and(|end| end <= status.window_starts_at_ms)
                {
                    return Err(AppError::BadRequest("invalid value quota watermark".into()));
                }
                let grants: Option<String> = tx.query_row(
                    "SELECT user_grants_json FROM shares WHERE share_id=?1 AND installation_id=?2",
                    params![report.share_id,input.installation_id],|r|r.get(0)).optional().map_err(db_error)?;
                let grants = parse_share_user_grants(grants).map_err(db_error)?;
                let grant = grants
                    .get(&status.email)
                    .filter(|g| g.active)
                    .ok_or_else(|| {
                        AppError::Conflict("grant is not active on this installation".into())
                    })?;
                let binding = grant.value_metering.as_ref().ok_or_else(|| {
                    AppError::Conflict("grant has no acknowledged value policy".into())
                })?;
                current_projection(grant, &status, Utc::now())?;
                if binding.quota_policy_epoch != status.policy_epoch
                    || binding.value_metered_from_ms != status.metered_from_ms
                    || grant
                        .policy
                        .value_quota
                        .as_ref()
                        .map(|q| &q.limit_usd_micros)
                        != status.limit_usd_micros.as_ref()
                {
                    return Err(AppError::Conflict("quota policy revision changed".into()));
                }
                let acknowledged: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM share_value_pricing_acks WHERE installation_id=?1 AND revision=?2)",
                    params![input.installation_id,status.active_pricing_revision],|r|r.get(0)).map_err(db_error)?;
                if !acknowledged {
                    return Err(AppError::Conflict(
                        "pricing revision was not acknowledged".into(),
                    ));
                }
                let old: Option<(i64, String)> = tx
                    .query_row(
                        "SELECT usage_watermark,payload_json FROM share_value_quota_projections
                     WHERE installation_id=?1 AND share_id=?2 AND email=?3 AND window_id=?4",
                        params![
                            input.installation_id,
                            report.share_id,
                            status.email,
                            status.window_id
                        ],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .map_err(db_error)?;
                let mark = i64::try_from(status.usage_watermark)
                    .map_err(|_| AppError::BadRequest("watermark out of range".into()))?;
                if old.as_ref().is_some_and(|(old_mark, _)| *old_mark > mark) {
                    serde_json::json!({"ok":true,"usageWatermark":old.unwrap().0})
                } else {
                    let json = serde_json::to_string(&status).map_err(db_error)?;
                    // Equal watermarks may update only policy/time projections, never consumption.
                    if let Some((old_mark, old_json)) = old {
                        let old: ValueQuotaStatus =
                            serde_json::from_str(&old_json).map_err(db_error)?;
                        if old_mark == mark
                            && (old.used_picos != status.used_picos
                                || old.observed_picos != status.observed_picos
                                || old.manual_offset_picos != status.manual_offset_picos
                                || old.pending_risk_picos != status.pending_risk_picos
                                || old.usage_pricing_revisions_digest
                                    != status.usage_pricing_revisions_digest)
                        {
                            return Err(AppError::Conflict(
                                "same watermark has conflicting usage".into(),
                            ));
                        }
                    }
                    tx.execute("INSERT INTO share_value_quota_projections VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
                        ON CONFLICT(installation_id,share_id,email,window_id) DO UPDATE SET
                        policy_epoch=excluded.policy_epoch,usage_watermark=excluded.usage_watermark,payload_json=excluded.payload_json,synced_at_ms=excluded.synced_at_ms",
                        params![input.installation_id,report.share_id,status.email,status.policy_epoch,status.window_id,mark,json,Utc::now().timestamp_millis()]).map_err(db_error)?;
                    serde_json::json!({"ok":true,"usageWatermark":mark})
                }
            }
            _ => {
                return Err(AppError::BadRequest(
                    "unsupported value quota action".into(),
                ));
            }
        };
        tx.commit().map_err(db_error)?;
        Ok(response)
    }
    pub(crate) async fn share_value_quota_status(
        &self,
        share_id: &str,
        email: Option<&str>,
    ) -> Result<Vec<ValueQuotaStatus>, AppError> {
        let conn = self.conn.lock().await;
        let grants: Option<String> = conn
            .query_row(
                "SELECT user_grants_json FROM shares WHERE share_id=?1",
                params![share_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?;
        let grants = parse_share_user_grants(grants).map_err(db_error)?;
        let mut query = conn.prepare("SELECT p.payload_json FROM share_value_quota_projections p
            JOIN shares s ON s.share_id=p.share_id AND s.installation_id=p.installation_id
            WHERE p.share_id=?1 AND (?2 IS NULL OR p.email=?2) ORDER BY p.synced_at_ms DESC,p.email,p.window_id").map_err(db_error)?;
        let mut seen = std::collections::HashSet::new();
        let mut statuses = Vec::new();
        for json in query
            .query_map(params![share_id, email], |r| r.get::<_, String>(0))
            .map_err(db_error)?
        {
            let status: ValueQuotaStatus =
                serde_json::from_str(&json.map_err(db_error)?).map_err(db_error)?;
            if grants
                .get(&status.email)
                .is_some_and(|g| current_projection(g, &status, Utc::now()).is_ok())
                && seen.insert(status.email.clone())
            {
                statuses.push(status);
            }
        }
        Ok(statuses)
    }
}
