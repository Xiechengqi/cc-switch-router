use chrono::Utc;

use crate::models::{AccountUsageResponse, GlobalUsageResponse, UsageModelRow};

struct ThemePalette {
    bg: &'static str,
    border: &'static str,
    text: &'static str,
    muted: &'static str,
    accent: &'static str,
    bar_track: &'static str,
    chip_bg: &'static str,
    chip_active_bg: &'static str,
    chip_active_text: &'static str,
}

fn palette(theme: &str) -> ThemePalette {
    if theme.eq_ignore_ascii_case("light") {
        ThemePalette {
            bg: "#f8fafc",
            border: "#cbd5e1",
            text: "#0f172a",
            muted: "#64748b",
            accent: "#0f766e",
            bar_track: "#e2e8f0",
            chip_bg: "#e2e8f0",
            chip_active_bg: "#0f766e",
            chip_active_text: "#f8fafc",
        }
    } else {
        ThemePalette {
            bg: "#0b1220",
            border: "#1e293b",
            text: "#e2e8f0",
            muted: "#94a3b8",
            accent: "#2dd4bf",
            bar_track: "#1e293b",
            chip_bg: "#1e293b",
            chip_active_bg: "#2dd4bf",
            chip_active_text: "#0b1220",
        }
    }
}

fn normalize_theme(theme: &str) -> &'static str {
    if theme.eq_ignore_ascii_case("light") {
        "light"
    } else {
        "dark"
    }
}

fn normalize_period_label(period: &str) -> &'static str {
    match period.trim().to_ascii_lowercase().as_str() {
        "24h" | "1d" => "24h",
        "30d" | "30天" => "30d",
        _ => "7d",
    }
}

