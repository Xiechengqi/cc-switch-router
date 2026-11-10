//! v2 resource dimensions. Rates and quantities are integers; a missing rate
//! is never a free resource. v1 serialization omits the empty resource map.
use super::{ValueError, unsigned_decimal};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const RESOURCE_TARIFF_POLICY: &str = "api-equivalent-v2";
pub const RESOURCE_ESTIMATE_VERSION: &str = "resource-bounds-v1";
pub const MAX_RESOURCE_DIMENSIONS: usize = 64;

pub fn valid_resource_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 160
        && key
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._/-".contains(&c))
        && key.split('/').next().is_some_and(|base| {
            matches!(
                base,
                "image.output"
                    | "video.output_millisecond"
                    | "audio.input_millisecond"
                    | "audio.output_millisecond"
                    | "image.input_token"
                    | "image.output_token"
                    | "audio.input_token"
                    | "audio.output_token"
                    | "tool.web_search"
                    | "tool.web_fetch"
                    | "tool.file_search"
                    | "tool.code_execution"
                    | "tool.computer"
                    | "tool.mcp"
                    | "context.cached_token"
            )
        })
}

pub fn validate_resource_rates(rates: &BTreeMap<String, String>) -> Result<(), ValueError> {
    if rates.len() > MAX_RESOURCE_DIMENSIONS {
        return Err(ValueError::InvalidSnapshot);
    }
    for (key, rate) in rates {
        if !valid_resource_key(key) || unsigned_decimal(rate)? > i128::from(i64::MAX) {
            return Err(ValueError::InvalidSnapshot);
        }
    }
    Ok(())
}

