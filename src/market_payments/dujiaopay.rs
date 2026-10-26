use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use chrono::Utc;
use hmac::{Hmac, Mac};
use rand::RngCore;
use reqwest::Method;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::error::AppError;

use super::chains::token_rail;

const OFFICIAL_ORIGIN: &str = "https://www.dujiaopay.com";
const MAX_RESPONSE_BYTES: usize = 512 * 1024;
const MAX_WEBHOOK_BYTES: usize = 64 * 1024;
const WEBHOOK_TOLERANCE_SECS: i64 = 300;
pub(super) const LIST_PAGE_LIMIT: u32 = 100;

/// Stable codes for every DujiaoPay client failure. Callers branch on these
/// codes rather than on HTTP text; `details` carries the gateway's own error
/// code and request ID for support without exposing secrets.
pub(super) const ERR_UNAVAILABLE: &str = "DUJIAOPAY_UNAVAILABLE";
pub(super) const ERR_RATE_LIMITED: &str = "DUJIAOPAY_RATE_LIMITED";
pub(super) const ERR_NO_CAPACITY: &str = "DUJIAOPAY_NO_CAPACITY";
pub(super) const ERR_AUTH_FAILED: &str = "DUJIAOPAY_AUTH_FAILED";
pub(super) const ERR_NOT_FOUND: &str = "DUJIAOPAY_ORDER_NOT_FOUND";
pub(super) const ERR_CONFLICT: &str = "DUJIAOPAY_CONFLICT";
pub(super) const ERR_REJECTED: &str = "DUJIAOPAY_REJECTED";
pub(super) const ERR_IDEMPOTENCY: &str = "DUJIAOPAY_IDEMPOTENCY_CONFLICT";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GatewayErrorKind {
    /// Network, 5xx, malformed or oversized response. Retry later.
    Transient,
    /// `request.rate_limited` / `billing.api_rate_limited`. Back off.
    RateLimited,
    /// `order.no_capacity`. The gateway accepted nothing; retry later.
    Capacity,
    /// HMAC authentication failed: a channel configuration problem.
    Auth,
    /// The order is unknown to this key/project.
    NotFound,
    /// A state conflict such as `order.not_cancelable`.
    Conflict,
    /// The same idempotency key or merchant order ID exists with another body.
    /// This proves that a gateway order *exists*.
    Idempotency,
    /// A validation rejection. For an identical replay this proves the body
    /// never created a gateway order.
    Rejected,
}

pub(super) fn error_kind(error: &AppError) -> Option<GatewayErrorKind> {
    match error.code()? {
        ERR_UNAVAILABLE => Some(GatewayErrorKind::Transient),
        ERR_RATE_LIMITED => Some(GatewayErrorKind::RateLimited),
        ERR_NO_CAPACITY => Some(GatewayErrorKind::Capacity),
        ERR_AUTH_FAILED => Some(GatewayErrorKind::Auth),
        ERR_NOT_FOUND => Some(GatewayErrorKind::NotFound),
        ERR_CONFLICT => Some(GatewayErrorKind::Conflict),
        ERR_IDEMPOTENCY => Some(GatewayErrorKind::Idempotency),
        ERR_REJECTED => Some(GatewayErrorKind::Rejected),
        _ => None,
    }
}

