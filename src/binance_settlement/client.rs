use std::collections::BTreeMap;
use std::time::Duration;

use chrono::Utc;
use futures_util::StreamExt;
use hmac::{Hmac, Mac};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use url::Url;

use super::crypto::BinanceCredentials;

const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone)]
pub struct BinanceClient {
    http: reqwest::Client,
    base_url: Url,
}

#[derive(Debug, Clone)]
pub struct BinanceApiError {
    pub code: String,
    pub retry_after_secs: Option<u64>,
}

impl std::fmt::Display for BinanceApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.code)
    }
}

impl std::error::Error for BinanceApiError {}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BinancePayTransaction {
    #[serde(default)]
    pub order_id: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub order_type: String,
    #[serde(default)]
    pub transaction_id: String,
    pub transaction_time: i64,
    pub amount: String,
    pub currency: String,
    #[serde(default)]
    pub counterparty_id: serde_json::Value,
    #[serde(default)]
    pub payer_info: PartyInfo,
    #[serde(default)]
    pub receiver_info: PartyInfo,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartyInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub binance_id: serde_json::Value,
}

impl PartyInfo {
    pub fn uid(&self) -> Option<String> {
        value_as_identifier(&self.binance_id)
    }
}

pub fn value_as_identifier(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(value) if !value.trim().is_empty() => {
            Some(value.trim().to_string())
        }
        serde_json::Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationResult {
    pub reading_enabled: bool,
    pub dangerous_permissions_disabled: bool,
    pub uid_confirmed: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiRestrictions {
    enable_reading: bool,
    enable_withdrawals: bool,
    enable_internal_transfer: bool,
    permits_universal_transfer: bool,
    enable_spot_and_margin_trading: bool,
    enable_margin: bool,
    enable_futures: bool,
    enable_portfolio_margin_trading: bool,
    enable_vanilla_options: bool,
    enable_fix_api_trade: bool,
    #[serde(flatten)]
    additional_fields: BTreeMap<String, serde_json::Value>,
}

impl ApiRestrictions {
    fn has_dangerous_permissions(&self) -> bool {
        self.enable_withdrawals
            || self.enable_internal_transfer
            || self.permits_universal_transfer
            || self.enable_spot_and_margin_trading
            || self.enable_margin
            || self.enable_futures
            || self.enable_portfolio_margin_trading
            || self.enable_vanilla_options
            || self.enable_fix_api_trade
            || self.additional_fields.iter().any(|(name, value)| {
                let normalized = name.to_ascii_lowercase();
                let permission_like =
                    normalized.starts_with("enable") || normalized.starts_with("permit");
                let explicitly_safe =
                    normalized == "enablefixreadonly" && value.as_bool().is_some();
                permission_like && !explicitly_safe && value.as_bool() != Some(false)
            })
    }
}

#[derive(Debug, Deserialize)]
struct PayTransactionsEnvelope {
    data: Vec<BinancePayTransaction>,
    success: bool,
}

#[derive(Debug, Deserialize)]
struct BinanceErrorEnvelope {
    code: i64,
}

impl BinanceClient {
    pub fn new(base_url: Url) -> Result<Self, anyhow::Error> {
        Self::with_timeout(base_url, Duration::from_secs(15))
    }

    fn with_timeout(base_url: Url, timeout: Duration) -> Result<Self, anyhow::Error> {
        let http = reqwest::Client::builder()
            .user_agent("cc-switch-router/0.1 binance-settlement")
            .connect_timeout(Duration::from_secs(5))
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(16)
            .build()?;
        Ok(Self { http, base_url })
    }

    pub async fn verify_credentials(
        &self,
        credentials: &BinanceCredentials,
        expected_uid: &str,
    ) -> Result<VerificationResult, BinanceApiError> {
        let restrictions: ApiRestrictions = self
            .signed_get(
                "/sapi/v1/account/apiRestrictions",
                BTreeMap::new(),
                credentials,
            )
            .await?;
        if !restrictions.enable_reading {
            return Err(BinanceApiError::new("READ_PERMISSION_REQUIRED"));
        }
        if restrictions.has_dangerous_permissions() {
            return Err(BinanceApiError::new("DANGEROUS_PERMISSION_ENABLED"));
        }
        let end_ms = Utc::now().timestamp_millis();
        let start_ms = end_ms.saturating_sub(30 * 24 * 60 * 60 * 1_000);
        let transactions = self
            .pay_transactions(credentials, start_ms, end_ms, 100)
            .await?;
        let observed_account_uids = transactions
            .iter()
            .filter_map(|transaction| {
                parse_decimal_units(&transaction.amount, 10_000)
                    .ok()
                    .and_then(|amount| match amount.cmp(&0) {
                        std::cmp::Ordering::Greater => transaction.receiver_info.uid(),
                        std::cmp::Ordering::Less => transaction.payer_info.uid(),
                        std::cmp::Ordering::Equal => None,
                    })
            })
            .collect::<Vec<_>>();
        if observed_account_uids
            .iter()
            .any(|observed_uid| observed_uid != expected_uid)
        {
            return Err(BinanceApiError::new("RECEIVER_UID_MISMATCH"));
        }
        Ok(VerificationResult {
            reading_enabled: true,
            dangerous_permissions_disabled: true,
            uid_confirmed: !observed_account_uids.is_empty(),
        })
    }

    pub async fn pay_transactions(
        &self,
        credentials: &BinanceCredentials,
        start_ms: i64,
        end_ms: i64,
        limit: usize,
    ) -> Result<Vec<BinancePayTransaction>, BinanceApiError> {
        let mut parameters = BTreeMap::new();
        parameters.insert("endTime", end_ms.to_string());
        parameters.insert("limit", limit.clamp(1, 100).to_string());
        parameters.insert("startTime", start_ms.to_string());
        let envelope: PayTransactionsEnvelope = self
            .signed_get("/sapi/v1/pay/transactions", parameters, credentials)
            .await?;
        if !envelope.success {
            return Err(BinanceApiError::new("BINANCE_PAY_QUERY_REJECTED"));
        }
        if envelope.data.iter().any(|transaction| {
            let transaction_id = transaction.transaction_id.trim();
            transaction_id.is_empty() || transaction_id.len() > 256
        }) {
            return Err(BinanceApiError::new("BINANCE_RESPONSE_INVALID"));
        }
        Ok(envelope.data)
    }

    async fn signed_get<T: DeserializeOwned>(
        &self,
        path: &str,
        mut parameters: BTreeMap<&str, String>,
        credentials: &BinanceCredentials,
    ) -> Result<T, BinanceApiError> {
        parameters.insert("recvWindow", "10000".into());
        parameters.insert("timestamp", Utc::now().timestamp_millis().to_string());
        let query = {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            for (key, value) in parameters {
                serializer.append_pair(key, &value);
            }
            serializer.finish()
        };
        let mut mac = Hmac::<Sha256>::new_from_slice(credentials.api_secret.as_bytes())
            .map_err(|_| BinanceApiError::new("SIGNATURE_SETUP_FAILED"))?;
        mac.update(query.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());
        let mut url = self
            .base_url
            .join(path)
            .map_err(|_| BinanceApiError::new("BINANCE_URL_INVALID"))?;
        url.set_query(Some(&format!("{query}&signature={signature}")));
        let response = self
            .http
            .get(url)
            .header("X-MBX-APIKEY", &credentials.api_key)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    BinanceApiError::new("BINANCE_TIMEOUT")
                } else {
                    BinanceApiError::new("BINANCE_NETWORK_ERROR")
                }
            })?;
        let status = response.status();
        let retry_after_secs = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_retry_after);
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(BinanceApiError::new("BINANCE_RESPONSE_TOO_LARGE"));
        }
        let mut bytes = Vec::new();
        let mut body = response.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|_| BinanceApiError::new("BINANCE_RESPONSE_READ_FAILED"))?;
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(BinanceApiError::new("BINANCE_RESPONSE_TOO_LARGE"));
            }
            bytes.extend_from_slice(&chunk);
        }
        if status != StatusCode::OK {
            let upstream_code = serde_json::from_slice::<BinanceErrorEnvelope>(&bytes)
                .ok()
                .map(|error| error.code);
            let code = match (status.as_u16(), upstream_code) {
                (418, _) => "BINANCE_IP_BANNED",
                (429, _) => "BINANCE_RATE_LIMITED",
                (401 | 403, _) => "BINANCE_CREDENTIALS_REJECTED",
                (_, Some(-1003)) => "BINANCE_RATE_LIMITED",
                (_, Some(-1021)) => "BINANCE_CLOCK_SKEW",
                (_, Some(-1022 | -2014 | -2015)) => "BINANCE_CREDENTIALS_REJECTED",
                (500..=599, _) => "BINANCE_UPSTREAM_ERROR",
                _ => "BINANCE_REQUEST_REJECTED",
            };
            return Err(BinanceApiError {
                code: code.into(),
                retry_after_secs,
            });
        }
        serde_json::from_slice(&bytes).map_err(|_| BinanceApiError::new("BINANCE_RESPONSE_INVALID"))
    }
}

