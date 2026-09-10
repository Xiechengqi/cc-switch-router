use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use futures_util::StreamExt;
use rand::RngCore;
use reqwest::header::RETRY_AFTER;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::{Host, Url};
use zeroize::{Zeroize, Zeroizing};

use crate::config::BarkSettings;
use crate::error::AppError;

pub const CHANNEL: &str = "bark";
const ENVELOPE_VERSION: &str = "v1";
const MAX_BIND_ATTEMPTS_PER_USER_HOUR: i64 = 5;
const MAX_BIND_ATTEMPTS_PER_IP_HOUR: i64 = 20;
const PROVIDER_CIRCUIT_FAILURE_THRESHOLD: i64 = 3;
const PROVIDER_CIRCUIT_OPEN_SECS: i64 = 5 * 60;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_SERVER_URL_BYTES: usize = 2 * 1024;
const MAX_PUSH_URL_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Default)]
pub struct ProviderRuntime {
    /// True only when the durable runtime row is enabled for the requested
    /// Provider identity. A missing/mismatched row must fail closed instead of
    /// being mistaken for a freshly initialized, ready Provider.
    pub configuration_active: bool,
    pub status: String,
    pub consecutive_failures: i64,
    pub circuit_open_until: Option<String>,
    pub last_error: Option<String>,
    pub last_failure_code: Option<String>,
    pub last_failure_hint: Option<String>,
    pub last_failure_at: Option<String>,
    pub last_success_at: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BindRequest {
    pub push_url: String,
}

#[derive(Clone)]
pub struct CredentialCipher {
    key: Zeroizing<Vec<u8>>,
    version: i64,
    key_fingerprint: String,
}

pub struct BindingRuntime<'a> {
    pub provider_identity: &'a str,
    pub binding_config_fingerprint: &'a str,
    pub cipher: &'a CredentialCipher,
}

impl CredentialCipher {
    pub fn from_settings(settings: &BarkSettings) -> Result<Option<Self>, AppError> {
        let Some(raw) = settings
            .credential_master_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };
        let mut decoded = if raw.len() == 64 && raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            hex::decode(raw)
                .map_err(|_| AppError::BadRequest("Bark credential master key is invalid".into()))?
        } else {
            STANDARD.decode(raw).map_err(|_| {
                AppError::BadRequest(
                    "Bark credential master key must be 64 hexadecimal characters or base64".into(),
                )
            })?
        };
        if decoded.len() != 32 || settings.credential_key_version < 1 {
            decoded.zeroize();
            return Err(AppError::BadRequest(
                "Bark credential master key must decode to exactly 32 bytes and its version must be positive"
                    .into(),
            ));
        }
        let key_fingerprint = credential_key_fingerprint(&decoded, settings.credential_key_version);
        Ok(Some(Self {
            key: Zeroizing::new(decoded),
            version: settings.credential_key_version,
            key_fingerprint,
        }))
    }

    pub fn version(&self) -> i64 {
        self.version
    }

    pub fn key_fingerprint(&self) -> &str {
        &self.key_fingerprint
    }

    pub fn seal(&self, secret: &str, aad: &str) -> Result<String, AppError> {
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&self.key));
        let mut nonce_bytes = [0_u8; 24];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&nonce_bytes),
                Payload {
                    msg: secret.as_bytes(),
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| AppError::Internal("encrypt Bark device key failed".into()))?;
        Ok(format!(
            "{ENVELOPE_VERSION}:{}:{}:{}",
            self.version,
            URL_SAFE_NO_PAD.encode(nonce_bytes),
            URL_SAFE_NO_PAD.encode(ciphertext)
        ))
    }

    pub fn open(&self, envelope: &str, aad: &str) -> Result<Zeroizing<String>, AppError> {
        let mut parts = envelope.split(':');
        let version = parts.next();
        let key_version = parts.next().and_then(|value| value.parse::<i64>().ok());
        let nonce = parts.next();
        let ciphertext = parts.next();
        if version != Some(ENVELOPE_VERSION)
            || key_version != Some(self.version)
            || nonce.is_none()
            || ciphertext.is_none()
            || parts.next().is_some()
        {
            return Err(AppError::Conflict(
                "Bark binding was encrypted with an unavailable key; rebind is required".into(),
            ));
        }
        let nonce = URL_SAFE_NO_PAD
            .decode(nonce.unwrap_or_default())
            .map_err(|_| AppError::Internal("decode Bark credential nonce failed".into()))?;
        let ciphertext = URL_SAFE_NO_PAD
            .decode(ciphertext.unwrap_or_default())
            .map_err(|_| AppError::Internal("decode Bark credential failed".into()))?;
        if nonce.len() != 24 {
            return Err(AppError::Internal(
                "stored Bark credential nonce is invalid".into(),
            ));
        }
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&self.key));
        let plaintext = cipher
            .decrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| {
                AppError::Conflict("Bark binding cannot be decrypted; rebind is required".into())
            })?;
        let plaintext = Zeroizing::new(plaintext);
        let value = std::str::from_utf8(&plaintext)
            .map_err(|_| AppError::Internal("stored Bark credential is not UTF-8".into()))?
            .to_owned();
        Ok(Zeroizing::new(value))
    }
}

pub fn credential_aad(user_id: &str, revision: i64, provider_identity: &str) -> String {
    format!("bark-device-key:{user_id}:{revision}:{provider_identity}")
}

fn credential_key_fingerprint(key: &[u8], version: i64) -> String {
    let mut digest = Sha256::new();
    digest.update(b"cc-switch-router-bark-credential-key-v1\0");
    digest.update(version.to_be_bytes());
    digest.update(key);
    hex::encode(digest.finalize())
}

pub fn binding_config_fingerprint(
    provider_identity: &str,
    credential_key_fingerprint: &str,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"cc-switch-router-bark-binding-config-v1\0");
    digest.update(provider_identity.as_bytes());
    digest.update(b"\0");
    digest.update(credential_key_fingerprint.as_bytes());
    hex::encode(digest.finalize())
}

pub fn normalize_server_url(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.len() > MAX_SERVER_URL_BYTES || raw.chars().any(char::is_control) {
        return Err("Bark Server URL is too long or contains control characters".into());
    }
    let mut url = Url::parse(raw).map_err(|_| "Bark Server must be a valid URL")?;
    let secure = url.scheme() == "https";
    let loopback_http = url.scheme() == "http"
        && match url.host() {
            Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
            Some(Host::Ipv4(address)) => address.is_loopback(),
            Some(Host::Ipv6(address)) => address.is_loopback(),
            None => false,
        };
    if !secure && !loopback_http {
        return Err(
            "Bark Server must use HTTPS; HTTP is allowed only for a loopback Server".into(),
        );
    }
    if url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "Bark Server must not contain credentials, query parameters, or a fragment".into(),
        );
    }
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(&path);
    Ok(url.to_string().trim_end_matches('/').to_string())
}

