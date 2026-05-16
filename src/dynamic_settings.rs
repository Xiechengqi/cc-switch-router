use std::collections::HashSet;
use std::net::IpAddr;

use crate::config::Config;

/// Settings that can change at runtime without restarting the process.
///
/// Everything else (paths, listen addresses, TTLs, all `auth_*` limits,
/// Resend sender + API key, verification service URLs) is read from the
/// immutable boot-time [`Config`] and requires a restart to take effect.
/// The admin UI flags those fields with a `RESTART` badge.
#[derive(Debug, Clone)]
pub struct DynamicSettings {
    pub admin_emails: HashSet<String>,
    pub security: SecuritySettings,
    pub telegram: TelegramSettings,
    pub board: BoardSettings,
}

#[derive(Debug, Clone)]
pub struct SecuritySettings {
    pub ip_blacklist: Vec<IpBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpBlock {
    addr: IpAddr,
    prefix: u8,
}

#[derive(Debug, Clone)]
pub struct TelegramSettings {
    pub bot_token: Option<String>,
    pub chat_id: Option<String>,
    pub topic_id: Option<i64>,
    pub notify_all: bool,
    pub notify_admin: bool,
}

#[derive(Debug, Clone)]
pub struct BoardSettings {
    pub max_len: usize,
    pub guest_per_hour: i64,
    pub user_per_hour: i64,
    pub pin_limit: i64,
    pub guest_self_delete_secs: i64,
}

impl DynamicSettings {
    pub fn from_config(config: &Config) -> Self {
        Self {
            admin_emails: config.admin_emails.clone(),
            security: SecuritySettings {
                ip_blacklist: parse_ip_blacklist(&config.ip_blacklist),
            },
            telegram: TelegramSettings {
                bot_token: config.telegram_bot_token.clone(),
                chat_id: config.telegram_chat_id.clone(),
                topic_id: config.telegram_topic_id,
                notify_all: config.telegram_notify_all,
                notify_admin: config.telegram_notify_admin,
            },
            board: BoardSettings {
                max_len: config.board_max_len,
                guest_per_hour: config.board_guest_per_hour,
                user_per_hour: config.board_user_per_hour,
                pin_limit: config.board_pin_limit,
                guest_self_delete_secs: config.board_guest_self_delete_secs,
            },
        }
    }

    pub fn is_admin(&self, email: &str) -> bool {
        let normalized = email.trim().to_ascii_lowercase();
        !normalized.is_empty() && self.admin_emails.contains(&normalized)
    }

    pub fn is_ip_blacklisted(&self, ip: IpAddr) -> bool {
        self.security
            .ip_blacklist
            .iter()
            .any(|block| block.contains(ip))
    }
}

impl IpBlock {
    pub fn parse(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        let (addr_raw, prefix_raw) = trimmed
            .split_once('/')
            .map(|(addr, prefix)| (addr.trim(), Some(prefix.trim())))
            .unwrap_or((trimmed, None));
        let addr: IpAddr = addr_raw.parse().ok()?;
        let max_prefix = match addr {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        let prefix = match prefix_raw {
            Some(raw) => raw.parse::<u8>().ok()?,
            None => max_prefix,
        };
        if prefix > max_prefix {
            return None;
        }
        Some(Self { addr, prefix })
    }

    pub fn contains(&self, candidate: IpAddr) -> bool {
        match (self.addr, candidate) {
            (IpAddr::V4(block), IpAddr::V4(ip)) => prefix_match(
                u32::from(block) as u128,
                u32::from(ip) as u128,
                self.prefix,
                32,
            ),
            (IpAddr::V6(block), IpAddr::V6(ip)) => {
                prefix_match(u128::from(block), u128::from(ip), self.prefix, 128)
            }
            _ => false,
        }
    }

    pub fn canonical(&self) -> String {
        let max_prefix = match self.addr {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        if self.prefix == max_prefix {
            self.addr.to_string()
        } else {
            format!("{}/{}", self.addr, self.prefix)
        }
    }
}

pub fn parse_ip_blacklist(value: &str) -> Vec<IpBlock> {
    split_ip_blacklist(value)
        .filter_map(IpBlock::parse)
        .collect()
}

pub fn normalize_ip_blacklist(value: &str) -> Option<String> {
    let mut blocks = Vec::new();
    for raw in split_ip_blacklist(value) {
        let block = IpBlock::parse(raw)?;
        blocks.push(block.canonical());
    }
    if blocks.is_empty() {
        None
    } else {
        Some(blocks.join(","))
    }
}

fn split_ip_blacklist(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .map(str::trim)
        .filter(|part| !part.is_empty())
}

fn prefix_match(block: u128, candidate: u128, prefix: u8, bits: u8) -> bool {
    if prefix == 0 {
        return true;
    }
    let shift = u32::from(bits - prefix);
    let mask = u128::MAX << shift;
    (block & mask) == (candidate & mask)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn ip_blacklist_supports_exact_ipv4_and_cidr() {
        let blocks = parse_ip_blacklist("203.0.113.10, 198.51.100.0/24");
        assert!(
            blocks
                .iter()
                .any(|block| block.contains(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10))))
        );
        assert!(
            blocks
                .iter()
                .any(|block| block.contains(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 42))))
        );
        assert!(
            !blocks
                .iter()
                .any(|block| block.contains(IpAddr::V4(Ipv4Addr::new(198, 51, 101, 42))))
        );
    }

    #[test]
    fn ip_blacklist_supports_ipv6_cidr() {
        let blocks = parse_ip_blacklist("2001:db8::/32");
        assert!(blocks.iter().any(|block| {
            block.contains(IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1)))
        }));
        assert!(
            !blocks
                .iter()
                .any(|block| block.contains(IpAddr::V6(Ipv6Addr::LOCALHOST)))
        );
    }

    #[test]
    fn normalize_ip_blacklist_rejects_invalid_entries() {
        assert_eq!(
            normalize_ip_blacklist("203.0.113.10 198.51.100.0/24").as_deref(),
            Some("203.0.113.10,198.51.100.0/24")
        );
        assert!(normalize_ip_blacklist("not-an-ip").is_none());
    }
}
