use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::namespace::PROTOCOL_EPOCH;

pub const INGRESS_CONTEXT_HEADER: &str = "x-cc-switch-ingress-context";
pub const INGRESS_SIGNATURE_HEADER: &str = "x-cc-switch-ingress-signature";
const SIGNING_DOMAIN: &str = "cc-switch-router-ingress-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IngressContext {
    pub protocol_epoch: String,
    pub router_id: String,
    pub route_id: String,
    pub installation_id: String,
    pub target_lane_id: String,
    pub public_host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_id: Option<String>,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_country: Option<String>,
    pub issued_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedIngressContext {
    pub encoded_context: String,
    pub signature: String,
}

pub fn sign(
    mut context: IngressContext,
    control_secret: &str,
) -> Result<SignedIngressContext, &'static str> {
    validate(&context, control_secret)?;
    context.protocol_epoch = PROTOCOL_EPOCH.to_string();
    let encoded_context = URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&context).map_err(|_| "serialize ingress context failed")?);
    let mut mac = Hmac::<Sha256>::new_from_slice(control_secret.as_bytes())
        .map_err(|_| "invalid ingress control secret")?;
    mac.update(SIGNING_DOMAIN.as_bytes());
    mac.update(b"\n");
    mac.update(PROTOCOL_EPOCH.as_bytes());
    mac.update(b"\n");
    mac.update(encoded_context.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    Ok(SignedIngressContext {
        encoded_context,
        signature,
    })
}

fn validate(context: &IngressContext, control_secret: &str) -> Result<(), &'static str> {
    if control_secret.len() < 32 {
        return Err("ingress control secret is too short");
    }
    if context.router_id.trim().is_empty()
        || context.route_id.trim().is_empty()
        || context.installation_id.trim().is_empty()
        || context.target_lane_id.trim().is_empty()
        || context.public_host.trim().is_empty()
        || context.request_id.trim().is_empty()
        || context.issued_at_ms <= 0
    {
        return Err("ingress context contains an empty required field");
    }
    if context
        .user_email
        .as_deref()
        .is_some_and(|value| value != value.trim() || value.is_empty())
    {
        return Err("ingress user email is not normalized");
    }
    if context
        .user_role
        .as_deref()
        .is_some_and(|value| !matches!(value, "owner" | "admin"))
    {
        return Err("ingress user role is invalid");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> IngressContext {
        IngressContext {
            protocol_epoch: PROTOCOL_EPOCH.into(),
            router_id: "router-jp".into(),
            route_id: "share:share-1".into(),
            installation_id: "installation-1".into(),
            target_lane_id: "installation-1:namespace-data".into(),
            public_host: "codex--alpha-iosg6hiidutqcmhceefb.router.test".into(),
            share_id: Some("share-1".into()),
            request_id: "req_123".into(),
            user_email: Some("owner@example.com".into()),
            user_role: None,
            user_country: Some("JP".into()),
            issued_at_ms: 1_750_000_000_000,
        }
    }

    #[test]
    fn signing_is_stable_and_covers_every_semantic_field() {
        let secret = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGH";
        let signed = sign(context(), secret).unwrap();
        assert_eq!(
            signed.signature,
            "RvdTGpCCJwSxo7Kn8meZ0Vx3MaHf3YocqnzKyqJxTeU"
        );
        let mut changed = context();
        changed.target_lane_id.push_str("-changed");
        assert_ne!(sign(changed, secret).unwrap().signature, signed.signature);
    }

    #[test]
    fn rejects_short_secrets_and_unbound_contexts() {
        assert!(sign(context(), "short").is_err());
        let mut missing_route = context();
        missing_route.route_id.clear();
        assert!(sign(missing_route, &"x".repeat(32)).is_err());
    }
}
