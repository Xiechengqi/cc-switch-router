pub const PROTOCOL_EPOCH: &str = "namespace-flat-1";
pub const PUBLIC_SLUG_MIN_LEN: usize = 6;
pub const PUBLIC_SLUG_MAX_LEN: usize = 30;
pub const DNS_LABEL_MAX_LEN: usize = 63;

const RESERVED_LABELS: &[&str] = &["admin", "api", "cdn-cgi", "router", "www"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicHostKind {
    Client,
    Share,
    Market,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedShareLabel {
    pub label: String,
    pub client_subdomain: String,
    pub share_slug: String,
}

pub fn normalize_client_subdomain(value: &str) -> Result<String, &'static str> {
    normalize_public_slug(value, "client subdomain")
}

pub fn normalize_share_slug(value: &str) -> Result<String, &'static str> {
    normalize_public_slug(value, "share slug")
}

pub fn normalize_market_slug(value: &str) -> Result<String, &'static str> {
    normalize_public_slug(value, "market slug")
}

#[cfg(test)]
pub fn build_share_label(share_slug: &str, client_subdomain: &str) -> Result<String, &'static str> {
    let share_slug = normalize_share_slug(share_slug)?;
    let client_subdomain = normalize_client_subdomain(client_subdomain)?;
    let label = format!("{share_slug}--{client_subdomain}");
    if label.len() > DNS_LABEL_MAX_LEN {
        return Err("share host label exceeds the DNS 63-byte limit");
    }
    Ok(label)
}

pub fn parse_share_label(value: &str) -> Result<ParsedShareLabel, &'static str> {
    let label = normalize_ascii_label(value)?;
    let mut parts = label.split("--");
    let share_slug = parts.next().unwrap_or_default().to_string();
    let client_subdomain = parts.next().unwrap_or_default().to_string();
    if share_slug.is_empty() || client_subdomain.is_empty() || parts.next().is_some() {
        return Err("share host label must contain exactly one '--' separator");
    }
    Ok(ParsedShareLabel {
        label,
        share_slug: normalize_share_slug(&share_slug)?,
        client_subdomain: normalize_client_subdomain(&client_subdomain)?,
    })
}

fn normalize_public_slug(value: &str, kind: &'static str) -> Result<String, &'static str> {
    let value = normalize_ascii_label(value)?;
    if !(PUBLIC_SLUG_MIN_LEN..=PUBLIC_SLUG_MAX_LEN).contains(&value.len()) {
        return Err(match kind {
            "client subdomain" => "client subdomain must be 6-30 characters",
            "share slug" => "share slug must be 6-30 characters",
            _ => "market slug must be 6-30 characters",
        });
    }
    if value.contains("--") {
        return Err("public slug cannot contain '--'");
    }
    if !value.as_bytes()[0].is_ascii_lowercase() || value.ends_with('-') {
        return Err("public slug must start with a letter and cannot end with '-'");
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err("public slug may only contain lowercase letters, digits, and '-'");
    }
    if RESERVED_LABELS.contains(&value.as_str()) {
        return Err("host label is reserved");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_parses_flat_share_label() {
        let label = build_share_label("codex-pro", "edge-main").unwrap();
        assert_eq!(label, "codex-pro--edge-main");
        let parsed = parse_share_label(&label).unwrap();
        assert_eq!(parsed.share_slug, "codex-pro");
        assert_eq!(parsed.client_subdomain, "edge-main");
    }

    #[test]
    fn longest_supported_share_label_is_62_bytes() {
        let label = build_share_label(&"s".repeat(30), &"c".repeat(30)).unwrap();
        assert_eq!(label.len(), 62);
    }

    #[test]
    fn rejects_ambiguous_or_invalid_slugs() {
        assert!(normalize_share_slug("share--bad").is_err());
        assert!(normalize_client_subdomain("short").is_err());
        assert!(normalize_client_subdomain("-edge-main").is_err());
        assert!(parse_share_label("shareonly").is_err());
        assert!(parse_share_label("share--client--extra").is_err());
    }
}
