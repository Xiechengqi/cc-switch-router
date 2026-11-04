//! Channel-agnostic notification composition.
//!
//! One [`ComposedNotice`] feeds every outbound channel and the in-app center:
//! Bark gets a scannable title + compact plain text, Telegram gets MarkdownV2,
//! email gets the shared HTML card. The header (Router identity) and footer
//! (region · domain · deep link) are built once here so every channel shows
//! the same, branded, minimal frame.

use crate::notification_kinds::{self, NoticeLevel};

pub const BRAND: &str = "TokenSwitch Router";

/// Everything a channel needs to render one notification. Facts are already
/// display-formatted strings; channels never re-interpret raw payloads.
/// Some renderers (Telegram Markdown, email) are wired in as user-facing
/// channels adopt the composer; until then they are exercised by unit tests.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ComposedNotice {
    pub level: NoticeLevel,
    /// Stable kind id, e.g. `llm_error_rate` or `market_c2c_dispute`.
    pub kind: String,
    /// Short state suffix for operator alerts: firing/resolved; empty for
    /// business events.
    pub state: String,
    pub title_zh: String,
    pub title_en: String,
    pub summary_zh: String,
    pub summary_en: String,
    pub action_zh: Option<String>,
    pub action_en: Option<String>,
    pub facts: Vec<(String, String)>,
    pub href: String,
    pub occurred_at: i64,
    pub region: String,
    pub domain: String,
}

impl ComposedNotice {
    /// Compose an operator alert / Owner signal from its registered kind.
    /// Unregistered kinds fall back to the raw title so a legacy incident
    /// still renders, just without translation.
    pub fn for_alert(
        kind: &str,
        raw_title: &str,
        state: &str,
        facts: &serde_json::Value,
        href: String,
        occurred_at: i64,
        region: String,
        domain: String,
    ) -> Self {
        let registered = notification_kinds::describe(kind);
        let level = registered
            .map(|entry| entry.level)
            .unwrap_or(NoticeLevel::Record);
        let (title_zh, title_en) = match registered {
            Some(entry) => (
                entry.copy.title_zh.to_string(),
                entry.copy.title_en.to_string(),
            ),
            None => (raw_title.to_string(), raw_title.to_string()),
        };
        let summary_zh = registered
            .map(|entry| notification_kinds::fill(entry.copy.summary_zh, facts))
            .unwrap_or_default();
        let summary_en = registered
            .map(|entry| notification_kinds::fill(entry.copy.summary_en, facts))
            .unwrap_or_default();
        let action_zh = registered
            .and_then(|entry| entry.copy.action_zh)
            .map(str::to_string);
        let action_en = registered
            .and_then(|entry| entry.copy.action_en)
            .map(str::to_string);
        Self {
            level,
            kind: kind.to_string(),
            state: state.to_string(),
            title_zh,
            title_en,
            summary_zh,
            summary_en,
            action_zh,
            action_en,
            facts: display_facts(facts),
            href,
            occurred_at,
            region,
            domain,
        }
    }

