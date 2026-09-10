//! Pure pricing kernel: no IO, no database, no clock.
//!
//! Turns a token split plus a catalog entry into an official-price-equivalent
//! amount in micro-USD. Everything accumulates in `i128`; there is no floating
//! point anywhere on this path (`docs/design-share-user-model-usage-and-pricing.md`
//! §7.2 rule 1 — libSQL has no decimal type, so fixed point is the only place
//! rounding stays auditable).
//!
//! Boundaries this module deliberately does not cross (§2):
//!   * R1 — amounts produced here are display-only and must never reach
//!     `market_accrual_entries` / `market_invoices` / any billing snapshot.
//!   * R3 — the Server never computes cost or USD; this lives on the Router.
//!
//! Split off from transport and storage in the same spirit as
//! `proxy/TokenHub/backend/internal/metering/pricing.go`.

use std::collections::BTreeMap;

/// micro-USD per 1M tokens. `usd_per_token * 10^12`.
pub type MicrosPer1M = i64;
/// micro-USD. 1 USD == 1_000_000.
pub type Micros = i128;

pub const TOKENS_PER_MILLION: i128 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ServiceTier {
    Standard,
    Priority,
    Flex,
}

impl ServiceTier {
    pub const ALL: [ServiceTier; 3] = [Self::Standard, Self::Priority, Self::Flex];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Priority => "priority",
            Self::Flex => "flex",
        }
    }

    /// Normalises one raw service-tier value (§6.5).
    ///
    /// `None` for an unrecognised value — callers fall back to `Standard` and
    /// attach `UnknownServiceTier` rather than guessing, so the unknown stays
    /// visible instead of being silently priced as standard.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "standard" | "default" | "auto" => Some(Self::Standard),
            // `cc-switch-server/src/proxy/codex_request_policy.rs:97` writes
            // `priority` for fast mode; `fast` is the client-facing spelling.
            "priority" | "fast" => Some(Self::Priority),
            "flex" | "batch" => Some(Self::Flex),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ContextTier {
    Base,
    Long,
}

impl ContextTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Long => "long",
        }
    }
}

/// Stable wire strings consumed by the frontend i18n map (§9.1).
///
/// Unknown values are ignored by the frontend, so adding a variant is forward
/// compatible; renaming one is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PricingNote {
    PriceKeyNotFound,
    CacheWriteAssumed5m,
    LongContextApplied,
    ServiceTierFellBack,
    ContextTierFellBack,
    UnknownServiceTier,
    UsageStateNotFullyObserved,
}

impl PricingNote {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PriceKeyNotFound => "priceKeyNotFound",
            Self::CacheWriteAssumed5m => "cacheWriteAssumed5m",
            Self::LongContextApplied => "longContextApplied",
            Self::ServiceTierFellBack => "serviceTierFellBack",
            Self::ContextTierFellBack => "contextTierFellBack",
            Self::UnknownServiceTier => "unknownServiceTier",
            Self::UsageStateNotFullyObserved => "usageStateNotFullyObserved",
        }
    }

    /// Every variant, so the audit script can assert i18n parity without
    /// re-deriving the list from prose.
    pub const ALL: [PricingNote; 7] = [
        Self::PriceKeyNotFound,
        Self::CacheWriteAssumed5m,
        Self::LongContextApplied,
        Self::ServiceTierFellBack,
        Self::ContextTierFellBack,
        Self::UnknownServiceTier,
        Self::UsageStateNotFullyObserved,
    ];
}

/// One resolved `(service_tier, context_tier)` rate row. Unit: micro-USD / 1M.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RateSet {
    pub input_micros_per_1m: MicrosPer1M,
    pub output_micros_per_1m: MicrosPer1M,
    pub cache_read_micros_per_1m: MicrosPer1M,
    pub cache_write_5m_micros_per_1m: MicrosPer1M,
    /// `None` when upstream carries no usable 1h price for THIS row; the §7.5
    /// upper bound then collapses onto the point estimate instead of inverting.
    pub cache_write_1h_micros_per_1m: Option<MicrosPer1M>,
}

