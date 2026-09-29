use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::db::{Connection, Transaction, TransactionBehavior, params};
use crate::error::AppError;
use crate::store::AppStore;

pub(crate) const ALGORITHM_VERSION: &str = "provider-rank-v1";
pub(crate) const REFRESH_INTERVAL_SECS: u64 = 15 * 60;
const SELECTION_WINDOW_DAYS: i64 = 90;
const SHARE_QUALITY_WINDOW_DAYS: i64 = 400;
const GENERATION_RETENTION_DAYS: i64 = 30;
const PERFORMANCE_MIN_GENERATION_MS: i64 = 100;
const MIN_INDEPENDENT_BUYERS: i64 = 3;
const MIN_OBSERVATION_DAYS: i64 = 7;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RankProjection {
    pub rank_state: String,
    pub rank_position: Option<i64>,
    pub score_bps: Option<i64>,
}

/// Read only the last fully published generation. Failed or partially built
/// generations must never affect buyer-facing market projections.
pub(crate) fn latest_rank_projections(
    conn: &Connection,
) -> Result<HashMap<String, RankProjection>, AppError> {
    let rows = conn
        .prepare(
            "SELECT entry.market_provider_id, entry.rank_state,
                    entry.rank_position, entry.score_bps
             FROM market_provider_rank_entries entry
             JOIN market_provider_profiles profile
               ON profile.id = entry.market_provider_id
             WHERE entry.generation_id = (
                 SELECT id FROM market_provider_rank_generations
                 WHERE state = 'published'
                 ORDER BY published_at DESC, created_at DESC LIMIT 1
             )
               AND profile.status IN ('active', 'paused')",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        RankProjection {
                            rank_state: row.get(1)?,
                            rank_position: row.get(2)?,
                            score_bps: row.get(3)?,
                        },
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("read latest Market Provider rank projections"))?;
    Ok(rows.into_iter().collect())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RankSignals {
    pub service_quality: f64,
    pub effective_choice: f64,
    pub fulfillment: f64,
    pub supply_breadth: f64,
    pub independent_buyers: i64,
    pub observation_days: i64,
}

impl RankSignals {
    pub(crate) fn eligible(self) -> bool {
        self.independent_buyers >= MIN_INDEPENDENT_BUYERS
            && self.observation_days >= MIN_OBSERVATION_DAYS
    }

    pub(crate) fn score(self) -> f64 {
        (0.45 * self.service_quality
            + 0.30 * self.effective_choice
            + 0.15 * self.fulfillment
            + 0.10 * self.supply_breadth)
            .clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone)]
struct Candidate {
    profile_id: String,
    signals: RankSignals,
    service_quality_bps: i64,
    effective_choice_bps: i64,
    fulfillment_bps: i64,
    supply_breadth_bps: i64,
    share_probe_count: i64,
    ttft_ms: Option<f64>,
    tps: Option<f64>,
    metrics_json: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderMetrics {
    share_probe_successes: i64,
    share_probe_total: i64,
    host_online_samples: i64,
    host_observed_samples: i64,
    fulfillment_successes: i64,
    fulfillment_total: i64,
    effective_choice_weight_millis: i64,
    active_share_count: i64,
    available_share_seats: i64,
    host_total: i64,
    idle_host_total: i64,
    country_count: i64,
    app_count: i64,
}

fn map_db(context: &'static str) -> impl FnOnce(crate::db::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

fn bps(value: f64) -> i64 {
    (value.clamp(0.0, 1.0) * 10_000.0).round() as i64
}

/// 95% Wilson lower confidence bound. Sparse perfect histories should not
/// outrank a well-observed Provider merely because their raw ratio is 100%.
pub(crate) fn wilson_lower_bound(successes: i64, total: i64) -> f64 {
    if total <= 0 || successes < 0 {
        return 0.0;
    }
    let successes = successes.min(total) as f64;
    let total = total as f64;
    let z = 1.959_963_984_540_054_f64;
    let proportion = successes / total;
    let denominator = 1.0 + z * z / total;
    let centre = proportion + z * z / (2.0 * total);
    let margin = z * ((proportion * (1.0 - proportion) + z * z / (4.0 * total)) / total).sqrt();
    ((centre - margin) / denominator).clamp(0.0, 1.0)
}

fn blended_quality(
    probe_success: i64,
    probe_total: i64,
    online_samples: i64,
    observed_samples: i64,
) -> f64 {
    match (probe_total > 0, observed_samples > 0) {
        (true, true) => {
            0.7 * wilson_lower_bound(probe_success, probe_total)
                + 0.3 * wilson_lower_bound(online_samples, observed_samples)
        }
        (true, false) => wilson_lower_bound(probe_success, probe_total),
        (false, true) => wilson_lower_bound(online_samples, observed_samples),
        (false, false) => 0.0,
    }
}

fn choice_score(weight_millis: i64) -> f64 {
    // Eight fully weighted independent buyers reaches roughly 63%; additional
    // buyers still matter but no single popularity wave can dominate quality.
    1.0 - (-(weight_millis.max(0) as f64 / 8_000.0)).exp()
}

fn breadth_score(apps: i64, countries: i64, supply_units: i64) -> f64 {
    let apps = (apps.max(0) as f64 / 3.0).min(1.0);
    let countries = (countries.max(0) as f64 / 5.0).min(1.0);
    let supply = (supply_units.max(0) as f64 / 10.0).min(1.0);
    0.5 * apps + 0.25 * countries + 0.25 * supply
}

fn validate_share_windows_tx(tx: &Transaction<'_>) -> Result<(), AppError> {
    let invalid_timestamp: bool = tx
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM market_provider_share_windows
                WHERE julianday(starts_at) IS NULL
                   OR (ends_at IS NOT NULL AND julianday(ends_at) IS NULL)
                   OR (ends_at IS NOT NULL
                       AND julianday(ends_at) < julianday(starts_at))
             )",
            [],
            |row| row.get(0),
        )
        .map_err(map_db("validate Market Provider Share window timestamps"))?;
    if invalid_timestamp {
        return Err(AppError::Internal(
            "Market Provider Share window contains an invalid or reversed timestamp".into(),
        ));
    }
    let overlap: bool = tx
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM market_provider_share_windows left_window
                JOIN market_provider_share_windows right_window
                  ON right_window.listing_id = left_window.listing_id
                 AND right_window.id > left_window.id
                WHERE julianday(left_window.starts_at) <
                      COALESCE(julianday(right_window.ends_at), 5373484.499999)
                  AND julianday(right_window.starts_at) <
                      COALESCE(julianday(left_window.ends_at), 5373484.499999)
             )",
            [],
            |row| row.get(0),
        )
        .map_err(map_db("validate Market Provider Share window overlap"))?;
    if overlap {
        return Err(AppError::Internal(
            "Market Provider Share windows overlap".into(),
        ));
    }
    let lifecycle_drift: bool = tx
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM share_market_listings listing
                LEFT JOIN market_provider_share_windows open_window
                  ON open_window.listing_id = listing.id
                 AND open_window.ends_at IS NULL
                WHERE (
                    listing.status = 'active' AND listing.deleted_at IS NULL
                    AND listing.market_provider_id IS NOT NULL
                    AND (open_window.id IS NULL
                         OR open_window.market_provider_id != listing.market_provider_id)
                ) OR (
                    (listing.status != 'active' OR listing.deleted_at IS NOT NULL
                     OR listing.market_provider_id IS NULL)
                    AND open_window.id IS NOT NULL
                )
             )",
            [],
            |row| row.get(0),
        )
        .map_err(map_db("validate Market Provider Share window lifecycle"))?;
    if lifecycle_drift {
        return Err(AppError::Internal(
            "Market Provider Share window lifecycle drift detected".into(),
        ));
    }
    Ok(())
}