/// The DujiaoPay error code (for example `order.no_capacity`) when the
/// gateway returned its documented error envelope.
pub(super) fn gateway_error_code(error: &AppError) -> Option<String> {
    match error {
        AppError::Coded { details, .. } => details
            .get("gatewayCode")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

fn gateway_error(
    code: &'static str,
    message: &str,
    gateway_code: Option<&str>,
    request_id: Option<&str>,
    http_status: Option<u16>,
) -> AppError {
    let status = if code == ERR_CONFLICT {
        StatusCode::CONFLICT
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    AppError::Coded {
        status,
        code,
        message: message.into(),
        details: json!({
            "gatewayCode": gateway_code,
            "requestId": request_id,
            "httpStatus": http_status,
        }),
    }
}

fn unavailable(message: &str) -> AppError {
    gateway_error(ERR_UNAVAILABLE, message, None, None, None)
}

/// Map a non-2xx response to a stable error. Only the documented `error.code`
/// and `request_id` are read; the free-text message is localized by the
/// gateway and never drives behavior.
fn classify_http_error(status: u16, body: &[u8]) -> AppError {
    let envelope: Option<Value> = serde_json::from_slice(body).ok();
    let error = envelope.as_ref().and_then(|value| value.get("error"));
    let gateway_code = error
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .filter(|code| code.len() <= 96 && code.bytes().all(|b| b.is_ascii_graphic()));
    let request_id = error
        .and_then(|value| value.get("request_id"))
        .and_then(Value::as_str)
        .filter(|id| id.len() <= 128 && id.bytes().all(|b| b.is_ascii_graphic()));
    let (code, message) = match (status, gateway_code) {
        (401, _) => (ERR_AUTH_FAILED, "DujiaoPay rejected the API credentials"),
        (429, _) => (ERR_RATE_LIMITED, "DujiaoPay rate limit reached"),
        (_, Some("order.no_capacity")) => (ERR_NO_CAPACITY, "DujiaoPay has no receiving capacity"),
        (_, Some("order.idempotency_conflict" | "order.merchant_order_id_conflict")) => (
            ERR_IDEMPOTENCY,
            "DujiaoPay order already exists with different content",
        ),
        (404, _) => (ERR_NOT_FOUND, "DujiaoPay order not found"),
        (409, _) => (ERR_CONFLICT, "DujiaoPay order state conflict"),
        (400..=499, _) => (ERR_REJECTED, "DujiaoPay rejected request"),
        _ => (ERR_UNAVAILABLE, "DujiaoPay request unavailable"),
    };
    gateway_error(code, message, gateway_code, request_id, Some(status))
}

pub(super) struct Credentials {
    pub key_id: String,
    pub api_secret: Zeroizing<String>,
    pub webhook_secret: Option<Zeroizing<String>>,
}

#[derive(Debug, Clone)]
pub(super) struct GatewayOrder {
    pub order_id: String,
    pub merchant_order_id: String,
    pub status: String,
    pub fiat_currency: String,
    pub fiat_amount_minor: i64,
    pub paid_source: Option<String>,
    pub chain: Option<String>,
    pub token_id: Option<String>,
    pub tx_id: Option<String>,
    pub settled_amount: Option<String>,
    pub confirmations: Option<i64>,
    pub paid_at: Option<String>,
    pub checkout_url: Option<String>,
    pub expires_at: Option<String>,
    pub selection_deadline: Option<String>,
    pub pay_address: Option<String>,
    pub payable_amount: Option<String>,
    pub fx_rate: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct CreatedOrder {
    pub order_id: String,
    pub checkout_url: Option<String>,
    pub expires_at: Option<String>,
    pub chain: Option<String>,
    pub token_id: Option<String>,
    pub pay_address: Option<String>,
    pub payable_amount: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct WebhookEvent {
    pub event_id: String,
    pub event_type: String,
    pub order_id: String,
    pub merchant_order_id: String,
    pub raw_sha256: String,
}

/// A signed event that carries no Router order, such as `webhook.test` or
/// `unmatched_claim.claimed`. It may prove the Webhook secret but never moves
/// money.
#[derive(Debug, Clone)]
pub(super) struct UnboundWebhookEvent {
    pub event_id: String,
    pub event_type: String,
    pub order_id: Option<String>,
}

/// Body of `POST /v1/orders`. The serialized bytes are frozen on the local
/// order before the first call so that every retry is byte-identical and an
/// `Idempotency-Key` replay can never become an idempotency conflict.
pub(super) struct CreateOrderInput<'a> {
    pub merchant_order_id: &'a str,
    pub amount_minor: i64,
    pub allowed_methods: &'a [String],
    /// Direct (in-site) mode fixes one token. `chain` is resolved from the
    /// static rail table, never from the string shape.
    pub fixed_token_id: Option<&'a str>,
    pub success_url: &'a str,
    pub cancel_url: &'a str,
    pub metadata: Option<Value>,
}

pub(super) fn create_order_body(input: &CreateOrderInput<'_>) -> Result<Vec<u8>, AppError> {
    if input.merchant_order_id.is_empty() || input.amount_minor <= 0 {
        return Err(AppError::BadRequest("invalid DujiaoPay order".into()));
    }
    let mut data = json!({
        "merchant_order_id": input.merchant_order_id,
        "fiat_currency": "USD",
        "fiat_amount": format!("{}.{:02}", input.amount_minor / 100, input.amount_minor % 100),
        "success_url": input.success_url,
        "cancel_url": input.cancel_url,
    });
    if !input.allowed_methods.is_empty() {
        let methods = input
            .allowed_methods
            .iter()
            .map(|token| {
                let chain = method_chain(token)?;
                Ok(json!({ "chain": chain, "token_id": token }))
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        data["allowed_methods"] = Value::Array(methods);
    }
    if let Some(token) = input.fixed_token_id {
        let chain = method_chain(token)?;
        if !input.allowed_methods.is_empty() && !input.allowed_methods.iter().any(|m| m == token) {
            return Err(AppError::BadRequest(
                "payment method is not allowed by this channel".into(),
            ));
        }
        data["chain"] = Value::String(chain.into());
        data["token_id"] = Value::String(token.into());
    }
    if let Some(metadata) = &input.metadata {
        data["metadata"] = metadata.clone();
    }
    serde_json::to_vec(&data)
        .map_err(|_| AppError::Internal("encode DujiaoPay order failed".into()))
}

/// The chain of a configured method. Known tokens use the documented rail
/// table; an unknown token falls back to its `{chain}-{symbol}` shape so that
/// historical revisions keep their original request bytes.
fn method_chain(token: &str) -> Result<String, AppError> {
    if let Some(rail) = token_rail(token) {
        return Ok(rail.chain.into());
    }
    token
        .rsplit_once('-')
        .map(|(chain, _)| chain.to_string())
        .filter(|chain| !chain.is_empty())
        .ok_or_else(|| AppError::BadRequest("unsupported DujiaoPay token".into()))
}

#[derive(Debug, Clone, Default)]
pub(super) struct ListOrdersQuery {
    pub status: Option<String>,
    pub created_after: Option<String>,
    pub created_before: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Clone)]
pub(super) struct ListedOrder {
    pub order_id: String,
    pub merchant_order_id: Option<String>,
    pub status: String,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct OrderPage {
    pub orders: Vec<ListedOrder>,
    /// Entries the gateway returned, including any that failed shape checks
    /// and are absent from `orders`. Paging advances by this count.
    pub returned: u32,
    /// `None` when the gateway omitted the total; never read as zero.
    pub total: Option<u64>,
}

impl OrderPage {
    /// Every returned entry was usable, so an absent ID is really absent
    /// from this page.
    pub fn complete(&self) -> bool {
        self.orders.len() == self.returned as usize
    }

    /// No further page can exist after `scanned` entries.
    pub fn is_last(&self, scanned: u64) -> bool {
        self.returned < LIST_PAGE_LIMIT || self.total.is_some_and(|total| scanned >= total)
    }
}

#[derive(Clone)]
pub(super) struct Client {
    http: reqwest::Client,
    base_url: String,
}

impl Client {
    pub fn production() -> Result<Self, AppError> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(12))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| AppError::Internal("build DujiaoPay client failed".into()))?;
        Ok(Self {
            http,
            base_url: OFFICIAL_ORIGIN.into(),
        })
    }

    #[cfg(test)]
    pub fn with_base_url(base_url: &str) -> Self {
        let mut client = Self::production().unwrap();
        client.base_url = base_url.into();
        client
    }

    async fn request(
        &self,
        credentials: &Credentials,
        method: Method,
        path: &str,
        raw_query: &str,
        body: Option<&[u8]>,
        idempotency_key: Option<&str>,
    ) -> Result<Value, AppError> {
        if !path.starts_with("/v1/") || path.contains('?') || path.contains('#') {
            return Err(AppError::Internal("invalid DujiaoPay API path".into()));
        }
        let body = body.unwrap_or_default();
        let timestamp = Utc::now().timestamp();
        let mut nonce = [0_u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let nonce = hex::encode(nonce);
        let query = canonical_query(raw_query);
        let signature = sign_request(
            credentials.api_secret.as_bytes(),
            method.as_str(),
            path,
            &query,
            body,
            timestamp,
            &nonce,
        );
        let empty_cancel_response = method == Method::POST && path.ends_with("/cancel");
        // Send exactly the canonical query so the signed bytes and the wire
        // bytes cannot differ by encoding.
        let url = if query.is_empty() {
            format!("{}{path}", self.base_url)
        } else {
            format!("{}{path}?{query}", self.base_url)
        };
        let mut request = self
            .http
            .request(method, &url)
            .header("Accept", "application/json")
            .header("DJP-Key-ID", &credentials.key_id)
            .header("DJP-Timestamp", timestamp.to_string())
            .header("DJP-Nonce", &nonce)
            .header("DJP-Signature", signature);
        if !body.is_empty() {
            request = request
                .header("Content-Type", "application/json")
                .body(body.to_vec());
        }
        if let Some(key) = idempotency_key {
            request = request.header("Idempotency-Key", key);
        }
        let response = request
            .send()
            .await
            .map_err(|_| unavailable("DujiaoPay request unavailable"))?;
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(unavailable("DujiaoPay response too large"));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| unavailable("DujiaoPay response unavailable"))?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(unavailable("DujiaoPay response too large"));
        }
        if !status.is_success() {
            let error = classify_http_error(status.as_u16(), &bytes);
            if let AppError::Coded { details, .. } = &error {
                tracing::warn!(
                    http_status = status.as_u16(),
                    gateway_code = details["gatewayCode"].as_str().unwrap_or("-"),
                    request_id = details["requestId"].as_str().unwrap_or("-"),
                    "DujiaoPay request rejected"
                );
            }
            return Err(error);
        }
        if bytes.is_empty() && empty_cancel_response {
            return Ok(Value::Null);
        }
        serde_json::from_slice::<Value>(&bytes)
            .map_err(|_| unavailable("DujiaoPay response invalid"))
    }

    pub async fn whoami(&self, credentials: &Credentials) -> Result<String, AppError> {
        let value = self
            .request(credentials, Method::GET, "/v1/whoami", "", None, None)
            .await?;
        let source = payload(&value);
        required_string(source, "merchant_id")
    }

    /// Submit a frozen create body. The merchant order ID doubles as the
    /// `Idempotency-Key`, so a replay returns the first committed order.
    pub async fn create_order_with_body(
        &self,
        credentials: &Credentials,
        merchant_order_id: &str,
        body: &[u8],
    ) -> Result<CreatedOrder, AppError> {
        if merchant_order_id.is_empty() || body.is_empty() {
            return Err(AppError::BadRequest("invalid DujiaoPay order".into()));
        }
        let value = self
            .request(
                credentials,
                Method::POST,
                "/v1/orders",
                "",
                Some(body),
                Some(merchant_order_id),
            )
            .await?;
        let source = payload(&value);
        let order_id = required_string(source, "order_id")?;
        if !valid_order_id(&order_id) {
            return Err(unavailable("DujiaoPay order ID invalid"));
        }
        // The gateway may have committed an order even when its checkout URL
        // is missing or unusable. Keep the order ID so that authenticated
        // reconciliation can recover it without creating another payment.
        let checkout_url = safe_checkout_url(optional_string(source, "checkout_url"));
        Ok(CreatedOrder {
            order_id,
            checkout_url,
            expires_at: optional_string(source, "expires_at"),
            chain: optional_string(source, "chain"),
            token_id: optional_string(source, "token_id"),
            pay_address: optional_string(source, "pay_address").filter(|a| safe_address(a)),
            payable_amount: optional_string(source, "payable_amount")
                .filter(|amount| valid_decimal(amount)),
        })
    }

    #[cfg(test)]
    pub async fn create_order(
        &self,
        credentials: &Credentials,
        merchant_order_id: &str,
        amount_minor: i64,
        allowed_methods: &[String],
        success_url: &str,
        cancel_url: &str,
    ) -> Result<CreatedOrder, AppError> {
        let body = create_order_body(&CreateOrderInput {
            merchant_order_id,
            amount_minor,
            allowed_methods,
            fixed_token_id: None,
            success_url,
            cancel_url,
            metadata: None,
        })?;
        self.create_order_with_body(credentials, merchant_order_id, &body)
            .await
    }

    pub async fn get_order(
        &self,
        credentials: &Credentials,
        order_id: &str,
    ) -> Result<GatewayOrder, AppError> {
        if !valid_order_id(order_id) {
            return Err(AppError::BadRequest("invalid DujiaoPay order ID".into()));
        }
        let path = format!("/v1/orders/{order_id}");
        let value = self
            .request(credentials, Method::GET, &path, "", None, None)
            .await?;
        parse_gateway_order(payload(&value))
    }

    pub async fn cancel_order(
        &self,
        credentials: &Credentials,
        order_id: &str,
    ) -> Result<(), AppError> {
        if !valid_order_id(order_id) {
            return Err(AppError::BadRequest("invalid DujiaoPay order ID".into()));
        }
        let path = format!("/v1/orders/{order_id}/cancel");
        self.request(credentials, Method::POST, &path, "", None, None)
            .await?;
        Ok(())
    }

    /// `GET /v1/orders`, newest first. Only identity and lifecycle fields are
    /// read; payment evidence always comes from `GET /v1/orders/{id}`.
    pub async fn list_orders(
        &self,
        credentials: &Credentials,
        query: &ListOrdersQuery,
    ) -> Result<OrderPage, AppError> {
        let mut pairs: Vec<(&str, String)> = Vec::new();
        if let Some(status) = &query.status {
            pairs.push(("status", status.clone()));
        }
        if let Some(after) = &query.created_after {
            pairs.push(("created_after", after.clone()));
        }
        if let Some(before) = &query.created_before {
            pairs.push(("created_before", before.clone()));
        }
        pairs.push(("limit", query.limit.clamp(1, LIST_PAGE_LIMIT).to_string()));
        pairs.push(("offset", query.offset.to_string()));
        let raw_query = pairs
            .iter()
            .map(|(key, value)| format!("{}={}", query_escape(key), query_escape(value)))
            .collect::<Vec<_>>()
            .join("&");
        let value = self
            .request(
                credentials,
                Method::GET,
                "/v1/orders",
                &raw_query,
                None,
                None,
            )
            .await?;
        let source = payload(&value);
        let entries = source
            .get("orders")
            .and_then(Value::as_array)
            .ok_or_else(|| unavailable("DujiaoPay order list invalid"))?;
        let orders: Vec<ListedOrder> = entries
            .iter()
            .filter_map(|order| {
                let order_id =
                    optional_string(order, "order_id").filter(|id| valid_order_id(id))?;
                Some(ListedOrder {
                    order_id,
                    merchant_order_id: optional_string(order, "merchant_order_id"),
                    status: optional_string(order, "status")?,
                    created_at: optional_string(order, "created_at"),
                })
            })
            .collect();
        if orders.len() != entries.len() {
            tracing::warn!(
                skipped = entries.len() - orders.len(),
                "DujiaoPay order list contained malformed entries"
            );
        }
        Ok(OrderPage {
            orders,
            returned: u32::try_from(entries.len()).unwrap_or(u32::MAX),
            total: source.get("total").and_then(Value::as_u64),
        })
    }
}

fn valid_order_id(order_id: &str) -> bool {
    !order_id.is_empty()
        && order_id.len() <= 128
        && order_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// A historical lookup may be scoped to an older API key. Any client-side
/// rejection except throttling may be retried once with the original key.
pub(super) fn may_be_key_scoped_order(error: &AppError) -> bool {
    matches!(
        error_kind(error),
        Some(
            GatewayErrorKind::Auth
                | GatewayErrorKind::NotFound
                | GatewayErrorKind::Rejected
                | GatewayErrorKind::Conflict
        )
    )
}

fn payload(value: &Value) -> &Value {
    value
        .get("data")
        .filter(|data| data.is_object())
        .unwrap_or(value)
}

fn required_string(value: &Value, key: &str) -> Result<String, AppError> {
    optional_string(value, key)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| unavailable("DujiaoPay response missing required field"))
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn optional_payment_string(value: &Value, key: &str) -> Result<Option<String>, AppError> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => {
            Ok(Some(text.trim().to_string()).filter(|text| !text.is_empty()))
        }
        _ => Err(unavailable("DujiaoPay payment evidence invalid")),
    }
}

fn optional_confirmations(value: &Value) -> Result<Option<i64>, AppError> {
    let confirmations = match value.get("confirmations") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Number(number)) => number.as_i64(),
        Some(Value::String(text)) => text.trim().parse::<i64>().ok(),
        _ => None,
    };
    confirmations
        .filter(|count| *count >= 0)
        .map(Some)
        .ok_or_else(|| unavailable("DujiaoPay payment evidence invalid"))
}