/// One model's rates within a single effectivity interval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPrice {
    pub price_key: String,
    pub display_name: String,
    pub long_context_threshold: Option<u64>,
    pub long_context_inclusive: bool,
    pub supports_cache_breakdown: bool,
    pub rates: BTreeMap<(ServiceTier, ContextTier), RateSet>,
}

impl ModelPrice {
    pub fn rate_at(&self, tier: ServiceTier, ctx: ContextTier) -> Option<&RateSet> {
        self.rates.get(&(tier, ctx))
    }

    /// Whether a request with this representative input size lands in the long
    /// tier. The comparison direction is per-vendor and carried in the catalog:
    /// upstream key names read "above N" (exclusive), but some vendors document
    /// "at or above".
    pub fn is_long_context(&self, representative_input_tokens: u64) -> bool {
        match self.long_context_threshold {
            Some(limit) if self.long_context_inclusive => representative_input_tokens >= limit,
            Some(limit) => representative_input_tokens > limit,
            None => false,
        }
    }
}

/// The four token categories carried by `share_request_logs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TokenSplit {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    /// Flat cache-creation total. Not splittable into 5m/1h from current data
    /// (§11.1), so it is priced at the 5m rate and bounded at the 1h rate.
    pub cache_write: u64,
}

impl TokenSplit {
    pub fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }

    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceLineKind {
    Input,
    Output,
    CacheRead,
    CacheWrite,
}

impl PriceLineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::CacheRead => "cacheRead",
            Self::CacheWrite => "cacheWrite",
        }
    }
}

/// One `tokens x unit price = amount` line. The frontend renders these; it
/// never recomputes them (§7.2 rule 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceLine {
    pub kind: PriceLineKind,
    pub tokens: u64,
    pub rate_micros_per_1m: MicrosPer1M,
    pub amount_micros: Micros,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricedUsage {
    pub lines: Vec<PriceLine>,
    pub total_micros: Micros,
    /// All cache-write tokens repriced at the 1h rate (§7.5). Always
    /// `>= total_micros`; equal when there is no cache write or no 1h price.
    pub upper_bound_micros: Micros,
    pub service_tier: ServiceTier,
    pub context_tier: ContextTier,
    pub notes: Vec<PricingNote>,
}

/// `tokens * rate / 1_000_000`, truncated toward zero (§7.2 rule 2).
///
/// Truncation rather than rounding keeps the equivalent amount from ever
/// exceeding the true official price by a rounding artefact — the number is
/// shown next to a real invoice, so erring low is the safe direction.
fn price_tokens(tokens: u64, rate_micros_per_1m: MicrosPer1M) -> Micros {
    if tokens == 0 || rate_micros_per_1m <= 0 {
        return 0;
    }
    i128::from(tokens) * i128::from(rate_micros_per_1m) / TOKENS_PER_MILLION
}

fn price_with(rates: &RateSet, split: &TokenSplit) -> (Vec<PriceLine>, Micros) {
    let mut lines = Vec::with_capacity(4);
    let mut total: Micros = 0;
    for (kind, tokens, rate) in [
        (PriceLineKind::Input, split.input, rates.input_micros_per_1m),
        (
            PriceLineKind::Output,
            split.output,
            rates.output_micros_per_1m,
        ),
        (
            PriceLineKind::CacheRead,
            split.cache_read,
            rates.cache_read_micros_per_1m,
        ),
        (
            PriceLineKind::CacheWrite,
            split.cache_write,
            rates.cache_write_5m_micros_per_1m,
        ),
    ] {
        // §7.2 rule 5: a zero-token category produces no line at all, so the
        // tooltip does not show rows the user did not generate.
        if tokens == 0 {
            continue;
        }
        let amount = price_tokens(tokens, rate);
        total += amount;
        lines.push(PriceLine {
            kind,
            tokens,
            rate_micros_per_1m: rate,
            amount_micros: amount,
        });
    }
    (lines, total)
}

