//! Share API-price-equivalent quotas. Repository-owned v1 wire and arithmetic.
//! This module has no IO, wallet, market accounting, or external price lookup.
//! Router and Server implement the same specification and golden vectors.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const TARIFF_POLICY: &str = "api-equivalent-v1";
pub const PICOS_PER_MICRO: i128 = 1_000_000;
pub const PICOS_PER_USD: i128 = 1_000_000_000_000;
pub const MAX_LIMIT_MICROS: i128 = 1_000_000_000_000_000;
pub const MAX_SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_MODELS: usize = 10_000;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValueUsageEdit {
    pub action: ValueUsageEditAction,
    pub target_picos: Option<String>,
    pub expected_grant_revision: u64,
    pub window_id: String,
    pub expected_watermark: u64,
    pub operation_id: String,
    pub reason: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ValueUsageEditAction {
    Set,
    Clear,
}

/// Excludes separately billed resources; tool-call text itself is token usage.
/// Deliberately conservative: unrecognized typed content is not made free.
pub fn is_metered_text_payload(bytes: &[u8]) -> bool {
    fn text(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(map) => {
                if map.keys().any(|key| {
                    matches!(
                        key.as_str(),
                        "inlineData"
                            | "inline_data"
                            | "fileData"
                            | "file_data"
                            | "image_url"
                            | "input_audio"
                            | "audio"
                            | "video"
                            | "image"
                            | "audioConfig"
                            | "speechConfig"
                            | "groundingConfig"
                            | "cachedContent"
                            | "googleSearch"
                            | "google_search"
                            | "googleSearchRetrieval"
                            | "google_search_retrieval"
                            | "codeExecution"
                            | "code_execution"
                            | "urlContext"
                            | "url_context"
                            | "computer_use"
                    )
                }) {
                    return false;
                }
                if let Some(kind) = map.get("type").and_then(serde_json::Value::as_str) {
                    if !matches!(
                        kind,
                        "text"
                            | "input_text"
                            | "output_text"
                            | "message"
                            | "function"
                            | "function_call"
                            | "function_call_output"
                            | "tool_use"
                            | "tool_result"
                    ) {
                        return false;
                    }
                }
                if map
                    .get("responseModalities")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|m| m.iter().any(|v| v.as_str() != Some("TEXT")))
                {
                    return false;
                }
                map.iter().all(|(key, value)| {
                    matches!(key.as_str(), "parameters" | "input_schema" | "schema") || text(value)
                })
            }
            serde_json::Value::Array(items) => items.iter().all(text),
            _ => true,
        }
    }
    serde_json::from_slice::<serde_json::Value>(bytes)
        .is_ok_and(|body| body.is_object() && text(&body))
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValueMeteringBinding {
    pub quota_policy_epoch: String,
    pub value_metered_from_ms: i64,
    pub effective_from_ms: i64,
    pub pricing_revision: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValueQuotaStatus {
    pub email: String,
    pub policy_epoch: String,
    pub window_id: String,
    pub window_starts_at_ms: i64,
    pub resets_at_ms: Option<i64>,
    pub metered_from_ms: i64,
    pub active_pricing_revision: String,
    pub usage_pricing_revisions_digest: String,
    pub usage_watermark: u64,
    pub limit_usd_micros: Option<String>,
    pub used_picos: String,
    pub observed_picos: String,
    pub manual_offset_picos: String,
    pub pending_risk_picos: String,
    pub metering_status: String,
    pub synced_at_ms: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PricingManifest {
    pub installation_id: String,
    pub revision: String,
    pub effective_from_ms: i64,
    pub snapshot: PricingSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueError {
    InvalidDecimal,
    OutOfRange,
    InvalidPolicy,
    InvalidSnapshot,
    PriceUnavailable,
    UsageUnavailable,
    Overflow,
}
impl std::fmt::Display for ValueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidDecimal => "invalid canonical unsigned decimal",
            Self::OutOfRange => "value exceeds the supported range",
            Self::InvalidPolicy => "invalid USD-equivalent quota policy",
            Self::InvalidSnapshot => "invalid pricing snapshot",
            Self::PriceUnavailable => "price_unavailable",
            Self::UsageUnavailable => "usage_unavailable",
            Self::Overflow => "quota arithmetic overflow",
        })
    }
}
impl std::error::Error for ValueError {}

pub fn unsigned_decimal(raw: &str) -> Result<i128, ValueError> {
    if raw.is_empty()
        || raw.len() > 39
        || (raw.len() > 1 && raw.starts_with('0'))
        || !raw.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(ValueError::InvalidDecimal);
    }
    raw.parse().map_err(|_| ValueError::OutOfRange)
}

