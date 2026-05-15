use std::collections::HashSet;

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
    pub telegram: TelegramSettings,
    pub board: BoardSettings,
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
}
