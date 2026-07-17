use std::time::Duration as StdDuration;

use chrono::{DateTime, Utc};
use tracing::{info, warn};
use url::Url;
use uuid::Uuid;

use crate::config::Config;
use crate::error::AppError;
use crate::notifications::{
    FrozenEmailEnvelope, NotificationTemplateContext, retry_delay_secs, sanitize_delivery_error,
    send_resend_frozen_email,
};
use crate::store::AppStore;

pub const CHAT_EMAIL_BATCH_WINDOW_SECS: i64 = 60;
pub const CHAT_ARCHIVE_RETENTION_SECS: i64 = 60 * 24 * 60 * 60;
pub const CHAT_MAX_BODY_CHARS: usize = 1_000;
pub const CHAT_USER_MESSAGES_PER_MINUTE: i64 = 20;
pub const CHAT_USER_MESSAGES_PER_HOUR: i64 = 120;
pub const CHAT_ROOM_MESSAGES_PER_MINUTE: i64 = 120;
pub const CHAT_PUBLIC_LOOKUP_MAX_ROOMS: usize = 100;
pub const CHAT_MESSAGE_PAGE_MAX: usize = 100;

const DELIVERY_INTERVAL_SECS: u64 = 5;
const CLAIM_LEASE_SECS: i64 = 90;
const MAX_DELIVERIES_PER_CYCLE: usize = 25;
const MAX_DELIVERY_ATTEMPTS: u32 = 12;

#[derive(Debug, Clone, Default)]
pub struct ChatAggregateStats {
    pub deliveries_created: usize,
    pub events_batched: usize,
}

impl ChatAggregateStats {
    pub fn has_activity(&self) -> bool {
        self.deliveries_created > 0 || self.events_batched > 0
    }
}

#[derive(Debug, Clone)]
pub struct ChatDeliveryClaim {
    pub id: String,
    pub recipient: String,
    pub from: String,
    pub reply_to: Option<String>,
    pub subject: String,
    pub html: String,
    pub text: String,
    pub idempotency_key: String,
    pub attempts: u32,
}

