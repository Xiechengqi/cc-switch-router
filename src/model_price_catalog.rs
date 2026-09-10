//! Loads the repository-owned price catalog into the database and serves an
//! in-process snapshot of it.
//!
//! `pricing/model-prices.json` is a repository-owned artefact DERIVED from
//! LiteLLM by `scripts/pricing/derive-model-prices.mjs` — not a vendored copy.
//! `SOURCE_PROVENANCE.json` policy (`runtimeAndBuildInputsMustBeRepositoryOwned`)
//! forbids vendoring a file that drives computation, so the derivation is
//! reviewed as a diff and committed as our own
//! (`docs/design-share-user-model-usage-and-pricing.md` §5.3).
//!
//! The embedded document is compiled in with `include_str!`, matching the
//! existing convention for `regions` (`src/api.rs:124`) and `schema/*.sql`
//! (`src/schema.rs:12`).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::db::{Connection, TransactionBehavior, params};
use crate::error::AppError;
use crate::model_pricing::{ContextTier, MicrosPer1M, ModelPrice, RateSet, ServiceTier};

const EMBEDDED_CATALOG: &str = include_str!("../pricing/model-prices.json");

/// The derived generation covers all of history: re-deriving replaces it in
/// place so that correcting a price retroactively corrects every displayed
/// amount. Admin overrides use their own `effective_from` and are never touched
/// by the loader (§5.4 rule 3).
const DERIVED_EFFECTIVE_FROM: i64 = 0;

