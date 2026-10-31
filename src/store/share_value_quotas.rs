//! Installation-bound executable tariffs and private quota projections.
use super::*;
use crate::share_value_pricing::{
    PricingManifest, PricingSnapshot, ValueQuotaStatus, unsigned_decimal,
};

/// Metering states a Server may report. `unavailable` is also the safe
/// display value for any state this Router does not recognise yet.
const METERING_STATUSES: &[&str] = &[
    "observed",
    "conservative",
    "pending",
    "blocked",
    "unavailable",
    // Current policy is not USD and a USD policy is pending. While a USD
    // policy is enforced the Server reports its real status instead; pending
    // fields always come from the stored descriptor, not the watermark.
    "scheduled",
];
/// A published tariff never activates sooner than this after publication, so a
/// browser clock or network delay cannot make a publish request fail.
const PUBLISH_MIN_LEAD_MS: i64 = 60_000;
const PUBLISH_MAX_LEAD_MS: i64 = 366 * 86_400_000;
/// Server watermark keepalive is 45 s with 60±6 s heartbeats; 180 s tolerates
/// a couple of missed beats before a projection counts as stale.
const PROJECTION_FRESH_MS: i64 = 180_000;

/// Signed Server envelope. `payload` holds every non-envelope body field; the
/// signature covers its canonical JSON (keys sorted by byte order, no
/// whitespace), which is exactly `serde_json::to_string(&payload)` because this
/// crate's `serde_json` does not enable `preserve_order`.
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
    /// Optional; absent or sooner than now+60s is clamped to now+60s.
    #[serde(default)]
    pub effective_from_ms: Option<i64>,
    pub snapshot: PricingSnapshot,
}

/// Private projection row with enough grant context to render on its own.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ValueQuotaView {
    #[serde(flatten)]
    pub status: ValueQuotaStatus,
    pub token_period: ShareTokenPeriod,
    pub pending_limit_usd_micros: Option<String>,
    pub pending_token_period: Option<ShareTokenPeriod>,
    pub pending_effective_from_ms: Option<i64>,
}
impl ValueQuotaView {
    fn new(grant: &ShareUserGrant, status: ValueQuotaStatus) -> Self {
        let pending = grant.pending_value_quota.as_ref();
        Self {
            status,
            token_period: grant.policy.token_period,
            pending_limit_usd_micros: pending
                .and_then(|p| p.policy.value_quota.as_ref())
                .map(|q| q.limit_usd_micros.clone()),
            pending_token_period: pending.map(|p| p.policy.token_period),
            pending_effective_from_ms: pending.map(|p| p.effective_from_ms),
        }
    }
}