pub fn escape_xml(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

pub fn format_compact_number(value: u64) -> String {
    const UNITS: [(u64, &str); 3] = [(1_000_000_000, "B"), (1_000_000, "M"), (1_000, "K")];
    for (threshold, suffix) in UNITS {
        if value >= threshold {
            let scaled = value as f64 / threshold as f64;
            let precision = if scaled >= 100.0 {
                0
            } else {
                1
            };
            let text = format!("{scaled:.precision$}");
            let trimmed = text.trim_end_matches('0').trim_end_matches('.');
            return format!("{trimmed}{suffix}");
        }
    }
    value.to_string()
}

fn period_links(current: &str, theme: &str) -> String {
    let theme = normalize_theme(theme);
    let current = normalize_period_label(current);
    let p = palette(theme);
    let mut out = String::new();
    let mut x = 520.0_f64;
    for period in ["24h", "7d", "30d"] {
        let active = period == current;
        let bg = if active {
            p.chip_active_bg
        } else {
            p.chip_bg
        };
        let fg = if active {
            p.chip_active_text
        } else {
            p.muted
        };
        out.push_str(&format!(
            r#"<a href="?period={period}&amp;theme={theme}"><rect x="{x:.1}" y="18" width="44" height="22" rx="6" fill="{bg}"/><text x="{label_x:.1}" y="33" text-anchor="middle" font-family="ui-sans-serif,system-ui,sans-serif" font-size="11" font-weight="600" fill="{fg}">{period}</text></a>"#,
            label_x = x + 22.0,
        ));
        x += 50.0;
    }
    out
}

fn model_rows_svg(models: &[UsageModelRow], theme: &str, max_rows: usize, start_y: f64) -> (String, f64) {
    let p = palette(theme);
    let rows = models.iter().take(max_rows).collect::<Vec<_>>();
    let max_total = rows
        .iter()
        .map(|row| row.total_tokens)
        .max()
        .unwrap_or(0)
        .max(1);
    let mut out = String::new();
    let mut y = start_y;
    if rows.is_empty() {
        out.push_str(&format!(
            r#"<text x="24" y="{y:.1}" font-family="ui-sans-serif,system-ui,sans-serif" font-size="12" fill="{muted}">No model usage in this period</text>"#,
            muted = p.muted,
        ));
        return (out, y + 18.0);
    }
    for row in rows {
        let ratio = row.total_tokens as f64 / max_total as f64;
        let bar_w = (ratio * 280.0).max(2.0);
        let model = escape_xml(&truncate_label(&row.model, 28));
        let total = escape_xml(&format_compact_number(row.total_tokens));
        out.push_str(&format!(
            r#"<text x="24" y="{y:.1}" font-family="ui-monospace,SFMono-Regular,Menlo,monospace" font-size="12" fill="{text}">{model}</text>
<rect x="300" y="{bar_y:.1}" width="280" height="8" rx="4" fill="{track}"/>
<rect x="300" y="{bar_y:.1}" width="{bar_w:.1}" height="8" rx="4" fill="{accent}"/>
<text x="656" y="{y:.1}" text-anchor="end" font-family="ui-sans-serif,system-ui,sans-serif" font-size="12" fill="{muted}">{total}</text>"#,
            text = p.text,
            track = p.bar_track,
            accent = p.accent,
            muted = p.muted,
            bar_y = y - 8.0,
        ));
        y += 22.0;
    }
    (out, y)
}

fn truncate_label(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let mut out: String = value.chars().take(keep).collect();
    out.push('…');
    out
}

fn footer_svg(theme: &str, y: f64) -> String {
    let p = palette(theme);
    let updated = Utc::now().format("%Y-%m-%d %H:%M UTC");
    format!(
        r#"<text x="24" y="{y:.1}" font-family="ui-sans-serif,system-ui,sans-serif" font-size="11" fill="{muted}">updated {updated}</text>"#,
        muted = p.muted,
        updated = escape_xml(&updated.to_string()),
    )
}

fn card_shell(height: f64, theme: &str, body: &str) -> String {
    let theme = normalize_theme(theme);
    let p = palette(theme);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="680" height="{height:.0}" viewBox="0 0 680 {height:.0}" role="img">
<rect x="0.5" y="0.5" width="679" height="{inner_h:.0}" rx="16" fill="{bg}" stroke="{border}"/>
{body}
</svg>"#,
        inner_h = height - 1.0,
        bg = p.bg,
        border = p.border,
    )
}

fn token_breakdown_line(
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
    theme: &str,
    y: f64,
) -> String {
    let p = palette(theme);
    let cache = cache_read.saturating_add(cache_creation);
    format!(
        r#"<text x="24" y="{y:.1}" font-family="ui-sans-serif,system-ui,sans-serif" font-size="12" fill="{muted}">in {input} · out {output} · cache {cache}</text>"#,
        muted = p.muted,
        input = escape_xml(&format_compact_number(input)),
        output = escape_xml(&format_compact_number(output)),
        cache = escape_xml(&format_compact_number(cache)),
    )
}

pub fn render_global_usage_svg(
    data: &GlobalUsageResponse,
    theme: &str,
    period: &str,
    router_host: &str,
) -> String {
    let theme = normalize_theme(theme);
    let period = normalize_period_label(if period.trim().is_empty() {
        data.period.as_str()
    } else {
        period
    });
    let p = palette(theme);
    let model_limit = data.models.len().max(1);
    let height = 168.0 + (model_limit as f64) * 22.0 + 28.0;
    let host_label = escape_xml(router_host.trim().trim_end_matches('.'));
    let title = format!(
        r#"<text x="24" y="34" font-family="ui-sans-serif,system-ui,sans-serif" font-size="16" font-weight="700" fill="{text}">TokenSwitch · {host_label}</text>
{period_links}
<text x="24" y="64" font-family="ui-sans-serif,system-ui,sans-serif" font-size="28" font-weight="700" fill="{accent}">{total} tokens</text>
<text x="24" y="86" font-family="ui-sans-serif,system-ui,sans-serif" font-size="12" fill="{muted}">{period}</text>
{breakdown}
<text x="24" y="126" font-family="ui-sans-serif,system-ui,sans-serif" font-size="12" fill="{muted}">activeShares {shares} · activeClients {clients}</text>"#,
        text = p.text,
        accent = p.accent,
        muted = p.muted,
        period_links = period_links(period, theme),
        total = escape_xml(&format_compact_number(data.total_tokens)),
        breakdown = token_breakdown_line(
            data.input_tokens,
            data.output_tokens,
            data.cache_read_tokens,
            data.cache_creation_tokens,
            theme,
            106.0,
        ),
        shares = data.active_shares,
        clients = data.active_clients,
    );
    let (models, models_end_y) = model_rows_svg(&data.models, theme, data.models.len().max(1), 152.0);
    let footer = footer_svg(theme, models_end_y + 10.0);
    let body = format!("{title}\n{models}\n{footer}");
    card_shell(height.max(models_end_y + 28.0), theme, &body)
}

