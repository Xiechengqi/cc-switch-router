//! Shared rules for platform-wallet markets.
use crate::db::{Connection, OptionalExtension, params};
use crate::error::AppError;
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};

pub const DEFAULT_TRIAL_HOURS: i64 = 12;
pub const TRIAL_SECONDS: i64 = DEFAULT_TRIAL_HOURS * 3_600;
pub(crate) const MAX_CLIENT_TRIAL_HOURS: i64 = 720;

pub(crate) fn validate_client_trial_hours(hours: Option<i64>) -> Result<Option<i64>, AppError> {
    if hours.is_some_and(|hours| !(0..=MAX_CLIENT_TRIAL_HOURS).contains(&hours)) {
        return Err(AppError::BadRequest(
            "Client trial hours must be between 0 and 720".into(),
        ));
    }
    Ok(hours)
}
pub(crate) const MARKET_CURRENCY: &str = "USD";
pub(crate) const MONEY_UNITS_PER_MINOR: i64 = 86_400;
pub(crate) const MAX_DAILY_RATE_MINOR: i64 = 100_000_000;
pub(crate) const USD_CNY_RATE_SCALE: i64 = 1_000_000;
pub(crate) const DEFAULT_USD_CNY_RATE_MICROS: i64 = 7 * USD_CNY_RATE_SCALE;
const MIN_USD_CNY_RATE_MICROS: i64 = USD_CNY_RATE_SCALE / 100;
const MAX_USD_CNY_RATE_MICROS: i64 = 100 * USD_CNY_RATE_SCALE;
const HEALTH_FRESHNESS_SECS: i64 = 90;
const MAX_ACCRUAL_GAP_SECS: i64 = 20;
pub(crate) const PRICING_FREE: &str = "free";
pub(crate) const PRICING_LEGACY_METERED_DAILY: &str = "legacy_metered_daily";
pub(crate) const PRICING_PREPAID_CALENDAR_MONTH: &str = "prepaid_calendar_month";
pub(crate) const BILLING_INTERVAL_CALENDAR_MONTH: &str = "calendar_month";
fn map_db(context: &'static str) -> impl FnOnce(crate::db::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

pub(crate) fn ceil_minor_units(amount_units: i64) -> i64 {
    ceil_minor(amount_units)
}

pub(crate) fn floor_minor_units(amount_units: i64) -> i64 {
    floor_minor(amount_units)
}

fn ceil_minor(amount_units: i64) -> i64 {
    if amount_units <= 0 {
        0
    } else {
        amount_units / MONEY_UNITS_PER_MINOR + i64::from(amount_units % MONEY_UNITS_PER_MINOR != 0)
    }
}

fn floor_minor(amount_units: i64) -> i64 {
    amount_units.max(0) / MONEY_UNITS_PER_MINOR
}

pub(crate) fn parse_usd_cny_rate_micros(value: &str) -> Result<i64, String> {
    let trimmed = value.trim();
    let (whole, fraction) = trimmed
        .split_once('.')
        .map(|(whole, fraction)| (whole, Some(fraction)))
        .unwrap_or((trimmed, None));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.is_some_and(|fraction| {
            fraction.is_empty()
                || fraction.len() > 6
                || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err("USD/CNY rate must be a decimal with at most 6 fractional digits".into());
    }
    let whole = whole
        .parse::<i64>()
        .map_err(|_| "USD/CNY rate is too large".to_string())?;
    let fraction = fraction.unwrap_or_default();
    let fractional_micros = if fraction.is_empty() {
        0
    } else {
        let value = fraction
            .parse::<i64>()
            .map_err(|_| "USD/CNY rate fraction is invalid".to_string())?;
        value * 10_i64.pow(u32::try_from(6 - fraction.len()).unwrap_or_default())
    };
    let rate_micros = whole
        .checked_mul(USD_CNY_RATE_SCALE)
        .and_then(|value| value.checked_add(fractional_micros))
        .ok_or_else(|| "USD/CNY rate is too large".to_string())?;
    if !(MIN_USD_CNY_RATE_MICROS..=MAX_USD_CNY_RATE_MICROS).contains(&rate_micros) {
        return Err("USD/CNY rate must be between 0.01 and 100".into());
    }
    Ok(rate_micros)
}

fn free_usage_allowance_seconds(free_duration_days: u32) -> Result<i64, AppError> {
    i64::from(free_duration_days)
        .checked_mul(86_400)
        .ok_or_else(|| AppError::BadRequest("free usage duration is too large".into()))
}

fn ensure_free_usage_scope_tx(
    conn: &Connection,
    buyer_user_id: &str,
    supplier_user_id: &str,
    product_kind: &str,
    scope_kind: &str,
    scope_ref: &str,
    allowance_seconds: i64,
    now: &str,
) -> Result<i64, AppError> {
    conn.execute(
        "INSERT INTO market_free_usage_ledgers (
            buyer_user_id, supplier_user_id, product_kind, scope_kind, scope_ref,
            allowance_seconds, granted_seconds, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?7)
         ON CONFLICT(buyer_user_id, supplier_user_id, product_kind, scope_kind, scope_ref)
         DO UPDATE SET allowance_seconds = MAX(market_free_usage_ledgers.allowance_seconds,
                                                excluded.allowance_seconds),
                       updated_at = excluded.updated_at",
        params![
            buyer_user_id,
            supplier_user_id,
            product_kind,
            scope_kind,
            scope_ref,
            allowance_seconds,
            now,
        ],
    )
    .map_err(map_db("ensure persistent free usage ledger"))?;
    conn.query_row(
        "SELECT MAX(allowance_seconds - granted_seconds, 0)
         FROM market_free_usage_ledgers
         WHERE buyer_user_id = ?1 AND supplier_user_id = ?2 AND product_kind = ?3
           AND scope_kind = ?4 AND scope_ref = ?5",
        params![
            buyer_user_id,
            supplier_user_id,
            product_kind,
            scope_kind,
            scope_ref,
        ],
        |row| row.get(0),
    )
    .map_err(map_db("read persistent free usage balance"))
}

pub(crate) fn free_usage_available_tx(
    conn: &Connection,
    buyer_user_id: &str,
    supplier_user_id: &str,
    product_kind: &str,
    service_ref: &str,
    free_duration_days: u32,
    now: &str,
) -> Result<bool, AppError> {
    let allowance_seconds = free_usage_allowance_seconds(free_duration_days)?;
    let service_remaining = ensure_free_usage_scope_tx(
        conn,
        buyer_user_id,
        supplier_user_id,
        product_kind,
        "service",
        service_ref,
        allowance_seconds,
        now,
    )?;
    let supplier_remaining = ensure_free_usage_scope_tx(
        conn,
        buyer_user_id,
        supplier_user_id,
        product_kind,
        "supplier",
        supplier_user_id,
        allowance_seconds,
        now,
    )?;
    Ok(service_remaining > 0 && supplier_remaining > 0)
}

pub(crate) fn claim_free_usage_tx(
    conn: &Connection,
    buyer_user_id: &str,
    supplier_user_id: &str,
    product_kind: &str,
    service_ref: &str,
    free_duration_days: u32,
    now: &str,
) -> Result<i64, AppError> {
    let allowance_seconds = free_usage_allowance_seconds(free_duration_days)?;
    let service_remaining = ensure_free_usage_scope_tx(
        conn,
        buyer_user_id,
        supplier_user_id,
        product_kind,
        "service",
        service_ref,
        allowance_seconds,
        now,
    )?;
    let supplier_remaining = ensure_free_usage_scope_tx(
        conn,
        buyer_user_id,
        supplier_user_id,
        product_kind,
        "supplier",
        supplier_user_id,
        allowance_seconds,
        now,
    )?;
    let granted_seconds = service_remaining.min(supplier_remaining);
    if granted_seconds <= 0 {
        return Ok(0);
    }
    for (scope_kind, scope_ref) in [("service", service_ref), ("supplier", supplier_user_id)] {
        conn.execute(
            "UPDATE market_free_usage_ledgers
             SET granted_seconds = MIN(allowance_seconds, granted_seconds + ?6),
                 updated_at = ?7
             WHERE buyer_user_id = ?1 AND supplier_user_id = ?2 AND product_kind = ?3
               AND scope_kind = ?4 AND scope_ref = ?5",
            params![
                buyer_user_id,
                supplier_user_id,
                product_kind,
                scope_kind,
                scope_ref,
                granted_seconds,
                now,
            ],
        )
        .map_err(map_db("claim persistent free usage"))?;
    }
    Ok(granted_seconds)
}

fn latest_health_observation_tx(
    tx: &Connection,
    product_kind: &str,
    product_ref: &str,
    service_ref: &str,
    observed_at: DateTime<Utc>,
) -> Result<Option<(i64, String, String)>, AppError> {
    let service_active = match product_kind {
        "share" => tx
            .query_row(
                "SELECT 1 FROM share_market_subscriptions
                 WHERE id = ?1 AND status IN ('active_free', 'active_postpaid', 'active_prepaid')",
                params![product_ref],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(map_db("check billable Share entitlement state"))?
            .is_some(),
        "client_host" => tx
            .query_row(
                "SELECT 1 FROM client_market_subscriptions
                 WHERE installation_id = ?1 AND status = 'active'",
                params![product_ref],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(map_db("check billable Client subscription state"))?
            .is_some(),
        _ => false,
    };
    if !service_active {
        return Ok(None);
    }
    if product_kind == "share" {
        return crate::share_market::share_market_billing_observation_tx(
            tx,
            product_ref,
            observed_at,
        );
    }
    let (table, key) = match product_kind {
        "client_host" => ("installation_health_checks", "installation_id"),
        _ => {
            return Err(AppError::Internal(format!(
                "unsupported billing product kind {product_kind}"
            )));
        }
    };
    tx.query_row(
        &format!(
            "SELECT checked_at, status, reason FROM {table}
             WHERE {key} = ?1 AND checked_at <= ?2
             ORDER BY checked_at DESC LIMIT 1"
        ),
        params![service_ref, observed_at.timestamp()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .optional()
    .map_err(map_db("read latest service health observation"))
}

pub(crate) fn effective_service_health_tx(
    tx: &Connection,
    product_kind: &str,
    product_ref: &str,
    service_ref: &str,
    evaluation_started_at: DateTime<Utc>,
    evaluation_ended_at: DateTime<Utc>,
) -> Result<(String, String), AppError> {
    let gap = (evaluation_ended_at - evaluation_started_at)
        .num_seconds()
        .max(0);
    let observation = latest_health_observation_tx(
        tx,
        product_kind,
        product_ref,
        service_ref,
        evaluation_ended_at,
    )?;
    let (observed_state, observation_reason) = match observation {
        Some((checked_at, status, reason))
            if status == "healthy"
                && evaluation_ended_at.timestamp() - checked_at <= HEALTH_FRESHNESS_SECS =>
        {
            ("healthy".to_string(), reason)
        }
        Some((_, status, reason)) if status == "unhealthy" => ("unhealthy".to_string(), reason),
        Some((_, _, reason)) => ("unknown".to_string(), reason),
        None => ("unknown".to_string(), "no_router_observation".to_string()),
    };
    if gap > MAX_ACCRUAL_GAP_SECS {
        Ok(("unknown".into(), "billing_worker_gap".into()))
    } else {
        Ok((observed_state, observation_reason))
    }
}

fn days_in_month(year: i32, month: u32) -> Result<u32, AppError> {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let next = NaiveDate::from_ymd_opt(next_year, next_month, 1).ok_or_else(|| {
        AppError::Internal("calendar month is outside the supported range".into())
    })?;
    Ok((next - Duration::days(1)).day())
}

pub(crate) fn next_calendar_month_at(
    current: DateTime<Utc>,
    anchor_day: u32,
) -> Result<DateTime<Utc>, AppError> {
    if !(1..=31).contains(&anchor_day) {
        return Err(AppError::Internal(
            "recurring billing anchor day is invalid".into(),
        ));
    }
    let (year, month) = if current.month() == 12 {
        (current.year() + 1, 1)
    } else {
        (current.year(), current.month() + 1)
    };
    let day = anchor_day.min(days_in_month(year, month)?);
    let date = NaiveDate::from_ymd_opt(year, month, day)
        .ok_or_else(|| AppError::Internal("recurring period date is invalid".into()))?;
    Ok(Utc.from_utc_datetime(&date.and_time(current.time())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(value: &str) -> DateTime<Utc> {
        value.parse().unwrap()
    }

    #[test]
    fn money_rounding_and_fx_parsing_are_exact_and_bounded() {
        for (units, floor, ceil) in [
            (0, 0, 0),
            (-1, 0, 0),
            (1, 0, 1),
            (86_400, 1, 1),
            (86_401, 1, 2),
        ] {
            assert_eq!(floor_minor_units(units), floor);
            assert_eq!(ceil_minor_units(units), ceil);
        }
        assert!(ceil_minor_units(i64::MAX) > 0);
        assert_eq!(parse_usd_cny_rate_micros(" 7.123456 ").unwrap(), 7_123_456);
        for value in ["0.001", "100.01", "7.1234567", "NaN", "+7", "1e1", "", "7."] {
            assert!(parse_usd_cny_rate_micros(value).is_err(), "{value}");
        }
    }

    #[test]
    fn calendar_month_keeps_anchor_time_leap_year_and_year_rollover() {
        let feb = next_calendar_month_at(at("2025-01-31T12:34:56.123Z"), 31).unwrap();
        assert_eq!(feb, at("2025-02-28T12:34:56.123Z"));
        assert_eq!(
            next_calendar_month_at(feb, 31).unwrap(),
            at("2025-03-31T12:34:56.123Z")
        );
        assert_eq!(
            next_calendar_month_at(at("2024-01-31T23:59:59Z"), 31).unwrap(),
            at("2024-02-29T23:59:59Z")
        );
        assert_eq!(
            next_calendar_month_at(at("2025-12-30T01:02:03Z"), 30).unwrap(),
            at("2026-01-30T01:02:03Z")
        );
        assert!(next_calendar_month_at(feb, 0).is_err());
    }

    #[test]
    fn free_usage_is_persistent_and_scoped_to_buyer_supplier_and_product() {
        let conn = crate::test_support::database().unwrap();
        let now = "2026-10-26T00:00:00Z";
        assert!(free_usage_available_tx(&conn, "buyer", "seller", "share", "a", 1, now).unwrap());
        assert_eq!(
            claim_free_usage_tx(&conn, "buyer", "seller", "share", "a", 1, now).unwrap(),
            86_400
        );
        for service in ["a", "b"] {
            assert!(
                !free_usage_available_tx(&conn, "buyer", "seller", "share", service, 1, now)
                    .unwrap()
            );
            assert_eq!(
                claim_free_usage_tx(&conn, "buyer", "seller", "share", service, 1, now).unwrap(),
                0
            );
        }
        for (buyer, seller, product) in [
            ("other", "seller", "share"),
            ("buyer", "other", "share"),
            ("buyer", "seller", "client_host"),
        ] {
            assert!(free_usage_available_tx(&conn, buyer, seller, product, "a", 1, now).unwrap());
        }
        assert_eq!(
            claim_free_usage_tx(&conn, "buyer", "seller", "share", "a", 2, now).unwrap(),
            86_400
        );
    }

    #[test]
    fn missing_service_observations_and_worker_gaps_never_become_billable() {
        let conn = crate::test_support::database().unwrap();
        let start = at("2026-10-26T00:00:00Z");
        assert_eq!(
            effective_service_health_tx(
                &conn,
                "client_host",
                "missing",
                "missing",
                start,
                start + Duration::seconds(20)
            )
            .unwrap(),
            ("unknown".into(), "no_router_observation".into())
        );
        assert_eq!(
            effective_service_health_tx(
                &conn,
                "client_host",
                "missing",
                "missing",
                start,
                start + Duration::seconds(21)
            )
            .unwrap(),
            ("unknown".into(), "billing_worker_gap".into())
        );
    }
}