pub fn provider_identity(server_url: &str) -> Result<String, String> {
    normalize_server_url(server_url).map(|value| hex::encode(Sha256::digest(value.as_bytes())))
}

pub fn target_fingerprint(provider_identity: &str, device_key: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"bark-target-v1\0");
    digest.update(provider_identity.as_bytes());
    digest.update(b"\0");
    digest.update(device_key.as_bytes());
    hex::encode(digest.finalize())
}

pub struct ParsedPushUrl {
    pub device_key: Zeroizing<String>,
    pub masked_key: String,
}

pub fn parse_push_url(push_url: &str, configured_server: &str) -> Result<ParsedPushUrl, AppError> {
    let server = normalize_server_url(configured_server).map_err(AppError::BadRequest)?;
    let push_url = push_url.trim();
    if push_url.len() > MAX_PUSH_URL_BYTES || push_url.chars().any(char::is_control) {
        return Err(AppError::BadRequest(
            "Bark Push URL is too long or contains control characters".into(),
        ));
    }
    let url = Url::parse(push_url)
        .map_err(|_| AppError::BadRequest("Bark Push URL is invalid".into()))?;
    if url.query().is_some()
        || url.fragment().is_some()
        || url.username() != ""
        || url.password().is_some()
    {
        return Err(AppError::BadRequest(
            "Bark Push URL must not contain credentials, query parameters, or a fragment".into(),
        ));
    }
    let server_url = Url::parse(&server)
        .map_err(|_| AppError::Internal("configured Bark Server URL is invalid".into()))?;
    if url.scheme() != server_url.scheme()
        || url.host_str().map(str::to_ascii_lowercase)
            != server_url.host_str().map(str::to_ascii_lowercase)
        || url.port_or_known_default() != server_url.port_or_known_default()
    {
        return Err(AppError::BadRequest(
            "Bark Push URL does not belong to the configured Bark Server".into(),
        ));
    }
    let base_segments = server_url
        .path_segments()
        .map(|parts| parts.filter(|part| !part.is_empty()).collect::<Vec<_>>())
        .unwrap_or_default();
    let segments = url
        .path_segments()
        .map(|parts| parts.filter(|part| !part.is_empty()).collect::<Vec<_>>())
        .unwrap_or_default();
    if segments.len() != base_segments.len() + 1 || !segments.starts_with(&base_segments) {
        return Err(AppError::BadRequest(
            "Bark Push URL must contain exactly one device key after the configured Server path"
                .into(),
        ));
    }
    let device_key =
        percent_encoding::percent_decode_str(segments.last().copied().unwrap_or_default())
            .decode_utf8()
            .map_err(|_| AppError::BadRequest("Bark device key is not valid UTF-8".into()))?
            .into_owned();
    validate_device_key(&device_key)?;
    Ok(ParsedPushUrl {
        masked_key: mask_device_key(&device_key),
        device_key: Zeroizing::new(device_key),
    })
}

pub fn validate_device_key(value: &str) -> Result<(), AppError> {
    if !(8..=256).contains(&value.len())
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(AppError::BadRequest(
            "Bark device key must be 8 to 256 ASCII letters, digits, underscores, or hyphens"
                .into(),
        ));
    }
    Ok(())
}

pub fn mask_device_key(value: &str) -> String {
    let suffix = value
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("••••{suffix}")
}

pub fn build_http_client(user_agent: &str) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent(user_agent)
        // Bark requests contain the private device key in their JSON body.
        // Never let a 307/308 response replay that body to another origin (or
        // downgrade it to plaintext HTTP / redirect it toward an internal URL).
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(8)
        .tcp_keepalive(Duration::from_secs(60))
        .build()
}

#[derive(Serialize)]
pub struct PushRequest<'a> {
    pub device_key: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub group: &'a str,
    pub url: &'a str,
    pub id: &'a str,
    pub level: &'a str,
}

#[derive(Debug, Clone)]
pub struct SendSuccess {
    pub provider_message_id: Option<String>,
    pub http_status: u16,
}

#[derive(Debug, Clone)]
pub struct SendFailure {
    pub retryable: bool,
    pub retry_at: Option<i64>,
    pub target_invalid: bool,
    pub http_status: Option<u16>,
    pub message: String,
    pub code: String,
    pub hint: String,
}

#[derive(Deserialize)]
struct BarkResponse {
    code: Option<i64>,
    message: Option<String>,
    data: Option<serde_json::Value>,
}

