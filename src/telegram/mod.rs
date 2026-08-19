//! Low-level Telegram Bot API client shared by two independent consumers:
//!
//! * `crate::alerting` — send-only operator alerts pinned to one chat.
//! * `crate::telegram::bind` / `crate::notifications` — the user-facing
//!   notification bot, which additionally *receives* `/start <token>` deep
//!   links to bind a Router account to a private chat.
//!
//! Everything here is transport: no policy, no persistence. Errors are shaped
//! as [`TelegramFailure`] so both consumers can reuse the same retry
//! classification (`retryable` + optional `retry_at` honoring Telegram's own
//! `parameters.retry_after`).

pub mod bind;
pub mod service;

use std::borrow::Cow;
use std::collections::HashSet;
use std::error::Error as StdError;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use chrono::Utc;
use futures_util::future::join_all;
use reqwest::header::RETRY_AFTER;
use serde::Serialize;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::Mutex;

/// Telegram rejects `sendMessage` payloads above 4096 UTF-16 code units. We cut
/// a little earlier so an appended ellipsis still fits.
pub const MAX_MESSAGE_CHARS: usize = 4_000;

/// Telegram's `start` deep-link parameter accepts 1..=64 chars from
/// `A-Za-z0-9_-`. Our tokens are 16 random bytes rendered as 32 hex chars.
pub const BIND_TOKEN_BYTES: usize = 16;

const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(300);
const TELEGRAM_LOCAL_ADDRESS: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
const TELEGRAM_API_HOST: &str = "api.telegram.org";
const NETWORK_DIAGNOSTIC_CACHE_TTL: Duration = Duration::from_secs(30);
const NETWORK_DIAGNOSTIC_DNS_TIMEOUT: Duration = Duration::from_secs(2);
const NETWORK_DIAGNOSTIC_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
const NETWORK_DIAGNOSTIC_MAX_ADDRESSES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelegramFailureCode {
    Configuration,
    DnsResolutionFailed,
    ApiEndpointUnreachable,
    ApiTimeout,
    TlsHandshakeFailed,
    InvalidToken,
    PollingConflict,
    RateLimited,
    ChatUnreachable,
    HttpError,
    RequestError,
    ResponseReadFailed,
    TransportError,
}