impl BinanceApiError {
    fn new(code: &str) -> Self {
        Self {
            code: code.into(),
            retry_after_secs: None,
        }
    }
}

fn parse_retry_after(value: &str) -> Option<u64> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(seconds);
    }
    let deadline = chrono::DateTime::parse_from_rfc2822(value.trim())
        .ok()?
        .with_timezone(&Utc);
    u64::try_from(
        deadline
            .signed_duration_since(Utc::now())
            .num_seconds()
            .max(0),
    )
    .ok()
}

pub fn parse_decimal_units(value: &str, scale: i64) -> Result<i64, ()> {
    let value = value.trim();
    if value.is_empty() || scale <= 0 {
        return Err(());
    }
    let negative = value.starts_with('-');
    let unsigned = value.strip_prefix(['-', '+']).unwrap_or(value);
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(());
    }
    let decimals = scale.ilog10() as usize;
    let fraction = fraction.trim_end_matches('0');
    if 10_i64.pow(decimals as u32) != scale || fraction.len() > decimals {
        return Err(());
    }
    let whole = whole.parse::<i64>().map_err(|_| ())?;
    let mut padded = fraction.to_string();
    padded.extend(std::iter::repeat_n(
        '0',
        decimals.saturating_sub(fraction.len()),
    ));
    let fraction = if padded.is_empty() {
        0
    } else {
        padded.parse::<i64>().map_err(|_| ())?
    };
    let result = whole
        .checked_mul(scale)
        .and_then(|whole| whole.checked_add(fraction))
        .ok_or(())?;
    if negative {
        result.checked_neg().ok_or(())
    } else {
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    type MockResponse = (u16, Vec<(&'static str, &'static str)>, &'static str, u64);

    async fn mock_server(
        responses: Vec<MockResponse>,
    ) -> (Url, tokio::sync::oneshot::Receiver<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock Binance server");
        let address = listener.local_addr().expect("mock server address");
        let (request_sender, request_receiver) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let mut requests = Vec::new();
            for (status, headers, body, delay_ms) in responses {
                let (mut socket, _) = listener.accept().await.expect("accept mock request");
                let mut buffer = vec![0_u8; 16 * 1024];
                let read = socket.read(&mut buffer).await.expect("read mock request");
                requests.push(String::from_utf8_lossy(&buffer[..read]).to_string());
                if delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
                let reason = match status {
                    200 => "OK",
                    418 => "I'm a teapot",
                    429 => "Too Many Requests",
                    503 => "Service Unavailable",
                    _ => "Error",
                };
                let extra_headers = headers
                    .into_iter()
                    .map(|(name, value)| format!("{name}: {value}\r\n"))
                    .collect::<String>();
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: application/json\r\n{extra_headers}Connection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
            let _ = request_sender.send(requests);
        });
        (
            Url::parse(&format!("http://{address}")).expect("mock server URL"),
            request_receiver,
        )
    }

    fn credentials() -> BinanceCredentials {
        BinanceCredentials {
            api_key: "0123456789abcdef0123456789abcdef".into(),
            api_secret: "fedcba9876543210fedcba9876543210".into(),
        }
    }

    #[test]
    fn decimal_amounts_are_parsed_without_floats() {
        assert_eq!(parse_decimal_units("10.0037", 10_000), Ok(100_037));
        assert_eq!(parse_decimal_units("10.00370000", 10_000), Ok(100_037));
        assert_eq!(parse_decimal_units("-1.2", 10_000), Ok(-12_000));
        assert_eq!(parse_decimal_units("1.00001", 10_000), Err(()));
        assert_eq!(parse_decimal_units("NaN", 10_000), Err(()));
        assert_eq!(parse_retry_after("120"), Some(120));
        assert!(parse_retry_after("not-a-retry-date").is_none());
    }

    #[test]
    fn api_restrictions_require_every_security_relevant_field() {
        let valid = serde_json::json!({
            "enableReading": true,
            "enableWithdrawals": false,
            "enableInternalTransfer": false,
            "permitsUniversalTransfer": false,
            "enableSpotAndMarginTrading": false,
            "enableMargin": false,
            "enableFutures": false,
            "enablePortfolioMarginTrading": false,
            "enableVanillaOptions": false,
            "enableFixApiTrade": false,
            "enableFixReadOnly": true
        });
        let parsed = serde_json::from_value::<ApiRestrictions>(valid.clone())
            .expect("parse safe restriction fixture");
        assert!(!parsed.has_dangerous_permissions());

        let mut fix_trading = valid.clone();
        fix_trading
            .as_object_mut()
            .expect("restriction fixture object")
            .insert("enableFixApiTrade".into(), true.into());
        assert!(
            serde_json::from_value::<ApiRestrictions>(fix_trading)
                .expect("parse FIX permission")
                .has_dangerous_permissions()
        );

        let mut future_dangerous = valid.clone();
        future_dangerous
            .as_object_mut()
            .expect("restriction fixture object")
            .insert("enableBroker".into(), true.into());
        assert!(
            serde_json::from_value::<ApiRestrictions>(future_dangerous)
                .expect("parse future permission")
                .has_dangerous_permissions()
        );

        let mut incomplete = valid;
        incomplete
            .as_object_mut()
            .expect("restriction fixture object")
            .remove("enableWithdrawals");
        assert!(serde_json::from_value::<ApiRestrictions>(incomplete).is_err());
    }

    #[test]
    fn pay_transaction_envelope_is_fail_closed() {
        let valid = serde_json::json!({ "success": true, "data": [] });
        assert!(serde_json::from_value::<PayTransactionsEnvelope>(valid).is_ok());
        assert!(
            serde_json::from_value::<PayTransactionsEnvelope>(serde_json::json!({ "data": [] }))
                .is_err()
        );
        assert!(
            serde_json::from_value::<PayTransactionsEnvelope>(serde_json::json!({
                "success": true
            }))
            .is_err()
        );
    }

    #[tokio::test]
    async fn pay_transactions_reject_missing_transaction_identity() {
        let body = r#"{
            "success": true,
            "data": [{
                "transactionId": "",
                "transactionTime": 1788325910559,
                "amount": "1.00000000",
                "currency": "USDT"
            }]
        }"#;
        let (base_url, _) = mock_server(vec![(200, vec![], body, 0)]).await;
        let client = BinanceClient::new(base_url).expect("Binance client");
        let error = client
            .pay_transactions(&credentials(), 1, 2, 100)
            .await
            .expect_err("transaction identity is required for safe deduplication");
        assert_eq!(error.code, "BINANCE_RESPONSE_INVALID");
    }

    #[tokio::test]
    async fn signed_request_sends_key_and_required_signature_parameters() {
        let (base_url, requests) = mock_server(vec![(200, vec![], "{}", 0)]).await;
        let client = BinanceClient::new(base_url).expect("Binance client");
        let response: serde_json::Value = client
            .signed_get("/signed", BTreeMap::new(), &credentials())
            .await
            .expect("signed request");
        assert_eq!(response, serde_json::json!({}));
        let requests = requests.await.expect("captured request");
        let request = &requests[0];
        assert!(request.starts_with("GET /signed?"));
        assert!(request.contains("recvWindow=10000"));
        assert!(request.contains("timestamp="));
        assert!(request.contains("signature="));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("x-mbx-apikey: 0123456789abcdef0123456789abcdef")
        );
        assert!(!request.contains("fedcba9876543210fedcba9876543210"));
    }

    #[tokio::test]
    async fn upstream_statuses_and_malformed_json_map_to_stable_codes() {
        for (status, headers, body, expected, retry_after) in [
            (
                429,
                vec![("Retry-After", "7")],
                "{}",
                "BINANCE_RATE_LIMITED",
                Some(7),
            ),
            (
                418,
                vec![],
                r#"{"code":-1003,"msg":"IP banned"}"#,
                "BINANCE_IP_BANNED",
                None,
            ),
            (503, vec![], "{}", "BINANCE_UPSTREAM_ERROR", None),
            (
                400,
                vec![],
                r#"{"code":-2015,"msg":"invalid credentials"}"#,
                "BINANCE_CREDENTIALS_REJECTED",
                None,
            ),
            (
                400,
                vec![],
                r#"{"code":-1021,"msg":"clock skew"}"#,
                "BINANCE_CLOCK_SKEW",
                None,
            ),
            (
                302,
                vec![("Location", "http://127.0.0.1:1/leak")],
                "{}",
                "BINANCE_REQUEST_REJECTED",
                None,
            ),
            (200, vec![], "not-json", "BINANCE_RESPONSE_INVALID", None),
        ] {
            let (base_url, _) = mock_server(vec![(status, headers, body, 0)]).await;
            let client = BinanceClient::new(base_url).expect("Binance client");
            let error = client
                .signed_get::<serde_json::Value>("/signed", BTreeMap::new(), &credentials())
                .await
                .expect_err("upstream response must fail");
            assert_eq!(error.code, expected);
            assert_eq!(error.retry_after_secs, retry_after);
        }
    }

    #[tokio::test]
    async fn request_timeout_maps_to_stable_code() {
        let (base_url, _) = mock_server(vec![(200, vec![], "{}", 200)]).await;
        let client = BinanceClient::with_timeout(base_url, Duration::from_millis(30))
            .expect("Binance client");
        let error = client
            .signed_get::<serde_json::Value>("/slow", BTreeMap::new(), &credentials())
            .await
            .expect_err("slow response must time out");
        assert_eq!(error.code, "BINANCE_TIMEOUT");
    }

    #[tokio::test]
    async fn dangerous_permissions_are_rejected_before_transaction_query() {
        let restrictions = serde_json::json!({
            "enableReading": true,
            "enableWithdrawals": false,
            "enableInternalTransfer": false,
            "permitsUniversalTransfer": false,
            "enableSpotAndMarginTrading": false,
            "enableMargin": false,
            "enableFutures": false,
            "enablePortfolioMarginTrading": true,
            "enableVanillaOptions": false,
            "enableFixApiTrade": false,
            "enableFixReadOnly": true
        })
        .to_string();
        let body: &'static str = Box::leak(restrictions.into_boxed_str());
        let (base_url, requests) = mock_server(vec![(200, vec![], body, 0)]).await;
        let client = BinanceClient::new(base_url).expect("Binance client");
        let error = client
            .verify_credentials(&credentials(), "123456789")
            .await
            .expect_err("dangerous permissions must fail");
        assert_eq!(error.code, "DANGEROUS_PERMISSION_ENABLED");
        assert_eq!(requests.await.expect("captured requests").len(), 1);
    }

    #[tokio::test]
    async fn explicit_outgoing_payer_uid_confirms_the_credential_account() {
        let restrictions = serde_json::json!({
            "enableReading": true,
            "enableWithdrawals": false,
            "enableInternalTransfer": false,
            "permitsUniversalTransfer": false,
            "enableSpotAndMarginTrading": false,
            "enableMargin": false,
            "enableFutures": false,
            "enablePortfolioMarginTrading": false,
            "enableVanillaOptions": false,
            "enableFixApiTrade": false,
            "enableFixReadOnly": true
        })
        .to_string();
        let restrictions: &'static str = Box::leak(restrictions.into_boxed_str());
        let transactions = r#"{
            "success": true,
            "data": [{
                "orderType": "C2C",
                "transactionId": "P_ACCOUNT_PROOF",
                "transactionTime": 1788325910559,
                "amount": "-0.10000000",
                "currency": "USDT",
                "payerInfo": {"binanceId": "123456789"}
            }]
        }"#;
        let (base_url, requests) = mock_server(vec![
            (200, vec![], restrictions, 0),
            (200, vec![], transactions, 0),
        ])
        .await;
        let client = BinanceClient::new(base_url).expect("Binance client");
        let verification = client
            .verify_credentials(&credentials(), "123456789")
            .await
            .expect("explicit payer UID proves the signed account");
        assert!(verification.uid_confirmed);
        assert_eq!(requests.await.expect("captured requests").len(), 2);
    }
}
