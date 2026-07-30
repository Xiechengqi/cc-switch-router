use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::error::AppError;
use crate::models::{
    AccountUsageResponse, GlobalUsageResponse, ProviderInstallationUsage, ProviderShareUsage,
    ProviderUsageResponse, UpdateUserProfileRequest, UsageCallerRow, UsageDailyBucket,
    UsageModelRow, UsageShareRow, UserProfilePublic, UserProfileResponse,
};
use crate::store::AppStore;

const ACTIVE_CLIENT_WINDOW_MINUTES: i64 = 15;
const TOP_MODELS_LIMIT: usize = 32;
const TOP_CALLERS_LIMIT: usize = 16;

struct AccountUsagePeriodWindow {
    period: String,
    bucket_granularity: String,
    days: u32,
    start_at: DateTime<Utc>,
    bucket_keys: Vec<String>,
}

#[derive(Default, Clone)]
struct TokenAgg {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
}

impl TokenAgg {
    fn add(&mut self, input: u64, output: u64, cache_read: u64, cache_creation: u64) {
        self.input += input;
        self.output += output;
        self.cache_read += cache_read;
        self.cache_creation += cache_creation;
    }

    fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_creation
    }

    fn to_model_row(&self, model: &str) -> UsageModelRow {
        UsageModelRow {
            model: model.to_string(),
            input_tokens: self.input,
            output_tokens: self.output,
            cache_read_tokens: self.cache_read,
            cache_creation_tokens: self.cache_creation,
            total_tokens: self.total(),
        }
    }

    fn to_daily(&self, date: &str) -> UsageDailyBucket {
        UsageDailyBucket {
            date: date.to_string(),
            input_tokens: self.input,
            output_tokens: self.output,
            cache_read_tokens: self.cache_read,
            cache_creation_tokens: self.cache_creation,
            total_tokens: self.total(),
        }
    }
}

pub fn init_schema(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS user_profiles (
            user_id TEXT PRIMARY KEY,
            username TEXT,
            username_normalized TEXT UNIQUE,
            public_stats_enabled INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_share_request_logs_user_email_created
            ON share_request_logs(user_email, created_at)
            WHERE user_email IS NOT NULL AND is_health_check = 0;

        CREATE INDEX IF NOT EXISTS idx_market_request_logs_user_email_created
            ON market_request_logs(user_email, created_at)
            WHERE user_email IS NOT NULL;
        ",
    )
    .map_err(|e| AppError::Internal(format!("init usage account schema failed: {e}")))?;
    Ok(())
}

fn normalize_account_usage_period(value: &str) -> Result<AccountUsagePeriodWindow, AppError> {
    let now = Utc::now();
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "24h" | "1d" | "1天" => {
            let start_at = now - Duration::hours(24);
            let bucket_keys = account_usage_hour_keys(start_at, now);
            Ok(AccountUsagePeriodWindow {
                period: "24h".to_string(),
                bucket_granularity: "hour".to_string(),
                days: account_usage_date_keys(start_at, now).len() as u32,
                start_at,
                bucket_keys,
            })
        }
        "1w" | "7d" => account_usage_day_window("7d", 7, now),
        "30d" | "30天" => account_usage_day_window("30d", 30, now),
        _ => Err(AppError::BadRequest(
            "usage period must be 24h, 7d, or 30d".into(),
        )),
    }
}

fn account_usage_day_window(
    period: &str,
    days: u32,
    now: DateTime<Utc>,
) -> Result<AccountUsagePeriodWindow, AppError> {
    let today = now.date_naive();
    let start_date = today - Duration::days(i64::from(days.saturating_sub(1)));
    let start_at = start_date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| AppError::Internal("invalid usage start date".into()))?
        .and_utc();
    let date_keys = (0..days)
        .map(|offset| {
            (start_date + Duration::days(i64::from(offset)))
                .format("%Y-%m-%d")
                .to_string()
        })
        .collect::<Vec<_>>();
    Ok(AccountUsagePeriodWindow {
        period: period.to_string(),
        bucket_granularity: "day".to_string(),
        days,
        start_at,
        bucket_keys: date_keys,
    })
}

fn account_usage_hour_keys(start_at: DateTime<Utc>, end_at: DateTime<Utc>) -> Vec<String> {
    let mut bucket = floor_to_utc_hour(start_at);
    let end = floor_to_utc_hour(end_at);
    let mut keys = Vec::new();
    while bucket <= end {
        keys.push(bucket.format("%Y-%m-%dT%H:00:00Z").to_string());
        bucket += Duration::hours(1);
    }
    keys
}

fn floor_to_utc_hour(value: DateTime<Utc>) -> DateTime<Utc> {
    let timestamp = value.timestamp();
    let floored = timestamp - timestamp.rem_euclid(3600);
    DateTime::<Utc>::from_timestamp(floored, 0).unwrap_or(value)
}

