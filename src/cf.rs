//! Cloudflare peer trust check.
//!
//! When this router sits behind Cloudflare (the production deployment), the headers
//! `cf-connecting-ip` and `cf-ipcountry` reflect the real end-user. Without verifying
//! the connecting peer is in fact a Cloudflare edge, anyone hitting the origin
//! directly could spoof those headers. This module owns that gate.
//!
//! Loopback / RFC1918 peers (LAN, dev) are also treated as trusted so local testing
//! and direct-to-origin operators are not surprised by missing geo data.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Cloudflare published IPv4 ranges. Source: <https://www.cloudflare.com/ips-v4>.
const CF_IPV4_CIDRS: &[(Ipv4Addr, u8)] = &[
    (Ipv4Addr::new(173, 245, 48, 0), 20),
    (Ipv4Addr::new(103, 21, 244, 0), 22),
    (Ipv4Addr::new(103, 22, 200, 0), 22),
    (Ipv4Addr::new(103, 31, 4, 0), 22),
    (Ipv4Addr::new(141, 101, 64, 0), 18),
    (Ipv4Addr::new(108, 162, 192, 0), 18),
    (Ipv4Addr::new(190, 93, 240, 0), 20),
    (Ipv4Addr::new(188, 114, 96, 0), 20),
    (Ipv4Addr::new(197, 234, 240, 0), 22),
    (Ipv4Addr::new(198, 41, 128, 0), 17),
    (Ipv4Addr::new(162, 158, 0, 0), 15),
    (Ipv4Addr::new(104, 16, 0, 0), 13),
    (Ipv4Addr::new(104, 24, 0, 0), 14),
    (Ipv4Addr::new(172, 64, 0, 0), 13),
    (Ipv4Addr::new(131, 0, 72, 0), 22),
];

/// Cloudflare published IPv6 ranges. Source: <https://www.cloudflare.com/ips-v6>.
const CF_IPV6_CIDRS: &[(Ipv6Addr, u8)] = &[
    (Ipv6Addr::new(0x2400, 0xcb00, 0, 0, 0, 0, 0, 0), 32),
    (Ipv6Addr::new(0x2606, 0x4700, 0, 0, 0, 0, 0, 0), 32),
    (Ipv6Addr::new(0x2803, 0xf800, 0, 0, 0, 0, 0, 0), 32),
    (Ipv6Addr::new(0x2405, 0xb500, 0, 0, 0, 0, 0, 0), 32),
    (Ipv6Addr::new(0x2405, 0x8100, 0, 0, 0, 0, 0, 0), 32),
    (Ipv6Addr::new(0x2a06, 0x98c0, 0, 0, 0, 0, 0, 0), 29),
    (Ipv6Addr::new(0x2c0f, 0xf248, 0, 0, 0, 0, 0, 0), 32),
];

fn ipv4_in_cidr(ip: Ipv4Addr, base: Ipv4Addr, prefix: u8) -> bool {
    if prefix == 0 {
        return true;
    }
    let mask = (!0u32).checked_shl(32 - u32::from(prefix)).unwrap_or(0);
    let ip_u32 = u32::from(ip);
    let base_u32 = u32::from(base);
    (ip_u32 & mask) == (base_u32 & mask)
}

fn ipv6_in_cidr(ip: Ipv6Addr, base: Ipv6Addr, prefix: u8) -> bool {
    if prefix == 0 {
        return true;
    }
    let mask = (!0u128).checked_shl(128 - u32::from(prefix)).unwrap_or(0);
    let ip_u128 = u128::from(ip);
    let base_u128 = u128::from(base);
    (ip_u128 & mask) == (base_u128 & mask)
}

/// Returns `true` if `ip` is allowed to set CF spoof-prone headers
/// (`cf-connecting-ip`, `cf-ipcountry`, …).
///
/// The peer is trusted when it is a Cloudflare edge OR a loopback/private host (the
/// latter keeps local development from failing the check).
pub fn is_cloudflare_peer(ip: IpAddr) -> bool {
    if ip.is_loopback() {
        return true;
    }
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_private() || v4.is_link_local() || v4.is_unspecified() {
                return true;
            }
            CF_IPV4_CIDRS
                .iter()
                .any(|(base, prefix)| ipv4_in_cidr(v4, *base, *prefix))
        }
        IpAddr::V6(v6) => {
            if v6.is_unspecified() {
                return true;
            }
            // ULA fc00::/7
            let seg = v6.segments();
            if (seg[0] & 0xfe00) == 0xfc00 {
                return true;
            }
            CF_IPV6_CIDRS
                .iter()
                .any(|(base, prefix)| ipv6_in_cidr(v6, *base, *prefix))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloudflare_v4_inside() {
        assert!(is_cloudflare_peer("104.16.123.45".parse().unwrap()));
        assert!(is_cloudflare_peer("172.68.10.10".parse().unwrap()));
    }

    #[test]
    fn arbitrary_v4_outside() {
        assert!(!is_cloudflare_peer("8.8.8.8".parse().unwrap()));
        assert!(!is_cloudflare_peer("1.2.3.4".parse().unwrap()));
    }

    #[test]
    fn dev_friendly_ranges_pass() {
        assert!(is_cloudflare_peer("127.0.0.1".parse().unwrap()));
        assert!(is_cloudflare_peer("::1".parse().unwrap()));
        assert!(is_cloudflare_peer("10.0.0.5".parse().unwrap()));
        assert!(is_cloudflare_peer("192.168.1.1".parse().unwrap()));
    }

    #[test]
    fn cloudflare_v6_inside() {
        assert!(is_cloudflare_peer("2606:4700::1".parse().unwrap()));
    }
}