const SUPPORTED_SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Embedded document
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CatalogDocument {
    schema_version: u32,
    revision: String,
    derived_at: String,
    source: CatalogSource,
    models: Vec<CatalogModel>,
    #[serde(default)]
    aliases: Vec<CatalogAlias>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CatalogSource {
    name: String,
    url: String,
    sha256: String,
    license: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CatalogModel {
    price_key: String,
    #[serde(default)]
    display_name: String,
    long_context_threshold: Option<u64>,
    #[serde(default)]
    long_context_inclusive: bool,
    #[serde(default)]
    supports_cache_breakdown: bool,
    #[serde(default)]
    source_note: String,
    rates: Vec<CatalogRate>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CatalogRate {
    service_tier: String,
    context_tier: String,
    /// Prices travel as decimal strings so no JSON parser ever rounds them.
    input: String,
    output: String,
    cache_read: String,
    cache_write5m: String,
    cache_write1h: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CatalogAlias {
    pattern: String,
    match_kind: String,
    price_key: String,
    #[serde(default)]
    priority: i64,
}

fn parse_micros(field: &str, raw: &str) -> Result<MicrosPer1M, AppError> {
    raw.trim().parse::<MicrosPer1M>().map_err(|error| {
        AppError::Internal(format!(
            "model price catalog: field `{field}` is not an integer micro-USD value ({raw}): {error}"
        ))
    })
}

fn parse_document(raw: &str) -> Result<CatalogDocument, AppError> {
    let document: CatalogDocument = serde_json::from_str(raw).map_err(|error| {
        AppError::Internal(format!("model price catalog is not parseable: {error}"))
    })?;
    if document.schema_version != SUPPORTED_SCHEMA_VERSION {
        return Err(AppError::Internal(format!(
            "model price catalog schema version {} is unsupported (expected {SUPPORTED_SCHEMA_VERSION})",
            document.schema_version
        )));
    }
    if document.models.is_empty() {
        return Err(AppError::Internal(
            "model price catalog contains no models".into(),
        ));
    }
    Ok(document)
}

// ---------------------------------------------------------------------------
// In-process snapshot
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Generation {
    effective_from: i64,
    effective_to: Option<i64>,
    price: ModelPrice,
}

#[derive(Debug, Default)]
pub struct PricingCatalog {
    /// Full lowercase SHA-256 hex of the canonicalised effective price set.
    revision: String,
    /// price_key -> generations, ascending by `effective_from`.
    models: BTreeMap<String, Vec<Generation>>,
    exact_aliases: HashMap<String, String>,
    /// `(pattern, price_key)` pre-sorted by priority desc then pattern length
    /// desc, so the first prefix match wins (§6.2 rule 3 — otherwise `claude-`
    /// would steal every `claude-sonnet-4-5` request).
    prefix_aliases: Vec<(String, String)>,
}

impl PricingCatalog {
    pub fn revision(&self) -> &str {
        &self.revision
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    pub fn model_count(&self) -> usize {
        self.models.len()
    }

    /// Maps observed model text to a catalog key (§6.2).
    ///
    /// Deliberately NOT a fuzzy family guess in the style of new-api's
    /// `strings.Contains` matching: a wrong price is worse than no price.
    pub fn resolve_price_key(&self, model_text: &str) -> Option<&str> {
        let needle = model_text.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return None;
        }
        if let Some(key) = self.exact_aliases.get(&needle) {
            return self.models.get_key_value(key).map(|(key, _)| key.as_str());
        }
        if let Some((key, _)) = self.models.get_key_value(&needle) {
            return Some(key.as_str());
        }
        self.prefix_aliases
            .iter()
            .find(|(pattern, key)| needle.starts_with(pattern.as_str()) && self.models.contains_key(key))
            .and_then(|(_, key)| self.models.get_key_value(key).map(|(key, _)| key.as_str()))
    }

    /// The price generation in force at `at_unix` (§8.3: prices are read at the
    /// window end, so history keeps the price that was actually effective).
    pub fn price_at(&self, price_key: &str, at_unix: i64) -> Option<&ModelPrice> {
        self.models
            .get(price_key)?
            .iter()
            .rev()
            .find(|generation| {
                generation.effective_from <= at_unix
                    && generation
                        .effective_to
                        .is_none_or(|until| until > at_unix)
            })
            .map(|generation| &generation.price)
    }

    /// Convenience for the two-step "resolve identity, then price" path.
    pub fn lookup(&self, model_text: &str, at_unix: i64) -> Option<(&str, &ModelPrice)> {
        let key = self.resolve_price_key(model_text)?;
        let price = self.price_at(key, at_unix)?;
        Some((key, price))
    }

    /// Every model effective at `at_unix`, for the public price card (§9.2).
    pub fn effective_models(&self, at_unix: i64) -> Vec<(&str, &ModelPrice)> {
        self.models
            .keys()
            .filter_map(|key| self.price_at(key, at_unix).map(|price| (key.as_str(), price)))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Load
// ---------------------------------------------------------------------------

/// Parses the embedded catalog, replaces the `source='derived'` rows in one
/// `BEGIN IMMEDIATE`, then returns the in-process snapshot (§5.4).
///
/// The snapshot is held by the `Store` rather than in a process global so that
/// each connection owns the catalog that matches its own database — a global
/// would make one test's load visible to another's in-memory database.
///
/// Fail-closed: any error aborts startup, mirroring the baseline-checksum
/// stance in `src/schema.rs`. A silently half-loaded price catalog would show
/// plausible-looking numbers that are wrong.
pub fn load(conn: &Connection, now_unix: i64) -> Result<Arc<PricingCatalog>, AppError> {
    let document = parse_document(EMBEDDED_CATALOG)?;
    write_derived_rows(conn, &document, now_unix)?;
    let catalog = Arc::new(read_snapshot(conn)?);
    tracing::info!(
        revision = %catalog.revision,
        models = catalog.models.len(),
        derived_revision = %document.revision,
        derived_at = %document.derived_at,
        source = %document.source.name,
        source_sha256 = %document.source.sha256,
        source_license = %document.source.license,
        source_url = %document.source.url,
        "loaded model price catalog"
    );
    Ok(catalog)
}

fn write_derived_rows(
    conn: &Connection,
    document: &CatalogDocument,
    now_unix: i64,
) -> Result<(), AppError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| AppError::Internal(format!("begin price catalog load failed: {error}")))?;

    // Wholesale replace of the derived generation. Idempotent, and the
    // `source` predicate is what keeps admin overrides alive across upgrades.
    // Rates cascade from the header delete.
    tx.execute(
        "DELETE FROM model_price_catalog WHERE source = 'derived'",
        params![],
    )
    .map_err(|error| AppError::Internal(format!("clear derived price catalog failed: {error}")))?;
    tx.execute(
        "DELETE FROM model_price_aliases WHERE source = 'derived'",
        params![],
    )
    .map_err(|error| AppError::Internal(format!("clear derived price aliases failed: {error}")))?;

    for model in &document.models {
        let price_key = model.price_key.trim().to_ascii_lowercase();
        if price_key.is_empty() {
            return Err(AppError::Internal(
                "model price catalog contains an empty priceKey".into(),
            ));
        }
        tx.execute(
            "INSERT INTO model_price_catalog (
                 price_key, effective_from, effective_to, display_name, currency,
                 long_context_threshold, long_context_inclusive, supports_cache_breakdown,
                 source, source_note, updated_at
             ) VALUES (?1, ?2, NULL, ?3, 'USD', ?4, ?5, ?6, 'derived', ?7, ?8)",
            params![
                price_key.as_str(),
                DERIVED_EFFECTIVE_FROM,
                model.display_name.as_str(),
                model.long_context_threshold.map(|value| value as i64),
                i64::from(model.long_context_inclusive),
                i64::from(model.supports_cache_breakdown),
                model.source_note.as_str(),
                now_unix,
            ],
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "insert price catalog entry `{price_key}` failed: {error}"
            ))
        })?;

        if model.rates.is_empty() {
            return Err(AppError::Internal(format!(
                "model price catalog entry `{price_key}` carries no rates"
            )));
        }

        for rate in &model.rates {
            let input = parse_micros("input", &rate.input)?;
            let output = parse_micros("output", &rate.output)?;
            let cache_read = parse_micros("cacheRead", &rate.cache_read)?;
            let cache_write_5m = parse_micros("cacheWrite5m", &rate.cache_write5m)?;
            let cache_write_1h = rate
                .cache_write1h
                .as_deref()
                .map(|value| parse_micros("cacheWrite1h", value))
                .transpose()?;
            tx.execute(
                "INSERT INTO model_price_rates (
                     price_key, effective_from, service_tier, context_tier,
                     input_micros_per_1m, output_micros_per_1m, cache_read_micros_per_1m,
                     cache_write_5m_micros_per_1m, cache_write_1h_micros_per_1m
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    price_key.as_str(),
                    DERIVED_EFFECTIVE_FROM,
                    rate.service_tier.as_str(),
                    rate.context_tier.as_str(),
                    input,
                    output,
                    cache_read,
                    cache_write_5m,
                    cache_write_1h,
                ],
            )
            .map_err(|error| {
                AppError::Internal(format!(
                    "insert price rate `{price_key}/{}/{}` failed: {error}",
                    rate.service_tier, rate.context_tier
                ))
            })?;
        }
    }

    for alias in &document.aliases {
        tx.execute(
            "INSERT INTO model_price_aliases (pattern, match_kind, price_key, priority, source, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'derived', ?5)",
            params![
                alias.pattern.trim().to_ascii_lowercase(),
                alias.match_kind.as_str(),
                alias.price_key.trim().to_ascii_lowercase(),
                alias.priority,
                now_unix,
            ],
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "insert price alias `{}` failed: {error}",
                alias.pattern
            ))
        })?;
    }

    tx.commit()
        .map_err(|error| AppError::Internal(format!("commit price catalog load failed: {error}")))
}