fn account_usage_date_keys(start_at: DateTime<Utc>, end_at: DateTime<Utc>) -> Vec<String> {
    let mut date = start_at.date_naive();
    let end = end_at.date_naive();
    let mut keys = Vec::new();
    while date <= end {
        keys.push(date.format("%Y-%m-%d").to_string());
        date += Duration::days(1);
    }
    keys
}

fn normalize_profile_email(value: &str) -> Result<String, AppError> {
    let email = value.trim().to_ascii_lowercase();
    let Some((local, domain)) = email.split_once('@') else {
        return Err(AppError::BadRequest("invalid email".into()));
    };
    if local.is_empty() || domain.is_empty() || !domain.contains('.') {
        return Err(AppError::BadRequest("invalid email".into()));
    }
    if email.len() > 254 {
        return Err(AppError::BadRequest("invalid email".into()));
    }
    Ok(email)
}

fn validate_username(value: &str) -> Result<(String, String), AppError> {
    let display = value.trim();
    if display.is_empty() {
        return Err(AppError::BadRequest("username is required".into()));
    }
    if !(3..=32).contains(&display.len()) {
        return Err(AppError::BadRequest(
            "username must be 3-32 characters".into(),
        ));
    }
    if !display
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(AppError::BadRequest(
            "username may only contain letters, digits, hyphen, and underscore".into(),
        ));
    }
    Ok((display.to_string(), display.to_ascii_lowercase()))
}

fn market_input_tokens_expr(alias: &str) -> String {
    let prefix = if alias.is_empty() {
        String::new()
    } else {
        format!("{alias}.")
    };
    format!(
        "CASE
            WHEN lower(COALESCE({prefix}request_agent, '')) = 'codex' THEN
                CASE
                    WHEN COALESCE({prefix}input_tokens, 0) > COALESCE({prefix}cache_read_tokens, 0)
                    THEN COALESCE({prefix}input_tokens, 0) - COALESCE({prefix}cache_read_tokens, 0)
                    ELSE 0
                END
            ELSE COALESCE({prefix}input_tokens, 0)
        END"
    )
}

fn market_total_tokens_expr(alias: &str) -> String {
    let prefix = if alias.is_empty() {
        String::new()
    } else {
        format!("{alias}.")
    };
    let input_expr = market_input_tokens_expr(alias);
    format!(
        "({input_expr}
          + COALESCE({prefix}output_tokens, 0)
          + COALESCE({prefix}cache_read_tokens, 0)
          + COALESCE({prefix}cache_creation_tokens, 0))"
    )
}

fn share_model_expr(alias: &str) -> String {
    let prefix = if alias.is_empty() {
        String::new()
    } else {
        format!("{alias}.")
    };
    format!(
        "COALESCE(NULLIF(trim({prefix}actual_model), ''), NULLIF(trim({prefix}model), ''), 'unknown')"
    )
}

fn market_model_expr(alias: &str) -> String {
    let prefix = if alias.is_empty() {
        String::new()
    } else {
        format!("{alias}.")
    };
    format!(
        "COALESCE(\
            NULLIF(trim({prefix}actual_model), ''), \
            NULLIF(trim({prefix}model), ''), \
            NULLIF(trim({prefix}requested_model), ''), \
            'unknown')"
    )
}

fn ensure_user_id(conn: &Connection, email: &str) -> Result<String, AppError> {
    let now = Utc::now().to_rfc3339();
    if let Some(id) = conn
        .query_row(
            "SELECT id FROM users WHERE email_normalized = ?1",
            params![email],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| AppError::Internal(format!("query user for profile failed: {e}")))?
    {
        return Ok(id);
    }
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO users (id, email_normalized, status, created_at, last_login_at)
         VALUES (?1, ?2, 'active', ?3, ?4)",
        params![id, email, now, now],
    )
    .map_err(|e| AppError::Internal(format!("insert user for profile failed: {e}")))?;
    Ok(id)
}

fn load_profile_row(
    conn: &Connection,
    user_id: &str,
) -> Result<Option<(Option<String>, bool, String)>, AppError> {
    conn.query_row(
        "SELECT username, public_stats_enabled, updated_at
         FROM user_profiles WHERE user_id = ?1",
        params![user_id],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, i64>(1)? != 0,
                row.get::<_, String>(2)?,
            ))
        },
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query user profile failed: {e}")))
}

fn profile_response(
    email: &str,
    username: Option<String>,
    public_stats_enabled: bool,
    updated_at: Option<String>,
) -> UserProfileResponse {
    UserProfileResponse {
        email: email.to_string(),
        username: username.filter(|value| !value.trim().is_empty()),
        public_stats_enabled,
        updated_at,
    }
}

#[cfg(test)]
fn empty_daily(bucket_keys: &[String]) -> Vec<UsageDailyBucket> {
    bucket_keys
        .iter()
        .map(|date| UsageDailyBucket {
            date: date.clone(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            total_tokens: 0,
        })
        .collect()
}

fn finish_models(models: BTreeMap<String, TokenAgg>, limit: usize) -> Vec<UsageModelRow> {
    let mut rows = models
        .into_iter()
        .map(|(model, agg)| agg.to_model_row(&model))
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        b.total_tokens
            .cmp(&a.total_tokens)
            .then_with(|| a.model.cmp(&b.model))
    });
    rows.truncate(limit);
    rows
}

