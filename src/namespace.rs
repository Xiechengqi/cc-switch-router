use sha2::{Digest, Sha256};

pub const PROTOCOL_EPOCH: &str = "namespace-flat-1";
pub const CLIENT_PREFIX_MIN_LEN: usize = 3;
pub const CLIENT_PREFIX_MAX_LEN: usize = 7;
pub const CLIENT_FINGERPRINT_LEN: usize = 20;
pub const SHARE_SLUG_MIN_LEN: usize = 3;
pub const SHARE_SLUG_MAX_LEN: usize = 32;
pub const MARKET_SLUG_MIN_LEN: usize = 3;
pub const MARKET_SLUG_MAX_LEN: usize = 32;
pub const DNS_LABEL_MAX_LEN: usize = 63;

const RESERVED_LABELS: &[&str] = &["admin", "api", "cdn-cgi", "router", "www"];
const BASE32_LOWER: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicHostKind {
    Client,
    Share,
    Market,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPublicLabel {
    pub kind: PublicHostKind,
    pub label: String,
    pub client_key: Option<String>,
    pub share_slug: Option<String>,
    pub market_slug: Option<String>,
}

pub fn client_fingerprint(public_key: &[u8]) -> String {
    let digest = Sha256::digest(public_key);
    encode_first_100_bits(&digest)
}

pub fn build_client_key(prefix: &str, public_key: &[u8]) -> Result<String, &'static str> {
    let prefix = normalize_client_prefix(prefix)?;
    if public_key.len() != 32 {
        return Err("client public key must contain exactly 32 bytes");
    }
    Ok(format!("{prefix}-{}", client_fingerprint(public_key)))
}

pub fn normalize_client_prefix(value: &str) -> Result<String, &'static str> {
    let value = normalize_ascii_label(value)?;
    if !(CLIENT_PREFIX_MIN_LEN..=CLIENT_PREFIX_MAX_LEN).contains(&value.len()) {
        return Err("client prefix must be 3-7 characters");
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return Err("client prefix may only contain lowercase letters and digits");
    }
    if !value.as_bytes()[0].is_ascii_lowercase() {
        return Err("client prefix must start with a lowercase letter");
    }
    ensure_not_reserved(&value)?;
    Ok(value)
}

pub fn normalize_client_key(value: &str) -> Result<String, &'static str> {
    let value = normalize_ascii_label(value)?;
    let Some((prefix, fingerprint)) = value.split_once('-') else {
        return Err("client key must contain its fingerprint separator");
    };
    if fingerprint.contains('-') {
        return Err("client key contains an unexpected separator");
    }
    normalize_client_prefix(prefix)?;
    if fingerprint.len() != CLIENT_FINGERPRINT_LEN
        || !fingerprint
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'2'..=b'7'))
    {
        return Err("client key fingerprint is invalid");
    }
    Ok(value)
}

pub fn normalize_share_slug(value: &str) -> Result<String, &'static str> {
    let value = normalize_limited_slug(value, SHARE_SLUG_MIN_LEN, SHARE_SLUG_MAX_LEN)?;
    if value.contains("--") {
        return Err("share slug cannot contain '--'");
    }
    ensure_not_reserved(&value)?;
    Ok(value)
}

pub fn normalize_market_slug(value: &str) -> Result<String, &'static str> {
    let value = normalize_limited_slug(value, MARKET_SLUG_MIN_LEN, MARKET_SLUG_MAX_LEN)?;
    if value.contains("--") {
        return Err("market slug cannot contain '--'");
    }
    if normalize_client_key(&value).is_ok() {
        return Err("market slug cannot match the client-key grammar");
    }
    ensure_not_reserved(&value)?;
    Ok(value)
}

pub fn build_share_label(share_slug: &str, client_key: &str) -> Result<String, &'static str> {
    let share_slug = normalize_share_slug(share_slug)?;
    let client_key = normalize_client_key(client_key)?;
    let label = format!("{share_slug}--{client_key}");
    if label.len() > DNS_LABEL_MAX_LEN {
        return Err("share host label exceeds the DNS 63-byte limit");
    }
    Ok(label)
}