pub async fn send(
    http: &reqwest::Client,
    server_url: &str,
    request: &PushRequest<'_>,
) -> Result<SendSuccess, SendFailure> {
    let server = normalize_server_url(server_url).map_err(|message| SendFailure {
        retryable: false,
        retry_at: None,
        target_invalid: false,
        http_status: None,
        message,
        code: "configuration".into(),
        hint: "Check the configured Bark Server URL.".into(),
    })?;
    let endpoint = format!("{server}/push");
    let response = http
        .post(endpoint)
        .json(request)
        .send()
        .await
        .map_err(|error| SendFailure {
            retryable: true,
            retry_at: None,
            target_invalid: false,
            http_status: error.status().map(|status| status.as_u16()),
            message: sanitize_error(&error.to_string()),
            code: if error.is_timeout() {
                "api_timeout"
            } else {
                "network"
            }
            .into(),
            hint: "The Bark Server could not be reached; Router will retry with backoff.".into(),
        })?;
    let status = response.status();
    let retry_at = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(parse_retry_after);
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(SendFailure {
            retryable: false,
            retry_at: None,
            target_invalid: false,
            http_status: Some(status.as_u16()),
            message: "Bark response exceeded 64 KiB".into(),
            code: "provider_protocol".into(),
            hint: "The Bark Server returned an unexpectedly large response.".into(),
        });
    }
    let mut response_bytes = Vec::new();
    let mut response_stream = response.bytes_stream();
    while let Some(chunk) = response_stream.next().await {
        let chunk = chunk.map_err(|error| SendFailure {
            retryable: true,
            retry_at: None,
            target_invalid: false,
            http_status: Some(status.as_u16()),
            message: sanitize_error(&format!("read Bark response failed: {error}")),
            code: "network".into(),
            hint: "The Bark Server response was interrupted; Router will retry with backoff."
                .into(),
        })?;
        if response_bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(SendFailure {
                retryable: false,
                retry_at: None,
                target_invalid: false,
                http_status: Some(status.as_u16()),
                message: "Bark response exceeded 64 KiB".into(),
                code: "provider_protocol".into(),
                hint: "The Bark Server returned an unexpectedly large response.".into(),
            });
        }
        response_bytes.extend_from_slice(&chunk);
    }
    let body = serde_json::from_slice::<BarkResponse>(&response_bytes);
    if status.is_success() {
        return match body {
            Ok(payload) if payload.code == Some(200) => Ok(SendSuccess {
                provider_message_id: payload
                    .data
                    .as_ref()
                    .and_then(|value| provider_message_id(value, request.device_key)),
                http_status: status.as_u16(),
            }),
            Ok(payload) => match payload.code.and_then(|code| u16::try_from(code).ok()) {
                Some(code) => Err(classified_failure(
                    code,
                    Some(status.as_u16()),
                    retry_at,
                    payload
                        .message
                        .as_deref()
                        .unwrap_or("Bark returned a non-success code"),
                    request.device_key,
                )),
                None => Err(SendFailure {
                    retryable: true,
                    retry_at: None,
                    target_invalid: false,
                    http_status: Some(status.as_u16()),
                    message: "Bark returned a response without a valid status code".into(),
                    code: "provider_protocol".into(),
                    hint: "The Bark Server returned an unexpected response.".into(),
                }),
            },
            Err(error) => Err(SendFailure {
                retryable: true,
                retry_at: None,
                target_invalid: false,
                http_status: Some(status.as_u16()),
                message: sanitize_error(&format!("invalid Bark response: {error}")),
                code: "provider_protocol".into(),
                hint: "The Bark Server returned an unexpected response.".into(),
            }),
        };
    }
    let status_code = status.as_u16();
    let provider_message = body
        .ok()
        .and_then(|payload| payload.message)
        .unwrap_or_else(|| format!("Bark HTTP {status_code}"));
    Err(classified_failure(
        status_code,
        Some(status_code),
        retry_at,
        &provider_message,
        request.device_key,
    ))
}

fn classified_failure(
    status_code: u16,
    http_status: Option<u16>,
    retry_at: Option<i64>,
    provider_message: &str,
    device_key: &str,
) -> SendFailure {
    let retryable = matches!(status_code, 408 | 425 | 429 | 500..=599);
    // bark-server uses HTTP/application 400 for several unrelated failures
    // (malformed JSON, route parsing, database outages, and unknown device
    // keys). Only the stable device-lookup errors are safe grounds for
    // deleting a user's binding. In particular, a reverse-proxy 404 says
    // nothing about the device key.
    let target_invalid = bark_target_invalid(status_code, provider_message, device_key);
    SendFailure {
        retryable,
        retry_at,
        target_invalid,
        http_status,
        message: sanitize_provider_error(provider_message, device_key),
        code: match status_code {
            _ if target_invalid => "target_invalid",
            401 | 403 => "authorization",
            400 | 404 | 405 => "provider_protocol",
            429 => "rate_limited",
            500..=599 => "server_error",
            _ => "http_error",
        }
        .into(),
        hint: if target_invalid {
            "Bark rejected this device key; bind the channel again."
        } else if retryable {
            "Bark is temporarily unavailable; Router will retry with backoff."
        } else {
            "Check the Bark Server and reverse-proxy configuration."
        }
        .into(),
    }
}

fn bark_target_invalid(status_code: u16, provider_message: &str, device_key: &str) -> bool {
    if status_code != 400 {
        return false;
    }
    let message = provider_message.trim().to_ascii_lowercase();
    let Some(reason) = message.strip_prefix("failed to get device token:") else {
        return false;
    };
    let reason = reason.trim();
    if matches!(
        reason,
        "key not found" | "device token invalid" | "sql: no rows in result set"
    ) {
        return true;
    }

    // The bbolt backend embeds the requested key in its missing-row error.
    // Match the entire stable error, including this request's key: accepting a
    // generic "device token from database" substring could turn an outage in
    // a custom backend into destructive credential invalidation.
    reason
        == format!(
            "failed to get [{}] device token from database",
            device_key.to_ascii_lowercase()
        )
}

fn provider_message_id(value: &serde_json::Value, device_key: &str) -> Option<String> {
    value
        .get("id")
        .and_then(|value| {
            value
                .as_str()
                .map(str::to_string)
                .or_else(|| value.as_i64().map(|value| value.to_string()))
        })
        .map(|value| sanitize_provider_error(&value, device_key))
}

fn parse_retry_after(value: &reqwest::header::HeaderValue) -> Option<i64> {
    let raw = value.to_str().ok()?.trim();
    let now = chrono::Utc::now().timestamp();
    let earliest_retry = now.saturating_add(1);
    let latest_retry = now.saturating_add(24 * 60 * 60);
    if let Ok(seconds) = raw.parse::<i64>() {
        return Some(now.saturating_add(seconds.clamp(1, 24 * 60 * 60)));
    }
    httpdate::parse_http_date(raw)
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .map(|timestamp| timestamp.clamp(earliest_retry, latest_retry))
}

fn sanitize_error(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(500)
        .collect()
}

fn sanitize_provider_error(value: &str, device_key: &str) -> String {
    let sanitized = sanitize_error(value);
    if device_key.is_empty() {
        sanitized
    } else {
        redact_ascii_case_insensitive(&sanitized, device_key)
    }
}

