//! Share owners' text-token prices, scoped to one installation (client) and
//! layered on the global USD tariff. They only price models the global tariff
//! cannot resolve; a catalog price always wins and leaves the owner price
//! inert ("shadowed"). An installation without effective owner prices is
//! served the global revision untouched; one with them is served its own
//! content-addressed overlay revision, re-derived whenever either side
//! changes. See `docs/share-user-value-quota-implementation-plan.md` §17.
use super::*;
use crate::model_pricing::{ContextTier, ModelPrice, RateSet, ServiceTier};
use crate::share_value_pricing::{
    CacheWritePolicy, ContextInputBasis, ExecutionModelPrice, ExecutionRates,
};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

/// Owner prices per installation; keeps overlays small next to the catalog.
pub(crate) const MAX_OWNER_PRICES: usize = 200;
const MAX_SUGGESTIONS: usize = 5;
const TEXT: &str = "text";
const RESOURCE: &str = "resource";

/// micro-USD per 1M tokens, the unit of the catalog and of executable rates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct OwnerRates {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write_5m: i64,
    pub cache_write_1h: Option<i64>,
}

/// Decimal USD per 1M tokens, the admin price editor's wire format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelPriceRatesView {
    pub input_usd_per1m: String,
    pub output_usd_per1m: String,
    pub cache_read_usd_per1m: String,
    pub cache_write5m_usd_per1m: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write1h_usd_per1m: Option<String>,
}
impl From<&OwnerRates> for ModelPriceRatesView {
    fn from(rates: &OwnerRates) -> Self {
        Self {
            input_usd_per1m: usd_per1m(rates.input),
            output_usd_per1m: usd_per1m(rates.output),
            cache_read_usd_per1m: usd_per1m(rates.cache_read),
            cache_write5m_usd_per1m: usd_per1m(rates.cache_write_5m),
            cache_write1h_usd_per1m: rates.cache_write_1h.map(usd_per1m),
        }
    }
}
fn usd_per1m(micros: i64) -> String {
    let (whole, fraction) = (micros / 1_000_000, micros % 1_000_000);
    if fraction == 0 {
        return whole.to_string();
    }
    let fraction = format!("{fraction:06}");
    format!("{whole}.{}", fraction.trim_end_matches('0'))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OwnerModelPrice {
    pub model_key: String,
    pub display_name: String,
    pub rates: ModelPriceRatesView,
    pub version: i64,
    pub updated_at_ms: i64,
    pub updated_by: String,
    /// The catalog now prices this model: the catalog price applies and this
    /// owner price is inert until removed (or the catalog drops the model).
    pub shadowed: bool,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelPriceSuggestion {
    pub model_key: String,
    pub display_name: String,
    pub rates: ModelPriceRatesView,
}
pub(crate) struct OwnerPriceEdit {
    pub model_key: String,
    pub display_name: Option<String>,
    pub rates: OwnerRates,
    pub expected_version: Option<i64>,
}

#[derive(Debug, Clone)]
pub(crate) struct OwnerPriceRow {
    pub model_key: String,
    pub display_name: String,
    pub rates: OwnerRates,
    pub version: i64,
    pub updated_at_ms: i64,
    pub updated_by: String,
}
impl OwnerPriceRow {
    fn view(&self, shadowed: bool) -> OwnerModelPrice {
        OwnerModelPrice {
            model_key: self.model_key.clone(),
            display_name: self.display_name.clone(),
            rates: (&self.rates).into(),
            version: self.version,
            updated_at_ms: self.updated_at_ms,
            updated_by: self.updated_by.clone(),
            shadowed,
        }
    }
}

pub(crate) fn price_invalid(reason: &'static str, details: serde_json::Value) -> AppError {
    let mut details = details;
    if !details.is_object() {
        details = serde_json::json!({});
    }
    details["reason"] = reason.into();
    AppError::Coded {
        status: axum::http::StatusCode::BAD_REQUEST,
        code: "MODEL_PRICE_INVALID",
        message: "The model price is not valid".into(),
        details,
    }
}
fn version_conflict(current: Option<i64>) -> AppError {
    AppError::coded_conflict(
        "MODEL_PRICE_VERSION_CONFLICT",
        "The model price changed; reload it and retry",
        serde_json::json!({"currentVersion": current}),
    )
}
pub(crate) fn share_not_found() -> AppError {
    AppError::Coded {
        status: axum::http::StatusCode::NOT_FOUND,
        code: "SHARE_NOT_FOUND",
        message: "Share not found".into(),
        details: serde_json::json!({}),
    }
}

const PRICE_COLUMNS: &str = "model_key,display_name,input_micros_per_1m,output_micros_per_1m,cache_read_micros_per_1m,cache_write_5m_micros_per_1m,cache_write_1h_micros_per_1m,version,updated_at_ms,updated_by";
fn price_row(r: &crate::db::Row<'_>) -> crate::db::Result<OwnerPriceRow> {
    Ok(OwnerPriceRow {
        model_key: r.get(0)?,
        display_name: r.get(1)?,
        rates: OwnerRates {
            input: r.get(2)?,
            output: r.get(3)?,
            cache_read: r.get(4)?,
            cache_write_5m: r.get(5)?,
            cache_write_1h: r.get(6)?,
        },
        version: r.get(7)?,
        updated_at_ms: r.get(8)?,
        updated_by: r.get(9)?,
    })
}
pub(super) fn read_owner_prices(
    conn: &Connection,
    installation: &str,
) -> Result<Vec<OwnerPriceRow>, AppError> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT {PRICE_COLUMNS} FROM share_value_owner_model_prices WHERE installation_id=?1 ORDER BY model_key"
        ))
        .map_err(db_error)?;
    let rows = statement
        .query_map(params![installation], price_row)
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error);
    rows
}
fn owner_digest(prices: &[OwnerPriceRow]) -> Result<String, AppError> {
    if prices.is_empty() {
        return Ok(String::new());
    }
    // Display names do not reach the tariff, so they never force a revision.
    let rates: Vec<_> = prices.iter().map(|p| (&p.model_key, &p.rates)).collect();
    Ok(PricingSnapshot::revision_of_canonical_json(
        &serde_json::to_string(&rates).map_err(db_error)?,
    ))
}

