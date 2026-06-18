//! Control-plane RPC client: server → installation.
//!
//! The dashboard is allowed to *request* share-settings changes, but the
//! desktop installation remains the single source of truth for its own config.
//! Rather than wait for the client to poll the pending-edit queue and re-sync
//! (slow, and lossy if the client applies a patch only partially), the server
//! calls the client's local `/_ctl/apply_share_settings` API synchronously over
//! the existing reverse SSH forward and lets the client apply + report back the
//! authoritative result. The server never mutates the returned descriptor; it
//! only validates it (see `store::apply_share_edit_directly`).
//!
//! Auth is a per-installation symmetric secret (issued at registration),
//! HMAC-SHA256 over a canonical string. This is independent of the client's
//! Ed25519 keypair and does not require the server to hold any private key.

use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::time::Duration;

use crate::models::{ShareDescriptor, ShareSettingsPatch};

type HmacSha256 = Hmac<Sha256>;

const APPLY_SHARE_SETTINGS_PATH: &str = "/_ctl/apply_share_settings";
const REFRESH_SHARE_USAGE_PATH: &str = "/_ctl/refresh_share_usage";
const CTL_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApplyShareSettingsBody<'a> {
    share_id: &'a str,
    patch: &'a ShareSettingsPatch,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApplyShareSettingsReply {
    #[serde(default)]
    ok: bool,
    share: Option<ShareDescriptor>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RefreshShareUsageBody<'a> {
    share_id: &'a str,
    app: Option<&'a str>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshShareUsageItem {
    pub app: String,
    pub provider_id: Option<String>,
    pub provider_name: Option<String>,
    pub auth_provider: Option<String>,
    #[serde(default)]
    pub refreshed: bool,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshShareUsageReply {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub refreshed: Vec<RefreshShareUsageItem>,
}

/// Why a control RPC did not produce an authoritative result. The caller maps
/// `Unreachable`/`Timeout` to the async fallback (pending edit + SSE) and maps
/// `Rejected`/`Malformed` to a hard error surfaced to the dashboard.
#[derive(Debug)]
pub enum CtlError {
    /// Could not establish a connection to the client's tunnel backend.
    Unreachable(String),
    /// Connected but the client did not answer within the deadline.
    Timeout,
    /// Client answered with a non-success HTTP status.
    Rejected { status: u16, body: String },
    /// Client answered 2xx but the payload was missing/!ok/unparseable.
    Malformed(String),
}

impl std::fmt::Display for CtlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CtlError::Unreachable(msg) => write!(f, "control client unreachable: {msg}"),
            CtlError::Timeout => write!(f, "control client timed out"),
            CtlError::Rejected { status, body } => {
                write!(f, "control client rejected ({status}): {body}")
            }
            CtlError::Malformed(msg) => write!(f, "control client malformed reply: {msg}"),
        }
    }
}

impl CtlError {
    /// True when the failure is a transport problem (offline/timeout) and the
    /// caller should fall back to the async pending-edit path rather than
    /// failing the dashboard request.
    pub fn is_transport(&self) -> bool {
        matches!(self, CtlError::Unreachable(_) | CtlError::Timeout)
    }
}

/// Canonical string signed by both sides:
/// `METHOD\nPATH\n<body>\n<timestamp_ms>\n<nonce>`
fn signature(path: &str, secret: &str, body: &str, timestamp_ms: i64, nonce: &str) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any size");
    mac.update(b"POST\n");
    mac.update(path.as_bytes());
    mac.update(b"\n");
    mac.update(body.as_bytes());
    mac.update(b"\n");
    mac.update(timestamp_ms.to_string().as_bytes());
    mac.update(b"\n");
    mac.update(nonce.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

async fn post_control<T: serde::de::DeserializeOwned>(
    backend: &str,
    installation_id: &str,
    control_secret: &str,
    path: &str,
    body: String,
) -> Result<T, CtlError> {
    let timestamp_ms = chrono::Utc::now().timestamp_millis();
    let nonce = uuid::Uuid::new_v4().to_string();
    let sig = signature(path, control_secret, &body, timestamp_ms, &nonce);

    let client = reqwest::Client::builder()
        .timeout(CTL_TIMEOUT)
        .build()
        .map_err(|e| CtlError::Unreachable(format!("build http client failed: {e}")))?;

    let url = format!("http://{backend}{path}");
    let resp = client
        .post(&url)
        .header("content-type", "application/json")
        .header("x-ctl-installation-id", installation_id)
        .header("x-ctl-timestamp-ms", timestamp_ms.to_string())
        .header("x-ctl-nonce", &nonce)
        .header("x-ctl-signature", sig)
        .body(body)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                CtlError::Timeout
            } else {
                CtlError::Unreachable(e.to_string())
            }
        })?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(CtlError::Rejected {
            status: status.as_u16(),
            body: body.chars().take(500).collect(),
        });
    }
    resp.json()
        .await
        .map_err(|e| CtlError::Malformed(e.to_string()))
}