    fn level_icon(&self) -> &'static str {
        match self.level {
            NoticeLevel::Action => "🔴",
            NoticeLevel::Attention => "🟠",
            NoticeLevel::Record => "ℹ️",
        }
    }

    /// Bark list title: icon + concrete subject, never a generic "alert push".
    pub fn bark_title(&self) -> String {
        let state = match self.state.as_str() {
            "resolved" => "（已恢复）",
            "escalated" => "（已升级）",
            _ => "",
        };
        format!(
            "{} {}{} · {}",
            self.level_icon(),
            self.title_zh,
            state,
            BRAND
        )
    }

    /// Compact plain text for Bark. Kept short: lock-screen real estate.
    pub fn bark_body(&self) -> String {
        let mut lines = vec![self.summary_zh.clone()];
        if let Some(action) = &self.action_zh {
            lines.push(format!("建议：{action}"));
        }
        for (key, value) in self.facts.iter().take(3) {
            lines.push(format!("{key}: {value}"));
        }
        lines.push(self.frame_line());
        lines.join("\n")
    }

    /// Plain-text payload for operator alert deliveries (Telegram plain mode
    /// and Bark share this): bilingual title + summary + facts + frame.
    pub fn bark_body_with_header(&self) -> String {
        let state = match self.state.as_str() {
            "resolved" => "（已恢复）",
            "escalated" => "（已升级）",
            _ => "",
        };
        let mut lines = vec![format!("{} {}{}", self.level_icon(), self.title_zh, state)];
        if !self.title_en.is_empty() && self.title_en != self.title_zh {
            lines.push(self.title_en.clone());
        }
        if !self.summary_zh.is_empty() {
            lines.push(self.summary_zh.clone());
        }
        if let Some(action) = &self.action_zh {
            lines.push(format!("建议：{action}"));
        }
        for (key, value) in &self.facts {
            lines.push(format!("{key}: {value}"));
        }
        lines.push(self.frame_line());
        lines.join("\n")
    }

    /// Telegram MarkdownV2 body. All dynamic content is escaped.
    pub fn telegram_markdown(&self) -> String {
        let state = match self.state.as_str() {
            "resolved" => " · 已恢复",
            "escalated" => " · 已升级",
            _ => "",
        };
        let mut out = format!(
            "{} *{}*{}\n",
            self.level_icon(),
            escape_markdown(&self.title_zh),
            escape_markdown(state),
        );
        if !self.summary_zh.is_empty() {
            out.push_str(&escape_markdown(&self.summary_zh));
            out.push('\n');
        }
        for (key, value) in &self.facts {
            out.push_str(&format!(
                "▸ {}: `{}`\n",
                escape_markdown(key),
                escape_markdown(value)
            ));
        }
        if let Some(action) = &self.action_zh {
            out.push_str(&format!("💡 {}\n", escape_markdown(action)));
        }
        out.push_str(&escape_markdown(&self.frame_line()));
        if !self.href.is_empty() {
            out.push_str(&format!(
                "\n[查看详情]({})",
                escape_markdown_url(&self.href)
            ));
        }
        out
    }

    /// HTML email body, dropped into the shared email card layout.
    pub fn email_body_html(&self) -> String {
        let level_color = match self.level {
            NoticeLevel::Action => "#dc2626",
            NoticeLevel::Attention => "#d97706",
            NoticeLevel::Record => "#2563eb",
        };
        let level_label = match self.level {
            NoticeLevel::Action => "需要处理",
            NoticeLevel::Attention => "值得关注",
            NoticeLevel::Record => "记录",
        };
        let state = match self.state.as_str() {
            "resolved" => " · 已恢复",
            "escalated" => " · 已升级",
            _ => "",
        };
        let facts = self
            .facts
            .iter()
            .map(|(key, value)| {
                format!(
                    "<tr><th align=\"left\" valign=\"top\" style=\"padding:8px 16px 8px 0;color:#475569;font-size:13px;font-weight:400\">{}</th><td align=\"right\" style=\"padding:8px 0;color:#0f172a;font-size:14px;word-break:break-all\">{}</td></tr>",
                    escape_html(key),
                    escape_html(value)
                )
            })
            .collect::<String>();
        let action_html = self
            .action_zh
            .as_ref()
            .map(|action| {
                format!(
                    "<p style=\"margin:16px 0 0;padding:10px 14px;border-left:3px solid #0052FF;background:#f8fafc;color:#334155;font-size:13px;line-height:1.6\">建议：{}</p>",
                    escape_html(action)
                )
            })
            .unwrap_or_default();
        let link_html = if self.href.is_empty() {
            String::new()
        } else {
            format!(
                "<a href=\"{}\" style=\"display:inline-block;margin-top:24px;padding:12px 20px;border-radius:8px;background:#2563eb;color:#ffffff;text-decoration:none;font-size:14px;font-weight:600;white-space:nowrap\">查看详情</a>",
                escape_html(&self.href)
            )
        };
        format!(
            "<p style=\"margin:0 0 4px;font-size:12px;font-weight:600;letter-spacing:.06em;color:{level_color}\">{level_label}{}</p>\
             <p style=\"margin:0 0 24px;color:#475569;font-size:15px;line-height:1.6\">{}</p>\
             <table role=\"presentation\" width=\"100%\" cellspacing=\"0\" cellpadding=\"0\">{facts}</table>\
             {action_html}{link_html}",
            escape_html(state),
            escape_html(&self.summary_zh),
        )
    }

    pub fn email_subject(&self) -> String {
        let state = match self.state.as_str() {
            "resolved" => "（已恢复）",
            _ => "",
        };
        format!("TokenSwitch: {}{state}", self.title_zh)
    }

    pub fn email_footer(&self) -> String {
        format!(
            "{} · {} · {}",
            BRAND,
            if self.region.is_empty() {
                "default region"
            } else {
                self.region.as_str()
            },
            self.domain
        )
    }

    /// Plain-text fallback for email clients without HTML.
    pub fn email_text(&self) -> String {
        let mut lines = vec![
            BRAND.to_string(),
            String::new(),
            self.title_zh.clone(),
            self.summary_zh.clone(),
        ];
        for (key, value) in &self.facts {
            lines.push(format!("{key}: {value}"));
        }
        if let Some(action) = &self.action_zh {
            lines.push(format!("建议：{action}"));
        }
        if !self.href.is_empty() {
            lines.push(format!("查看详情: {}", self.href));
        }
        lines.push(String::new());
        lines.push(self.email_footer());
        lines.join("\n")
    }

    fn frame_line(&self) -> String {
        let when = chrono::DateTime::from_timestamp(self.occurred_at, 0)
            .map(|value| {
                value
                    .with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).unwrap())
                    .format("%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_else(|| self.occurred_at.to_string());
        let region = if self.region.is_empty() {
            "default"
        } else {
            self.region.as_str()
        };
        format!("{BRAND} · {region} · {when} (UTC+8)")
    }
}

