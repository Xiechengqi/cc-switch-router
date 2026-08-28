use std::collections::HashSet;
use std::net::IpAddr;

use crate::config::{AlertingSettings, ClientNotificationSettings, Config, TelegramBotSettings};

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
    pub client_notifications: ClientNotificationSettings,
    pub alerting: AlertingSettings,
    /// User-facing Telegram notification bot. Hot-reloading this group has to
    /// restart the update listener and re-run `getMe`; see
    /// `crate::telegram::service`.
    pub telegram_bot: TelegramBotSettings,
    pub market_usd_cny_rate_micros: i64,
    pub footer_telegram_url: String,
    pub server_log_public_enabled: bool,
    /// GitHub Release tag rendered into newly downloaded Client installers.
    pub client_server_release: String,
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

impl DynamicSettings {
    pub fn from_config(config: &Config) -> Self {
        Self {
            admin_emails: config.admin_emails.clone(),
            security: SecuritySettings {
                ip_blacklist: parse_ip_blacklist(&config.ip_blacklist),
            },
            client_notifications: config.client_notifications.clone(),
            alerting: config.metrics.alerting.clone(),
            telegram_bot: config.telegram_bot.clone(),
            market_usd_cny_rate_micros: config.market_usd_cny_rate_micros,
            footer_telegram_url: config.footer_telegram_url.clone(),
            server_log_public_enabled: crate::server_logs::public_enabled_from_env(),
            client_server_release: crate::client_server_release::client_server_release_from_env(),
        }
    }

    pub fn footer_telegram_link(&self) -> Option<String> {
        let trimmed = self.footer_telegram_url.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
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
    fn dynamic_settings_preserve_registration_notification_caps() {
        let mut config = Config::from_env();
        config
            .client_notifications
            .registration_recipient_hourly_limit = 8;
        config.client_notifications.registration_global_hourly_limit = 21;

        let dynamic = DynamicSettings::from_config(&config);

        assert_eq!(
            dynamic
                .client_notifications
                .registration_recipient_hourly_limit,
            8
        );
        assert_eq!(
            dynamic
                .client_notifications
                .registration_global_hourly_limit,
            21
        );
    }

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
