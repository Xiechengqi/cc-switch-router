//! Wire contract shared verbatim by Router and Server. Keep fixtures in sync.
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

pub const VERSION: u32 = 1;
pub const MAX_BODY: usize = 8192;
pub const EARLY_SECONDS: i64 = 120;
pub const GRACE_SECONDS: i64 = 300;
pub const SIGNING_DOMAIN: &[u8] = b"cc-switch-host-health/report/v1\n";

pub fn due_at(host_id: &str, hour: i64) -> i64 {
    let digest = Sha256::digest(format!(
        "cc-switch-host-health/schedule/v1\n{host_id}\n{hour}"
    ));
    let value = u64::from_be_bytes(digest[..8].try_into().expect("SHA256 prefix"));
    hour * 3600 + (value % 3600) as i64
}

pub fn signing_message(body: &[u8]) -> Vec<u8> {
    let mut message = SIGNING_DOMAIN.to_vec();
    message.extend_from_slice(&Sha256::digest(body));
    message
}

// The deserializer makes a missing nullable field an error.
fn nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reading {
    #[serde(deserialize_with = "nullable")]
    pub value: Option<f64>,
    #[serde(deserialize_with = "nullable")]
    pub unknown_reason: Option<String>,
}

impl Reading {
    pub fn known(value: f64) -> Self {
        Self {
            value: Some(value),
            unknown_reason: None,
        }
    }

    pub fn unknown(reason: &str) -> Self {
        Self {
            value: None,
            unknown_reason: Some(reason.to_owned()),
        }
    }

    fn validate(&self, maximum: f64, integral: bool) -> bool {
        match (&self.value, self.unknown_reason.as_deref()) {
            (Some(value), None) => {
                value.is_finite()
                    && *value >= 0.0
                    && *value <= maximum
                    && (!integral || value.fract() == 0.0)
            }
            (
                None,
                Some("scope_unverified" | "unavailable" | "counter_reset" | "private" | "expired"),
            ) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub os: String,
    pub arch: String,
    pub effective_vcpu: Reading,
    pub memory_total_bytes: Reading,
    pub memory_available_bytes: Reading,
    pub data_volume_total_bytes: Reading,
    pub data_volume_available_bytes: Reading,
    pub cpu_usage_pct: Reading,
}

impl Snapshot {
    pub fn hide_dynamic(&mut self, reason: &str) {
        self.memory_available_bytes = Reading::unknown(reason);
        self.data_volume_available_bytes = Reading::unknown(reason);
        self.cpu_usage_pct = Reading::unknown(reason);
    }

    pub fn validate(&self) -> bool {
        let byte_limit = 9_007_199_254_740_991.0;
        !self.os.is_empty()
            && self.os.len() <= 128
            && !self.os.chars().any(char::is_control)
            && matches!(self.arch.as_str(), "x86_64" | "aarch64")
            && self.effective_vcpu.validate(1_000_000.0, false)
            && self.effective_vcpu.value != Some(0.0)
            && self.memory_total_bytes.validate(byte_limit, true)
            && self.memory_available_bytes.validate(byte_limit, true)
            && self.data_volume_total_bytes.validate(byte_limit, true)
            && self.data_volume_available_bytes.validate(byte_limit, true)
            && self.cpu_usage_pct.validate(100.0, false)
            && !matches!((self.memory_available_bytes.value, self.memory_total_bytes.value), (Some(a), Some(t)) if a > t)
            && !matches!((self.data_volume_available_bytes.value, self.data_volume_total_bytes.value), (Some(a), Some(t)) if a > t)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Report {
    pub report_version: u32,
    pub host_id: String,
    pub machine_epoch: u64,
    pub key_generation: u64,
    pub utc_hour: i64,
    pub sampled_at: String,
    pub build_version: String,
    pub snapshot: Snapshot,
}

impl Report {
    pub fn validate(&self) -> bool {
        self.report_version == VERSION
            && !self.host_id.is_empty()
            && self.host_id.len() <= 64
            && self
                .host_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && (1..=i64::MAX as u64).contains(&self.machine_epoch)
            && (1..=i64::MAX as u64).contains(&self.key_generation)
            && (0..=100_000_000).contains(&self.utc_hour)
            && !self.build_version.is_empty()
            && self.build_version.len() <= 128
            && self.snapshot.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use ed25519_dalek::{Signature, VerifyingKey};

    #[test]
    fn shared_wire_and_schedule_vector() {
        let encoding = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        assert_eq!(due_at("host-fixture", 4), 4 * 3600 + 3592);
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/host-health-v1.json")).unwrap();
        let body = fixture["body"].as_str().unwrap();
        let key: [u8; 32] = encoding
            .decode(fixture["publicKey"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let signature = Signature::from_slice(
            &encoding
                .decode(fixture["signature"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        VerifyingKey::from_bytes(&key)
            .unwrap()
            .verify_strict(&signing_message(body.as_bytes()), &signature)
            .unwrap();
        let report: Report = serde_json::from_str(body).unwrap();
        assert!(report.validate());
        let mut altered: serde_json::Value = serde_json::from_str(body).unwrap();
        altered["snapshot"]
            .as_object_mut()
            .unwrap()
            .remove("memoryTotalBytes");
        assert!(serde_json::from_value::<Report>(altered).is_err());
    }
}