fn redact_ascii_case_insensitive(value: &str, secret: &str) -> String {
    let secret = secret.as_bytes();
    if secret.is_empty() || value.len() < secret.len() {
        return value.to_string();
    }

    let bytes = value.as_bytes();
    let mut output = String::with_capacity(value.len());
    let mut copied_through = 0;
    let mut cursor = 0;
    while cursor + secret.len() <= bytes.len() {
        if value.is_char_boundary(cursor)
            && value.is_char_boundary(cursor + secret.len())
            && bytes[cursor..cursor + secret.len()].eq_ignore_ascii_case(secret)
        {
            output.push_str(&value[copied_through..cursor]);
            output.push_str("[REDACTED]");
            cursor += secret.len();
            copied_through = cursor;
        } else {
            cursor += value[cursor..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
        }
    }
    output.push_str(&value[copied_through..]);
    output
}

/// Publish the exact Bark configuration that may create encrypted bindings.
/// The monotonically increasing generation fences an HTTP verification that
/// started before an enable/server/key transition, including a transition away
/// from and back to the same values.
pub(crate) fn sync_provider_configuration_tx(
    conn: &crate::db::Connection,
    enabled: bool,
    provider_identity: Option<&str>,
    binding_config_fingerprint: Option<&str>,
    now: &str,
) -> Result<i64, AppError> {
    use crate::db::params;

    let enabled = enabled && provider_identity.is_some() && binding_config_fingerprint.is_some();
    let current = conn
        .query_row(
            "SELECT config_fingerprint, binding_config_fingerprint, generation, enabled
             FROM bark_provider_runtime WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)? != 0,
                ))
            },
        )
        .map_err(|error| {
            AppError::Internal(format!("read Bark provider configuration failed: {error}"))
        })?;
    let changed = current.0.as_deref() != provider_identity
        || current.1.as_deref() != binding_config_fingerprint
        || current.3 != enabled;
    let generation = current.2.saturating_add(i64::from(changed));
    if changed {
        conn.execute(
            "UPDATE bark_provider_runtime
             SET config_fingerprint = ?1, binding_config_fingerprint = ?2,
                 generation = ?3, enabled = ?4, status = ?5,
                 consecutive_failures = 0, circuit_open_until = NULL,
                 last_error = NULL, last_failure_code = NULL,
                 last_failure_hint = NULL, last_failure_at = NULL,
                 last_success_at = NULL, updated_at = ?6
             WHERE id = 1",
            params![
                provider_identity,
                binding_config_fingerprint,
                generation,
                i64::from(enabled),
                if enabled { "ready" } else { "disabled" },
                now,
            ],
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "publish Bark provider configuration failed: {error}"
            ))
        })?;
    }
    Ok(generation)
}

impl crate::store::AppStore {
    pub async fn bark_provider_runtime(
        &self,
        config_fingerprint: &str,
    ) -> Result<ProviderRuntime, AppError> {
        use crate::db::{OptionalExtension, params};
        let conn = self.conn.lock().await;
        conn.query_row(
            "SELECT status, consecutive_failures, circuit_open_until, last_error,
                    last_failure_code, last_failure_hint, last_failure_at, last_success_at
             FROM bark_provider_runtime
             WHERE id = 1 AND enabled = 1 AND config_fingerprint = ?1",
            params![config_fingerprint],
            |row| {
                Ok(ProviderRuntime {
                    configuration_active: true,
                    status: row.get(0)?,
                    consecutive_failures: row.get(1)?,
                    circuit_open_until: row.get(2)?,
                    last_error: row.get(3)?,
                    last_failure_code: row.get(4)?,
                    last_failure_hint: row.get(5)?,
                    last_failure_at: row.get(6)?,
                    last_success_at: row.get(7)?,
                })
            },
        )
        .optional()
        .map(Option::unwrap_or_default)
        .map_err(|error| AppError::Internal(format!("read Bark provider runtime failed: {error}")))
    }

