use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde_json::Value;
use tokio::sync::watch;

use crate::db::{Connection, OptionalExtension};
use crate::error::AppError;
use crate::store::AppStore;

pub(crate) fn router() -> axum::Router<crate::ServerState> {
    axum::Router::new().route("/v1/market/config", axum::routing::get(get_config))
}

async fn get_config(
    axum::extract::State(state): axum::extract::State<crate::ServerState>,
) -> impl axum::response::IntoResponse {
    let fx = state.store.market_fx_snapshot();
    let fresh = fx.is_fresh_at(Utc::now().timestamp());
    (
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        axum::Json(serde_json::json!({
            "currency": crate::market_common::MARKET_CURRENCY,
            "usdCnyRateMicros": fx.rate_micros,
            "fxRateSource": if fresh { "api.exchangerate.fun" } else if fx.source_timestamp.is_some() { "stale_cache" } else { "fallback" },
            "fxRateSourceTimestamp": fx.source_timestamp,
            "fxRateFetchedAt": fx.fetched_at,
            "fxRateFresh": fresh,
        })),
    )
}

pub(crate) const SOURCE_URL: &str = "https://api.exchangerate.fun/latest";
const POLL_INTERVAL: Duration = Duration::from_secs(15 * 60);
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_SOURCE_AGE_SECS: i64 = 48 * 60 * 60;
const MAX_FUTURE_SKEW_SECS: i64 = 5 * 60;

#[derive(Debug, Clone, Copy)]
pub(crate) struct MarketFxSnapshot {
    pub rate_micros: i64,
    pub source_timestamp: Option<i64>,
    pub fetched_at: Option<i64>,
}

impl MarketFxSnapshot {
    pub fn fallback(rate_micros: i64) -> Self {
        Self {
            rate_micros,
            source_timestamp: None,
            fetched_at: None,
        }
    }

    pub fn is_fresh_at(self, now: i64) -> bool {
        self.source_timestamp.is_some_and(|source| {
            source <= now + MAX_FUTURE_SKEW_SECS
                && now.saturating_sub(source) <= MAX_SOURCE_AGE_SECS
        })
    }
}

pub(crate) fn load_snapshot(
    conn: &Connection,
    fallback_rate_micros: i64,
) -> Result<MarketFxSnapshot, AppError> {
    conn.query_row(
        "SELECT rate_micros, source_timestamp, fetched_at FROM market_fx_rate_cache WHERE id = 1",
        [],
        |row| {
            Ok(MarketFxSnapshot {
                rate_micros: row.get(0)?,
                source_timestamp: Some(row.get(1)?),
                fetched_at: Some(row.get(2)?),
            })
        },
    )
    .optional()
    .map(|value| value.unwrap_or_else(|| MarketFxSnapshot::fallback(fallback_rate_micros)))
    .map_err(|error| AppError::Internal(format!("load verified market FX rate failed: {error}")))
}

fn parse_latest(body: &[u8], now: i64) -> Result<MarketFxSnapshot> {
    let value: Value = serde_json::from_slice(body).context("invalid FX response JSON")?;
    if value.get("base").and_then(Value::as_str) != Some("USD") {
        bail!("FX response base is not USD");
    }
    let source_timestamp = value
        .get("timestamp")
        .and_then(Value::as_i64)
        .context("FX response timestamp is missing")?;
    if source_timestamp > now + MAX_FUTURE_SKEW_SECS
        || now.saturating_sub(source_timestamp) > MAX_SOURCE_AGE_SECS
    {
        bail!("FX response is stale or future-dated");
    }
    let rate = value
        .pointer("/rates/CNY")
        .and_then(Value::as_f64)
        .context("FX response CNY rate is missing")?;
    if !rate.is_finite() || !(1.0..=20.0).contains(&rate) {
        bail!("FX response CNY rate is outside safe bounds");
    }
    let rate_micros = (rate * 1_000_000.0).round() as i64;
    Ok(MarketFxSnapshot {
        rate_micros,
        source_timestamp: Some(source_timestamp),
        fetched_at: Some(now),
    })
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("cc-switch-router/0.1 market-fx")
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .context("build market FX HTTP client failed")
}

pub(crate) async fn refresh_on_startup(store: &AppStore) -> Result<()> {
    refresh_once(store, &client()?).await
}

pub(crate) async fn refresh_once(store: &AppStore, client: &reqwest::Client) -> Result<()> {
    let mut response = client
        .get(SOURCE_URL)
        .send()
        .await
        .context("fetch USD/CNY rate failed")?
        .error_for_status()
        .context("USD/CNY rate service returned an error")?;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.context("read FX response failed")? {
        if body.len().saturating_add(chunk.len()) > MAX_BODY_BYTES {
            bail!("FX response exceeds size limit");
        }
        body.extend_from_slice(&chunk);
    }
    let snapshot = parse_latest(&body, Utc::now().timestamp())?;
    store.publish_market_fx_rate(snapshot).await?;
    tracing::info!(
        rate_micros = snapshot.rate_micros,
        source_timestamp = snapshot.source_timestamp.unwrap_or_default(),
        "verified USD/CNY rate refreshed"
    );
    Ok(())
}

pub(crate) async fn run_service(
    store: AppStore,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let client = client()?;
    let mut interval = tokio::time::interval(POLL_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Err(error) = refresh_once(&store, &client).await {
                    tracing::warn!(error = %error, "USD/CNY rate refresh failed; retaining last verified quote");
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { return Ok(()); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_fresh_usd_cny_quotes() {
        let now = 1_792_030_000;
        let fixture = serde_json::json!({
            "timestamp": now - 3_600,
            "base": "USD",
            "rates": { "CNY": 6.6931 }
        });
        let quote = parse_latest(fixture.to_string().as_bytes(), now).unwrap();
        assert_eq!(quote.rate_micros, 6_693_100);
        assert!(quote.is_fresh_at(now));

        for invalid in [
            serde_json::json!({ "timestamp": now - 3_600, "base": "EUR", "rates": { "CNY": 6.6931 } }),
            serde_json::json!({ "timestamp": now - 172_801, "base": "USD", "rates": { "CNY": 6.6931 } }),
            serde_json::json!({ "timestamp": now + 301, "base": "USD", "rates": { "CNY": 6.6931 } }),
            serde_json::json!({ "timestamp": now - 3_600, "base": "USD", "rates": { "CNY": 0.01 } }),
            serde_json::json!({ "timestamp": now - 3_600, "base": "USD", "rates": { "CNH": 6.6931 } }),
        ] {
            assert!(parse_latest(invalid.to_string().as_bytes(), now).is_err());
        }
    }

    #[tokio::test]
    async fn verified_quote_is_persisted_and_cannot_regress() {
        let store = AppStore::new_in_memory_for_tests().unwrap();
        let now = Utc::now().timestamp();
        let newer = MarketFxSnapshot {
            rate_micros: 6_693_100,
            source_timestamp: Some(now),
            fetched_at: Some(now),
        };
        store.publish_market_fx_rate(newer).await.unwrap();
        let older = MarketFxSnapshot {
            rate_micros: 7_000_000,
            source_timestamp: Some(now - 60),
            fetched_at: Some(now),
        };
        store.publish_market_fx_rate(older).await.unwrap();
        assert_eq!(store.market_fx_snapshot().rate_micros, newer.rate_micros);
        let conn = store.conn.lock().await;
        let restored = load_snapshot(&conn, 7_000_000).unwrap();
        assert_eq!(restored.rate_micros, newer.rate_micros);
        assert!(restored.is_fresh_at(now));
    }
}