fn refresh_effective_selections_tx(
    tx: &Transaction<'_>,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    #[derive(Debug)]
    struct Selection {
        first: String,
        last: String,
        source_mask: i64,
        weight_millis: i64,
    }

    let cutoff = (now - Duration::days(SELECTION_WINDOW_DAYS)).to_rfc3339();
    let mut selections = HashMap::<(String, String), Selection>::new();
    let share_rows = tx
        .prepare(
            "SELECT window.market_provider_id, subscription.renter_user_id,
                    MIN(subscription.activated_at), MAX(subscription.activated_at),
                    MAX(CASE WHEN subscription.daily_rate_minor IS NULL
                                  AND subscription.cycle_price_minor IS NULL
                             THEN 250 ELSE 1000 END)
             FROM share_market_subscriptions subscription
             JOIN market_provider_share_windows window
               ON window.listing_id = subscription.listing_id
              AND julianday(subscription.activated_at) >= julianday(window.starts_at)
              AND (window.ends_at IS NULL
                   OR julianday(subscription.activated_at) < julianday(window.ends_at))
             JOIN market_provider_profiles profile
               ON profile.id = window.market_provider_id
             WHERE subscription.activated_at >= ?1
               AND subscription.activated_at IS NOT NULL
               AND NOT (
                    (profile.user_id IS NOT NULL
                        AND subscription.renter_user_id = profile.user_id)
                    OR lower(trim(subscription.renter_email)) =
                       profile.canonical_email
               )
               AND (subscription.status != 'released'
                    OR julianday(COALESCE(subscription.released_at, subscription.updated_at))
                       - julianday(subscription.activated_at) >= 1.0)
             GROUP BY window.market_provider_id, subscription.renter_user_id",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![cutoff], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("read Share Market effective choices"))?;
    for (profile, buyer, first, last, weight) in share_rows {
        selections.insert(
            (profile, buyer),
            Selection {
                first,
                last,
                source_mask: 1,
                weight_millis: weight,
            },
        );
    }

    let client_rows = tx
        .prepare(
            "SELECT alias.market_provider_id, subscription.client_user_id,
                    MIN(subscription.activated_at), MAX(subscription.activated_at),
                    MAX(CASE WHEN subscription.daily_rate_minor IS NULL
                                  AND subscription.cycle_price_minor IS NULL
                             THEN 250 ELSE 1000 END)
             FROM client_market_subscriptions subscription
             JOIN market_provider_aliases alias
               ON alias.alias_kind = 'host_provider_id'
              AND alias.alias_value = subscription.provider_id
             JOIN market_provider_profiles profile
               ON profile.id = alias.market_provider_id
             WHERE subscription.activated_at >= ?1
               AND subscription.activated_at IS NOT NULL
               AND NOT (
                    (profile.user_id IS NOT NULL
                     AND subscription.client_user_id = profile.user_id)
                    OR lower(trim(subscription.client_owner_email)) =
                       profile.canonical_email
                    OR lower(trim(subscription.client_owner_email)) =
                       lower(trim(subscription.host_owner_email))
               )
               AND (subscription.status != 'released'
                    OR julianday(COALESCE(subscription.released_at, subscription.updated_at))
                       - julianday(subscription.activated_at) >= 1.0)
             GROUP BY alias.market_provider_id, subscription.client_user_id",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![cutoff], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("read Client Market effective choices"))?;
    for (profile, buyer, first, last, weight) in client_rows {
        selections
            .entry((profile, buyer))
            .and_modify(|selection| {
                if first < selection.first {
                    selection.first = first.clone();
                }
                if last > selection.last {
                    selection.last = last.clone();
                }
                selection.source_mask |= 2;
                selection.weight_millis = selection.weight_millis.max(weight);
            })
            .or_insert(Selection {
                first,
                last,
                source_mask: 2,
                weight_millis: weight,
            });
    }

    tx.execute("DELETE FROM market_provider_effective_selections", [])
        .map_err(map_db("clear Market Provider effective choices"))?;
    let refreshed_at = now.to_rfc3339();
    for ((profile, buyer), selection) in selections {
        tx.execute(
            "INSERT INTO market_provider_effective_selections (
                market_provider_id, buyer_user_id, first_selected_at,
                last_selected_at, source_mask, weight_millis, refreshed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                profile,
                buyer,
                selection.first,
                selection.last,
                selection.source_mask,
                selection.weight_millis,
                refreshed_at,
            ],
        )
        .map_err(map_db("write Market Provider effective choice"))?;
    }
    Ok(())
}

fn candidate_tx(
    tx: &Transaction<'_>,
    profile_id: &str,
    now: DateTime<Utc>,
) -> Result<Candidate, AppError> {
    let probe_cutoff = (now - Duration::days(SHARE_QUALITY_WINDOW_DAYS)).timestamp();
    let host_cutoff = (now - Duration::days(30)).format("%Y-%m-%d").to_string();
    let request_cutoff = (now - Duration::days(30)).timestamp();
    let (probe_success, probe_total): (i64, i64) = tx
        .query_row(
            "SELECT COALESCE(SUM(CASE WHEN observation.outcome = 'success' THEN 1 ELSE 0 END), 0),
                    COUNT(observation.observation_id)
             FROM share_model_probe_observations observation
             WHERE observation.slot_start >= ?2
               AND EXISTS (
                   SELECT 1 FROM share_model_health_slots slot
                   JOIN share_market_listings listing
                     ON listing.share_id = slot.share_id
                   JOIN market_provider_share_windows window
                     ON window.listing_id = listing.id
                   WHERE slot.observation_id = observation.observation_id
                     AND slot.slot_start = observation.slot_start
                     AND listing.installation_id = observation.installation_id
                     AND window.market_provider_id = ?1
                     AND observation.slot_start >= unixepoch(window.starts_at)
                     AND (window.ends_at IS NULL
                          OR observation.slot_start < unixepoch(window.ends_at))
               )",
            params![profile_id, probe_cutoff],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_db("read Market Provider Share quality"))?;
    let (online_samples, observed_samples): (i64, i64) = tx
        .query_row(
            "SELECT COALESCE(SUM(stats.online_samples), 0),
                    COALESCE(SUM(stats.observed_samples), 0)
             FROM host_provider_profiles host
             JOIN host_provider_daily_stats stats ON stats.provider_id = host.provider_id
             WHERE host.market_provider_id = ?1 AND stats.stat_date >= ?2",
            params![profile_id, host_cutoff],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_db("read Market Provider Host quality"))?;
    let observation_days: i64 = tx
        .query_row(
            "SELECT COUNT(DISTINCT observation_day) FROM (
                SELECT date(observation.slot_start, 'unixepoch') AS observation_day
                FROM share_model_probe_observations observation
                WHERE observation.slot_start >= ?2
                  AND EXISTS (
                      SELECT 1 FROM share_model_health_slots slot
                      JOIN share_market_listings listing
                        ON listing.share_id = slot.share_id
                      JOIN market_provider_share_windows window
                        ON window.listing_id = listing.id
                      WHERE slot.observation_id = observation.observation_id
                        AND slot.slot_start = observation.slot_start
                        AND listing.installation_id = observation.installation_id
                        AND window.market_provider_id = ?1
                        AND observation.slot_start >= unixepoch(window.starts_at)
                        AND (window.ends_at IS NULL
                             OR observation.slot_start < unixepoch(window.ends_at))
                  )
                UNION
                SELECT stats.stat_date AS observation_day
                FROM host_provider_profiles host
                JOIN host_provider_daily_stats stats ON stats.provider_id = host.provider_id
                WHERE host.market_provider_id = ?1 AND stats.stat_date >= ?3
                  AND (stats.host_samples > 0 OR stats.observed_samples > 0)
             )",
            params![profile_id, probe_cutoff, host_cutoff],
            |row| row.get(0),
        )
        .map_err(map_db("count Market Provider observation days"))?;
    let (independent_buyers, choice_weight): (i64, i64) = tx
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(weight_millis), 0)
             FROM market_provider_effective_selections WHERE market_provider_id = ?1",
            params![profile_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_db("read Market Provider effective choice score"))?;
    let (share_success, share_total): (i64, i64) = tx
        .query_row(
            "SELECT COALESCE(SUM(CASE WHEN subscription.activated_at IS NOT NULL THEN 1 ELSE 0 END), 0),
                    COUNT(*)
             FROM share_market_subscriptions subscription
             JOIN market_provider_share_windows window
               ON window.listing_id = subscription.listing_id
              AND julianday(COALESCE(subscription.activated_at, subscription.created_at))
                    >= julianday(window.starts_at)
              AND (window.ends_at IS NULL
                   OR julianday(COALESCE(subscription.activated_at, subscription.created_at))
                        < julianday(window.ends_at))
             JOIN market_provider_profiles profile
               ON profile.id = window.market_provider_id
             WHERE window.market_provider_id = ?1
               AND NOT (
                    (profile.user_id IS NOT NULL
                        AND subscription.renter_user_id = profile.user_id)
                    OR lower(trim(subscription.renter_email)) =
                       profile.canonical_email
               )
               AND (subscription.activated_at IS NOT NULL
                    OR subscription.status = 'grant_failed')",
            params![profile_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_db("read Market Provider Share fulfillment"))?;
    let (client_success, client_total): (i64, i64) = tx
        .query_row(
            "SELECT COALESCE(SUM(CASE WHEN job.status = 'succeeded' THEN 1 ELSE 0 END), 0),
                    COUNT(*)
             FROM provisioning_jobs job
             JOIN router_ssh_hosts host ON host.id = job.host_id
             JOIN market_provider_profiles profile ON profile.id = ?1
             WHERE lower(trim(COALESCE(NULLIF(job.host_owner_email, ''),
                                       host.host_owner_email))) = profile.canonical_email
               AND NOT (
                    (profile.user_id IS NOT NULL
                     AND COALESCE(job.client_owner_user_id, '') = profile.user_id)
                    OR lower(trim(COALESCE(job.client_owner_email, ''))) =
                       profile.canonical_email
                    OR lower(trim(COALESCE(job.client_owner_email, ''))) =
                       lower(trim(COALESCE(NULLIF(job.host_owner_email, ''),
                                           host.host_owner_email)))
               )
               AND job.type = 'create' AND job.status IN ('succeeded', 'failed')",
            params![profile_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_db("read Market Provider Client fulfillment"))?;
    let fulfillment_success = share_success.saturating_add(client_success);
    let fulfillment_total = share_total.saturating_add(client_total);
    let (active_shares, available_seats): (i64, i64) = tx
        .query_row(
            "SELECT COUNT(DISTINCT listing.id),
                    COUNT(DISTINCT CASE WHEN seat.status = 'available'
                                             AND seat.retired_at IS NULL
                                        THEN seat.id END)
             FROM share_market_listings listing
             JOIN shares share ON share.share_id = listing.share_id
             LEFT JOIN share_market_seats seat ON seat.listing_id = listing.id
             WHERE listing.market_provider_id = ?1
               AND listing.status = 'active' AND listing.deleted_at IS NULL
               AND share.share_status = 'active'
               AND lower(COALESCE(share.owner_email, '')) = lower(listing.owner_email)
               AND EXISTS (
                   SELECT 1 FROM share_market_seats visible_seat
                   WHERE visible_seat.listing_id = listing.id
                     AND visible_seat.retired_at IS NULL
                     AND visible_seat.status IN ('available', 'reserved', 'occupied', 'revoking')
               )",
            params![profile_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_db("read Market Provider Share breadth"))?;
    let (host_total, idle_hosts, countries): (i64, i64, i64) = tx
        .query_row(
            "SELECT COUNT(host.id),
                    COALESCE(SUM(CASE WHEN host.status = 'idle' THEN 1 ELSE 0 END), 0),
                    COUNT(DISTINCT CASE WHEN host.country_code IS NOT NULL
                                       THEN host.country_code END)
             FROM host_provider_profiles provider
             LEFT JOIN router_ssh_hosts host
               ON host.provider_id = provider.provider_id AND host.status != 'disabled'
             WHERE provider.market_provider_id = ?1",
            params![profile_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(map_db("read Market Provider Host breadth"))?;
    let app_count: i64 = tx
        .query_row(
            "SELECT COUNT(DISTINCT binding.app_type)
             FROM share_market_listings listing
             JOIN shares share ON share.share_id = listing.share_id
             JOIN share_bindings binding ON binding.share_id = listing.share_id
             WHERE listing.market_provider_id = ?1
               AND listing.status = 'active' AND listing.deleted_at IS NULL
               AND share.share_status = 'active'
               AND lower(COALESCE(share.owner_email, '')) = lower(listing.owner_email)
               AND (
                   (binding.app_type = 'claude' AND share.enabled_claude != 0)
                   OR (binding.app_type = 'codex' AND share.enabled_codex != 0)
                   OR (binding.app_type = 'gemini' AND share.enabled_gemini != 0)
               )
               AND EXISTS (
                   SELECT 1 FROM share_market_seats visible_seat
                   WHERE visible_seat.listing_id = listing.id
                     AND visible_seat.retired_at IS NULL
                     AND visible_seat.status IN ('available', 'reserved', 'occupied', 'revoking')
               )",
            params![profile_id],
            |row| row.get(0),
        )
        .map_err(map_db("read Market Provider app breadth"))?;
    let (ttft_ms, tps) = tx
        .query_row(
            "SELECT AVG(CASE WHEN log.is_streaming != 0
                                  AND lower(COALESCE(log.stream_status, '')) = 'completed'
                                  AND log.first_token_ms > 0
                                  AND log.latency_ms > log.first_token_ms
                             THEN log.first_token_ms END),
                    AVG(CASE WHEN log.is_streaming != 0
                                  AND lower(COALESCE(log.stream_status, '')) = 'completed'
                                  AND lower(log.usage_state) = 'observed'
                                  AND log.first_token_ms > 0
                                  AND log.output_tokens > 0
                                  AND log.latency_ms - log.first_token_ms
                                      >= MAX(?3, (log.latency_ms + 99) / 100)
                             THEN (log.output_tokens * 1000.0)
                                  / (log.latency_ms - log.first_token_ms) END)
             FROM share_request_logs log
             WHERE log.created_at >= ?2
               AND log.status_code BETWEEN 200 AND 299 AND log.is_health_check = 0
               AND EXISTS (
                   SELECT 1 FROM share_market_listings listing
                   JOIN market_provider_share_windows window
                     ON window.listing_id = listing.id
                   WHERE window.market_provider_id = ?1
                     AND listing.share_id = log.share_id
                     AND log.created_at >= unixepoch(window.starts_at)
                     AND (window.ends_at IS NULL
                          OR log.created_at < unixepoch(window.ends_at))
               )",
            params![profile_id, request_cutoff, PERFORMANCE_MIN_GENERATION_MS],
            |row| Ok((row.get::<_, Option<f64>>(0)?, row.get::<_, Option<f64>>(1)?)),
        )
        .map_err(map_db("read display-only Market Provider performance"))?;

    let service_quality =
        blended_quality(probe_success, probe_total, online_samples, observed_samples);
    let effective_choice = choice_score(choice_weight);
    let fulfillment = wilson_lower_bound(fulfillment_success, fulfillment_total);
    let supply_breadth = breadth_score(
        app_count,
        countries,
        available_seats.saturating_add(idle_hosts),
    );
    let signals = RankSignals {
        service_quality,
        effective_choice,
        fulfillment,
        supply_breadth,
        independent_buyers,
        observation_days,
    };
    let metrics_json = serde_json::to_string(&ProviderMetrics {
        share_probe_successes: probe_success,
        share_probe_total: probe_total,
        host_online_samples: online_samples,
        host_observed_samples: observed_samples,
        fulfillment_successes: fulfillment_success,
        fulfillment_total,
        effective_choice_weight_millis: choice_weight,
        active_share_count: active_shares,
        available_share_seats: available_seats,
        host_total,
        idle_host_total: idle_hosts,
        country_count: countries,
        app_count,
    })
    .map_err(|error| {
        AppError::Internal(format!("encode Market Provider metrics failed: {error}"))
    })?;
    Ok(Candidate {
        profile_id: profile_id.to_string(),
        signals,
        service_quality_bps: bps(service_quality),
        effective_choice_bps: bps(effective_choice),
        fulfillment_bps: bps(fulfillment),
        supply_breadth_bps: bps(supply_breadth),
        share_probe_count: probe_total,
        ttft_ms,
        tps,
        metrics_json,
    })
}

fn generate_tx(
    tx: &Transaction<'_>,
    now: DateTime<Utc>,
    generation_id: &str,
) -> Result<usize, AppError> {
    let observed_from = (now - Duration::days(SHARE_QUALITY_WINDOW_DAYS)).to_rfc3339();
    let observed_to = now.to_rfc3339();
    tx.execute(
        "INSERT INTO market_provider_rank_generations (
            id, algorithm_version, state, observed_from, observed_to,
            provider_count, created_at
         ) VALUES (?1, ?2, 'building', ?3, ?4, 0, ?4)",
        params![generation_id, ALGORITHM_VERSION, observed_from, observed_to],
    )
    .map_err(map_db("start Market Provider rank generation"))?;
    validate_share_windows_tx(tx)?;
    refresh_effective_selections_tx(tx, now)?;
    let profile_ids = tx
        .prepare(
            "SELECT profile.id FROM market_provider_profiles profile
             WHERE profile.status IN ('active', 'paused')
               AND (
                   EXISTS (
                       SELECT 1 FROM share_market_listings listing
                       JOIN shares share ON share.share_id = listing.share_id
                       WHERE listing.market_provider_id = profile.id
                         AND listing.status = 'active' AND listing.deleted_at IS NULL
                         AND share.share_status = 'active'
                         AND lower(COALESCE(share.owner_email, '')) =
                             lower(listing.owner_email)
                         AND EXISTS (
                             SELECT 1 FROM share_market_seats visible_seat
                             WHERE visible_seat.listing_id = listing.id
                               AND visible_seat.retired_at IS NULL
                               AND visible_seat.status IN (
                                   'available', 'reserved', 'occupied', 'revoking'
                               )
                         )
                   )
                   OR EXISTS (
                       SELECT 1 FROM host_provider_profiles provider
                       JOIN router_ssh_hosts host ON host.provider_id = provider.provider_id
                       WHERE provider.market_provider_id = profile.id
                         AND host.status != 'disabled'
                   )
               )
             ORDER BY profile.id",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("read Market Provider rank candidates"))?;
    let mut candidates = profile_ids
        .iter()
        .map(|profile_id| candidate_tx(tx, profile_id, now))
        .collect::<Result<Vec<_>, _>>()?;
    candidates.sort_by(|left, right| {
        right
            .signals
            .eligible()
            .cmp(&left.signals.eligible())
            .then_with(|| {
                right
                    .signals
                    .score()
                    .partial_cmp(&left.signals.score())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| left.profile_id.cmp(&right.profile_id))
    });
    let mut rank_position = 0i64;
    for candidate in &candidates {
        let eligible = candidate.signals.eligible();
        if eligible {
            rank_position += 1;
        }
        tx.execute(
            "INSERT INTO market_provider_rank_entries (
                generation_id, market_provider_id, rank_position, rank_state,
                score_bps, service_quality_bps, effective_choice_bps,
                fulfillment_bps, supply_breadth_bps, independent_buyer_count,
                observation_days, share_probe_count, ttft_ms, tps,
                metrics_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                       ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                generation_id,
                candidate.profile_id,
                eligible.then_some(rank_position),
                if eligible { "ranked" } else { "collecting" },
                eligible.then(|| bps(candidate.signals.score())),
                candidate.service_quality_bps,
                candidate.effective_choice_bps,
                candidate.fulfillment_bps,
                candidate.supply_breadth_bps,
                candidate.signals.independent_buyers,
                candidate.signals.observation_days,
                candidate.share_probe_count,
                candidate.ttft_ms,
                candidate.tps,
                candidate.metrics_json,
                observed_to,
            ],
        )
        .map_err(map_db("write Market Provider rank entry"))?;
    }
    tx.execute(
        "UPDATE market_provider_rank_generations
         SET state = 'published', provider_count = ?2, published_at = ?3
         WHERE id = ?1 AND state = 'building'",
        params![
            generation_id,
            i64::try_from(candidates.len()).unwrap_or(i64::MAX),
            observed_to,
        ],
    )
    .map_err(map_db("publish Market Provider rank generation"))?;
    tx.execute(
        "DELETE FROM market_provider_rank_generations
         WHERE id != ?1 AND created_at < ?2",
        params![
            generation_id,
            (now - Duration::days(GENERATION_RETENTION_DAYS)).to_rfc3339(),
        ],
    )
    .map_err(map_db("prune old Market Provider rank generations"))?;
    Ok(candidates.len())
}

/// Build the complete generation inside one IMMEDIATE transaction. Readers see
/// either the previous published generation or the new one, never a partial set.
pub(crate) fn generate_conn(conn: &Connection, now: DateTime<Utc>) -> Result<usize, AppError> {
    let generation_id = Uuid::new_v4().to_string();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_db("begin Market Provider rank generation"))?;
    match generate_tx(&tx, now, &generation_id) {
        Ok(count) => {
            tx.commit()
                .map_err(map_db("commit Market Provider rank generation"))?;
            Ok(count)
        }
        Err(error) => {
            drop(tx);
            // Best-effort diagnostic only. A failure never replaces the last
            // published generation, and diagnostic text contains no raw rows.
            let timestamp = now.to_rfc3339();
            let _ = conn.execute(
                "INSERT INTO market_provider_rank_generations (
                    id, algorithm_version, state, observed_from, observed_to,
                    provider_count, failure_summary, created_at
                 ) VALUES (?1, ?2, 'failed', ?3, ?4, 0, ?5, ?4)",
                params![
                    generation_id,
                    ALGORITHM_VERSION,
                    (now - Duration::days(SHARE_QUALITY_WINDOW_DAYS)).to_rfc3339(),
                    timestamp,
                    "generation_failed",
                ],
            );
            let _ = conn.execute(
                "DELETE FROM market_provider_rank_generations
                 WHERE created_at < ?1
                   AND id NOT IN (
                       SELECT id FROM market_provider_rank_generations
                       WHERE state = 'published'
                       ORDER BY published_at DESC, created_at DESC LIMIT 1
                   )",
                params![(now - Duration::days(GENERATION_RETENTION_DAYS)).to_rfc3339()],
            );
            Err(error)
        }
    }
}

impl AppStore {
    pub(crate) async fn refresh_market_provider_rank(
        &self,
        now: DateTime<Utc>,
    ) -> Result<usize, AppError> {
        let conn = self.conn.lock().await;
        crate::market_provider_identity::reconcile_conn(&conn, now)?;
        generate_conn(&conn, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let conn = Connection::open_in_memory().expect("open rank test database");
        conn.pragma_update(None, "foreign_keys", "ON")
            .expect("enable foreign keys");
        crate::schema::apply(&conn).expect("install schema");
        conn
    }

    fn insert_profile(conn: &Connection, profile_id: &str, user_id: Option<&str>) {
        if let Some(user_id) = user_id {
            conn.execute(
                "INSERT INTO users (id, email_normalized, status, created_at, last_login_at)
                 VALUES (?1, ?2, 'active', 'now', 'now')",
                params![user_id, format!("{user_id}@example.com")],
            )
            .expect("insert Provider user");
        }
        conn.execute(
            "INSERT INTO market_provider_profiles (
                id, user_id, canonical_email, display_name, claim_state,
                status, created_at, updated_at, claimed_at
             ) VALUES (?1, ?2, ?3, 'Provider test', ?4, 'active', 'now', 'now', ?5)",
            params![
                profile_id,
                user_id,
                format!("{profile_id}@example.com"),
                if user_id.is_some() {
                    "claimed"
                } else {
                    "unclaimed"
                },
                user_id.map(|_| "now"),
            ],
        )
        .expect("insert Market Provider profile");
    }

    fn insert_listing(
        conn: &Connection,
        profile_id: &str,
        listing_id: &str,
        share_id: &str,
        installation_id: &str,
        status: &str,
    ) {
        conn.execute(
            "INSERT INTO share_market_listings (
                id, share_id, installation_id, owner_user_id, owner_email,
                status, created_at, updated_at, market_provider_id
             ) VALUES (?1, ?2, ?3, 'owner', 'owner@example.com',
                       ?4, '2020-01-01T00:00:00Z', '2020-01-01T00:00:00Z', ?5)",
            params![listing_id, share_id, installation_id, status, profile_id],
        )
        .expect("insert Provider Share listing");
        conn.execute(
            "INSERT INTO market_provider_share_windows (
                id, listing_id, market_provider_id, starts_at, ends_at, created_at
             ) VALUES (?1, ?2, ?3, '2020-01-01T00:00:00Z', ?4,
                       '2020-01-01T00:00:00Z')",
            params![
                format!("window-{listing_id}"),
                listing_id,
                profile_id,
                (status != "active").then_some("2020-01-01T00:00:00Z"),
            ],
        )
        .expect("insert Provider Share window");
        if status == "active" {
            conn.execute(
                "INSERT OR IGNORE INTO shares (
                    share_id, capacity_pool_id, installation_id, share_name,
                    owner_email, app_type, enabled_claude, token_limit,
                    parallel_limit, tokens_used, requests_count, share_status,
                    created_at, expires_at, user_grants_json, bindings_json,
                    supported_user_token_periods_json, config_revision, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, 'owner@example.com', 'claude', 1,
                           -1, 3, 0, 0, 'active', '2020-01-01T00:00:00Z',
                           '9999-12-31T23:59:59Z', '{}',
                           '{\"claude\":\"provider-test\"}', '[]', 1,
                           '2020-01-01T00:00:00Z')",
                params![
                    share_id,
                    format!("pool-{share_id}"),
                    installation_id,
                    format!("Share {share_id}"),
                ],
            )
            .expect("insert active Share backing row");
            conn.execute(
                "INSERT OR IGNORE INTO share_bindings (share_id, app_type, provider_id)
                 VALUES (?1, 'claude', 'provider-test')",
                params![share_id],
            )
            .expect("insert active Share binding");
            conn.execute(
                "INSERT INTO share_market_seats (
                    id, listing_id, position, status, token_period_json,
                    offer_revision, created_at, updated_at
                 ) VALUES (?1, ?2, 1, 'available', '{}', 1,
                           '2020-01-01T00:00:00Z', '2020-01-01T00:00:00Z')",
                params![format!("seat-{listing_id}"), listing_id],
            )
            .expect("insert active Share seat");
        }
    }

    fn insert_probe_projection(
        conn: &Connection,
        share_id: &str,
        observation_id: &str,
        slot_start: i64,
        outcome: &str,
    ) {
        let (status, failure_domain) = if outcome == "success" {
            ("success", None)
        } else {
            ("failed", Some("upstream"))
        };
        conn.execute(
            "INSERT INTO share_model_health_slots (
                share_id, slot_start, claim_token, claimed_at, app_type, api_type,
                requested_model, actual_model, status, provider_id,
                health_fingerprint, observation_id, outcome, failure_domain,
                evidence_scope, evidence_version, checked_at, source, updated_at
             ) VALUES (
                ?1, ?2, ?3, ?2, 'claude', 'anthropic', 'model-test', 'model-test',
                ?4, 'upstream-test', 'fingerprint-test', ?5, ?6, ?7,
                'provider_runtime', 2, ?2, 'rank-test', ?2
             )",
            params![
                share_id,
                slot_start,
                format!("claim-{share_id}-{slot_start}"),
                status,
                observation_id,
                outcome,
                failure_domain,
            ],
        )
        .expect("insert Share probe projection");
    }

    fn insert_share_subscription(
        conn: &Connection,
        profile_id: &str,
        id: &str,
        status: &str,
        renter_user_id: &str,
        activated_at: Option<&str>,
        daily_rate_minor: Option<i64>,
    ) {
        let listing_id = format!("listing-{id}");
        let share_id = format!("share-{id}");
        let seat_id = format!("seat-{id}");
        conn.execute(
            "INSERT INTO share_market_listings (
                id, share_id, installation_id, owner_user_id, owner_email,
                status, created_at, updated_at, market_provider_id
             ) VALUES (?1, ?2, ?3, 'owner', 'owner@example.com',
                       'active', 'now', 'now', ?4)",
            params![
                listing_id,
                share_id,
                format!("installation-{id}"),
                profile_id
            ],
        )
        .expect("insert ranked Share listing");
        conn.execute(
            "INSERT INTO market_provider_share_windows (
                id, listing_id, market_provider_id, starts_at, ends_at, created_at
             ) VALUES (?1, ?2, ?3, '2020-01-01T00:00:00Z', NULL,
                       '2020-01-01T00:00:00Z')",
            params![format!("window-{listing_id}"), listing_id, profile_id],
        )
        .expect("insert ranked Share window");
        conn.execute(
            "INSERT INTO share_market_seats (
                id, listing_id, position, status, token_period_json,
                daily_rate_minor, offer_revision, created_at, updated_at
             ) VALUES (?1, ?2, 1, 'occupied', '{}', ?3, 1, 'now', 'now')",
            params![seat_id, listing_id, daily_rate_minor],
        )
        .expect("insert ranked Share seat");
        conn.execute(
            "INSERT INTO share_market_subscriptions (
                id, seat_id, listing_id, share_id, installation_id,
                entitlement_id, owner_user_id, owner_email, renter_user_id,
                renter_email, status, token_period_json, daily_rate_minor,
                offer_revision, activated_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'owner', 'owner@example.com',
                       ?7, ?8, ?9, '{}', ?10, 1, ?11, 'now', 'now')",
            params![
                id,
                seat_id,
                listing_id,
                share_id,
                format!("installation-{id}"),
                format!("entitlement-{id}"),
                renter_user_id,
                format!("{renter_user_id}@example.com"),
                status,
                daily_rate_minor,
                activated_at,
            ],
        )
        .expect("insert ranked Share subscription");
    }

    #[test]
    fn wilson_bound_rewards_evidence_not_sparse_perfection() {
        assert!(wilson_lower_bound(90, 100) > wilson_lower_bound(1, 1));
        assert!(wilson_lower_bound(99, 100) > wilson_lower_bound(90, 100));
        assert_eq!(wilson_lower_bound(0, 0), 0.0);
    }

    #[test]
    fn rank_requires_both_independent_buyers_and_observation_days() {
        let base = RankSignals {
            service_quality: 1.0,
            effective_choice: 1.0,
            fulfillment: 1.0,
            supply_breadth: 1.0,
            independent_buyers: 3,
            observation_days: 7,
        };
        assert!(base.eligible());
        assert!(
            !RankSignals {
                independent_buyers: 2,
                ..base
            }
            .eligible()
        );
        assert!(
            !RankSignals {
                observation_days: 6,
                ..base
            }
            .eligible()
        );
    }

    #[test]
    fn weights_match_provider_rank_v1_contract() {
        let quality = RankSignals {
            service_quality: 1.0,
            effective_choice: 0.0,
            fulfillment: 0.0,
            supply_breadth: 0.0,
            independent_buyers: 3,
            observation_days: 7,
        };
        let choices = RankSignals {
            service_quality: 0.0,
            effective_choice: 1.0,
            ..quality
        };
        assert!((quality.score() - 0.45).abs() < f64::EPSILON);
        assert!((choices.score() - 0.30).abs() < f64::EPSILON);
    }

    #[test]
    fn share_choice_and_fulfillment_require_real_activation() {
        let conn = database();
        insert_profile(&conn, "mp_share", None);
        conn.execute(
            "UPDATE market_provider_profiles
             SET canonical_email = 'owner@example.com' WHERE id = 'mp_share'",
            [],
        )
        .expect("align ranked Share owner identity");
        let now = Utc::now();
        let activated_at = (now - Duration::hours(2)).to_rfc3339();
        insert_share_subscription(
            &conn,
            "mp_share",
            "pending",
            "grant_pending",
            "pending-buyer",
            None,
            Some(100),
        );
        insert_share_subscription(
            &conn,
            "mp_share",
            "failed",
            "grant_failed",
            "failed-buyer",
            None,
            Some(100),
        );
        insert_share_subscription(
            &conn,
            "mp_share",
            "self",
            "active",
            "owner",
            Some(&activated_at),
            Some(100),
        );
        insert_share_subscription(
            &conn,
            "mp_share",
            "self-email",
            "active",
            "legacy-owner-id",
            Some(&activated_at),
            Some(100),
        );
        conn.execute(
            "UPDATE share_market_subscriptions
             SET renter_email = owner_email WHERE id = 'self-email'",
            [],
        )
        .expect("simulate legacy Share owner id drift");
        insert_share_subscription(
            &conn,
            "mp_share",
            "active",
            "active",
            "active-buyer",
            Some(&activated_at),
            None,
        );

        let tx = conn.transaction().expect("start rank signal transaction");
        refresh_effective_selections_tx(&tx, now).expect("refresh effective choices");
        let selections: Vec<(String, i64)> = tx
            .prepare(
                "SELECT buyer_user_id, weight_millis
                 FROM market_provider_effective_selections ORDER BY buyer_user_id",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .expect("read effective choices");
        assert_eq!(selections, vec![("active-buyer".into(), 250)]);

        let candidate = candidate_tx(&tx, "mp_share", now).expect("calculate candidate");
        let metrics: serde_json::Value =
            serde_json::from_str(&candidate.metrics_json).expect("decode candidate metrics");
        assert_eq!(metrics["fulfillmentSuccesses"], 1);
        assert_eq!(metrics["fulfillmentTotal"], 2);
        assert_eq!(metrics["effectiveChoiceWeightMillis"], 250);
        drop(tx);
    }

    #[test]
    fn share_samples_are_not_multiplied_by_listing_history() {
        let conn = database();
        insert_profile(&conn, "mp_samples", None);
        insert_profile(&conn, "mp_unprojected", None);
        insert_listing(
            &conn,
            "mp_samples",
            "listing-a-old",
            "share-a",
            "installation-samples",
            "closed",
        );
        insert_listing(
            &conn,
            "mp_samples",
            "listing-a-current",
            "share-a",
            "installation-samples",
            "active",
        );
        insert_listing(
            &conn,
            "mp_unprojected",
            "listing-unprojected",
            "share-unprojected",
            "installation-samples",
            "active",
        );
        insert_listing(
            &conn,
            "mp_samples",
            "listing-b",
            "share-b",
            "installation-samples",
            "active",
        );
        let now = Utc::now();
        let slot_start = now.timestamp().div_euclid(1_800) * 1_800;
        conn.execute(
            "INSERT INTO share_model_probe_observations (
                observation_id, installation_id, cycle_id, slot_start,
                capacity_pool_id, app_type, api_type, provider_id,
                health_fingerprint, requested_model, actual_model, status,
                outcome, latency_ms, checked_at, created_at
             ) VALUES (
                'observation-samples', 'installation-samples', 'cycle-samples', ?1,
                'pool-samples', 'claude', 'anthropic', 'upstream-samples',
                'fingerprint-samples', 'model-samples', 'model-samples', 'success',
                'success', 100, ?1, ?1
             )",
            params![slot_start],
        )
        .expect("insert one canonical Provider observation");
        insert_probe_projection(
            &conn,
            "share-a",
            "observation-samples",
            slot_start,
            "success",
        );
        for (request_id, share_id, latency_ms, first_token_ms, output_tokens) in [
            ("request-a", "share-a", 1_100, 100, 10),
            ("request-b", "share-b", 1_100, 100, 30),
            // A terminal-flush-sized window has TTFT evidence but is not a
            // reliable throughput sample.
            ("request-flush", "share-b", 1_000, 950, 1_000),
        ] {
            conn.execute(
                "INSERT INTO share_request_logs (
                    request_id, installation_id, share_id, share_name,
                    provider_id, provider_name, app_type, model, request_model,
                    status_code, latency_ms, first_token_ms, input_tokens,
                    output_tokens, cache_read_tokens, cache_creation_tokens,
                    is_streaming, stream_status, usage_state, is_health_check,
                    created_at
                 ) VALUES (
                    ?1, 'installation-samples', ?2, ?2,
                    'upstream-samples', 'Upstream', 'claude', 'model-samples',
                    'model-samples', 200, ?3, ?4, 1, ?5, 0, 0,
                    1, 'completed', 'observed', 0, ?6
                 )",
                params![
                    request_id,
                    share_id,
                    latency_ms,
                    first_token_ms,
                    output_tokens,
                    now.timestamp(),
                ],
            )
            .expect("insert Provider performance sample");
        }

        let tx = conn
            .transaction()
            .expect("start Provider sample transaction");
        let candidate = candidate_tx(&tx, "mp_samples", now).expect("calculate sample candidate");
        let unprojected = candidate_tx(&tx, "mp_unprojected", now)
            .expect("calculate unprojected Provider candidate");
        assert_eq!(candidate.share_probe_count, 1);
        assert_eq!(unprojected.share_probe_count, 0);
        assert_eq!(candidate.tps, Some(20.0));
        assert!(
            candidate
                .ttft_ms
                .is_some_and(|value| (value - (1_150.0 / 3.0)).abs() < 0.001)
        );
        drop(tx);
    }

    #[test]
    fn share_history_follows_the_listing_ownership_window() {
        let conn = database();
        insert_profile(&conn, "mp_old_owner", None);
        insert_profile(&conn, "mp_new_owner", None);
        insert_listing(
            &conn,
            "mp_new_owner",
            "listing-transfer",
            "share-transfer",
            "installation-transfer",
            "active",
        );
        let now = Utc::now();
        let transfer_at = now - Duration::days(2);
        let reopened_at = now - Duration::days(1);
        conn.execute_batch(
            "DELETE FROM market_provider_share_windows
             WHERE listing_id = 'listing-transfer';",
        )
        .expect("replace default Provider window");
        conn.execute(
            "INSERT INTO market_provider_share_windows (
                id, listing_id, market_provider_id, starts_at, ends_at, created_at
             ) VALUES
                ('window-before-transfer', 'listing-transfer', 'mp_old_owner', ?1, ?2, ?1),
                ('window-after-transfer', 'listing-transfer', 'mp_new_owner', ?3, NULL, ?3)",
            params![
                (now - Duration::days(10)).to_rfc3339(),
                transfer_at.to_rfc3339(),
                reopened_at.to_rfc3339(),
            ],
        )
        .expect("record Provider ownership windows on one stable listing");

        for (suffix, observed_at, outcome, first_token_ms) in [
            ("old", now - Duration::days(3), "failure", 100),
            ("closed-gap", now - Duration::hours(36), "success", 900),
            ("new", now - Duration::hours(12), "success", 300),
        ] {
            let timestamp = observed_at.timestamp();
            let slot_start = timestamp.div_euclid(1_800) * 1_800;
            let status = if outcome == "success" {
                "success"
            } else {
                "failed"
            };
            let failure_domain = (outcome == "failure").then_some("upstream");
            conn.execute(
                "INSERT INTO share_model_probe_observations (
                    observation_id, installation_id, cycle_id, slot_start,
                    capacity_pool_id, app_type, api_type, provider_id,
                    health_fingerprint, requested_model, actual_model, status,
                    outcome, failure_domain, latency_ms, checked_at, created_at
                 ) VALUES (?1, 'installation-transfer', ?2, ?3,
                           'pool-transfer', 'claude', 'anthropic', 'upstream-transfer',
                           'fingerprint-transfer', 'model-transfer', 'model-transfer',
                           ?4, ?5, ?6, 500, ?3, ?3)",
                params![
                    format!("observation-{suffix}"),
                    format!("cycle-{suffix}"),
                    slot_start,
                    status,
                    outcome,
                    failure_domain,
                ],
            )
            .expect("insert ownership-window probe");
            insert_probe_projection(
                &conn,
                "share-transfer",
                &format!("observation-{suffix}"),
                slot_start,
                outcome,
            );
            conn.execute(
                "INSERT INTO share_request_logs (
                    request_id, installation_id, share_id, share_name,
                    provider_id, provider_name, app_type, model, request_model,
                    status_code, latency_ms, first_token_ms, input_tokens,
                    output_tokens, cache_read_tokens, cache_creation_tokens,
                    is_streaming, stream_status, usage_state, is_health_check,
                    created_at
                 ) VALUES (?1, 'installation-transfer', 'share-transfer', 'Transfer',
                           'upstream-transfer', 'Upstream', 'claude', 'model-transfer',
                           'model-transfer', 200, 1300, ?2, 1, 10, 0, 0,
                           1, 'completed', 'observed', 0, ?3)",
                params![format!("request-{suffix}"), first_token_ms, timestamp],
            )
            .expect("insert ownership-window performance sample");
        }
        let activated_before_transfer = (now - Duration::days(3)).to_rfc3339();
        conn.execute(
            "INSERT INTO share_market_subscriptions (
                id, seat_id, listing_id, share_id, installation_id,
                entitlement_id, owner_user_id, owner_email, renter_user_id,
                renter_email, status, token_period_json, daily_rate_minor,
                offer_revision, activated_at, created_at, updated_at
             ) VALUES (
                'subscription-before-transfer', 'seat-listing-transfer',
                'listing-transfer', 'share-transfer', 'installation-transfer',
                'entitlement-before-transfer', 'old-owner', 'old-owner@example.com',
                'buyer-before-transfer', 'buyer-before-transfer@example.com',
                'active', '{}', 100, 1, ?1, ?1, ?1
             )",
            params![activated_before_transfer],
        )
        .expect("insert pre-transfer Share choice and fulfillment");

        let tx = conn
            .transaction()
            .expect("start ownership-window transaction");
        refresh_effective_selections_tx(&tx, now).expect("attribute transfer choices by window");
        let old = candidate_tx(&tx, "mp_old_owner", now).expect("calculate old owner");
        let new = candidate_tx(&tx, "mp_new_owner", now).expect("calculate new owner");
        let old_metrics: serde_json::Value =
            serde_json::from_str(&old.metrics_json).expect("decode old metrics");
        let new_metrics: serde_json::Value =
            serde_json::from_str(&new.metrics_json).expect("decode new metrics");
        assert_eq!(old_metrics["shareProbeTotal"], 1);
        assert_eq!(old_metrics["shareProbeSuccesses"], 0);
        assert_eq!(new_metrics["shareProbeTotal"], 1);
        assert_eq!(new_metrics["shareProbeSuccesses"], 1);
        assert_eq!(old_metrics["fulfillmentSuccesses"], 1);
        assert_eq!(old_metrics["fulfillmentTotal"], 1);
        assert_eq!(new_metrics["fulfillmentTotal"], 0);
        assert_eq!(old.signals.independent_buyers, 1);
        assert_eq!(new.signals.independent_buyers, 0);
        assert_eq!(old.ttft_ms, Some(100.0));
        assert_eq!(new.ttft_ms, Some(300.0));
        drop(tx);
    }

    #[test]
    fn public_share_supply_requires_a_live_share_and_visible_seat() {
        let conn = database();
        insert_profile(&conn, "mp_live_supply", None);
        insert_listing(
            &conn,
            "mp_live_supply",
            "listing-live-supply",
            "share-live-supply",
            "installation-live-supply",
            "active",
        );
        conn.execute(
            "UPDATE shares SET share_status = 'paused' WHERE share_id = 'share-live-supply'",
            [],
        )
        .expect("pause backing Share");
        assert_eq!(
            generate_conn(&conn, Utc::now()).expect("generate without live Share"),
            0
        );

        conn.execute_batch(
            "UPDATE shares SET share_status = 'active' WHERE share_id = 'share-live-supply';
             UPDATE share_market_seats SET retired_at = 'now'
             WHERE listing_id = 'listing-live-supply';",
        )
        .expect("retire the only Share seat");
        assert_eq!(
            generate_conn(&conn, Utc::now()).expect("generate without visible seat"),
            0
        );

        conn.execute(
            "UPDATE share_market_seats SET retired_at = NULL
             WHERE listing_id = 'listing-live-supply'",
            [],
        )
        .expect("restore visible Share seat");
        conn.execute(
            "UPDATE shares SET enabled_claude = 0 WHERE share_id = 'share-live-supply'",
            [],
        )
        .expect("disable the stale Share binding");
        let tx = conn
            .transaction()
            .expect("start disabled Share app transaction");
        let candidate =
            candidate_tx(&tx, "mp_live_supply", Utc::now()).expect("calculate Share app breadth");
        let metrics: serde_json::Value =
            serde_json::from_str(&candidate.metrics_json).expect("decode Share app metrics");
        assert_eq!(metrics["appCount"], 0);
        drop(tx);
        assert_eq!(
            generate_conn(&conn, Utc::now()).expect("generate live Share supply"),
            1
        );
    }

    #[test]
    fn disabled_client_hosts_are_not_public_supply() {
        let conn = database();
        insert_profile(&conn, "mp_disabled_host", None);
        conn.execute_batch(
            "INSERT INTO host_provider_profiles (
                provider_id, owner_email, created_at, updated_at, market_provider_id
             ) VALUES ('disabled-provider', 'disabled@example.com', 'now', 'now',
                       'mp_disabled_host');
             INSERT INTO router_ssh_hosts (
                id, ip, port, host_owner_email, country_code, status,
                created_at, updated_at, provider_id
             ) VALUES ('disabled-host', '203.0.113.30', 22, 'disabled@example.com',
                       'US', 'disabled', 'now', 'now', 'disabled-provider');",
        )
        .expect("seed disabled Client Host");
        assert_eq!(
            generate_conn(&conn, Utc::now()).expect("generate disabled Host supply"),
            0
        );

        conn.execute(
            "UPDATE router_ssh_hosts SET status = 'idle' WHERE id = 'disabled-host'",
            [],
        )
        .expect("enable Client Host");
        assert_eq!(
            generate_conn(&conn, Utc::now()).expect("generate enabled Host supply"),
            1
        );
    }

    #[test]
    fn host_observation_days_require_real_samples() {
        let conn = database();
        insert_profile(&conn, "mp_host_days", None);
        conn.execute(
            "INSERT INTO host_provider_profiles (
                provider_id, owner_email, created_at, updated_at, market_provider_id
             ) VALUES (
                'host-days', 'host-days@example.com', 'now', 'now', 'mp_host_days'
             )",
            [],
        )
        .expect("insert Host Provider for observation days");
        let now = Utc::now();
        for (stat_date, host_total, observed_samples, host_samples) in [
            (
                (now - Duration::days(1)).format("%Y-%m-%d").to_string(),
                0,
                0,
                0,
            ),
            (now.format("%Y-%m-%d").to_string(), 1, 0, 1),
        ] {
            conn.execute(
                "INSERT INTO host_provider_daily_stats (
                    provider_id, stat_date, host_total, idle_total, allocated_total,
                    external_client_total, online_samples, observed_samples,
                    anomalous_host_samples, host_samples, updated_at
                 ) VALUES ('host-days', ?1, ?2, 0, 0, 0, 0, ?3, 0, ?4, 'now')",
                params![stat_date, host_total, observed_samples, host_samples],
            )
            .expect("insert Host Provider daily sample");
        }

        let tx = conn
            .transaction()
            .expect("start Host observation-day transaction");
        let candidate =
            candidate_tx(&tx, "mp_host_days", now).expect("calculate Host observation days");
        assert_eq!(candidate.signals.observation_days, 1);
        drop(tx);
    }

    #[test]
    fn client_self_rental_is_not_an_independent_choice() {
        let conn = database();
        insert_profile(&conn, "mp_client", Some("provider-user"));
        conn.execute(
            "UPDATE market_provider_profiles
             SET canonical_email = 'provider-user@example.com'
             WHERE id = 'mp_client'",
            [],
        )
        .expect("align Provider canonical email");
        conn.execute(
            "INSERT INTO market_provider_aliases (
                market_provider_id, alias_kind, alias_value, created_at
             ) VALUES ('mp_client', 'host_provider_id', 'legacy-host-provider', 'now')",
            [],
        )
        .expect("insert Host Provider alias");
        conn.execute(
            "INSERT INTO host_provider_profiles (
                provider_id, owner_email, created_at, updated_at, market_provider_id
             ) VALUES (
                'legacy-host-provider', 'provider-user@example.com', 'now', 'now', 'mp_client'
             )",
            [],
        )
        .expect("insert self-rental Host Provider");
        conn.execute(
            "INSERT INTO router_ssh_hosts (
                id, ip, port, host_owner_email, status, created_at, updated_at, provider_id
             ) VALUES (
                'host-self', '203.0.113.10', 22, 'provider-user@example.com',
                'allocated', 'now', 'now', 'legacy-host-provider'
             );
             INSERT INTO provisioning_jobs (
                id, type, host_id, host_owner_email, client_owner_email,
                client_owner_user_id, status, created_at, updated_at
             ) VALUES (
                'job-self', 'create', 'host-self', 'provider-user@example.com',
                'provider-user@example.com', NULL, 'succeeded', 'now', 'now'
             );
             INSERT INTO router_ssh_hosts (
                id, ip, port, host_owner_email, status, created_at, updated_at, provider_id
             ) VALUES (
                'host-self-email', '203.0.113.11', 22, 'legacy-owner@example.com',
                'allocated', 'now', 'now', 'legacy-host-provider'
             );
             INSERT INTO provisioning_jobs (
                id, type, host_id, host_owner_email, client_owner_email,
                client_owner_user_id, status, created_at, updated_at
             ) VALUES (
                'job-self-email', 'create', 'host-self-email', 'legacy-owner@example.com',
                'legacy-owner@example.com', 'legacy-client-id', 'succeeded', 'now', 'now'
             );",
            [],
        )
        .expect("insert self-rental fulfillment attempt");
        let activated_at = (Utc::now() - Duration::hours(1)).to_rfc3339();
        conn.execute(
            "INSERT INTO client_market_subscriptions (
                installation_id, host_id, provider_id, host_owner_email,
                client_user_id, client_owner_email, status, daily_rate_minor,
                offer_revision, activated_at, created_at, updated_at
             ) VALUES ('client-self', 'host-self', 'legacy-host-provider',
                       'provider-user@example.com', 'legacy-provider-client',
                       'provider-user@example.com', 'active', 100, 1, ?1, 'now', 'now');
             INSERT INTO client_market_subscriptions (
                installation_id, host_id, provider_id, host_owner_email,
                client_user_id, client_owner_email, status, daily_rate_minor,
                offer_revision, activated_at, created_at, updated_at
             ) VALUES ('client-self-email', 'host-self-email', 'legacy-host-provider',
                       'legacy-owner@example.com', 'legacy-client-id',
                       'legacy-owner@example.com', 'active', 100, 1, ?1, 'now', 'now')",
            params![activated_at],
        )
        .expect("insert self-rental");

        let now = Utc::now();
        let tx = conn.transaction().expect("start self-rental transaction");
        refresh_effective_selections_tx(&tx, now).expect("refresh self-rental choices");
        let selection_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM market_provider_effective_selections",
                [],
                |row| row.get(0),
            )
            .expect("count self-rental choices");
        assert_eq!(selection_count, 0);
        let candidate = candidate_tx(&tx, "mp_client", now).expect("calculate self-rental rank");
        let metrics: serde_json::Value =
            serde_json::from_str(&candidate.metrics_json).expect("decode self-rental metrics");
        assert_eq!(metrics["fulfillmentSuccesses"], 0);
        assert_eq!(metrics["fulfillmentTotal"], 0);
        drop(tx);
    }

    #[test]
    fn client_fulfillment_stays_with_the_job_time_host_owner() {
        let conn = database();
        insert_profile(&conn, "mp_old_host_owner", None);
        insert_profile(&conn, "mp_new_host_owner", None);
        conn.execute_batch(
            "UPDATE market_provider_profiles
             SET canonical_email = 'old-owner@example.com'
             WHERE id = 'mp_old_host_owner';
             UPDATE market_provider_profiles
             SET canonical_email = 'new-owner@example.com'
             WHERE id = 'mp_new_host_owner';
             INSERT INTO host_provider_profiles (
                provider_id, owner_email, created_at, updated_at, market_provider_id
             ) VALUES ('current-host-provider', 'new-owner@example.com', 'now', 'now',
                       'mp_new_host_owner');
             INSERT INTO router_ssh_hosts (
                id, ip, port, host_owner_email, status, created_at, updated_at, provider_id
             ) VALUES ('transferred-host', '203.0.113.20', 22,
                       'new-owner@example.com', 'allocated', 'now', 'now',
                       'current-host-provider');
             INSERT INTO provisioning_jobs (
                id, type, host_id, host_owner_email, client_owner_email,
                client_owner_user_id, status, created_at, updated_at
             ) VALUES ('job-before-transfer', 'create', 'transferred-host',
                       'old-owner@example.com', 'buyer@example.com', 'buyer',
                       'succeeded', 'now', 'now');",
        )
        .expect("seed transferred Client Host history");

        let now = Utc::now();
        let tx = conn
            .transaction()
            .expect("start transferred Host transaction");
        let old = candidate_tx(&tx, "mp_old_host_owner", now).expect("calculate old Host owner");
        let new = candidate_tx(&tx, "mp_new_host_owner", now).expect("calculate new Host owner");
        let old_metrics: serde_json::Value =
            serde_json::from_str(&old.metrics_json).expect("decode old Host metrics");
        let new_metrics: serde_json::Value =
            serde_json::from_str(&new.metrics_json).expect("decode new Host metrics");
        assert_eq!(old_metrics["fulfillmentSuccesses"], 1);
        assert_eq!(old_metrics["fulfillmentTotal"], 1);
        assert_eq!(new_metrics["fulfillmentSuccesses"], 0);
        assert_eq!(new_metrics["fulfillmentTotal"], 0);
        drop(tx);
    }

    #[test]
    fn generation_publishes_a_complete_snapshot_and_retains_last_good() {
        let conn = database();
        insert_profile(&conn, "mp_generation", None);
        insert_listing(
            &conn,
            "mp_generation",
            "listing-generation",
            "share-generation",
            "installation-generation",
            "active",
        );
        let now = Utc::now();
        let expired_at = (now - Duration::days(GENERATION_RETENTION_DAYS + 1)).to_rfc3339();
        conn.execute(
            "INSERT INTO market_provider_rank_generations (
                id, algorithm_version, state, observed_from, observed_to,
                provider_count, failure_summary, created_at
             ) VALUES ('expired-failed', ?1, 'failed', ?2, ?2, 0,
                       'generation_failed', ?2)",
            params![ALGORITHM_VERSION, expired_at],
        )
        .expect("insert expired diagnostic generation");
        assert_eq!(generate_conn(&conn, now).expect("publish generation"), 1);
        let expired_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_provider_rank_generations
                 WHERE id = 'expired-failed'",
                [],
                |row| row.get(0),
            )
            .expect("count expired generation");
        assert_eq!(expired_count, 0);

        let published_id: String = conn
            .query_row(
                "SELECT id FROM market_provider_rank_generations
                 WHERE state = 'published'",
                [],
                |row| row.get(0),
            )
            .expect("read published generation");
        let projections = latest_rank_projections(&conn).expect("read published projection");
        assert_eq!(
            projections.get("mp_generation"),
            Some(&RankProjection {
                rank_state: "collecting".into(),
                rank_position: None,
                score_bps: None,
            })
        );
        conn.execute(
            "UPDATE market_provider_profiles
             SET status = 'identity_conflict' WHERE id = 'mp_generation'",
            [],
        )
        .expect("fence published Provider identity");
        assert!(
            !latest_rank_projections(&conn)
                .expect("filter fenced published projection")
                .contains_key("mp_generation")
        );
        conn.execute(
            "UPDATE market_provider_profiles SET status = 'active' WHERE id = 'mp_generation'",
            [],
        )
        .expect("restore Provider identity for last-good test");

        for (id, state, offset) in [
            ("newer-building", "building", 1),
            ("newer-failed", "failed", 2),
        ] {
            let created_at = (now + Duration::seconds(offset)).to_rfc3339();
            conn.execute(
                "INSERT INTO market_provider_rank_generations (
                    id, algorithm_version, state, observed_from, observed_to,
                    provider_count, failure_summary, created_at, published_at
                 ) VALUES (?1, ?2, ?3, ?4, ?4, 0, ?5, ?4, NULL)",
                params![
                    id,
                    ALGORITHM_VERSION,
                    state,
                    created_at,
                    (state == "failed").then_some("generation_failed"),
                ],
            )
            .expect("insert non-published generation");
        }

        let still_published: String = conn
            .query_row(
                "SELECT id FROM market_provider_rank_generations
                 WHERE state = 'published'
                 ORDER BY published_at DESC, created_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("read retained generation");
        assert_eq!(still_published, published_id);
        assert_eq!(
            latest_rank_projections(&conn)
                .expect("read retained projection")
                .get("mp_generation")
                .map(|projection| projection.rank_state.as_str()),
            Some("collecting")
        );
    }

    #[test]
    fn generation_fails_closed_on_overlapping_share_windows() {
        let conn = database();
        insert_profile(&conn, "mp_window_drift", None);
        insert_listing(
            &conn,
            "mp_window_drift",
            "listing-window-drift",
            "share-window-drift",
            "installation-window-drift",
            "active",
        );
        conn.execute(
            "INSERT INTO market_provider_share_windows (
                id, listing_id, market_provider_id, starts_at, ends_at, created_at
             ) VALUES (
                'overlapping-window', 'listing-window-drift', 'mp_window_drift',
                '2025-01-01T00:00:00Z', '2030-01-01T00:00:00Z',
                '2025-01-01T00:00:00Z'
             )",
            [],
        )
        .expect("insert corrupt overlapping Provider window");

        assert!(generate_conn(&conn, Utc::now()).is_err());
        let states: (i64, i64) = conn
            .query_row(
                "SELECT
                    SUM(CASE WHEN state = 'failed' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN state = 'published' THEN 1 ELSE 0 END)
                 FROM market_provider_rank_generations",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read failed closed generation state");
        assert_eq!(states, (1, 0));
    }

    #[test]
    fn generation_excludes_private_funding_only_identities() {
        let conn = database();
        insert_profile(&conn, "mp_funding_only", None);
        insert_profile(&conn, "mp_public_supply", None);
        insert_listing(
            &conn,
            "mp_public_supply",
            "listing-public-supply",
            "share-public-supply",
            "installation-public-supply",
            "active",
        );

        assert_eq!(
            generate_conn(&conn, Utc::now()).expect("publish public supply generation"),
            1
        );
        let projections = latest_rank_projections(&conn).expect("read public supply projection");
        assert!(projections.contains_key("mp_public_supply"));
        assert!(!projections.contains_key("mp_funding_only"));
    }
}