pub fn render_user_usage_svg(
    username: &str,
    data: &AccountUsageResponse,
    theme: &str,
    period: &str,
) -> String {
    let theme = normalize_theme(theme);
    let period = normalize_period_label(if period.trim().is_empty() {
        data.period.as_str()
    } else {
        period
    });
    let p = palette(theme);
    let model_limit = data.models.len().max(1);
    let height = 148.0 + (model_limit as f64) * 22.0 + 28.0;
    let handle = escape_xml(&format!("@{}", username.trim().trim_start_matches('@')));
    let title = format!(
        r#"<text x="24" y="34" font-family="ui-sans-serif,system-ui,sans-serif" font-size="16" font-weight="700" fill="{text}">TokenSwitch · {handle}</text>
{period_links}
<text x="24" y="64" font-family="ui-sans-serif,system-ui,sans-serif" font-size="28" font-weight="700" fill="{accent}">{total} tokens</text>
<text x="24" y="86" font-family="ui-sans-serif,system-ui,sans-serif" font-size="12" fill="{muted}">{period}</text>
{breakdown}"#,
        text = p.text,
        accent = p.accent,
        muted = p.muted,
        period_links = period_links(period, theme),
        total = escape_xml(&format_compact_number(data.total_tokens)),
        breakdown = token_breakdown_line(
            data.input_tokens,
            data.output_tokens,
            data.cache_read_tokens,
            data.cache_creation_tokens,
            theme,
            106.0,
        ),
    );
    let (models, models_end_y) = model_rows_svg(&data.models, theme, data.models.len().max(1), 132.0);
    let footer = footer_svg(theme, models_end_y + 10.0);
    let body = format!("{title}\n{models}\n{footer}");
    card_shell(height.max(models_end_y + 28.0), theme, &body)
}

pub fn render_usage_error_svg(message: &str, theme: &str) -> String {
    let theme = normalize_theme(theme);
    let p = palette(theme);
    let body = format!(
        r#"<text x="24" y="40" font-family="ui-sans-serif,system-ui,sans-serif" font-size="16" font-weight="700" fill="{text}">TokenSwitch · Usage</text>
{period_links}
<text x="24" y="96" font-family="ui-sans-serif,system-ui,sans-serif" font-size="14" fill="{muted}">{message}</text>
{footer}"#,
        text = p.text,
        muted = p.muted,
        period_links = period_links("7d", theme),
        message = escape_xml(message),
        footer = footer_svg(theme, 128.0),
    );
    card_shell(150.0, theme, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_number_formatting() {
        assert_eq!(format_compact_number(999), "999");
        assert_eq!(format_compact_number(1_200), "1.2K");
        assert_eq!(format_compact_number(3_400_000), "3.4M");
    }

    #[test]
    fn escape_xml_encodes_specials() {
        assert_eq!(
            escape_xml(r#"a&b<c>"d"'e"#),
            "a&amp;b&lt;c&gt;&quot;d&quot;&apos;e"
        );
    }
}
