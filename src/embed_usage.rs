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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedNumberFormat {
    Compact,
    Full,
}

#[derive(Debug, Clone)]
pub struct EmbedRenderOptions {
    pub theme: String,
    pub period: String,
    pub show_breakdown: bool,
    pub show_models: bool,
    pub compact: bool,
    pub format: EmbedNumberFormat,
}

impl Default for EmbedRenderOptions {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            period: "7d".to_string(),
            show_breakdown: true,
            show_models: true,
            compact: false,
            format: EmbedNumberFormat::Compact,
        }
    }
}

impl EmbedRenderOptions {
    pub fn from_query(
        period: Option<&str>,
        theme: Option<&str>,
        show_breakdown: Option<&str>,
        show_models: Option<&str>,
        compact: Option<&str>,
        format: Option<&str>,
    ) -> Self {
        Self {
            theme: normalize_theme(theme.unwrap_or("dark")).to_string(),
            period: normalize_period_label(period.unwrap_or("7d")).to_string(),
            show_breakdown: parse_bool_flag(show_breakdown, true),
            show_models: parse_bool_flag(show_models, true),
            compact: parse_bool_flag(compact, false),
            format: normalize_format(format.unwrap_or("compact")),
        }
    }
}

fn parse_bool_flag(value: Option<&str>, default: bool) -> bool {
    match value.map(|v| v.trim().to_ascii_lowercase()) {
        None => default,
        Some(ref v) if v.is_empty() => default,
        Some(ref v) if matches!(v.as_str(), "0" | "false" | "no" | "off") => false,
        Some(ref v) if matches!(v.as_str(), "1" | "true" | "yes" | "on") => true,
        Some(_) => default,
    }
}