/// Rebuilds the in-process snapshot from the database, so admin overrides
/// participate on exactly the same footing as derived rows.
fn read_snapshot(conn: &Connection) -> Result<PricingCatalog, AppError> {
    let mut headers = conn
        .prepare(
            "SELECT price_key, effective_from, effective_to, display_name,
                    long_context_threshold, long_context_inclusive, supports_cache_breakdown
             FROM model_price_catalog
             ORDER BY price_key ASC, effective_from ASC",
        )
        .map_err(|error| AppError::Internal(format!("prepare price catalog read failed: {error}")))?;
    let header_rows = headers
        .query_map(params![], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })
        .map_err(|error| AppError::Internal(format!("read price catalog failed: {error}")))?;

    let mut models: BTreeMap<String, Vec<Generation>> = BTreeMap::new();
    for row in header_rows {
        let (price_key, effective_from, effective_to, display_name, threshold, inclusive, cache_breakdown) =
            row.map_err(|error| {
                AppError::Internal(format!("read price catalog row failed: {error}"))
            })?;
        models
            .entry(price_key.clone())
            .or_default()
            .push(Generation {
                effective_from,
                effective_to,
                price: ModelPrice {
                    price_key,
                    display_name,
                    long_context_threshold: threshold.and_then(|value| u64::try_from(value).ok()),
                    long_context_inclusive: inclusive != 0,
                    supports_cache_breakdown: cache_breakdown != 0,
                    rates: BTreeMap::new(),
                },
            });
    }

    let mut rates = conn
        .prepare(
            "SELECT price_key, effective_from, service_tier, context_tier,
                    input_micros_per_1m, output_micros_per_1m, cache_read_micros_per_1m,
                    cache_write_5m_micros_per_1m, cache_write_1h_micros_per_1m
             FROM model_price_rates
             ORDER BY price_key ASC, effective_from ASC, service_tier ASC, context_tier ASC",
        )
        .map_err(|error| AppError::Internal(format!("prepare price rate read failed: {error}")))?;
    let rate_rows = rates
        .query_map(params![], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, Option<i64>>(8)?,
            ))
        })
        .map_err(|error| AppError::Internal(format!("read price rates failed: {error}")))?;

    for row in rate_rows {
        let (price_key, effective_from, tier, ctx, input, output, cache_read, write_5m, write_1h) =
            row.map_err(|error| {
                AppError::Internal(format!("read price rate row failed: {error}"))
            })?;
        // The schema CHECKs already constrain these columns; an unrecognised
        // value here means the table was written outside the loader, so drop
        // the row rather than guess a tier.
        let (Some(tier), Some(ctx)) = (parse_service_tier(&tier), parse_context_tier(&ctx)) else {
            continue;
        };
        let Some(generations) = models.get_mut(&price_key) else {
            continue;
        };
        let Some(generation) = generations
            .iter_mut()
            .find(|generation| generation.effective_from == effective_from)
        else {
            continue;
        };
        generation.price.rates.insert(
            (tier, ctx),
            RateSet {
                input_micros_per_1m: input,
                output_micros_per_1m: output,
                cache_read_micros_per_1m: cache_read,
                cache_write_5m_micros_per_1m: write_5m,
                cache_write_1h_micros_per_1m: write_1h,
            },
        );
    }

    // A header with no rates can never produce an amount; keeping it would only
    // let `resolve_price_key` succeed and then `price_usage` return None, which
    // reads as "unpriced" anyway. Drop it so the counts stay honest.
    for generations in models.values_mut() {
        generations.retain(|generation| !generation.price.rates.is_empty());
    }
    models.retain(|_, generations| !generations.is_empty());

    let mut aliases = conn
        .prepare(
            "SELECT pattern, match_kind, price_key
             FROM model_price_aliases
             ORDER BY priority DESC, length(pattern) DESC, pattern ASC",
        )
        .map_err(|error| AppError::Internal(format!("prepare price alias read failed: {error}")))?;
    let alias_rows = aliases
        .query_map(params![], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| AppError::Internal(format!("read price aliases failed: {error}")))?;

    let mut exact_aliases = HashMap::new();
    let mut prefix_aliases = Vec::new();
    for row in alias_rows {
        let (pattern, kind, price_key) = row.map_err(|error| {
            AppError::Internal(format!("read price alias row failed: {error}"))
        })?;
        match kind.as_str() {
            // §6.2 rule 1: exact wins over prefix, so they live in separate
            // structures and are consulted in order.
            "exact" => {
                exact_aliases.entry(pattern).or_insert(price_key);
            }
            "prefix" => prefix_aliases.push((pattern, price_key)),
            _ => continue,
        }
    }

    let revision = compute_revision(&models, &exact_aliases, &prefix_aliases);
    Ok(PricingCatalog {
        revision,
        models,
        exact_aliases,
        prefix_aliases,
    })
}