impl TelegramFailureCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Configuration => "configuration",
            Self::DnsResolutionFailed => "dns_resolution_failed",
            Self::ApiEndpointUnreachable => "api_endpoint_unreachable",
            Self::ApiTimeout => "api_timeout",
            Self::TlsHandshakeFailed => "tls_handshake_failed",
            Self::InvalidToken => "invalid_token",
            Self::PollingConflict => "polling_conflict",
            Self::RateLimited => "rate_limited",
            Self::ChatUnreachable => "chat_unreachable",
            Self::HttpError => "http_error",
            Self::RequestError => "request_error",
            Self::ResponseReadFailed => "response_read_failed",
            Self::TransportError => "transport_error",
        }
    }

    pub const fn default_hint(self) -> &'static str {
        match self {
            Self::Configuration => "Check the Telegram Bot configuration and save it again.",
            Self::DnsResolutionFailed => {
                "Router could not resolve api.telegram.org. Check the host DNS configuration and try another resolver such as 8.8.8.8 or 1.1.1.1."
            }
            Self::ApiEndpointUnreachable => {
                "Router resolved Telegram API addresses, but none accepted a TCP connection on port 443. Check DNS results and outbound TCP 443; try another resolver such as 8.8.8.8 or 1.1.1.1."
            }
            Self::ApiTimeout => {
                "The Telegram API connection timed out. Check DNS results and outbound TCP 443 from the Router host."
            }
            Self::TlsHandshakeFailed => {
                "TCP connected, but the Telegram API TLS handshake failed. Check the system clock, CA certificates, and TLS interception."
            }
            Self::InvalidToken => "The Telegram Bot Token is invalid or revoked.",
            Self::PollingConflict => {
                "Another process or webhook is consuming this Telegram Bot. Stop the other consumer or switch the Bot mode."
            }
            Self::RateLimited => {
                "Telegram rate limited this request. Wait until the retry time before trying again."
            }
            Self::ChatUnreachable => {
                "Telegram cannot deliver to this chat. The user may have blocked the Bot or the chat no longer exists."
            }
            Self::HttpError => {
                "Telegram returned an unexpected HTTP error. Check the technical details and retry."
            }
            Self::RequestError => {
                "The Telegram request could not be constructed. Check the Router configuration."
            }
            Self::ResponseReadFailed => {
                "Telegram returned a response that Router could not read. Retry and inspect the network."
            }
            Self::TransportError => {
                "The Telegram network transport failed. Check the Router host network and retry."
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramNetworkDiagnostics {
    pub host: String,
    pub resolved_addresses: Vec<String>,
    pub reachable_addresses: Vec<String>,
    pub dns_error: Option<String>,
}

#[derive(Debug, Clone)]
struct NetworkDiagnosticCache {
    captured_at: Instant,
    value: TelegramNetworkDiagnostics,
}

fn network_diagnostic_cache() -> &'static Mutex<Option<NetworkDiagnosticCache>> {
    static CACHE: OnceLock<Mutex<Option<NetworkDiagnosticCache>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

#[derive(Debug, Clone)]
pub struct TelegramSuccess {
    pub provider_message_id: Option<String>,
    pub http_status: u16,
}

#[derive(Debug, Clone)]
pub struct TelegramFailure {
    pub code: TelegramFailureCode,
    pub retryable: bool,
    pub retry_at: Option<i64>,
    pub http_status: Option<u16>,
    pub message: String,
    pub hint: String,
    pub diagnostics: Option<TelegramNetworkDiagnostics>,
    /// Telegram told us this chat can never be delivered to again: the user
    /// blocked the bot, deleted the chat, or the id is bogus. Callers unbind
    /// instead of retrying.
    pub chat_unreachable: bool,
    /// Telegram refused the markup, not the message. Retrying the same body is
    /// pointless; resending it as plain text is not.
    pub parse_error: bool,
}

impl TelegramFailure {
    pub fn config(message: impl Into<String>) -> Self {
        Self::new(
            TelegramFailureCode::Configuration,
            false,
            None,
            None,
            message.into(),
            None,
            false,
        )
    }

    fn new(
        code: TelegramFailureCode,
        retryable: bool,
        retry_at: Option<i64>,
        http_status: Option<u16>,
        message: String,
        diagnostics: Option<TelegramNetworkDiagnostics>,
        chat_unreachable: bool,
    ) -> Self {
        Self {
            code,
            retryable,
            retry_at,
            http_status,
            message,
            hint: code.default_hint().into(),
            diagnostics,
            chat_unreachable,
            parse_error: false,
        }
    }
}

/// Result of `getMe`, used to prove a bot token works and to learn the
/// `@username` needed for `https://t.me/<username>?start=<token>` deep links.
#[derive(Debug, Clone)]
pub struct BotIdentity {
    pub id: i64,
    pub username: String,
    pub can_read_all_group_messages: bool,
}

pub fn build_update_http_client(user_agent: &str) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent(user_agent.to_string())
        // Telegram Bot API has a stable IPv4 endpoint. Binding an IPv4 local
        // address avoids an unusable IPv6 route delaying direct requests.
        .local_address(TELEGRAM_LOCAL_ADDRESS)
        .connect_timeout(Duration::from_secs(10))
        // Long polling holds the response open for `timeout` seconds; the read
        // timeout has to outlive it or every poll aborts mid-flight.
        .timeout(Duration::from_secs(60))
        .build()
}

pub fn build_send_http_client(user_agent: &str) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent(user_agent.to_string())
        .local_address(TELEGRAM_LOCAL_ADDRESS)
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .build()
}

fn endpoint(token: &str, method: &str) -> String {
    format!("https://api.telegram.org/bot{token}/{method}")
}

