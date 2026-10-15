use std::time::Duration;

use axum::http::HeaderMap;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rand::RngCore;
use reqwest::Method;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::error::AppError;
use crate::market_wallet::parse_usd_minor;

const OFFICIAL_ORIGIN: &str = "https://www.dujiaopay.com";
const MAX_RESPONSE_BYTES: usize = 128 * 1024;
const MAX_WEBHOOK_BYTES: usize = 64 * 1024;
const WEBHOOK_TOLERANCE_SECS: i64 = 300;

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
}

#[derive(Debug, Clone)]
pub(super) struct CreatedOrder {
    pub order_id: String,
    pub checkout_url: String,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct WebhookEvent {
    pub event_id: String,
    pub event_type: String,
    pub order_id: String,
    pub merchant_order_id: String,
    pub raw_sha256: String,
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
        let signature = sign_request(
            credentials.api_secret.as_bytes(),
            method.as_str(),
            path,
            body,
            timestamp,
            &nonce,
        );
        let url = format!("{}{path}", self.base_url);
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
            .map_err(|_| AppError::ServiceUnavailable("DujiaoPay request unavailable".into()))?;
        if !response.status().is_success() {
            return Err(AppError::ServiceUnavailable(
                "DujiaoPay rejected request".into(),
            ));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(AppError::ServiceUnavailable(
                "DujiaoPay response too large".into(),
            ));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| AppError::ServiceUnavailable("DujiaoPay response unavailable".into()))?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(AppError::ServiceUnavailable(
                "DujiaoPay response too large".into(),
            ));
        }
        serde_json::from_slice::<Value>(&bytes)
            .map_err(|_| AppError::ServiceUnavailable("DujiaoPay response invalid".into()))
    }

    pub async fn whoami(&self, credentials: &Credentials) -> Result<String, AppError> {
        let value = self
            .request(credentials, Method::GET, "/v1/whoami", None, None)
            .await?;
        let source = payload(&value);
        required_string(source, "merchant_id")
    }

    pub async fn create_order(
        &self,
        credentials: &Credentials,
        merchant_order_id: &str,
        amount_minor: i64,
        allowed_methods: &[String],
        success_url: &str,
        cancel_url: &str,
    ) -> Result<CreatedOrder, AppError> {
        if merchant_order_id.is_empty() || amount_minor <= 0 {
            return Err(AppError::BadRequest("invalid DujiaoPay order".into()));
        }
        let mut data = json!({
            "merchant_order_id": merchant_order_id,
            "fiat_currency": "USD",
            "fiat_amount": format!("{}.{:02}", amount_minor / 100, amount_minor % 100),
            "success_url": success_url,
            "cancel_url": cancel_url,
        });
        if !allowed_methods.is_empty() {
            let methods = allowed_methods
                .iter()
                .map(|token| {
                    let chain = token.rsplit_once('-').map(|(chain, _)| chain).unwrap_or("");
                    json!({ "chain": chain, "token_id": token })
                })
                .collect::<Vec<_>>();
            data["allowed_methods"] = Value::Array(methods);
        }
        let body = serde_json::to_vec(&data)
            .map_err(|_| AppError::Internal("encode DujiaoPay order failed".into()))?;
        let value = self
            .request(
                credentials,
                Method::POST,
                "/v1/orders",
                Some(&body),
                Some(merchant_order_id),
            )
            .await?;
        let source = payload(&value);
        let order_id = required_string(source, "order_id")?;
        let checkout_url = required_string(source, "checkout_url")?;
        validate_checkout_url(&checkout_url)?;
        Ok(CreatedOrder {
            order_id,
            checkout_url,
            expires_at: optional_string(source, "expires_at"),
        })
    }

    pub async fn get_order(
        &self,
        credentials: &Credentials,
        order_id: &str,
    ) -> Result<GatewayOrder, AppError> {
        if order_id.is_empty()
            || !order_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(AppError::BadRequest("invalid DujiaoPay order ID".into()));
        }
        let path = format!("/v1/orders/{order_id}");
        let value = self
            .request(credentials, Method::GET, &path, None, None)
            .await?;
        parse_gateway_order(payload(&value))
    }
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
        .ok_or_else(|| {
            AppError::ServiceUnavailable("DujiaoPay response missing required field".into())
        })
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn parse_gateway_order(value: &Value) -> Result<GatewayOrder, AppError> {
    let fiat_amount = required_string(value, "fiat_amount")?;
    let fiat_amount_minor = parse_usd_minor(&fiat_amount)
        .map_err(|_| AppError::ServiceUnavailable("DujiaoPay amount invalid".into()))?;
    Ok(GatewayOrder {
        order_id: required_string(value, "order_id")?,
        merchant_order_id: required_string(value, "merchant_order_id")?,
        status: required_string(value, "status")?,
        fiat_currency: required_string(value, "fiat_currency")?.to_ascii_uppercase(),
        fiat_amount_minor,
        paid_source: optional_string(value, "paid_source"),
        chain: optional_string(value, "chain"),
        token_id: optional_string(value, "token_id"),
        tx_id: optional_string(value, "tx_hash").or_else(|| optional_string(value, "tx_id")),
        settled_amount: optional_string(value, "settled_amount"),
        confirmations: value.get("confirmations").and_then(Value::as_i64),
        paid_at: optional_string(value, "paid_at"),
    })
}