fn parse_service_tier(raw: &str) -> Option<ServiceTier> {
    match raw {
        "standard" => Some(ServiceTier::Standard),
        "priority" => Some(ServiceTier::Priority),
        "flex" => Some(ServiceTier::Flex),
        _ => None,
    }
}

fn parse_context_tier(raw: &str) -> Option<ContextTier> {
    match raw {
        "base" => Some(ContextTier::Base),
        "long" => Some(ContextTier::Long),
        _ => None,
    }
}

/// SHA-256 over a canonical rendering of the whole effective price set.
///
/// Any price, threshold or alias change moves the revision, which is what the
/// public endpoint's cache key and the owner response's `pricingRevision` are
/// built on. Field order is fixed here rather than inherited from a serializer
/// so the digest cannot drift with a serde attribute change.
fn compute_revision(
    models: &BTreeMap<String, Vec<Generation>>,
    exact_aliases: &HashMap<String, String>,
    prefix_aliases: &[(String, String)],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"model-price-catalog\x1fv1\x1e");
    for (price_key, generations) in models {
        for generation in generations {
            hasher.update(price_key.as_bytes());
            hasher.update(b"\x1f");
            hasher.update(generation.effective_from.to_string().as_bytes());
            hasher.update(b"\x1f");
            hasher.update(
                generation
                    .effective_to
                    .map(|value| value.to_string())
                    .unwrap_or_default()
                    .as_bytes(),
            );
            hasher.update(b"\x1f");
            hasher.update(
                generation
                    .price
                    .long_context_threshold
                    .map(|value| value.to_string())
                    .unwrap_or_default()
                    .as_bytes(),
            );
            hasher.update(b"\x1f");
            hasher.update(if generation.price.long_context_inclusive {
                b"1"
            } else {
                b"0"
            });
            for ((tier, ctx), rates) in &generation.price.rates {
                hasher.update(b"\x1f");
                hasher.update(tier.as_str().as_bytes());
                hasher.update(b":");
                hasher.update(ctx.as_str().as_bytes());
                for value in [
                    rates.input_micros_per_1m,
                    rates.output_micros_per_1m,
                    rates.cache_read_micros_per_1m,
                    rates.cache_write_5m_micros_per_1m,
                    rates.cache_write_1h_micros_per_1m.unwrap_or(-1),
                ] {
                    hasher.update(b":");
                    hasher.update(value.to_string().as_bytes());
                }
            }
            hasher.update(b"\x1e");
        }
    }
    let mut exact: Vec<_> = exact_aliases.iter().collect();
    exact.sort();
    for (pattern, price_key) in exact {
        hasher.update(b"exact\x1f");
        hasher.update(pattern.as_bytes());
        hasher.update(b"\x1f");
        hasher.update(price_key.as_bytes());
        hasher.update(b"\x1e");
    }
    for (pattern, price_key) in prefix_aliases {
        hasher.update(b"prefix\x1f");
        hasher.update(pattern.as_bytes());
        hasher.update(b"\x1f");
        hasher.update(price_key.as_bytes());
        hasher.update(b"\x1e");
    }
    format!("{:x}", hasher.finalize())
}

