use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tracing::warn;

use crate::config::Config;
use crate::models::BoardMessageView;

#[derive(Clone)]
pub struct TelegramNotifier {
    client: reqwest::Client,
    bot_token: String,
    chat_id: String,
    topic_id: Option<i64>,
    notify_admin: bool,
    dashboard_url: String,
}

impl TelegramNotifier {
    pub fn from_config(config: &Config) -> Option<Arc<Self>> {
        if !config.telegram_notify_all {
            return None;
        }
        let token = config.telegram_bot_token.as_deref()?.trim();
        let chat_id = config.telegram_chat_id.as_deref()?.trim();
        if token.is_empty() || chat_id.is_empty() {
            return None;
        }
        let client = reqwest::Client::builder()
            .user_agent("cc-switch-router/0.1 board-telegram")
            .timeout(Duration::from_secs(8))
            .build()
            .ok()?;
        let scheme = if config.use_localhost { "http" } else { "https" };
        let dashboard_url = format!("{scheme}://{}", config.tunnel_domain);
        Some(Arc::new(Self {
            client,
            bot_token: token.to_string(),
            chat_id: chat_id.to_string(),
            topic_id: config.telegram_topic_id,
            notify_admin: config.telegram_notify_admin,
            dashboard_url,
        }))
    }

    pub async fn notify_new_message(&self, message: &BoardMessageView) {
        if message.author_kind == "admin" && !self.notify_admin {
            return;
        }
        let text = format_message(&self.dashboard_url, message);
        let url = format!("https://api.telegram.org/bot{}/sendMessage", self.bot_token);
        let mut payload = json!({
            "chat_id": self.chat_id,
            "text": text,
            "parse_mode": "MarkdownV2",
            "disable_web_page_preview": true,
        });
        if let Some(topic) = self.topic_id {
            payload["message_thread_id"] = json!(topic);
        }

        let mut last_error: Option<String> = None;
        for attempt in 0..3 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(400 * attempt as u64)).await;
            }
            match self.client.post(&url).json(&payload).send().await {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        return;
                    }
                    let body = response.text().await.unwrap_or_default();
                    if status.is_client_error() {
                        warn!(
                            status = %status,
                            body = %body,
                            message_id = %message.id,
                            "telegram board notify rejected by api"
                        );
                        return;
                    }
                    last_error = Some(format!("HTTP {status}: {body}"));
                }
                Err(err) => {
                    last_error = Some(err.to_string());
                }
            }
        }
        if let Some(err) = last_error {
            warn!(
                error = %err,
                message_id = %message.id,
                "telegram board notify failed after retries"
            );
        }
    }
}

fn format_message(dashboard_url: &str, message: &BoardMessageView) -> String {
    let badge = match message.author_kind.as_str() {
        "admin" => "👑 Official",
        "user" => "👤 User",
        _ => "💬 Guest",
    };
    let author = escape_md(&message.author_label);
    let time = message.created_at.format("%Y-%m-%d %H:%M UTC").to_string();
    let time = escape_md(&time);
    let body_excerpt = excerpt(&message.body, 400);
    let body = escape_md(&body_excerpt)
        .replace('\n', "\n> ");
    let link = format!("{}/#board-{}", dashboard_url.trim_end_matches('/'), message.id);
    let link_label = escape_md("Open dashboard");
    let link_target = escape_url(&link);
    let mut tags = Vec::new();
    if message.pinned {
        tags.push("📌 pinned");
    }
    if message.featured {
        tags.push("⭐ featured");
    }
    let tag_line = if tags.is_empty() {
        String::new()
    } else {
        format!("\n_{}_\n", escape_md(&tags.join(" · ")))
    };

    format!(
        "🗒 *New message on cc\\-switch\\-router*\n\n*From:* {badge} {author}\n*At:* {time}{tag_line}\n> {body}\n\n[{link_label}]({link_target})",
        badge = escape_md(badge),
        author = author,
        time = time,
        tag_line = tag_line,
        body = body,
        link_label = link_label,
        link_target = link_target,
    )
}

fn excerpt(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max_chars).collect();
    out.push('…');
    out
}

fn escape_md(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(
            ch,
            '_' | '*'
                | '['
                | ']'
                | '('
                | ')'
                | '~'
                | '`'
                | '>'
                | '#'
                | '+'
                | '-'
                | '='
                | '|'
                | '{'
                | '}'
                | '.'
                | '!'
                | '\\'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn escape_url(value: &str) -> String {
    value.replace(')', "%29").replace('\\', "%5C")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_message(kind: &str) -> BoardMessageView {
        BoardMessageView {
            id: "abc".into(),
            body: "Hello, world!\nHave a [great] day_".into(),
            author_kind: kind.into(),
            author_label: "alice@example.com".into(),
            is_mine: false,
            pinned: true,
            featured: false,
            created_at: chrono::Utc.with_ymd_and_hms(2026, 5, 14, 12, 33, 0).unwrap(),
            pinned_at: None,
            featured_at: None,
        }
    }

    #[test]
    fn format_message_escapes_markdown_specials() {
        let formatted = format_message("https://router.example", &sample_message("user"));
        assert!(formatted.contains("alice@example\\.com"));
        assert!(formatted.contains("Have a \\[great\\] day\\_"));
        assert!(formatted.contains("📌 pinned"));
    }

    #[test]
    fn excerpt_truncates_long_text() {
        let result = excerpt("abcdefghij", 5);
        assert_eq!(result, "abcde…");
    }
}