fn execution_rates(rates: &OwnerRates) -> ExecutionRates {
    let write_5m = rates.cache_write_5m.to_string();
    ExecutionRates {
        input: Some(rates.input.to_string()),
        output: Some(rates.output.to_string()),
        cache_read: Some(rates.cache_read.to_string()),
        // Same rule as the catalog: a missing 1h price charges the 5m rate.
        cache_write1h: Some(
            rates
                .cache_write_1h
                .map_or_else(|| write_5m.clone(), |v| v.to_string()),
        ),
        cache_write5m: Some(write_5m),
        cache_write_policy: CacheWritePolicy::Both,
    }
}
/// Owner prices only fill models the base tariff cannot resolve (same
/// [`PricingSnapshot::model`] resolution the Server meters with): a catalog
/// price always wins and leaves a same-named owner price inert ("shadowed").
/// A filled model gets every service tier at base context, so standard,
/// priority and flex lookups all hit. Aliases are untouched: a name the base
/// resolves through an alias is never filled. Returns the applied count.
pub(crate) fn apply_owner_prices(
    snapshot: &mut PricingSnapshot,
    prices: &[OwnerPriceRow],
) -> usize {
    // Decide against the base first, so one filled price never shadows another.
    let filled: Vec<&OwnerPriceRow> = prices
        .iter()
        .filter(|price| snapshot.model(&price.model_key).is_err())
        .collect();
    for price in &filled {
        let rates = execution_rates(&price.rates);
        snapshot.models.insert(
            price.model_key.clone(),
            ExecutionModelPrice {
                context_input_basis: ContextInputBasis::RawInput,
                long_context_threshold: None,
                long_context_inclusive: false,
                rates: ServiceTier::ALL
                    .iter()
                    .map(|tier| (format!("{}/base", tier.as_str()), rates.clone()))
                    .collect(),
                resources: BTreeMap::new(),
            },
        );
    }
    filled.len()
}
/// The base tariff's own price wins; the owner price is inert.
fn shadowed_by(base: &PricingSnapshot, model_key: &str) -> bool {
    base.model(model_key).is_ok()
}
/// The display-side twin of [`apply_owner_prices`] for the usage breakdown.
pub(crate) fn owner_model_price(key: &str, display_name: &str, rates: &OwnerRates) -> ModelPrice {
    let set = RateSet {
        input_micros_per_1m: rates.input,
        output_micros_per_1m: rates.output,
        cache_read_micros_per_1m: rates.cache_read,
        cache_write_5m_micros_per_1m: rates.cache_write_5m,
        cache_write_1h_micros_per_1m: rates.cache_write_1h,
    };
    ModelPrice {
        price_key: key.to_string(),
        display_name: display_name.to_string(),
        long_context_threshold: None,
        long_context_inclusive: false,
        supports_cache_breakdown: true,
        rates: ServiceTier::ALL
            .iter()
            .map(|tier| ((*tier, ContextTier::Base), set))
            .collect(),
    }
}

struct InstallationTariff {
    base_revision: String,
    owner_digest: String,
    revision: String,
    effective_from_ms: i64,
    version: i64,
}
fn read_installation_tariff(
    conn: &Connection,
    installation: &str,
    kind: &str,
) -> Result<Option<InstallationTariff>, AppError> {
    conn.query_row(
        "SELECT base_revision,owner_digest,revision,effective_from_ms,version
         FROM share_value_pricing_installation_tariffs WHERE installation_id=?1 AND tariff_kind=?2",
        params![installation, kind],
        |r| {
            Ok(InstallationTariff {
                base_revision: r.get(0)?,
                owner_digest: r.get(1)?,
                revision: r.get(2)?,
                effective_from_ms: r.get(3)?,
                version: r.get(4)?,
            })
        },
    )
    .optional()
    .map_err(db_error)
}

/// The tariff this installation is served: its own row, else the global one.
pub(super) fn installation_tariff_raw(
    conn: &Connection,
    installation: &str,
    resources: bool,
) -> Result<RawSettings, AppError> {
    let own: RawSettings = conn
        .query_row(
            "SELECT t.version,t.revision,max(t.effective_from_ms,s.effective_from_ms),s.payload_json
             FROM share_value_pricing_installation_tariffs t
             JOIN share_value_pricing_snapshots s ON s.revision=t.revision
             WHERE t.installation_id=?1 AND t.tariff_kind=?2",
            params![installation, if resources { RESOURCE } else { TEXT }],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(db_error)?;
    match own {
        Some(raw) => Ok(Some(raw)),
        None if resources => resource_tariff::read_resource_settings_raw(conn),
        None => read_settings_raw(conn),
    }
}
pub(super) fn installation_tariff_meta(
    conn: &Connection,
    installation: &str,
    resources: bool,
) -> Result<SettingsMeta, AppError> {
    let own: Option<(i64, String, i64)> = conn
        .query_row(
            "SELECT t.version,t.revision,max(t.effective_from_ms,s.effective_from_ms)
             FROM share_value_pricing_installation_tariffs t
             JOIN share_value_pricing_snapshots s ON s.revision=t.revision
             WHERE t.installation_id=?1 AND t.tariff_kind=?2",
            params![installation, if resources { RESOURCE } else { TEXT }],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(db_error)?;
    match own {
        Some((version, revision, at)) => Ok(SettingsMeta {
            version,
            revision: Some(revision),
            effective_from_ms: Some(at),
        }),
        None if resources => resource_tariff::read_resource_settings_meta(conn),
        None => read_settings_meta(conn),
    }
}
/// The revision a Market quote for a Share on this installation pins.
pub(crate) fn installation_tariff_revision(
    conn: &Connection,
    installation: &str,
    resources: bool,
) -> Result<Option<String>, AppError> {
    Ok(installation_tariff_meta(conn, installation, resources)?.revision)
}
/// Global revisions may be acknowledged by any installation; an owner overlay
/// only by the installation it was issued to.
pub(super) fn revision_issued_to(
    conn: &Connection,
    installation: &str,
    revision: &str,
) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM share_value_pricing_installation_revisions WHERE installation_id=?1 AND revision=?2)
         OR EXISTS(SELECT 1 FROM share_value_pricing_snapshots WHERE revision=?2 AND published_by<>'owner-prices')
         OR EXISTS(SELECT 1 FROM share_value_pricing_config WHERE revision=?2)
         OR EXISTS(SELECT 1 FROM share_value_resource_pricing_config WHERE revision=?2)",
        params![installation, revision],
        |r| r.get(0),
    )
    .map_err(db_error)
}