/// Pick the operator-meaningful facts for display. Internal bookkeeping keys
/// (ids, cursors, event keys) never reach a phone screen.
fn display_facts(facts: &serde_json::Value) -> Vec<(String, String)> {
    const SKIP: &[&str] = &[
        "clientEventId",
        "notificationEventKey",
        "reviewCaseId",
        "summary",
    ];
    let Some(object) = facts.as_object() else {
        return Vec::new();
    };
    object
        .iter()
        .filter(|(key, _)| !SKIP.contains(&key.as_str()))
        .filter(|(_, value)| value.is_string() || value.is_number() || value.is_boolean())
        .take(6)
        .map(|(key, value)| {
            let rendered = notification_kinds::fill("{v}", &serde_json::json!({ "v": value }));
            (fact_label(key), rendered)
        })
        .collect()
}

fn fact_label(key: &str) -> String {
    match key {
        "errorRate" => "错误率".into(),
        "rpm" => "每分钟请求".into(),
        "rateLimitPerMinute" => "每分钟限流".into(),
        "fdUsagePercent" => "FD 使用率".into(),
        "cpuPercent" => "CPU 使用率".into(),
        "memoryUsagePercent" => "内存使用率".into(),
        "diskUsagePercent" => "磁盘使用率".into(),
        "newErrors" => "新增错误".into(),
        "newTimeouts" => "新增超时".into(),
        "newStalls" => "新增停滞".into(),
        "newReleases" => "新增释放".into(),
        "newRejections" => "新增拒绝".into(),
        "total" => "累计".into(),
        "activeBridges" => "活跃桥接".into(),
        "pendingChannelOpens" => "待开通道".into(),
        "activeRequests" => "活跃请求".into(),
        "mismatches" => "不一致数".into(),
        "clockOffsetMs" => "时钟偏移(ms)".into(),
        "offsetSeconds" => "偏移(秒)".into(),
        "direction" => "方向".into(),
        "validSources" => "可用时间源".into(),
        "totalSources" => "时间源总数".into(),
        "clientLabel" => "Client".into(),
        other => other.to_string(),
    }
}

#[allow(dead_code)]
pub fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Telegram MarkdownV2 escaping: every reserved byte outside an ASCII letter
/// or digit must be backslash-escaped.
#[allow(dead_code)]
pub fn escape_markdown(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    for ch in value.chars() {
        if matches!(ch, '_' | '*' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '=' | '|' | '{' | '}' | '.' | '!') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn escape_markdown_url(value: &str) -> String {
    value.replace('\\', "\\\\").replace(')', "\\)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn notice() -> ComposedNotice {
        ComposedNotice::for_alert(
            "llm_error_rate",
            "LLM request error rate",
            "firing",
            &json!({"errorRate": 0.32, "rpm": 18.0}),
            "https://router.example.com/metrics/?tab=alerts".into(),
            1_762_204_260,
            "japan".into(),
            "router.example.com".into(),
        )
    }

    #[test]
    fn bark_title_names_the_alert() {
        let title = notice().bark_title();
        assert!(title.contains("LLM 请求错误率偏高"), "{title}");
        assert!(title.contains(BRAND));
        assert!(!title.contains("alert push"));
    }

    #[test]
    fn telegram_body_is_escaped_and_structured() {
        let body = notice().telegram_markdown();
        assert!(body.contains("*LLM 请求错误率偏高*"), "{body}");
        assert!(body.contains("错误率"), "{body}");
        assert!(body.contains("32%"), "{body}");
        assert!(body.contains("查看详情"), "{body}");
    }

    #[test]
    fn resolved_state_marks_title() {
        let mut resolved = notice();
        resolved.state = "resolved".into();
        assert!(resolved.bark_title().contains("已恢复"));
        assert!(resolved.email_subject().contains("已恢复"));
    }

    #[test]
    fn unregistered_kind_falls_back_to_raw_title_quietly() {
        let legacy = ComposedNotice::for_alert(
            "legacy_unknown",
            "Legacy thing",
            "firing",
            &json!({}),
            String::new(),
            0,
            String::new(),
            "router.example.com".into(),
        );
        assert_eq!(legacy.level, NoticeLevel::Record);
        assert_eq!(legacy.title_zh, "Legacy thing");
    }

    #[test]
    fn internal_keys_never_render() {
        let with_ids = ComposedNotice::for_alert(
            "market_c2c_dispute",
            "C2C",
            "firing",
            &json!({"clientEventId": "evt-1", "orderId": "ord-9", "summary": "x"}),
            String::new(),
            0,
            String::new(),
            "router.example.com".into(),
        );
        let keys: Vec<&str> = with_ids.facts.iter().map(|(key, _)| key.as_str()).collect();
        assert!(!keys.contains(&"clientEventId"));
        assert!(keys.contains(&"orderId"));
    }

    #[test]
    fn markdown_escapes_reserved_bytes() {
        assert_eq!(escape_markdown("a.b_c(d)"), "a\\.b\\_c\\(d\\)");
    }
}
