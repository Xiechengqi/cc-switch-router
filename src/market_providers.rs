use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::{get, put};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ServerState;
use crate::db::{OptionalExtension, params};
use crate::error::AppError;
use crate::market_provider_rank as rank;

const PUBLIC_CACHE_CONTROL: &str = "public, max-age=60, stale-while-revalidate=300";
const GENERATION_STALE_AFTER_SECS: i64 =
    2 * crate::market_provider_rank::REFRESH_INTERVAL_SECS as i64;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketProviderRankGenerationView {
    pub id: String,
    pub algorithm_version: String,
    pub published_at: String,
    pub stale: bool,
    pub weights: MarketProviderRankWeightsView,
    pub minimum_independent_buyers: i64,
    pub minimum_observation_days: i64,
    pub windows: MarketProviderRankWindowsView,
    pub refresh_interval_secs: u64,
}

/// How far back each signal looks, so the method view can say it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketProviderRankWindowsView {
    pub share_quality_days: i64,
    pub host_quality_days: i64,
    pub effective_choice_days: i64,
    pub fulfillment_days: i64,
    pub performance_days: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketProviderRankWeightsView {
    pub service_quality: u8,
    pub effective_choice: u8,
    pub fulfillment: u8,
    pub supply_breadth: u8,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketProviderRankComponentsView {
    pub service_quality_bps: i64,
    pub effective_choice_bps: i64,
    pub fulfillment_bps: i64,
    pub supply_breadth_bps: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketProviderInventoryView {
    pub active_share_count: i64,
    pub available_share_seats: i64,
    pub host_total: i64,
    pub idle_host_total: i64,
    pub country_count: i64,
    pub app_count: i64,
    pub has_share_market: bool,
    pub has_client_market: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketProviderQualityView {
    pub share_probe_successes: i64,
    pub share_probe_total: i64,
    pub host_online_samples: i64,
    pub host_observed_samples: i64,
    pub fulfillment_successes: i64,
    pub fulfillment_total: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketProviderPerformanceView {
    pub average_ttft_ms: Option<f64>,
    pub average_tps: Option<f64>,
    pub sample_count: i64,
    pub display_only: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketProviderView {
    pub id: String,
    pub display_name: String,
    pub official: bool,
    pub status: String,
    pub rank_state: String,
    pub rank_position: Option<i64>,
    /// Places gained (positive) or lost since the generation a day earlier
    /// under the same algorithm; absent when either generation left the
    /// Provider unranked, or no comparable generation exists yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank_change: Option<i64>,
    pub score_bps: Option<i64>,
    pub components: MarketProviderRankComponentsView,
    pub independent_buyer_count: i64,
    pub observation_days: i64,
    pub inventory: MarketProviderInventoryView,
    pub quality: MarketProviderQualityView,
    pub performance: MarketProviderPerformanceView,
    pub exploration: bool,
    pub joined_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketProviderListView<'a> {
    pub generation: MarketProviderRankGenerationView,
    /// Borrowed from the shared list cache; serialized without copying it.
    pub providers: &'a [MarketProviderView],
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketProviderShareSupplyView {
    pub listing_id: String,
    /// Public catalog key, so a row can open the same Share in Share Market.
    pub share_id: String,
    pub share_name: String,
    pub apps: Vec<String>,
    pub available_seats: i64,
    pub total_seats: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketProviderClientSupplyView {
    pub country_code: String,
    pub idle_hosts: i64,
    pub total_hosts: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketProviderDetailView {
    #[serde(flatten)]
    pub provider: MarketProviderView,
    pub share_supply: Vec<MarketProviderShareSupplyView>,
    pub client_supply: Vec<MarketProviderClientSupplyView>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredMetrics {
    #[serde(default)]
    share_probe_successes: i64,
    #[serde(default)]
    share_probe_total: i64,
    #[serde(default)]
    host_online_samples: i64,
    #[serde(default)]
    host_observed_samples: i64,
    #[serde(default)]
    fulfillment_successes: i64,
    #[serde(default)]
    fulfillment_total: i64,
    #[serde(default)]
    active_share_count: i64,
    #[serde(default)]
    available_share_seats: i64,
    #[serde(default)]
    host_total: i64,
    #[serde(default)]
    idle_host_total: i64,
    #[serde(default)]
    country_count: i64,
    #[serde(default)]
    app_count: i64,
    #[serde(default)]
    performance_samples: i64,
}

#[derive(Debug, Clone)]
struct InternalProviderView {
    public: MarketProviderView,
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/market-providers", get(list_market_providers))
        .route("/v1/market-providers/:id", get(get_market_provider))
        .route("/v1/account/market-provider", get(get_my_market_provider))
        .route(
            "/v1/account/market-provider/display-name",
            put(update_my_display_name),
        )
}

fn map_db(context: &'static str) -> impl FnOnce(crate::db::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn latest_generation_view(
    conn: &crate::db::Connection,
    now: DateTime<Utc>,
) -> Result<MarketProviderRankGenerationView, AppError> {
    let row = conn
        .query_row(
            "SELECT id, algorithm_version, published_at
             FROM market_provider_rank_generations
             WHERE state = 'published'
             ORDER BY published_at DESC, created_at DESC LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("read public Market Provider generation"))?
        .ok_or_else(|| AppError::Coded {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "provider_rank_not_ready",
            message: "Market Provider rank is not ready".into(),
            details: serde_json::Value::Null,
        })?;
    let stale = parse_timestamp(&row.2)
        .is_none_or(|published| now - published > Duration::seconds(GENERATION_STALE_AFTER_SECS));
    Ok(MarketProviderRankGenerationView {
        id: row.0,
        algorithm_version: row.1,
        published_at: row.2,
        stale,
        weights: MarketProviderRankWeightsView {
            service_quality: rank::RANK_WEIGHTS.service_quality,
            effective_choice: rank::RANK_WEIGHTS.effective_choice,
            fulfillment: rank::RANK_WEIGHTS.fulfillment,
            supply_breadth: rank::RANK_WEIGHTS.supply_breadth,
        },
        minimum_independent_buyers: rank::MIN_INDEPENDENT_BUYERS,
        minimum_observation_days: rank::MIN_OBSERVATION_DAYS,
        windows: MarketProviderRankWindowsView {
            share_quality_days: rank::SHARE_QUALITY_WINDOW_DAYS,
            host_quality_days: rank::HOST_QUALITY_WINDOW_DAYS,
            effective_choice_days: rank::SELECTION_WINDOW_DAYS,
            fulfillment_days: rank::FULFILLMENT_WINDOW_DAYS,
            performance_days: rank::PERFORMANCE_WINDOW_DAYS,
        },
        refresh_interval_secs: rank::REFRESH_INTERVAL_SECS,
    })
}

fn provider_not_found() -> AppError {
    AppError::Coded {
        status: StatusCode::NOT_FOUND,
        code: "provider_not_found",
        message: "Market Provider not found".into(),
        details: serde_json::Value::Null,
    }
}

fn public_provider_display_name(profile_id: &str, stored: &str) -> String {
    let stored = stored.trim();
    if !stored.is_empty() && stored.chars().count() <= 80 && !stored.contains('@') {
        return stored.to_string();
    }
    crate::market_provider_identity::default_display_name(profile_id)
}

fn internal_provider_views(
    conn: &crate::db::Connection,
    generation_id: &str,
    official_email: Option<&str>,
) -> Result<Vec<InternalProviderView>, AppError> {
    let official_email = official_email.map(|value| value.trim().to_ascii_lowercase());
    let rows = conn
        .prepare(
            "SELECT profile.id, profile.user_id, profile.canonical_email,
                    profile.display_name, profile.status, profile.created_at,
                    entry.rank_state, entry.rank_position, entry.score_bps,
                    entry.service_quality_bps, entry.effective_choice_bps,
                    entry.fulfillment_bps, entry.supply_breadth_bps,
                    entry.independent_buyer_count, entry.observation_days,
                    entry.ttft_ms, entry.tps, entry.metrics_json,
                    previous.rank_position
             FROM market_provider_rank_entries entry
             JOIN market_provider_profiles profile ON profile.id = entry.market_provider_id
             LEFT JOIN market_provider_rank_entries previous
               ON previous.market_provider_id = entry.market_provider_id
              AND previous.rank_state = 'ranked'
              AND previous.generation_id = (
                  SELECT earlier.id FROM market_provider_rank_generations earlier
                  JOIN market_provider_rank_generations current ON current.id = ?1
                  WHERE earlier.state = 'published'
                    AND earlier.algorithm_version = current.algorithm_version
                    AND julianday(earlier.published_at)
                        <= julianday(current.published_at) - 1.0
                  ORDER BY earlier.published_at DESC LIMIT 1
              )
             WHERE entry.generation_id = ?1
               AND profile.status IN ('active', 'paused')
             ORDER BY CASE WHEN entry.rank_state = 'ranked' THEN 0 ELSE 1 END,
                      entry.rank_position, profile.id",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![generation_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Option<i64>>(7)?,
                        row.get::<_, Option<i64>>(8)?,
                        row.get::<_, i64>(9)?,
                        row.get::<_, i64>(10)?,
                        row.get::<_, i64>(11)?,
                        row.get::<_, i64>(12)?,
                        row.get::<_, i64>(13)?,
                        row.get::<_, i64>(14)?,
                        row.get::<_, Option<f64>>(15)?,
                        row.get::<_, Option<f64>>(16)?,
                        row.get::<_, String>(17)?,
                        row.get::<_, Option<i64>>(18)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("read public Market Provider rank"))?;
    let mut providers = Vec::with_capacity(rows.len());
    for row in rows {
        let metrics = serde_json::from_str::<StoredMetrics>(&row.17).map_err(|error| {
            AppError::Internal(format!(
                "decode published Market Provider metrics failed: {error}"
            ))
        })?;
        let official = official_email.as_deref() == Some(row.2.as_str());
        let public_display_name = public_provider_display_name(&row.0, &row.3);
        providers.push(InternalProviderView {
            public: MarketProviderView {
                id: row.0,
                display_name: if official {
                    "Official Provider".into()
                } else {
                    public_display_name
                },
                official,
                status: row.4,
                rank_change: match (row.18, row.7) {
                    (Some(previous), Some(current)) if row.6 == "ranked" => {
                        Some(previous - current)
                    }
                    _ => None,
                },
                rank_state: row.6,
                rank_position: row.7,
                score_bps: row.8,
                components: MarketProviderRankComponentsView {
                    service_quality_bps: row.9,
                    effective_choice_bps: row.10,
                    fulfillment_bps: row.11,
                    supply_breadth_bps: row.12,
                },
                independent_buyer_count: row.13,
                observation_days: row.14,
                inventory: MarketProviderInventoryView {
                    active_share_count: metrics.active_share_count,
                    available_share_seats: metrics.available_share_seats,
                    host_total: metrics.host_total,
                    idle_host_total: metrics.idle_host_total,
                    country_count: metrics.country_count,
                    app_count: metrics.app_count,
                    has_share_market: metrics.active_share_count > 0,
                    has_client_market: metrics.host_total > 0,
                },
                quality: MarketProviderQualityView {
                    share_probe_successes: metrics.share_probe_successes,
                    share_probe_total: metrics.share_probe_total,
                    host_online_samples: metrics.host_online_samples,
                    host_observed_samples: metrics.host_observed_samples,
                    fulfillment_successes: metrics.fulfillment_successes,
                    fulfillment_total: metrics.fulfillment_total,
                },
                performance: MarketProviderPerformanceView {
                    average_ttft_ms: row.15,
                    average_tps: row.16,
                    sample_count: metrics.performance_samples,
                    display_only: true,
                },
                exploration: false,
                joined_at: row.5,
            },
        });
    }
    Ok(providers)
}

fn exploration_key(generation_id: &str, provider_id: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(generation_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(provider_id.as_bytes());
    hasher.finalize().into()
}

/// The name Share Market cards show (the subdomain). A Share's own name is
/// free text that is often its owner's email, so it never leaves here; a
/// Share without a subdomain gets a stable anonymous label instead.
fn public_share_name(listing_id: &str, subdomain: &str) -> String {
    let subdomain = subdomain.trim();
    if !subdomain.is_empty() && !subdomain.contains('@') {
        return subdomain.to_string();
    }
    let digest = Sha256::digest(listing_id.as_bytes());
    format!("Share {}", &hex::encode(digest)[..8].to_ascii_uppercase())
}

/// Formal rank first, with a few collecting Providers given exploration slots
/// so new supply can earn its first buyers. Paused Providers keep their rank
/// number but move to the end: they are not taking new buyers right now.
fn recommended_order(
    generation_id: &str,
    providers: Vec<InternalProviderView>,
) -> Vec<InternalProviderView> {
    let (open, paused): (Vec<_>, Vec<_>) = providers
        .into_iter()
        .partition(|provider| provider.public.status != "paused");
    let (mut ranked, mut collecting): (Vec<_>, Vec<_>) = open
        .into_iter()
        .partition(|provider| provider.public.rank_state == "ranked");
    collecting.sort_by_key(|provider| exploration_key(generation_id, &provider.public.id));
    let exploration_count = collecting.len().min(ranked.len().div_ceil(5).max(1));
    for (index, mut provider) in collecting.drain(..exploration_count).enumerate() {
        provider.public.exploration = true;
        let insertion = (3 + index * 6).min(ranked.len());
        ranked.insert(insertion, provider);
    }
    ranked.extend(collecting);
    ranked.extend(paused);
    ranked
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderListKey {
    generation_id: String,
    official_email: Option<String>,
    profile_count: i64,
    profiles_updated_at: String,
}

/// The ordered public list only changes with a new generation, the official
/// operator, or a profile edit (name or status). Anonymous traffic reuses it
/// instead of re-reading every rank entry under the store lock.
static PROVIDER_LIST_CACHE: Mutex<Option<(ProviderListKey, Arc<Vec<MarketProviderView>>)>> =
    Mutex::new(None);

fn published_providers(
    conn: &crate::db::Connection,
    generation_id: &str,
    official_email: Option<&str>,
) -> Result<Arc<Vec<MarketProviderView>>, AppError> {
    let (profile_count, profiles_updated_at): (i64, String) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(MAX(updated_at), '') FROM market_provider_profiles",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_db("read Market Provider profile revision"))?;
    let key = ProviderListKey {
        generation_id: generation_id.to_string(),
        official_email: official_email.map(|value| value.trim().to_ascii_lowercase()),
        profile_count,
        profiles_updated_at,
    };
    if let Some((cached_key, providers)) = PROVIDER_LIST_CACHE
        .lock()
        .ok()
        .and_then(|cache| cache.clone())
        && cached_key == key
    {
        return Ok(providers);
    }
    let providers = Arc::new(
        recommended_order(
            generation_id,
            internal_provider_views(conn, generation_id, official_email)?,
        )
        .into_iter()
        .map(|provider| provider.public)
        .collect::<Vec<_>>(),
    );
    if let Ok(mut cache) = PROVIDER_LIST_CACHE.lock() {
        *cache = Some((key, providers.clone()));
    }
    Ok(providers)
}

fn if_none_match_matches(raw: &str, etag: &str) -> bool {
    raw.split(',').any(|candidate| {
        let candidate = candidate.trim();
        candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag
    })
}

fn public_json<T: Serialize>(headers: &HeaderMap, value: &T) -> Result<Response, AppError> {
    let body = serde_json::to_vec(value).map_err(|error| {
        AppError::Internal(format!("encode public Provider response failed: {error}"))
    })?;
    let digest = Sha256::digest(&body);
    let etag = format!("\"{}\"", hex::encode(digest));
    if headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| if_none_match_matches(value, &etag))
    {
        let mut response = Response::new(axum::body::Body::empty());
        *response.status_mut() = StatusCode::NOT_MODIFIED;
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static(PUBLIC_CACHE_CONTROL),
        );
        response.headers_mut().insert(
            header::ETAG,
            HeaderValue::from_str(&etag)
                .map_err(|_| AppError::Internal("invalid Provider ETag".into()))?,
        );
        return Ok(response);
    }
    let mut response = Response::new(axum::body::Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(PUBLIC_CACHE_CONTROL),
    );
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag)
            .map_err(|_| AppError::Internal("invalid Provider ETag".into()))?,
    );
    Ok(response)
}

async fn list_market_providers(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let conn = state.store.conn.lock().await;
    let generation = latest_generation_view(&conn, Utc::now())?;
    let providers = published_providers(
        &conn,
        &generation.id,
        state.config.official_provider_email(),
    )?;
    drop(conn);
    public_json(
        &headers,
        &MarketProviderListView {
            generation,
            providers: &providers,
        },
    )
}

fn share_supply(
    conn: &crate::db::Connection,
    profile_id: &str,
) -> Result<Vec<MarketProviderShareSupplyView>, AppError> {
    conn.prepare(
        "SELECT listing.id, share.share_id, COALESCE(share.subdomain, ''),
                EXISTS(SELECT 1 FROM share_bindings binding
                       WHERE binding.share_id = listing.share_id
                         AND binding.app_type = 'claude'
                         AND COALESCE(share.enabled_claude, 0) != 0),
                EXISTS(SELECT 1 FROM share_bindings binding
                       WHERE binding.share_id = listing.share_id
                         AND binding.app_type = 'codex'
                         AND COALESCE(share.enabled_codex, 0) != 0),
                EXISTS(SELECT 1 FROM share_bindings binding
                       WHERE binding.share_id = listing.share_id
                         AND binding.app_type = 'gemini'
                         AND COALESCE(share.enabled_gemini, 0) != 0),
                COUNT(DISTINCT CASE WHEN seat.status = 'available'
                                         AND seat.retired_at IS NULL
                                    THEN seat.id END),
                COUNT(DISTINCT CASE
                    WHEN seat.status IN ('available', 'reserved', 'occupied', 'revoking')
                         AND seat.retired_at IS NULL THEN seat.id END)
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
           )
         GROUP BY listing.id, share.share_id, share.subdomain,
                  share.enabled_claude, share.enabled_codex, share.enabled_gemini
         ORDER BY listing.created_at, listing.id",
    )
    .and_then(|mut statement| {
        statement
            .query_map(params![profile_id], |row| {
                let listing_id = row.get::<_, String>(0)?;
                let subdomain = row.get::<_, String>(2)?;
                let mut apps = Vec::new();
                if row.get::<_, i64>(3)? != 0 {
                    apps.push("claude".into());
                }
                if row.get::<_, i64>(4)? != 0 {
                    apps.push("codex".into());
                }
                if row.get::<_, i64>(5)? != 0 {
                    apps.push("gemini".into());
                }
                Ok(MarketProviderShareSupplyView {
                    share_name: public_share_name(&listing_id, &subdomain),
                    share_id: row.get(1)?,
                    listing_id,
                    apps,
                    available_seats: row.get(6)?,
                    total_seats: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
    })
    .map_err(map_db("read Market Provider Share supply"))
}

fn client_supply(
    conn: &crate::db::Connection,
    profile_id: &str,
) -> Result<Vec<MarketProviderClientSupplyView>, AppError> {
    conn.prepare(
        "SELECT COALESCE(NULLIF(upper(trim(host.country_code)), ''), '—'),
                SUM(CASE WHEN host.status = 'idle' THEN 1 ELSE 0 END), COUNT(*)
         FROM host_provider_profiles provider
         JOIN router_ssh_hosts host ON host.provider_id = provider.provider_id
         WHERE provider.market_provider_id = ?1 AND host.status != 'disabled'
         GROUP BY COALESCE(NULLIF(upper(trim(host.country_code)), ''), '—')
         ORDER BY COALESCE(NULLIF(upper(trim(host.country_code)), ''), '—')",
    )
    .and_then(|mut statement| {
        statement
            .query_map(params![profile_id], |row| {
                Ok(MarketProviderClientSupplyView {
                    country_code: row.get(0)?,
                    idle_hosts: row.get(1)?,
                    total_hosts: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
    })
    .map_err(map_db("read Market Provider Client supply"))
}

async fn get_market_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let conn = state.store.conn.lock().await;
    let generation = latest_generation_view(&conn, Utc::now())?;
    let mut provider = published_providers(
        &conn,
        &generation.id,
        state.config.official_provider_email(),
    )?
    .iter()
    .find(|provider| provider.id == id)
    .cloned()
    .ok_or_else(provider_not_found)?;
    let share_supply = share_supply(&conn, &id)?;
    let client_supply = client_supply(&conn, &id)?;
    drop(conn);
    apply_live_inventory(&mut provider.inventory, &share_supply, &client_supply);
    let detail = MarketProviderDetailView {
        share_supply,
        client_supply,
        provider,
    };
    public_json(&headers, &detail)
}

/// The detail lists live supply, so its counts replace the generation's
/// snapshot: a drawer must not say "3 available" above an empty list.
fn apply_live_inventory(
    inventory: &mut MarketProviderInventoryView,
    share_supply: &[MarketProviderShareSupplyView],
    client_supply: &[MarketProviderClientSupplyView],
) {
    inventory.active_share_count = share_supply.len() as i64;
    inventory.available_share_seats = share_supply.iter().map(|row| row.available_seats).sum();
    inventory.app_count = share_supply
        .iter()
        .flat_map(|row| row.apps.iter())
        .collect::<std::collections::BTreeSet<_>>()
        .len() as i64;
    inventory.host_total = client_supply.iter().map(|row| row.total_hosts).sum();
    inventory.idle_host_total = client_supply.iter().map(|row| row.idle_hosts).sum();
    inventory.country_count = client_supply
        .iter()
        .filter(|row| row.country_code != "—")
        .count() as i64;
    inventory.has_share_market = inventory.active_share_count > 0;
    inventory.has_client_market = inventory.host_total > 0;
}

const DISPLAY_NAME_MIN_CHARS: usize = 2;
const DISPLAY_NAME_MAX_CHARS: usize = 32;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MyMarketProviderProfileView {
    id: String,
    display_name: String,
    default_display_name: String,
    status: String,
    official: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MyMarketProviderView {
    profile: Option<MyMarketProviderProfileView>,
    /// The Provider's row in the latest generation; absent until it has live
    /// supply to rank.
    standing: Option<MarketProviderView>,
    generation: Option<MarketProviderRankGenerationView>,
    display_name_min_chars: usize,
    display_name_max_chars: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateDisplayNameRequest {
    display_name: String,
}

fn display_name_error(code: &'static str, message: &str) -> AppError {
    AppError::Coded {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        code,
        message: message.into(),
        details: serde_json::json!({
            "minChars": DISPLAY_NAME_MIN_CHARS,
            "maxChars": DISPLAY_NAME_MAX_CHARS,
        }),
    }
}

/// A public name must identify a Provider without impersonating the operator,
/// another Provider's generated name, or leaking contact details. An empty
/// name restores the generated default.
fn normalize_display_name(raw: &str, own_default: &str) -> Result<String, AppError> {
    let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Ok(own_default.to_string());
    }
    let length = name.chars().count();
    if !(DISPLAY_NAME_MIN_CHARS..=DISPLAY_NAME_MAX_CHARS).contains(&length) {
        return Err(display_name_error(
            "provider_name_length",
            "display name length is out of range",
        ));
    }
    let lower = name.to_lowercase();
    if name.chars().any(char::is_control)
        || name.contains('@')
        || lower.contains("://")
        || lower.starts_with("www.")
    {
        return Err(display_name_error(
            "provider_name_invalid",
            "display name must not contain contact details or links",
        ));
    }
    let generated = lower.strip_prefix("provider ").is_some_and(|suffix| {
        (6..=8).contains(&suffix.len()) && suffix.chars().all(|c| c.is_ascii_alphanumeric())
    });
    if lower.contains("official")
        || name.contains("官方")
        || (generated && !name.eq_ignore_ascii_case(own_default))
    {
        return Err(display_name_error(
            "provider_name_reserved",
            "display name is reserved",
        ));
    }
    Ok(name)
}

fn my_profile(
    conn: &crate::db::Connection,
    user_id: &str,
    official_email: Option<&str>,
) -> Result<Option<MyMarketProviderProfileView>, AppError> {
    let official_email = official_email.map(|value| value.trim().to_ascii_lowercase());
    conn.query_row(
        "SELECT id, display_name, status, canonical_email
         FROM market_provider_profiles
         WHERE user_id = ?1 AND claim_state = 'claimed'
           AND status IN ('active', 'paused')",
        params![user_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    )
    .optional()
    .map_err(map_db("read own Market Provider profile"))
    .map(|row| {
        row.map(|(id, stored, status, email)| MyMarketProviderProfileView {
            display_name: public_provider_display_name(&id, &stored),
            default_display_name: crate::market_provider_identity::default_display_name(&id),
            official: official_email.as_deref() == Some(email.to_ascii_lowercase().as_str()),
            id,
            status,
        })
    })
}

async fn require_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<crate::models::AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))
}

fn my_market_provider_view(
    conn: &crate::db::Connection,
    user_id: &str,
    official_email: Option<&str>,
) -> Result<MyMarketProviderView, AppError> {
    let profile = my_profile(conn, user_id, official_email)?;
    let generation = match latest_generation_view(conn, Utc::now()) {
        Ok(generation) => Some(generation),
        Err(AppError::Coded {
            code: "provider_rank_not_ready",
            ..
        }) => None,
        Err(error) => return Err(error),
    };
    let standing = match (&profile, &generation) {
        (Some(profile), Some(generation)) => {
            published_providers(conn, &generation.id, official_email)?
                .iter()
                .find(|provider| provider.id == profile.id)
                .cloned()
        }
        _ => None,
    };
    Ok(MyMarketProviderView {
        profile,
        standing,
        generation,
        display_name_min_chars: DISPLAY_NAME_MIN_CHARS,
        display_name_max_chars: DISPLAY_NAME_MAX_CHARS,
    })
}

async fn get_my_market_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let view = my_market_provider_view(
        &conn,
        &session.user_id,
        state.config.official_provider_email(),
    )?;
    Ok(([(header::CACHE_CONTROL, "private, no-store")], Json(view)))
}

fn update_display_name_conn(
    conn: &crate::db::Connection,
    user_id: &str,
    raw: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let (profile_id, previous): (String, String) = conn
        .query_row(
            "SELECT id, display_name FROM market_provider_profiles
             WHERE user_id = ?1 AND claim_state = 'claimed'
               AND status IN ('active', 'paused')",
            params![user_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(map_db("read own Market Provider profile"))?
        .ok_or_else(|| AppError::Coded {
            status: StatusCode::NOT_FOUND,
            code: "provider_profile_missing",
            message: "no claimed Market Provider profile".into(),
            details: serde_json::Value::Null,
        })?;
    let own_default = crate::market_provider_identity::default_display_name(&profile_id);
    let name = normalize_display_name(raw, &own_default)?;
    if name == previous {
        return Ok(());
    }
    let taken: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM market_provider_profiles
                           WHERE id != ?1 AND lower(display_name) = lower(?2))",
            params![profile_id, name],
            |row| row.get(0),
        )
        .map_err(map_db("check Market Provider display name"))?;
    if taken {
        return Err(display_name_error(
            "provider_name_taken",
            "another Provider already uses this display name",
        ));
    }
    let timestamp = now.to_rfc3339();
    conn.execute(
        "UPDATE market_provider_profiles SET display_name = ?2, updated_at = ?3 WHERE id = ?1",
        params![profile_id, name, timestamp],
    )
    .map_err(map_db("update Market Provider display name"))?;
    conn.execute(
        "INSERT INTO market_provider_identity_events (
            id, market_provider_id, event_kind, detail_json, created_at
         ) VALUES (?1, ?2, 'display_name_changed', ?3, ?4)",
        params![
            uuid::Uuid::new_v4().to_string(),
            profile_id,
            serde_json::json!({ "from": previous, "to": name }).to_string(),
            timestamp,
        ],
    )
    .map_err(map_db("audit Market Provider display name"))?;
    Ok(())
}

async fn update_my_display_name(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<UpdateDisplayNameRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    update_display_name_conn(&conn, &session.user_id, &input.display_name, Utc::now())?;
    let view = my_market_provider_view(
        &conn,
        &session.user_id,
        state.config.official_provider_email(),
    )?;
    Ok(([(header::CACHE_CONTROL, "private, no-store")], Json(view)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> crate::db::Connection {
        let conn = crate::test_support::database().expect("copy Provider test database");
        conn.pragma_update(None, "foreign_keys", "ON")
            .expect("enable foreign keys");
        conn
    }

    fn provider(id: &str, rank_state: &str) -> InternalProviderView {
        InternalProviderView {
            public: MarketProviderView {
                id: id.into(),
                display_name: id.into(),
                official: false,
                status: "active".into(),
                rank_state: rank_state.into(),
                rank_position: (rank_state == "ranked").then_some(1),
                rank_change: None,
                score_bps: (rank_state == "ranked").then_some(5_000),
                components: MarketProviderRankComponentsView {
                    service_quality_bps: 5_000,
                    effective_choice_bps: 5_000,
                    fulfillment_bps: 5_000,
                    supply_breadth_bps: 5_000,
                },
                independent_buyer_count: 3,
                observation_days: 7,
                inventory: MarketProviderInventoryView {
                    active_share_count: 1,
                    available_share_seats: 1,
                    host_total: 0,
                    idle_host_total: 0,
                    country_count: 0,
                    app_count: 1,
                    has_share_market: true,
                    has_client_market: false,
                },
                quality: MarketProviderQualityView {
                    share_probe_successes: 1,
                    share_probe_total: 1,
                    host_online_samples: 0,
                    host_observed_samples: 0,
                    fulfillment_successes: 1,
                    fulfillment_total: 1,
                },
                performance: MarketProviderPerformanceView {
                    average_ttft_ms: None,
                    average_tps: None,
                    sample_count: 0,
                    display_only: true,
                },
                exploration: false,
                joined_at: "now".into(),
            },
        }
    }

    #[test]
    fn public_provider_shape_never_serializes_private_identity() {
        assert!(!public_provider_display_name("mp_public", "seller@example.com").contains('@'));
        let internal = provider("mp_public", "ranked");
        let json = serde_json::to_value(&internal.public).expect("serialize provider");
        fn assert_public_keys(value: &serde_json::Value) {
            match value {
                serde_json::Value::Object(object) => {
                    for (key, value) in object {
                        let key = key.to_ascii_lowercase();
                        assert!(!key.contains("email"), "public key leaks email: {key}");
                        assert!(!key.contains("userid"), "public key leaks user id: {key}");
                        assert!(!key.contains("funding"), "public key leaks funding: {key}");
                        assert!(!key.contains("balance"), "public key leaks balance: {key}");
                        assert!(!key.contains("credit"), "public key leaks credit: {key}");
                        assert_public_keys(value);
                    }
                }
                serde_json::Value::Array(values) => {
                    for value in values {
                        assert_public_keys(value);
                    }
                }
                _ => {}
            }
        }
        assert_public_keys(&json);
    }

    #[test]
    fn exploration_is_deterministic_and_does_not_change_formal_rank() {
        let input = vec![
            provider("rank-1", "ranked"),
            provider("rank-2", "ranked"),
            provider("rank-3", "ranked"),
            provider("rank-4", "ranked"),
            provider("rank-5", "ranked"),
            provider("new-a", "collecting"),
            provider("new-b", "collecting"),
        ];
        let first = recommended_order("generation", input.clone());
        let second = recommended_order("generation", input);
        assert_eq!(
            first
                .iter()
                .map(|provider| &provider.public.id)
                .collect::<Vec<_>>(),
            second
                .iter()
                .map(|provider| &provider.public.id)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            first
                .iter()
                .filter(|provider| provider.public.exploration)
                .count(),
            1
        );
        assert!(
            first
                .iter()
                .filter(|provider| provider.public.exploration)
                .all(|provider| {
                    provider.public.rank_state == "collecting"
                        && provider.public.rank_position.is_none()
                })
        );
    }

    #[test]
    fn public_share_detail_uses_the_same_live_supply_boundary_as_the_catalog() {
        let conn = database();
        conn.execute_batch(
            "INSERT INTO market_provider_profiles (
                id, canonical_email, display_name, claim_state,
                status, created_at, updated_at
             ) VALUES ('mp_supply', 'supplier@example.com', 'Supplier',
                       'unclaimed', 'active', 'now', 'now');
             INSERT INTO shares (
                share_id, capacity_pool_id, installation_id, share_name,
                owner_email, app_type, enabled_claude, token_limit,
                parallel_limit, tokens_used, requests_count, share_status,
                created_at, expires_at, user_grants_json, bindings_json,
                supported_user_token_periods_json, config_revision, updated_at
             ) VALUES ('share-supply', 'pool-supply', 'installation-supply',
                       'supplier@example.com', 'supplier@example.com', 'claude', 1, -1, 3,
                       0, 0, 'active', 'now', '9999-12-31T23:59:59Z', '{}',
                       '{\"claude\":\"provider-test\"}', '[]', 1, 'now');
             INSERT INTO share_market_listings (
                id, share_id, installation_id, owner_user_id, owner_email,
                status, created_at, updated_at, market_provider_id
             ) VALUES ('listing-supply', 'share-supply', 'installation-supply',
                       'supplier', 'supplier@example.com', 'active', 'now', 'now',
                       'mp_supply');
             INSERT INTO share_bindings (share_id, app_type, provider_id)
             VALUES ('share-supply', 'claude', 'provider-test');
             INSERT INTO share_market_seats (
                id, listing_id, position, status, token_period_json,
                offer_revision, created_at, updated_at
             ) VALUES ('seat-supply', 'listing-supply', 1, 'available', '{}',
                       1, 'now', 'now');",
        )
        .expect("seed public Share supply");
        let initial = share_supply(&conn, "mp_supply").unwrap();
        assert_eq!(initial.len(), 1);
        assert_eq!(initial[0].apps, vec!["claude"]);
        assert!(initial[0].share_name.starts_with("Share "));
        assert!(!initial[0].share_name.contains('@'));
        assert!(
            !serde_json::to_string(&initial)
                .expect("serialize public Share supply")
                .contains("supplier@example.com")
        );

        conn.execute(
            "DELETE FROM share_bindings WHERE share_id = 'share-supply'",
            [],
        )
        .expect("remove stale Share binding");
        assert!(share_supply(&conn, "mp_supply").unwrap()[0].apps.is_empty());
        conn.execute(
            "INSERT INTO share_bindings (share_id, app_type, provider_id)
             VALUES ('share-supply', 'claude', 'provider-test')",
            [],
        )
        .expect("restore Share binding");

        conn.execute(
            "UPDATE shares SET share_status = 'paused' WHERE share_id = 'share-supply'",
            [],
        )
        .expect("pause public Share");
        assert!(share_supply(&conn, "mp_supply").unwrap().is_empty());

        conn.execute_batch(
            "UPDATE shares SET share_status = 'active' WHERE share_id = 'share-supply';
             UPDATE share_market_seats SET retired_at = 'now' WHERE id = 'seat-supply';",
        )
        .expect("retire public Share seat");
        assert!(share_supply(&conn, "mp_supply").unwrap().is_empty());
    }

    #[test]
    fn public_client_detail_omits_disabled_hosts() {
        let conn = database();
        conn.execute_batch(
            "INSERT INTO market_provider_profiles (
                id, canonical_email, display_name, claim_state,
                status, created_at, updated_at
             ) VALUES ('mp_client_supply', 'client-supplier@example.com',
                       'Client Supplier', 'unclaimed', 'active', 'now', 'now');
             INSERT INTO host_provider_profiles (
                provider_id, owner_email, created_at, updated_at, market_provider_id
             ) VALUES ('client-supplier', 'client-supplier@example.com', 'now', 'now',
                       'mp_client_supply');
             INSERT INTO router_ssh_hosts (
                id, ip, port, host_owner_email, country_code, status,
                created_at, updated_at, provider_id
             ) VALUES ('client-disabled', '203.0.113.40', 22,
                       'client-supplier@example.com', 'US', 'disabled', 'now', 'now',
                       'client-supplier');",
        )
        .expect("seed disabled public Client supply");
        assert!(client_supply(&conn, "mp_client_supply").unwrap().is_empty());

        conn.execute(
            "UPDATE router_ssh_hosts SET status = 'idle' WHERE id = 'client-disabled'",
            [],
        )
        .expect("enable public Client supply");
        let supply = client_supply(&conn, "mp_client_supply").unwrap();
        assert_eq!(supply.len(), 1);
        assert_eq!(supply[0].country_code, "US");
        assert_eq!(supply[0].idle_hosts, 1);
        assert_eq!(supply[0].total_hosts, 1);

        conn.execute(
            "UPDATE router_ssh_hosts SET country_code = NULL WHERE id = 'client-disabled'",
            [],
        )
        .expect("clear Client Host country");
        let unknown = client_supply(&conn, "mp_client_supply").unwrap();
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].country_code, "—");
    }

    #[test]
    fn public_responses_are_cacheable_and_private_funding_is_not() {
        let initial = public_json(&HeaderMap::new(), &serde_json::json!({ "id": "mp_public" }))
            .expect("build public response");
        assert_eq!(initial.status(), StatusCode::OK);
        assert_eq!(
            initial.headers().get(header::CACHE_CONTROL).unwrap(),
            PUBLIC_CACHE_CONTROL
        );
        let etag = initial
            .headers()
            .get(header::ETAG)
            .cloned()
            .expect("public ETag");
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag.clone());
        let revalidated = public_json(&headers, &serde_json::json!({ "id": "mp_public" }))
            .expect("revalidate public response");
        assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(revalidated.headers().get(header::ETAG), Some(&etag));

        let mut weak_headers = HeaderMap::new();
        weak_headers.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_str(&format!("W/{}", etag.to_str().unwrap())).unwrap(),
        );
        assert_eq!(
            public_json(&weak_headers, &serde_json::json!({ "id": "mp_public" }))
                .expect("weakly revalidate public response")
                .status(),
            StatusCode::NOT_MODIFIED
        );

        let mut wildcard_headers = HeaderMap::new();
        wildcard_headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static("*"));
        assert_eq!(
            public_json(&wildcard_headers, &serde_json::json!({ "id": "mp_public" }))
                .expect("wildcard revalidate public response")
                .status(),
            StatusCode::NOT_MODIFIED
        );

        let mut repeated_headers = HeaderMap::new();
        repeated_headers.append(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("\"not-this-one\""),
        );
        repeated_headers.append(header::IF_NONE_MATCH, etag.clone());
        assert_eq!(
            public_json(&repeated_headers, &serde_json::json!({ "id": "mp_public" }))
                .expect("revalidate across repeated headers")
                .status(),
            StatusCode::NOT_MODIFIED
        );
    }
    #[test]
    fn paused_providers_trail_and_never_take_exploration_slots() {
        let mut paused_ranked = provider("paused-ranked", "ranked");
        paused_ranked.public.status = "paused".into();
        let mut paused_new = provider("paused-new", "collecting");
        paused_new.public.status = "paused".into();
        let order = recommended_order(
            "generation",
            vec![
                paused_ranked,
                provider("rank-1", "ranked"),
                paused_new,
                provider("new-a", "collecting"),
            ],
        );
        let ids = order
            .iter()
            .map(|provider| provider.public.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(&ids[..2], &["rank-1", "new-a"]);
        assert_eq!(&ids[2..], &["paused-ranked", "paused-new"]);
        assert!(order[1].public.exploration);
        assert!(
            order[2..]
                .iter()
                .all(|provider| !provider.public.exploration)
        );
    }

    #[test]
    fn public_share_names_reuse_the_catalog_subdomain_but_never_an_email() {
        assert_eq!(public_share_name("listing-a", " team-gpu "), "team-gpu");
        assert!(public_share_name("listing-a", "").starts_with("Share "));
        assert!(public_share_name("listing-a", "seller@example.com").starts_with("Share "));
        assert_eq!(
            public_share_name("listing-a", ""),
            public_share_name("listing-a", "")
        );
    }

    #[test]
    fn live_inventory_replaces_the_generation_snapshot_in_detail() {
        let mut inventory = provider("mp_live", "ranked").public.inventory;
        inventory.available_share_seats = 3;
        apply_live_inventory(
            &mut inventory,
            &[MarketProviderShareSupplyView {
                listing_id: "listing".into(),
                share_id: "share".into(),
                share_name: "share".into(),
                apps: vec!["claude".into(), "codex".into()],
                available_seats: 0,
                total_seats: 2,
            }],
            &[
                MarketProviderClientSupplyView {
                    country_code: "US".into(),
                    idle_hosts: 1,
                    total_hosts: 2,
                },
                MarketProviderClientSupplyView {
                    country_code: "—".into(),
                    idle_hosts: 0,
                    total_hosts: 1,
                },
            ],
        );
        assert_eq!(inventory.available_share_seats, 0);
        assert_eq!(inventory.active_share_count, 1);
        assert_eq!(inventory.app_count, 2);
        assert_eq!(inventory.host_total, 3);
        assert_eq!(inventory.idle_host_total, 1);
        assert_eq!(inventory.country_count, 1);
        assert!(inventory.has_share_market && inventory.has_client_market);
    }

    fn error_code(error: AppError) -> &'static str {
        match error {
            AppError::Coded { code, .. } => code,
            other => panic!("expected a coded error, got {other:?}"),
        }
    }

    #[test]
    fn display_names_reject_contact_details_and_impersonation() {
        let own = "Provider ABC123";
        assert_eq!(
            normalize_display_name("  Fast   GPU  ", own).unwrap(),
            "Fast GPU"
        );
        assert_eq!(normalize_display_name("   ", own).unwrap(), own);
        assert_eq!(
            normalize_display_name("provider abc123", own).unwrap(),
            "provider abc123"
        );
        assert_eq!(
            normalize_display_name("东京稳定节点", own).unwrap(),
            "东京稳定节点"
        );
        for (raw, code) in [
            ("x", "provider_name_length"),
            (
                &"长".repeat(DISPLAY_NAME_MAX_CHARS + 1)[..],
                "provider_name_length",
            ),
            ("me@example.com", "provider_name_invalid"),
            ("https://spam.example", "provider_name_invalid"),
            ("www.spam.example", "provider_name_invalid"),
            ("Router Official", "provider_name_reserved"),
            ("官方直营", "provider_name_reserved"),
            ("Provider FFFFFF", "provider_name_reserved"),
        ] {
            assert_eq!(
                error_code(normalize_display_name(raw, own).unwrap_err()),
                code,
                "{raw}"
            );
        }
    }

    #[test]
    fn display_name_updates_are_unique_audited_and_owner_scoped() {
        let conn = database();
        conn.execute_batch(
            "INSERT INTO users (id, email_normalized, status, created_at, last_login_at)
             VALUES ('seller', 'seller@example.com', 'active', 'now', 'now');
             INSERT INTO market_provider_profiles (
                id, user_id, canonical_email, display_name, claim_state,
                status, created_at, updated_at, claimed_at
             ) VALUES ('mp_seller', 'seller', 'seller@example.com', 'Provider SELLER',
                       'claimed', 'active', 'now', 'now', 'now');
             INSERT INTO market_provider_profiles (
                id, canonical_email, display_name, claim_state,
                status, created_at, updated_at
             ) VALUES ('mp_other', 'other@example.com', 'Tokyo GPU',
                       'unclaimed', 'active', 'now', 'now');",
        )
        .expect("seed Provider profiles");
        let now = Utc::now();
        assert_eq!(
            error_code(update_display_name_conn(&conn, "seller", "tokyo gpu", now).unwrap_err()),
            "provider_name_taken"
        );
        assert_eq!(
            error_code(update_display_name_conn(&conn, "nobody", "Osaka GPU", now).unwrap_err()),
            "provider_profile_missing"
        );
        update_display_name_conn(&conn, "seller", "Osaka GPU", now).expect("rename Provider");
        let (name, events): (String, i64) = conn
            .query_row(
                "SELECT display_name,
                        (SELECT COUNT(*) FROM market_provider_identity_events
                         WHERE market_provider_id = 'mp_seller'
                           AND event_kind = 'display_name_changed')
                 FROM market_provider_profiles WHERE id = 'mp_seller'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read renamed Provider");
        assert_eq!((name.as_str(), events), ("Osaka GPU", 1));
        let mine = my_market_provider_view(&conn, "seller", None).expect("read own Provider");
        assert_eq!(mine.profile.expect("own profile").display_name, "Osaka GPU");
        assert!(mine.generation.is_none() && mine.standing.is_none());
    }
    #[test]
    fn rank_change_compares_a_day_earlier_under_the_same_algorithm() {
        let conn = database();
        let now = Utc::now();
        let generation = |id: &str, version: &str, published: DateTime<Utc>| {
            conn.execute(
                "INSERT INTO market_provider_rank_generations (
                    id, algorithm_version, state, observed_from, observed_to,
                    provider_count, created_at, published_at
                 ) VALUES (?1, ?2, 'published', ?3, ?3, 2, ?3, ?3)",
                params![id, version, published.to_rfc3339()],
            )
            .expect("insert rank generation");
        };
        let entry = |generation_id: &str, provider_id: &str, position: Option<i64>| {
            conn.execute(
                "INSERT INTO market_provider_rank_entries (
                    generation_id, market_provider_id, rank_position, rank_state,
                    score_bps, service_quality_bps, effective_choice_bps,
                    fulfillment_bps, supply_breadth_bps, independent_buyer_count,
                    observation_days, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 0, 0, 0, 0, 3, 7, 'now')",
                params![
                    generation_id,
                    provider_id,
                    position,
                    if position.is_some() {
                        "ranked"
                    } else {
                        "collecting"
                    },
                    position.map(|_| 5_000),
                ],
            )
            .expect("insert rank entry");
        };
        conn.execute_batch(
            "INSERT INTO market_provider_profiles (
                id, canonical_email, display_name, claim_state, status, created_at, updated_at
             ) VALUES ('mp_riser', 'riser@example.com', 'Riser', 'unclaimed', 'active', 'now', 'now'),
                      ('mp_faller', 'faller@example.com', 'Faller', 'unclaimed', 'active', 'now', 'now'),
                      ('mp_new', 'new@example.com', 'Newcomer', 'unclaimed', 'active', 'now', 'now');",
        )
        .expect("seed ranked Providers");
        generation("older-v1", "provider-rank-v1", now - Duration::days(3));
        entry("older-v1", "mp_riser", Some(1));
        entry("older-v1", "mp_faller", Some(2));
        generation(
            "yesterday",
            rank::ALGORITHM_VERSION,
            now - Duration::hours(30),
        );
        entry("yesterday", "mp_riser", Some(3));
        entry("yesterday", "mp_faller", Some(1));
        entry("yesterday", "mp_new", None);
        generation("recent", rank::ALGORITHM_VERSION, now - Duration::hours(2));
        entry("recent", "mp_riser", Some(2));
        entry("recent", "mp_faller", Some(3));
        generation("current", rank::ALGORITHM_VERSION, now);
        entry("current", "mp_riser", Some(1));
        entry("current", "mp_faller", Some(2));
        entry("current", "mp_new", Some(3));

        let change = |generation_id: &str, provider_id: &str| {
            internal_provider_views(&conn, generation_id, None)
                .expect("read rank views")
                .into_iter()
                .find(|provider| provider.public.id == provider_id)
                .expect("ranked Provider")
                .public
                .rank_change
        };
        // Compared with the generation at least a day old, not the latest one.
        assert_eq!(change("current", "mp_riser"), Some(2));
        assert_eq!(change("current", "mp_faller"), Some(-1));
        assert_eq!(change("current", "mp_new"), None);
        // The only older generation ran another algorithm: nothing to compare.
        assert_eq!(change("yesterday", "mp_riser"), None);
    }
}