/// `sha256:<full hex>` for the owner surface (§9.1).
pub fn qualified_revision(revision: &str) -> String {
    if revision.is_empty() {
        return String::new();
    }
    format!("sha256:{revision}")
}

/// Short form for the public surface's cache key (§9.2).
pub fn short_revision(revision: &str) -> String {
    revision.chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_pricing::{TokenSplit, price_usage};

    fn fixture_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory database");
        conn.pragma_update(None, "foreign_keys", "ON")
            .expect("enable foreign keys");
        crate::schema::apply(&conn).expect("apply schema");
        conn
    }

    #[test]
    fn embedded_document_parses() {
        let document = parse_document(EMBEDDED_CATALOG).expect("parse embedded catalog");
        assert_eq!(document.schema_version, SUPPORTED_SCHEMA_VERSION);
        assert!(document.models.len() > 100, "{}", document.models.len());
        assert_eq!(document.source.license, "MIT");
        for model in &document.models {
            assert_eq!(model.price_key, model.price_key.to_ascii_lowercase());
            assert!(!model.rates.is_empty(), "{} has no rates", model.price_key);
            // Every model must carry the standard/base row: it is the last rung
            // of the §5.2 fallback chain, so its absence would make some tier
            // combinations unpriceable.
            assert!(
                model
                    .rates
                    .iter()
                    .any(|rate| rate.service_tier == "standard" && rate.context_tier == "base"),
                "{} lacks standard/base",
                model.price_key
            );
        }
    }

    #[test]
    fn load_is_idempotent_and_matches_the_document() {
        let conn = fixture_conn();
        let document = parse_document(EMBEDDED_CATALOG).expect("parse");
        let first = load(&conn, 1_757_462_400).expect("first load");
        let second = load(&conn, 1_757_462_500).expect("second load");
        assert_eq!(first.revision(), second.revision());
        assert_eq!(first.model_count(), document.models.len());

        let headers: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM model_price_catalog WHERE source = 'derived'",
                params![],
                |row| row.get(0),
            )
            .expect("count headers");
        assert_eq!(headers as usize, document.models.len());
        let rates: i64 = conn
            .query_row("SELECT COUNT(*) FROM model_price_rates", params![], |row| {
                row.get(0)
            })
            .expect("count rates");
        let expected_rates: usize = document.models.iter().map(|model| model.rates.len()).sum();
        assert_eq!(rates as usize, expected_rates);
    }

    #[test]
    fn load_never_touches_admin_rows() {
        let conn = fixture_conn();
        load(&conn, 1_757_462_400).expect("initial load");
        conn.execute(
            "INSERT INTO model_price_catalog (
                 price_key, effective_from, display_name, source, updated_at
             ) VALUES ('house-model', 100, 'House Model', 'admin', 100)",
            params![],
        )
        .expect("insert admin header");
        conn.execute(
            "INSERT INTO model_price_rates (
                 price_key, effective_from, service_tier, context_tier,
                 input_micros_per_1m, output_micros_per_1m,
                 cache_read_micros_per_1m, cache_write_5m_micros_per_1m
             ) VALUES ('house-model', 100, 'standard', 'base', 1000000, 2000000, 100000, 1250000)",
            params![],
        )
        .expect("insert admin rate");

        let reloaded = load(&conn, 1_757_462_500).expect("reload");
        let survived: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM model_price_catalog WHERE source = 'admin'",
                params![],
                |row| row.get(0),
            )
            .expect("count admin rows");
        assert_eq!(survived, 1);
        assert!(reloaded.price_at("house-model", 1_757_462_500).is_some());
        // Before its effective_from the admin entry does not exist yet.
        assert!(reloaded.price_at("house-model", 99).is_none());
    }

    #[test]
    fn revision_moves_when_a_price_moves() {
        let conn = fixture_conn();
        let before = load(&conn, 1_757_462_400).expect("load").revision().to_string();
        conn.execute(
            "UPDATE model_price_rates SET output_micros_per_1m = output_micros_per_1m + 1
             WHERE price_key = (SELECT MIN(price_key) FROM model_price_rates)",
            params![],
        )
        .expect("bump a price");
        let after = read_snapshot(&conn).expect("re-read").revision;
        assert_ne!(before, after);
        assert_eq!(before.len(), 64);
        assert_eq!(short_revision(&before).len(), 12);
        assert_eq!(qualified_revision(&before), format!("sha256:{before}"));
    }

    #[test]
    fn resolves_a_known_model_and_prices_it() {
        let conn = fixture_conn();
        let catalog = load(&conn, 1_757_462_400).expect("load");
        let (key, price) = catalog
            .lookup("  Claude-3-7-Sonnet-20250219 ", 1_757_462_400)
            .expect("resolve claude 3.7 sonnet");
        assert_eq!(key, "claude-3-7-sonnet-20250219");
        let priced = price_usage(
            price,
            ServiceTier::Standard,
            &TokenSplit {
                input: 1_000_000,
                ..TokenSplit::default()
            },
            0,
        )
        .expect("price");
        assert_eq!(priced.total_micros, 3_000_000);
    }

    #[test]
    fn unknown_model_resolves_to_nothing_rather_than_a_guess() {
        let conn = fixture_conn();
        let catalog = load(&conn, 1_757_462_400).expect("load");
        assert!(catalog.resolve_price_key("totally-made-up-model").is_none());
        assert!(catalog.resolve_price_key("").is_none());
        assert!(catalog.resolve_price_key("   ").is_none());
    }

    #[test]
    fn prefix_aliases_prefer_the_longest_pattern() {
        let conn = fixture_conn();
        load(&conn, 1_757_462_400).expect("load");
        conn.execute(
            "INSERT INTO model_price_aliases (pattern, match_kind, price_key, priority, source, updated_at)
             VALUES ('claude-', 'prefix', 'claude-3-7-sonnet-20250219', 0, 'admin', 1)",
            params![],
        )
        .expect("insert broad alias");
        conn.execute(
            "INSERT INTO model_price_aliases (pattern, match_kind, price_key, priority, source, updated_at)
             VALUES ('claude-3-7-sonnet-thinking', 'prefix', 'claude-3-7-sonnet-20250219', 0, 'admin', 1)",
            params![],
        )
        .expect("insert specific alias");
        let catalog = read_snapshot(&conn).expect("re-read");
        assert_eq!(
            catalog.resolve_price_key("claude-3-7-sonnet-thinking-2025"),
            Some("claude-3-7-sonnet-20250219")
        );
        assert_eq!(
            catalog.prefix_aliases.first().map(|(pattern, _)| pattern.as_str()),
            Some("claude-3-7-sonnet-thinking")
        );
    }

    #[test]
    fn exact_alias_beats_prefix_alias() {
        let conn = fixture_conn();
        load(&conn, 1_757_462_400).expect("load");
        conn.execute(
            "INSERT INTO model_price_aliases (pattern, match_kind, price_key, priority, source, updated_at)
             VALUES ('gpt-5-codex-mini', 'prefix', 'claude-3-7-sonnet-20250219', 9, 'admin', 1)",
            params![],
        )
        .expect("insert prefix alias");
        conn.execute(
            "INSERT INTO model_price_aliases (pattern, match_kind, price_key, priority, source, updated_at)
             VALUES ('gpt-5-codex-mini', 'exact', 'claude-3-7-sonnet-20250219', 0, 'admin', 1)",
            params![],
        )
        .expect("insert exact alias");
        let catalog = read_snapshot(&conn).expect("re-read");
        assert!(catalog.exact_aliases.contains_key("gpt-5-codex-mini"));
    }

    #[test]
    fn rejects_a_document_with_a_bad_schema_version() {
        let raw = r#"{"schemaVersion":99,"revision":"x","derivedAt":"2026-01-01",
            "source":{"name":"n","url":"u","sha256":"s","license":"MIT"},
            "models":[],"aliases":[]}"#;
        assert!(parse_document(raw).is_err());
    }

    #[test]
    fn rejects_an_empty_document() {
        let raw = r#"{"schemaVersion":1,"revision":"x","derivedAt":"2026-01-01",
            "source":{"name":"n","url":"u","sha256":"s","license":"MIT"},
            "models":[],"aliases":[]}"#;
        assert!(parse_document(raw).is_err());
    }
}