/// POST a Bot API method and unwrap the `{ok, result}` envelope.
async fn call(
    http: &reqwest::Client,
    token: &str,
    method: &str,
    payload: &Value,
) -> Result<(Value, u16), TelegramFailure> {
    let url = endpoint(token, method);
    let send = || http.post(&url).json(payload).send();
    let response = match send().await {
        // `sendMessage` is not idempotent: a connection error can happen
        // after Telegram accepted the message but before Router received the
        // response. Retrying it would risk duplicate user/operator alerts.
        // Setup and polling methods are safe to retry because repeating them
        // with the same payload/offset is idempotent.
        Err(error) if error.is_connect() && retryable_method(method) => {
            tracing::debug!(method, "retrying Telegram request after connection failure");
            tokio::time::sleep(CONNECT_RETRY_DELAY).await;
            send().await
        }
        result => result,
    };
    let response = match response {
        Ok(response) => response,
        Err(error) => return Err(request_failure(method, error, Some(token)).await),
    };
    // A successful TCP/HTTP response proves the cached failure probe is stale;
    // let the next failure collect fresh DNS and reachability data.
    network_diagnostic_cache().lock().await.take();
    let status = response.status();
    let header_retry_at = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after);
    let body = match response.text().await {
        Ok(body) => body,
        Err(error) => {
            let diagnostics = if error.is_connect() || error.is_timeout() {
                Some(network_diagnostics().await)
            } else {
                None
            };
            let code = diagnostics
                .as_ref()
                .map_or(TelegramFailureCode::ResponseReadFailed, |value| {
                    classify_transport_failure(&error, Some(value))
                });
            return Err(TelegramFailure::new(
                code,
                error.is_connect() || error.is_timeout() || error.is_request(),
                header_retry_at,
                Some(status.as_u16()),
                sanitize(
                    &format!(
                        "read Telegram {method} response failed after HTTP {}: {error}",
                        status.as_u16()
                    ),
                    Some(token),
                ),
                diagnostics,
                false,
            ));
        }
    };
    let value = serde_json::from_str::<Value>(&body).unwrap_or_else(|_| serde_json::json!({}));
    if status.is_success() && value.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok((
            value.get("result").cloned().unwrap_or(Value::Null),
            status.as_u16(),
        ));
    }
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or(&body);
    let telegram_retry_at = value
        .pointer("/parameters/retry_after")
        .and_then(Value::as_i64)
        .map(|seconds| {
            Utc::now()
                .timestamp()
                .saturating_add(seconds.clamp(1, 86_400))
        });
    let chat_unreachable = is_chat_unreachable(status.as_u16(), description);
    let code = if status == 401 {
        TelegramFailureCode::InvalidToken
    } else if status == 409 && method == "getUpdates" {
        TelegramFailureCode::PollingConflict
    } else if status == 429 {
        TelegramFailureCode::RateLimited
    } else if chat_unreachable {
        TelegramFailureCode::ChatUnreachable
    } else {
        TelegramFailureCode::HttpError
    };
    let mut failure = TelegramFailure::new(
        code,
        is_retryable_status(status.as_u16()),
        telegram_retry_at.or(header_retry_at),
        Some(status.as_u16()),
        sanitize(
            &format!(
                "Telegram {method} returned HTTP {}: {}",
                status.as_u16(),
                truncate(description, 1_000)
            ),
            Some(token),
        ),
        None,
        chat_unreachable,
    );
    failure.parse_error = is_parse_error(status.as_u16(), description);
    Err(failure)
}

pub async fn send_message(
    http: &reqwest::Client,
    token: &str,
    chat_id: &str,
    topic_id: Option<i64>,
    text: &str,
    parse_mode: TelegramParseMode,
) -> Result<TelegramSuccess, TelegramFailure> {
    let token = require(Some(token), "Telegram bot token is not configured")?;
    let chat_id = require(Some(chat_id), "Telegram chat id is not configured")?;
    let body = match parse_mode {
        TelegramParseMode::Html => Cow::Owned(truncate_html(text, MAX_MESSAGE_CHARS)),
        TelegramParseMode::Plain => Cow::Borrowed(truncate(text, MAX_MESSAGE_CHARS)),
    };
    let mut payload = serde_json::json!({
        "chat_id": chat_id,
        "text": body,
        "disable_web_page_preview": true,
    });
    if let Some(mode) = parse_mode.as_str() {
        payload["parse_mode"] = serde_json::json!(mode);
    }
    if let Some(topic_id) = topic_id {
        payload["message_thread_id"] = serde_json::json!(topic_id);
    }
    let (result, http_status) = call(http, token, "sendMessage", &payload).await?;
    Ok(TelegramSuccess {
        provider_message_id: result.get("message_id").and_then(|value| match value {
            Value::Number(number) => Some(number.to_string()),
            Value::String(value) => Some(value.clone()),
            _ => None,
        }),
        http_status,
    })
}

