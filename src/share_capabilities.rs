//! Independent Share execution permissions. Missing policy is legacy mode;
//! an explicit empty set denies all extended operations, regardless of quota unit.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ShareOperation {
    #[serde(rename = "image.generate")]
    ImageGenerate,
    #[serde(rename = "image.edit")]
    ImageEdit,
    #[serde(rename = "video.generate")]
    VideoGenerate,
    #[serde(rename = "audio.generate")]
    AudioGenerate,
    #[serde(rename = "tool.web_search")]
    WebSearch,
    #[serde(rename = "tool.web_fetch")]
    WebFetch,
    #[serde(rename = "tool.file_search")]
    FileSearch,
    #[serde(rename = "tool.code_execution")]
    CodeExecution,
    #[serde(rename = "tool.computer")]
    Computer,
    #[serde(rename = "tool.remote_mcp")]
    RemoteMcp,
}
impl ShareOperation {
    pub const ALL: [Self; 10] = [
        Self::ImageGenerate,
        Self::ImageEdit,
        Self::VideoGenerate,
        Self::AudioGenerate,
        Self::WebSearch,
        Self::WebFetch,
        Self::FileSearch,
        Self::CodeExecution,
        Self::Computer,
        Self::RemoteMcp,
    ];
    pub fn resource(self) -> &'static str {
        match self {
            Self::ImageGenerate | Self::ImageEdit => "image.output",
            Self::VideoGenerate => "video.output_millisecond",
            Self::AudioGenerate => "audio.output_token",
            Self::WebSearch => "tool.web_search",
            Self::WebFetch => "tool.web_fetch",
            Self::FileSearch => "tool.file_search",
            Self::CodeExecution => "tool.code_execution",
            Self::Computer => "tool.computer",
            Self::RemoteMcp => "tool.mcp",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantCapabilityEdit {
    pub operations: Vec<ShareOperation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reconfirm: Vec<ShareOperation>,
    pub expected_revision: u64,
}
impl GrantCapabilityEdit {
    pub fn validate(&self) -> Result<(), String> {
        validate_operations(&self.operations)?;
        validate_operations(&self.reconfirm)?;
        if self
            .reconfirm
            .iter()
            .any(|op| !self.operations.contains(op))
        {
            return Err("reconfirmation requires an enabled operation".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationBinding {
    pub app: String,
    /// Opaque identity: provider, Driver, auth identity generation and policy.
    pub fingerprint: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoverageAcknowledgement {
    /// Server-derived scope/unit/operation/runtime/gap identity.
    pub condition: String,
    pub scope: String,
    pub operation: ShareOperation,
    pub quota_unit: String,
    pub binding_fingerprint: String,
    pub actor: String,
    pub accepted_at_ms: i64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareCapabilityPolicy {
    pub version: u16,
    pub allowed_operations: Vec<ShareOperation>,
    #[serde(default)]
    pub operation_bindings: BTreeMap<ShareOperation, Vec<OperationBinding>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub coverage_acknowledgements: Vec<CoverageAcknowledgement>,
}
impl Default for ShareCapabilityPolicy {
    fn default() -> Self {
        Self {
            version: 1,
            allowed_operations: vec![],
            operation_bindings: BTreeMap::new(),
            coverage_acknowledgements: vec![],
        }
    }
}
impl ShareCapabilityPolicy {
    pub fn validate(&self) -> Result<(), String> {
        validate_operations(&self.allowed_operations)?;
        if self.version != 1
            || self.operation_bindings.len() > 10
            || self.coverage_acknowledgements.len() > 16_384
        {
            return Err("invalid capability policy version or size".into());
        }
        for (operation, bindings) in &self.operation_bindings {
            if !self.allowed_operations.contains(operation) || bindings.len() > 3 {
                return Err("invalid capability operation bindings".into());
            }
            let mut seen = BTreeSet::new();
            for binding in bindings {
                if !["claude", "codex", "gemini"].contains(&binding.app.as_str())
                    || binding.fingerprint.len() != 64
                    || !binding.fingerprint.bytes().all(|c| c.is_ascii_hexdigit())
                    || !seen.insert(&binding.app)
                {
                    return Err("invalid capability binding identity".into());
                }
            }
        }
        let mut acknowledged = BTreeSet::new();
        for acknowledgement in &self.coverage_acknowledgements {
            let hex_id = |id: &str| id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit());
            if !hex_id(&acknowledgement.condition)
                || !hex_id(&acknowledgement.binding_fingerprint)
                || !self.allowed_operations.contains(&acknowledgement.operation)
                || !["tokens", "usdEquivalent"].contains(&acknowledgement.quota_unit.as_str())
                || acknowledgement.scope.is_empty()
                || acknowledgement.scope.len() > 254
                || acknowledgement.actor.len() > 254
                || acknowledgement.actor.chars().any(char::is_control)
                || acknowledgement.accepted_at_ms < 0
                || !acknowledged.insert(&acknowledgement.condition)
            {
                return Err("invalid capability coverage acknowledgement".into());
            }
        }
        Ok(())
    }
}
pub fn validate_operations(operations: &[ShareOperation]) -> Result<(), String> {
    if operations.len() > 10 || operations.iter().collect::<BTreeSet<_>>().len() != operations.len()
    {
        return Err("duplicate or too many capability operations".into());
    }
    Ok(())
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityConsent {
    pub credential: String,
    #[serde(default)]
    pub accept_uncovered: Vec<String>,
}

pub fn visible_policy(
    policy: Option<&ShareCapabilityPolicy>,
    manager: bool,
    email: Option<&str>,
    grants: &BTreeMap<String, crate::models::ShareUserGrant>,
) -> Option<ShareCapabilityPolicy> {
    policy.map(|p| {
        let mut p = p.clone();
        if !manager {
            p.coverage_acknowledgements.clear();
            p.operation_bindings.clear();
            let grant = email
                .and_then(|email| grants.get(&email.to_ascii_lowercase()))
                .filter(|g| g.active);
            if let Some(ops) = grant.and_then(|g| g.policy.allowed_operations.as_ref()) {
                p.allowed_operations.retain(|op| ops.contains(op));
            }
        }
        p
    })
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn capability_policy_shared_fixture_preserves_empty_set_and_rejects_unknown_authority() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/share-capability-policy-v1.json"
        ))
        .unwrap();
        let empty: ShareCapabilityPolicy =
            serde_json::from_value(fixture["emptyPolicy"].clone()).unwrap();
        empty.validate().unwrap();
        assert_eq!(
            serde_json::to_value(&empty).unwrap(),
            fixture["emptyPolicy"]
        );
        assert_eq!(
            serde_json::to_value(ShareOperation::ALL).unwrap(),
            fixture["operations"]
        );
        assert!(
            serde_json::from_value::<ShareCapabilityPolicy>(
                json!({"version":1,"allowedOperations":["future.operation"],"operationBindings":{}})
            )
            .is_err()
        );
        assert!(serde_json::from_value::<ShareCapabilityPolicy>(json!({"version":1,"allowedOperations":[],"operationBindings":{},"permissionBypass":true})).is_err());
        let duplicate:ShareCapabilityPolicy=serde_json::from_value(json!({"version":1,"allowedOperations":["image.generate","image.generate"],"operationBindings":{}})).unwrap();
        assert!(duplicate.validate().is_err());
    }
}