/// A non-negative decimal string such as `20`, `20.0001`. No sign, exponent or
/// trailing dot.
pub(super) fn valid_decimal(value: &str) -> bool {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    !whole.is_empty()
        && whole.len() <= 24
        && whole.bytes().all(|byte| byte.is_ascii_digit())
        && (!value.contains('.') || !fraction.is_empty())
        && fraction.len() <= 30
        && fraction.bytes().all(|byte| byte.is_ascii_digit())
}

/// Parse the gateway's USD fiat amount. More than two fractional digits are
/// accepted only when the extra digits are zero (`"20.000"`); any value that
/// is not an exact cent amount fails closed.
pub(super) fn parse_gateway_fiat_minor(raw: &str) -> Result<i64, AppError> {
    if !valid_decimal(raw) {
        return Err(unavailable("DujiaoPay amount invalid"));
    }
    let (whole, fraction) = raw.split_once('.').unwrap_or((raw, ""));
    if fraction.len() > 2 && fraction[2..].bytes().any(|byte| byte != b'0') {
        return Err(unavailable("DujiaoPay amount invalid"));
    }
    let cents = match fraction.len() {
        0 => 0,
        1 => i64::from(fraction.as_bytes()[0] - b'0') * 10,
        _ => fraction[..2]
            .parse::<i64>()
            .map_err(|_| unavailable("DujiaoPay amount invalid"))?,
    };
    whole
        .parse::<i64>()
        .ok()
        .and_then(|dollars| dollars.checked_mul(100))
        .and_then(|value| value.checked_add(cents))
        .ok_or_else(|| unavailable("DujiaoPay amount invalid"))
}