pub async fn get_me(http: &reqwest::Client, token: &str) -> Result<BotIdentity, TelegramFailure> {
    let token = require(Some(token), "Telegram bot token is not configured")?;
    let (result, _) = call(http, token, "getMe", &serde_json::json!({})).await?;
    let username = result
        .get("username")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .trim_start_matches('@')
        .to_string();
    if username.is_empty() {
        return Err(TelegramFailure::config(
            "Telegram getMe returned a bot without a username",
        ));
    }
    Ok(BotIdentity {
        id: result.get("id").and_then(Value::as_i64).unwrap_or_default(),
        username,
        can_read_all_group_messages: result
            .get("can_read_all_group_messages")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// Register the commands users see in the Telegram UI's `/` menu.
pub async fn set_my_commands(
    http: &reqwest::Client,
    token: &str,
    commands: &[(&str, &str)],
) -> Result<(), TelegramFailure> {
    let token = require(Some(token), "Telegram bot token is not configured")?;
    let payload = serde_json::json!({
        "commands": commands
            .iter()
            .map(|(command, description)| serde_json::json!({
                "command": command,
                "description": description,
            }))
            .collect::<Vec<_>>(),
        "scope": { "type": "all_private_chats" },
    });
    call(http, token, "setMyCommands", &payload).await?;
    Ok(())
}

pub async fn set_webhook(
    http: &reqwest::Client,
    token: &str,
    url: &str,
    secret: &str,
) -> Result<(), TelegramFailure> {
    let token = require(Some(token), "Telegram bot token is not configured")?;
    let payload = serde_json::json!({
        "url": url,
        "secret_token": secret,
        "allowed_updates": ["message"],
        "drop_pending_updates": false,
        "max_connections": 20,
    });
    call(http, token, "setWebhook", &payload).await?;
    Ok(())
}

pub async fn delete_webhook(http: &reqwest::Client, token: &str) -> Result<(), TelegramFailure> {
    let token = require(Some(token), "Telegram bot token is not configured")?;
    call(
        http,
        token,
        "deleteWebhook",
        &serde_json::json!({ "drop_pending_updates": false }),
    )
    .await?;
    Ok(())
}

/// Long-poll for updates. Returns raw update objects; the caller advances
/// `offset` past the highest `update_id` it has processed.
pub async fn get_updates(
    http: &reqwest::Client,
    token: &str,
    offset: i64,
    timeout_secs: u16,
) -> Result<Vec<Value>, TelegramFailure> {
    let token = require(Some(token), "Telegram bot token is not configured")?;
    let payload = serde_json::json!({
        "offset": offset,
        "timeout": timeout_secs,
        "limit": 50,
        "allowed_updates": ["message"],
    });
    let (result, _) = call(http, token, "getUpdates", &payload).await?;
    Ok(match result {
        Value::Array(updates) => updates,
        _ => Vec::new(),
    })
}

fn require<'a>(value: Option<&'a str>, message: &str) -> Result<&'a str, TelegramFailure> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| TelegramFailure::config(message))
}

fn retryable_method(method: &str) -> bool {
    matches!(
        method,
        "getMe" | "setMyCommands" | "setWebhook" | "deleteWebhook" | "getUpdates"
    )
}

async fn request_failure(
    method: &str,
    error: reqwest::Error,
    secret: Option<&str>,
) -> TelegramFailure {
    let kind = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_request() {
        "request"
    } else {
        "transport"
    };
    let diagnostics = if error.is_connect() || error.is_timeout() {
        Some(network_diagnostics().await)
    } else {
        None
    };
    let code = classify_transport_failure(&error, diagnostics.as_ref());
    TelegramFailure::new(
        code,
        error.is_timeout() || error.is_connect() || error.is_request(),
        None,
        error.status().map(|status| status.as_u16()),
        sanitize(
            &format!(
                "Telegram {method} request failed ({kind}): {}",
                error_chain(&error)
            ),
            secret,
        ),
        diagnostics,
        false,
    )
}

fn classify_transport_failure(
    error: &reqwest::Error,
    diagnostics: Option<&TelegramNetworkDiagnostics>,
) -> TelegramFailureCode {
    // A TLS error can also be reported by reqwest as a connect failure. Check
    // the error chain before the lightweight TCP probe so a reachable endpoint
    // is not mislabelled as merely unreachable when the handshake is what
    // failed.
    if looks_like_tls_error(error) {
        return TelegramFailureCode::TlsHandshakeFailed;
    }
    if diagnostics.is_some_and(|value| value.dns_error.is_some()) {
        return TelegramFailureCode::DnsResolutionFailed;
    }
    if diagnostics.is_some_and(|value| value.resolved_addresses.is_empty()) {
        return TelegramFailureCode::DnsResolutionFailed;
    }
    if diagnostics.is_some_and(|value| value.reachable_addresses.is_empty()) {
        return TelegramFailureCode::ApiEndpointUnreachable;
    }
    if error.is_timeout() {
        return TelegramFailureCode::ApiTimeout;
    }
    if error.is_request() {
        return TelegramFailureCode::RequestError;
    }
    if error.is_connect() {
        return TelegramFailureCode::TransportError;
    }
    TelegramFailureCode::TransportError
}

fn looks_like_tls_error(error: &reqwest::Error) -> bool {
    let text = error_chain(error).to_ascii_lowercase();
    [
        "tls",
        "rustls",
        "certificate",
        "handshake",
        "unknown ca",
        "invalid peer certificate",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

async fn network_diagnostics() -> TelegramNetworkDiagnostics {
    {
        let cache = network_diagnostic_cache().lock().await;
        if let Some(cached) = cache.as_ref()
            && cached.captured_at.elapsed() < NETWORK_DIAGNOSTIC_CACHE_TTL
        {
            return cached.value.clone();
        }
    }

    let value = collect_network_diagnostics().await;
    let mut cache = network_diagnostic_cache().lock().await;
    // A second failure may have completed a newer probe while this one was
    // resolving. Prefer that result rather than replacing it with stale data.
    if let Some(cached) = cache.as_ref()
        && cached.captured_at.elapsed() < NETWORK_DIAGNOSTIC_CACHE_TTL
    {
        return cached.value.clone();
    }
    *cache = Some(NetworkDiagnosticCache {
        captured_at: Instant::now(),
        value: value.clone(),
    });
    value
}

async fn collect_network_diagnostics() -> TelegramNetworkDiagnostics {
    let lookup = tokio::time::timeout(
        NETWORK_DIAGNOSTIC_DNS_TIMEOUT,
        tokio::net::lookup_host((TELEGRAM_API_HOST, 443)),
    )
    .await;
    let addresses = match lookup {
        Ok(Ok(addresses)) => {
            let mut seen = HashSet::new();
            addresses
                // The Telegram clients below intentionally bind an IPv4
                // local address, so probe the same address family that the
                // real request can use. This avoids reporting a reachable
                // IPv6 record as a false recovery when IPv4 is blocked.
                .filter(|address| address.is_ipv4())
                .filter(|address| seen.insert(*address))
                .take(NETWORK_DIAGNOSTIC_MAX_ADDRESSES)
                .collect::<Vec<_>>()
        }
        Ok(Err(error)) => {
            return TelegramNetworkDiagnostics {
                host: TELEGRAM_API_HOST.into(),
                resolved_addresses: Vec::new(),
                reachable_addresses: Vec::new(),
                dns_error: Some(sanitize(&error.to_string(), None)),
            };
        }
        Err(_) => {
            return TelegramNetworkDiagnostics {
                host: TELEGRAM_API_HOST.into(),
                resolved_addresses: Vec::new(),
                reachable_addresses: Vec::new(),
                dns_error: Some("DNS lookup timed out".into()),
            };
        }
    };
    let reachable = join_all(addresses.iter().copied().map(probe_tcp_address))
        .await
        .into_iter()
        .zip(addresses.iter().copied())
        .filter_map(|(reachable, address)| reachable.then(|| address.to_string()))
        .collect::<Vec<_>>();
    TelegramNetworkDiagnostics {
        host: TELEGRAM_API_HOST.into(),
        resolved_addresses: addresses
            .into_iter()
            .map(|address| address.to_string())
            .collect(),
        reachable_addresses: reachable,
        dns_error: None,
    }
}

async fn probe_tcp_address(address: SocketAddr) -> bool {
    matches!(
        tokio::time::timeout(
            NETWORK_DIAGNOSTIC_CONNECT_TIMEOUT,
            TcpStream::connect(address)
        )
        .await,
        Ok(Ok(_))
    )
}

fn error_chain(error: &(dyn StdError + 'static)) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = error.source();
    while let Some(cause) = source {
        let message = cause.to_string();
        if parts.last().is_none_or(|previous| previous != &message) {
            parts.push(message);
        }
        if parts.len() >= 6 {
            break;
        }
        source = cause.source();
    }
    parts.join("; cause: ")
}

pub fn is_retryable_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500..=599)
}

/// Telegram reports bad markup as a plain 400 whose description names the
/// entity parser. That description is the only signal there is.
fn is_parse_error(status: u16, description: &str) -> bool {
    if status != 400 {
        return false;
    }
    let lowered = description.to_ascii_lowercase();
    lowered.contains("can't parse entities")
        || lowered.contains("cant parse entities")
        || lowered.contains("unsupported start tag")
        || lowered.contains("can't find end tag")
        || lowered.contains("can't find end of the entity")
}

/// Distinguish "this chat is permanently gone" from other 4xx. Telegram has no
/// machine-readable code for it, so the description is the only signal.
fn is_chat_unreachable(status: u16, description: &str) -> bool {
    if !matches!(status, 400 | 403) {
        return false;
    }
    let lowered = description.to_ascii_lowercase();
    lowered.contains("bot was blocked by the user")
        || lowered.contains("user is deactivated")
        || lowered.contains("chat not found")
        || lowered.contains("bot was kicked")
        || lowered.contains("peer_id_invalid")
        || lowered.contains("have no rights to send a message")
}

fn parse_retry_after(value: &str) -> Option<i64> {
    let now = Utc::now();
    if let Ok(seconds) = value.trim().parse::<i64>() {
        return Some(now.timestamp().saturating_add(seconds.clamp(1, 86_400)));
    }
    chrono::DateTime::parse_from_rfc2822(value.trim())
        .ok()
        .map(|value| value.with_timezone(&Utc))
        .filter(|value| *value > now)
        .map(|value| value.min(now + chrono::Duration::hours(24)).timestamp())
}

/// Collapse newlines and scrub the bot token before anything reaches a log or
/// a database row.
pub fn sanitize(value: &str, secret: Option<&str>) -> String {
    let mut sanitized = value.replace(['\r', '\n'], " ");
    if let Some(secret) = secret.filter(|value| !value.is_empty()) {
        sanitized = sanitized.replace(secret, "[REDACTED]");
    }
    truncate(&sanitized, 1_200).to_string()
}

/// How Telegram should read a message body.
///
/// The variant is stored alongside the message rather than inferred at send
/// time: a delivery frozen into the outbox before rich formatting shipped must
/// still go out as plain text, or the `<` in its error excerpt becomes a parse
/// failure instead of a character.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TelegramParseMode {
    #[default]
    Plain,
    Html,
}

impl TelegramParseMode {
    pub const fn as_str(self) -> Option<&'static str> {
        match self {
            Self::Plain => None,
            Self::Html => Some("HTML"),
        }
    }

    /// Anything we do not recognise is treated as plain text — the reading that
    /// can never turn a stored message into a 400.
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some(value) if value.trim().eq_ignore_ascii_case("HTML") => Self::Html,
            _ => Self::Plain,
        }
    }

    pub const fn is_html(self) -> bool {
        matches!(self, Self::Html)
    }
}