fn finish_daily(
    bucket_keys: &[String],
    mut daily: BTreeMap<String, TokenAgg>,
) -> Vec<UsageDailyBucket> {
    bucket_keys
        .iter()
        .map(|key| {
            daily
                .remove(key)
                .map(|agg| agg.to_daily(key))
                .unwrap_or_else(|| UsageDailyBucket {
                    date: key.clone(),
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                    total_tokens: 0,
                })
        })
        .collect()
}

type UsageEvent = (String, String, String, String, u64, u64, u64, u64);

fn apply_usage_event(
    totals: &mut TokenAgg,
    models: &mut BTreeMap<String, TokenAgg>,
    daily: &mut BTreeMap<String, TokenAgg>,
    by_share: &mut BTreeMap<String, (String, TokenAgg, BTreeMap<String, TokenAgg>)>,
    share_id: &str,
    share_name: &str,
    model: &str,
    bucket: &str,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
) {
    totals.add(input, output, cache_read, cache_creation);
    models
        .entry(model.to_string())
        .or_default()
        .add(input, output, cache_read, cache_creation);
    daily
        .entry(bucket.to_string())
        .or_default()
        .add(input, output, cache_read, cache_creation);
    if !share_id.is_empty() {
        let entry = by_share
            .entry(share_id.to_string())
            .or_insert_with(|| (share_name.to_string(), TokenAgg::default(), BTreeMap::new()));
        if entry.0.is_empty() && !share_name.is_empty() {
            entry.0 = share_name.to_string();
        }
        entry.1.add(input, output, cache_read, cache_creation);
        entry.2.entry(model.to_string()).or_default().add(
            input,
            output,
            cache_read,
            cache_creation,
        );
    }
}