/// Compare two non-negative decimal strings exactly. `None` when either is not
/// a valid decimal.
pub(super) fn compare_decimal(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    if !valid_decimal(left) || !valid_decimal(right) {
        return None;
    }
    fn split(value: &str) -> (&str, &str) {
        let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
        (
            whole.trim_start_matches('0'),
            fraction.trim_end_matches('0'),
        )
    }
    let (left_whole, left_fraction) = split(left);
    let (right_whole, right_fraction) = split(right);
    let ordering = left_whole
        .len()
        .cmp(&right_whole.len())
        .then_with(|| left_whole.cmp(right_whole));
    if ordering != std::cmp::Ordering::Equal {
        return Some(ordering);
    }
    let width = left_fraction.len().max(right_fraction.len());
    let left_padded = format!("{left_fraction:0<width$}");
    let right_padded = format!("{right_fraction:0<width$}");
    Some(left_padded.cmp(&right_padded))
}

fn safe_address(address: &str) -> bool {
    (16..=128).contains(&address.len()) && address.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn parse_gateway_order(value: &Value) -> Result<GatewayOrder, AppError> {
    let fiat_amount = required_string(value, "fiat_amount")?;
    let fiat_amount_minor = parse_gateway_fiat_minor(&fiat_amount)?;
    let order_id = required_string(value, "order_id")?;
    if !valid_order_id(&order_id) {
        return Err(unavailable("DujiaoPay order ID invalid"));
    }
    Ok(GatewayOrder {
        order_id,
        merchant_order_id: required_string(value, "merchant_order_id")?,
        status: required_string(value, "status")?,
        fiat_currency: required_string(value, "fiat_currency")?.to_ascii_uppercase(),
        fiat_amount_minor,
        paid_source: optional_payment_string(value, "paid_source")?,
        chain: optional_payment_string(value, "chain")?,
        token_id: optional_payment_string(value, "token_id")?,
        tx_id: optional_payment_string(value, "tx_hash")?
            .or(optional_payment_string(value, "tx_id")?),
        settled_amount: optional_payment_string(value, "settled_amount")?,
        confirmations: optional_confirmations(value)?,
        paid_at: optional_payment_string(value, "paid_at")?,
        checkout_url: safe_checkout_url(optional_string(value, "checkout_url")),
        expires_at: optional_string(value, "expires_at"),
        selection_deadline: optional_string(value, "selection_deadline"),
        pay_address: optional_string(value, "pay_address").filter(|a| safe_address(a)),
        payable_amount: optional_string(value, "payable_amount")
            .filter(|amount| valid_decimal(amount)),
        fx_rate: optional_string(value, "fx_rate").filter(|rate| valid_decimal(rate)),
    })
}

fn verify_webhook_signature(
    secret: &[u8],
    headers: &HeaderMap,
    raw_body: &[u8],
    now_unix: i64,
) -> Result<(), AppError> {
    if raw_body.len() > MAX_WEBHOOK_BYTES {
        return Err(AppError::BadRequest("DujiaoPay webhook too large".into()));
    }
    let timestamp = headers
        .get("DJP-Webhook-Timestamp")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::Unauthorized("DujiaoPay webhook signature missing".into()))?;
    let signed_at = timestamp
        .parse::<i64>()
        .map_err(|_| AppError::Unauthorized("DujiaoPay webhook timestamp invalid".into()))?;
    if signed_at.abs_diff(now_unix) > WEBHOOK_TOLERANCE_SECS as u64 {
        return Err(AppError::Unauthorized(
            "DujiaoPay webhook timestamp expired".into(),
        ));
    }
    let signature = headers
        .get("DJP-Webhook-Signature")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::Unauthorized("DujiaoPay webhook signature missing".into()))?;
    let signature = signature
        .trim()
        .strip_prefix("sha256=")
        .unwrap_or(signature.trim());
    let decoded = hex::decode(signature)
        .map_err(|_| AppError::Unauthorized("DujiaoPay webhook signature invalid".into()))?;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret)
        .map_err(|_| AppError::Internal("DujiaoPay webhook verifier failed".into()))?;
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(raw_body);
    mac.verify_slice(&decoded)
        .map_err(|_| AppError::Unauthorized("DujiaoPay webhook signature invalid".into()))
}

fn signed_event_id(value: &Value, raw_body: &[u8]) -> String {
    optional_string(value, "event_id")
        // A header not covered by the body HMAC cannot be a replay key. When
        // the provider omits its event ID, the signed bytes are the identity.
        .unwrap_or_else(|| format!("body-{}", hex::encode(Sha256::digest(raw_body))))
}

pub(super) fn verify_webhook(
    credentials: &Credentials,
    headers: &HeaderMap,
    raw_body: &[u8],
    now_unix: i64,
) -> Result<WebhookEvent, AppError> {
    let webhook_secret = credentials.webhook_secret.as_ref().ok_or_else(|| {
        AppError::Unauthorized("DujiaoPay webhook signature not configured".into())
    })?;
    verify_webhook_signature(webhook_secret.as_bytes(), headers, raw_body, now_unix)?;
    let value: Value = serde_json::from_slice(raw_body)
        .map_err(|_| AppError::BadRequest("DujiaoPay webhook JSON invalid".into()))?;
    let data = value.get("data").unwrap_or(&Value::Null);
    Ok(WebhookEvent {
        event_id: signed_event_id(&value, raw_body),
        event_type: required_string(&value, "event_type")?,
        order_id: required_string(data, "order_id")?,
        merchant_order_id: required_string(data, "merchant_order_id")?,
        raw_sha256: hex::encode(Sha256::digest(raw_body)),
    })
}