/// Escape the four characters Telegram's HTML parser treats as markup.
///
/// Telegram's flavour of HTML only knows `&lt; &gt; &amp; &quot;`, so this is
/// the whole surface — far less than MarkdownV2's eighteen escapes, which is
/// exactly why this design uses HTML.
pub fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

/// Cut an HTML message to size without handing Telegram something it cannot
/// parse.
///
/// A blind character cut can land inside `<b>` or inside `&lt;`, and Telegram
/// answers an unbalanced entity with a 400 for the entire message — which would
/// turn an unusually long outage alert into a dead letter, exactly when it
/// matters most. So tags are copied through untouched, only text counts against
/// the budget, and anything still open at the cut is closed.
pub fn truncate_html(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    // Headroom for the ellipsis and the closing tags appended below.
    let budget = max_chars.saturating_sub(64);
    let mut out = String::with_capacity(value.len().min(max_chars * 4));
    let mut open: Vec<&str> = Vec::new();
    let mut used = 0usize;
    let mut rest = value;
    let mut cut = false;
    loop {
        let next_tag = rest.find('<');
        let text = &rest[..next_tag.unwrap_or(rest.len())];
        let taken = take_text(text, budget.saturating_sub(used));
        out.push_str(taken);
        used += taken.chars().count();
        if taken.len() < text.len() {
            cut = true;
            break;
        }
        let Some(next_tag) = next_tag else { break };
        rest = &rest[next_tag..];
        let Some(close) = rest.find('>') else {
            // Malformed input: stop before copying a tag that never ends.
            cut = true;
            break;
        };
        let tag = &rest[..=close];
        out.push_str(tag);
        let name = tag
            .trim_start_matches('<')
            .trim_start_matches('/')
            .trim_end_matches('>')
            .split([' ', '=', '/'])
            .next()
            .unwrap_or_default();
        if tag.starts_with("</") {
            if open.last() == Some(&name) {
                open.pop();
            }
        } else if !name.is_empty() {
            open.push(name);
        }
        rest = &rest[close + 1..];
    }
    if cut {
        out.push('…');
    }
    for name in open.iter().rev() {
        out.push_str("</");
        out.push_str(name);
        out.push('>');
    }
    out
}

