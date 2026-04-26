use std::net::SocketAddr;

use axum::http::HeaderMap;

use crate::models::ClientMetadata;

pub fn extract_client_metadata(headers: &HeaderMap, addr: SocketAddr) -> ClientMetadata {
    // Only honor Cloudflare-spoof-prone headers when the connecting peer is in fact a
    // Cloudflare edge (or a loopback/private host for dev). Otherwise an attacker
    // hitting the origin directly could forge `cf-connecting-ip` / `cf-ipcountry`.
    let cf_trusted = crate::cf::is_cloudflare_peer(addr.ip());

    let ip = if cf_trusted {
        headers
            .get("cf-connecting-ip")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .or_else(|| {
                headers
                    .get("x-forwarded-for")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.split(',').next())
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(str::to_string)
            })
            .or_else(|| Some(addr.ip().to_string()))
    } else {
        Some(addr.ip().to_string())
    };

    let country_code = if cf_trusted {
        headers
            .get("cf-ipcountry")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|v| v.len() == 2 && *v != "XX" && *v != "T1")
            .map(|v| v.to_ascii_uppercase())
    } else {
        None
    };

    ClientMetadata { ip, country_code }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn trusted_cloudflare_peer_uses_forwarded_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("cf-connecting-ip", HeaderValue::from_static("203.0.113.42"));
        headers.insert("cf-ipcountry", HeaderValue::from_static("us"));
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(173, 245, 48, 5)), 443);

        let metadata = extract_client_metadata(&headers, addr);

        assert_eq!(metadata.ip.as_deref(), Some("203.0.113.42"));
        assert_eq!(metadata.country_code.as_deref(), Some("US"));
    }

    #[test]
    fn untrusted_peer_ignores_spoofable_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("cf-connecting-ip", HeaderValue::from_static("203.0.113.42"));
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7)), 443);

        let metadata = extract_client_metadata(&headers, addr);

        assert_eq!(metadata.ip.as_deref(), Some("198.51.100.7"));
        assert_eq!(metadata.country_code, None);
    }
}
