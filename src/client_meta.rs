use std::net::{IpAddr, SocketAddr};

use axum::http::HeaderMap;

use crate::models::ClientMetadata;

pub fn extract_client_metadata(headers: &HeaderMap, addr: SocketAddr) -> ClientMetadata {
    // Only honor Cloudflare-spoof-prone headers when the connecting peer is in fact a
    // Cloudflare edge. Otherwise an attacker hitting the origin directly could
    // forge `cf-connecting-ip` / `cf-ipcountry` and rotate rate-limit scopes.
    let cf_trusted = crate::cf::is_cloudflare_peer(addr.ip());

    let ip = if cf_trusted {
        headers
            .get("cf-connecting-ip")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .and_then(parse_ip)
            .or_else(|| Some(addr.ip().to_string()))
    } else {
        Some(addr.ip().to_string())
    };

    let country_code = if cf_trusted {
        headers
            .get("cf-ipcountry")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .map(|v| v.to_ascii_uppercase())
            .filter(|value| {
                value.len() == 2
                    && value.bytes().all(|byte| byte.is_ascii_uppercase())
                    && value != "XX"
                    && value != "T1"
            })
    } else {
        None
    };

    ClientMetadata { ip, country_code }
}

fn parse_ip(value: &str) -> Option<String> {
    value.parse::<IpAddr>().ok().map(|ip| ip.to_string())
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

    #[test]
    fn trusted_peer_does_not_fall_back_to_spoofable_x_forwarded_for() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "cf-connecting-ip",
            HeaderValue::from_static("not-an-ip/../../secret"),
        );
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("203.0.113.42, 198.51.100.8"),
        );
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(173, 245, 48, 5)), 443);

        let metadata = extract_client_metadata(&headers, addr);

        assert_eq!(metadata.ip.as_deref(), Some("173.245.48.5"));
    }

    #[test]
    fn trusted_peer_canonicalizes_forwarded_ipv6() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "cf-connecting-ip",
            HeaderValue::from_static("2001:0DB8:0000:0000:0000:0000:0000:0001"),
        );
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(173, 245, 48, 5)), 443);

        let metadata = extract_client_metadata(&headers, addr);

        assert_eq!(metadata.ip.as_deref(), Some("2001:db8::1"));
    }

    #[test]
    fn trusted_peer_normalizes_and_filters_country_sentinels() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(173, 245, 48, 5)), 443);
        for value in ["xx", "t1", "1A"] {
            let mut headers = HeaderMap::new();
            headers.insert("cf-ipcountry", HeaderValue::from_str(value).unwrap());
            assert_eq!(extract_client_metadata(&headers, addr).country_code, None);
        }
    }
}