/// Telegram's HTML subset. Anything outside it is markup we never emit, so a
/// stored body carrying it is a corrupted row rather than a formatting choice.
const HTML_TAGS: [&str; 10] = [
    "b",
    "strong",
    "i",
    "em",
    "u",
    "s",
    "code",
    "pre",
    "a",
    "blockquote",
];

/// Whether a stored body can go out as-is under `parse_mode`.
///
/// Deliveries are frozen at fan-out and replayed by a worker minutes or hours
/// later, so this guards the outbox rather than the renderer: an unbalanced tag
/// in an HTML body earns a 400 for the whole message and burns every retry on a
/// failure that will never resolve itself.
pub fn is_sendable(text: &str, parse_mode: TelegramParseMode) -> bool {
    match parse_mode {
        TelegramParseMode::Plain => true,
        TelegramParseMode::Html => html_tags_balanced(text),
    }
}

fn html_tags_balanced(value: &str) -> bool {
    let mut open: Vec<&str> = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find('<') {
        rest = &rest[start..];
        let Some(close) = rest.find('>') else {
            return false;
        };
        let tag = &rest[..=close];
        if tag.contains('<') && tag[1..].contains('<') {
            return false;
        }
        let closing = tag.starts_with("</");
        let name = tag
            .trim_start_matches('<')
            .trim_start_matches('/')
            .trim_end_matches('>')
            .split([' ', '=', '/'])
            .next()
            .unwrap_or_default();
        if !HTML_TAGS.contains(&name) {
            return false;
        }
        if closing {
            if open.pop() != Some(name) {
                return false;
            }
        } else {
            open.push(name);
        }
        rest = &rest[close + 1..];
    }
    open.is_empty()
}