pub fn parse_public_label(value: &str) -> Result<ParsedPublicLabel, &'static str> {
    let label = normalize_ascii_label(value)?;
    if label.contains("--") {
        let mut parts = label.split("--");
        let share_slug = parts.next().unwrap_or_default();
        let client_key = parts.next().unwrap_or_default();
        if parts.next().is_some() {
            return Err("share host label contains more than one '--' separator");
        }
        let share_slug = normalize_share_slug(share_slug)?;
        let client_key = normalize_client_key(client_key)?;
        return Ok(ParsedPublicLabel {
            kind: PublicHostKind::Share,
            label,
            client_key: Some(client_key),
            share_slug: Some(share_slug),
            market_slug: None,
        });
    }
    if let Ok(client_key) = normalize_client_key(&label) {
        return Ok(ParsedPublicLabel {
            kind: PublicHostKind::Client,
            label,
            client_key: Some(client_key),
            share_slug: None,
            market_slug: None,
        });
    }
    let market_slug = normalize_market_slug(&label)?;
    Ok(ParsedPublicLabel {
        kind: PublicHostKind::Market,
        label,
        client_key: None,
        share_slug: None,
        market_slug: Some(market_slug),
    })
}

fn normalize_limited_slug(
    value: &str,
    min_len: usize,
    max_len: usize,
) -> Result<String, &'static str> {
    let value = normalize_ascii_label(value)?;
    if !(min_len..=max_len).contains(&value.len()) {
        return Err("slug length is invalid");
    }
    if value.starts_with('-') || value.ends_with('-') {
        return Err("slug cannot start or end with '-'");
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err("slug may only contain lowercase letters, digits, and '-'");
    }
    Ok(value)
}

fn normalize_ascii_label(value: &str) -> Result<String, &'static str> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() || value.len() > DNS_LABEL_MAX_LEN || !value.is_ascii() {
        return Err("invalid DNS label");
    }
    Ok(value)
}

fn ensure_not_reserved(value: &str) -> Result<(), &'static str> {
    if RESERVED_LABELS.contains(&value) {
        return Err("host label is reserved");
    }
    Ok(())
}

fn encode_first_100_bits(digest: &[u8]) -> String {
    let mut output = String::with_capacity(CLIENT_FINGERPRINT_LEN);
    let mut bit_offset = 0usize;
    for _ in 0..CLIENT_FINGERPRINT_LEN {
        let mut value = 0u8;
        for _ in 0..5 {
            let byte = digest[bit_offset / 8];
            let bit = (byte >> (7 - (bit_offset % 8))) & 1;
            value = (value << 1) | bit;
            bit_offset += 1;
        }
        output.push(BASE32_LOWER[value as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_and_uses_exactly_100_bits() {
        assert_eq!(client_fingerprint(b"public-key"), "iosg6hiidutqcmhceefb");
        assert_eq!(client_fingerprint(b"public-key").len(), 20);
    }

    #[test]
    fn builds_the_longest_supported_share_label_with_room_to_spare() {
        let client = build_client_key("seventh", &[7; 32]).unwrap();
        let label = build_share_label(&"s".repeat(32), &client).unwrap();
        assert_eq!(label.len(), 62);
        assert_eq!(
            parse_public_label(&label).unwrap().kind,
            PublicHostKind::Share
        );
    }

    #[test]
    fn public_label_types_are_unambiguous() {
        let client = build_client_key("alpha", &[7; 32]).unwrap();
        let share = build_share_label("team-one", &client).unwrap();
        assert_eq!(
            parse_public_label(&client).unwrap().kind,
            PublicHostKind::Client
        );
        assert_eq!(
            parse_public_label(&share).unwrap().kind,
            PublicHostKind::Share
        );
        assert_eq!(
            parse_public_label("public-market").unwrap().kind,
            PublicHostKind::Market
        );
    }

    #[test]
    fn rejects_ambiguous_or_reserved_labels() {
        assert!(normalize_share_slug("a--b").is_err());
        let client = build_client_key("alpha", &[7; 32]).unwrap();
        assert!(normalize_market_slug(&client).is_err());
        assert!(normalize_market_slug("www").is_err());
        assert!(parse_public_label("share--bad-client").is_err());
    }
}
