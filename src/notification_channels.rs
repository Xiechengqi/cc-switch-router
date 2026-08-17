use std::collections::BTreeSet;
use std::fmt;

use crate::error::AppError;

pub const EMAIL_CHANNEL: &str = "email";
pub const TELEGRAM_CHANNEL: &str = "telegram";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NotificationChannelId(String);

impl NotificationChannelId {
    pub fn parse(value: &str) -> Result<Self, AppError> {
        let normalized = value.trim().to_ascii_lowercase();
        if normalized.is_empty()
            || normalized.len() > 32
            || !normalized
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(AppError::BadRequest(format!(
                "invalid notification channel: {value}"
            )));
        }
        Ok(Self(normalized))
    }

    pub fn email() -> Self {
        Self(EMAIL_CHANNEL.into())
    }

    pub fn telegram() -> Self {
        Self(TELEGRAM_CHANNEL.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_email(&self) -> bool {
        self.0 == EMAIL_CHANNEL
    }

    pub fn is_telegram(&self) -> bool {
        self.0 == TELEGRAM_CHANNEL
    }
}

impl fmt::Display for NotificationChannelId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationTarget {
    pub channel: NotificationChannelId,
    pub address: String,
    pub revision: i64,
    pub provider_identity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationTargets {
    pub email: String,
    pub targets: Vec<NotificationTarget>,
}

impl NotificationTargets {
    pub fn email_only(email: impl Into<String>) -> Self {
        let email = email.into();
        Self {
            targets: vec![NotificationTarget {
                channel: NotificationChannelId::email(),
                address: email.clone(),
                revision: 0,
                provider_identity: None,
            }],
            email,
        }
    }

    pub fn enabled_channels(&self) -> BTreeSet<String> {
        self.targets
            .iter()
            .map(|target| target.channel.as_str().to_string())
            .collect()
    }

    pub fn target(&self, channel: &NotificationChannelId) -> Option<&NotificationTarget> {
        self.targets
            .iter()
            .find(|target| &target.channel == channel)
    }

    pub fn delivery_targets(
        &self,
        telegram_available: bool,
        telegram_allowed: bool,
    ) -> Vec<NotificationTarget> {
        let mut targets = self
            .targets
            .iter()
            .filter(|target| {
                if target.channel.is_telegram() {
                    telegram_available && telegram_allowed
                } else {
                    true
                }
            })
            .cloned()
            .collect::<Vec<_>>();
        if targets.is_empty() {
            targets.push(NotificationTarget {
                channel: NotificationChannelId::email(),
                address: self.email.clone(),
                revision: 0,
                provider_identity: None,
            });
        }
        targets.sort_by_key(|target| !target.channel.is_email());
        targets
    }
}

pub fn normalize_enabled_channels(values: &[String]) -> Result<BTreeSet<String>, AppError> {
    if values.is_empty() {
        return Err(AppError::BadRequest(
            "at least one notification channel must be enabled".into(),
        ));
    }
    values
        .iter()
        .map(|value| NotificationChannelId::parse(value).map(|channel| channel.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_ids_are_stable_and_extensible() {
        assert_eq!(
            NotificationChannelId::parse(" Telegram ")
                .expect("telegram")
                .as_str(),
            TELEGRAM_CHANNEL
        );
        assert_eq!(
            NotificationChannelId::parse("matrix_v2")
                .expect("future channel")
                .as_str(),
            "matrix_v2"
        );
        assert!(NotificationChannelId::parse("bad-channel").is_err());
    }

    #[test]
    fn unavailable_targets_fall_back_to_account_email() {
        let targets = NotificationTargets {
            email: "owner@example.com".into(),
            targets: vec![NotificationTarget {
                channel: NotificationChannelId::telegram(),
                address: "42".into(),
                revision: 3,
                provider_identity: Some("7".into()),
            }],
        };
        let resolved = targets.delivery_targets(false, true);
        assert_eq!(resolved.len(), 1);
        assert!(resolved[0].channel.is_email());
    }
}