fn build_account_usage(
    window: &AccountUsagePeriodWindow,
    events: impl IntoIterator<Item = UsageEvent>,
    include_by_share: bool,
) -> AccountUsageResponse {
    let mut totals = TokenAgg::default();
    let mut models = BTreeMap::<String, TokenAgg>::new();
    let mut daily = BTreeMap::<String, TokenAgg>::new();
    let mut by_share = BTreeMap::<String, (String, TokenAgg, BTreeMap<String, TokenAgg>)>::new();

    for (share_id, share_name, model, bucket, input, output, cache_read, cache_creation) in events {
        apply_usage_event(
            &mut totals,
            &mut models,
            &mut daily,
            &mut by_share,
            &share_id,
            &share_name,
            &model,
            &bucket,
            input,
            output,
            cache_read,
            cache_creation,
        );
    }

    let mut by_share_rows = if include_by_share {
        by_share
            .into_iter()
            .map(|(share_id, (share_name, agg, models))| UsageShareRow {
                share_id,
                share_name,
                input_tokens: agg.input,
                output_tokens: agg.output,
                cache_read_tokens: agg.cache_read,
                cache_creation_tokens: agg.cache_creation,
                total_tokens: agg.total(),
                models: finish_models(models, 8),
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    by_share_rows.sort_by(|a, b| {
        b.total_tokens
            .cmp(&a.total_tokens)
            .then_with(|| a.share_id.cmp(&b.share_id))
    });

    AccountUsageResponse {
        period: window.period.clone(),
        bucket_granularity: window.bucket_granularity.clone(),
        days: window.days,
        input_tokens: totals.input,
        output_tokens: totals.output,
        cache_read_tokens: totals.cache_read,
        cache_creation_tokens: totals.cache_creation,
        total_tokens: totals.total(),
        models: finish_models(models, TOP_MODELS_LIMIT),
        daily: finish_daily(&window.bucket_keys, daily),
        by_share: by_share_rows,
    }
}

fn query_consumer_events(
    conn: &Connection,
    email: Option<&str>,
    window: &AccountUsagePeriodWindow,
) -> Result<Vec<UsageEvent>, AppError> {
    let start_ts = window.start_at.timestamp();
    let start_rfc3339 = window.start_at.to_rfc3339();
    let market_bucket_expr = if window.bucket_granularity == "hour" {
        "strftime('%Y-%m-%dT%H:00:00Z', ml.created_at)"
    } else {
        "date(ml.created_at)"
    };
    let share_bucket_expr = if window.bucket_granularity == "hour" {
        "strftime('%Y-%m-%dT%H:00:00Z', sl.created_at, 'unixepoch')"
    } else {
        "date(sl.created_at, 'unixepoch')"
    };
    let market_input = market_input_tokens_expr("ml");
    let market_total = market_total_tokens_expr("ml");
    let market_model = market_model_expr("ml");
    let share_model = share_model_expr("sl");

    let (market_email_filter, share_email_filter, start_placeholder) = if email.is_some() {
        (
            "AND lower(trim(ml.user_email)) = lower(trim(?1))",
            "AND lower(trim(sl.user_email)) = lower(trim(?1))",
            "?2",
        )
    } else {
        (
            "AND ml.user_email IS NOT NULL AND trim(ml.user_email) != ''",
            "AND sl.user_email IS NOT NULL AND trim(sl.user_email) != ''",
            "?1",
        )
    };

    let market_sql = format!(
        "SELECT COALESCE(ml.share_id, ''),
                COALESCE(NULLIF(trim(s.share_name), ''), COALESCE(ml.share_subdomain, ''), ''),
                {market_model} AS usage_model,
                {market_bucket_expr} AS usage_bucket,
                COALESCE(SUM(CASE WHEN ({market_total}) > 0 OR sl.request_id IS NULL THEN {market_input} ELSE COALESCE(sl.input_tokens, 0) END), 0),
                COALESCE(SUM(CASE WHEN ({market_total}) > 0 OR sl.request_id IS NULL THEN COALESCE(ml.output_tokens, 0) ELSE COALESCE(sl.output_tokens, 0) END), 0),
                COALESCE(SUM(CASE WHEN ({market_total}) > 0 OR sl.request_id IS NULL THEN COALESCE(ml.cache_read_tokens, 0) ELSE COALESCE(sl.cache_read_tokens, 0) END), 0),
                COALESCE(SUM(CASE WHEN ({market_total}) > 0 OR sl.request_id IS NULL THEN COALESCE(ml.cache_creation_tokens, 0) ELSE COALESCE(sl.cache_creation_tokens, 0) END), 0)
         FROM market_request_logs ml
         LEFT JOIN shares s ON s.share_id = ml.share_id
         LEFT JOIN share_request_logs sl
           ON sl.request_id = ml.request_id
          AND sl.share_id = ml.share_id
          AND sl.is_health_check = 0
         WHERE ml.created_at >= {start_placeholder}
           {market_email_filter}
         GROUP BY COALESCE(ml.share_id, ''), usage_model, usage_bucket,
                  COALESCE(NULLIF(trim(s.share_name), ''), COALESCE(ml.share_subdomain, ''), '')"
    );

    let share_sql = format!(
        "SELECT COALESCE(sl.share_id, ''),
                COALESCE(NULLIF(trim(sl.share_name), ''), ''),
                {share_model} AS usage_model,
                {share_bucket_expr} AS usage_bucket,
                COALESCE(SUM(sl.input_tokens), 0),
                COALESCE(SUM(sl.output_tokens), 0),
                COALESCE(SUM(sl.cache_read_tokens), 0),
                COALESCE(SUM(sl.cache_creation_tokens), 0)
         FROM share_request_logs sl
         WHERE sl.created_at >= {start_placeholder}
           AND sl.is_health_check = 0
           {share_email_filter}
           AND NOT EXISTS (
                SELECT 1
                FROM market_request_logs ml
                WHERE ml.request_id = sl.request_id
                  AND COALESCE(ml.share_id, '') = sl.share_id
                  AND ml.user_email IS NOT NULL
                  AND trim(ml.user_email) != ''
           )
         GROUP BY COALESCE(sl.share_id, ''), usage_model, usage_bucket,
                  COALESCE(NULLIF(trim(sl.share_name), ''), '')"
    );

    let mut events = Vec::new();

    {
        let mut stmt = conn.prepare(&market_sql).map_err(|e| {
            AppError::Internal(format!("prepare consumer market usage failed: {e}"))
        })?;
        let mapped = if let Some(email) = email {
            stmt.query_map(params![email, start_rfc3339], map_usage_event_row)
        } else {
            stmt.query_map(params![start_rfc3339], map_usage_event_row)
        }
        .map_err(|e| AppError::Internal(format!("query consumer market usage failed: {e}")))?;
        for row in mapped {
            events.push(
                row.map_err(|e| AppError::Internal(format!("read market usage row failed: {e}")))?,
            );
        }
    }

    {
        let mut stmt = conn
            .prepare(&share_sql)
            .map_err(|e| AppError::Internal(format!("prepare consumer share usage failed: {e}")))?;
        let mapped = if let Some(email) = email {
            stmt.query_map(params![email, start_ts], map_usage_event_row)
        } else {
            stmt.query_map(params![start_ts], map_usage_event_row)
        }
        .map_err(|e| AppError::Internal(format!("query consumer share usage failed: {e}")))?;
        for row in mapped {
            events.push(
                row.map_err(|e| AppError::Internal(format!("read share usage row failed: {e}")))?,
            );
        }
    }

    Ok(events)
}

fn map_usage_event_row(row: &rusqlite::Row<'_>) -> Result<UsageEvent, rusqlite::Error> {
    Ok((
        row.get::<_, String>(0)?,
        row.get::<_, String>(1)?,
        row.get::<_, String>(2)?,
        row.get::<_, String>(3)?,
        row.get::<_, i64>(4)?.max(0) as u64,
        row.get::<_, i64>(5)?.max(0) as u64,
        row.get::<_, i64>(6)?.max(0) as u64,
        row.get::<_, i64>(7)?.max(0) as u64,
    ))
}

fn count_active_network(conn: &Connection) -> Result<(usize, usize), AppError> {
    let active_cutoff = (Utc::now() - Duration::minutes(ACTIVE_CLIENT_WINDOW_MINUTES)).to_rfc3339();
    let active_shares: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM shares WHERE share_status = 'active'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| AppError::Internal(format!("count active shares failed: {e}")))?;
    let active_clients: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT i.id)
             FROM installations i
             INNER JOIN shares s ON s.installation_id = i.id
             WHERE i.last_seen_at >= ?1
               AND s.share_status = 'active'",
            params![active_cutoff],
            |row| row.get(0),
        )
        .map_err(|e| AppError::Internal(format!("count active clients failed: {e}")))?;
    Ok((
        active_shares.max(0) as usize,
        active_clients.max(0) as usize,
    ))
}

