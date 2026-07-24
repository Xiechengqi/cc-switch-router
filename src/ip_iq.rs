use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::time::timeout;

use crate::error::AppError;

const IQ_LOOKUP_HOSTS: &[&str] = &["3.0.3.0", "3.0.2.1", "3.0.2.9"];
const IQ_TIMEOUT: Duration = Duration::from_secs(5);
const IQ_RESPONSE_MAX_BYTES: usize = 256 * 1024;

/// Important IP intelligence fields persisted for Client Market hosts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostIpIntel {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk_score: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk_level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<i64>,
    pub country_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latitude: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub longitude: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub as_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classification_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vpn: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hosting: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tor: Option<bool>,
    pub source: String,
}

#[derive(Debug, Deserialize)]
struct IqResponse {
    query: Option<String>,
    ip: Option<String>,
    location: Option<String>,
    score: Option<i64>,
    level: Option<String>,
    risk_score: Option<i64>,
    risk_level: Option<String>,
    confidence: Option<i64>,
    geo: Option<IqGeo>,
    network: Option<IqNetwork>,
    classification: Option<IqClassification>,
}

#[derive(Debug, Deserialize)]
struct IqGeo {
    country: Option<String>,
    country_code: Option<String>,
    region: Option<String>,
    city: Option<String>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    timezone: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IqNetwork {
    asn: Option<String>,
    as_name: Option<String>,
    owner: Option<String>,
    isp: Option<String>,
    network_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IqClassification {
    #[serde(rename = "type")]
    kind: Option<String>,
    proxy: Option<bool>,
    vpn: Option<bool>,
    hosting: Option<bool>,
    tor: Option<bool>,
}

pub async fn lookup_host_ip_intel(ip: &str) -> Result<HostIpIntel, AppError> {
    let trimmed = ip.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest("ip is required".into()));
    }
    let client = reqwest::Client::builder()
        .user_agent("cc-switch-router/client-market")
        .timeout(IQ_TIMEOUT)
        .build()
        .map_err(|e| AppError::Internal(format!("build iq http client failed: {e}")))?;

    let mut last_error = String::from("all iq endpoints failed");
    for host in IQ_LOOKUP_HOSTS {
        match lookup_one(&client, host, trimmed).await {
            Ok(intel) => return Ok(intel),
            Err(error) => {
                last_error = format!("{host}: {error}");
                tracing::warn!(endpoint = %host, ip = %trimmed, error = %error, "iq ip lookup failed");
            }
        }
    }
    Err(AppError::ServiceUnavailable(format!(
        "could not determine host country; retry later ({last_error})"
    )))
}

async fn lookup_one(client: &reqwest::Client, host: &str, ip: &str) -> Result<HostIpIntel, String> {
    let url = format!("http://{host}/iq?ip={ip}");
    let mut response = timeout(IQ_TIMEOUT, client.get(&url).send())
        .await
        .map_err(|_| "request timed out".to_string())?
        .map_err(|e| format!("request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("http {}", response.status()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > IQ_RESPONSE_MAX_BYTES as u64)
    {
        return Err("response too large".into());
    }

    let mut body = Vec::new();
    loop {
        let chunk = timeout(IQ_TIMEOUT, response.chunk())
            .await
            .map_err(|_| "body timed out".to_string())?
            .map_err(|e| format!("read body failed: {e}"))?;
        let Some(chunk) = chunk else { break };
        if body.len() + chunk.len() > IQ_RESPONSE_MAX_BYTES {
            return Err("response too large".into());
        }
        body.extend_from_slice(&chunk);
    }

    let payload: IqResponse =
        serde_json::from_slice(&body).map_err(|e| format!("invalid json: {e}"))?;
    let geo = payload.geo.ok_or_else(|| "missing geo".to_string())?;
    let country_code = geo
        .country_code
        .as_deref()
        .map(str::trim)
        .filter(|code| code.len() == 2 && code.chars().all(|ch| ch.is_ascii_alphabetic()))
        .map(|code| code.to_ascii_uppercase())
        .ok_or_else(|| "missing country_code".to_string())?;

    let network = payload.network;
    let classification = payload.classification;
    Ok(HostIpIntel {
        query: payload.query.unwrap_or_else(|| ip.to_string()),
        ip: payload.ip,
        location: payload.location,
        score: payload.score,
        level: payload.level,
        risk_score: payload.risk_score,
        risk_level: payload.risk_level,
        confidence: payload.confidence,
        country_code,
        country: geo.country,
        region: geo.region,
        city: geo.city,
        latitude: geo.latitude,
        longitude: geo.longitude,
        timezone: geo.timezone,
        asn: network.as_ref().and_then(|value| value.asn.clone()),
        as_name: network.as_ref().and_then(|value| value.as_name.clone()),
        isp: network.as_ref().and_then(|value| value.isp.clone()),
        owner: network.as_ref().and_then(|value| value.owner.clone()),
        network_type: network
            .as_ref()
            .and_then(|value| value.network_type.clone()),
        classification_type: classification.as_ref().and_then(|value| value.kind.clone()),
        proxy: classification.as_ref().and_then(|value| value.proxy),
        vpn: classification.as_ref().and_then(|value| value.vpn),
        hosting: classification.as_ref().and_then(|value| value.hosting),
        tor: classification.as_ref().and_then(|value| value.tor),
        source: host.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_important_iq_fields() {
        let raw = r#"{
            "query": "103.106.228.132",
            "ip": "103.106.228.132",
            "location": "日本 · 東京都",
            "score": 32,
            "level": "风险",
            "risk_score": 68,
            "risk_level": "稍高风险",
            "confidence": 90,
            "geo": {
                "country": "日本",
                "country_code": "JP",
                "region": "東京都",
                "city": "東京都",
                "latitude": 35.6894973,
                "longitude": 139.6923172,
                "timezone": "Asia/Tokyo"
            },
            "network": {
                "asn": "AS136258",
                "as_name": "ONEPROVIDER-AS",
                "owner": "Oneprovider.com",
                "isp": "Brainstorm Network, INC",
                "network_type": "business"
            },
            "classification": {
                "type": "VPN 出口节点",
                "proxy": true,
                "vpn": true,
                "hosting": true,
                "tor": false
            }
        }"#;
        let payload: IqResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(
            payload.geo.as_ref().unwrap().country_code.as_deref(),
            Some("JP")
        );
        assert_eq!(payload.classification.as_ref().unwrap().vpn, Some(true));
    }
}