/// A grant whose current policy is not USD but has a scheduled USD policy and
/// no current projection yet. Usage fields are absent: nothing was metered.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScheduledValueQuotaView {
    pub email: String,
    pub metering_status: &'static str,
    pub token_period: ShareTokenPeriod,
    pub pending_limit_usd_micros: String,
    pub pending_token_period: ShareTokenPeriod,
    pub pending_effective_from_ms: i64,
}
impl ScheduledValueQuotaView {
    fn new(key: &str, grant: &ShareUserGrant) -> Option<Self> {
        let pending = grant.pending_value_quota.as_ref()?;
        let quota = pending.policy.value_quota.as_ref()?;
        (grant.active && grant.policy.value_quota.is_none()).then(|| Self {
            email: key.to_string(),
            metering_status: "scheduled",
            token_period: grant.policy.token_period,
            pending_limit_usd_micros: quota.limit_usd_micros.clone(),
            pending_token_period: pending.policy.token_period,
            pending_effective_from_ms: pending.effective_from_ms,
        })
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub(crate) enum ValueQuotaRow {
    Projected(ValueQuotaView),
    Scheduled(ScheduledValueQuotaView),
}
impl ValueQuotaRow {
    pub(crate) fn email(&self) -> &str {
        match self {
            Self::Projected(view) => &view.status.email,
            Self::Scheduled(view) => &view.email,
        }
    }
    /// The Server-confirmed projection, absent for a not-yet-synced schedule.
    #[cfg(test)]
    pub(crate) fn projected(&self) -> Option<&ValueQuotaStatus> {
        match self {
            Self::Projected(view) => Some(&view.status),
            Self::Scheduled(_) => None,
        }
    }
}

fn db_error(error: impl std::fmt::Display) -> AppError {
    AppError::Internal(format!("Share value quota storage: {error}"))
}

struct SettingsMeta {
    version: i64,
    revision: Option<String>,
    effective_from_ms: Option<i64>,
}
/// Cheap read for readiness, health and `knownRevision` checks; never parses
/// or hashes the (up to 4 MiB) snapshot.
fn read_settings_meta(conn: &Connection) -> Result<SettingsMeta, AppError> {
    let row: Option<(i64, String, i64)> = conn
        .query_row(
            "SELECT c.version,s.revision,s.effective_from_ms FROM share_value_pricing_config c
             JOIN share_value_pricing_snapshots s ON s.revision=c.revision WHERE c.singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(db_error)?;
    Ok(match row {
        None => SettingsMeta {
            version: 0,
            revision: None,
            effective_from_ms: None,
        },
        Some((version, revision, at)) => SettingsMeta {
            version,
            revision: Some(revision),
            effective_from_ms: Some(at),
        },
    })
}
type RawSettings = Option<(i64, String, i64, String)>;
fn read_settings_raw(conn: &Connection) -> Result<RawSettings, AppError> {
    conn.query_row(
        "SELECT c.version,s.revision,s.effective_from_ms,s.payload_json FROM share_value_pricing_config c
         JOIN share_value_pricing_snapshots s ON s.revision=c.revision WHERE c.singleton=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .optional()
    .map_err(db_error)
}
/// Pure decode + integrity check; callers may run it after releasing the DB lock.
fn decode_settings(raw: RawSettings) -> Result<PricingSettings, AppError> {
    match raw {
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

/// Persisted window ids. These exact strings are stored keys; never derive them
/// from `Debug` output.
fn natural_window_id(period: ShareTokenPeriod, start: i64, end: Option<i64>) -> String {
    let name = match period {
        ShareTokenPeriod::Lifetime => "Lifetime",
        ShareTokenPeriod::Day => "Day",
        ShareTokenPeriod::Week => "Week",
        ShareTokenPeriod::SevenDays => "SevenDays",
        ShareTokenPeriod::CalendarMonth => "CalendarMonth",
        ShareTokenPeriod::ThirtyDays => "ThirtyDays",
        ShareTokenPeriod::AccountReset => "AccountReset",
    };
    format!("{name}:{start}:{}", end.unwrap_or(0))
}
struct ExpectedWindow {
    start: i64,
    end: Option<i64>,
    id: String,
}
fn expected_window(grant: &ShareUserGrant, now: DateTime<Utc>) -> Result<ExpectedWindow, AppError> {
    if grant.policy.token_period == ShareTokenPeriod::AccountReset {
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
        return Ok(ExpectedWindow {
            start: cycle.starts_at_ms,
            end: cycle.ends_at_ms,
            id: format!(
                "account:{}:{}:{}:{}",
                cycle.source_ref, cycle.source_epoch, cycle.bucket_id, cycle.cycle_id
            ),
        });
    }
    let (start, end) = token_period_window(&grant.policy, now)?;
    let start = start.map_or(0, |v| v.timestamp_millis());
    let end = end.map(|v| v.timestamp_millis());
    Ok(ExpectedWindow {
        start,
        end,
        id: natural_window_id(grant.policy.token_period, start, end),
    })
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
    let window = expected_window(grant, now)?;
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
        || status.window_starts_at_ms != window.start
        || status.resets_at_ms != window.end
        || status.window_id != window.id
        || observed.checked_add(offset) != Some(used)
    {
        return Err(AppError::Conflict(
            "quota policy or current window changed".into(),
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FetchPayload {
    #[serde(default)]
    known_revision: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AckPayload {
    revision: String,
    storage_schema: u16,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WatermarkPayload {
    share_id: String,
    status: ValueQuotaStatus,
}
enum ValueOperation {
    Fetch(FetchPayload),
    Ack(AckPayload),
    Watermark(WatermarkPayload),
}
fn bad_payload(error: serde_json::Error) -> AppError {
    AppError::BadRequest(error.to_string())
}
/// Pure payload parsing/validation, run before any write lock is taken.
fn parse_value_operation(
    action: &str,
    payload: &serde_json::Value,
) -> Result<ValueOperation, AppError> {
    match action {
        "share_value_pricing_fetch" => FetchPayload::deserialize(payload)
            .map(ValueOperation::Fetch)
            .map_err(bad_payload),
        "share_value_pricing_ack" => {
            let ack = AckPayload::deserialize(payload).map_err(bad_payload)?;
            if ack.storage_schema < 3 {
                return Err(AppError::BadRequest(
                    "committed value quota storage is required".into(),
                ));
            }
            Ok(ValueOperation::Ack(ack))
        }
        "share_value_quota_watermark" => {
            let mut report = WatermarkPayload::deserialize(payload).map_err(bad_payload)?;
            let status = &mut report.status;
            unsigned_decimal(&status.used_picos)
                .and_then(|_| unsigned_decimal(&status.pending_risk_picos))
                .map_err(|e| AppError::BadRequest(e.to_string()))?;
            if status.email != status.email.trim().to_ascii_lowercase()
                || status.window_id.is_empty()
                || status.window_starts_at_ms < 0
                || status
                    .resets_at_ms
                    .is_some_and(|end| end <= status.window_starts_at_ms)
            {
                return Err(AppError::BadRequest("invalid value quota watermark".into()));
            }
            if !METERING_STATUSES.contains(&status.metering_status.as_str()) {
                // A newer Server state must neither break projection sync nor
                // be rendered as usable quota.
                tracing::warn!(
                    share_id = %report.share_id,
                    metering_status = %status.metering_status,
                    "unknown Share value metering status stored as unavailable"
                );
                status.metering_status = "unavailable".into();
            }
            Ok(ValueOperation::Watermark(report))
        }
        _ => Err(AppError::BadRequest(
            "unsupported value quota action".into(),
        )),
    }
}

/// What the IMMEDIATE transaction produced; a full manifest is decoded and
/// serialized only after the global DB lock is released.
enum ValueOutcome {
    Ready(serde_json::Value),
    Manifest {
        installation_id: String,
        raw: RawSettings,
    },
}

impl AppStore {
    /// Latest publication is reported separately from the Server's active tariff.
    /// A future publication must not invalidate an already acknowledged active one.
    pub(crate) async fn value_pricing_readiness(
        &self,
        installation: &str,
    ) -> Result<serde_json::Value, AppError> {
        let conn = self.conn.lock().await;
        let settings = read_settings_meta(&conn)?;
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
        let now = Utc::now();
        // One pass over projections that belong to the Share's current installation.
        let mut projections: HashMap<(String, String), Vec<(String, String)>> = HashMap::new();
        {
            let mut rows = conn
                .prepare(
                    "SELECT p.share_id,p.email,p.window_id,p.payload_json
                     FROM share_value_quota_projections p
                     JOIN shares s ON s.share_id=p.share_id AND s.installation_id=p.installation_id",
                )
                .map_err(db_error)?;
            for row in rows
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })
                .map_err(db_error)?
            {
                let (share, email, window, json) = row.map_err(db_error)?;
                projections
                    .entry((share, email))
                    .or_default()
                    .push((window, json));
            }
        }
        let mut shares = conn
            .prepare("SELECT share_id,user_grants_json FROM shares")
            .map_err(db_error)?;
        let (mut enabled, mut scheduled, mut legacy_bindings, mut stale) = (0, 0, 0, 0);
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
                } else if grant.value_metering.is_some() && grant.pending_value_quota.is_none() {
                    // Pre-C4 shadow binding not yet replaced by a fresh descriptor sync.
                    legacy_bindings += 1;
                }
                if grant.policy.value_quota.is_none() && grant.pending_value_quota.is_none() {
                    continue;
                }
                let window = expected_window(&grant, now).ok();
                let fresh = window.is_some_and(|window| {
                    projections
                        .get(&(id.clone(), email.clone()))
                        .into_iter()
                        .flatten()
                        .filter(|(window_id, _)| *window_id == window.id)
                        .filter_map(|(_, json)| serde_json::from_str::<ValueQuotaStatus>(json).ok())
                        .any(|status| {
                            current_projection(&grant, &status, now).is_ok()
                                && now.timestamp_millis().saturating_sub(status.synced_at_ms)
                                    <= PROJECTION_FRESH_MS
                        })
                });
                if !fresh {
                    stale += 1;
                }
            }
        }
        let settings = read_settings_meta(&conn)?;
        let acknowledgements: i64 = conn
            .query_row(
                "SELECT count(*) FROM share_value_pricing_acks WHERE revision=?1",
                params![settings.revision],
                |r| r.get(0),
            )
            .map_err(db_error)?;
        // `shadowGrants` is retained for older UIs: shadow metering was removed,
        // so it only counts legacy bindings awaiting the next descriptor sync.
        Ok(
            serde_json::json!({"activeUsdGrants":enabled,"scheduledGrants":scheduled,"shadowGrants":legacy_bindings,"staleGrants":stale,"acknowledgedInstallations":acknowledgements,
                "publishedRevision":settings.revision,"effectiveFromMs":settings.effective_from_ms}),
        )
    }
    pub(crate) async fn value_pricing_settings(&self) -> Result<PricingSettings, AppError> {
        let raw = {
            let conn = self.conn.lock().await;
            read_settings_raw(&conn)?
        };
        decode_settings(raw)
    }
    pub(crate) async fn publish_value_pricing(
        &self,
        input: PublishPricing,
        actor: &str,
    ) -> Result<PricingSettings, AppError> {
        let json = input
            .snapshot
            .canonical_json()
            .map_err(|e| AppError::BadRequest(e.to_string()))?;
        let revision = PricingSnapshot::revision_of_canonical_json(&json);
        let now = Utc::now().timestamp_millis();
        let earliest = now + PUBLISH_MIN_LEAD_MS;
        let effective_from_ms = input
            .effective_from_ms
            .map_or(earliest, |at| at.max(earliest));
        if effective_from_ms > now + PUBLISH_MAX_LEAD_MS {
            return Err(AppError::BadRequest(
                "effectiveFromMs must be within the next year".into(),
            ));
        }
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let current = read_settings_meta(&tx)?;
        if current.version != input.expected_version {
            return Err(AppError::Conflict(
                "pricing configuration changed; reload before publishing".into(),
            ));
        }
        // A content revision is immutable, including its initial activation time.
        tx.execute(
            "INSERT OR IGNORE INTO share_value_pricing_snapshots VALUES(?1,?2,?3,?4,?5)",
            params![revision, json, effective_from_ms, actor, now],
        )
        .map_err(db_error)?;
        tx.execute("INSERT INTO share_value_pricing_config VALUES(1,?1,?2)
            ON CONFLICT(singleton) DO UPDATE SET revision=excluded.revision,version=excluded.version",
            params![revision,current.version.checked_add(1).ok_or_else(|| db_error("configuration version overflow"))?]).map_err(db_error)?;
        let meta = read_settings_meta(&tx)?;
        tx.commit().map_err(db_error)?;
        drop(conn);
        // The stored activation time may predate this request (immutable revision).
        Ok(PricingSettings {
            version: meta.version,
            revision: meta.revision,
            effective_from_ms: meta.effective_from_ms,
            snapshot: Some(input.snapshot),
        })
    }
    /// Ed25519 envelope check happens before the IMMEDIATE transaction: only the
    /// installation key is read under a short lock, the signature and payload
    /// are verified lock-free, then the write transaction re-checks the key,
    /// consumes the nonce and applies the operation.
    pub(crate) async fn signed_value_operation(
        &self,
        action: &str,
        input: SignedValueRequest,
    ) -> Result<serde_json::Value, AppError> {
        if input.protocol_epoch != PROTOCOL_EPOCH {
            return Err(AppError::BadRequest("unsupported protocol epoch".into()));
        }
        if !matches!(
            action,
            "share_value_pricing_fetch" | "share_value_pricing_ack" | "share_value_quota_watermark"
        ) {
            return Err(AppError::BadRequest(
                "unsupported value quota action".into(),
            ));
        }
        if signed_request_timestamp_is_stale(Utc::now(), input.timestamp_ms) {
            return Err(AppError::Unauthorized("stale signed request".into()));
        }
        let payload_json = serde_json::to_string(&input.payload)
            .map_err(|_| AppError::Unauthorized("invalid signed payload".into()))?;
        let public_key = {
            let conn = self.conn.lock().await;
            get_installation(&conn, &input.installation_id)?
                .ok_or_else(|| AppError::Unauthorized("installation not found".into()))?
                .public_key
        };
        verify_signed_payload_json(
            &public_key,
            &input.installation_id,
            action,
            &payload_json,
            input.timestamp_ms,
            &input.nonce,
            &input.signature,
        )?;
        let operation = parse_value_operation(action, &input.payload)?;
        let outcome = {
            let conn = self.conn.lock().await;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(db_error)?;
            // The key may have rotated or the installation retired meanwhile.
            if get_installation(&tx, &input.installation_id)?
                .is_none_or(|installation| installation.public_key != public_key)
            {
                return Err(AppError::Unauthorized("installation not found".into()));
            }
            consume_authenticated_installation_nonce(
                &tx,
                &input.installation_id,
                action,
                &input.nonce,
                Utc::now(),
            )?;
            let outcome = apply_value_operation(&tx, &input.installation_id, operation)?;
            tx.commit().map_err(db_error)?;
            outcome
        };
        match outcome {
            ValueOutcome::Ready(value) => Ok(value),
            ValueOutcome::Manifest {
                installation_id,
                raw,
            } => {
                let settings = decode_settings(raw)?;
                match (
                    settings.revision,
                    settings.snapshot,
                    settings.effective_from_ms,
                ) {
                    (Some(revision), Some(snapshot), Some(effective_from_ms)) => {
                        serde_json::to_value(PricingManifest {
                            installation_id,
                            revision,
                            effective_from_ms,
                            snapshot,
                        })
                        .map_err(db_error)
                    }
                    _ => Ok(serde_json::Value::Null),
                }
            }
        }
    }
    pub(crate) async fn share_value_quota_status(
        &self,
        share_id: &str,
        email: Option<&str>,
    ) -> Result<Vec<ValueQuotaRow>, AppError> {
        let conn = self.conn.lock().await;
        let share: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT installation_id,user_grants_json FROM shares WHERE share_id=?1",
                params![share_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(db_error)?;
        let Some((installation_id, grants)) = share else {
            return Ok(Vec::new());
        };
        let grants = parse_share_user_grants(grants).map_err(db_error)?;
        let now = Utc::now();
        let visible = |key: &str, g: &ShareUserGrant| g.active && email.is_none_or(|e| e == key);
        // Only the current window of each bound grant is ever read or parsed.
        let current: HashMap<&str, String> = grants
            .iter()
            .filter(|(key, g)| visible(key, g) && g.value_metering.is_some())
            .filter_map(|(key, g)| Some((key.as_str(), expected_window(g, now).ok()?.id)))
            .collect();
        let mut rows = Vec::new();
        if !current.is_empty() {
            let mut query = conn
                .prepare(
                    "SELECT email,window_id,payload_json FROM share_value_quota_projections
                     WHERE installation_id=?1 AND share_id=?2 AND (?3 IS NULL OR email=?3)",
                )
                .map_err(db_error)?;
            for row in query
                .query_map(params![installation_id, share_id, email], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .map_err(db_error)?
            {
                let (row_email, window_id, json) = row.map_err(db_error)?;
                if current.get(row_email.as_str()) != Some(&window_id) {
                    continue;
                }
                let status: ValueQuotaStatus = serde_json::from_str(&json).map_err(db_error)?;
                if let Some(grant) = grants
                    .get(&status.email)
                    .filter(|g| current_projection(g, &status, now).is_ok())
                {
                    rows.push(ValueQuotaRow::Projected(ValueQuotaView::new(grant, status)));
                }
            }
        }
        // A scheduled USD policy is visible before its first Server watermark,
        // from the stored descriptor only; usage is never fabricated.
        let projected: HashSet<String> = rows.iter().map(|r| r.email().to_string()).collect();
        rows.extend(
            grants
                .iter()
                .filter(|(key, g)| visible(key, g) && !projected.contains(key.as_str()))
                .filter_map(|(key, g)| ScheduledValueQuotaView::new(key, g))
                .map(ValueQuotaRow::Scheduled),
        );
        rows.sort_by(|a, b| a.email().cmp(b.email()));
        Ok(rows)
    }
}

fn revision_acknowledged(
    conn: &Connection,
    installation_id: &str,
    revision: &str,
) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM share_value_pricing_acks WHERE installation_id=?1 AND revision=?2)",
        params![installation_id, revision],
        |r| r.get(0),
    )
    .map_err(db_error)
}

fn apply_value_operation(
    tx: &Connection,
    installation_id: &str,
    operation: ValueOperation,
) -> Result<ValueOutcome, AppError> {
    match operation {
        ValueOperation::Fetch(fetch) => {
            let meta = read_settings_meta(tx)?;
            match (fetch.known_revision, meta.revision) {
                (_, None) => Ok(ValueOutcome::Ready(serde_json::Value::Null)),
                // `unchanged` only when this installation's ACK row exists too;
                // otherwise the full manifest lets the Server re-install and
                // re-ACK (e.g. after a lost ACK or a Router-side restore).
                (Some(known), Some(revision))
                    if known == revision
                        && revision_acknowledged(tx, installation_id, &revision)? =>
                {
                    Ok(ValueOutcome::Ready(
                        serde_json::json!({"unchanged":true,"revision":revision}),
                    ))
                }
                _ => Ok(ValueOutcome::Manifest {
                    installation_id: installation_id.to_string(),
                    raw: read_settings_raw(tx)?,
                }),
            }
        }
        ValueOperation::Ack(ack) => {
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM share_value_pricing_snapshots WHERE revision=?1)",
                    params![ack.revision],
                    |r| r.get(0),
                )
                .map_err(db_error)?;
            if !exists {
                return Err(AppError::NotFound("pricing revision not found".into()));
            }
            // Idempotent: a repeated ACK keeps the original acknowledgement time.
            tx.execute(
                "INSERT INTO share_value_pricing_acks VALUES(?1,?2,?3)
                 ON CONFLICT(installation_id,revision) DO NOTHING",
                params![installation_id, ack.revision, Utc::now().timestamp_millis()],
            )
            .map_err(db_error)?;
            Ok(ValueOutcome::Ready(
                serde_json::json!({"ok":true,"revision":ack.revision}),
            ))
        }
        ValueOperation::Watermark(report) => {
            apply_watermark(tx, installation_id, report).map(ValueOutcome::Ready)
        }
    }
}

fn apply_watermark(
    tx: &Connection,
    installation_id: &str,
    report: WatermarkPayload,
) -> Result<serde_json::Value, AppError> {
    let mut status = report.status;
    // This is the Router's confirmation time, not an unbounded Server-clock
    // promise that a stale projection is fresh.
    let now = Utc::now();
    status.synced_at_ms = now.timestamp_millis();
    let grants: Option<String> = tx
        .query_row(
            "SELECT user_grants_json FROM shares WHERE share_id=?1 AND installation_id=?2",
            params![report.share_id, installation_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?;
    let grants = parse_share_user_grants(grants).map_err(db_error)?;
    let grant = grants
        .get(&status.email)
        .filter(|g| g.active)
        .ok_or_else(|| AppError::Conflict("grant is not active on this installation".into()))?;
    // Binds installation, grant, policy epoch, metering origin, limit and window.
    current_projection(grant, &status, now)?;
    if !revision_acknowledged(tx, installation_id, &status.active_pricing_revision)? {
        return Err(AppError::Conflict(
            "pricing revision was not acknowledged".into(),
        ));
    }
    let old: Option<(i64, String)> = tx
        .query_row(
            "SELECT usage_watermark,payload_json FROM share_value_quota_projections
             WHERE installation_id=?1 AND share_id=?2 AND email=?3 AND window_id=?4",
            params![
                installation_id,
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
    let response = match old {
        Some((old_mark, _)) if old_mark > mark => {
            serde_json::json!({"ok":true,"usageWatermark":old_mark})
        }
        old => {
            // Equal watermarks may update only policy/time projections, never consumption.
            if let Some((old_mark, old_json)) = old {
                let old: ValueQuotaStatus = serde_json::from_str(&old_json).map_err(db_error)?;
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
            let json = serde_json::to_string(&status).map_err(db_error)?;
            tx.execute(
                "INSERT INTO share_value_quota_projections VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(installation_id,share_id,email,window_id) DO UPDATE SET
                 policy_epoch=excluded.policy_epoch,usage_watermark=excluded.usage_watermark,
                 payload_json=excluded.payload_json,synced_at_ms=excluded.synced_at_ms",
                params![
                    installation_id,
                    report.share_id,
                    status.email,
                    status.policy_epoch,
                    status.window_id,
                    mark,
                    json,
                    status.synced_at_ms
                ],
            )
            .map_err(db_error)?;
            serde_json::json!({"ok":true,"usageWatermark":mark})
        }
    };
    // Only the current window is readable (`current_projection` rejects every
    // other window), so older windows and rows left by a previous installation
    // of this Share are dropped once the current row exists.
    tx.execute(
        "DELETE FROM share_value_quota_projections
         WHERE share_id=?1 AND email=?2 AND NOT (installation_id=?3 AND window_id=?4)",
        params![
            report.share_id,
            status.email,
            installation_id,
            status.window_id
        ],
    )
    .map_err(db_error)?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_ids_are_pinned_for_every_period() {
        let cases = [
            (ShareTokenPeriod::Lifetime, 0, None, "Lifetime:0:0"),
            (ShareTokenPeriod::Day, 10, Some(20), "Day:10:20"),
            (ShareTokenPeriod::Week, 10, Some(20), "Week:10:20"),
            (ShareTokenPeriod::SevenDays, 10, Some(20), "SevenDays:10:20"),
            (
                ShareTokenPeriod::CalendarMonth,
                10,
                Some(20),
                "CalendarMonth:10:20",
            ),
            (
                ShareTokenPeriod::ThirtyDays,
                10,
                Some(20),
                "ThirtyDays:10:20",
            ),
            (
                ShareTokenPeriod::AccountReset,
                10,
                Some(20),
                "AccountReset:10:20",
            ),
        ];
        for (period, start, end, expected) in cases {
            assert_eq!(natural_window_id(period, start, end), expected);
            // Persisted keys predate the explicit match and used `Debug`.
            assert_eq!(format!("{period:?}:{start}:{}", end.unwrap_or(0)), expected);
        }
    }

    #[test]
    fn signed_value_payload_fixture_reserializes_to_canonical_bytes() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/share-value-signed-payload-v1.json"
        ))
        .unwrap();
        let mut body = serde_json::json!({
            "protocolEpoch": PROTOCOL_EPOCH,
            "installationId": "inst-fixture",
            "timestampMs": 1_793_360_000_000_i64,
            "nonce": "nonce-fixture",
            "signature": "c2lnbmF0dXJl",
        });
        for (key, value) in fixture["payload"].as_object().unwrap() {
            body[key] = value.clone();
        }
        // Non-canonical wire order/whitespace must not matter.
        let wire = serde_json::to_string_pretty(&body).unwrap();
        let request: SignedValueRequest = serde_json::from_str(&wire).unwrap();
        assert_eq!(request.installation_id, "inst-fixture");
        assert_eq!(
            serde_json::to_string(&request.payload).unwrap(),
            fixture["canonical"].as_str().unwrap()
        );
        assert!(matches!(
            parse_value_operation(fixture["action"].as_str().unwrap(), &request.payload),
            Ok(ValueOperation::Watermark(_))
        ));
    }

    #[test]
    fn fetch_payload_accepts_known_revision_and_rejects_unknown_keys() {
        assert!(matches!(
            parse_value_operation("share_value_pricing_fetch", &serde_json::json!({})),
            Ok(ValueOperation::Fetch(FetchPayload {
                known_revision: None
            }))
        ));
        assert!(matches!(
            parse_value_operation(
                "share_value_pricing_fetch",
                &serde_json::json!({"knownRevision":"sha256:abc"})
            ),
            Ok(ValueOperation::Fetch(FetchPayload { known_revision: Some(r) })) if r == "sha256:abc"
        ));
        assert!(
            parse_value_operation(
                "share_value_pricing_fetch",
                &serde_json::json!({"knownRevision":"x","extra":1})
            )
            .is_err()
        );
        assert!(parse_value_operation("share_value_unknown", &serde_json::json!({})).is_err());
    }

    #[test]
    fn watermark_metering_statuses_accept_blocked_and_degrade_unknown_values() {
        let status = |metering: &str| {
            serde_json::json!({"shareId":"s","status":{
                "email":"user@example.test","policyEpoch":"e","windowId":"Lifetime:0:0",
                "windowStartsAtMs":0,"resetsAtMs":null,"meteredFromMs":0,
                "activePricingRevision":"r","usagePricingRevisionsDigest":"d","usageWatermark":1,
                "limitUsdMicros":"1","usedPicos":"0","observedPicos":"0","manualOffsetPicos":"0",
                "pendingRiskPicos":"0","meteringStatus":metering,"syncedAtMs":0}})
        };
        for accepted in [
            "observed",
            "conservative",
            "pending",
            "blocked",
            "unavailable",
        ] {
            let Ok(ValueOperation::Watermark(report)) =
                parse_value_operation("share_value_quota_watermark", &status(accepted))
            else {
                panic!("{accepted} must parse");
            };
            assert_eq!(report.status.metering_status, accepted);
        }
        let Ok(ValueOperation::Watermark(report)) =
            parse_value_operation("share_value_quota_watermark", &status("future_state"))
        else {
            panic!("unknown status must not reject the watermark");
        };
        assert_eq!(report.status.metering_status, "unavailable");
    }

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
                &grant,
                &ValueQuotaStatus {
                    limit_usd_micros: Some("2000000".into()),
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
                    metered_from_ms: 1,
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