fn installation_label(platform: &str, app_version: &str, subdomain: Option<&str>) -> String {
    if let Some(subdomain) = subdomain.map(str::trim).filter(|value| !value.is_empty()) {
        return subdomain.to_string();
    }
    let platform = platform.trim();
    let app_version = app_version.trim();
    match (platform.is_empty(), app_version.is_empty()) {
        (false, false) => format!("{platform} {app_version}"),
        (false, true) => platform.to_string(),
        (true, false) => app_version.to_string(),
        (true, true) => "installation".to_string(),
    }
}

impl AppStore {
    pub async fn get_user_profile(&self, email: &str) -> Result<UserProfileResponse, AppError> {
        let email = normalize_profile_email(email)?;
        let conn = self.conn.lock().await;
        let user_id = ensure_user_id(&conn, &email)?;
        match load_profile_row(&conn, &user_id)? {
            Some((username, public_stats_enabled, updated_at)) => Ok(profile_response(
                &email,
                username,
                public_stats_enabled,
                Some(updated_at),
            )),
            None => Ok(profile_response(&email, None, false, None)),
        }
    }

    pub async fn update_user_profile(
        &self,
        email: &str,
        patch: UpdateUserProfileRequest,
    ) -> Result<UserProfileResponse, AppError> {
        let email = normalize_profile_email(email)?;
        if patch.username.is_none() && patch.public_stats_enabled.is_none() {
            return Err(AppError::BadRequest("no profile fields to update".into()));
        }
        let conn = self.conn.lock().await;
        let user_id = ensure_user_id(&conn, &email)?;
        let now = Utc::now().to_rfc3339();
        let existing = load_profile_row(&conn, &user_id)?;
        let (mut username, mut username_normalized, mut public_stats_enabled) = match &existing {
            Some((username, enabled, _)) => (
                username.clone(),
                username.as_ref().map(|value| value.to_ascii_lowercase()),
                *enabled,
            ),
            None => (None, None, false),
        };

        if let Some(raw) = patch.username {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                username = None;
                username_normalized = None;
            } else {
                let (display, normalized) = validate_username(trimmed)?;
                if let Some(owner) = conn
                    .query_row(
                        "SELECT user_id FROM user_profiles
                         WHERE username_normalized = ?1 AND user_id != ?2",
                        params![normalized, user_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(|e| {
                        AppError::Internal(format!("check username uniqueness failed: {e}"))
                    })?
                {
                    let _ = owner;
                    return Err(AppError::Conflict("username is already taken".into()));
                }
                username = Some(display);
                username_normalized = Some(normalized);
            }
        }

        if let Some(enabled) = patch.public_stats_enabled {
            if enabled
                && username
                    .as_ref()
                    .map(|v| v.trim().is_empty())
                    .unwrap_or(true)
            {
                return Err(AppError::BadRequest(
                    "username is required to enable public stats".into(),
                ));
            }
            public_stats_enabled = enabled;
        }

        let created_at = existing
            .as_ref()
            .map(|(_, _, updated_at)| updated_at.clone())
            .unwrap_or_else(|| now.clone());
        // Prefer original created_at when present.
        let created_at = conn
            .query_row(
                "SELECT created_at FROM user_profiles WHERE user_id = ?1",
                params![user_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("query profile created_at failed: {e}")))?
            .unwrap_or(created_at);

        conn.execute(
            "INSERT INTO user_profiles (
                user_id, username, username_normalized, public_stats_enabled, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(user_id) DO UPDATE SET
                username = excluded.username,
                username_normalized = excluded.username_normalized,
                public_stats_enabled = excluded.public_stats_enabled,
                updated_at = excluded.updated_at",
            params![
                user_id,
                username,
                username_normalized,
                i64::from(public_stats_enabled),
                created_at,
                now,
            ],
        )
        .map_err(|e| {
            let message = e.to_string();
            if message.contains("UNIQUE") || message.contains("unique") {
                AppError::Conflict("username is already taken".into())
            } else {
                AppError::Internal(format!("upsert user profile failed: {e}"))
            }
        })?;

        Ok(profile_response(
            &email,
            username,
            public_stats_enabled,
            Some(now),
        ))
    }

    pub async fn usage_consumer(
        &self,
        email: &str,
        period: &str,
    ) -> Result<AccountUsageResponse, AppError> {
        let email = normalize_profile_email(email)?;
        let window = normalize_account_usage_period(period)?;
        let conn = self.conn.lock().await;
        let events = query_consumer_events(&conn, Some(&email), &window)?;
        Ok(build_account_usage(&window, events, true))
    }

    pub async fn usage_provider(
        &self,
        email: &str,
        period: &str,
    ) -> Result<ProviderUsageResponse, AppError> {
        let email = normalize_profile_email(email)?;
        let window = normalize_account_usage_period(period)?;
        let start_ts = window.start_at.timestamp();
        let start_rfc3339 = window.start_at.to_rfc3339();
        let conn = self.conn.lock().await;

        let market_input = market_input_tokens_expr("ml");
        let market_total = market_total_tokens_expr("ml");
        let market_model = market_model_expr("ml");
        let share_model = share_model_expr("sl");

        // (installation_id, share_id, share_name, model, caller_email, tokens...)
        type ProvRow = (String, String, String, String, String, u64, u64, u64, u64);
        let mut rows: Vec<ProvRow> = Vec::new();

        let market_sql = format!(
            "SELECT COALESCE(s.installation_id, ''),
                    COALESCE(ml.share_id, ''),
                    COALESCE(NULLIF(trim(s.share_name), ''), COALESCE(ml.share_subdomain, ''), ''),
                    {market_model} AS usage_model,
                    COALESCE(lower(trim(ml.user_email)), ''),
                    COALESCE(SUM(CASE WHEN ({market_total}) > 0 OR sl.request_id IS NULL THEN {market_input} ELSE COALESCE(sl.input_tokens, 0) END), 0),
                    COALESCE(SUM(CASE WHEN ({market_total}) > 0 OR sl.request_id IS NULL THEN COALESCE(ml.output_tokens, 0) ELSE COALESCE(sl.output_tokens, 0) END), 0),
                    COALESCE(SUM(CASE WHEN ({market_total}) > 0 OR sl.request_id IS NULL THEN COALESCE(ml.cache_read_tokens, 0) ELSE COALESCE(sl.cache_read_tokens, 0) END), 0),
                    COALESCE(SUM(CASE WHEN ({market_total}) > 0 OR sl.request_id IS NULL THEN COALESCE(ml.cache_creation_tokens, 0) ELSE COALESCE(sl.cache_creation_tokens, 0) END), 0)
             FROM market_request_logs ml
             INNER JOIN shares s
               ON s.share_id = ml.share_id
              AND lower(trim(s.owner_email)) = lower(trim(?1))
             LEFT JOIN share_request_logs sl
               ON sl.request_id = ml.request_id
              AND sl.share_id = ml.share_id
              AND sl.is_health_check = 0
             WHERE ml.created_at >= ?2
             GROUP BY COALESCE(s.installation_id, ''), COALESCE(ml.share_id, ''),
                      usage_model, COALESCE(lower(trim(ml.user_email)), ''),
                      COALESCE(NULLIF(trim(s.share_name), ''), COALESCE(ml.share_subdomain, ''), '')"
        );
        {
            let mut stmt = conn.prepare(&market_sql).map_err(|e| {
                AppError::Internal(format!("prepare provider market usage failed: {e}"))
            })?;
            let mapped = stmt
                .query_map(params![email, start_rfc3339], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?.max(0) as u64,
                        row.get::<_, i64>(6)?.max(0) as u64,
                        row.get::<_, i64>(7)?.max(0) as u64,
                        row.get::<_, i64>(8)?.max(0) as u64,
                    ))
                })
                .map_err(|e| {
                    AppError::Internal(format!("query provider market usage failed: {e}"))
                })?;
            for row in mapped {
                rows.push(row.map_err(|e| {
                    AppError::Internal(format!("read provider market row failed: {e}"))
                })?);
            }
        }

        let share_sql = format!(
            "SELECT COALESCE(sl.installation_id, s.installation_id, ''),
                    COALESCE(sl.share_id, ''),
                    COALESCE(NULLIF(trim(sl.share_name), ''), NULLIF(trim(s.share_name), ''), ''),
                    {share_model} AS usage_model,
                    COALESCE(lower(trim(sl.user_email)), ''),
                    COALESCE(SUM(sl.input_tokens), 0),
                    COALESCE(SUM(sl.output_tokens), 0),
                    COALESCE(SUM(sl.cache_read_tokens), 0),
                    COALESCE(SUM(sl.cache_creation_tokens), 0)
             FROM share_request_logs sl
             INNER JOIN shares s
               ON s.share_id = sl.share_id
              AND lower(trim(s.owner_email)) = lower(trim(?1))
             WHERE sl.created_at >= ?2
               AND sl.is_health_check = 0
               AND NOT EXISTS (
                    SELECT 1
                    FROM market_request_logs ml
                    WHERE ml.request_id = sl.request_id
                      AND COALESCE(ml.share_id, '') = sl.share_id
               )
             GROUP BY COALESCE(sl.installation_id, s.installation_id, ''),
                      COALESCE(sl.share_id, ''), usage_model,
                      COALESCE(lower(trim(sl.user_email)), ''),
                      COALESCE(NULLIF(trim(sl.share_name), ''), NULLIF(trim(s.share_name), ''), '')"
        );
        {
            let mut stmt = conn.prepare(&share_sql).map_err(|e| {
                AppError::Internal(format!("prepare provider share usage failed: {e}"))
            })?;
            let mapped = stmt
                .query_map(params![email, start_ts], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?.max(0) as u64,
                        row.get::<_, i64>(6)?.max(0) as u64,
                        row.get::<_, i64>(7)?.max(0) as u64,
                        row.get::<_, i64>(8)?.max(0) as u64,
                    ))
                })
                .map_err(|e| {
                    AppError::Internal(format!("query provider share usage failed: {e}"))
                })?;
            for row in mapped {
                rows.push(row.map_err(|e| {
                    AppError::Internal(format!("read provider share row failed: {e}"))
                })?);
            }
        }

        // Also include owned shares with zero usage so empty provider views still list clients.
        let owned_shares = conn
            .prepare(
                "SELECT s.share_id, COALESCE(s.share_name, ''), COALESCE(s.installation_id, ''),
                        COALESCE(i.platform, ''), COALESCE(i.app_version, ''),
                        t.subdomain
                 FROM shares s
                 LEFT JOIN installations i ON i.id = s.installation_id
                 LEFT JOIN installation_client_tunnels t ON t.installation_id = s.installation_id
                 WHERE lower(trim(s.owner_email)) = lower(trim(?1))",
            )
            .and_then(|mut stmt| {
                let mapped = stmt.query_map(params![email], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                })?;
                mapped.collect::<Result<Vec<_>, _>>()
            })
            .map_err(|e| AppError::Internal(format!("query owned shares failed: {e}")))?;

        let mut labels = BTreeMap::<String, String>::new();
        for (_share_id, _share_name, installation_id, platform, app_version, subdomain) in
            &owned_shares
        {
            if installation_id.is_empty() {
                continue;
            }
            labels
                .entry(installation_id.clone())
                .or_insert_with(|| installation_label(platform, app_version, subdomain.as_deref()));
        }

        // installation -> share -> (name, totals, models, callers)
        #[derive(Default)]
        struct ShareAcc {
            name: String,
            totals: TokenAgg,
            models: BTreeMap<String, TokenAgg>,
            callers: BTreeMap<String, TokenAgg>,
        }
        #[derive(Default)]
        struct InstAcc {
            totals: TokenAgg,
            shares: BTreeMap<String, ShareAcc>,
        }
        let mut installations = BTreeMap::<String, InstAcc>::new();

        for (share_id, share_name, installation_id, _, _, _) in &owned_shares {
            let inst_key = if installation_id.is_empty() {
                "_unknown".to_string()
            } else {
                installation_id.clone()
            };
            let inst = installations.entry(inst_key).or_default();
            let share = inst.shares.entry(share_id.clone()).or_default();
            if share.name.is_empty() {
                share.name = share_name.clone();
            }
        }

        let mut totals = TokenAgg::default();
        for (
            installation_id,
            share_id,
            share_name,
            model,
            caller,
            input,
            output,
            cache_read,
            cache_creation,
        ) in rows
        {
            totals.add(input, output, cache_read, cache_creation);
            let inst_key = if installation_id.is_empty() {
                "_unknown".to_string()
            } else {
                installation_id
            };
            let inst = installations.entry(inst_key).or_default();
            inst.totals.add(input, output, cache_read, cache_creation);
            let share = inst.shares.entry(share_id).or_default();
            if share.name.is_empty() && !share_name.is_empty() {
                share.name = share_name;
            }
            share.totals.add(input, output, cache_read, cache_creation);
            share
                .models
                .entry(model)
                .or_default()
                .add(input, output, cache_read, cache_creation);
            if !caller.is_empty() {
                share.callers.entry(caller).or_default().add(
                    input,
                    output,
                    cache_read,
                    cache_creation,
                );
            }
        }

        let mut installation_rows = installations
            .into_iter()
            .map(|(installation_id, inst)| {
                let mut shares = inst
                    .shares
                    .into_iter()
                    .map(|(share_id, share)| {
                        let mut callers = share
                            .callers
                            .into_iter()
                            .map(|(email, agg)| UsageCallerRow {
                                email,
                                input_tokens: agg.input,
                                output_tokens: agg.output,
                                cache_read_tokens: agg.cache_read,
                                cache_creation_tokens: agg.cache_creation,
                                total_tokens: agg.total(),
                            })
                            .collect::<Vec<_>>();
                        callers.sort_by(|a, b| {
                            b.total_tokens
                                .cmp(&a.total_tokens)
                                .then_with(|| a.email.cmp(&b.email))
                        });
                        callers.truncate(TOP_CALLERS_LIMIT);
                        ProviderShareUsage {
                            share_id,
                            share_name: share.name,
                            input_tokens: share.totals.input,
                            output_tokens: share.totals.output,
                            cache_read_tokens: share.totals.cache_read,
                            cache_creation_tokens: share.totals.cache_creation,
                            total_tokens: share.totals.total(),
                            models: finish_models(share.models, TOP_MODELS_LIMIT),
                            callers,
                        }
                    })
                    .collect::<Vec<_>>();
                shares.sort_by(|a, b| {
                    b.total_tokens
                        .cmp(&a.total_tokens)
                        .then_with(|| a.share_id.cmp(&b.share_id))
                });
                let label = if installation_id == "_unknown" {
                    "unknown".to_string()
                } else {
                    labels
                        .get(&installation_id)
                        .cloned()
                        .unwrap_or_else(|| installation_id.clone())
                };
                ProviderInstallationUsage {
                    installation_id: if installation_id == "_unknown" {
                        String::new()
                    } else {
                        installation_id
                    },
                    label,
                    input_tokens: inst.totals.input,
                    output_tokens: inst.totals.output,
                    cache_read_tokens: inst.totals.cache_read,
                    cache_creation_tokens: inst.totals.cache_creation,
                    total_tokens: inst.totals.total(),
                    shares,
                }
            })
            .collect::<Vec<_>>();
        installation_rows.sort_by(|a, b| {
            b.total_tokens
                .cmp(&a.total_tokens)
                .then_with(|| a.installation_id.cmp(&b.installation_id))
        });

        Ok(ProviderUsageResponse {
            period: window.period,
            bucket_granularity: window.bucket_granularity,
            days: window.days,
            input_tokens: totals.input,
            output_tokens: totals.output,
            cache_read_tokens: totals.cache_read,
            cache_creation_tokens: totals.cache_creation,
            total_tokens: totals.total(),
            installations: installation_rows,
        })
    }

    pub async fn usage_global(&self, period: &str) -> Result<GlobalUsageResponse, AppError> {
        let window = normalize_account_usage_period(period)?;
        let conn = self.conn.lock().await;
        let events = query_consumer_events(&conn, None, &window)?;
        let usage = build_account_usage(&window, events, false);
        let (active_shares, active_clients) = count_active_network(&conn)?;
        Ok(GlobalUsageResponse {
            period: usage.period,
            bucket_granularity: usage.bucket_granularity,
            days: usage.days,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_creation_tokens: usage.cache_creation_tokens,
            total_tokens: usage.total_tokens,
            models: usage.models,
            daily: usage.daily,
            active_shares,
            active_clients,
        })
    }

    pub async fn usage_consumer_by_username(
        &self,
        username: &str,
        period: &str,
    ) -> Result<Option<(UserProfilePublic, AccountUsageResponse)>, AppError> {
        let normalized = username.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return Ok(None);
        }
        let window = normalize_account_usage_period(period)?;
        let conn = self.conn.lock().await;
        let Some((display_username, email, public_stats_enabled)) = conn
            .query_row(
                "SELECT p.username, u.email_normalized, p.public_stats_enabled
                 FROM user_profiles p
                 INNER JOIN users u ON u.id = p.user_id
                 WHERE p.username_normalized = ?1",
                params![normalized],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)? != 0,
                    ))
                },
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("query public usage profile failed: {e}")))?
        else {
            return Ok(None);
        };
        if !public_stats_enabled {
            return Ok(None);
        }
        let username = display_username
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| username.trim().to_string());
        let events = query_consumer_events(&conn, Some(&email), &window)?;
        let usage = build_account_usage(&window, events, false);
        Ok(Some((
            UserProfilePublic {
                username,
                public_stats_enabled: true,
            },
            usage,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_period_maps_aliases_to_canonical() {
        let p24 = normalize_account_usage_period("24h").unwrap();
        assert_eq!(p24.period, "24h");
        assert_eq!(p24.bucket_granularity, "hour");

        let p7 = normalize_account_usage_period("1w").unwrap();
        assert_eq!(p7.period, "7d");
        assert_eq!(p7.bucket_granularity, "day");
        assert_eq!(p7.days, 7);
        assert_eq!(p7.bucket_keys.len(), 7);

        let p30 = normalize_account_usage_period("30d").unwrap();
        assert_eq!(p30.period, "30d");
        assert_eq!(p30.days, 30);

        assert!(normalize_account_usage_period("year").is_err());
    }

    #[test]
    fn validate_username_rules() {
        assert!(validate_username("ab").is_err());
        assert!(validate_username("a".repeat(33).as_str()).is_err());
        assert!(validate_username("bad name").is_err());
        assert!(validate_username("bad@name").is_err());
        let (display, normalized) = validate_username("Foo_Bar-1").unwrap();
        assert_eq!(display, "Foo_Bar-1");
        assert_eq!(normalized, "foo_bar-1");
    }

    #[test]
    fn empty_daily_fills_all_buckets() {
        let keys = vec!["2026-07-01".into(), "2026-07-02".into()];
        let daily = empty_daily(&keys);
        assert_eq!(daily.len(), 2);
        assert_eq!(daily[0].total_tokens, 0);
    }
}