/// Walks the four-step fallback chain of §5.2 and reports which rung was hit.
///
/// Tier fallback is tried before context fallback: a model with priority prices
/// but no priority long-context row usually means priority pricing simply is not
/// tiered by context, so `(priority, base)` is closer to the truth than
/// `(standard, long)`.
pub fn resolve_rates(
    price: &ModelPrice,
    tier: ServiceTier,
    ctx: ContextTier,
) -> Option<(&RateSet, Vec<PricingNote>)> {
    if let Some(rates) = price.rate_at(tier, ctx) {
        return Some((rates, Vec::new()));
    }
    if ctx != ContextTier::Base {
        if let Some(rates) = price.rate_at(tier, ContextTier::Base) {
            return Some((rates, vec![PricingNote::ContextTierFellBack]));
        }
    }
    if tier != ServiceTier::Standard {
        if let Some(rates) = price.rate_at(ServiceTier::Standard, ctx) {
            return Some((rates, vec![PricingNote::ServiceTierFellBack]));
        }
        if let Some(rates) = price.rate_at(ServiceTier::Standard, ContextTier::Base) {
            return Some((
                rates,
                vec![
                    PricingNote::ServiceTierFellBack,
                    PricingNote::ContextTierFellBack,
                ],
            ));
        }
    }
    None
}

/// Prices one pre-aggregated group.
///
/// `representative_input_tokens` is the input size of a SINGLE request in the
/// group, not the group's sum — §8.2 makes the long-context decision a grouping
/// dimension precisely so every request in a group shares one verdict, which is
/// what makes "sum then price" equal to "price then sum".
///
/// Returns `None` when the catalog has no usable rate row at all; the caller
/// emits `priced: false` with `null` amounts rather than a misleading `"0"`.
pub fn price_usage(
    price: &ModelPrice,
    tier: ServiceTier,
    split: &TokenSplit,
    representative_input_tokens: u64,
) -> Option<PricedUsage> {
    let ctx = if price.is_long_context(representative_input_tokens) {
        ContextTier::Long
    } else {
        ContextTier::Base
    };
    price_usage_in_tier(price, tier, ctx, split)
}