/// Parsed tariffs by revision. Revisions are content hashes, so one small
/// process-wide cache is correct for every store; it keeps multi-MiB global
/// snapshots from being parsed per request or per owner-price edit.
const PARSED_TARIFF_CACHE: usize = 4;
type ParsedTariffs = StdMutex<VecDeque<(String, Arc<PricingSnapshot>)>>;
fn parsed_tariffs() -> &'static ParsedTariffs {
    static CACHE: OnceLock<ParsedTariffs> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
fn cached_tariff(revision: &str) -> Option<Arc<PricingSnapshot>> {
    let mut cache = parsed_tariffs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let at = cache.iter().position(|(cached, _)| cached == revision)?;
    let entry = cache.remove(at)?;
    let snapshot = Arc::clone(&entry.1);
    cache.push_front(entry);
    Some(snapshot)
}
fn remember_tariff(revision: &str, snapshot: &Arc<PricingSnapshot>) {
    let mut cache = parsed_tariffs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.retain(|(cached, _)| cached != revision);
    cache.push_front((revision.to_string(), Arc::clone(snapshot)));
    cache.truncate(PARSED_TARIFF_CACHE);
}
fn join_error(error: tokio::task::JoinError) -> AppError {
    AppError::Internal(format!("derive owner-price tariff failed: {error}"))
}

