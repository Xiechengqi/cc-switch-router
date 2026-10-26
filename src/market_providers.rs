use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::get;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ServerState;
use crate::db::{OptionalExtension, params};
use crate::error::AppError;

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
pub struct MarketProviderListView {
    pub generation: MarketProviderRankGenerationView,
    pub providers: Vec<MarketProviderView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketProviderShareSupplyView {
    pub listing_id: String,
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
}

#[derive(Debug, Clone)]
struct InternalProviderView {
    public: MarketProviderView,
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/market-providers", get(list_market_providers))
        .route("/v1/market-providers/:id", get(get_market_provider))
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
        .ok_or_else(|| AppError::ServiceUnavailable("Market Provider rank is not ready".into()))?;
    let stale = parse_timestamp(&row.2)
        .is_none_or(|published| now - published > Duration::seconds(GENERATION_STALE_AFTER_SECS));
    Ok(MarketProviderRankGenerationView {
        id: row.0,
        algorithm_version: row.1,
        published_at: row.2,
        stale,
        weights: MarketProviderRankWeightsView {
            service_quality: 45,
            effective_choice: 30,
            fulfillment: 15,
            supply_breadth: 10,
        },
        minimum_independent_buyers: 3,
        minimum_observation_days: 7,
    })
}

fn public_provider_display_name(profile_id: &str, stored: &str) -> String {
    let stored = stored.trim();
    if !stored.is_empty() && stored.len() <= 80 && !stored.contains('@') {
        return stored.to_string();
    }
    let digest = Sha256::digest(profile_id.as_bytes());
    format!(
        "Provider {}",
        &hex::encode(digest)[..8].to_ascii_uppercase()
    )
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
                    entry.ttft_ms, entry.tps, entry.metrics_json
             FROM market_provider_rank_entries entry
             JOIN market_provider_profiles profile ON profile.id = entry.market_provider_id
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

fn public_share_label(listing_id: &str) -> String {
    let digest = Sha256::digest(listing_id.as_bytes());
    format!("Share {}", &hex::encode(digest)[..8].to_ascii_uppercase())
}

fn recommended_order(
    generation_id: &str,
    providers: Vec<InternalProviderView>,
) -> Vec<InternalProviderView> {
    let (mut ranked, mut collecting): (Vec<_>, Vec<_>) = providers
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
    ranked
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
    let providers = internal_provider_views(
        &conn,
        &generation.id,
        state.config.official_provider_email(),
    )?;
    let providers = recommended_order(&generation.id, providers)
        .into_iter()
        .map(|provider| provider.public)
        .collect();
    public_json(
        &headers,
        &MarketProviderListView {
            generation,
            providers,
        },
    )
}

fn share_supply(
    conn: &crate::db::Connection,
    profile_id: &str,
) -> Result<Vec<MarketProviderShareSupplyView>, AppError> {
    conn.prepare(
        "SELECT listing.id, share.share_name,
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
         GROUP BY listing.id, share.share_name, listing.share_id,
                  share.enabled_claude, share.enabled_codex, share.enabled_gemini
         ORDER BY listing.created_at, listing.id",
    )
    .and_then(|mut statement| {
        statement
            .query_map(params![profile_id], |row| {
                let listing_id = row.get::<_, String>(0)?;
                let mut apps = Vec::new();
                if row.get::<_, i64>(2)? != 0 {
                    apps.push("claude".into());
                }
                if row.get::<_, i64>(3)? != 0 {
                    apps.push("codex".into());
                }
                if row.get::<_, i64>(4)? != 0 {
                    apps.push("gemini".into());
                }
                Ok(MarketProviderShareSupplyView {
                    share_name: public_share_label(&listing_id),
                    listing_id,
                    apps,
                    available_seats: row.get(5)?,
                    total_seats: row.get(6)?,
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
    let provider = recommended_order(
        &generation.id,
        internal_provider_views(
            &conn,
            &generation.id,
            state.config.official_provider_email(),
        )?,
    )
    .into_iter()
    .find(|provider| provider.public.id == id)
    .ok_or_else(|| AppError::NotFound("Market Provider not found".into()))?;
    let detail = MarketProviderDetailView {
        share_supply: share_supply(&conn, &id)?,
        client_supply: client_supply(&conn, &id)?,
        provider: provider.public,
    };
    public_json(&headers, &detail)
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
}