/// USD user input to exact micro-USD. The wire itself uses integer strings.
pub fn usd_to_micros(raw: &str) -> Result<String, ValueError> {
    let (whole, fraction) = raw.split_once('.').unwrap_or((raw, ""));
    if whole.is_empty()
        || (raw.contains('.') && fraction.is_empty())
        || fraction.len() > 6
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(ValueError::InvalidDecimal);
    }
    let whole: i128 = whole.parse().map_err(|_| ValueError::OutOfRange)?;
    let fraction: i128 = format!("{fraction:0<6}")
        .parse()
        .map_err(|_| ValueError::InvalidDecimal)?;
    let micros = whole
        .checked_mul(1_000_000)
        .and_then(|v| v.checked_add(fraction))
        .ok_or(ValueError::Overflow)?;
    if micros == 0 || micros > MAX_LIMIT_MICROS {
        return Err(ValueError::OutOfRange);
    }
    Ok(micros.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValueQuota {
    pub limit_usd_micros: String,
    pub tariff_policy_id: String,
}
impl ValueQuota {
    pub fn limit_picos(&self) -> Result<i128, ValueError> {
        let micros = unsigned_decimal(&self.limit_usd_micros)?;
        if self.tariff_policy_id != TARIFF_POLICY || micros == 0 || micros > MAX_LIMIT_MICROS {
            return Err(ValueError::InvalidPolicy);
        }
        micros
            .checked_mul(PICOS_PER_MICRO)
            .ok_or(ValueError::Overflow)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContextInputBasis {
    FreshInput,
    RawInput,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CacheWritePolicy {
    Unavailable,
    FiveMinutesOnly,
    OneHourOnly,
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionRates {
    pub input: Option<String>,
    pub output: Option<String>,
    pub cache_read: Option<String>,
    pub cache_write5m: Option<String>,
    pub cache_write1h: Option<String>,
    pub cache_write_policy: CacheWritePolicy,
}
impl ExecutionRates {
    fn rate(raw: &Option<String>) -> Result<Option<i128>, ValueError> {
        raw.as_deref()
            .map(|raw| {
                let value = unsigned_decimal(raw)?;
                if value > i128::from(i64::MAX) {
                    return Err(ValueError::OutOfRange);
                }
                Ok(value)
            })
            .transpose()
    }
    pub fn validate(&self) -> Result<(), ValueError> {
        for rate in [
            &self.input,
            &self.output,
            &self.cache_read,
            &self.cache_write5m,
            &self.cache_write1h,
        ] {
            Self::rate(rate)?;
        }
        if self.input.is_none() || self.output.is_none() {
            return Err(ValueError::PriceUnavailable);
        }
        match self.cache_write_policy {
            CacheWritePolicy::Unavailable => {
                if self.cache_write5m.is_some() || self.cache_write1h.is_some() {
                    return Err(ValueError::InvalidSnapshot);
                }
            }
            CacheWritePolicy::FiveMinutesOnly
                if self.cache_write5m.is_none() || self.cache_write1h.is_some() =>
            {
                return Err(ValueError::InvalidSnapshot);
            }
            CacheWritePolicy::OneHourOnly
                if self.cache_write1h.is_none() || self.cache_write5m.is_some() =>
            {
                return Err(ValueError::InvalidSnapshot);
            }
            CacheWritePolicy::Both
                if self.cache_write5m.is_none() || self.cache_write1h.is_none() =>
            {
                return Err(ValueError::InvalidSnapshot);
            }
            _ => {}
        }
        Ok(())
    }
    pub fn price(&self, tokens: &ValueTokens) -> Result<PricedValue, ValueError> {
        self.validate()?;
        let mut total = 0i128;
        let mut add = |count: u64, rate: Option<i128>| -> Result<(), ValueError> {
            if count == 0 {
                return Ok(());
            }
            let value = i128::from(count)
                .checked_mul(rate.ok_or(ValueError::PriceUnavailable)?)
                .ok_or(ValueError::Overflow)?;
            total = total.checked_add(value).ok_or(ValueError::Overflow)?;
            Ok(())
        };
        add(tokens.fresh_input, Self::rate(&self.input)?)?;
        add(tokens.output, Self::rate(&self.output)?)?;
        add(tokens.cache_read, Self::rate(&self.cache_read)?)?;
        add(tokens.cache_write5m, Self::rate(&self.cache_write5m)?)?;
        add(tokens.cache_write1h, Self::rate(&self.cache_write1h)?)?;
        let unknown_rate = match self.cache_write_policy {
            CacheWritePolicy::Unavailable => None,
            CacheWritePolicy::FiveMinutesOnly => Self::rate(&self.cache_write5m)?,
            CacheWritePolicy::OneHourOnly => Self::rate(&self.cache_write1h)?,
            CacheWritePolicy::Both => Some(
                Self::rate(&self.cache_write5m)?
                    .ok_or(ValueError::PriceUnavailable)?
                    .max(Self::rate(&self.cache_write1h)?.ok_or(ValueError::PriceUnavailable)?),
            ),
        };
        add(tokens.cache_write_unknown, unknown_rate)?;
        Ok(PricedValue {
            usage_picos: total,
            conservative_cache_ttl: tokens.cache_write_unknown > 0
                && self.cache_write_policy == CacheWritePolicy::Both,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValueTokens {
    pub fresh_input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write5m: u64,
    pub cache_write1h: u64,
    pub cache_write_unknown: u64,
}
impl ValueTokens {
    pub fn raw_input(&self) -> Result<u64, ValueError> {
        [
            self.cache_read,
            self.cache_write5m,
            self.cache_write1h,
            self.cache_write_unknown,
        ]
        .into_iter()
        .try_fold(self.fresh_input, |sum, n| {
            sum.checked_add(n).ok_or(ValueError::Overflow)
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PricedValue {
    pub usage_picos: i128,
    pub conservative_cache_ttl: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionModelPrice {
    pub context_input_basis: ContextInputBasis,
    pub long_context_threshold: Option<u64>,
    pub long_context_inclusive: bool,
    #[serde(deserialize_with = "unique_map")]
    pub rates: BTreeMap<String, ExecutionRates>,
}
impl ExecutionModelPrice {
    pub fn validate(&self) -> Result<(), ValueError> {
        if self.long_context_threshold == Some(0) || self.rates.is_empty() || self.rates.len() > 6 {
            return Err(ValueError::InvalidSnapshot);
        }
        for (key, rates) in &self.rates {
            if !matches!(
                key.as_str(),
                "standard/base"
                    | "standard/long"
                    | "priority/base"
                    | "priority/long"
                    | "flex/base"
                    | "flex/long"
            ) {
                return Err(ValueError::InvalidSnapshot);
            }
            if key.ends_with("/long") && self.long_context_threshold.is_none() {
                return Err(ValueError::InvalidSnapshot);
            }
            rates.validate()?;
        }
        for tier in ["standard", "priority", "flex"] {
            if self
                .rates
                .keys()
                .any(|key| key.starts_with(&format!("{tier}/")))
                && !self.supports_tier(tier)
            {
                return Err(ValueError::InvalidSnapshot);
            }
        }
        Ok(())
    }
    pub fn price(
        &self,
        effective_tier: &str,
        tokens: &ValueTokens,
    ) -> Result<PricedValue, ValueError> {
        if !matches!(effective_tier, "standard" | "priority" | "flex") {
            return Err(ValueError::PriceUnavailable);
        }
        let context_input = match self.context_input_basis {
            ContextInputBasis::FreshInput => tokens.fresh_input,
            ContextInputBasis::RawInput => tokens.raw_input()?,
        };
        let long = self.long_context_threshold.is_some_and(|n| {
            if self.long_context_inclusive {
                context_input >= n
            } else {
                context_input > n
            }
        });
        let key = format!("{effective_tier}/{}", if long { "long" } else { "base" });
        self.rates
            .get(&key)
            .ok_or(ValueError::PriceUnavailable)?
            .price(tokens)
    }
    pub fn supports_tier(&self, tier: &str) -> bool {
        self.rates.contains_key(&format!("{tier}/base"))
            && (self.long_context_threshold.is_none()
                || self.rates.contains_key(&format!("{tier}/long")))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PricingSnapshot {
    pub schema_version: u16,
    pub tariff_policy_id: String,
    pub currency: String,
    #[serde(deserialize_with = "unique_map")]
    pub models: BTreeMap<String, ExecutionModelPrice>,
    #[serde(deserialize_with = "unique_map")]
    pub exact_aliases: BTreeMap<String, String>,
}
fn valid_model_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 250 && key.trim() == key && !key.chars().any(char::is_control)
}
impl PricingSnapshot {
    pub fn validate(&self) -> Result<(), ValueError> {
        if self.schema_version != 1
            || self.tariff_policy_id != TARIFF_POLICY
            || self.currency != "USD"
            || self.models.is_empty()
            || self.models.len() > MAX_MODELS
            || self.exact_aliases.len() > MAX_MODELS * 4
        {
            return Err(ValueError::InvalidSnapshot);
        }
        for (key, price) in &self.models {
            if !valid_model_key(key) {
                return Err(ValueError::InvalidSnapshot);
            }
            price.validate()?;
        }
        for (alias, key) in &self.exact_aliases {
            if !valid_model_key(alias)
                || !self.models.contains_key(key)
                || self.models.contains_key(alias)
            {
                return Err(ValueError::InvalidSnapshot);
            }
        }
        if serde_json::to_vec(self)
            .map_err(|_| ValueError::InvalidSnapshot)?
            .len()
            > MAX_SNAPSHOT_BYTES
        {
            return Err(ValueError::InvalidSnapshot);
        }
        Ok(())
    }
    pub fn revision(&self) -> Result<String, ValueError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| ValueError::InvalidSnapshot)?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
    pub fn model(&self, effective_model: &str) -> Result<&ExecutionModelPrice, ValueError> {
        let key = self
            .exact_aliases
            .get(effective_model)
            .map(String::as_str)
            .unwrap_or(effective_model);
        self.models.get(key).ok_or(ValueError::PriceUnavailable)
    }
}

// Serde's default map reader silently accepts repeated model/rate keys.
// Executable snapshots reject them before hashing or persisting anything.
fn unique_map<'de, D, V>(deserializer: D) -> Result<BTreeMap<String, V>, D::Error>
where
    D: serde::Deserializer<'de>,
    V: Deserialize<'de>,
{
    struct Unique<V>(std::marker::PhantomData<V>);
    impl<'de, V: Deserialize<'de>> serde::de::Visitor<'de> for Unique<V> {
        type Value = BTreeMap<String, V>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a map with unique keys")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> Result<Self::Value, M::Error> {
            let mut values = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, V>()? {
                if values.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("duplicate pricing key"));
                }
            }
            Ok(values)
        }
    }
    deserializer.deserialize_map(Unique(std::marker::PhantomData))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_golden_vectors_freeze_snapshot_hash_aliases_and_exact_amounts() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/share-value-pricing-v1.json"
        ))
        .unwrap();
        let snapshot: PricingSnapshot =
            serde_json::from_value(fixture["snapshot"].clone()).unwrap();
        assert_eq!(
            snapshot.revision().unwrap(),
            fixture["revision"].as_str().unwrap()
        );
        for vector in fixture["vectors"].as_array().unwrap() {
            let tokens: ValueTokens = serde_json::from_value(vector["tokens"].clone()).unwrap();
            let price = snapshot
                .model(vector["model"].as_str().unwrap())
                .unwrap()
                .price(vector["tier"].as_str().unwrap(), &tokens)
                .unwrap();
            assert_eq!(
                price.usage_picos.to_string(),
                vector["picos"].as_str().unwrap(),
                "{}",
                vector["name"]
            );
            assert_eq!(
                price.conservative_cache_ttl,
                vector["conservative"].as_bool().unwrap()
            );
        }
        assert!(snapshot.model("guessed-model-family").is_err());
        assert!(
            snapshot
                .model("text-model")
                .unwrap()
                .price("priority", &ValueTokens::default())
                .is_err()
        );
    }
    #[test]
    fn only_text_and_client_function_tools_are_metered_not_media_or_paid_builtin_tools() {
        assert!(is_metered_text_payload(br#"{"messages":[{"role":"user","content":"hello"}],"tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object","properties":{"image":{"type":"string"}}}}}]}"#));
        for request in [
            br#"{"tools":[{"googleSearch":{}}]}"#.as_slice(),
            br#"{"tools":[{"type":"web_search"}]}"#,
            br#"{"messages":[{"content":[{"type":"image_url","image_url":"url"}]}]}"#,
            br#"{"generationConfig":{"responseModalities":["IMAGE"]}}"#,
            br#"{"cachedContent":"unverified-context"}"#,
        ] {
            assert!(!is_metered_text_payload(request));
        }
    }
    fn rates() -> ExecutionRates {
        ExecutionRates {
            input: Some("3000000".into()),
            output: Some("15000000".into()),
            cache_read: Some("300000".into()),
            cache_write5m: Some("3750000".into()),
            cache_write1h: Some("6000000".into()),
            cache_write_policy: CacheWritePolicy::Both,
        }
    }
    #[test]
    fn exact_picos_keep_sub_micro_requests_and_real_zero_separate_from_missing() {
        let r = rates();
        let t = ValueTokens {
            fresh_input: 1000,
            output: 1000,
            ..Default::default()
        };
        assert_eq!(r.price(&t).unwrap().usage_picos, 18_000_000_000);
        let mut tiny = r.clone();
        tiny.input = Some("1".into());
        assert_eq!(
            tiny.price(&ValueTokens {
                fresh_input: 1,
                ..Default::default()
            })
            .unwrap()
            .usage_picos,
            1
        );
        tiny.input = Some("0".into());
        assert_eq!(tiny.price(&t).unwrap().usage_picos, 15_000_000_000);
        tiny.input = None;
        assert_eq!(tiny.price(&t), Err(ValueError::PriceUnavailable));
    }
    #[test]
    fn unknown_cache_ttl_uses_proven_upper_bound_without_double_counting() {
        let mut r = rates();
        let t = ValueTokens {
            cache_write_unknown: 4,
            ..Default::default()
        };
        assert_eq!(
            r.price(&t).unwrap(),
            PricedValue {
                usage_picos: 24_000_000,
                conservative_cache_ttl: true
            }
        );
        r.cache_write_policy = CacheWritePolicy::FiveMinutesOnly;
        r.cache_write1h = None;
        assert_eq!(r.price(&t).unwrap().usage_picos, 15_000_000);
        let t = ValueTokens {
            cache_write1h: 1,
            ..Default::default()
        };
        assert_eq!(r.price(&t), Err(ValueError::PriceUnavailable));
    }
    #[test]
    fn raw_context_and_strict_tiers_do_not_underprice_cache_heavy_requests() {
        let mut long = rates();
        long.input = Some("6000000".into());
        let p = ExecutionModelPrice {
            context_input_basis: ContextInputBasis::RawInput,
            long_context_threshold: Some(200_000),
            long_context_inclusive: false,
            rates: BTreeMap::from([
                ("standard/base".into(), rates()),
                ("standard/long".into(), long),
            ]),
        };
        let t = ValueTokens {
            fresh_input: 100,
            cache_read: 200_000,
            ..Default::default()
        };
        assert_eq!(p.price("standard", &t).unwrap().usage_picos, 60_600_000_000);
        assert_eq!(p.price("priority", &t), Err(ValueError::PriceUnavailable));
        assert_eq!(p.price("auto", &t), Err(ValueError::PriceUnavailable));
    }
    #[test]
    fn wire_decimals_and_limits_are_bounded_and_exact() {
        for raw in ["", "-1", "+1", "01", "1.0", " 1", "1e6"] {
            assert!(unsigned_decimal(raw).is_err());
        }
        for raw in ["0", "1.", ".1", "1e2", "-1", "1.0000001", " 1"] {
            assert!(usd_to_micros(raw).is_err());
        }
        assert_eq!(usd_to_micros("10.000001").unwrap(), "10000001");
        assert_eq!(
            ValueQuota {
                limit_usd_micros: "10000000".into(),
                tariff_policy_id: TARIFF_POLICY.into()
            }
            .limit_picos()
            .unwrap(),
            10 * PICOS_PER_USD
        );
        assert!(
            ValueQuota {
                limit_usd_micros: "0".into(),
                tariff_policy_id: TARIFF_POLICY.into()
            }
            .limit_picos()
            .is_err()
        );
    }
    #[test]
    fn snapshots_are_canonical_bounded_and_reject_duplicate_keys() {
        let snapshot = PricingSnapshot {
            schema_version: 1,
            tariff_policy_id: TARIFF_POLICY.into(),
            currency: "USD".into(),
            models: BTreeMap::from([(
                "test-model".into(),
                ExecutionModelPrice {
                    context_input_basis: ContextInputBasis::RawInput,
                    long_context_threshold: None,
                    long_context_inclusive: false,
                    rates: BTreeMap::from([("standard/base".into(), rates())]),
                },
            )]),
            exact_aliases: BTreeMap::new(),
        };
        let encoded = serde_json::to_string(&snapshot).unwrap();
        assert_eq!(
            snapshot.revision().unwrap(),
            serde_json::from_str::<PricingSnapshot>(&encoded)
                .unwrap()
                .revision()
                .unwrap()
        );
        let duplicate = encoded.replace(
            "\"standard/base\":",
            &format!(
                "\"standard/base\":{},\"standard/base\":",
                serde_json::to_string(&rates()).unwrap()
            ),
        );
        assert!(serde_json::from_str::<PricingSnapshot>(&duplicate).is_err());
        let mut corrupt = snapshot;
        corrupt
            .exact_aliases
            .insert("test-model".into(), "missing".into());
        assert!(corrupt.revision().is_err());
    }
}