/// Same as [`price_usage`], but with the context tier already decided.
///
/// The aggregation in §8.2 makes the long-context verdict a GROUP BY dimension,
/// so by the time a group reaches the kernel the decision is already made and
/// re-deriving it from a summed input count would be wrong.
pub fn price_usage_in_tier(
    price: &ModelPrice,
    tier: ServiceTier,
    requested_ctx: ContextTier,
    split: &TokenSplit,
) -> Option<PricedUsage> {
    let (rates, mut notes) = resolve_rates(price, tier, requested_ctx)?;
    let (lines, total) = price_with(rates, split);

    // §7.3: `longContextApplied` reports the RESULT, not the condition. A
    // cache-heavy request can cross the threshold and still cost the same or
    // less, and flagging that as "long-context billing applied" misleads the
    // reader. Borrowed from TokenRouter's `LongContextBillingApplied`
    // (`internal/service/billing_service.go:1463`).
    if requested_ctx == ContextTier::Long && !notes.contains(&PricingNote::ContextTierFellBack) {
        if let Some((base_rates, _)) = resolve_rates(price, tier, ContextTier::Base) {
            let (_, base_total) = price_with(base_rates, split);
            if total > base_total {
                notes.push(PricingNote::LongContextApplied);
            }
        }
    }

    // §7.5 upper bound: identical except cache writes go at the 1h rate.
    let upper_bound = match rates.cache_write_1h_micros_per_1m {
        Some(rate_1h) if split.cache_write > 0 => {
            total - price_tokens(split.cache_write, rates.cache_write_5m_micros_per_1m)
                + price_tokens(split.cache_write, rate_1h)
        }
        _ => total,
    };

    if split.cache_write > 0 {
        notes.push(PricingNote::CacheWriteAssumed5m);
    }

    notes.sort();
    notes.dedup();

    Some(PricedUsage {
        lines,
        total_micros: total,
        upper_bound_micros: upper_bound.max(total),
        service_tier: tier,
        context_tier: requested_ctx,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rates(input: i64, output: i64, cache_read: i64, w5: i64, w1h: Option<i64>) -> RateSet {
        RateSet {
            input_micros_per_1m: input,
            output_micros_per_1m: output,
            cache_read_micros_per_1m: cache_read,
            cache_write_5m_micros_per_1m: w5,
            cache_write_1h_micros_per_1m: w1h,
        }
    }

    fn sonnet() -> ModelPrice {
        let mut map = BTreeMap::new();
        map.insert(
            (ServiceTier::Standard, ContextTier::Base),
            rates(3_000_000, 15_000_000, 300_000, 3_750_000, Some(6_000_000)),
        );
        map.insert(
            (ServiceTier::Standard, ContextTier::Long),
            rates(6_000_000, 22_500_000, 600_000, 7_500_000, None),
        );
        ModelPrice {
            price_key: "claude-sonnet-4-5".into(),
            display_name: "Claude Sonnet 4.5".into(),
            long_context_threshold: Some(200_000),
            long_context_inclusive: false,
            supports_cache_breakdown: true,
            rates: map,
        }
    }

    fn one_million_each() -> TokenSplit {
        TokenSplit {
            input: 1_000_000,
            output: 1_000_000,
            cache_read: 1_000_000,
            cache_write: 1_000_000,
        }
    }

    #[test]
    fn prices_each_category_independently() {
        let priced = price_usage(&sonnet(), ServiceTier::Standard, &one_million_each(), 100).unwrap();
        assert_eq!(priced.lines.len(), 4);
        // $3 + $15 + $0.30 + $3.75 = $22.05
        assert_eq!(priced.total_micros, 22_050_000);
        assert_eq!(priced.context_tier, ContextTier::Base);
    }

    #[test]
    fn zero_token_categories_produce_no_lines() {
        let split = TokenSplit {
            output: 500_000,
            ..TokenSplit::default()
        };
        let priced = price_usage(&sonnet(), ServiceTier::Standard, &split, 0).unwrap();
        assert_eq!(priced.lines.len(), 1);
        assert_eq!(priced.lines[0].kind, PriceLineKind::Output);
        assert_eq!(priced.total_micros, 7_500_000);
    }

    #[test]
    fn truncates_toward_zero_per_category() {
        // 1 token at 300_000 micros/1M is 0.3 micros -> 0, never 1.
        assert_eq!(price_tokens(1, 300_000), 0);
        assert_eq!(price_tokens(3, 300_000), 0);
        assert_eq!(price_tokens(4, 300_000), 1);
        assert_eq!(price_tokens(1, 3_000_000), 3);
        assert_eq!(price_tokens(0, 15_000_000), 0);
    }

    #[test]
    fn upper_bound_reprices_only_cache_writes_at_1h() {
        let split = TokenSplit {
            input: 1_000_000,
            cache_write: 1_000_000,
            ..TokenSplit::default()
        };
        let priced = price_usage(&sonnet(), ServiceTier::Standard, &split, 0).unwrap();
        assert_eq!(priced.total_micros, 3_000_000 + 3_750_000);
        // 1h is 1.6x the 5m rate, not 2x.
        assert_eq!(priced.upper_bound_micros, 3_000_000 + 6_000_000);
        assert!(priced.notes.contains(&PricingNote::CacheWriteAssumed5m));
    }

    #[test]
    fn upper_bound_equals_total_without_1h_price_or_cache_writes() {
        let mut price = sonnet();
        price
            .rates
            .get_mut(&(ServiceTier::Standard, ContextTier::Base))
            .unwrap()
            .cache_write_1h_micros_per_1m = None;
        let split = TokenSplit {
            cache_write: 1_000_000,
            ..TokenSplit::default()
        };
        let priced = price_usage(&price, ServiceTier::Standard, &split, 0).unwrap();
        assert_eq!(priced.upper_bound_micros, priced.total_micros);

        let no_writes = TokenSplit {
            input: 1_000_000,
            ..TokenSplit::default()
        };
        let priced = price_usage(&sonnet(), ServiceTier::Standard, &no_writes, 0).unwrap();
        assert_eq!(priced.upper_bound_micros, priced.total_micros);
        assert!(!priced.notes.contains(&PricingNote::CacheWriteAssumed5m));
    }

    #[test]
    fn long_context_switches_every_category_not_just_input() {
        let priced =
            price_usage(&sonnet(), ServiceTier::Standard, &one_million_each(), 250_000).unwrap();
        assert_eq!(priced.context_tier, ContextTier::Long);
        // $6 + $22.50 + $0.60 + $7.50 = $36.60 — cache_read moved too.
        assert_eq!(priced.total_micros, 36_600_000);
        let cache_read = priced
            .lines
            .iter()
            .find(|line| line.kind == PriceLineKind::CacheRead)
            .unwrap();
        assert_eq!(cache_read.rate_micros_per_1m, 600_000);
    }

    #[test]
    fn long_context_threshold_respects_inclusive_flag() {
        let mut price = sonnet();
        assert!(!price.is_long_context(200_000));
        assert!(price.is_long_context(200_001));
        price.long_context_inclusive = true;
        assert!(price.is_long_context(200_000));
        assert!(!price.is_long_context(199_999));
        price.long_context_threshold = None;
        assert!(!price.is_long_context(u64::MAX));
    }

    #[test]
    fn long_context_note_reports_result_not_condition() {
        let mut price = sonnet();
        // Same rates in both tiers: the threshold is crossed but nothing got
        // more expensive, so the note must not appear.
        let base = *price
            .rates
            .get(&(ServiceTier::Standard, ContextTier::Base))
            .unwrap();
        price
            .rates
            .insert((ServiceTier::Standard, ContextTier::Long), base);
        let priced = price_usage(&price, ServiceTier::Standard, &one_million_each(), 250_000).unwrap();
        assert_eq!(priced.context_tier, ContextTier::Long);
        assert!(!priced.notes.contains(&PricingNote::LongContextApplied));

        let priced =
            price_usage(&sonnet(), ServiceTier::Standard, &one_million_each(), 250_000).unwrap();
        assert!(priced.notes.contains(&PricingNote::LongContextApplied));
    }

    #[test]
    fn fallback_prefers_tier_over_context() {
        let mut price = sonnet();
        // priority exists at base only; standard has a long row.
        price.rates.insert(
            (ServiceTier::Priority, ContextTier::Base),
            rates(6_000_000, 30_000_000, 600_000, 7_500_000, None),
        );
        let priced =
            price_usage(&price, ServiceTier::Priority, &one_million_each(), 250_000).unwrap();
        assert!(priced.notes.contains(&PricingNote::ContextTierFellBack));
        assert!(!priced.notes.contains(&PricingNote::ServiceTierFellBack));
        // (priority, base) input rate, not (standard, long).
        let input = priced
            .lines
            .iter()
            .find(|line| line.kind == PriceLineKind::Input)
            .unwrap();
        assert_eq!(input.rate_micros_per_1m, 6_000_000);
    }

    #[test]
    fn fallback_to_standard_is_flagged() {
        let priced = price_usage(&sonnet(), ServiceTier::Flex, &one_million_each(), 0).unwrap();
        assert!(priced.notes.contains(&PricingNote::ServiceTierFellBack));
        assert_eq!(priced.total_micros, 22_050_000);
        // The requested tier is still reported; the note explains the price.
        assert_eq!(priced.service_tier, ServiceTier::Flex);
    }

    #[test]
    fn fallback_marks_both_dimensions_when_both_degrade() {
        let mut price = sonnet();
        price
            .rates
            .remove(&(ServiceTier::Standard, ContextTier::Long));
        let priced =
            price_usage(&price, ServiceTier::Priority, &one_million_each(), 250_000).unwrap();
        assert!(priced.notes.contains(&PricingNote::ServiceTierFellBack));
        assert!(priced.notes.contains(&PricingNote::ContextTierFellBack));
    }

    #[test]
    fn long_context_note_suppressed_when_context_fell_back() {
        let mut price = sonnet();
        price
            .rates
            .remove(&(ServiceTier::Standard, ContextTier::Long));
        let priced =
            price_usage(&price, ServiceTier::Standard, &one_million_each(), 250_000).unwrap();
        assert!(priced.notes.contains(&PricingNote::ContextTierFellBack));
        assert!(!priced.notes.contains(&PricingNote::LongContextApplied));
    }

    #[test]
    fn empty_catalog_entry_prices_nothing() {
        let price = ModelPrice {
            price_key: "mystery".into(),
            display_name: "Mystery".into(),
            long_context_threshold: None,
            long_context_inclusive: false,
            supports_cache_breakdown: false,
            rates: BTreeMap::new(),
        };
        assert!(price_usage(&price, ServiceTier::Standard, &one_million_each(), 0).is_none());
    }

    #[test]
    fn bucketed_aggregation_equals_per_request_pricing_within_truncation_drift() {
        let price = sonnet();
        let requests = [(1_234_u64, 567_u64), (89_012, 3_456), (77, 88_888)];
        let per_request: Micros = requests
            .iter()
            .map(|(input, output)| {
                price_usage(
                    &price,
                    ServiceTier::Standard,
                    &TokenSplit {
                        input: *input,
                        output: *output,
                        ..TokenSplit::default()
                    },
                    *input,
                )
                .unwrap()
                .total_micros
            })
            .sum();
        let bucketed = price_usage(
            &price,
            ServiceTier::Standard,
            &TokenSplit {
                input: requests.iter().map(|(input, _)| input).sum(),
                output: requests.iter().map(|(_, output)| output).sum(),
                ..TokenSplit::default()
            },
            requests[0].0,
        )
        .unwrap()
        .total_micros;
        // Truncation happens once per category, so pre-summed tokens can differ
        // from summed per-request amounts by at most 1 micro per request per
        // category. Assert that bound rather than exact equality.
        let drift = bucketed - per_request;
        assert!((0..=(requests.len() as i128) * 2).contains(&drift), "drift {drift}");
    }

    #[test]
    fn no_overflow_at_implausible_scale() {
        // 1e15 output tokens at $15/M is ~1.5e10 USD. i128 is untroubled, but
        // the multiply happens before the divide, so exercise it.
        let split = TokenSplit {
            output: 1_000_000_000_000_000,
            ..TokenSplit::default()
        };
        let priced = price_usage(&sonnet(), ServiceTier::Standard, &split, 0).unwrap();
        assert_eq!(priced.total_micros, 15_000_000_000_000_000);
    }

    #[test]
    fn service_tier_normalisation() {
        assert_eq!(ServiceTier::parse("priority"), Some(ServiceTier::Priority));
        assert_eq!(ServiceTier::parse("  FAST "), Some(ServiceTier::Priority));
        assert_eq!(ServiceTier::parse("flex"), Some(ServiceTier::Flex));
        assert_eq!(ServiceTier::parse("Default"), Some(ServiceTier::Standard));
        assert_eq!(ServiceTier::parse("scale-tier-7"), None);
        assert_eq!(ServiceTier::parse(""), None);
    }

    #[test]
    fn note_wire_strings_are_stable() {
        let wire: Vec<&str> = PricingNote::ALL.iter().map(|note| note.as_str()).collect();
        assert_eq!(
            wire,
            vec![
                "priceKeyNotFound",
                "cacheWriteAssumed5m",
                "longContextApplied",
                "serviceTierFellBack",
                "contextTierFellBack",
                "unknownServiceTier",
                "usageStateNotFullyObserved",
            ]
        );
    }
}
