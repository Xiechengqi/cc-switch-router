//! What a Share Market rental actually got for its rent this billing period:
//! the API list-price value of the renter's usage in the current paid period,
//! priced with the same catalog the USD-equivalent quota tariff derives from.
//!
//! This is an estimate for display. It reads the Router's request logs, so
//! it is bounded by their retention and by which models have a price; it is
//! never used for quota enforcement, which stays with the Server ledger.
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use crate::ServerState;
use crate::db::{OptionalExtension, params};
use crate::error::AppError;

/// Wallet money units per minor unit (cent); see `market_common`.
const UNITS_PER_MINOR: i128 = crate::market_common::MONEY_UNITS_PER_MINOR as i128;
const MICROS_PER_MINOR: i128 = 10_000;

pub fn router() -> Router<ServerState> {
    Router::new()
        .route(
            "/v1/share-market/subscriptions/:id/value-usage",
            get(value_usage),
        )
        .layer(middleware::map_response(no_store))
}

async fn no_store(mut response: Response) -> Response {
    // Authentication failures and extractor errors are private as well.
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RentalValueUsage {
    pub subscription_id: String,
    /// Start of the measured window: the later of the paid period start and
    /// the paid service start. Absent while there is no paid period.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period_end: Option<String>,
    /// Rent charged for the current period, in USD micros.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rent_micros: Option<String>,
    /// API list-price value of the priced usage, in USD micros. Absent when
    /// nothing in the window could be priced (never a confident zero).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_used_micros: Option<String>,
    pub priced_coverage_percent: f64,
    /// Rent as a share of the used API value, in basis points.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ratio_bps: Option<u64>,
    /// The window starts before request-log retention or some usage is
    /// unpriced, so the value is a lower bound.
    pub partial: bool,
    /// Why no ratio is shown: `trial`, `no_paid_period`, `no_usage`, `unpriced_usage`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    pub pricing_revision: String,
    pub priced_at: i64,
}

#[derive(Debug)]
struct RentalPeriod {
    share_id: String,
    renter_email: String,
    renter_user_id: String,
    owner_user_id: String,
    contract_status: Option<String>,
    period: Option<(String, String, i64)>,
    paid_started_at: Option<String>,
    ended_at: Option<String>,
    released_at: Option<String>,
}

async fn value_usage(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(subscription_id): Path<String>,
) -> Result<Response, AppError> {
    let session = crate::api::resolve_router_session(&state, &headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("authenticated user session required".into()))?;
    let rental = rental_period(&state, &subscription_id).await?;
    if session.user_id != rental.renter_user_id && session.user_id != rental.owner_user_id {
        return Err(AppError::NotFound("Share subscription not found".into()));
    }
    let now = Utc::now();
    let retention_days = i64::from(state.config.request_log_retention_days);
    let usage =
        rental_value_usage(&state.store, &subscription_id, rental, now, retention_days).await?;
    Ok(Json(usage).into_response())
}

async fn rental_period(
    state: &ServerState,
    subscription_id: &str,
) -> Result<RentalPeriod, AppError> {
    let conn = state.store.conn.lock().await;
    conn.query_row(
        "SELECT sub.share_id, sub.renter_email, sub.renter_user_id, sub.owner_user_id,
                contract.status, period.period_start, period.period_end, period.amount_units,
                contract.paid_started_at, contract.ended_at, sub.released_at
         FROM share_market_subscriptions sub
         LEFT JOIN market_platform_contracts contract
           ON contract.product_kind = 'share' AND contract.product_ref = sub.id
          AND contract.billing_model = 'calendar_month'
         LEFT JOIN market_platform_periods period
           ON period.contract_id = contract.id
          AND period.sequence = contract.current_period_sequence
         WHERE sub.id = ?1",
        params![subscription_id],
        |row| {
            let period = match (
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<i64>>(7)?,
            ) {
                (Some(start), Some(end), Some(units)) => Some((start, end, units)),
                _ => None,
            };
            Ok(RentalPeriod {
                share_id: row.get(0)?,
                renter_email: row.get(1)?,
                renter_user_id: row.get(2)?,
                owner_user_id: row.get(3)?,
                contract_status: row.get(4)?,
                period,
                paid_started_at: row.get(8)?,
                ended_at: row.get(9)?,
                released_at: row.get(10)?,
            })
        },
    )
    .optional()
    .map_err(|error| AppError::Internal(format!("read Share rental period failed: {error}")))?
    .ok_or_else(|| AppError::NotFound("Share subscription not found".into()))
}

fn parse_time(value: &str) -> Result<DateTime<Utc>, AppError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| AppError::Internal("stored rental time is invalid".into()))
}

/// A released rental must never include this buyer's later rental on the
/// same Share, even while the old calendar billing period is still open.
fn measurement_end(
    period_end: &str,
    now: DateTime<Utc>,
    ended_at: Option<&str>,
    released_at: Option<&str>,
) -> Result<DateTime<Utc>, AppError> {
    let mut end = parse_time(period_end)?.min(now);
    for cutoff in [ended_at, released_at].into_iter().flatten() {
        end = end.min(parse_time(cutoff)?);
    }
    Ok(end)
}