pub(super) fn verify_webhook(
    credentials: &Credentials,
    headers: &HeaderMap,
    raw_body: &[u8],
    now_unix: i64,
) -> Result<WebhookEvent, AppError> {
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
    let webhook_secret = credentials.webhook_secret.as_ref().ok_or_else(|| {
        AppError::Unauthorized("DujiaoPay webhook signature not configured".into())
    })?;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(webhook_secret.as_bytes())
        .map_err(|_| AppError::Internal("DujiaoPay webhook verifier failed".into()))?;
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(raw_body);
    mac.verify_slice(&decoded)
        .map_err(|_| AppError::Unauthorized("DujiaoPay webhook signature invalid".into()))?;
    let value: Value = serde_json::from_slice(raw_body)
        .map_err(|_| AppError::BadRequest("DujiaoPay webhook JSON invalid".into()))?;
    let data = value.get("data").unwrap_or(&Value::Null);
    let event_id = optional_string(&value, "event_id")
        // A header not covered by the body HMAC cannot be a replay key. When
        // the provider omits its event ID, the signed bytes are the identity.
        .unwrap_or_else(|| format!("body-{}", hex::encode(Sha256::digest(raw_body))));
    Ok(WebhookEvent {
        event_id,
        event_type: required_string(&value, "event_type")?,
        order_id: required_string(data, "order_id")?,
        merchant_order_id: required_string(data, "merchant_order_id")?,
        raw_sha256: hex::encode(Sha256::digest(raw_body)),
    })
}

/// An unauthenticated hint is permitted solely for finding the original key
/// revision. Callers must verify the HMAC before using any parsed field.
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
    if hint.len() != 39
        || !hint.starts_with("router-")
        || !hint[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(AppError::BadRequest(
            "DujiaoPay merchant order invalid".into(),
        ));
    }
    Ok(Some(hint.to_string()))
}

fn sign_request(
    secret: &[u8],
    method: &str,
    path: &str,
    body: &[u8],
    timestamp: i64,
    nonce: &str,
) -> String {
    let body_hash = hex::encode(Sha256::digest(body));
    let canonical = format!(
        "{}\n{}\n\n{}\n{}\n{}",
        method.to_ascii_uppercase(),
        path,
        body_hash,
        timestamp,
        nonce
    );
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(canonical.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn validate_checkout_url(raw: &str) -> Result<(), AppError> {
    let url = url::Url::parse(raw)
        .map_err(|_| AppError::ServiceUnavailable("DujiaoPay checkout URL invalid".into()))?;
    if url.scheme() != "https"
        || url.host_str() != Some("www.dujiaopay.com")
        || url.port().is_some()
        || !url.path().starts_with("/c/")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(AppError::ServiceUnavailable(
            "DujiaoPay checkout URL invalid".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use hmac::Mac;

    fn assert_signed_request(headers: &HeaderMap, method: &str, path: &str, body: &[u8]) {
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
            sign_request(b"api-secret", method, path, body, timestamp, nonce)
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
            "checkout_url": "https://www.dujiaopay.com/c/gateway_1",
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

    #[tokio::test]
    async fn mock_gateway_roundtrip_checks_signed_creation_and_verified_shape() {
        let app = Router::new()
            .route("/v1/whoami", get(mock_whoami))
            .route("/v1/orders", post(mock_create))
            .route("/v1/orders/gateway_1", get(mock_get));
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
        let order = client
            .get_order(&credentials, &created.order_id)
            .await
            .unwrap();
        assert_eq!(order.fiat_amount_minor, 123);
        assert_eq!(order.paid_source.as_deref(), Some("chain"));
        assert_eq!(order.confirmations, Some(12));
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
        assert!(validate_checkout_url("https://www.dujiaopay.com/c/ct_1").is_ok());
        for bad in [
            "http://www.dujiaopay.com/c/ct_1",
            "https://www.dujiaopay.com.evil/c/ct_1",
            "https://evil@www.dujiaopay.com/c/ct_1",
            "https://www.dujiaopay.com:444/c/ct_1",
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
}
