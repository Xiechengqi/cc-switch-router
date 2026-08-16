use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::namespace::PROTOCOL_EPOCH;

pub const INGRESS_CONTEXT_HEADER: &str = "x-cc-switch-ingress-context";
pub const INGRESS_SIGNATURE_HEADER: &str = "x-cc-switch-ingress-signature";
/// Router 为本次请求实际应用的请求体上限（字节，十进制）。
///
/// 该头**不参与签名**：它只承载 Router 侧的天花板声明，Client 永远取
/// `min(本地上限, 声明值)`。因此伪造只能把上限压低（伪造者自伤），无法抬高
/// Client 的本地配置。同时 `is_internal_share_context_header()` 会剥离来自
/// 公网的同名头，客户端看到的值只可能由 Router 写入。
///
/// 旧版 Client 不认识该头，会沿用自身硬编码上限；旧版 Router 不发送该头，
/// 新版 Client 会回退到历史默认值。两个方向都可独立升级。
pub const INGRESS_BODY_LIMIT_HEADER: &str = "x-cc-switch-ingress-body-limit";
pub const INTERNAL_INGRESS_ERROR_HEADER: &str = "x-cc-switch-internal-ingress-error";
pub const INTERNAL_INGRESS_AGE_MS_HEADER: &str = "x-cc-switch-internal-ingress-age-ms";
pub const INTERNAL_INGRESS_SERVER_TIME_MS_HEADER: &str =
    "x-cc-switch-internal-ingress-server-time-ms";
pub const SIGNATURE_VERSION: u8 = 2;
const SIGNING_DOMAIN: &str = "cc-switch-router-ingress-v2";
const SHA256_HEX_LENGTH: usize = 64;
const MAX_PATH_AND_QUERY_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IngressContext {
    pub signature_version: u8,
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
    pub method: String,
    pub path_and_query: String,
    pub body_sha256: String,
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
    context.signature_version = SIGNATURE_VERSION;
    context.protocol_epoch = PROTOCOL_EPOCH.to_string();
    validate(&context, control_secret)?;
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
    if context.signature_version != SIGNATURE_VERSION
        || context.router_id.trim().is_empty()
        || context.route_id.trim().is_empty()
        || context.installation_id.trim().is_empty()
        || context.target_lane_id.trim().is_empty()
        || context.public_host.trim().is_empty()
        || context.request_id.trim().is_empty()
        || context.issued_at_ms <= 0
    {
        return Err("ingress context contains an empty required field");
    }
    if normalize_method(&context.method).as_deref() != Some(context.method.as_str())
        || normalize_path_and_query(&context.path_and_query).as_deref()
            != Some(context.path_and_query.as_str())
        || context.body_sha256.len() != SHA256_HEX_LENGTH
        || !context
            .body_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("ingress request binding is invalid");
    }
    if context.user_email.as_deref().is_some_and(|value| {
        value != value.trim()
            || value.is_empty()
            || value != value.to_ascii_lowercase()
            || !value.contains('@')
    }) {
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

pub fn body_sha256_hex(body: &[u8]) -> String {
    hex::encode(Sha256::digest(body))
}

pub fn normalize_method(method: &str) -> Option<String> {
    let method = method.trim();
    (!method.is_empty()
        && method.len() <= 16
        && method.bytes().all(|byte| byte.is_ascii_uppercase()))
    .then(|| method.to_string())
}

pub fn normalize_path_and_query(path_and_query: &str) -> Option<String> {
    let target = path_and_query.trim();
    (target.starts_with('/')
        && target.len() <= MAX_PATH_AND_QUERY_BYTES
        && !target.contains('#')
        && !target.bytes().any(|byte| byte.is_ascii_control()))
    .then(|| target.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingress_body_limit_header_is_a_distinct_valid_header_name() {
        // `HeaderName::from_static` 要求全小写；Client 侧也按同名常量解析。
        let name = axum::http::HeaderName::from_static(INGRESS_BODY_LIMIT_HEADER);
        assert_eq!(name.as_str(), INGRESS_BODY_LIMIT_HEADER);
        assert_ne!(INGRESS_BODY_LIMIT_HEADER, INGRESS_CONTEXT_HEADER);
        assert_ne!(INGRESS_BODY_LIMIT_HEADER, INGRESS_SIGNATURE_HEADER);
    }

    fn context() -> IngressContext {
        IngressContext {
            signature_version: SIGNATURE_VERSION,
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
            method: "POST".into(),
            path_and_query: "/v1/messages?beta=true".into(),
            body_sha256: body_sha256_hex(br#"{"model":"claude-sonnet-4-6"}"#),
            issued_at_ms: 1_750_000_000_000,
        }
    }

    #[test]
    fn signing_is_stable_and_covers_every_semantic_field() {
        let secret = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGH";
        let signed = sign(context(), secret).unwrap();
        assert_eq!(
            signed.signature,
            "J1a63NviixVTTd2fuMrF3P696OeA-JP_abaLzW7PVEg"
        );
        for changed in [
            {
                let mut changed = context();
                changed.target_lane_id.push_str("-changed");
                changed
            },
            {
                let mut changed = context();
                changed.request_id.push_str("-changed");
                changed
            },
            {
                let mut changed = context();
                changed.method = "GET".into();
                changed
            },
            {
                let mut changed = context();
                changed.path_and_query.push_str("&changed=true");
                changed
            },
            {
                let mut changed = context();
                changed.body_sha256 = body_sha256_hex(b"changed");
                changed
            },
        ] {
            assert_ne!(sign(changed, secret).unwrap().signature, signed.signature);
        }
    }

    #[test]
    fn rejects_short_secrets_and_unbound_contexts() {
        assert!(sign(context(), "short").is_err());
        let mut missing_route = context();
        missing_route.route_id.clear();
        assert!(sign(missing_route, &"x".repeat(32)).is_err());
    }

    #[test]
    fn signed_user_email_is_canonical_and_signature_bound() {
        let secret = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGH";
        let signed = sign(context(), secret).unwrap();

        let mut changed = context();
        changed.user_email = Some("buyer@example.com".into());
        assert_ne!(sign(changed, secret).unwrap().signature, signed.signature);

        for email in [
            "Owner@example.com",
            " owner@example.com",
            "owner.example.com",
            "",
        ] {
            let mut invalid = context();
            invalid.user_email = Some(email.into());
            assert!(sign(invalid, secret).is_err(), "accepted {email:?}");
        }
    }
}
