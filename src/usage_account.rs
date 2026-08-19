use std::collections::BTreeMap;

use crate::db::{Connection, OptionalExtension, params};
use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use crate::error::AppError;
use crate::models::{
    AccountUsageResponse, GlobalUsageResponse, ProviderInstallationUsage, ProviderShareUsage,
    ProviderUsageResponse, UpdateUsageCardSettingsRequest, UsageCallerRow,
    UsageCardSettingsResponse, UsageDailyBucket, UsageModelRow, UsageShareRow,
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

fn normalize_usage_email(value: &str) -> Result<String, AppError> {
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

fn observation_input_tokens_expr(alias: &str) -> String {
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

fn observation_total_tokens_expr(alias: &str) -> String {
    let prefix = if alias.is_empty() {
        String::new()
    } else {
        format!("{alias}.")
    };
    let input_expr = observation_input_tokens_expr(alias);
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

fn observation_model_expr(alias: &str) -> String {
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
        .map_err(|e| AppError::Internal(format!("query usage card user failed: {e}")))?
    {
        return Ok(id);
    }
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO users (id, email_normalized, status, created_at, last_login_at)
         VALUES (?1, ?2, 'active', ?3, ?4)",
        params![id, email, now, now],
    )
    .map_err(|e| AppError::Internal(format!("insert usage card user failed: {e}")))?;
    Ok(id)
}

fn usage_card_settings_response(
    user_id: &str,
    email: &str,
    public_stats_enabled: bool,
) -> UsageCardSettingsResponse {
    UsageCardSettingsResponse {
        user_id: user_id.to_string(),
        email: email.to_string(),
        public_stats_enabled,
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
    let observation_bucket_expr = if window.bucket_granularity == "hour" {
        "strftime('%Y-%m-%dT%H:00:00Z', ml.created_at)"
    } else {
        "date(ml.created_at)"
    };
    let share_bucket_expr = if window.bucket_granularity == "hour" {
        "strftime('%Y-%m-%dT%H:00:00Z', sl.created_at, 'unixepoch')"
    } else {
        "date(sl.created_at, 'unixepoch')"
    };
    let observation_input = observation_input_tokens_expr("ml");
    let observation_total = observation_total_tokens_expr("ml");
    let observation_model = observation_model_expr("ml");
    let share_model = share_model_expr("sl");

    let (observation_email_filter, share_email_filter, start_placeholder) = if email.is_some() {
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

    let observation_sql = format!(
        "SELECT COALESCE(ml.share_id, ''),
                COALESCE(NULLIF(trim(s.share_name), ''), COALESCE(ml.share_subdomain, ''), ''),
                {observation_model} AS usage_model,
                {observation_bucket_expr} AS usage_bucket,
                COALESCE(SUM(CASE WHEN ({observation_total}) > 0 OR sl.request_id IS NULL THEN {observation_input} ELSE COALESCE(sl.input_tokens, 0) END), 0),
                COALESCE(SUM(CASE WHEN ({observation_total}) > 0 OR sl.request_id IS NULL THEN COALESCE(ml.output_tokens, 0) ELSE COALESCE(sl.output_tokens, 0) END), 0),
                COALESCE(SUM(CASE WHEN ({observation_total}) > 0 OR sl.request_id IS NULL THEN COALESCE(ml.cache_read_tokens, 0) ELSE COALESCE(sl.cache_read_tokens, 0) END), 0),
                COALESCE(SUM(CASE WHEN ({observation_total}) > 0 OR sl.request_id IS NULL THEN COALESCE(ml.cache_creation_tokens, 0) ELSE COALESCE(sl.cache_creation_tokens, 0) END), 0)
         FROM capacity_request_observations ml
         LEFT JOIN shares s ON s.share_id = ml.share_id
         LEFT JOIN share_request_logs sl
           ON sl.request_id = ml.request_id
          AND sl.share_id = ml.share_id
          AND sl.is_health_check = 0
         WHERE ml.created_at >= {start_placeholder}
           {observation_email_filter}
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
                FROM capacity_request_observations ml
                WHERE ml.request_id = sl.request_id
                  AND COALESCE(ml.share_id, '') = sl.share_id
           )
         GROUP BY COALESCE(sl.share_id, ''), usage_model, usage_bucket,
                  COALESCE(NULLIF(trim(sl.share_name), ''), '')"
    );

    let mut events = Vec::new();

    {
        let mut stmt = conn.prepare(&observation_sql).map_err(|e| {
            AppError::Internal(format!("prepare consumer observation usage failed: {e}"))
        })?;
        let mapped = if let Some(email) = email {
            stmt.query_map(params![email, start_rfc3339], map_usage_event_row)
        } else {
            stmt.query_map(params![start_rfc3339], map_usage_event_row)
        }
        .map_err(|e| AppError::Internal(format!("query consumer observation usage failed: {e}")))?;
        for row in mapped {
            events.push(row.map_err(|e| {
                AppError::Internal(format!("read observation usage row failed: {e}"))
            })?);
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

fn map_usage_event_row(row: &crate::db::Row<'_>) -> Result<UsageEvent, crate::db::Error> {
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
            "SELECT COUNT(*)
             FROM shares s
             INNER JOIN installations i ON i.id = s.installation_id
             WHERE s.share_status = 'active'
               AND i.lifecycle = 'active'
               AND i.client_activated_at IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .map_err(|e| AppError::Internal(format!("count active shares failed: {e}")))?;
    let active_clients: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT i.id)
             FROM installations i
             INNER JOIN shares s ON s.installation_id = i.id
             INNER JOIN installation_notification_state ns ON ns.installation_id = i.id
             WHERE ns.last_heartbeat_at >= ?1
               AND i.lifecycle = 'active'
               AND i.client_activated_at IS NOT NULL
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
    pub async fn get_usage_card_settings(
        &self,
        email: &str,
    ) -> Result<UsageCardSettingsResponse, AppError> {
        let email = normalize_usage_email(email)?;
        let conn = self.conn.lock().await;
        let user_id = ensure_user_id(&conn, &email)?;
        let public_stats_enabled = conn
            .query_row(
                "SELECT public_stats_enabled FROM users WHERE id = ?1",
                params![user_id],
                |row| Ok(row.get::<_, i64>(0)? != 0),
            )
            .map_err(|e| AppError::Internal(format!("query usage card settings failed: {e}")))?;
        Ok(usage_card_settings_response(
            &user_id,
            &email,
            public_stats_enabled,
        ))
    }

    pub async fn update_usage_card_settings(
        &self,
        email: &str,
        patch: UpdateUsageCardSettingsRequest,
    ) -> Result<UsageCardSettingsResponse, AppError> {
        let email = normalize_usage_email(email)?;
        let conn = self.conn.lock().await;
        let user_id = ensure_user_id(&conn, &email)?;
        conn.execute(
            "UPDATE users SET public_stats_enabled = ?1 WHERE id = ?2",
            params![i64::from(patch.public_stats_enabled), user_id],
        )
        .map_err(|e| AppError::Internal(format!("update usage card settings failed: {e}")))?;
        Ok(usage_card_settings_response(
            &user_id,
            &email,
            patch.public_stats_enabled,
        ))
    }

    pub async fn usage_consumer(
        &self,
        email: &str,
        period: &str,
    ) -> Result<AccountUsageResponse, AppError> {
        let email = normalize_usage_email(email)?;
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
        let email = normalize_usage_email(email)?;
        let window = normalize_account_usage_period(period)?;
        let start_ts = window.start_at.timestamp();
        let start_rfc3339 = window.start_at.to_rfc3339();
        let conn = self.conn.lock().await;

        let observation_input = observation_input_tokens_expr("ml");
        let observation_total = observation_total_tokens_expr("ml");
        let observation_model = observation_model_expr("ml");
        let share_model = share_model_expr("sl");

        // (installation_id, share_id, share_name, model, caller_email, tokens...)
        type ProvRow = (String, String, String, String, String, u64, u64, u64, u64);
        let mut rows: Vec<ProvRow> = Vec::new();

        let observation_sql = format!(
            "SELECT COALESCE(s.installation_id, ''),
                    COALESCE(ml.share_id, ''),
                    COALESCE(NULLIF(trim(s.share_name), ''), COALESCE(ml.share_subdomain, ''), ''),
                    {observation_model} AS usage_model,
                    COALESCE(lower(trim(ml.user_email)), ''),
                    COALESCE(SUM(CASE WHEN ({observation_total}) > 0 OR sl.request_id IS NULL THEN {observation_input} ELSE COALESCE(sl.input_tokens, 0) END), 0),
                    COALESCE(SUM(CASE WHEN ({observation_total}) > 0 OR sl.request_id IS NULL THEN COALESCE(ml.output_tokens, 0) ELSE COALESCE(sl.output_tokens, 0) END), 0),
                    COALESCE(SUM(CASE WHEN ({observation_total}) > 0 OR sl.request_id IS NULL THEN COALESCE(ml.cache_read_tokens, 0) ELSE COALESCE(sl.cache_read_tokens, 0) END), 0),
                    COALESCE(SUM(CASE WHEN ({observation_total}) > 0 OR sl.request_id IS NULL THEN COALESCE(ml.cache_creation_tokens, 0) ELSE COALESCE(sl.cache_creation_tokens, 0) END), 0)
             FROM capacity_request_observations ml
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
            let mut stmt = conn.prepare(&observation_sql).map_err(|e| {
                AppError::Internal(format!("prepare provider observation usage failed: {e}"))
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
                    AppError::Internal(format!("query provider observation usage failed: {e}"))
                })?;
            for row in mapped {
                rows.push(row.map_err(|e| {
                    AppError::Internal(format!("read provider observation row failed: {e}"))
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
                    FROM capacity_request_observations ml
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

    pub async fn usage_consumer_by_user_id(
        &self,
        user_id: &str,
        period: &str,
    ) -> Result<Option<(String, AccountUsageResponse)>, AppError> {
        let Ok(user_id) = Uuid::parse_str(user_id.trim()) else {
            return Ok(None);
        };
        let window = normalize_account_usage_period(period)?;
        let conn = self.conn.lock().await;
        let Some((email, public_stats_enabled)) = conn
            .query_row(
                "SELECT email_normalized, public_stats_enabled
                 FROM users
                 WHERE id = ?1 AND status = 'active'",
                params![user_id.to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0)),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("query public usage card failed: {e}")))?
        else {
            return Ok(None);
        };
        if !public_stats_enabled {
            return Ok(None);
        }
        let events = query_consumer_events(&conn, Some(&email), &window)?;
        let usage = build_account_usage(&window, events, false);
        Ok(Some((email, usage)))
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

    #[tokio::test]
    async fn usage_card_is_public_by_default_and_can_be_disabled() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let settings = store
            .get_usage_card_settings("Owner@Example.com")
            .await
            .expect("default usage card settings");

        assert_eq!(settings.email, "owner@example.com");
        assert!(settings.public_stats_enabled);
        assert_eq!(
            Uuid::parse_str(&settings.user_id).unwrap().to_string(),
            settings.user_id
        );

        let (public_email, _) = store
            .usage_consumer_by_user_id(&settings.user_id, "24h")
            .await
            .expect("public usage lookup")
            .expect("usage card should be public by default");
        assert_eq!(public_email, settings.email);
        assert!(
            store
                .usage_consumer_by_user_id("not-a-uuid", "24h")
                .await
                .expect("invalid public usage lookup")
                .is_none()
        );

        let updated = store
            .update_usage_card_settings(
                &settings.email,
                UpdateUsageCardSettingsRequest {
                    public_stats_enabled: false,
                },
            )
            .await
            .expect("disable public usage card");
        assert!(!updated.public_stats_enabled);
        assert!(
            store
                .usage_consumer_by_user_id(&settings.user_id, "24h")
                .await
                .expect("private usage lookup")
                .is_none()
        );
    }

    #[test]
    fn empty_daily_fills_all_buckets() {
        let keys = vec!["2026-07-01".into(), "2026-07-02".into()];
        let daily = empty_daily(&keys);
        assert_eq!(daily.len(), 2);
        assert_eq!(daily[0].total_tokens, 0);
    }

    #[tokio::test]
    async fn active_network_requires_activation_and_fresh_heartbeat() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let now = Utc::now();
        let now_text = now.to_rfc3339();
        let stale_heartbeat = (now - Duration::minutes(16)).to_rfc3339();
        let expires_at = (now + Duration::hours(1)).to_rfc3339();
        let conn = store.conn.lock().await;

        for (installation_id, activated_at, heartbeat_at) in [
            ("inst-online", Some(now_text.as_str()), now_text.as_str()),
            (
                "inst-stale",
                Some(now_text.as_str()),
                stale_heartbeat.as_str(),
            ),
            ("inst-unactivated", None, now_text.as_str()),
        ] {
            conn.execute(
                "INSERT INTO installations (
                    id, public_key, platform, app_version, owner_email, owner_verified_at,
                    created_at, last_seen_at, client_activated_at, control_secret_b64
                 ) VALUES (?1, ?2, 'linux', '1.0.0', 'owner@example.com', ?3, ?3, ?3, ?4, ?5)",
                params![
                    installation_id,
                    format!("key-{installation_id}"),
                    now_text,
                    activated_at,
                    format!("secret-{installation_id}"),
                ],
            )
            .expect("insert usage installation");
            conn.execute(
                "INSERT INTO installation_notification_state (
                    installation_id, monitoring_enabled, presence_state, last_heartbeat_at,
                    created_at, updated_at
                 ) VALUES (?1, 1, 'online', ?2, ?3, ?3)",
                params![installation_id, heartbeat_at, now_text],
            )
            .expect("insert usage heartbeat");
            conn.execute(
                "INSERT INTO shares (
                    share_id, capacity_pool_id, installation_id, share_name,
                    app_type, token_limit,
                    tokens_used, requests_count, share_status, created_at, expires_at, updated_at
                 ) VALUES (?1, ?1, ?2, ?1, 'proxy', 1000, 0, 0,
                           'active', ?3, ?4, ?3)",
                params![
                    format!("share-{installation_id}"),
                    installation_id,
                    now_text,
                    expires_at,
                ],
            )
            .expect("insert usage share");
        }

        let (active_shares, active_clients) =
            count_active_network(&conn).expect("count active network");

        assert_eq!(active_shares, 2);
        assert_eq!(active_clients, 1);
    }
}