/// Verify a signed event that is not bound to a Router order. Only its type
/// and optional gateway order ID are returned.
pub(super) fn verify_unbound_webhook(
    credentials: &Credentials,
    headers: &HeaderMap,
    raw_body: &[u8],
    now_unix: i64,
) -> Result<UnboundWebhookEvent, AppError> {
    let webhook_secret = credentials.webhook_secret.as_ref().ok_or_else(|| {
        AppError::Unauthorized("DujiaoPay webhook signature not configured".into())
    })?;
    verify_webhook_signature(webhook_secret.as_bytes(), headers, raw_body, now_unix)?;
    let value: Value = serde_json::from_slice(raw_body)
        .map_err(|_| AppError::BadRequest("DujiaoPay webhook JSON invalid".into()))?;
    let data = value.get("data").unwrap_or(&Value::Null);
    Ok(UnboundWebhookEvent {
        event_id: signed_event_id(&value, raw_body),
        event_type: required_string(&value, "event_type")?,
        order_id: optional_string(data, "order_id").filter(|id| valid_order_id(id)),
    })
}

/// Verified `order.method_selected` payment terms. Only shape-checked values
/// are returned; an authenticated lookup may overwrite them later.
#[derive(Debug, Clone, Default)]
pub(super) struct SelectedTerms {
    pub chain: Option<String>,
    pub token_id: Option<String>,
    pub pay_address: Option<String>,
    pub payable_amount: Option<String>,
    pub expires_at: Option<String>,
}

/// Read payment terms from a body whose HMAC was already verified.
pub(super) fn verified_selected_terms(raw_body: &[u8]) -> SelectedTerms {
    let Ok(value) = serde_json::from_slice::<Value>(raw_body) else {
        return SelectedTerms::default();
    };
    let data = value.get("data").unwrap_or(&Value::Null);
    SelectedTerms {
        chain: optional_string(data, "chain"),
        token_id: optional_string(data, "token_id").filter(|token| token_rail(token).is_some()),
        pay_address: optional_string(data, "pay_address").filter(|a| safe_address(a)),
        payable_amount: optional_string(data, "payable_amount").filter(|a| valid_decimal(a)),
        expires_at: optional_string(data, "expires_at"),
    }
}

/// An unauthenticated hint is permitted solely for finding the original key
/// revision. Callers must verify the HMAC before using any parsed field.
#[cfg(test)]
pub(super) fn merchant_order_hint(raw_body: &[u8]) -> Result<String, AppError> {
    optional_merchant_order_hint(raw_body)?
        .ok_or_else(|| AppError::BadRequest("DujiaoPay merchant order missing".into()))
}

/// Dashboard test callbacks may have no order. They are acknowledged by the
/// fixed endpoint without reading or changing any payment state.
pub(super) fn optional_merchant_order_hint(raw_body: &[u8]) -> Result<Option<String>, AppError> {
    if raw_body.len() > MAX_WEBHOOK_BYTES {
        return Err(AppError::BadRequest("DujiaoPay webhook too large".into()));
    }
    let value: Value = serde_json::from_slice(raw_body)
        .map_err(|_| AppError::BadRequest("DujiaoPay webhook JSON invalid".into()))?;
    if !value.is_object() {
        return Err(AppError::BadRequest(
            "DujiaoPay webhook JSON invalid".into(),
        ));
    }
    let Some(hint) = value
        .get("data")
        .and_then(|data| data.get("merchant_order_id"))
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    if !is_router_merchant_order_id(hint) {
        return Err(AppError::BadRequest(
            "DujiaoPay merchant order invalid".into(),
        ));
    }
    Ok(Some(hint.to_string()))
}

pub(super) fn is_router_merchant_order_id(value: &str) -> bool {
    value.len() == 39
        && value.starts_with("router-")
        && value[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Go `url.QueryEscape` semantics with spaces as `%20`, as required by the
/// DujiaoPay canonical query: unreserved `A-Za-z0-9-_.~` stay literal and
/// every other byte is `%XX` with upper-case hex.
pub(super) fn query_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            escaped.push(byte as char);
        } else {
            escaped.push_str(&format!("%{byte:02X}"));
        }
    }
    escaped
}

fn query_unescape(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => decoded.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                let hex_value = |byte: u8| (byte as char).to_digit(16);
                match (hex_value(bytes[index + 1]), hex_value(bytes[index + 2])) {
                    (Some(high), Some(low)) => {
                        decoded.push((high * 16 + low) as u8);
                        index += 2;
                    }
                    _ => decoded.push(b'%'),
                }
            }
            byte => decoded.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Canonical query: decode, group by key, sort keys and each key's values,