/// Reduce an HTML body to the text it renders as.
///
/// The fallback path when Telegram rejects our markup: a plain message that
/// arrives beats a formatted one that dead-letters, and an outage alert is
/// exactly the message that must not be the one we lose.
pub fn strip_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        match rest.find('>') {
            Some(close) => rest = &rest[close + 1..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

/// Take at most `budget` characters of plain text, never splitting an entity —
/// half of `&amp;` is not text Telegram will render back as an ampersand.
fn take_text(text: &str, budget: usize) -> &str {
    let taken = truncate(text, budget);
    if taken.len() == text.len() {
        return taken;
    }
    match taken.rfind('&') {
        Some(index) if !taken[index..].contains(';') => &taken[..index],
        _ => taken,
    }
}

pub fn truncate(value: &str, max_chars: usize) -> &str {
    if value.chars().count() <= max_chars {
        return value;
    }
    let end = value
        .char_indices()
        .nth(max_chars)
        .map(|(index, _)| index)
        .unwrap_or(value.len());
    &value[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;

    #[test]
    fn bot_token_is_redacted_from_messages() {
        let message = sanitize(
            "request to https://api.telegram.org/botsecret-token/sendMessage failed",
            Some("secret-token"),
        );
        assert!(!message.contains("secret-token"));
        assert!(message.contains("[REDACTED]"));
    }

    #[test]
    fn failure_codes_have_stable_machine_names_and_actionable_hints() {
        let codes = [
            TelegramFailureCode::Configuration,
            TelegramFailureCode::DnsResolutionFailed,
            TelegramFailureCode::ApiEndpointUnreachable,
            TelegramFailureCode::ApiTimeout,
            TelegramFailureCode::TlsHandshakeFailed,
            TelegramFailureCode::InvalidToken,
            TelegramFailureCode::PollingConflict,
            TelegramFailureCode::RateLimited,
            TelegramFailureCode::ChatUnreachable,
            TelegramFailureCode::HttpError,
            TelegramFailureCode::RequestError,
            TelegramFailureCode::ResponseReadFailed,
            TelegramFailureCode::TransportError,
        ];
        for code in codes {
            assert!(!code.as_str().is_empty());
            assert!(!code.default_hint().is_empty());
        }
        assert!(
            TelegramFailureCode::ApiEndpointUnreachable
                .default_hint()
                .contains("8.8.8.8")
        );
    }

    #[test]
    fn config_failure_has_no_network_diagnostics() {
        let failure = TelegramFailure::config("missing token");
        assert_eq!(failure.code, TelegramFailureCode::Configuration);
        assert!(failure.diagnostics.is_none());
        assert!(!failure.retryable);
    }

    #[test]
    fn only_idempotent_bot_api_methods_are_connection_retried() {
        assert!(retryable_method("getMe"));
        assert!(retryable_method("getUpdates"));
        assert!(retryable_method("setWebhook"));
        assert!(!retryable_method("sendMessage"));
    }

    #[test]
    fn error_chain_preserves_nested_transport_cause() {
        #[derive(Debug)]
        struct NestedError;

        impl fmt::Display for NestedError {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("dns lookup failed")
            }
        }

        impl StdError for NestedError {}

        #[derive(Debug)]
        struct RequestError {
            source: NestedError,
        }

        impl fmt::Display for RequestError {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("request failed")
            }
        }

        impl StdError for RequestError {
            fn source(&self) -> Option<&(dyn StdError + 'static)> {
                Some(&self.source)
            }
        }

        assert_eq!(
            error_chain(&RequestError {
                source: NestedError,
            }),
            "request failed; cause: dns lookup failed"
        );
    }

    #[test]
    fn retryable_statuses_match_telegram_guidance() {
        assert!(is_retryable_status(408));
        assert!(is_retryable_status(425));
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(503));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(403));
        assert!(!is_retryable_status(409));
    }

    #[test]
    fn blocked_and_missing_chats_are_permanent() {
        assert!(is_chat_unreachable(
            403,
            "Forbidden: bot was blocked by the user"
        ));
        assert!(is_chat_unreachable(400, "Bad Request: chat not found"));
        assert!(is_chat_unreachable(403, "Forbidden: user is deactivated"));
        // A generic bad request must not silently unbind the user.
        assert!(!is_chat_unreachable(
            400,
            "Bad Request: message text is empty"
        ));
        assert!(!is_chat_unreachable(429, "Too Many Requests"));
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        let value = "中文中文中文";
        assert_eq!(truncate(value, 3), "中文中");
        assert_eq!(truncate(value, 99), value);
    }

    #[test]
    fn mode_round_trips_through_env_text() {
        use crate::config::TelegramBotMode;
        assert_eq!(
            TelegramBotMode::parse("polling"),
            Some(TelegramBotMode::Polling)
        );
        assert_eq!(
            TelegramBotMode::parse("  WEBHOOK "),
            Some(TelegramBotMode::Webhook)
        );
        assert_eq!(TelegramBotMode::parse("carrier-pigeon"), None);
        assert_eq!(TelegramBotMode::Polling.as_str(), "polling");
    }
}