fn palette(theme: &str) -> ThemePalette {
    if theme.eq_ignore_ascii_case("light") {
        ThemePalette {
            bg: "#fafafa",
            border: "#e2e8f0",
            text: "#0f172a",
            muted: "#64748b",
            accent: "#0052FF",
            bar_track: "#e2e8f0",
            chip_bg: "#f1f5f9",
            chip_active_bg: "#0052FF",
            chip_active_text: "#ffffff",
        }
    } else {
        ThemePalette {
            bg: "#0f172a",
            border: "#1e293b",
            text: "#e2e8f0",
            muted: "#94a3b8",
            accent: "#4D7CFF",
            bar_track: "#1e293b",
            chip_bg: "#1e293b",
            chip_active_bg: "#0052FF",
            chip_active_text: "#ffffff",
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

fn normalize_format(format: &str) -> EmbedNumberFormat {
    if format.trim().eq_ignore_ascii_case("full") {
        EmbedNumberFormat::Full
    } else {
        EmbedNumberFormat::Compact
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
            let precision = if scaled >= 100.0 { 0 } else { 1 };
            let text = format!("{scaled:.precision$}");
            let trimmed = text.trim_end_matches('0').trim_end_matches('.');
            return format!("{trimmed}{suffix}");
        }
    }
    value.to_string()
}

fn format_number(value: u64, format: EmbedNumberFormat) -> String {
    match format {
        EmbedNumberFormat::Compact => format_compact_number(value),
        EmbedNumberFormat::Full => {
            let raw = value.to_string();
            let mut out = String::new();
            for (idx, ch) in raw.chars().rev().enumerate() {
                if idx > 0 && idx % 3 == 0 {
                    out.push(',');
                }
                out.push(ch);
            }
            out.chars().rev().collect()
        }
    }
}

fn query_suffix(opts: &EmbedRenderOptions) -> String {
    format!(
        "theme={}&amp;showBreakdown={}&amp;showModels={}&amp;compact={}&amp;format={}",
        opts.theme,
        if opts.show_breakdown { "1" } else { "0" },
        if opts.show_models { "1" } else { "0" },
        if opts.compact { "1" } else { "0" },
        match opts.format {
            EmbedNumberFormat::Compact => "compact",
            EmbedNumberFormat::Full => "full",
        },
    )
}

fn period_links(opts: &EmbedRenderOptions) -> String {
    let theme = normalize_theme(&opts.theme);
    let current = normalize_period_label(&opts.period);
    let p = palette(theme);
    let suffix = query_suffix(opts);
    let chip_h = if opts.compact { 18.0 } else { 22.0 };
    let chip_w = if opts.compact { 38.0 } else { 44.0 };
    let font = if opts.compact { 10 } else { 11 };
    let mut out = String::new();
    let mut x = if opts.compact { 540.0_f64 } else { 520.0_f64 };
    for period in ["24h", "7d", "30d"] {
        let active = period == current;
        let bg = if active { p.chip_active_bg } else { p.chip_bg };
        let fg = if active { p.chip_active_text } else { p.muted };
        out.push_str(&format!(
            r#"<a href="?period={period}&amp;{suffix}"><rect x="{x:.1}" y="18" width="{chip_w:.0}" height="{chip_h:.0}" rx="6" fill="{bg}"/><text x="{label_x:.1}" y="{label_y:.1}" text-anchor="middle" font-family="ui-sans-serif,system-ui,sans-serif" font-size="{font}" font-weight="600" fill="{fg}">{period}</text></a>"#,
            label_x = x + chip_w / 2.0,
            label_y = 18.0 + chip_h * 0.68,
        ));
        x += chip_w + 6.0;
    }
    out
}

fn model_rows_svg(
    models: &[UsageModelRow],
    opts: &EmbedRenderOptions,
    max_rows: usize,
    start_y: f64,
) -> (String, f64) {
    let p = palette(&opts.theme);
    let rows = models.iter().take(max_rows).collect::<Vec<_>>();
    let max_total = rows
        .iter()
        .map(|row| row.total_tokens)
        .max()
        .unwrap_or(0)
        .max(1);
    let row_h = if opts.compact { 18.0 } else { 22.0 };
    let font = if opts.compact { 11 } else { 12 };
    let bar_h = if opts.compact { 6.0 } else { 8.0 };
    let mut out = String::new();
    let mut y = start_y;
    if rows.is_empty() {
        out.push_str(&format!(
            r#"<text x="24" y="{y:.1}" font-family="ui-sans-serif,system-ui,sans-serif" font-size="{font}" fill="{muted}">No model usage in this period</text>"#,
            muted = p.muted,
        ));
        return (out, y + 16.0);
    }
    for row in rows {
        let ratio = row.total_tokens as f64 / max_total as f64;
        let bar_w = (ratio * 280.0).max(2.0);
        let model = escape_xml(&truncate_label(
            &row.model,
            if opts.compact { 24 } else { 28 },
        ));
        let total = escape_xml(&format_number(row.total_tokens, opts.format));
        out.push_str(&format!(
            r#"<text x="24" y="{y:.1}" font-family="ui-monospace,SFMono-Regular,Menlo,monospace" font-size="{font}" fill="{text}">{model}</text>
<rect x="300" y="{bar_y:.1}" width="280" height="{bar_h:.0}" rx="4" fill="{track}"/>
<rect x="300" y="{bar_y:.1}" width="{bar_w:.1}" height="{bar_h:.0}" rx="4" fill="{accent}"/>
<text x="656" y="{y:.1}" text-anchor="end" font-family="ui-sans-serif,system-ui,sans-serif" font-size="{font}" fill="{muted}">{total}</text>"#,
            text = p.text,
            track = p.bar_track,
            accent = p.accent,
            muted = p.muted,
            bar_y = y - bar_h * 0.9,
        ));
        y += row_h;
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

fn footer_svg(theme: &str, y: f64, compact: bool) -> String {
    let p = palette(theme);
    let updated = Utc::now().format("%Y-%m-%d %H:%M UTC");
    let font = if compact { 10 } else { 11 };
    format!(
        r#"<text x="24" y="{y:.1}" font-family="ui-sans-serif,system-ui,sans-serif" font-size="{font}" fill="{muted}">updated {updated}</text>"#,
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
    opts: &EmbedRenderOptions,
    y: f64,
) -> String {
    let p = palette(&opts.theme);
    let cache = cache_read.saturating_add(cache_creation);
    let font = if opts.compact { 11 } else { 12 };
    format!(
        r#"<text x="24" y="{y:.1}" font-family="ui-sans-serif,system-ui,sans-serif" font-size="{font}" fill="{muted}">in {input} · out {output} · cache {cache}</text>"#,
        muted = p.muted,
        input = escape_xml(&format_number(input, opts.format)),
        output = escape_xml(&format_number(output, opts.format)),
        cache = escape_xml(&format_number(cache, opts.format)),
    )
}

pub fn render_global_usage_svg(
    data: &GlobalUsageResponse,
    opts: &EmbedRenderOptions,
    router_host: &str,
) -> String {
    let mut opts = opts.clone();
    opts.theme = normalize_theme(&opts.theme).to_string();
    opts.period = normalize_period_label(if opts.period.trim().is_empty() {
        data.period.as_str()
    } else {
        opts.period.as_str()
    })
    .to_string();
    let p = palette(&opts.theme);
    let title_size = if opts.compact { 14 } else { 16 };
    let total_size = if opts.compact { 22 } else { 28 };
    let host_label = escape_xml(router_host.trim().trim_end_matches('.'));
    let mut y = if opts.compact { 30.0 } else { 34.0 };
    let mut body = format!(
        r#"<text x="24" y="{y:.1}" font-family="ui-sans-serif,system-ui,sans-serif" font-size="{title_size}" font-weight="700" fill="{text}">TokenSwitch · {host_label}</text>
{period_links}"#,
        text = p.text,
        period_links = period_links(&opts),
    );
    y = if opts.compact { 54.0 } else { 64.0 };
    body.push_str(&format!(
        r#"
<text x="24" y="{y:.1}" font-family="ui-sans-serif,system-ui,sans-serif" font-size="{total_size}" font-weight="700" fill="{accent}">{total} tokens</text>"#,
        accent = p.accent,
        total = escape_xml(&format_number(data.total_tokens, opts.format)),
    ));
    y += if opts.compact { 18.0 } else { 22.0 };
    body.push_str(&format!(
        r#"
<text x="24" y="{y:.1}" font-family="ui-sans-serif,system-ui,sans-serif" font-size="12" fill="{muted}">{period}</text>"#,
        muted = p.muted,
        period = escape_xml(&opts.period),
    ));
    if opts.show_breakdown {
        y += if opts.compact { 16.0 } else { 20.0 };
        body.push('\n');
        body.push_str(&token_breakdown_line(
            data.input_tokens,
            data.output_tokens,
            data.cache_read_tokens,
            data.cache_creation_tokens,
            &opts,
            y,
        ));
    }
    y += if opts.compact { 18.0 } else { 20.0 };
    body.push_str(&format!(
        r#"
<text x="24" y="{y:.1}" font-family="ui-sans-serif,system-ui,sans-serif" font-size="12" fill="{muted}">activeShares {shares} · activeClients {clients}</text>"#,
        muted = p.muted,
        shares = data.active_shares,
        clients = data.active_clients,
    ));
    let mut end_y = y + 12.0;
    if opts.show_models {
        let (models, models_end_y) =
            model_rows_svg(&data.models, &opts, data.models.len().max(1), end_y + 12.0);
        body.push('\n');
        body.push_str(&models);
        end_y = models_end_y;
    }
    let footer = footer_svg(&opts.theme, end_y + 10.0, opts.compact);
    body.push('\n');
    body.push_str(&footer);
    card_shell((end_y + 28.0).max(120.0), &opts.theme, &body)
}

pub fn render_user_usage_svg(
    username: &str,
    data: &AccountUsageResponse,
    opts: &EmbedRenderOptions,
) -> String {
    let mut opts = opts.clone();
    opts.theme = normalize_theme(&opts.theme).to_string();
    opts.period = normalize_period_label(if opts.period.trim().is_empty() {
        data.period.as_str()
    } else {
        opts.period.as_str()
    })
    .to_string();
    let p = palette(&opts.theme);
    let title_size = if opts.compact { 14 } else { 16 };
    let total_size = if opts.compact { 22 } else { 28 };
    let handle = escape_xml(&format!("@{}", username.trim().trim_start_matches('@')));
    let mut y = if opts.compact { 30.0 } else { 34.0 };
    let mut body = format!(
        r#"<text x="24" y="{y:.1}" font-family="ui-sans-serif,system-ui,sans-serif" font-size="{title_size}" font-weight="700" fill="{text}">TokenSwitch · {handle}</text>
{period_links}"#,
        text = p.text,
        period_links = period_links(&opts),
    );
    y = if opts.compact { 54.0 } else { 64.0 };
    body.push_str(&format!(
        r#"
<text x="24" y="{y:.1}" font-family="ui-sans-serif,system-ui,sans-serif" font-size="{total_size}" font-weight="700" fill="{accent}">{total} tokens</text>"#,
        accent = p.accent,
        total = escape_xml(&format_number(data.total_tokens, opts.format)),
    ));
    y += if opts.compact { 18.0 } else { 22.0 };
    body.push_str(&format!(
        r#"
<text x="24" y="{y:.1}" font-family="ui-sans-serif,system-ui,sans-serif" font-size="12" fill="{muted}">{period}</text>"#,
        muted = p.muted,
        period = escape_xml(&opts.period),
    ));
    if opts.show_breakdown {
        y += if opts.compact { 16.0 } else { 20.0 };
        body.push('\n');
        body.push_str(&token_breakdown_line(
            data.input_tokens,
            data.output_tokens,
            data.cache_read_tokens,
            data.cache_creation_tokens,
            &opts,
            y,
        ));
    }
    let mut end_y = y + 12.0;
    if opts.show_models {
        let (models, models_end_y) =
            model_rows_svg(&data.models, &opts, data.models.len().max(1), end_y + 8.0);
        body.push('\n');
        body.push_str(&models);
        end_y = models_end_y;
    }
    let footer = footer_svg(&opts.theme, end_y + 10.0, opts.compact);
    body.push('\n');
    body.push_str(&footer);
    card_shell((end_y + 28.0).max(110.0), &opts.theme, &body)
}

pub fn render_usage_error_svg(message: &str, opts: &EmbedRenderOptions) -> String {
    let theme = normalize_theme(&opts.theme);
    let p = palette(theme);
    let mut opts = opts.clone();
    opts.theme = theme.to_string();
    let body = format!(
        r#"<text x="24" y="40" font-family="ui-sans-serif,system-ui,sans-serif" font-size="16" font-weight="700" fill="{text}">TokenSwitch · Usage</text>
{period_links}
<text x="24" y="96" font-family="ui-sans-serif,system-ui,sans-serif" font-size="14" fill="{muted}">{message}</text>
{footer}"#,
        text = p.text,
        muted = p.muted,
        period_links = period_links(&opts),
        message = escape_xml(message),
        footer = footer_svg(theme, 128.0, opts.compact),
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
    fn full_number_formatting() {
        assert_eq!(
            format_number(1_234_567, EmbedNumberFormat::Full),
            "1,234,567"
        );
    }

    #[test]
    fn escape_xml_encodes_specials() {
        assert_eq!(
            escape_xml(r#"a&b<c>"d"'e"#),
            "a&amp;b&lt;c&gt;&quot;d&quot;&apos;e"
        );
    }

    #[test]
    fn bool_flag_defaults() {
        assert!(parse_bool_flag(None, true));
        assert!(!parse_bool_flag(Some("0"), true));
        assert!(parse_bool_flag(Some("true"), false));
        assert!(!parse_bool_flag(Some("off"), true));
    }

    #[test]
    fn render_hides_optional_blocks() {
        let data = AccountUsageResponse {
            period: "7d".into(),
            bucket_granularity: "day".into(),
            days: 7,
            total_tokens: 1200,
            input_tokens: 800,
            output_tokens: 300,
            cache_read_tokens: 80,
            cache_creation_tokens: 20,
            models: vec![UsageModelRow {
                model: "claude".into(),
                total_tokens: 1200,
                input_tokens: 800,
                output_tokens: 300,
                cache_read_tokens: 80,
                cache_creation_tokens: 20,
            }],
            daily: vec![],
            by_share: vec![],
        };
        let mut opts = EmbedRenderOptions::default();
        opts.show_breakdown = false;
        opts.show_models = false;
        let svg = render_user_usage_svg("alice", &data, &opts);
        assert!(!svg.contains("in "));
        assert!(!svg.contains("claude"));
        assert!(svg.contains("1.2K tokens"));
    }
}