#[derive(Debug, Clone)]
pub struct ChatEmailMessageData {
    pub created_at: DateTime<Utc>,
    pub author_label: String,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct ChatEmailData {
    pub installation_id: String,
    pub client_label: String,
    pub messages: Vec<ChatEmailMessageData>,
    pub dashboard_url: String,
}

#[derive(Debug, Clone)]
pub struct RenderedChatEmail {
    pub subject: String,
    pub html: String,
    pub text: String,
}

pub fn render_chat_email(data: &ChatEmailData) -> RenderedChatEmail {
    let count = data.messages.len();
    let subject_label = data.client_label.chars().take(80).collect::<String>();
    let subject = format!(
        "[Client Chat] {subject_label}: {count} new message{}",
        if count == 1 { "" } else { "s" }
    );
    let room_url = chat_room_url(&data.dashboard_url, &data.installation_id);
    let message_html = data
        .messages
        .iter()
        .map(|message| {
            format!(
                "<div style=\"padding:14px 0;border-bottom:1px solid #e2e8f0\"><div style=\"font:600 13px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace;color:#475569\">[{}] [{}]</div><div style=\"margin-top:7px;white-space:pre-wrap;word-break:break-word;font:14px/1.65 system-ui,-apple-system,sans-serif;color:#0f172a\">{}</div></div>",
                escape_html(&message.created_at.to_rfc3339()),
                escape_html(&message.author_label),
                escape_html(&message.body),
            )
        })
        .collect::<Vec<_>>()
        .join("");
    let message_text = data
        .messages
        .iter()
        .map(|message| {
            format!(
                "[{}] [{}]\n{}",
                message.created_at.to_rfc3339(),
                message.author_label,
                message.body
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let html = format!(
        "<!doctype html><html><body style=\"margin:0;background:#f8fafc;color:#0f172a\"><div style=\"max-width:680px;margin:0 auto;padding:32px 20px\"><div style=\"background:#fff;border:1px solid #e2e8f0;border-radius:8px;padding:28px\"><div style=\"font:600 12px/1.4 system-ui,-apple-system,sans-serif;color:#64748b;text-transform:uppercase\">Client chat</div><h1 style=\"margin:8px 0 4px;font:700 22px/1.3 system-ui,-apple-system,sans-serif\">{}</h1><p style=\"margin:0 0 18px;font:14px/1.6 system-ui,-apple-system,sans-serif;color:#64748b\">{} new message{} in the last minute.</p>{}<a href=\"{}\" style=\"display:inline-block;margin-top:24px;padding:10px 16px;border-radius:6px;background:#0052ff;color:#fff;text-decoration:none;font:600 14px/1.4 system-ui,-apple-system,sans-serif\">Open chat</a></div></div></body></html>",
        escape_html(&data.client_label),
        count,
        if count == 1 { "" } else { "s" },
        message_html,
        escape_html(&room_url),
    );
    let text = format!(
        "Client chat: {}\n{} new message{} in the last minute.\n\n{}\n\nOpen chat: {}",
        data.client_label,
        count,
        if count == 1 { "" } else { "s" },
        message_text,
        room_url,
    );
    RenderedChatEmail {
        subject,
        html,
        text,
    }
}

fn chat_room_url(dashboard_url: &str, installation_id: &str) -> String {
    Url::parse(dashboard_url)
        .ok()
        .map(|mut url| {
            url.set_path("/clients");
            url.set_query(None);
            url.query_pairs_mut().append_pair("chat", installation_id);
            url.to_string()
        })
        .unwrap_or_else(|| {
            format!(
                "{}/clients?chat={installation_id}",
                dashboard_url.trim_end_matches('/')
            )
        })
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub async fn run_client_chat_email_service(store: AppStore, config: Config) -> anyhow::Result<()> {
    let http = reqwest::Client::builder()
        .user_agent("cc-switch-router/0.1 client-chat")
        .connect_timeout(StdDuration::from_secs(10))
        .timeout(StdDuration::from_secs(20))
        .build()?;
    let worker_id = format!("chat-{}", Uuid::new_v4());
    let template = NotificationTemplateContext::from_config(&config);
    let mut interval = tokio::time::interval(StdDuration::from_secs(DELIVERY_INTERVAL_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;
        if let Err(error) =
            run_chat_email_cycle(&store, &config, &template, &http, &worker_id, Utc::now()).await
        {
            warn!(error = %error, "client chat email cycle failed");
        }
    }
}

async fn run_chat_email_cycle(
    store: &AppStore,
    config: &Config,
    template: &NotificationTemplateContext,
    http: &reqwest::Client,
    worker_id: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let aggregated = store
        .aggregate_client_chat_deliveries(template, now)
        .await?;
    if aggregated.has_activity() {
        info!(
            deliveries_created = aggregated.deliveries_created,
            events_batched = aggregated.events_batched,
            "client chat email events aggregated"
        );
    }

    for _ in 0..MAX_DELIVERIES_PER_CYCLE {
        let Some(delivery) = store
            .claim_client_chat_delivery(worker_id, Utc::now(), CLAIM_LEASE_SECS)
            .await?
        else {
            break;
        };
        if !store
            .validate_client_chat_delivery(&delivery.id, worker_id, Utc::now())
            .await?
        {
            continue;
        }

        let Some(api_key) = config
            .resend_api_key
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        else {
            store
                .mark_client_chat_delivery_retry(
                    &delivery.id,
                    worker_id,
                    "Resend API key is not configured",
                    Utc::now() + chrono::Duration::minutes(5),
                    Utc::now(),
                )
                .await?;
            continue;
        };

        let envelope = FrozenEmailEnvelope {
            from: &delivery.from,
            recipient: &delivery.recipient,
            subject: &delivery.subject,
            html: &delivery.html,
            text: &delivery.text,
            reply_to: delivery.reply_to.as_deref(),
            idempotency_key: &delivery.idempotency_key,
        };
        match send_resend_frozen_email(http, api_key, envelope).await {
            Ok(provider_message_id) => {
                store
                    .mark_client_chat_delivery_sent(
                        &delivery.id,
                        worker_id,
                        &provider_message_id,
                        Utc::now(),
                    )
                    .await?;
                info!(delivery_id = %delivery.id, provider_message_id, "client chat email sent");
            }
            Err(failure) => {
                let error = sanitize_delivery_error(&failure.message);
                if failure.retryable && delivery.attempts < MAX_DELIVERY_ATTEMPTS {
                    let next_attempt_at = failure.retry_at.unwrap_or_else(|| {
                        Utc::now()
                            + chrono::Duration::seconds(retry_delay_secs(
                                delivery.attempts,
                                &delivery.id,
                            ))
                    });
                    store
                        .mark_client_chat_delivery_retry(
                            &delivery.id,
                            worker_id,
                            &error,
                            next_attempt_at,
                            Utc::now(),
                        )
                        .await?;
                } else {
                    store
                        .mark_client_chat_delivery_dead_letter(
                            &delivery.id,
                            worker_id,
                            &error,
                            Utc::now(),
                        )
                        .await?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_contains_every_message_and_escapes_html() {
        let rendered = render_chat_email(&ChatEmailData {
            installation_id: "client-1".into(),
            client_label: "client.example.com".into(),
            messages: vec![
                ChatEmailMessageData {
                    created_at: DateTime::parse_from_rfc3339("2026-07-17T10:00:00Z")
                        .unwrap()
                        .with_timezone(&Utc),
                    author_label: "alice".into(),
                    body: "first <script>".into(),
                },
                ChatEmailMessageData {
                    created_at: DateTime::parse_from_rfc3339("2026-07-17T10:00:30Z")
                        .unwrap()
                        .with_timezone(&Utc),
                    author_label: "bob".into(),
                    body: "second".into(),
                },
            ],
            dashboard_url: "https://router.example.com".into(),
        });

        assert!(rendered.subject.contains("2 new messages"));
        assert!(rendered.html.contains("first &lt;script&gt;"));
        assert!(!rendered.html.contains("first <script>"));
        assert!(rendered.html.contains("second"));
        assert!(rendered.text.contains("[alice]"));
        assert!(rendered.text.contains("[bob]"));
        assert!(rendered.html.contains("/clients?chat=client-1"));
    }
}