/// Synchronously ask the installation behind `backend` (a `host:port` tunnel
/// target) to apply `patch` to `share_id`, returning the descriptor the client
/// actually wrote. `backend` comes from `RouteEntry::route_target()`.
pub async fn apply_share_settings(
    backend: &str,
    installation_id: &str,
    control_secret: &str,
    share_id: &str,
    patch: &ShareSettingsPatch,
) -> Result<ShareDescriptor, CtlError> {
    let body = serde_json::to_string(&ApplyShareSettingsBody { share_id, patch })
        .map_err(|e| CtlError::Malformed(format!("serialize control body failed: {e}")))?;
    let reply: ApplyShareSettingsReply = post_control(
        backend,
        installation_id,
        control_secret,
        APPLY_SHARE_SETTINGS_PATH,
        body,
    )
    .await?;
    if !reply.ok {
        return Err(CtlError::Malformed("client replied ok=false".into()));
    }
    reply
        .share
        .ok_or_else(|| CtlError::Malformed("client reply missing share".into()))
}

pub async fn refresh_share_usage(
    backend: &str,
    installation_id: &str,
    control_secret: &str,
    share_id: &str,
    app: Option<&str>,
) -> Result<RefreshShareUsageReply, CtlError> {
    let body = serde_json::to_string(&RefreshShareUsageBody { share_id, app })
        .map_err(|e| CtlError::Malformed(format!("serialize control body failed: {e}")))?;
    let reply: RefreshShareUsageReply = post_control(
        backend,
        installation_id,
        control_secret,
        REFRESH_SHARE_USAGE_PATH,
        body,
    )
    .await?;
    if !reply.ok {
        return Err(CtlError::Malformed("client replied ok=false".into()));
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_is_stable_and_key_sensitive() {
        let body = r#"{"shareId":"s1","patch":{"ownerEmail":"a@b.com"}}"#;
        let a = signature(
            APPLY_SHARE_SETTINGS_PATH,
            "secret-1",
            body,
            1700000000000,
            "nonce-1",
        );
        let b = signature(
            APPLY_SHARE_SETTINGS_PATH,
            "secret-1",
            body,
            1700000000000,
            "nonce-1",
        );
        assert_eq!(a, b, "same inputs must produce same signature");
        let c = signature(
            APPLY_SHARE_SETTINGS_PATH,
            "secret-2",
            body,
            1700000000000,
            "nonce-1",
        );
        assert_ne!(a, c, "different secret must change signature");
        let d = signature(
            APPLY_SHARE_SETTINGS_PATH,
            "secret-1",
            body,
            1700000000001,
            "nonce-1",
        );
        assert_ne!(a, d, "different timestamp must change signature");
        let e = signature(
            APPLY_SHARE_SETTINGS_PATH,
            "secret-1",
            body,
            1700000000000,
            "nonce-2",
        );
        assert_ne!(a, e, "different nonce must change signature");
        let f = signature(
            REFRESH_SHARE_USAGE_PATH,
            "secret-1",
            body,
            1700000000000,
            "nonce-1",
        );
        assert_ne!(a, f, "different path must change signature");
    }

    #[test]
    fn transport_errors_fall_back_others_do_not() {
        assert!(CtlError::Timeout.is_transport());
        assert!(CtlError::Unreachable("x".into()).is_transport());
        assert!(
            !CtlError::Rejected {
                status: 422,
                body: "x".into()
            }
            .is_transport()
        );
        assert!(!CtlError::Malformed("x".into()).is_transport());
    }
}