/// re-encode with [`query_escape`], join with `&`. Empty input yields "".
pub(super) fn canonical_query(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let mut pairs = raw
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (query_unescape(key), query_unescape(value))
        })
        .collect::<Vec<_>>();
    pairs.sort();
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", query_escape(key), query_escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn sign_request(
    secret: &[u8],
    method: &str,
    path: &str,
    canonical_query: &str,
    body: &[u8],
    timestamp: i64,
    nonce: &str,
) -> String {
    let body_hash = hex::encode(Sha256::digest(body));
    let canonical = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method.to_ascii_uppercase(),
        path,
        canonical_query,
        body_hash,
        timestamp,
        nonce
    );
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(canonical.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn safe_checkout_url(raw: Option<String>) -> Option<String> {
    let raw = raw?;
    match validate_checkout_url(&raw) {
        Ok(url) => Some(url),
        Err(_) => {
            // Never log the URL: its path or query may contain a checkout token.
            tracing::warn!("DujiaoPay checkout URL rejected; order remains recoverable");
            None
        }
    }
}

fn validate_checkout_url(raw: &str) -> Result<String, AppError> {
    if raw.contains('\\') || raw.bytes().any(|byte| byte <= b' ') {
        return Err(unavailable("DujiaoPay checkout URL invalid"));
    }
    let url = url::Url::parse(raw).map_err(|_| unavailable("DujiaoPay checkout URL invalid"))?;
    // DujiaoPay's current hosted-checkout contract documents this exact
    // origin and /checkout/{token} path. The token must never be logged.
    let token = url.path().strip_prefix("/checkout/");
    if url.scheme() != "https"
        || url.host_str() != Some("www.dujiaopay.com")
        || url.port().is_some()
        || !token.is_some_and(|token| {
            !token.is_empty()
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(unavailable("DujiaoPay checkout URL invalid"));
    }
    Ok(url.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;
    use axum::http::StatusCode;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use hmac::Mac;

    fn assert_signed_request(headers: &HeaderMap, method: &str, path: &str, body: &[u8]) {
        assert_signed_request_with_query(headers, method, path, "", body);
    }

    fn assert_signed_request_with_query(
        headers: &HeaderMap,
        method: &str,
        path: &str,
        query: &str,
        body: &[u8],
    ) {
        assert_eq!(headers.get("DJP-Key-ID").unwrap(), "key");
        let timestamp = headers
            .get("DJP-Timestamp")
            .unwrap()
            .to_str()
            .unwrap()
            .parse::<i64>()
            .unwrap();
        let nonce = headers.get("DJP-Nonce").unwrap().to_str().unwrap();
        let signature = headers.get("DJP-Signature").unwrap().to_str().unwrap();
        assert_eq!(
            signature,
            sign_request(b"api-secret", method, path, query, body, timestamp, nonce)
        );
    }

    async fn mock_whoami(headers: HeaderMap) -> Json<Value> {
        assert_signed_request(&headers, "GET", "/v1/whoami", &[]);
        Json(json!({ "data": { "merchant_id": "merchant-1" } }))
    }

    async fn mock_create(headers: HeaderMap, body: Bytes) -> Json<Value> {
        assert_signed_request(&headers, "POST", "/v1/orders", &body);
        assert_eq!(
            headers.get("Idempotency-Key").unwrap(),
            "router-00000000000000000000000000000001"
        );
        let request: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(request["fiat_currency"], "USD");
        assert_eq!(request["fiat_amount"], "1.23");
        assert_eq!(request["allowed_methods"][0]["chain"], "tron");
        Json(json!({ "data": {
            "order_id": "gateway_1",
            "checkout_url": "https://www.dujiaopay.com/checkout/ct_1",
            "expires_at": "2026-10-01T01:00:00Z"
        } }))
    }

    async fn mock_get(headers: HeaderMap) -> Json<Value> {
        assert_signed_request(&headers, "GET", "/v1/orders/gateway_1", &[]);
        Json(json!({ "data": {
            "order_id": "gateway_1",
            "merchant_order_id": "router-00000000000000000000000000000001",
            "status": "paid",
            "fiat_currency": "USD",
            "fiat_amount": "1.23",
            "paid_source": "chain",
            "chain": "tron",
            "token_id": "tron-usdt",
            "tx_hash": "chain-tx",
            "settled_amount": "1.23",
            "confirmations": 12
        } }))
    }

    async fn mock_cancel(headers: HeaderMap, body: Bytes) -> StatusCode {
        assert_signed_request(&headers, "POST", "/v1/orders/gateway_1/cancel", &body);
        assert!(body.is_empty());
        assert!(headers.get("Idempotency-Key").is_none());
        StatusCode::NO_CONTENT
    }

    async fn mock_cancel_conflict(headers: HeaderMap, body: Bytes) -> StatusCode {
        assert_signed_request(&headers, "POST", "/v1/orders/gateway_1/cancel", &body);
        assert!(body.is_empty());
        StatusCode::CONFLICT
    }

    async fn mock_terminal_get(headers: HeaderMap) -> Json<Value> {
        assert_signed_request(&headers, "GET", "/v1/orders/gateway_1", &[]);
        Json(json!({ "data": {
            "order_id": "gateway_1",
            "merchant_order_id": "router-00000000000000000000000000000001",
            "status": "canceled",
            "fiat_currency": "USD",
            "fiat_amount": "1.23",
            "selection_deadline": "2026-10-01T00:30:00Z",
            "expires_at": "2026-10-01T01:00:00Z"
        } }))
    }

    #[tokio::test]
    async fn mock_gateway_roundtrip_checks_signed_creation_and_verified_shape() {
        let app = Router::new()
            .route("/v1/whoami", get(mock_whoami))
            .route("/v1/orders", post(mock_create))
            .route("/v1/orders/gateway_1", get(mock_get))
            .route("/v1/orders/gateway_1/cancel", post(mock_cancel));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = Client::with_base_url(&origin);
        let credentials = credentials();
        assert_eq!(client.whoami(&credentials).await.unwrap(), "merchant-1");
        let created = client
            .create_order(
                &credentials,
                "router-00000000000000000000000000000001",
                123,
                &["tron-usdt".into()],
                "https://router.example/account",
                "https://router.example/account?cancelled=1",
            )
            .await
            .unwrap();
        assert_eq!(created.order_id, "gateway_1");
        assert_eq!(
            created.checkout_url.as_deref(),
            Some("https://www.dujiaopay.com/checkout/ct_1")
        );
        let order = client
            .get_order(&credentials, &created.order_id)
            .await
            .unwrap();
        assert_eq!(order.fiat_amount_minor, 123);
        assert_eq!(order.paid_source.as_deref(), Some("chain"));
        assert_eq!(order.confirmations, Some(12));
        client
            .cancel_order(&credentials, &created.order_id)
            .await
            .unwrap();
        assert!(client.cancel_order(&credentials, "../other").await.is_err());
        server.abort();
    }

    #[tokio::test]
    async fn rejected_cancel_is_not_terminal_evidence_and_get_remains_available() {
        let app = Router::new()
            .route("/v1/orders/gateway_1", get(mock_terminal_get))
            .route("/v1/orders/gateway_1/cancel", post(mock_cancel_conflict));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = Client::with_base_url(&origin);
        let credentials = credentials();
        assert!(
            client
                .cancel_order(&credentials, "gateway_1")
                .await
                .is_err()
        );
        let checked = client.get_order(&credentials, "gateway_1").await.unwrap();
        assert_eq!(checked.status, "canceled");
        assert_eq!(
            checked.selection_deadline.as_deref(),
            Some("2026-10-01T00:30:00Z")
        );
        assert_eq!(checked.expires_at.as_deref(), Some("2026-10-01T01:00:00Z"));
        server.abort();
    }

    #[tokio::test]
    async fn create_keeps_gateway_identity_when_checkout_link_is_rejected() {
        async fn invalid_checkout() -> Json<Value> {
            Json(json!({ "data": {
                "order_id": "gateway_1",
                "checkout_url": "https://evil.example/checkout/ct_1"
            } }))
        }
        let app = Router::new().route("/v1/orders", post(invalid_checkout));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let created = Client::with_base_url(&origin)
            .create_order(
                &credentials(),
                "router-order",
                100,
                &[],
                "https://router.example/success",
                "https://router.example/cancel",
            )
            .await
            .unwrap();
        assert_eq!(created.order_id, "gateway_1");
        assert!(created.checkout_url.is_none());
        server.abort();
    }

    fn credentials() -> Credentials {
        Credentials {
            key_id: "key".into(),
            api_secret: Zeroizing::new("api-secret".into()),
            webhook_secret: Some(Zeroizing::new("webhook-secret".into())),
        }
    }

    #[test]
    fn signing_matches_canonical_raw_body_contract() {
        let actual = sign_request(
            b"secret",
            "POST",
            "/v1/orders",
            "",
            br#"{"fiat_amount":"20.00"}"#,
            1_750_000_000,
            "abc",
        );
        let body_hash = hex::encode(Sha256::digest(br#"{"fiat_amount":"20.00"}"#));
        let canonical = format!("POST\n/v1/orders\n\n{body_hash}\n1750000000\nabc");
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(b"secret").unwrap();
        mac.update(canonical.as_bytes());
        assert_eq!(actual, hex::encode(mac.finalize().into_bytes()));
    }

    #[test]
    fn webhook_uses_exact_body_bytes_and_time_window() {
        let raw = br#"{"event_id":"evt","event_type":"order.paid","data":{"order_id":"do_1","merchant_order_id":"mo_1"}}"#;
        let timestamp = "1750000000";
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(b"webhook-secret").unwrap();
        mac.update(timestamp.as_bytes());
        mac.update(b".");
        mac.update(raw);
        let mut headers = HeaderMap::new();
        headers.insert("DJP-Webhook-Timestamp", timestamp.parse().unwrap());
        headers.insert(
            "DJP-Webhook-Signature",
            hex::encode(mac.finalize().into_bytes()).parse().unwrap(),
        );
        assert_eq!(
            verify_webhook(&credentials(), &headers, raw, 1_750_000_000)
                .unwrap()
                .event_id,
            "evt"
        );
        assert!(verify_webhook(&credentials(), &headers, b"{}", 1_750_000_000).is_err());
        assert!(verify_webhook(&credentials(), &headers, raw, 1_750_000_301).is_err());
        let mut poll_only = credentials();
        poll_only.webhook_secret = None;
        assert!(verify_webhook(&poll_only, &headers, raw, 1_750_000_000).is_err());
    }

    #[test]
    fn unsigned_webhook_order_hint_is_strict_and_not_an_authorization() {
        let raw = br#"{"event_type":"order.paid","data":{"order_id":"do_1","merchant_order_id":"router-00000000000000000000000000000001"}}"#;
        assert_eq!(
            merchant_order_hint(raw).unwrap(),
            "router-00000000000000000000000000000001"
        );
        assert!(merchant_order_hint(br#"{"data":{"merchant_order_id":"other"}}"#).is_err());
        assert!(verify_webhook(&credentials(), &HeaderMap::new(), raw, 1_750_000_000).is_err());
        assert_eq!(
            optional_merchant_order_hint(br#"{"event_type":"webhook.test"}"#).unwrap(),
            None
        );
        assert!(optional_merchant_order_hint(b"not-json").is_err());
    }

    #[test]
    fn checkout_url_cannot_redirect_to_another_origin() {
        assert_eq!(
            validate_checkout_url("https://www.dujiaopay.com/checkout/ct_1").unwrap(),
            "https://www.dujiaopay.com/checkout/ct_1"
        );
        for bad in [
            "http://www.dujiaopay.com/checkout/ct_1",
            "https://www.dujiaopay.com.evil/checkout/ct_1",
            "https://dujiaopay.com/checkout/ct_1",
            "https://pay.dujiaopay.com/checkout/ct_1",
            "https://evil@www.dujiaopay.com/checkout/ct_1",
            "https://www.dujiaopay.com:444/checkout/ct_1",
            "https://www.dujiaopay.com/",
            "https://www.dujiaopay.com/c/ct_1",
            "/checkout/ct_1",
            "//evil.example/checkout/ct_1",
            "https://www.dujiaopay.com/checkout/ct_1#redirect",
            "https://www.dujiaopay.com/checkout/ct_1?next=https://evil.example",
            "https://www.dujiaopay.com/checkout/ct_1/other",
            "https://www.dujiaopay.com/checkout/%2f%2fevil.example",
            "https://www.dujiaopay.com\\checkout\\ct_1",
        ] {
            assert!(validate_checkout_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn gateway_paid_source_and_amount_are_exact() {
        let order = parse_gateway_order(&json!({
            "order_id":"do_1", "merchant_order_id":"mo_1", "status":"paid",
            "fiat_currency":"usd", "fiat_amount":"20.00", "paid_source":"chain",
            "chain":"tron", "token_id":"tron-usdt", "tx_hash":"tx1",
            "settled_amount":"20.00", "confirmations":20
        }))
        .unwrap();
        assert_eq!(order.fiat_amount_minor, 2000);
        assert_eq!(order.paid_source.as_deref(), Some("chain"));
    }

    #[test]
    fn gateway_payment_evidence_with_unexpected_types_fails_closed() {
        let base = json!({
            "order_id": "do_1", "merchant_order_id": "mo_1", "status": "expired",
            "fiat_currency": "USD", "fiat_amount": "20.00"
        });
        for (field, invalid) in [
            ("paid_source", json!(true)),
            ("tx_hash", json!(123)),
            ("tx_id", json!({"hash": "tx1"})),
            ("settled_amount", json!(0.01)),
            ("paid_at", json!(123)),
            ("confirmations", json!({"count": 1})),
            ("confirmations", json!(-1)),
            ("confirmations", json!("not-a-number")),
        ] {
            let mut value = base.clone();
            value[field] = invalid;
            assert!(parse_gateway_order(&value).is_err(), "{field}");
        }
        let mut value = base;
        value["confirmations"] = json!("0");
        assert_eq!(parse_gateway_order(&value).unwrap().confirmations, Some(0));
        value["confirmations"] = json!("1");
        assert_eq!(parse_gateway_order(&value).unwrap().confirmations, Some(1));
    }

    #[test]
    fn canonical_query_sorts_decoded_pairs_and_uses_go_query_escape() {
        assert_eq!(canonical_query(""), "");
        assert_eq!(
            canonical_query("b=2&a=1&a=0&q=hello+world"),
            "a=0&a=1&b=2&q=hello%20world"
        );
        assert_eq!(
            canonical_query("created_after=2026-10-01T00%3A00%3A00Z&limit=100"),
            "created_after=2026-10-01T00%3A00%3A00Z&limit=100"
        );
        assert_eq!(query_escape("a~b.c-d_e"), "a~b.c-d_e");
        assert_eq!(query_escape("x!*'()"), "x%21%2A%27%28%29");
        // A malformed or multi-byte escape never panics.
        assert_eq!(canonical_query("k=%zz&v=%E2%82"), "k=%25zz&v=%EF%BF%BD");
    }

    #[test]
    fn signing_includes_the_canonical_query_line() {
        let with_query = sign_request(b"s", "GET", "/v1/orders", "limit=1", b"", 1, "n");
        let without_query = sign_request(b"s", "GET", "/v1/orders", "", b"", 1, "n");
        assert_ne!(with_query, without_query);
        let body_hash = hex::encode(Sha256::digest(b""));
        let canonical = format!("GET\n/v1/orders\nlimit=1\n{body_hash}\n1\nn");
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(b"s").unwrap();
        mac.update(canonical.as_bytes());
        assert_eq!(with_query, hex::encode(mac.finalize().into_bytes()));
    }

    #[test]
    fn gateway_errors_are_classified_by_status_and_documented_code() {
        let body = |code: &str| {
            serde_json::to_vec(&json!({ "error": {
                "code": code, "message": "localized", "request_id": "req_1"
            }}))
            .unwrap()
        };
        let cases = [
            (401, "auth.request_unauthorized", GatewayErrorKind::Auth),
            (
                429,
                "billing.api_rate_limited",
                GatewayErrorKind::RateLimited,
            ),
            (422, "order.no_capacity", GatewayErrorKind::Capacity),
            (
                422,
                "order.idempotency_conflict",
                GatewayErrorKind::Idempotency,
            ),
            (
                409,
                "order.merchant_order_id_conflict",
                GatewayErrorKind::Idempotency,
            ),
            (404, "order.not_found", GatewayErrorKind::NotFound),
            (409, "order.not_cancelable", GatewayErrorKind::Conflict),
            (400, "request.invalid_argument", GatewayErrorKind::Rejected),
            (500, "internal.error", GatewayErrorKind::Transient),
        ];
        for (status, code, kind) in cases {
            let error = classify_http_error(status, &body(code));
            assert_eq!(error_kind(&error), Some(kind), "{code}");
            assert_eq!(gateway_error_code(&error).as_deref(), Some(code));
            if let AppError::Coded { details, .. } = &error {
                assert_eq!(details["requestId"], "req_1");
            }
        }
        let opaque = classify_http_error(400, b"<html>");
        assert_eq!(error_kind(&opaque), Some(GatewayErrorKind::Rejected));
        assert!(gateway_error_code(&opaque).is_none());
        assert!(may_be_key_scoped_order(&classify_http_error(404, b"")));
        assert!(!may_be_key_scoped_order(&classify_http_error(429, b"")));
        assert!(!may_be_key_scoped_order(&classify_http_error(503, b"")));
    }

    #[test]
    fn fiat_amount_accepts_only_exact_cents() {
        assert_eq!(parse_gateway_fiat_minor("20").unwrap(), 2000);
        assert_eq!(parse_gateway_fiat_minor("20.5").unwrap(), 2050);
        assert_eq!(parse_gateway_fiat_minor("20.000").unwrap(), 2000);
        assert_eq!(parse_gateway_fiat_minor("0.01").unwrap(), 1);
        for bad in ["20.001", "1.", ".5", "-1", "1e3", "", "20.0.0", " 1"] {
            assert!(parse_gateway_fiat_minor(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn decimal_comparison_is_exact_and_precision_independent() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_decimal("20.0001", "20.0001"), Some(Equal));
        assert_eq!(compare_decimal("20.00010", "20.0001"), Some(Equal));
        assert_eq!(compare_decimal("007.5", "7.50"), Some(Equal));
        assert_eq!(compare_decimal("20", "20.0001"), Some(Less));
        assert_eq!(compare_decimal("100", "99.9999"), Some(Greater));
        assert_eq!(compare_decimal("0", "0.000"), Some(Equal));
        assert_eq!(compare_decimal("1e2", "100"), None);
    }

    #[test]
    fn direct_create_body_fixes_a_tabled_rail_and_carries_metadata() {
        let allowed = vec!["x-layer-usdt0".to_string(), "tron-usdt".to_string()];
        let body = create_order_body(&CreateOrderInput {
            merchant_order_id: "router-1",
            amount_minor: 2000,
            allowed_methods: &allowed,
            fixed_token_id: Some("x-layer-usdt0"),
            success_url: "https://router.example/ok",
            cancel_url: "https://router.example/cancel",
            metadata: Some(json!({ "router_order_id": "local" })),
        })
        .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["chain"], "x-layer");
        assert_eq!(value["token_id"], "x-layer-usdt0");
        assert_eq!(value["fiat_amount"], "20.00");
        assert_eq!(value["allowed_methods"][0]["chain"], "x-layer");
        assert_eq!(value["metadata"]["router_order_id"], "local");
        assert!(
            create_order_body(&CreateOrderInput {
                merchant_order_id: "router-1",
                amount_minor: 2000,
                allowed_methods: &allowed,
                fixed_token_id: Some("bsc-usdt"),
                success_url: "https://router.example/ok",
                cancel_url: "https://router.example/cancel",
                metadata: None,
            })
            .is_err()
        );
        // The same input always serializes to the same frozen bytes.
        let again = create_order_body(&CreateOrderInput {
            merchant_order_id: "router-1",
            amount_minor: 2000,
            allowed_methods: &allowed,
            fixed_token_id: Some("x-layer-usdt0"),
            success_url: "https://router.example/ok",
            cancel_url: "https://router.example/cancel",
            metadata: Some(json!({ "router_order_id": "local" })),
        })
        .unwrap();
        assert_eq!(body, again);
    }

    async fn mock_list(
        headers: HeaderMap,
        axum::extract::RawQuery(query): axum::extract::RawQuery,
    ) -> Json<Value> {
        let query = query.unwrap_or_default();
        assert_eq!(canonical_query(&query), query);
        assert_signed_request_with_query(&headers, "GET", "/v1/orders", &query, &[]);
        assert!(query.contains("status=paid"));
        Json(json!({
            "orders": [
                { "order_id": "do_1", "merchant_order_id": "router-x", "status": "paid",
                  "created_at": "2026-10-01T00:00:00Z" },
                { "order_id": "../bad", "status": "paid" },
                { "order_id": "do_2", "status": "paid" }
            ],
            "total": 3, "limit": 100, "offset": 0
        }))
    }

    #[tokio::test]
    async fn list_orders_signs_its_query_and_skips_unsafe_rows() {
        let app = Router::new().route("/v1/orders", get(mock_list));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let page = Client::with_base_url(&origin)
            .list_orders(
                &credentials(),
                &ListOrdersQuery {
                    status: Some("paid".into()),
                    created_after: Some("2026-10-01T00:00:00Z".into()),
                    limit: 100,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.total, Some(3));
        assert_eq!(page.orders.len(), 2);
        // The skipped row still counts for paging but makes the page
        // incomplete, so it can never prove an order absent.
        assert_eq!(page.returned, 3);
        assert!(!page.complete());
        assert!(page.is_last(3));
        assert_eq!(
            page.orders[0].merchant_order_id.as_deref(),
            Some("router-x")
        );
        assert!(page.orders[1].merchant_order_id.is_none());
        server.abort();
    }

    #[test]
    fn signed_unbound_events_and_selected_terms_are_shape_checked() {
        let raw = br#"{"event_id":"evt_t","event_type":"webhook.test","data":{}}"#;
        let timestamp = "1750000000";
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(b"webhook-secret").unwrap();
        mac.update(timestamp.as_bytes());
        mac.update(b".");
        mac.update(raw);
        let mut headers = HeaderMap::new();
        headers.insert("DJP-Webhook-Timestamp", timestamp.parse().unwrap());
        headers.insert(
            "DJP-Webhook-Signature",
            hex::encode(mac.finalize().into_bytes()).parse().unwrap(),
        );
        let event = verify_unbound_webhook(&credentials(), &headers, raw, 1_750_000_000).unwrap();
        assert_eq!(event.event_type, "webhook.test");
        assert!(event.order_id.is_none());
        let mut wrong = credentials();
        wrong.webhook_secret = Some(Zeroizing::new("other".into()));
        assert!(verify_unbound_webhook(&wrong, &headers, raw, 1_750_000_000).is_err());

        let terms = verified_selected_terms(
            br#"{"data":{"chain":"tron","token_id":"tron-usdt","pay_address":"TQ5YwK8nM9zJx1aBcDeFgHiJkLmNoPqRsT",
                "payable_amount":"20.0001","expires_at":"2026-10-01T00:15:00Z"}}"#,
        );
        assert_eq!(terms.token_id.as_deref(), Some("tron-usdt"));
        assert_eq!(terms.payable_amount.as_deref(), Some("20.0001"));
        assert!(terms.pay_address.is_some());
        let unsafe_terms = verified_selected_terms(
            br#"{"data":{"token_id":"evil-coin","pay_address":"javascript:alert(1)","payable_amount":"-1"}}"#,
        );
        assert!(unsafe_terms.token_id.is_none());
        assert!(unsafe_terms.pay_address.is_none());
        assert!(unsafe_terms.payable_amount.is_none());
    }
}
