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

    pub fn selected_channel(&self) -> Option<&NotificationChannelId> {
        self.targets.first().map(|target| &target.channel)
    }

    pub fn target(&self, channel: &NotificationChannelId) -> Option<&NotificationTarget> {
        self.targets
            .iter()
            .find(|target| &target.channel == channel)
    }

    /// Resolve the single channel a notification is delivered on.
    ///
    /// The selected channel is honoured whenever it can actually carry the
    /// notification. When it cannot — the bot is down, or the lane refuses
    /// Telegram — the account email is used instead: a selection is a
    /// preference, not a reason to drop an alert on the floor.
    pub fn delivery_targets(
        &self,
        telegram_available: bool,
        telegram_allowed: bool,
    ) -> Vec<NotificationTarget> {
        let selected = self.targets.iter().find(|target| {
            if target.channel.is_telegram() {
                telegram_available && telegram_allowed
            } else {
                true
            }
        });
        match selected {
            Some(target) => vec![target.clone()],
            None => vec![NotificationTarget {
                channel: NotificationChannelId::email(),
                address: self.email.clone(),
                revision: 0,
                provider_identity: None,
            }],
        }
    }
}

/// Parse the channel a user selected. Unlike the delivery-time resolution
/// above, this is a strict contract check: an unknown or malformed id is a bad
/// request, never a silent fallback to email.
pub fn parse_delivery_channel(value: &str) -> Result<NotificationChannelId, AppError> {
    let channel = NotificationChannelId::parse(value)?;
    if !channel.is_email() && !channel.is_telegram() {
        return Err(AppError::BadRequest(format!(
            "unsupported notification channel: {channel}"
        )));
    }
    Ok(channel)
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
    fn only_shipped_channels_can_be_selected() {
        assert!(
            parse_delivery_channel(" Telegram ")
                .expect("telegram")
                .is_telegram()
        );
        assert!(parse_delivery_channel("EMAIL").expect("email").is_email());
        // Storable, but not something a user can be switched onto yet.
        assert!(parse_delivery_channel("matrix_v2").is_err());
        assert!(parse_delivery_channel("").is_err());
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
        assert_eq!(resolved[0].address, "owner@example.com");
        assert_eq!(
            resolved[0].revision, 0,
            "a fallback delivery must not claim the Telegram row's revision"
        );
    }

    #[test]
    fn a_usable_selection_is_the_only_delivery_target() {
        let targets = NotificationTargets {
            email: "owner@example.com".into(),
            targets: vec![NotificationTarget {
                channel: NotificationChannelId::telegram(),
                address: "42".into(),
                revision: 3,
                provider_identity: Some("7".into()),
            }],
        };
        let resolved = targets.delivery_targets(true, true);
        assert_eq!(resolved.len(), 1, "one notification, one destination");
        assert!(resolved[0].channel.is_telegram());
        assert_eq!(resolved[0].revision, 3);
    }

    #[test]
    fn a_lane_that_refuses_telegram_still_reaches_the_owner() {
        let targets = NotificationTargets {
            email: "owner@example.com".into(),
            targets: vec![NotificationTarget {
                channel: NotificationChannelId::telegram(),
                address: "42".into(),
                revision: 3,
                provider_identity: Some("7".into()),
            }],
        };
        let resolved = targets.delivery_targets(true, false);
        assert_eq!(resolved.len(), 1);
        assert!(resolved[0].channel.is_email());
    }
}