async fn rental_value_usage(
    store: &crate::store::AppStore,
    subscription_id: &str,
    rental: RentalPeriod,
    now: DateTime<Utc>,
    retention_days: i64,
) -> Result<RentalValueUsage, AppError> {
    let empty = |reason| RentalValueUsage {
        subscription_id: subscription_id.to_string(),
        period_start: None,
        period_end: None,
        rent_micros: None,
        value_used_micros: None,
        priced_coverage_percent: 0.0,
        ratio_bps: None,
        partial: false,
        reason: Some(reason),
        pricing_revision: String::new(),
        priced_at: now.timestamp(),
    };
    if rental.contract_status.as_deref() == Some("trialing") {
        return Ok(empty("trial"));
    }
    let Some((period_start, period_end, amount_units)) = rental.period else {
        return Ok(empty("no_paid_period"));
    };
    let mut start = parse_time(&period_start)?;
    if let Some(paid_started_at) = rental.paid_started_at.as_deref() {
        start = start.max(parse_time(paid_started_at)?);
    }
    let end = measurement_end(
        &period_end,
        now,
        rental.ended_at.as_deref(),
        rental.released_at.as_deref(),
    )?;
    if end <= start {
        return Ok(empty("no_usage"));
    }
    let breakdown = store
        .share_user_usage_breakdown_in_window(
            &rental.share_id,
            Some(&rental.renter_email),
            Some((start, end)),
        )
        .await?;
    let row = breakdown.rows.into_iter().next();
    let rent_micros = i128::from(amount_units) * MICROS_PER_MINOR / UNITS_PER_MINOR;
    let value_micros = row
        .as_ref()
        .and_then(|row| row.equivalent_usd_micros.as_deref())
        .and_then(|value| value.parse::<i128>().ok())
        .filter(|value| *value >= 0);
    let has_usage = row
        .as_ref()
        .is_some_and(|row| row.observed_totals.total > 0);
    let coverage = row.as_ref().map_or(0.0, |row| row.priced_coverage_percent);
    let retained_from = now - Duration::days(retention_days);
    Ok(RentalValueUsage {
        subscription_id: subscription_id.to_string(),
        period_start: Some(start.to_rfc3339()),
        period_end: Some(parse_time(&period_end)?.to_rfc3339()),
        rent_micros: Some(rent_micros.to_string()),
        value_used_micros: value_micros.map(|value| value.to_string()),
        priced_coverage_percent: coverage,
        ratio_bps: value_micros
            .filter(|value| *value > 0)
            .map(|value| ratio_bps(rent_micros, value)),
        partial: start < retained_from || (has_usage && coverage < 100.0),
        reason: if !has_usage {
            Some("no_usage")
        } else if value_micros.is_none() {
            Some("unpriced_usage")
        } else {
            None
        },
        pricing_revision: breakdown.pricing_revision,
        priced_at: breakdown.priced_at,
    })
}

/// Rent over used value in basis points, rounded half up.
fn ratio_bps(rent_micros: i128, value_micros: i128) -> u64 {
    let bps = (rent_micros * 10_000 + value_micros / 2) / value_micros;
    // Paid rent is never advertised as a free/100%-off service after rounding.
    u64::try_from(bps.max(i128::from(rent_micros > 0))).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rent_ratio_is_exact_and_rounds_half_up() {
        // $20 of rent for $45 of API value is 44.44%.
        assert_eq!(ratio_bps(20_000_000, 45_000_000), 4_444);
        // $20 for $300 is 6.67%, rounded half up from 6.666…%.
        assert_eq!(ratio_bps(20_000_000, 300_000_000), 667);
        assert_eq!(ratio_bps(30_000_000, 10_000_000), 30_000);
        assert_eq!(ratio_bps(1, 1_000_000_000_000), 1);
        assert_eq!(ratio_bps(0, 1), 0);
    }

    #[test]
    fn wallet_units_convert_to_micros_without_loss() {
        let units = i128::from(crate::market_common::MONEY_UNITS_PER_MINOR) * 2_000;
        assert_eq!(units * MICROS_PER_MINOR / UNITS_PER_MINOR, 20_000_000);
    }

    #[test]
    fn ended_rentals_do_not_count_later_usage_in_the_old_billing_period() {
        let now = parse_time("2026-11-20T00:00:00Z").unwrap();
        let period_end = "2026-12-01T00:00:00Z";
        let ended = "2026-11-10T00:00:00Z";
        let released = "2026-11-09T00:00:00Z";
        assert_eq!(measurement_end(period_end, now, None, None).unwrap(), now);
        assert_eq!(
            measurement_end(period_end, now, Some(ended), Some(released)).unwrap(),
            parse_time(released).unwrap()
        );
        assert_eq!(
            measurement_end(period_end, now, Some(ended), None).unwrap(),
            parse_time(ended).unwrap()
        );
    }

    #[tokio::test]
    async fn a_trialing_or_unbilled_rental_reports_why_there_is_no_ratio() {
        let store = crate::store::AppStore::new_in_memory_for_tests().expect("test store");
        let rental = |status: Option<&str>| RentalPeriod {
            share_id: "share".into(),
            renter_email: "renter@example.com".into(),
            renter_user_id: "renter".into(),
            owner_user_id: "owner".into(),
            contract_status: status.map(str::to_string),
            period: None,
            paid_started_at: None,
            ended_at: None,
            released_at: None,
        };
        let now = Utc::now();
        let trial = rental_value_usage(&store, "sub", rental(Some("trialing")), now, 30)
            .await
            .unwrap();
        assert_eq!(trial.reason, Some("trial"));
        assert_eq!(trial.ratio_bps, None);
        let unbilled = rental_value_usage(&store, "sub", rental(Some("active")), now, 30)
            .await
            .unwrap();
        assert_eq!(unbilled.reason, Some("no_paid_period"));
    }

    #[tokio::test]
    async fn failed_private_reads_are_not_cacheable() {
        let response =
            no_store(AppError::Unauthorized("login required".into()).into_response()).await;
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
    }
}