/// The published global tariff of one kind.
fn global_meta(conn: &Connection, kind: &str) -> Result<Option<(String, i64)>, AppError> {
    conn.query_row(
        if kind == RESOURCE {
            "SELECT s.revision,s.effective_from_ms FROM share_value_resource_pricing_config c
             JOIN share_value_pricing_snapshots s ON s.revision=c.revision WHERE c.singleton=1"
        } else {
            "SELECT s.revision,s.effective_from_ms FROM share_value_pricing_config c
             JOIN share_value_pricing_snapshots s ON s.revision=c.revision WHERE c.singleton=1"
        },
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .map_err(db_error)
}
enum TariffSource {
    Parsed(Arc<PricingSnapshot>),
    Json(String),
}
/// A global tariff as read under the DB lock: the payload is copied only
/// when its parsed form is not cached; parsing happens after the lock.
struct GlobalBase {
    kind: &'static str,
    revision: String,
    source: TariffSource,
}
impl GlobalBase {
    fn read(conn: &Connection, kind: &'static str) -> Result<Option<Self>, AppError> {
        let Some((revision, _)) = global_meta(conn, kind)? else {
            return Ok(None);
        };
        let source = match cached_tariff(&revision) {
            Some(parsed) => TariffSource::Parsed(parsed),
            None => TariffSource::Json(
                conn.query_row(
                    "SELECT payload_json FROM share_value_pricing_snapshots WHERE revision=?1",
                    params![revision],
                    |r| r.get(0),
                )
                .map_err(db_error)?,
            ),
        };
        Ok(Some(Self {
            kind,
            revision,
            source,
        }))
    }
    fn read_all(conn: &Connection) -> Result<Vec<Self>, AppError> {
        Ok([TEXT, RESOURCE]
            .into_iter()
            .map(|kind| Self::read(conn, kind))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect())
    }
    /// The base an installation is served: its negotiated kind, else text.
    fn read_served(conn: &Connection, installation: &str) -> Result<Option<Self>, AppError> {
        if resource_tariff::installation_supports_resources(conn, installation)?
            && let Some(base) = Self::read(conn, RESOURCE)?
        {
            return Ok(Some(base));
        }
        Self::read(conn, TEXT)
    }
    /// CPU work: call it off the async runtime unless the base is cached.
    fn snapshot(&self) -> Result<Arc<PricingSnapshot>, AppError> {
        match &self.source {
            TariffSource::Parsed(parsed) => Ok(Arc::clone(parsed)),
            TariffSource::Json(json) => {
                let parsed = Arc::new(serde_json::from_str(json).map_err(db_error)?);
                remember_tariff(&self.revision, &parsed);
                Ok(parsed)
            }
        }
    }
    async fn snapshot_off_runtime(self) -> Result<Arc<PricingSnapshot>, AppError> {
        match self.source {
            TariffSource::Parsed(parsed) => Ok(parsed),
            TariffSource::Json(_) => tokio::task::spawn_blocking(move || self.snapshot())
                .await
                .map_err(join_error)?,
        }
    }
}

/// One installation's derivation inputs, read under the DB lock.
struct InstallationInputs {
    installation: String,
    prices: Vec<OwnerPriceRow>,
    digest: String,
    /// `(base revision, owner digest)` of each stored tariff row.
    current: HashMap<&'static str, (String, String)>,
}
fn read_inputs(
    conn: &Connection,
    installation: String,
    prices: Vec<OwnerPriceRow>,
) -> Result<InstallationInputs, AppError> {
    let digest = owner_digest(&prices)?;
    let mut current = HashMap::new();
    for kind in [TEXT, RESOURCE] {
        if let Some(row) = read_installation_tariff(conn, &installation, kind)? {
            current.insert(kind, (row.base_revision, row.owner_digest));
        }
    }
    Ok(InstallationInputs {
        installation,
        prices,
        digest,
        current,
    })
}
/// An overlay derived off the DB lock from one base revision and one set of
/// owner prices; the write re-checks both before using it.
struct DerivedOverlay {
    base_revision: String,
    digest: String,
    /// Served revision and, for a new overlay, its canonical payload.
    result: Result<(String, Option<String>), String>,
}
type DerivedOverlays = HashMap<(String, &'static str), DerivedOverlay>;
fn overlay_of(
    base: &PricingSnapshot,
    base_revision: &str,
    prices: &[OwnerPriceRow],
) -> Result<(String, Option<String>), String> {
    // Every price shadowed by the base: the base itself is served.
    if prices
        .iter()
        .all(|price| shadowed_by(base, &price.model_key))
    {
        return Ok((base_revision.to_string(), None));
    }
    let mut snapshot = base.clone();
    apply_owner_prices(&mut snapshot, prices);
    let json = snapshot
        .canonical_json()
        .map_err(|error| error.to_string())?;
    Ok((
        PricingSnapshot::revision_of_canonical_json(&json),
        Some(json),
    ))
}
/// CPU work (parse, clone, serialize, hash); runs on a blocking thread.
/// Rows already derived from the same base and owner prices are skipped, and
/// a base is parsed only when needed (always, with `parse_all`).
fn derive_overlays(
    bases: &[GlobalBase],
    inputs: &[InstallationInputs],
    parse_all: bool,
) -> Result<(Vec<(&'static str, Arc<PricingSnapshot>)>, DerivedOverlays), AppError> {
    let mut parsed: Vec<Option<Arc<PricingSnapshot>>> = vec![None; bases.len()];
    let mut overlays = HashMap::new();
    for (index, base) in bases.iter().enumerate() {
        if parse_all {
            parsed[index] = Some(base.snapshot()?);
        }
        for input in inputs.iter().filter(|input| !input.prices.is_empty()) {
            if input
                .current
                .get(base.kind)
                .is_some_and(|(revision, digest)| {
                    *revision == base.revision && *digest == input.digest
                })
            {
                continue;
            }
            let snapshot = match &parsed[index] {
                Some(snapshot) => Arc::clone(snapshot),
                None => {
                    let snapshot = base.snapshot()?;
                    parsed[index] = Some(Arc::clone(&snapshot));
                    snapshot
                }
            };
            overlays.insert(
                (input.installation.clone(), base.kind),
                DerivedOverlay {
                    base_revision: base.revision.clone(),
                    digest: input.digest.clone(),
                    result: overlay_of(&snapshot, &base.revision, &input.prices),
                },
            );
        }
    }
    let parsed = bases
        .iter()
        .zip(parsed)
        .filter_map(|(base, snapshot)| snapshot.map(|snapshot| (base.kind, snapshot)))
        .collect();
    Ok((parsed, overlays))
}

/// The base revision or the owner prices moved between derivation and write.
fn tariff_changed() -> AppError {
    AppError::coded_conflict(
        "MODEL_PRICE_TARIFF_CHANGED",
        "The USD tariff changed while saving; retry",
        serde_json::json!({}),
    )
}
fn is_tariff_changed(error: &AppError) -> bool {
    matches!(
        error,
        AppError::Coded {
            code: "MODEL_PRICE_TARIFF_CHANGED",
            ..
        }
    )
}
/// Derivations retried when a concurrent publication or edit raced them.
const DERIVE_ATTEMPTS: usize = 3;

/// Writes one tariff kind of one installation from an overlay derived off
/// the lock; returns the activation time of the revision it is served
/// afterwards (`None`: nothing published). Cheap: no parsing or hashing.
fn write_kind(
    conn: &Connection,
    installation: &str,
    kind: &'static str,
    overlays: &DerivedOverlays,
    prices_empty: bool,
    digest: &str,
    now: i64,
) -> Result<Option<i64>, AppError> {
    let current = read_installation_tariff(conn, installation, kind)?;
    let delete_row = || {
        conn.execute(
            "DELETE FROM share_value_pricing_installation_tariffs WHERE installation_id=?1 AND tariff_kind=?2",
            params![installation, kind],
        )
        .map_err(db_error)
    };
    let Some((global_revision, global_effective)) = global_meta(conn, kind)? else {
        delete_row()?;
        return Ok(None);
    };
    if let Some(row) = &current
        && row.base_revision == global_revision
        && row.owner_digest == digest
    {
        // A scheduled return to the global revision is no longer needed once
        // it is active: the installation is then served exactly the global one.
        if digest.is_empty() && row.revision == global_revision && row.effective_from_ms <= now {
            delete_row()?;
            return Ok(Some(global_effective));
        }
        return Ok(Some(row.effective_from_ms.max(global_effective)));
    }
    if current.is_none() && prices_empty {
        return Ok(Some(global_effective));
    }
    let (target, json): (&str, Option<&str>) = if prices_empty {
        (global_revision.as_str(), None)
    } else {
        let overlay = overlays
            .get(&(installation.to_string(), kind))
            .filter(|overlay| overlay.base_revision == global_revision && overlay.digest == digest)
            .ok_or_else(tariff_changed)?;
        match &overlay.result {
            Ok((target, json)) => (target.as_str(), json.as_deref()),
            Err(detail) => {
                return Err(price_invalid(
                    "tariff_invalid",
                    serde_json::json!({"detail": detail}),
                ));
            }
        }
    };
    let (served, served_at) = current
        .as_ref()
        .map_or((global_revision.as_str(), global_effective), |row| {
            (row.revision.as_str(), row.effective_from_ms)
        });
    let changed = target != served;
    // Replacements keep the publication lead time, including a return to an
    // earlier (content-addressed, hence older) revision.
    let effective = if changed {
        (now + PUBLISH_MIN_LEAD_MS).max(global_effective)
    } else {
        served_at
    };
    if let Some(json) = json {
        conn.execute(
            "INSERT INTO share_value_pricing_snapshots VALUES(?1,?2,?3,'owner-prices',?4)
             ON CONFLICT(revision) DO UPDATE SET payload_json=excluded.payload_json
             WHERE share_value_pricing_snapshots.payload_json='{}'",
            params![target, json, effective, now],
        )
        .map_err(db_error)?;
        if kind == RESOURCE {
            conn.execute(
                "INSERT OR IGNORE INTO share_value_resource_pricing_revisions(revision) VALUES(?1)",
                params![target],
            )
            .map_err(db_error)?;
        }
    }
    conn.execute(
        if changed {
            "INSERT INTO share_value_pricing_installation_revisions VALUES(?1,?2,?3,?4)
             ON CONFLICT(installation_id,revision) DO UPDATE SET
             effective_from_ms=excluded.effective_from_ms,issued_at_ms=excluded.issued_at_ms"
        } else {
            "INSERT OR IGNORE INTO share_value_pricing_installation_revisions VALUES(?1,?2,?3,?4)"
        },
        params![installation, target, effective, now],
    )
    .map_err(db_error)?;
    if prices_empty && target == global_revision && effective <= now {
        delete_row()?;
        return Ok(Some(global_effective));
    }
    let version = match &current {
        Some(row) if changed => row
            .version
            .checked_add(1)
            .ok_or_else(|| db_error("installation tariff version exhausted"))?,
        Some(row) => row.version,
        None => 1,
    };
    conn.execute(
        "INSERT INTO share_value_pricing_installation_tariffs VALUES(?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(installation_id,tariff_kind) DO UPDATE SET base_revision=excluded.base_revision,
         owner_digest=excluded.owner_digest,revision=excluded.revision,
         effective_from_ms=excluded.effective_from_ms,version=excluded.version",
        params![
            installation,
            kind,
            global_revision,
            digest,
            target,
            effective,
            version
        ],
    )
    .map_err(db_error)?;
    if changed {
        tracing::info!(%installation, %kind, revision = %target, "published an installation USD quota tariff");
    }
    Ok(Some(effective))
}

/// Both tariff kinds of one installation, from its prices as stored now;
/// returns the activation time of the kind this installation negotiated.
fn write_installation(
    conn: &Connection,
    installation: &str,
    overlays: &DerivedOverlays,
    now: i64,
) -> Result<Option<i64>, AppError> {
    let prices = read_owner_prices(conn, installation)?;
    let digest = owner_digest(&prices)?;
    let empty = prices.is_empty();
    let text_at = write_kind(conn, installation, TEXT, overlays, empty, &digest, now)?;
    let resource_at = write_kind(conn, installation, RESOURCE, overlays, empty, &digest, now)?;
    Ok(
        if resource_tariff::installation_supports_resources(conn, installation)? {
            resource_at
        } else {
            text_at
        },
    )
}

/// The largest catalog rate of each kind: owners may not price above it.
fn catalog_rate_caps(
    catalog: &crate::model_price_catalog::PricingCatalog,
    at_unix: i64,
) -> Option<OwnerRates> {
    let mut caps: Option<OwnerRates> = None;
    for (_, price) in catalog.effective_models(at_unix) {
        for set in price.rates.values() {
            let caps = caps.get_or_insert(OwnerRates {
                input: 0,
                output: 0,
                cache_read: 0,
                cache_write_5m: 0,
                cache_write_1h: None,
            });
            caps.input = caps.input.max(set.input_micros_per_1m);
            caps.output = caps.output.max(set.output_micros_per_1m);
            caps.cache_read = caps.cache_read.max(set.cache_read_micros_per_1m);
            caps.cache_write_5m = caps.cache_write_5m.max(set.cache_write_5m_micros_per_1m);
            if let Some(rate) = set.cache_write_1h_micros_per_1m {
                caps.cache_write_1h = Some(caps.cache_write_1h.unwrap_or(0).max(rate));
            }
        }
    }
    caps
}
pub(crate) fn validate_owner_rates(
    rates: &OwnerRates,
    catalog: &crate::model_price_catalog::PricingCatalog,
    at_unix: i64,
) -> Result<(), AppError> {
    let caps = catalog_rate_caps(catalog, at_unix)
        .ok_or_else(|| price_invalid("catalog_unavailable", serde_json::json!({})))?;
    // Without any 1h catalog price, 1h writes are capped like 5m writes.
    let cap_1h = caps.cache_write_1h.unwrap_or(caps.cache_write_5m);
    for (field, value, cap) in [
        ("inputUsdPer1m", rates.input, caps.input),
        ("outputUsdPer1m", rates.output, caps.output),
        ("cacheReadUsdPer1m", rates.cache_read, caps.cache_read),
        (
            "cacheWrite5mUsdPer1m",
            rates.cache_write_5m,
            caps.cache_write_5m,
        ),
        (
            "cacheWrite1hUsdPer1m",
            rates.cache_write_1h.unwrap_or(0),
            cap_1h,
        ),
    ] {
        if value < 0 {
            return Err(price_invalid(
                "rate_negative",
                serde_json::json!({"field": field}),
            ));
        }
        if value > cap {
            return Err(price_invalid(
                "rate_above_catalog_max",
                serde_json::json!({"field": field, "maxUsdPer1m": usd_per1m(cap)}),
            ));
        }
    }
    if rates
        .cache_write_1h
        .is_some_and(|rate| rate < rates.cache_write_5m)
    {
        return Err(price_invalid(
            "cache_write_1h_below_5m",
            serde_json::json!({"field": "cacheWrite1hUsdPer1m"}),
        ));
    }
    if rates.input == 0 && rates.output == 0 {
        return Err(price_invalid("rate_required", serde_json::json!({})));
    }
    Ok(())
}
/// Exact model key, same rules as the admin price editor.
pub(crate) fn normalize_owner_model_key(raw: &str) -> Result<String, AppError> {
    let key = raw.trim().to_ascii_lowercase();
    if key.is_empty()
        || key.len() > 200
        || key.chars().any(char::is_control)
        || key.contains('*')
        || !crate::share_value_pricing::valid_model_key(&key)
    {
        return Err(price_invalid(
            "model_key_invalid",
            serde_json::json!({"field": "modelKey"}),
        ));
    }
    Ok(key)
}
fn display_name(raw: Option<&str>, key: &str) -> Result<String, AppError> {
    let name = raw
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(key);
    if name.len() > 200 || name.chars().any(char::is_control) {
        return Err(price_invalid(
            "display_name_invalid",
            serde_json::json!({"field": "displayName"}),
        ));
    }
    Ok(name.to_string())
}

/// Lowercase, provider-qualifier-free and separator-free spelling used only to
/// rank suggestions: `Moonshot/Kimi-K3.1_Pro` → `kimik31pro`.
fn suggestion_key(raw: &str) -> String {
    let lower = raw.trim().to_ascii_lowercase();
    let mut name = lower.rsplit('/').next().unwrap_or_default();
    while let Some((head, tail)) = name.split_once('.')
        && !head.is_empty()
        && head.bytes().all(|b| b.is_ascii_alphabetic())
        && tail.starts_with(|c: char| c.is_ascii_alphabetic())
    {
        name = tail;
    }
    name.chars()
        .filter(|c| !matches!(c, '-' | '.' | '_' | ':' | ' '))
        .collect()
}
fn bigrams(value: &str) -> Vec<(char, char)> {
    let chars: Vec<char> = value.chars().collect();
    chars.windows(2).map(|w| (w[0], w[1])).collect()
}
/// Shared prefix dominates (model families share it), then bigram overlap.
fn suggestion_score(query: &str, candidate: &str) -> Option<i64> {
    let prefix = query
        .chars()
        .zip(candidate.chars())
        .take_while(|(a, b)| a == b)
        .count() as i64;
    let (left, mut right) = (bigrams(query), bigrams(candidate));
    let total = left.len() + right.len();
    let mut shared = 0;
    for pair in &left {
        if let Some(at) = right.iter().position(|other| other == pair) {
            right.swap_remove(at);
            shared += 1;
        }
    }
    let dice = if total == 0 {
        0
    } else {
        (shared * 2 * 100 / total) as i64
    };
    if prefix < 3 && dice < 50 {
        return None;
    }
    let length_gap = (query.chars().count() as i64 - candidate.chars().count() as i64).abs();
    Some(prefix * 100 + dice - length_gap)
}
pub(crate) fn model_price_suggestions(
    catalog: &crate::model_price_catalog::PricingCatalog,
    model: &str,
    at_unix: i64,
) -> Vec<ModelPriceSuggestion> {
    let query = suggestion_key(model);
    if query.is_empty() {
        return Vec::new();
    }
    let mut ranked: Vec<(i64, &str, &ModelPrice)> = catalog
        .effective_models(at_unix)
        .into_iter()
        .filter_map(|(key, price)| {
            suggestion_score(&query, &suggestion_key(key)).map(|score| (score, key, price))
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    ranked
        .into_iter()
        .filter_map(|(_, key, price)| {
            let (set, _) = crate::model_pricing::resolve_rates(
                price,
                ServiceTier::Standard,
                ContextTier::Base,
            )?;
            Some(ModelPriceSuggestion {
                model_key: key.to_string(),
                display_name: if price.display_name.is_empty() {
                    key.to_string()
                } else {
                    price.display_name.clone()
                },
                rates: (&OwnerRates {
                    input: set.input_micros_per_1m,
                    output: set.output_micros_per_1m,
                    cache_read: set.cache_read_micros_per_1m,
                    cache_write_5m: set.cache_write_5m_micros_per_1m,
                    cache_write_1h: set.cache_write_1h_micros_per_1m,
                })
                    .into(),
            })
        })
        .take(MAX_SUGGESTIONS)
        .collect()
}

/// One owner-price generation as the usage breakdown reads it.
pub(crate) struct OwnerPriceGeneration {
    pub effective_from_ms: i64,
    pub price: Option<ModelPrice>,
}
/// Every generation of an installation's owner prices, oldest first per model.
pub(crate) fn owner_price_generations(
    conn: &Connection,
    installation: &str,
) -> Result<HashMap<String, Vec<OwnerPriceGeneration>>, AppError> {
    let mut statement = conn
        .prepare(
            "SELECT model_key,action,display_name,input_micros_per_1m,output_micros_per_1m,
                    cache_read_micros_per_1m,cache_write_5m_micros_per_1m,cache_write_1h_micros_per_1m,
                    effective_from_ms
             FROM share_value_owner_model_price_history WHERE installation_id=?1
             ORDER BY model_key,effective_from_ms,id",
        )
        .map_err(db_error)?;
    let mut out: HashMap<String, Vec<OwnerPriceGeneration>> = HashMap::new();
    for row in statement
        .query_map(params![installation], |r| {
            let key: String = r.get(0)?;
            let action: String = r.get(1)?;
            let price = if action == "upsert" {
                let name: Option<String> = r.get(2)?;
                Some(owner_model_price(
                    &key,
                    name.as_deref().unwrap_or(&key),
                    &OwnerRates {
                        input: r.get(3)?,
                        output: r.get(4)?,
                        cache_read: r.get(5)?,
                        cache_write_5m: r.get(6)?,
                        cache_write_1h: r.get(7)?,
                    },
                ))
            } else {
                None
            };
            Ok((
                key,
                OwnerPriceGeneration {
                    effective_from_ms: r.get(8)?,
                    price,
                },
            ))
        })
        .map_err(db_error)?
    {
        let (key, generation) = row.map_err(db_error)?;
        out.entry(key).or_default().push(generation);
    }
    Ok(out)
}
/// The owner price in force at `at_ms`, if any.
pub(crate) fn owner_price_at<'a>(
    generations: &'a HashMap<String, Vec<OwnerPriceGeneration>>,
    key: &str,
    at_ms: i64,
) -> Option<&'a ModelPrice> {
    generations
        .get(key)?
        .iter()
        .rev()
        .find(|generation| generation.effective_from_ms <= at_ms)?
        .price
        .as_ref()
}

fn share_installation(conn: &Connection, share_id: &str) -> Result<String, AppError> {
    conn.query_row(
        "SELECT installation_id FROM shares WHERE share_id=?1",
        params![share_id],
        |r| r.get(0),
    )
    .optional()
    .map_err(db_error)?
    .ok_or_else(share_not_found)
}
fn next_price_version(conn: &Connection, installation: &str, key: &str) -> Result<i64, AppError> {
    // Versions never restart after a delete, so a stale editor cannot match a
    // re-created price by accident.
    let latest: Option<i64> = conn
        .query_row(
            "SELECT max(version) FROM share_value_owner_model_price_history WHERE installation_id=?1 AND model_key=?2",
            params![installation, key],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    latest
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| db_error("model price version exhausted"))
}
/// One installation's overlays for an owner-price edit, derived off the lock.
struct EditDerivation {
    bases: Vec<(&'static str, Arc<PricingSnapshot>)>,
    overlays: DerivedOverlays,
}

/// The price set an installation is metered with: its served global base
/// plus the owner prices filling models that base cannot resolve.
pub(crate) struct EffectivePrices {
    base: Arc<PricingSnapshot>,
    owner_filled: HashSet<String>,
}
impl EffectivePrices {
    /// The model key a wire model meters under and whether an owner price
    /// supplies it, in exactly the materialized overlay's
    /// [`PricingSnapshot::model`] candidate order: the overlay is the base
    /// plus the filled keys, which are never aliases of the base.
    pub(crate) fn resolve(&self, model: &str) -> Option<(String, bool)> {
        self.base.resolve(model, |key| {
            if self.base.models.contains_key(key) {
                Some((key.to_string(), false))
            } else if self.owner_filled.contains(key) {
                Some((key.to_string(), true))
            } else {
                None
            }
        })
    }
}

impl AppStore {
    pub(crate) async fn owner_model_prices(
        &self,
        share_id: &str,
    ) -> Result<(String, Vec<OwnerModelPrice>), AppError> {
        let (installation, prices, base) = {
            let conn = self.conn.lock().await;
            let installation = share_installation(&conn, share_id)?;
            let prices = read_owner_prices(&conn, &installation)?;
            let base = if prices.is_empty() {
                None
            } else {
                GlobalBase::read_served(&conn, &installation)?
            };
            (installation, prices, base)
        };
        let base = match base {
            Some(base) => Some(base.snapshot_off_runtime().await?),
            None => None,
        };
        let views = prices
            .iter()
            .map(|price| {
                price.view(
                    base.as_deref()
                        .is_some_and(|base| shadowed_by(base, &price.model_key)),
                )
            })
            .collect();
        Ok((installation, views))
    }
    /// The installation's effective prices for the usage breakdown. Before
    /// any global publication the catalog's would-be tariff stands in.
    pub(crate) async fn installation_effective_prices(
        &self,
        installation: &str,
    ) -> Result<EffectivePrices, AppError> {
        let (prices, base) = {
            let conn = self.conn.lock().await;
            (
                read_owner_prices(&conn, installation)?,
                GlobalBase::read_served(&conn, installation)?,
            )
        };
        let base = match base {
            Some(base) => base.snapshot_off_runtime().await?,
            None => {
                let catalog = self.pricing_catalog();
                let key = format!("catalog:{}", catalog.revision());
                match cached_tariff(&key) {
                    Some(base) => base,
                    None => tokio::task::spawn_blocking(move || {
                        let base = Arc::new(
                            catalog
                                .execution_snapshot(Utc::now().timestamp())
                                .map_err(db_error)?,
                        );
                        remember_tariff(&key, &base);
                        Ok::<_, AppError>(base)
                    })
                    .await
                    .map_err(join_error)??,
                }
            }
        };
        let owner_filled = prices
            .into_iter()
            .map(|price| price.model_key)
            .filter(|key| !shadowed_by(&base, key))
            .collect();
        Ok(EffectivePrices { base, owner_filled })
    }
    /// Reads the installation's prices as they will be after `edit`, then
    /// derives its overlays on a blocking thread, outside the DB lock.
    async fn derive_owner_edit(
        &self,
        share_id: &str,
        edit: impl FnOnce(&mut Vec<OwnerPriceRow>),
    ) -> Result<EditDerivation, AppError> {
        let (bases, input) = {
            let conn = self.conn.lock().await;
            let installation = share_installation(&conn, share_id)?;
            let mut prices = read_owner_prices(&conn, &installation)?;
            edit(&mut prices);
            // The order `read_owner_prices` returns, so the digests match.
            prices.sort_by(|a, b| a.model_key.cmp(&b.model_key));
            (
                GlobalBase::read_all(&conn)?,
                read_inputs(&conn, installation, prices)?,
            )
        };
        let (bases, overlays) = tokio::task::spawn_blocking(move || {
            derive_overlays(&bases, std::slice::from_ref(&input), true)
        })
        .await
        .map_err(join_error)??;
        Ok(EditDerivation { bases, overlays })
    }
    /// Creates (`expected_version` absent) or replaces (optimistic CAS) one
    /// owner price for a model the catalog does not price, then writes this
    /// installation's tariff in the same short transaction from an overlay
    /// derived beforehand; a publication or edit racing the derivation is
    /// retried. Returns the price and when it takes effect.
    pub(crate) async fn upsert_owner_model_price(
        &self,
        share_id: &str,
        edit: OwnerPriceEdit,
        actor: &str,
    ) -> Result<(OwnerModelPrice, i64), AppError> {
        let key = normalize_owner_model_key(&edit.model_key)?;
        let name = display_name(edit.display_name.as_deref(), &key)?;
        validate_owner_rates(&edit.rates, &self.pricing_catalog(), Utc::now().timestamp())?;
        let rates = edit.rates;
        for _ in 0..DERIVE_ATTEMPTS {
            let derivation = self
                .derive_owner_edit(share_id, |prices| {
                    prices.retain(|price| price.model_key != key);
                    prices.push(OwnerPriceRow {
                        model_key: key.clone(),
                        display_name: name.clone(),
                        rates,
                        version: 0,
                        updated_at_ms: 0,
                        updated_by: String::new(),
                    });
                })
                .await?;
            // Nothing to overlay before the first publication, and nothing to
            // check the model against.
            if derivation.bases.is_empty() {
                return Err(price_invalid("catalog_unavailable", serde_json::json!({})));
            }
            if derivation
                .bases
                .iter()
                .any(|(_, base)| shadowed_by(base, &key))
            {
                return Err(price_invalid(
                    "catalog_priced",
                    serde_json::json!({"field": "modelKey"}),
                ));
            }
            let now = Utc::now().timestamp_millis();
            let conn = self.conn.lock().await;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(db_error)?;
            let installation = share_installation(&tx, share_id)?;
            let current: Option<i64> = tx
                .query_row(
                    "SELECT version FROM share_value_owner_model_prices WHERE installation_id=?1 AND model_key=?2",
                    params![installation, key],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db_error)?;
            if edit.expected_version != current {
                return Err(version_conflict(current));
            }
            if current.is_none() {
                let count: i64 = tx
                    .query_row(
                        "SELECT count(*) FROM share_value_owner_model_prices WHERE installation_id=?1",
                        params![installation],
                        |r| r.get(0),
                    )
                    .map_err(db_error)?;
                if count >= MAX_OWNER_PRICES as i64 {
                    return Err(price_invalid(
                        "too_many_prices",
                        serde_json::json!({"max": MAX_OWNER_PRICES}),
                    ));
                }
            }
            let version = next_price_version(&tx, &installation, &key)?;
            tx.execute(
                "INSERT INTO share_value_owner_model_prices VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?10,?11)
                 ON CONFLICT(installation_id,model_key) DO UPDATE SET display_name=excluded.display_name,
                 input_micros_per_1m=excluded.input_micros_per_1m,output_micros_per_1m=excluded.output_micros_per_1m,
                 cache_read_micros_per_1m=excluded.cache_read_micros_per_1m,
                 cache_write_5m_micros_per_1m=excluded.cache_write_5m_micros_per_1m,
                 cache_write_1h_micros_per_1m=excluded.cache_write_1h_micros_per_1m,version=excluded.version,
                 effective_from_ms=excluded.effective_from_ms,updated_at_ms=excluded.updated_at_ms,updated_by=excluded.updated_by",
                params![
                    installation,
                    key,
                    name,
                    rates.input,
                    rates.output,
                    rates.cache_read,
                    rates.cache_write_5m,
                    rates.cache_write_1h,
                    version,
                    now,
                    actor
                ],
            )
            .map_err(db_error)?;
            // Re-checks the base revision and owner prices against the
            // derivation; a race rolls this attempt back and derives again.
            let effective = match write_installation(&tx, &installation, &derivation.overlays, now)
            {
                Err(error) if is_tariff_changed(&error) => continue,
                result => result?.unwrap_or(now),
            };
            tx.execute(
                "UPDATE share_value_owner_model_prices SET effective_from_ms=?3 WHERE installation_id=?1 AND model_key=?2",
                params![installation, key, effective],
            )
            .map_err(db_error)?;
            tx.execute(
                "INSERT INTO share_value_owner_model_price_history(installation_id,model_key,action,display_name,
                    input_micros_per_1m,output_micros_per_1m,cache_read_micros_per_1m,cache_write_5m_micros_per_1m,
                    cache_write_1h_micros_per_1m,version,effective_from_ms,actor,created_at_ms)
                 VALUES(?1,?2,'upsert',?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![
                    installation,
                    key,
                    name,
                    rates.input,
                    rates.output,
                    rates.cache_read,
                    rates.cache_write_5m,
                    rates.cache_write_1h,
                    version,
                    effective,
                    actor,
                    now
                ],
            )
            .map_err(db_error)?;
            let price = tx
                .query_row(
                    &format!("SELECT {PRICE_COLUMNS} FROM share_value_owner_model_prices WHERE installation_id=?1 AND model_key=?2"),
                    params![installation, key],
                    price_row,
                )
                .map_err(db_error)?;
            tx.commit().map_err(db_error)?;
            return Ok((price.view(false), effective));
        }
        Err(tariff_changed())
    }
    pub(crate) async fn delete_owner_model_price(
        &self,
        share_id: &str,
        model_key: &str,
        expected_version: i64,
        actor: &str,
    ) -> Result<i64, AppError> {
        let key = normalize_owner_model_key(model_key)?;
        for _ in 0..DERIVE_ATTEMPTS {
            let derivation = self
                .derive_owner_edit(share_id, |prices| {
                    prices.retain(|price| price.model_key != key)
                })
                .await?;
            let now = Utc::now().timestamp_millis();
            let conn = self.conn.lock().await;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(db_error)?;
            let installation = share_installation(&tx, share_id)?;
            let current: Option<i64> = tx
                .query_row(
                    "SELECT version FROM share_value_owner_model_prices WHERE installation_id=?1 AND model_key=?2",
                    params![installation, key],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db_error)?;
            if current != Some(expected_version) {
                return Err(version_conflict(current));
            }
            let version = next_price_version(&tx, &installation, &key)?;
            tx.execute(
                "DELETE FROM share_value_owner_model_prices WHERE installation_id=?1 AND model_key=?2",
                params![installation, key],
            )
            .map_err(db_error)?;
            let effective = match write_installation(&tx, &installation, &derivation.overlays, now)
            {
                Err(error) if is_tariff_changed(&error) => continue,
                result => result?.unwrap_or(now),
            };
            tx.execute(
                "INSERT INTO share_value_owner_model_price_history(installation_id,model_key,action,version,effective_from_ms,actor,created_at_ms)
                 VALUES(?1,?2,'delete',?3,?4,?5,?6)",
                params![installation, key, version, effective, actor, now],
            )
            .map_err(db_error)?;
            tx.commit().map_err(db_error)?;
            return Ok(effective);
        }
        Err(tariff_changed())
    }
    /// Re-derives every owner-price overlay after a global publication. The
    /// multi-MiB work never holds the DB lock or the async runtime: inputs are
    /// read under a short lock, overlays derived on a blocking thread, then
    /// each installation is written in its own savepoint after re-checking
    /// its base revision and owner prices. A failing or raced overlay is
    /// rolled back to its previous revision and logged; the next sync (and
    /// the next owner edit) derives it again.
    pub(crate) async fn rederive_owner_tariffs(&self) -> Result<(), AppError> {
        let (bases, inputs) = {
            let conn = self.conn.lock().await;
            let installations: Vec<String> = {
                let mut statement = conn
                    .prepare(
                        "SELECT installation_id FROM share_value_owner_model_prices
                         UNION SELECT installation_id FROM share_value_pricing_installation_tariffs",
                    )
                    .map_err(db_error)?;
                let rows = statement
                    .query_map([], |r| r.get(0))
                    .map_err(db_error)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(db_error)?;
                rows
            };
            if installations.is_empty() {
                return Ok(());
            }
            let inputs = installations
                .into_iter()
                .map(|installation| {
                    let prices = read_owner_prices(&conn, &installation)?;
                    read_inputs(&conn, installation, prices)
                })
                .collect::<Result<Vec<_>, AppError>>()?;
            (GlobalBase::read_all(&conn)?, inputs)
        };
        let (inputs, overlays) = tokio::task::spawn_blocking(move || {
            let (_, overlays) = derive_overlays(&bases, &inputs, false)?;
            Ok::<_, AppError>((inputs, overlays))
        })
        .await
        .map_err(join_error)??;
        let now = Utc::now().timestamp_millis();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        for InstallationInputs { installation, .. } in &inputs {
            tx.execute_batch("SAVEPOINT owner_tariff")
                .map_err(db_error)?;
            match write_installation(&tx, installation, &overlays, now) {
                Ok(_) => tx
                    .execute_batch("RELEASE SAVEPOINT owner_tariff")
                    .map_err(db_error)?,
                Err(error) => {
                    tx.execute_batch(
                        "ROLLBACK TO SAVEPOINT owner_tariff; RELEASE SAVEPOINT owner_tariff",
                    )
                    .map_err(db_error)?;
                    tracing::warn!(%installation, %error, "installation USD quota tariff kept its previous revision");
                }
            }
        }
        tx.commit().map_err(db_error)
    }
    pub(crate) fn owner_model_price_suggestions(&self, model: &str) -> Vec<ModelPriceSuggestion> {
        model_price_suggestions(&self.pricing_catalog(), model, Utc::now().timestamp())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usd_per1m_round_trips_the_admin_decimal_format() {
        assert_eq!(usd_per1m(0), "0");
        assert_eq!(usd_per1m(3_000_000), "3");
        assert_eq!(usd_per1m(1_250_000), "1.25");
        assert_eq!(usd_per1m(1), "0.000001");
    }

    #[test]
    fn suggestion_keys_ignore_separators_case_and_provider_qualifiers() {
        assert_eq!(suggestion_key("kimi-k3-1"), "kimik31");
        assert_eq!(suggestion_key("Moonshot/Kimi-K3.1_Pro"), "kimik31pro");
        assert_eq!(
            suggestion_key("anthropic.claude-sonnet-4-5"),
            "claudesonnet45"
        );
        assert!(suggestion_score("kimik31", "kimik31pro") > suggestion_score("kimik31", "kimik3"));
        assert!(suggestion_score("kimik31", "kimik3") > suggestion_score("kimik31", "kimik2"));
        assert_eq!(suggestion_score("kimik31", "gpt5"), None);
    }
}