    pub async fn bark_circuit_open_until(
        &self,
        config_fingerprint: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>, AppError> {
        use crate::db::{OptionalExtension, params};
        let conn = self.conn.lock().await;
        let open_until = conn
            .query_row(
                "SELECT circuit_open_until FROM bark_provider_runtime
                 WHERE id = 1 AND enabled = 1 AND config_fingerprint = ?1",
                params![config_fingerprint],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("read Bark circuit state failed: {error}"))
            })?
            .flatten()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(&value).ok())
            .map(|value| value.with_timezone(&chrono::Utc))
            .filter(|until| *until > now);
        Ok(open_until)
    }

    pub async fn mark_bark_provider_healthy(
        &self,
        config_fingerprint: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), AppError> {
        use crate::db::params;
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE bark_provider_runtime
             SET config_fingerprint = ?1, status = 'ready', consecutive_failures = 0,
                 circuit_open_until = NULL, last_error = NULL, last_failure_code = NULL,
                 last_failure_hint = NULL, last_failure_at = NULL, last_success_at = ?2,
                 updated_at = ?2
             WHERE id = 1 AND enabled = 1 AND config_fingerprint = ?1",
            params![config_fingerprint, now.to_rfc3339()],
        )
        .map_err(|error| {
            AppError::Internal(format!("mark Bark provider healthy failed: {error}"))
        })?;
        Ok(())
    }

    pub async fn mark_bark_provider_failure(
        &self,
        config_fingerprint: &str,
        failure_code: Option<&str>,
        failure_hint: Option<&str>,
        failure_message: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), AppError> {
        use crate::db::{TransactionBehavior, params};
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!(
                    "begin Bark provider failure update failed: {error}"
                ))
            })?;
        let prior = tx
            .query_row(
                "SELECT CASE WHEN config_fingerprint = ?1 THEN consecutive_failures ELSE 0 END
                 FROM bark_provider_runtime WHERE id = 1",
                params![config_fingerprint],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| {
                AppError::Internal(format!("read Bark provider failures failed: {error}"))
            })?;
        let failures = prior.saturating_add(1);
        let circuit_open_until = (failures >= PROVIDER_CIRCUIT_FAILURE_THRESHOLD)
            .then(|| now + chrono::Duration::seconds(PROVIDER_CIRCUIT_OPEN_SECS))
            .map(|value| value.to_rfc3339());
        tx.execute(
            "UPDATE bark_provider_runtime
             SET config_fingerprint = ?1, status = 'degraded', consecutive_failures = ?2,
                 circuit_open_until = ?3, last_error = ?4, last_failure_code = ?5,
                 last_failure_hint = ?6, last_failure_at = ?7, updated_at = ?7
             WHERE id = 1 AND enabled = 1 AND config_fingerprint = ?1",
            params![
                config_fingerprint,
                failures,
                circuit_open_until,
                sanitize_error(failure_message),
                failure_code,
                failure_hint,
                now.to_rfc3339()
            ],
        )
        .map_err(|error| {
            AppError::Internal(format!("persist Bark provider failure failed: {error}"))
        })?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit Bark provider failure failed: {error}"))
        })?;
        Ok(())
    }

    pub async fn begin_bark_binding_attempt(
        &self,
        email: &str,
        source_ip: Option<&str>,
        provider_identity: &str,
        binding_config_fingerprint: &str,
        target_fingerprint: &str,
    ) -> Result<(String, String, i64), AppError> {
        use crate::db::{OptionalExtension, TransactionBehavior, params};

        let email = crate::telegram::bind::normalize_email(email)?;
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin Bark binding attempt failed: {error}"))
            })?;
        let generation = tx
            .query_row(
                "SELECT generation FROM bark_provider_runtime
                 WHERE id = 1 AND enabled = 1 AND config_fingerprint = ?1
                   AND binding_config_fingerprint = ?2",
                params![provider_identity, binding_config_fingerprint],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!(
                    "read Bark binding configuration fence failed: {error}"
                ))
            })?
            .ok_or_else(|| {
                AppError::Conflict(
                    "Bark configuration changed or is awaiting Router restart".into(),
                )
            })?;
        let user_id = crate::telegram::bind::ensure_user_id(&tx, &email)?;
        tx.execute(
            "DELETE FROM bark_binding_attempts WHERE created_at < ?1",
            params![
                (chrono::Utc::now()
                    - chrono::Duration::seconds(
                        crate::notification_channels::CHANNEL_BINDING_AUDIT_RETENTION_SECS,
                    ))
                .to_rfc3339()
            ],
        )
        .map_err(|error| {
            AppError::Internal(format!("prune Bark binding attempts failed: {error}"))
        })?;
        let cutoff = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        let user_attempts = tx
            .query_row(
                "SELECT COUNT(*) FROM bark_binding_attempts WHERE user_id = ?1 AND created_at >= ?2",
                params![user_id, cutoff],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| AppError::Internal(format!("count Bark binding attempts failed: {error}")))?;
        if user_attempts >= MAX_BIND_ATTEMPTS_PER_USER_HOUR {
            return Err(AppError::TooManyRequests(
                "too many Bark binding attempts; try again later".into(),
            ));
        }
        if let Some(source_ip) = source_ip.filter(|value| !value.trim().is_empty()) {
            let ip_attempts = tx
                .query_row(
                    "SELECT COUNT(*) FROM bark_binding_attempts WHERE source_ip = ?1 AND created_at >= ?2",
                    params![source_ip, cutoff],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(|error| AppError::Internal(format!("count Bark IP binding attempts failed: {error}")))?
                .unwrap_or_default();
            if ip_attempts >= MAX_BIND_ATTEMPTS_PER_IP_HOUR {
                return Err(AppError::TooManyRequests(
                    "too many Bark binding attempts from this address; try again later".into(),
                ));
            }
        }
        let attempt_id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO bark_binding_attempts (
                id, user_id, source_ip, provider_identity, target_fingerprint,
                status, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'started', ?6)",
            params![
                attempt_id,
                user_id,
                source_ip,
                provider_identity,
                target_fingerprint,
                chrono::Utc::now().to_rfc3339()
            ],
        )
        .map_err(|error| {
            AppError::Internal(format!("record Bark binding attempt failed: {error}"))
        })?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit Bark binding attempt failed: {error}"))
        })?;
        Ok((attempt_id, user_id, generation))
    }

    pub async fn finish_bark_binding_attempt(
        &self,
        attempt_id: &str,
        success: bool,
        http_status: Option<u16>,
        failure_code: Option<&str>,
    ) -> Result<(), AppError> {
        use crate::db::params;
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE bark_binding_attempts
             SET status = ?2, http_status = ?3, failure_code = ?4
             WHERE id = ?1 AND status = 'started'",
            params![
                attempt_id,
                if success { "success" } else { "failed" },
                http_status.map(i64::from),
                failure_code
            ],
        )
        .map_err(|error| {
            AppError::Internal(format!("finish Bark binding attempt failed: {error}"))
        })?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn commit_bark_binding(
        &self,
        email: &str,
        user_id: &str,
        provider_identity: &str,
        masked_key: &str,
        device_key: &str,
        cipher: &CredentialCipher,
        expected_binding_config_fingerprint: &str,
        expected_generation: i64,
    ) -> Result<(), AppError> {
        use crate::db::{OptionalExtension, TransactionBehavior, params};
        let email = crate::telegram::bind::normalize_email(email)?;
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin Bark binding commit failed: {error}"))
            })?;
        let fence_matches = tx
            .query_row(
                "SELECT 1 FROM bark_provider_runtime
                 WHERE id = 1 AND enabled = 1 AND config_fingerprint = ?1
                   AND binding_config_fingerprint = ?2 AND generation = ?3",
                params![
                    provider_identity,
                    expected_binding_config_fingerprint,
                    expected_generation
                ],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!(
                    "verify Bark binding configuration fence failed: {error}"
                ))
            })?
            .is_some();
        if !fence_matches
            || crate::bark::binding_config_fingerprint(provider_identity, cipher.key_fingerprint())
                != expected_binding_config_fingerprint
        {
            return Err(AppError::Conflict(
                "Bark configuration changed while the device was being verified".into(),
            ));
        }
        let current_revisions = tx
            .query_row(
                "SELECT revision, credential_revision
                 FROM user_notification_channels WHERE user_id = ?1 AND channel = 'bark'",
                params![user_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("read Bark binding revisions failed: {error}"))
            })?
            .unwrap_or((0, 0));
        let revision = current_revisions.0.saturating_add(1).max(1);
        let credential_revision = current_revisions.1.saturating_add(1).max(1);
        let aad = credential_aad(user_id, credential_revision, provider_identity);
        let envelope = cipher.seal(device_key, &aad)?;
        let now = chrono::Utc::now().to_rfc3339();
        crate::telegram::bind::cancel_channel_deliveries(
            &tx,
            &email,
            CHANNEL,
            &now,
            "Bark channel rebound by user",
        )?;
        for deselected in
            crate::telegram::bind::deselect_other_channels(&tx, user_id, CHANNEL, &now)?
        {
            crate::telegram::bind::cancel_channel_deliveries(
                &tx,
                &email,
                &deselected,
                &now,
                "notification delivery moved to Bark on bind",
            )?;
        }
        tx.execute(
            "INSERT INTO user_notification_channels (
                user_id, channel, enabled, state, target, target_label,
                provider_identity, revision, credential_revision,
                credential_key_fingerprint, verified_at, created_at, updated_at
             ) VALUES (?1, 'bark', 1, 'ready', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?8)
             ON CONFLICT(user_id, channel) DO UPDATE SET
                enabled = 1, state = 'ready', target = excluded.target,
                target_label = excluded.target_label,
                provider_identity = excluded.provider_identity,
                revision = excluded.revision,
                credential_revision = excluded.credential_revision,
                credential_key_fingerprint = excluded.credential_key_fingerprint,
                verified_at = excluded.verified_at,
                invalidated_at = NULL, updated_at = excluded.updated_at",
            params![
                user_id,
                envelope,
                masked_key,
                provider_identity,
                revision,
                credential_revision,
                cipher.key_fingerprint(),
                now
            ],
        )
        .map_err(|error| AppError::Internal(format!("save Bark binding failed: {error}")))?;
        tx.commit()
            .map_err(|error| AppError::Internal(format!("commit Bark binding failed: {error}")))?;
        Ok(())
    }

    pub async fn unbind_bark(&self, email: &str) -> Result<(), AppError> {
        use crate::db::{TransactionBehavior, params};
        let email = crate::telegram::bind::normalize_email(email)?;
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| AppError::Internal(format!("begin Bark unbind failed: {error}")))?;
        let user_id = crate::telegram::bind::ensure_user_id(&tx, &email)?;
        let now = chrono::Utc::now().to_rfc3339();
        let was_selected = crate::telegram::bind::channel_is_selected(&tx, &user_id, CHANNEL)?;
        tx.execute(
            "UPDATE user_notification_channels
             SET enabled = 0, state = 'unbound', target = NULL, target_label = NULL,
                 provider_identity = NULL, credential_key_fingerprint = NULL,
                 revision = revision + 1,
                 invalidated_at = ?2, updated_at = ?2
             WHERE user_id = ?1 AND channel = 'bark'",
            params![user_id, now],
        )
        .map_err(|error| AppError::Internal(format!("unbind Bark channel failed: {error}")))?;
        crate::telegram::bind::cancel_channel_deliveries(
            &tx,
            &email,
            CHANNEL,
            &now,
            "Bark channel unbound by user",
        )?;
        if was_selected {
            crate::telegram::bind::enable_email_fallback(&tx, &user_id, &email, &now)?;
        }
        tx.commit()
            .map_err(|error| AppError::Internal(format!("commit Bark unbind failed: {error}")))?;
        Ok(())
    }

    /// Invalidates the exact Bark binding used by an administrator-initiated
    /// channel test. A concurrent rebind changes the encrypted target, so the
    /// stale test result cannot clear the replacement credential.
    pub async fn invalidate_bark_binding_after_test_failure(
        &self,
        user_id: &str,
        provider_identity: &str,
        encrypted_target: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, AppError> {
        use crate::db::{OptionalExtension, TransactionBehavior, params};
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!(
                    "begin invalid Bark test binding fallback failed: {error}"
                ))
            })?;
        let binding = tx
            .query_row(
                "SELECT users.email_normalized, channel.enabled
                 FROM user_notification_channels channel
                 INNER JOIN users ON users.id = channel.user_id
                 WHERE channel.user_id = ?1 AND channel.channel = 'bark'
                   AND channel.provider_identity = ?2 AND channel.target = ?3
                   AND channel.state = 'ready'",
                params![user_id, provider_identity, encrypted_target],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0)),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("query invalid Bark test binding failed: {error}"))
            })?;
        let Some((email, was_selected)) = binding else {
            tx.commit().map_err(|error| {
                AppError::Internal(format!(
                    "commit stale Bark test binding check failed: {error}"
                ))
            })?;
            return Ok(false);
        };
        let now = now.to_rfc3339();
        let changed = tx
            .execute(
                "UPDATE user_notification_channels
                 SET enabled = 0, state = 'invalid', target = NULL, target_label = NULL,
                     provider_identity = NULL, credential_key_fingerprint = NULL,
                     revision = revision + 1,
                     invalidated_at = ?4, updated_at = ?4
                 WHERE user_id = ?1 AND channel = 'bark'
                   AND provider_identity = ?2 AND target = ?3 AND state = 'ready'",
                params![user_id, provider_identity, encrypted_target, now],
            )
            .map_err(|error| {
                AppError::Internal(format!(
                    "invalidate Bark binding after channel test failed: {error}"
                ))
            })?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "Bark binding changed before test failure handling".into(),
            ));
        }
        crate::telegram::bind::cancel_channel_deliveries(
            &tx,
            &email,
            CHANNEL,
            &now,
            "Bark device key became invalid during channel test",
        )?;
        if was_selected {
            crate::telegram::bind::enable_email_fallback(&tx, user_id, &email, &now)?;
        }
        tx.commit().map_err(|error| {
            AppError::Internal(format!(
                "commit invalid Bark test binding fallback failed: {error}"
            ))
        })?;
        Ok(true)
    }

    pub async fn handle_invalid_bark_delivery(
        &self,
        delivery_id: &str,
        worker_id: &str,
        encrypted_target: &str,
        error_message: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<String>, AppError> {
        use crate::db::{OptionalExtension, TransactionBehavior, params};
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| AppError::Internal(format!("begin Bark fallback failed: {error}")))?;
        let delivery = tx
            .query_row(
                "SELECT recipient_user_id, provider_identity, channel_target
                 FROM notification_deliveries
                 WHERE id = ?1 AND channel = 'bark' AND status = 'claimed'
                   AND claim_owner = ?2",
                params![delivery_id, worker_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("query invalid Bark delivery failed: {error}"))
            })?
            .ok_or_else(|| {
                AppError::Conflict("Bark delivery claim is no longer owned by this worker".into())
            })?;
        if delivery.2.as_deref() != Some(encrypted_target) {
            return Err(AppError::Conflict(
                "Bark delivery target changed before failure handling".into(),
            ));
        }
        let binding = match (delivery.0.as_deref(), delivery.1.as_deref()) {
            (Some(user_id), Some(provider_identity)) => tx
                .query_row(
                    "SELECT channel.user_id, users.email_normalized, channel.enabled
                     FROM user_notification_channels channel
                     INNER JOIN users ON users.id = channel.user_id
                     WHERE channel.user_id = ?1 AND channel.channel = 'bark'
                       AND channel.provider_identity = ?2 AND channel.target = ?3
                       AND channel.state = 'ready'",
                    params![user_id, provider_identity, encrypted_target],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)? != 0,
                        ))
                    },
                )
                .optional()
                .map_err(|error| {
                    AppError::Internal(format!("query invalid Bark binding failed: {error}"))
                })?,
            _ => None,
        };
        let now_text = now.to_rfc3339();
        let changed = tx
            .execute(
                "UPDATE notification_deliveries
             SET status = 'dead_letter', failure_kind = 'endpoint_unreachable',
                 error_message = ?3, blocked_reason_code = NULL, next_attempt_at = NULL,
                 claim_owner = NULL, claim_expires_at = NULL, updated_at = ?4
             WHERE id = ?1 AND status = 'claimed' AND claim_owner = ?2",
                params![delivery_id, worker_id, error_message, now_text],
            )
            .map_err(|error| {
                AppError::Internal(format!("finish invalid Bark delivery failed: {error}"))
            })?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "Bark delivery claim is no longer owned by this worker".into(),
            ));
        }
        tx.execute(
            "UPDATE notification_delivery_attempts
             SET status = 'failed', finished_at = ?2, error_message = ?3
             WHERE delivery_id = ?1 AND status = 'started'",
            params![delivery_id, now_text, error_message],
        )
        .map_err(|error| {
            AppError::Internal(format!("finish invalid Bark attempt failed: {error}"))
        })?;
        crate::telegram::bind::release_delivery_events_for_retry(&tx, delivery_id, &now_text)?;
        if let Some((user_id, email, was_selected)) = binding.as_ref() {
            tx.execute(
                "UPDATE user_notification_channels
                 SET enabled = 0, state = 'invalid', target = NULL, target_label = NULL,
                     provider_identity = NULL, credential_key_fingerprint = NULL,
                     revision = revision + 1,
                     invalidated_at = ?2, updated_at = ?2
                 WHERE user_id = ?1 AND channel = 'bark'",
                params![user_id, now_text],
            )
            .map_err(|error| {
                AppError::Internal(format!("invalidate Bark binding failed: {error}"))
            })?;
            crate::telegram::bind::cancel_channel_deliveries(
                &tx,
                email,
                CHANNEL,
                &now_text,
                "Bark device key became invalid",
            )?;
            if *was_selected {
                crate::telegram::bind::enable_email_fallback(&tx, user_id, email, &now_text)?;
            }
        }
        tx.commit()
            .map_err(|error| AppError::Internal(format!("commit Bark fallback failed: {error}")))?;
        Ok(binding.map(|(_, email, _)| email))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::body::{Body, Bytes};
    use axum::http::header::LOCATION;
    use axum::http::{HeaderValue, Response, StatusCode};
    use axum::routing::post;

    use super::*;

    fn settings() -> BarkSettings {
        BarkSettings {
            enabled: true,
            credential_master_key: Some("11".repeat(32)),
            ..BarkSettings::default()
        }
    }

    #[test]
    fn push_url_must_match_configured_server() {
        let parsed = parse_push_url("https://api.day.app/abcDEF_123", "https://api.day.app")
            .expect("valid push URL");
        assert_eq!(&*parsed.device_key, "abcDEF_123");
        assert_eq!(parsed.masked_key, "••••_123");
        assert!(parse_push_url("https://example.com/abcDEF_123", "https://api.day.app").is_err());
        assert!(
            parse_push_url("https://api.day.app/abcDEF_123/body", "https://api.day.app").is_err()
        );
        assert!(
            parse_push_url(
                "https://api.day.app/abcDEF_123?secret=x",
                "https://api.day.app"
            )
            .is_err()
        );
        assert!(
            parse_push_url("https://api.day.app.evil/abcDEF_123", "https://api.day.app").is_err()
        );
        assert!(
            parse_push_url("https://api.day.app/%20abcDEF_123", "https://api.day.app").is_err()
        );
        assert!(
            parse_push_url(
                "https://api.day.app/abcDEF_123\n/ignored",
                "https://api.day.app"
            )
            .is_err()
        );
        assert!(
            parse_push_url(&"x".repeat(MAX_PUSH_URL_BYTES + 1), "https://api.day.app").is_err()
        );
    }

    #[test]
    fn server_url_rejects_remote_plaintext_and_credentials() {
        assert_eq!(
            normalize_server_url("https://api.day.app/").unwrap(),
            "https://api.day.app"
        );
        assert!(normalize_server_url("http://api.day.app").is_err());
        assert!(normalize_server_url("http://127.0.0.1:8080").is_ok());
        assert!(normalize_server_url("http://127.0.0.2:8080").is_ok());
        assert_eq!(
            normalize_server_url("http://[::1]:8080/").unwrap(),
            "http://[::1]:8080"
        );
        assert!(normalize_server_url("https://user:pass@example.com").is_err());
        assert!(normalize_server_url("https://api.day.app/\nignored").is_err());
        assert!(normalize_server_url(&"x".repeat(MAX_SERVER_URL_BYTES + 1)).is_err());
    }

    #[test]
    fn credential_envelope_is_authenticated_and_versioned() {
        let cipher = CredentialCipher::from_settings(&settings())
            .unwrap()
            .unwrap();
        let aad = credential_aad("user", 3, "provider");
        let envelope = cipher.seal("device-secret", &aad).unwrap();
        assert!(!envelope.contains("device-secret"));
        assert_eq!(&*cipher.open(&envelope, &aad).unwrap(), "device-secret");
        assert!(cipher.open(&envelope, "wrong-aad").is_err());

        let mut rotated = settings();
        rotated.credential_key_version = 2;
        let rotated = CredentialCipher::from_settings(&rotated).unwrap().unwrap();
        assert!(rotated.open(&envelope, &aad).is_err());
    }

    #[test]
    fn retry_after_clamps_zero_seconds_and_past_dates() {
        let before = chrono::Utc::now().timestamp();
        let zero_seconds = HeaderValue::from_static("0");
        assert!(parse_retry_after(&zero_seconds).expect("parse zero seconds") > before);

        let oversized = HeaderValue::from_static("31536000");
        assert!(parse_retry_after(&oversized).expect("parse oversized delay") <= before + 86_401);

        let past_date = HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT");
        assert!(parse_retry_after(&past_date).expect("parse past HTTP date") > before);
    }

    async fn mock_bark(
        status: StatusCode,
        response_body: &'static str,
        retry_after: Option<&'static str>,
    ) -> (
        String,
        Arc<Mutex<Vec<serde_json::Value>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let app = Router::new().route(
            "/push",
            post(move |axum::Json(payload): axum::Json<serde_json::Value>| {
                let captured = captured.clone();
                async move {
                    captured.lock().expect("capture Bark request").push(payload);
                    let mut response = Response::new(Body::from(response_body));
                    *response.status_mut() = status;
                    response.headers_mut().insert(
                        axum::http::header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    );
                    if let Some(value) = retry_after {
                        response.headers_mut().insert(
                            axum::http::header::RETRY_AFTER,
                            HeaderValue::from_static(value),
                        );
                    }
                    response
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind Bark mock");
        let address = listener.local_addr().expect("Bark mock address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve Bark mock");
        });
        (format!("http://{address}"), requests, server)
    }

    fn push_request<'a>(device_key: &'a str) -> PushRequest<'a> {
        PushRequest {
            device_key,
            title: "title",
            body: "body",
            group: "group",
            url: "https://router.example.com/account/notifications",
            id: "stable-id",
            level: "active",
        }
    }

    #[tokio::test]
    async fn bark_send_classifies_target_provider_and_retry_failures() {
        let client = build_http_client("bark-test").expect("Bark HTTP client");

        let (server_url, requests, server) = mock_bark(
            StatusCode::OK,
            r#"{"code":200,"data":{"id":"message-device_key_123"}}"#,
            None,
        )
        .await;
        let success = send(&client, &server_url, &push_request("device_key_123"))
            .await
            .expect("successful Bark push");
        assert_eq!(
            success.provider_message_id.as_deref(),
            Some("message-[REDACTED]")
        );
        let payload = requests.lock().expect("read Bark requests")[0].clone();
        assert_eq!(payload["device_key"], "device_key_123");
        assert_eq!(payload["id"], "stable-id");
        server.abort();

        let (server_url, _, server) = mock_bark(
            StatusCode::BAD_REQUEST,
            r#"{"code":400,"message":"failed to get device token: failed to get [device_key_123] device token from database"}"#,
            None,
        )
        .await;
        let failure = send(&client, &server_url, &push_request("device_key_123"))
            .await
            .expect_err("400 must fail");
        assert!(failure.target_invalid);
        assert!(!failure.retryable);
        assert_eq!(failure.code, "target_invalid");
        assert!(!failure.message.contains("device_key_123"));
        server.abort();

        let (server_url, _, server) = mock_bark(
            StatusCode::BAD_REQUEST,
            r#"{"code":400,"message":"request bind failed: malformed JSON"}"#,
            None,
        )
        .await;
        let failure = send(&client, &server_url, &push_request("device_key_123"))
            .await
            .expect_err("generic 400 must fail");
        assert!(
            !failure.target_invalid,
            "a generic request error must not delete the binding"
        );
        assert_eq!(failure.code, "provider_protocol");
        server.abort();

        let (server_url, _, server) = mock_bark(
            StatusCode::NOT_FOUND,
            r#"{"code":404,"message":"Cannot POST /push"}"#,
            None,
        )
        .await;
        let failure = send(&client, &server_url, &push_request("device_key_123"))
            .await
            .expect_err("404 must fail");
        assert!(
            !failure.target_invalid,
            "a missing Provider route must not delete the binding"
        );
        assert_eq!(failure.code, "provider_protocol");
        server.abort();

        let (server_url, _, server) = mock_bark(
            StatusCode::FORBIDDEN,
            r#"{"code":403,"message":"forbidden"}"#,
            None,
        )
        .await;
        let failure = send(&client, &server_url, &push_request("device_key_123"))
            .await
            .expect_err("403 must fail");
        assert!(
            !failure.target_invalid,
            "provider authorization is not a bad device key"
        );
        assert_eq!(failure.code, "authorization");
        server.abort();

        let (server_url, _, server) = mock_bark(
            StatusCode::OK,
            r#"{"code":403,"message":"device_key_123 forbidden"}"#,
            None,
        )
        .await;
        let failure = send(&client, &server_url, &push_request("device_key_123"))
            .await
            .expect_err("application-level 403 must fail");
        assert!(!failure.target_invalid);
        assert_eq!(failure.code, "authorization");
        assert!(!failure.message.contains("device_key_123"));
        server.abort();

        let (server_url, _, server) =
            mock_bark(StatusCode::TOO_MANY_REQUESTS, r#"{"code":429}"#, Some("30")).await;
        let failure = send(&client, &server_url, &push_request("device_key_123"))
            .await
            .expect_err("429 must fail");
        assert!(failure.retryable);
        assert!(failure.retry_at.is_some());
        assert!(!failure.target_invalid);
        server.abort();
    }

    #[test]
    fn bark_target_invalidation_requires_a_known_device_lookup_error() {
        for message in [
            "failed to get device token: key not found",
            "failed to get device token: device token invalid",
            "failed to get device token: sql: no rows in result set",
            "failed to get device token: failed to get [secret] device token from database",
        ] {
            assert!(
                bark_target_invalid(400, message, "secret"),
                "known Bark lookup failure was not recognized: {message}"
            );
        }
        for (status, message) in [
            (400, "request bind failed: malformed JSON"),
            (400, "failed to get device token: database is locked"),
            (
                400,
                "failed to get device token: failed to get [another-key] device token from database",
            ),
            (
                400,
                "failed to get device token: failed to update [secret] device token from database",
            ),
            (404, "failed to get device token: key not found"),
            (404, "Cannot POST /push"),
        ] {
            assert!(
                !bark_target_invalid(status, message, "secret"),
                "ambiguous Provider failure was treated as an invalid target: {status} {message}"
            );
        }
    }

    #[test]
    fn provider_errors_redact_device_keys_case_insensitively_and_preserve_utf8() {
        let sanitized = sanitize_provider_error(
            "设备 DEVICE_KEY_123 / device_Key_123 / ok",
            "device_key_123",
        );
        assert_eq!(sanitized, "设备 [REDACTED] / [REDACTED] / ok");
        assert!(!sanitized.to_ascii_lowercase().contains("device_key_123"));
    }

    #[tokio::test]
    async fn bark_client_never_replays_device_key_across_redirects() {
        let client = build_http_client("bark-redirect-test").expect("Bark HTTP client");
        for status in [
            StatusCode::TEMPORARY_REDIRECT,
            StatusCode::PERMANENT_REDIRECT,
        ] {
            let leaked_requests = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
            let leaked_capture = leaked_requests.clone();
            let target_app = Router::new().route(
                "/leak",
                post(move |body: Bytes| {
                    let leaked_capture = leaked_capture.clone();
                    async move {
                        leaked_capture
                            .lock()
                            .expect("capture redirect target request")
                            .push(body.to_vec());
                        StatusCode::OK
                    }
                }),
            );
            let target_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind redirect target");
            let target_address = target_listener
                .local_addr()
                .expect("redirect target address");
            let target_server = tokio::spawn(async move {
                axum::serve(target_listener, target_app)
                    .await
                    .expect("serve redirect target");
            });

            let location = HeaderValue::from_str(&format!("http://{target_address}/leak"))
                .expect("redirect location");
            let redirect_app = Router::new().route(
                "/push",
                post(move || {
                    let location = location.clone();
                    async move {
                        let mut response = Response::new(Body::empty());
                        *response.status_mut() = status;
                        response.headers_mut().insert(LOCATION, location);
                        response
                    }
                }),
            );
            let redirect_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind redirecting Bark mock");
            let redirect_address = redirect_listener
                .local_addr()
                .expect("redirecting Bark address");
            let redirect_server = tokio::spawn(async move {
                axum::serve(redirect_listener, redirect_app)
                    .await
                    .expect("serve redirecting Bark mock");
            });

            let failure = send(
                &client,
                &format!("http://{redirect_address}"),
                &push_request("device_key_redirect_secret"),
            )
            .await
            .expect_err("redirect response must not be followed");
            assert_eq!(failure.http_status, Some(status.as_u16()));
            assert!(
                leaked_requests
                    .lock()
                    .expect("read redirect target requests")
                    .is_empty(),
                "Bark request body was replayed for HTTP {}",
                status.as_u16()
            );

            redirect_server.abort();
            target_server.abort();
        }
    }
}