/// Keys may include exact size/quality/resolution after `/`. A base price is
/// an explicit administrator-declared fallback, never inferred from a family.
pub fn price_resources(
    rates: &BTreeMap<String, String>,
    quantities: &BTreeMap<String, u64>,
) -> Result<i128, ValueError> {
    validate_resource_rates(rates)?;
    if quantities.len() > MAX_RESOURCE_DIMENSIONS {
        return Err(ValueError::UsageUnavailable);
    }
    quantities.iter().try_fold(0i128, |total, (key, count)| {
        if !valid_resource_key(key) {
            return Err(ValueError::UsageUnavailable);
        }
        let base = key.split('/').next().ok_or(ValueError::UsageUnavailable)?;
        // Even zero requires an explicit price: unpriced is not a zero rate.
        let rate = rates
            .get(key)
            .or_else(|| rates.get(base))
            .ok_or(ValueError::PriceUnavailable)?;
        unsigned_decimal(rate)?
            .checked_mul(i128::from(*count))
            .and_then(|amount| total.checked_add(amount))
            .ok_or(ValueError::Overflow)
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourcePlan {
    pub quantities: BTreeMap<String, u64>,
    /// Media generation has no text-token debit; media carried by a text
    /// request retains the model's token debit, including vision input.
    pub text_tokens: bool,
    pub requires_v2: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceUsage {
    pub quantities: BTreeMap<String, u64>,
    pub source: String,
    pub estimated: bool,
    pub version: String,
}
impl ResourceUsage {
    pub fn validate(&self) -> Result<(), ValueError> {
        if self.quantities.len() > MAX_RESOURCE_DIMENSIONS
            || self.source.is_empty()
            || self.source.len() > 80
            || self.source.chars().any(char::is_control)
            || self.version != RESOURCE_ESTIMATE_VERSION
            || self.quantities.keys().any(|key| !valid_resource_key(key))
        {
            return Err(ValueError::UsageUnavailable);
        }
        Ok(())
    }
}

/// Audio components included in aggregate text counters are charged exactly
/// once. Long-context selection still uses the original full context size.
/// An estimated resource bound cannot be subtracted from observed text.
impl super::ExecutionModelPrice {
    pub fn price_text_with_resources(
        &self,
        tier: &str,
        tokens: &super::ValueTokens,
        resources: Option<&ResourceUsage>,
    ) -> Result<super::PricedValue, ValueError> {
        let Some(resources) = resources.filter(|usage| !usage.estimated) else {
            return self.price(tier, tokens);
        };
        let mut billed = *tokens;
        for (key, count) in &resources.quantities {
            match key.as_str() {
                "audio.input_token" => {
                    billed.fresh_input = billed
                        .fresh_input
                        .checked_sub(*count)
                        .ok_or(ValueError::UsageUnavailable)?
                }
                "audio.output_token" => {
                    billed.output = billed
                        .output
                        .checked_sub(*count)
                        .ok_or(ValueError::UsageUnavailable)?
                }
                _ => {}
            }
        }
        if !matches!(tier, "standard" | "priority" | "flex") {
            return Err(ValueError::PriceUnavailable);
        }
        let context = match self.context_input_basis {
            super::ContextInputBasis::FreshInput => tokens.fresh_input,
            super::ContextInputBasis::RawInput => tokens.raw_input()?,
        };
        let long = self.long_context_threshold.is_some_and(|threshold| {
            if self.long_context_inclusive {
                context >= threshold
            } else {
                context > threshold
            }
        });
        self.rates
            .get(&format!("{tier}/{}", if long { "long" } else { "base" }))
            .ok_or(ValueError::PriceUnavailable)?
            .price(&billed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resources_use_exact_integers_and_explicit_prices() {
        let rates = BTreeMap::from([
            ("image.output".into(), "1250000000".into()),
            ("video.output_millisecond/720p".into(), "5000000".into()),
        ]);
        let quantities = BTreeMap::from([
            ("image.output/1024x1024/high".into(), 2),
            ("video.output_millisecond/720p".into(), 1250),
        ]);
        assert_eq!(price_resources(&rates, &quantities).unwrap(), 8_750_000_000);
        assert!(price_resources(&rates, &BTreeMap::from([("tool.web_search".into(), 0)])).is_err());
        assert_eq!(
            price_resources(
                &BTreeMap::from([("image.output".into(), "0".into())]),
                &BTreeMap::from([("image.output".into(), 1)])
            )
            .unwrap(),
            0
        );
    }
    #[test]
    fn audio_is_charged_once_without_changing_long_context_selection() {
        use super::super::{
            CacheWritePolicy, ContextInputBasis, ExecutionModelPrice, ExecutionRates, ValueTokens,
        };
        let rates = |input: &str| ExecutionRates {
            input: Some(input.into()),
            output: Some(input.into()),
            cache_read: None,
            cache_write5m: None,
            cache_write1h: None,
            cache_write_policy: CacheWritePolicy::Unavailable,
        };
        let price = ExecutionModelPrice {
            context_input_basis: ContextInputBasis::RawInput,
            long_context_threshold: Some(100),
            long_context_inclusive: false,
            rates: BTreeMap::from([
                ("standard/base".into(), rates("1")),
                ("standard/long".into(), rates("3")),
            ]),
            resources: BTreeMap::from([
                ("audio.input_token".into(), "10".into()),
                ("audio.output_token".into(), "20".into()),
            ]),
        };
        let tokens = ValueTokens {
            fresh_input: 110,
            output: 20,
            ..Default::default()
        };
        let mut usage = ResourceUsage {
            quantities: BTreeMap::from([
                ("audio.input_token".into(), 60),
                ("audio.output_token".into(), 10),
            ]),
            source: "upstream.protocol".into(),
            estimated: false,
            version: RESOURCE_ESTIMATE_VERSION.into(),
        };
        // Still selects long (110 input), even though only 50 fresh text tokens remain.
        assert_eq!(
            price
                .price_text_with_resources("standard", &tokens, Some(&usage))
                .unwrap()
                .usage_picos,
            180
        );
        assert_eq!(
            price_resources(&price.resources, &usage.quantities).unwrap(),
            800
        );
        usage.estimated = true;
        assert_eq!(
            price
                .price_text_with_resources("standard", &tokens, Some(&usage))
                .unwrap()
                .usage_picos,
            390
        );
        usage.estimated = false;
        usage.quantities.insert("audio.input_token".into(), 111);
        assert_eq!(
            price
                .price_text_with_resources("standard", &tokens, Some(&usage))
                .unwrap_err(),
            ValueError::UsageUnavailable
        );
    }
}
