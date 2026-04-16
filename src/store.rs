use std::collections::{HashMap, HashSet, hash_map::Entry};
use std::sync::Arc;
use std::time::Duration as StdDuration;

use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rand::distributions::{Alphanumeric, DistString};
use rusqlite::{Connection, OptionalExtension, params};
use tokio::sync::Mutex;
use tokio::time::timeout;
use uuid::Uuid;

use crate::ServerGeo;
use crate::config::Config;
use crate::error::AppError;
use crate::models::{
    ClientMetadata, DashboardClientView, DashboardMap, DashboardMapPoint, DashboardPresenceRequest,
    DashboardResponse, DashboardStats, HealthCheckEntry, Installation, InstallationView,
    IssueLeaseRequest, IssueLeaseResponse, LatLonPoint, LeaseView, PublicMapPointsResponse,
    RegisterInstallationRequest, RegisterInstallationResponse, ShareBatchSyncRequest,
    ShareClaimSubdomainRequest, ShareDeleteRequest, ShareDescriptor, ShareHeartbeatRequest,
    ShareRequestLogBatchSyncRequest, ShareRequestLogEntry, ShareRequestLogFetchResponse,
    ShareSupport, ShareSyncRequest, ShareView, TunnelLease,
};
use crate::proxy::ProxyRegistry;

const SHARE_REQUEST_LOG_RECOVERY_LIMIT: usize = 10;
const PUBLIC_MAP_CLIENT_ACTIVE_WINDOW_MINUTES: i64 = 5;

fn country_centroid(country_code: &str) -> Option<(f64, f64)> {
    match country_code {
        "AR" => Some((-38.42, -63.62)),
        "AT" => Some((47.52, 14.55)),
        "AU" => Some((-25.27, 133.77)),
        "BE" => Some((50.50, 4.47)),
        "BR" => Some((-14.23, -51.92)),
        "CA" => Some((56.13, -106.35)),
        "CH" => Some((46.82, 8.23)),
        "CL" => Some((-35.68, -71.54)),
        "CN" => Some((35.86, 104.20)),
        "CO" => Some((4.57, -74.30)),
        "DE" => Some((51.17, 10.45)),
        "DK" => Some((56.26, 9.50)),
        "EG" => Some((26.82, 30.80)),
        "ES" => Some((40.46, -3.75)),
        "FI" => Some((61.92, 25.75)),
        "FR" => Some((46.22, 2.21)),
        "GB" => Some((55.38, -3.43)),
        "GR" => Some((39.07, 21.82)),
        "HK" => Some((22.32, 114.17)),
        "ID" => Some((-0.79, 113.92)),
        "IE" => Some((53.41, -8.24)),
        "IL" => Some((31.05, 34.85)),
        "IN" => Some((20.59, 78.96)),
        "IT" => Some((41.87, 12.57)),
        "JP" => Some((36.20, 138.25)),
        "KR" => Some((35.91, 127.77)),
        "MX" => Some((23.63, -102.55)),
        "MY" => Some((4.21, 101.98)),
        "NG" => Some((9.08, 8.67)),
        "NL" => Some((52.13, 5.29)),
        "NO" => Some((60.47, 8.47)),
        "NZ" => Some((-40.90, 174.89)),
        "PE" => Some((-9.19, -75.02)),
        "PH" => Some((12.88, 121.77)),
        "PL" => Some((51.92, 19.15)),
        "PT" => Some((39.40, -8.22)),
        "RU" => Some((61.52, 105.31)),
        "SA" => Some((23.89, 45.08)),
        "SE" => Some((60.13, 18.64)),
        "SG" => Some((1.35, 103.82)),
        "TH" => Some((15.87, 100.99)),
        "TR" => Some((38.96, 35.24)),
        "TW" => Some((23.70, 120.96)),
        "UA" => Some((48.38, 31.17)),
        "US" => Some((39.83, -98.58)),
        "VN" => Some((14.06, 108.28)),
        "ZA" => Some((-30.56, 22.94)),
        _ => None,
    }
}

#[derive(Clone)]
pub struct AppStore {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Debug, Clone)]
struct GeoLookupResult {
    country_code: Option<String>,
    country: Option<String>,
    region: Option<String>,
    city: Option<String>,
    latitude: Option<f64>,
    longitude: Option<f64>,
}

#[derive(Debug, Clone)]
struct InstallationGeoState {
    last_seen_ip: Option<String>,
    country_code: Option<String>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    geo_candidate_country_code: Option<String>,
    geo_candidate_latitude: Option<f64>,
    geo_candidate_longitude: Option<f64>,
    geo_candidate_hits: i64,
    geo_candidate_first_seen_at: Option<DateTime<Utc>>,
    geo_last_changed_at: Option<DateTime<Utc>>,
}

const GEO_STABLE_DISTANCE_KM: f64 = 120.0;
const GEO_CANDIDATE_DISTANCE_KM: f64 = 120.0;
const GEO_CANDIDATE_CONFIRM_HITS: i64 = 3;
const GEO_CANDIDATE_MIN_AGE_SECS: i64 = 10 * 60;
const GEO_STABLE_MIN_SWITCH_SECS: i64 = 30 * 60;

#[derive(Debug, Clone)]
pub struct ShareRouteTarget {
    pub share_id: String,
    pub subdomain: String,
}

impl AppStore {
    pub fn new(config: &Config) -> Result<Self, AppError> {
        if let Some(parent) = config.db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AppError::Internal(format!("create db dir failed: {e}")))?;
        }
        let conn = Connection::open(&config.db_path)
            .map_err(|e| AppError::Internal(format!("open db failed: {e}")))?;
        init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub async fn register_installation(
        &self,
        input: RegisterInstallationRequest,
        metadata: ClientMetadata,
    ) -> Result<RegisterInstallationResponse, AppError> {
        let public_key = input.public_key.trim();
        if public_key.is_empty() {
            return Err(AppError::BadRequest("public_key is required".into()));
        }
        let platform = input.platform.trim();
        if platform.is_empty() {
            return Err(AppError::BadRequest("platform is required".into()));
        }
        let now = Utc::now();
        let ip = metadata.ip.clone();
        let country_code = metadata.country_code.clone();
        let conn = self.conn.lock().await;
        if let Some(existing_installation_id) =
            find_installation_id_by_public_key(&conn, public_key)?
        {
            conn.execute(
                "UPDATE installations
                 SET public_key = ?2,
                     platform = ?3,
                     app_version = ?4,
                     last_seen_ip = COALESCE(?5, last_seen_ip),
                     country_code = COALESCE(?6, country_code),
                     last_seen_at = ?7
                 WHERE id = ?1",
                params![
                    existing_installation_id,
                    public_key,
                    platform,
                    input.app_version,
                    ip,
                    country_code,
                    now.to_rfc3339(),
                ],
            )
            .map_err(|e| AppError::Internal(format!("update installation failed: {e}")))?;
            drop(conn);
            self.refresh_installation_geo(&existing_installation_id, &ip, true)
                .await?;
            return Ok(RegisterInstallationResponse {
                installation_id: existing_installation_id,
            });
        }

        let installation = Installation {
            id: Uuid::new_v4().to_string(),
            public_key: public_key.to_string(),
            platform: platform.to_string(),
            app_version: input.app_version,
            last_seen_ip: ip.clone(),
            country_code,
            country: None,
            region: None,
            city: None,
            latitude: None,
            longitude: None,
            geo_candidate_country_code: None,
            geo_candidate_country: None,
            geo_candidate_region: None,
            geo_candidate_city: None,
            geo_candidate_latitude: None,
            geo_candidate_longitude: None,
            geo_candidate_hits: 0,
            geo_candidate_first_seen_at: None,
            geo_last_changed_at: None,
            created_at: now,
            last_seen_at: now,
        };
        conn.execute(
            "INSERT INTO installations (
                id, public_key, platform, app_version, last_seen_ip, country_code, country, region,
                city, latitude, longitude, geo_candidate_country_code, geo_candidate_country,
                geo_candidate_region, geo_candidate_city, geo_candidate_latitude,
                geo_candidate_longitude, geo_candidate_hits, geo_candidate_first_seen_at,
                geo_last_changed_at, created_at, last_seen_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
            params![
                installation.id,
                installation.public_key,
                installation.platform,
                installation.app_version,
                installation.last_seen_ip,
                installation.country_code,
                installation.country,
                installation.region,
                installation.city,
                installation.latitude,
                installation.longitude,
                installation.geo_candidate_country_code,
                installation.geo_candidate_country,
                installation.geo_candidate_region,
                installation.geo_candidate_city,
                installation.geo_candidate_latitude,
                installation.geo_candidate_longitude,
                installation.geo_candidate_hits,
                installation
                    .geo_candidate_first_seen_at
                    .map(|value| value.to_rfc3339()),
                installation.geo_last_changed_at.map(|value| value.to_rfc3339()),
                installation.created_at.to_rfc3339(),
                installation.last_seen_at.to_rfc3339(),
            ],
        )
        .map_err(|e| AppError::Internal(format!("insert installation failed: {e}")))?;
        drop(conn);
        self.refresh_installation_geo(&installation.id, &ip, true)
            .await?;
        Ok(RegisterInstallationResponse {
            installation_id: installation.id,
        })
    }

    pub async fn record_dashboard_presence(
        &self,
        input: DashboardPresenceRequest,
    ) -> Result<usize, AppError> {
        let session_id = input.session_id.trim();
        if session_id.is_empty() {
            return Err(AppError::BadRequest("session_id is required".into()));
        }

        let now = Utc::now().timestamp();
        let cutoff = now - 30;
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO dashboard_presence (session_id, last_seen_at)
             VALUES (?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET last_seen_at = excluded.last_seen_at",
            params![session_id, now],
        )
        .map_err(|e| AppError::Internal(format!("upsert dashboard presence failed: {e}")))?;
        conn.execute(
            "DELETE FROM dashboard_presence WHERE last_seen_at < ?1",
            params![cutoff],
        )
        .map_err(|e| AppError::Internal(format!("prune dashboard presence failed: {e}")))?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dashboard_presence WHERE last_seen_at >= ?1",
                params![cutoff],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Internal(format!("count dashboard presence failed: {e}")))?;
        Ok(count as usize)
    }

    pub async fn issue_lease(
        &self,
        config: &Config,
        proxy: &ProxyRegistry,
        input: IssueLeaseRequest,
        metadata: ClientMetadata,
    ) -> Result<IssueLeaseResponse, AppError> {
        let now = Utc::now();
        let skew = (now.timestamp_millis() - input.timestamp_ms).abs();
        if skew > 60_000 {
            return Err(AppError::Unauthorized("stale lease request".into()));
        }

        let installation = {
            let conn = self.conn.lock().await;
            let installation = get_installation(&conn, &input.installation_id)?
                .ok_or_else(|| AppError::Unauthorized("installation not found".into()))?;
            let should_refresh_geo =
                should_refresh_installation_geo(&installation, metadata.ip.as_deref());
            touch_installation_presence(&conn, &input.installation_id, &metadata, now)?;
            (installation, should_refresh_geo)
        };
        if installation.1 {
            self.refresh_installation_geo(&input.installation_id, &metadata.ip, false)
                .await?;
        }
        let installation = installation.0;

        let tunnel_type = input.tunnel_type.to_ascii_lowercase();
        if tunnel_type != "http" {
            return Err(AppError::BadRequest(
                "only http tunnels are supported".into(),
            ));
        }

        let requested_subdomain = normalize_subdomain(&input.requested_subdomain)?;
        let subdomain = if let Some(share) = input.share.as_ref() {
            let conn = self.conn.lock().await;
            let owned_subdomain =
                get_share_owned_subdomain(&conn, &input.installation_id, &share.share_id)?
                    .ok_or_else(|| AppError::Conflict("share subdomain is not claimed".into()))?;
            if owned_subdomain != requested_subdomain {
                return Err(AppError::Conflict(
                    "requested subdomain does not match claimed subdomain".into(),
                ));
            }
            owned_subdomain
        } else {
            requested_subdomain
        };
        {
            let conn = self.conn.lock().await;
            let live_lease_exists: bool = conn
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM leases
                        WHERE subdomain = ?1 AND expires_at > ?2
                    )",
                    params![subdomain, now.to_rfc3339()],
                    |row| row.get(0),
                )
                .map_err(|e| AppError::Internal(format!("check live lease failed: {e}")))?;
            if live_lease_exists {
                return Err(AppError::Conflict("subdomain already leased".into()));
            }
        }
        if proxy
            .backend_for_host(
                &format!("{subdomain}.{}", config.tunnel_domain),
                &config.tunnel_domain,
            )
            .await
            .is_some()
        {
            return Err(AppError::Conflict("subdomain already in use".into()));
        }

        verify_signature(&installation.public_key, &input)?;

        if let Some(share) = input.share.clone() {
            self.upsert_share(&input.installation_id, share).await?;
        }

        let issued_at = Utc::now();
        let expires_at = issued_at + Duration::seconds(config.lease_ttl_secs);
        let connection_id = Uuid::new_v4().to_string();
        let ssh_password = Alphanumeric.sample_string(&mut rand::thread_rng(), 24);
        let lease = TunnelLease {
            id: Uuid::new_v4().to_string(),
            installation_id: installation.id.clone(),
            connection_id: connection_id.clone(),
            subdomain: subdomain.clone(),
            tunnel_type,
            ssh_username: connection_id.clone(),
            ssh_password: ssh_password.clone(),
            issued_at,
            expires_at,
            used_at: None,
            share: input.share,
        };

        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO leases (
                id, installation_id, connection_id, subdomain, tunnel_type,
                ssh_username, ssh_password, issued_at, expires_at, used_at, share_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                lease.id,
                lease.installation_id,
                lease.connection_id,
                lease.subdomain,
                lease.tunnel_type,
                lease.ssh_username,
                lease.ssh_password,
                lease.issued_at.to_rfc3339(),
                lease.expires_at.to_rfc3339(),
                Option::<String>::None,
                lease
                    .share
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|e| AppError::Internal(format!("serialize share failed: {e}")))?,
            ],
        )
        .map_err(|e| AppError::Internal(format!("insert lease failed: {e}")))?;

        Ok(IssueLeaseResponse {
            lease_id: lease.id,
            connection_id: lease.connection_id,
            ssh_username: lease.ssh_username,
            ssh_password,
            ssh_addr: config.effective_ssh_public_addr(),
            expires_at,
            tunnel_url: config.tunnel_url(&subdomain),
            subdomain,
        })
    }

    pub async fn consume_lease(
        &self,
        username: &str,
        password: &str,
    ) -> Result<TunnelLease, AppError> {
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let mut lease = get_lease_by_connection_id(&conn, username)?
            .ok_or_else(|| AppError::Unauthorized("lease not found".into()))?;
        if lease.expires_at < now {
            return Err(AppError::Unauthorized("lease expired".into()));
        }
        if lease.used_at.is_some() {
            return Err(AppError::Unauthorized("lease already used".into()));
        }
        if lease.ssh_password != password {
            return Err(AppError::Unauthorized("invalid ssh credentials".into()));
        }
        lease.used_at = Some(now);
        conn.execute(
            "UPDATE leases SET used_at = ?2 WHERE connection_id = ?1",
            params![username, now.to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("update lease use failed: {e}")))?;
        Ok(lease)
    }

    pub async fn sync_share(
        &self,
        input: ShareSyncRequest,
        metadata: ClientMetadata,
    ) -> Result<(), AppError> {
        {
            let conn = self.conn.lock().await;
            let installation = get_installation(&conn, &input.installation_id)?;
            let Some(installation) = installation else {
                return Err(AppError::Unauthorized("installation not found".into()));
            };
            let should_refresh_geo =
                should_refresh_installation_geo(&installation, metadata.ip.as_deref());
            touch_installation_presence(&conn, &input.installation_id, &metadata, Utc::now())?;
            drop(conn);
            if should_refresh_geo {
                self.refresh_installation_geo(&input.installation_id, &metadata.ip, false)
                    .await?;
            }
        }
        self.upsert_share(&input.installation_id, input.share).await
    }

    pub async fn claim_share_subdomain(
        &self,
        input: ShareClaimSubdomainRequest,
        metadata: ClientMetadata,
    ) -> Result<(), AppError> {
        let subdomain = normalize_subdomain(&input.share.subdomain)?;
        ensure_subdomain_allowed(&subdomain)?;
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?;
        let Some(installation) = installation else {
            return Err(AppError::Unauthorized("installation not found".into()));
        };
        let should_refresh_geo =
            should_refresh_installation_geo(&installation, metadata.ip.as_deref());
        touch_installation_presence(&conn, &input.installation_id, &metadata, Utc::now())?;
        drop(conn);
        if should_refresh_geo {
            self.refresh_installation_geo(&input.installation_id, &metadata.ip, false)
                .await?;
        }

        let conn = self.conn.lock().await;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AppError::Internal(format!("begin share claim tx failed: {e}")))?;
        let mut share = input.share;
        share.subdomain = subdomain;
        upsert_share_tx(&tx, &input.installation_id, share)?;
        tx.commit().map_err(map_share_constraint_error)?;
        Ok(())
    }

    pub async fn delete_share(&self, input: ShareDeleteRequest) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "DELETE FROM shares WHERE share_id = ?1 AND installation_id = ?2",
            params![input.share_id, input.installation_id],
        )
        .map_err(|e| AppError::Internal(format!("delete share failed: {e}")))?;
        Ok(())
    }

    pub async fn batch_sync_shares(
        &self,
        input: ShareBatchSyncRequest,
        metadata: ClientMetadata,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?;
        let Some(installation) = installation else {
            return Err(AppError::Unauthorized("installation not found".into()));
        };
        let should_refresh_geo =
            should_refresh_installation_geo(&installation, metadata.ip.as_deref());
        touch_installation_presence(&conn, &input.installation_id, &metadata, Utc::now())?;
        drop(conn);
        if should_refresh_geo {
            self.refresh_installation_geo(&input.installation_id, &metadata.ip, false)
                .await?;
        }

        let conn = self.conn.lock().await;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AppError::Internal(format!("begin batch sync tx failed: {e}")))?;
        for op in input.ops {
            match op.kind.as_str() {
                "upsert" => {
                    let share = op.share.ok_or_else(|| {
                        AppError::BadRequest("share is required for upsert".into())
                    })?;
                    upsert_share_tx(&tx, &input.installation_id, share)?;
                }
                "delete" => {
                    let share_id = op.share_id.ok_or_else(|| {
                        AppError::BadRequest("shareId is required for delete".into())
                    })?;
                    tx.execute(
                        "DELETE FROM shares WHERE share_id = ?1 AND installation_id = ?2",
                        params![share_id, input.installation_id],
                    )
                    .map_err(|e| {
                        AppError::Internal(format!("delete share in batch failed: {e}"))
                    })?;
                }
                other => {
                    return Err(AppError::BadRequest(format!(
                        "unsupported share batch op: {other}"
                    )));
                }
            }
        }
        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit batch sync failed: {e}")))?;
        Ok(())
    }

    pub async fn batch_sync_share_request_logs(
        &self,
        input: ShareRequestLogBatchSyncRequest,
        metadata: ClientMetadata,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?;
        let Some(installation) = installation else {
            return Err(AppError::Unauthorized("installation not found".into()));
        };
        let should_refresh_geo =
            should_refresh_installation_geo(&installation, metadata.ip.as_deref());
        touch_installation_presence(&conn, &input.installation_id, &metadata, Utc::now())?;
        drop(conn);
        if should_refresh_geo {
            self.refresh_installation_geo(&input.installation_id, &metadata.ip, false)
                .await?;
        }

        let conn = self.conn.lock().await;
        let tx = conn.unchecked_transaction().map_err(|e| {
            AppError::Internal(format!("begin request log batch sync tx failed: {e}"))
        })?;
        for log in input.logs {
            upsert_share_request_log_tx(&tx, &input.installation_id, log)?;
        }
        tx.commit().map_err(|e| {
            AppError::Internal(format!("commit request log batch sync failed: {e}"))
        })?;
        Ok(())
    }

    pub async fn dashboard_snapshot(
        &self,
        config: &Config,
        server_geo: &ServerGeo,
        proxy: &ProxyRegistry,
    ) -> Result<DashboardResponse, AppError> {
        let active_subdomains = proxy
            .active_subdomains()
            .await
            .into_iter()
            .collect::<HashSet<_>>();
        let now = Utc::now();
        let (installations, leases, shares, health_by_share, online_by_share, recent_logs) = {
            let conn = self.conn.lock().await;
            (
                list_installations(&conn)?,
                list_leases(&conn)?,
                list_shares(&conn)?,
                list_health_checks(&conn, 10)?,
                list_online_minutes_24h(&conn)?,
                list_recent_share_request_logs(&conn, SHARE_REQUEST_LOG_RECOVERY_LIMIT)?,
            )
        };
        let logs_by_share = recent_logs.into_iter().fold(
            HashMap::<String, Vec<ShareRequestLogEntry>>::new(),
            |mut acc, log| {
                acc.entry(log.share_id.clone()).or_default().push(log);
                acc
            },
        );
        let logs_by_share = self
            .recover_missing_share_request_logs(config, &active_subdomains, &shares, logs_by_share)
            .await?;

        let mut leases_by_installation: HashMap<String, Vec<TunnelLease>> = HashMap::new();
        for lease in leases {
            leases_by_installation
                .entry(lease.installation_id.clone())
                .or_default()
                .push(lease);
        }
        let mut active_share_subdomains_by_installation: HashMap<String, HashSet<String>> =
            HashMap::new();
        for (installation_id, share, _) in &shares {
            if active_subdomains.contains(&share.subdomain) {
                active_share_subdomains_by_installation
                    .entry(installation_id.clone())
                    .or_default()
                    .insert(share.subdomain.clone());
            }
        }
        let installations = deduplicate_dashboard_installations(
            installations,
            &leases_by_installation,
            &active_share_subdomains_by_installation,
        );

        let mut installation_views = Vec::new();
        let mut client_map_points = Vec::new();
        for installation in installations {
            let mut lease_views = Vec::new();
            let mut active_subdomains_for_installation = HashSet::new();
            if let Some(items) = leases_by_installation.get(&installation.id) {
                for lease in items {
                    let is_active =
                        lease.expires_at > now && active_subdomains.contains(&lease.subdomain);
                    if is_active {
                        active_subdomains_for_installation.insert(lease.subdomain.clone());
                    }
                    lease_views.push(LeaseView {
                        connection_id: lease.connection_id.clone(),
                        subdomain: lease.subdomain.clone(),
                        tunnel_type: lease.tunnel_type.clone(),
                        issued_at: lease.issued_at,
                        expires_at: lease.expires_at,
                        used_at: lease.used_at,
                        is_active,
                        share: lease.share.clone(),
                    });
                }
                lease_views.sort_by(|a, b| b.issued_at.cmp(&a.issued_at));
            }
            let active_lease_count = active_subdomains_for_installation.len();
            let is_active = active_share_subdomains_by_installation
                .get(&installation.id)
                .map(|subdomains| !subdomains.is_empty())
                .unwrap_or(false);
            client_map_points.push(DashboardMapPoint {
                id: installation.id.clone(),
                label: installation.platform.clone(),
                point_type: "client".into(),
                platform: Some(installation.platform.clone()),
                country_code: installation.country_code.clone(),
                country: installation.country.clone(),
                region: installation.region.clone(),
                city: installation.city.clone(),
                lat: installation.latitude,
                lon: installation.longitude,
                last_seen_at: Some(installation.last_seen_at),
                is_active,
            });
            installation_views.push(InstallationView {
                id: installation.id,
                platform: installation.platform,
                app_version: installation.app_version,
                region: installation.region,
                country_code: installation.country_code,
                created_at: installation.created_at,
                last_seen_at: installation.last_seen_at,
                active_lease_count,
                leases: lease_views,
            });
        }
        installation_views.sort_by(|a, b| b.last_seen_at.cmp(&a.last_seen_at));

        let share_views = shares
            .into_iter()
            .map(|(installation_id, share, _active_lease_count)| {
                let active_lease_count = usize::from(active_subdomains.contains(&share.subdomain));
                let recent_requests = logs_by_share
                    .get(&share.share_id)
                    .cloned()
                    .unwrap_or_default();
                let health_checks = health_by_share
                    .get(&share.share_id)
                    .cloned()
                    .unwrap_or_default();
                let online_minutes_24h = online_by_share.get(&share.share_id).copied().unwrap_or(0);
                ShareView {
                    share_id: share.share_id,
                    share_name: share.share_name,
                    description: share.description,
                    for_sale: share.for_sale,
                    subdomain: share.subdomain,
                    share_token: share.share_token,
                    app_type: share.app_type,
                    provider_id: share.provider_id,
                    token_limit: share.token_limit,
                    tokens_used: share.tokens_used,
                    requests_count: share.requests_count,
                    share_status: share.share_status,
                    created_at: share.created_at,
                    expires_at: share.expires_at,
                    support: share.support,
                    installation_id,
                    active_lease_count,
                    online_minutes_24h,
                    online_rate_24h: (online_minutes_24h as f64 / 1440.0) * 100.0,
                    recent_requests,
                    health_checks,
                }
            })
            .collect::<Vec<_>>();
        let mut share_by_installation = HashMap::<String, ShareView>::new();
        for share in &share_views {
            let installation_id = share.installation_id.clone();
            match share_by_installation.get(&installation_id) {
                Some(existing) if !prefer_dashboard_share(share, existing) => {}
                _ => {
                    share_by_installation.insert(installation_id, share.clone());
                }
            }
        }
        let client_views = installation_views
            .iter()
            .cloned()
            .map(|installation| DashboardClientView {
                share: share_by_installation.remove(&installation.id),
                installation,
            })
            .collect::<Vec<_>>();
        let clients_count = client_views.len();
        let active_shares_count = client_views
            .iter()
            .filter(|client| matches!(client.share.as_ref(), Some(share) if share.share_status == "active"))
            .count();
        let active_leases_count = client_views
            .iter()
            .map(|client| client.installation.active_lease_count)
            .sum();

        Ok(DashboardResponse {
            generated_at: now,
            stats: DashboardStats {
                clients: clients_count,
                active_shares: active_shares_count,
                active_leases: active_leases_count,
            },
            map: DashboardMap {
                server: server_geo
                    .lat
                    .zip(server_geo.lon)
                    .map(|(lat, lon)| DashboardMapPoint {
                        id: "server".into(),
                        label: "server".into(),
                        point_type: "server".into(),
                        platform: None,
                        country_code: None,
                        country: None,
                        region: None,
                        city: None,
                        lat: Some(lat),
                        lon: Some(lon),
                        last_seen_at: Some(now),
                        is_active: true,
                    }),
                clients: client_map_points,
            },
            clients: client_views,
        })
    }

    async fn recover_missing_share_request_logs(
        &self,
        config: &Config,
        active_subdomains: &HashSet<String>,
        shares: &[(String, ShareDescriptor, usize)],
        mut logs_by_share: HashMap<String, Vec<ShareRequestLogEntry>>,
    ) -> Result<HashMap<String, Vec<ShareRequestLogEntry>>, AppError> {
        let missing_shares = shares
            .iter()
            .filter(|(_, share, _)| {
                active_subdomains.contains(&share.subdomain)
                    && logs_by_share
                        .get(&share.share_id)
                        .map(|logs| logs.is_empty())
                        .unwrap_or(true)
            })
            .map(|(installation_id, share, _)| {
                (
                    installation_id.clone(),
                    share.share_id.clone(),
                    share.subdomain.clone(),
                )
            })
            .collect::<Vec<_>>();

        if missing_shares.is_empty() {
            return Ok(logs_by_share);
        }

        let client = reqwest::Client::builder()
            .user_agent("portr-rs/0.1 share-log-recovery")
            .timeout(StdDuration::from_secs(5))
            .build()
            .map_err(|e| {
                AppError::Internal(format!("build share log recovery client failed: {e}"))
            })?;

        for (installation_id, share_id, subdomain) in missing_shares {
            let response =
                match fetch_share_request_logs_from_route(config, &client, &subdomain).await {
                    Ok(response) => response,
                    Err(err) => {
                        tracing::debug!(
                            share_id = %share_id,
                            subdomain = %subdomain,
                            "share request log recovery skipped: {err}"
                        );
                        continue;
                    }
                };

            if response.logs.is_empty() {
                continue;
            }

            if let Some(response_share_id) = response.share_id.as_deref() {
                if response_share_id != share_id {
                    tracing::debug!(
                        share_id = %share_id,
                        response_share_id = %response_share_id,
                        subdomain = %subdomain,
                        "share request log recovery returned mismatched share id"
                    );
                }
            }

            {
                let mut recovered_logs = response.logs;
                recovered_logs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                recovered_logs.truncate(SHARE_REQUEST_LOG_RECOVERY_LIMIT);
                let conn = self.conn.lock().await;
                let tx = conn.unchecked_transaction().map_err(|e| {
                    AppError::Internal(format!("begin share request log recovery tx failed: {e}"))
                })?;
                for log in &recovered_logs {
                    upsert_share_request_log_tx(&tx, &installation_id, log.clone())?;
                }
                tx.commit().map_err(|e| {
                    AppError::Internal(format!("commit share request log recovery tx failed: {e}"))
                })?;
                logs_by_share.insert(share_id.clone(), recovered_logs);
            }

            tracing::info!(
                share_id = %share_id,
                subdomain = %subdomain,
                recovered = logs_by_share.get(&share_id).map(|logs| logs.len()).unwrap_or(0),
                "recovered share request logs from active route"
            );
        }

        Ok(logs_by_share)
    }

    pub async fn cleanup_expired_data(&self, config: &Config) -> Result<(usize, usize), AppError> {
        let cutoff = (Utc::now() - Duration::seconds(config.lease_retention_secs)).to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AppError::Internal(format!("begin cleanup tx failed: {e}")))?;

        let deleted_leases = tx
            .execute(
                "DELETE FROM leases
                 WHERE expires_at < ?1
                   AND (used_at IS NULL OR used_at < ?1)",
                params![cutoff],
            )
            .map_err(|e| AppError::Internal(format!("delete expired leases failed: {e}")))?
            as usize;

        let deleted_shares = tx
            .execute(
                "DELETE FROM shares
                 WHERE share_status IN ('expired', 'deleted')
                   AND updated_at < ?1",
                params![cutoff],
            )
            .map_err(|e| AppError::Internal(format!("delete stale shares failed: {e}")))?
            as usize;

        let _deleted_request_logs = tx
            .execute(
                "DELETE FROM share_request_logs
                 WHERE created_at < ?1",
                params![
                    DateTime::parse_from_rfc3339(&cutoff)
                        .map(|dt| dt.timestamp())
                        .unwrap_or_default()
                ],
            )
            .map_err(|e| AppError::Internal(format!("delete stale request logs failed: {e}")))?;

        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit cleanup tx failed: {e}")))?;

        Ok((deleted_leases, deleted_shares))
    }

    /// Legacy heartbeat endpoint kept for compatibility with older cc-switch
    /// clients. It updates installation presence only and no longer feeds
    /// dashboard health state.
    pub async fn record_share_heartbeat(
        &self,
        input: ShareHeartbeatRequest,
        metadata: ClientMetadata,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?;
        let Some(installation) = installation else {
            return Err(AppError::Unauthorized("installation not found".into()));
        };
        let should_refresh_geo =
            should_refresh_installation_geo(&installation, metadata.ip.as_deref());
        touch_installation_presence(&conn, &input.installation_id, &metadata, Utc::now())?;
        drop(conn);
        if should_refresh_geo {
            self.refresh_installation_geo(&input.installation_id, &metadata.ip, false)
                .await?;
        }
        Ok(())
    }

    pub async fn list_share_route_targets(&self) -> Result<Vec<ShareRouteTarget>, AppError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT share_id, subdomain
                 FROM shares
                 WHERE subdomain IS NOT NULL
                   AND subdomain != ''
                   AND subdomain != '-'
                   AND share_status != 'deleted'
                 ORDER BY share_name ASC",
            )
            .map_err(|e| AppError::Internal(format!("prepare route targets failed: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ShareRouteTarget {
                    share_id: row.get(0)?,
                    subdomain: row.get(1)?,
                })
            })
            .map_err(|e| AppError::Internal(format!("query route targets failed: {e}")))?;
        collect_rows(rows)
    }

    pub async fn public_map_points(
        &self,
        server_geo: &ServerGeo,
    ) -> Result<PublicMapPointsResponse, AppError> {
        let active_cutoff =
            (Utc::now() - Duration::minutes(PUBLIC_MAP_CLIENT_ACTIVE_WINDOW_MINUTES)).to_rfc3339();
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT latitude, longitude, country_code
                 FROM installations
                 WHERE last_seen_at >= ?1
                 ORDER BY last_seen_at DESC",
            )
            .map_err(|e| AppError::Internal(format!("prepare public map clients failed: {e}")))?;
        let rows = stmt
            .query_map(params![active_cutoff], |row| {
                let lat = row.get::<_, Option<f64>>(0)?;
                let lon = row.get::<_, Option<f64>>(1)?;
                let country_code = row.get::<_, Option<String>>(2)?;
                Ok(lat
                    .zip(lon)
                    .map(|(lat, lon)| LatLonPoint { lat, lon })
                    .or_else(|| {
                        country_code
                            .as_deref()
                            .and_then(country_centroid)
                            .map(|(lat, lon)| LatLonPoint { lat, lon })
                    }))
            })
            .map_err(|e| AppError::Internal(format!("query public map clients failed: {e}")))?;
        let clients = collect_rows(rows)?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();

        Ok(PublicMapPointsResponse {
            server: server_geo
                .lat
                .zip(server_geo.lon)
                .map(|(lat, lon)| LatLonPoint { lat, lon }),
            clients,
        })
    }

    pub async fn record_share_route_health(
        &self,
        share_id: &str,
        is_healthy: bool,
    ) -> Result<(), AppError> {
        let now = Utc::now().timestamp();
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO share_health_checks (share_id, checked_at, is_healthy) VALUES (?1, ?2, ?3)",
            params![share_id, now, if is_healthy { 1 } else { 0 }],
        )
        .map_err(|e| AppError::Internal(format!("insert route health failed: {e}")))?;
        conn.execute(
            "DELETE FROM share_health_checks WHERE checked_at < ?1",
            params![now - 86_400],
        )
        .map_err(|e| AppError::Internal(format!("prune route health failed: {e}")))?;
        Ok(())
    }

    async fn upsert_share(
        &self,
        installation_id: &str,
        mut share: ShareDescriptor,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let existing_subdomain =
            get_share_owned_subdomain(&conn, installation_id, &share.share_id)?
                .ok_or_else(|| AppError::Conflict("share subdomain is not claimed".into()))?;
        share.subdomain = existing_subdomain;
        upsert_share_tx(&conn, installation_id, share)?;
        Ok(())
    }

    async fn refresh_installation_geo(
        &self,
        installation_id: &str,
        ip: &Option<String>,
        force: bool,
    ) -> Result<(), AppError> {
        let Some(ip) = ip.as_deref().map(str::trim).filter(|v| !v.is_empty()) else {
            return Ok(());
        };
        let current_state = {
            let conn = self.conn.lock().await;
            let state = get_installation_geo_state(&conn, installation_id)?;
            let Some(state) = state else {
                return Ok(());
            };
            if !force
                && state.last_seen_ip.as_deref() == Some(ip)
                && state.latitude.is_some()
                && state.longitude.is_some()
            {
                return Ok(());
            }
            state
        };
        let Some(geo) = lookup_ip_im_geo(ip).await else {
            return Ok(());
        };
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let no_stable_position =
            current_state.latitude.is_none() || current_state.longitude.is_none();
        if no_stable_position {
            persist_stable_geo(&conn, installation_id, &geo, now)?;
            return Ok(());
        }

        let stable_distance_km = haversine_distance_km(
            current_state.latitude,
            current_state.longitude,
            geo.latitude,
            geo.longitude,
        );
        let crossed_country = current_state.country_code != geo.country_code
            && current_state.country_code.is_some()
            && geo.country_code.is_some();
        let can_stay_stable = !crossed_country
            && stable_distance_km
                .map(|distance| distance <= GEO_STABLE_DISTANCE_KM)
                .unwrap_or(false);

        if can_stay_stable {
            persist_stable_geo(&conn, installation_id, &geo, now)?;
            return Ok(());
        }

        let candidate_matches = current_state
            .geo_candidate_latitude
            .zip(current_state.geo_candidate_longitude)
            .and_then(|(lat, lon)| {
                haversine_distance_km(Some(lat), Some(lon), geo.latitude, geo.longitude)
            })
            .map(|distance| distance <= GEO_CANDIDATE_DISTANCE_KM)
            .unwrap_or(false)
            && current_state.geo_candidate_country_code == geo.country_code;

        let candidate_hits = if candidate_matches {
            current_state.geo_candidate_hits + 1
        } else {
            1
        };
        let candidate_first_seen_at = if candidate_matches {
            current_state.geo_candidate_first_seen_at.unwrap_or(now)
        } else {
            now
        };
        persist_candidate_geo(
            &conn,
            installation_id,
            &geo,
            candidate_hits,
            candidate_first_seen_at,
        )?;

        let candidate_age_secs = (now - candidate_first_seen_at).num_seconds();
        let last_change_age_secs = current_state
            .geo_last_changed_at
            .map(|value| (now - value).num_seconds())
            .unwrap_or(i64::MAX);
        let promote_candidate = candidate_hits >= GEO_CANDIDATE_CONFIRM_HITS
            && candidate_age_secs >= GEO_CANDIDATE_MIN_AGE_SECS
            && last_change_age_secs >= GEO_STABLE_MIN_SWITCH_SECS;
        if promote_candidate {
            persist_stable_geo(&conn, installation_id, &geo, now)?;
        }
        Ok(())
    }
}

async fn fetch_share_request_logs_from_route(
    config: &Config,
    client: &reqwest::Client,
    subdomain: &str,
) -> Result<ShareRequestLogFetchResponse, AppError> {
    let url = format!("{}/_portr/request-logs", config.tunnel_url(subdomain));
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("fetch share request logs failed: {e}")))?;

    if !response.status().is_success() {
        return Err(AppError::Internal(format!(
            "fetch share request logs failed with status {}",
            response.status()
        )));
    }

    response
        .json::<ShareRequestLogFetchResponse>()
        .await
        .map_err(|e| AppError::Internal(format!("decode share request logs failed: {e}")))
}

fn upsert_share_tx(
    conn: &Connection,
    installation_id: &str,
    share: ShareDescriptor,
) -> Result<(), AppError> {
    let description = normalize_share_description(share.description.clone())?;
    let for_sale = normalize_share_for_sale(&share.for_sale)?;
    let masked_share_token = mask_share_token(&share.share_token);
    conn.execute(
        "INSERT INTO shares (
            share_id, installation_id, share_name, description, for_sale, subdomain, share_token, app_type, provider_id,
            enabled_claude, enabled_codex, enabled_gemini,
            token_limit, tokens_used, requests_count, share_status, created_at, expires_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
        ON CONFLICT(share_id) DO UPDATE SET
            installation_id = excluded.installation_id,
            share_name = excluded.share_name,
            description = excluded.description,
            for_sale = excluded.for_sale,
            subdomain = excluded.subdomain,
            share_token = excluded.share_token,
            app_type = excluded.app_type,
            provider_id = excluded.provider_id,
            enabled_claude = excluded.enabled_claude,
            enabled_codex = excluded.enabled_codex,
            enabled_gemini = excluded.enabled_gemini,
            token_limit = MAX(shares.token_limit, excluded.token_limit),
            tokens_used = MAX(shares.tokens_used, excluded.tokens_used),
            requests_count = MAX(shares.requests_count, excluded.requests_count),
            share_status = excluded.share_status,
            created_at = excluded.created_at,
            expires_at = excluded.expires_at,
            updated_at = excluded.updated_at",
        params![
            share.share_id,
            installation_id,
            share.share_name,
            description,
            for_sale,
            share.subdomain,
            masked_share_token,
            share.app_type,
            share.provider_id,
            i64::from(share.support.claude as u8),
            i64::from(share.support.codex as u8),
            i64::from(share.support.gemini as u8),
            share.token_limit,
            share.tokens_used,
            share.requests_count,
            share.share_status,
            share.created_at,
            share.expires_at,
            Utc::now().to_rfc3339(),
        ],
    )
    .map_err(map_share_constraint_error)?;
    Ok(())
}

fn backfill_share_usage_from_logs_tx(conn: &Connection, share_id: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE shares
         SET tokens_used = MAX(
                 tokens_used,
                 COALESCE((
                     SELECT SUM(
                         input_tokens + output_tokens + cache_read_tokens + cache_creation_tokens
                     )
                     FROM share_request_logs
                     WHERE share_id = ?1
                 ), 0)
             ),
             requests_count = MAX(
                 requests_count,
                 COALESCE((
                     SELECT COUNT(*)
                     FROM share_request_logs
                     WHERE share_id = ?1
                 ), 0)
             ),
             updated_at = ?2
         WHERE share_id = ?1",
        params![share_id, Utc::now().to_rfc3339()],
    )
    .map_err(|e| AppError::Internal(format!("backfill share usage from logs failed: {e}")))?;
    Ok(())
}

fn upsert_share_request_log_tx(
    conn: &Connection,
    installation_id: &str,
    log: ShareRequestLogEntry,
) -> Result<(), AppError> {
    let share_id = log.share_id.clone();
    conn.execute(
        "INSERT INTO share_request_logs (
            request_id, installation_id, share_id, share_name, provider_id, provider_name,
            app_type, model, request_model, status_code, latency_ms, first_token_ms,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            is_streaming, session_id, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
        ON CONFLICT(request_id) DO UPDATE SET
            installation_id = excluded.installation_id,
            share_id = excluded.share_id,
            share_name = excluded.share_name,
            provider_id = excluded.provider_id,
            provider_name = excluded.provider_name,
            app_type = excluded.app_type,
            model = excluded.model,
            request_model = excluded.request_model,
            status_code = excluded.status_code,
            latency_ms = excluded.latency_ms,
            first_token_ms = excluded.first_token_ms,
            input_tokens = excluded.input_tokens,
            output_tokens = excluded.output_tokens,
            cache_read_tokens = excluded.cache_read_tokens,
            cache_creation_tokens = excluded.cache_creation_tokens,
            is_streaming = excluded.is_streaming,
            session_id = excluded.session_id,
            created_at = excluded.created_at",
        params![
            log.request_id,
            installation_id,
            log.share_id,
            log.share_name,
            log.provider_id,
            log.provider_name,
            log.app_type,
            log.model,
            log.request_model,
            i64::from(log.status_code),
            log.latency_ms as i64,
            log.first_token_ms.map(|v| v as i64),
            i64::from(log.input_tokens),
            i64::from(log.output_tokens),
            i64::from(log.cache_read_tokens),
            i64::from(log.cache_creation_tokens),
            i64::from(log.is_streaming as u8),
            log.session_id,
            log.created_at,
        ],
    )
    .map_err(|e| AppError::Internal(format!("upsert share request log failed: {e}")))?;
    backfill_share_usage_from_logs_tx(conn, &share_id)?;
    Ok(())
}

fn init_schema(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS installations (
            id TEXT PRIMARY KEY,
            public_key TEXT NOT NULL,
            platform TEXT NOT NULL,
            app_version TEXT NOT NULL,
            last_seen_ip TEXT,
            country_code TEXT,
            country TEXT,
            region TEXT,
            city TEXT,
            latitude REAL,
            longitude REAL,
            geo_candidate_country_code TEXT,
            geo_candidate_country TEXT,
            geo_candidate_region TEXT,
            geo_candidate_city TEXT,
            geo_candidate_latitude REAL,
            geo_candidate_longitude REAL,
            geo_candidate_hits INTEGER NOT NULL DEFAULT 0,
            geo_candidate_first_seen_at TEXT,
            geo_last_changed_at TEXT,
            created_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS leases (
            id TEXT PRIMARY KEY,
            installation_id TEXT NOT NULL,
            connection_id TEXT NOT NULL UNIQUE,
            subdomain TEXT NOT NULL,
            tunnel_type TEXT NOT NULL,
            ssh_username TEXT NOT NULL,
            ssh_password TEXT NOT NULL,
            issued_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            used_at TEXT,
            share_json TEXT
        );

        CREATE TABLE IF NOT EXISTS shares (
            share_id TEXT PRIMARY KEY,
            installation_id TEXT NOT NULL,
            share_name TEXT NOT NULL,
            description TEXT,
            for_sale TEXT NOT NULL DEFAULT 'No',
            subdomain TEXT,
            share_token TEXT NOT NULL,
            app_type TEXT NOT NULL,
            provider_id TEXT,
            enabled_claude INTEGER NOT NULL DEFAULT 0,
            enabled_codex INTEGER NOT NULL DEFAULT 0,
            enabled_gemini INTEGER NOT NULL DEFAULT 0,
            token_limit INTEGER NOT NULL,
            tokens_used INTEGER NOT NULL,
            requests_count INTEGER NOT NULL,
            share_status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS share_request_logs (
            request_id TEXT PRIMARY KEY,
            installation_id TEXT NOT NULL,
            share_id TEXT NOT NULL,
            share_name TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            provider_name TEXT NOT NULL,
            app_type TEXT NOT NULL,
            model TEXT NOT NULL,
            request_model TEXT NOT NULL,
            status_code INTEGER NOT NULL,
            latency_ms INTEGER NOT NULL,
            first_token_ms INTEGER,
            input_tokens INTEGER NOT NULL,
            output_tokens INTEGER NOT NULL,
            cache_read_tokens INTEGER NOT NULL,
            cache_creation_tokens INTEGER NOT NULL,
            is_streaming INTEGER NOT NULL,
            session_id TEXT,
            created_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS share_health_checks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            share_id TEXT NOT NULL,
            checked_at INTEGER NOT NULL,
            is_healthy INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS dashboard_presence (
            session_id TEXT PRIMARY KEY,
            last_seen_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_leases_installation_id ON leases(installation_id);
        CREATE INDEX IF NOT EXISTS idx_leases_subdomain ON leases(subdomain);
        CREATE INDEX IF NOT EXISTS idx_shares_installation_id ON shares(installation_id);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_shares_subdomain_unique ON shares(subdomain) WHERE subdomain IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_share_request_logs_share_id ON share_request_logs(share_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_share_health_checks ON share_health_checks(share_id, checked_at DESC);
        CREATE INDEX IF NOT EXISTS idx_dashboard_presence_last_seen ON dashboard_presence(last_seen_at DESC);
        ",
    )
    .map_err(|e| AppError::Internal(format!("init schema failed: {e}")))?;
    let columns = conn
        .prepare("PRAGMA table_info(installations)")
        .and_then(|mut stmt| {
            let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .map_err(|e| AppError::Internal(format!("inspect installations schema failed: {e}")))?;
    if !columns.iter().any(|name| name == "last_seen_ip") {
        conn.execute("ALTER TABLE installations ADD COLUMN last_seen_ip TEXT", [])
            .map_err(|e| {
                AppError::Internal(format!("add installations last_seen_ip failed: {e}"))
            })?;
    }
    if !columns.iter().any(|name| name == "country_code") {
        conn.execute("ALTER TABLE installations ADD COLUMN country_code TEXT", [])
            .map_err(|e| {
                AppError::Internal(format!("add installations country_code failed: {e}"))
            })?;
    }
    if !columns.iter().any(|name| name == "country") {
        conn.execute("ALTER TABLE installations ADD COLUMN country TEXT", [])
            .map_err(|e| AppError::Internal(format!("add installations country failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "region") {
        conn.execute("ALTER TABLE installations ADD COLUMN region TEXT", [])
            .map_err(|e| AppError::Internal(format!("add installations region failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "city") {
        conn.execute("ALTER TABLE installations ADD COLUMN city TEXT", [])
            .map_err(|e| AppError::Internal(format!("add installations city failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "latitude") {
        conn.execute("ALTER TABLE installations ADD COLUMN latitude REAL", [])
            .map_err(|e| AppError::Internal(format!("add installations latitude failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "longitude") {
        conn.execute("ALTER TABLE installations ADD COLUMN longitude REAL", [])
            .map_err(|e| AppError::Internal(format!("add installations longitude failed: {e}")))?;
    }
    if !columns
        .iter()
        .any(|name| name == "geo_candidate_country_code")
    {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN geo_candidate_country_code TEXT",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!(
                "add installations geo_candidate_country_code failed: {e}"
            ))
        })?;
    }
    if !columns.iter().any(|name| name == "geo_candidate_country") {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN geo_candidate_country TEXT",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!(
                "add installations geo_candidate_country failed: {e}"
            ))
        })?;
    }
    if !columns.iter().any(|name| name == "geo_candidate_region") {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN geo_candidate_region TEXT",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!(
                "add installations geo_candidate_region failed: {e}"
            ))
        })?;
    }
    if !columns.iter().any(|name| name == "geo_candidate_city") {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN geo_candidate_city TEXT",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!("add installations geo_candidate_city failed: {e}"))
        })?;
    }
    if !columns.iter().any(|name| name == "geo_candidate_latitude") {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN geo_candidate_latitude REAL",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!(
                "add installations geo_candidate_latitude failed: {e}"
            ))
        })?;
    }
    if !columns.iter().any(|name| name == "geo_candidate_longitude") {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN geo_candidate_longitude REAL",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!(
                "add installations geo_candidate_longitude failed: {e}"
            ))
        })?;
    }
    if !columns.iter().any(|name| name == "geo_candidate_hits") {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN geo_candidate_hits INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!("add installations geo_candidate_hits failed: {e}"))
        })?;
    }
    if !columns
        .iter()
        .any(|name| name == "geo_candidate_first_seen_at")
    {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN geo_candidate_first_seen_at TEXT",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!(
                "add installations geo_candidate_first_seen_at failed: {e}"
            ))
        })?;
    }
    if !columns.iter().any(|name| name == "geo_last_changed_at") {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN geo_last_changed_at TEXT",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!("add installations geo_last_changed_at failed: {e}"))
        })?;
    }
    let columns = conn
        .prepare("PRAGMA table_info(shares)")
        .and_then(|mut stmt| {
            let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .map_err(|e| AppError::Internal(format!("inspect shares schema failed: {e}")))?;
    if !columns.iter().any(|name| name == "subdomain") {
        conn.execute("ALTER TABLE shares ADD COLUMN subdomain TEXT", [])
            .map_err(|e| AppError::Internal(format!("add shares subdomain failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "description") {
        conn.execute("ALTER TABLE shares ADD COLUMN description TEXT", [])
            .map_err(|e| AppError::Internal(format!("add shares description failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "for_sale") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN for_sale TEXT NOT NULL DEFAULT 'No'",
            [],
        )
        .map_err(|e| AppError::Internal(format!("add shares for_sale failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "enabled_claude") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN enabled_claude INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|e| AppError::Internal(format!("add shares enabled_claude failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "enabled_codex") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN enabled_codex INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|e| AppError::Internal(format!("add shares enabled_codex failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "enabled_gemini") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN enabled_gemini INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|e| AppError::Internal(format!("add shares enabled_gemini failed: {e}")))?;
    }
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_shares_subdomain_unique ON shares(subdomain) WHERE subdomain IS NOT NULL",
        [],
    )
    .map_err(|e| AppError::Internal(format!("create subdomain unique index failed: {e}")))?;
    Ok(())
}

fn get_installation(
    conn: &Connection,
    installation_id: &str,
) -> Result<Option<Installation>, AppError> {
    conn.query_row(
        "SELECT id, public_key, platform, app_version, last_seen_ip, country_code, country, region, city, latitude, longitude,
                geo_candidate_country_code, geo_candidate_country, geo_candidate_region, geo_candidate_city,
                geo_candidate_latitude, geo_candidate_longitude, geo_candidate_hits, geo_candidate_first_seen_at,
                geo_last_changed_at, created_at, last_seen_at
         FROM installations WHERE id = ?1",
        params![installation_id],
        |row| {
            Ok(Installation {
                id: row.get(0)?,
                public_key: row.get(1)?,
                platform: row.get(2)?,
                app_version: row.get(3)?,
                last_seen_ip: row.get(4)?,
                country_code: row.get(5)?,
                country: row.get(6)?,
                region: row.get(7)?,
                city: row.get(8)?,
                latitude: row.get(9)?,
                longitude: row.get(10)?,
                geo_candidate_country_code: row.get(11)?,
                geo_candidate_country: row.get(12)?,
                geo_candidate_region: row.get(13)?,
                geo_candidate_city: row.get(14)?,
                geo_candidate_latitude: row.get(15)?,
                geo_candidate_longitude: row.get(16)?,
                geo_candidate_hits: row.get(17)?,
                geo_candidate_first_seen_at: row
                    .get::<_, Option<String>>(18)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
                geo_last_changed_at: row
                    .get::<_, Option<String>>(19)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
                created_at: parse_dt_sql(&row.get::<_, String>(20)?)?,
                last_seen_at: parse_dt_sql(&row.get::<_, String>(21)?)?,
            })
        },
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query installation failed: {e}")))
}

fn find_installation_id_by_public_key(
    conn: &Connection,
    public_key: &str,
) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT id
         FROM installations
         WHERE public_key = ?1
         ORDER BY last_seen_at DESC, created_at DESC
         LIMIT 1",
        params![public_key],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query installation by public key failed: {e}")))
}

fn get_lease_by_connection_id(
    conn: &Connection,
    connection_id: &str,
) -> Result<Option<TunnelLease>, AppError> {
    conn.query_row(
        "SELECT id, installation_id, connection_id, subdomain, tunnel_type, ssh_username,
                ssh_password, issued_at, expires_at, used_at, share_json
         FROM leases WHERE connection_id = ?1",
        params![connection_id],
        map_lease_row,
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query lease failed: {e}")))
}

fn list_installations(conn: &Connection) -> Result<Vec<Installation>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, public_key, platform, app_version, last_seen_ip, country_code, country, region, city, latitude, longitude,
                    geo_candidate_country_code, geo_candidate_country, geo_candidate_region, geo_candidate_city,
                    geo_candidate_latitude, geo_candidate_longitude, geo_candidate_hits, geo_candidate_first_seen_at,
                    geo_last_changed_at, created_at, last_seen_at
             FROM installations ORDER BY last_seen_at DESC",
        )
        .map_err(|e| AppError::Internal(format!("prepare installations failed: {e}")))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Installation {
                id: row.get(0)?,
                public_key: row.get(1)?,
                platform: row.get(2)?,
                app_version: row.get(3)?,
                last_seen_ip: row.get(4)?,
                country_code: row.get(5)?,
                country: row.get(6)?,
                region: row.get(7)?,
                city: row.get(8)?,
                latitude: row.get(9)?,
                longitude: row.get(10)?,
                geo_candidate_country_code: row.get(11)?,
                geo_candidate_country: row.get(12)?,
                geo_candidate_region: row.get(13)?,
                geo_candidate_city: row.get(14)?,
                geo_candidate_latitude: row.get(15)?,
                geo_candidate_longitude: row.get(16)?,
                geo_candidate_hits: row.get(17)?,
                geo_candidate_first_seen_at: row
                    .get::<_, Option<String>>(18)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
                geo_last_changed_at: row
                    .get::<_, Option<String>>(19)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
                created_at: parse_dt_sql(&row.get::<_, String>(20)?)?,
                last_seen_at: parse_dt_sql(&row.get::<_, String>(21)?)?,
            })
        })
        .map_err(|e| AppError::Internal(format!("query installations failed: {e}")))?;
    collect_rows(rows)
}

fn get_installation_geo_state(
    conn: &Connection,
    installation_id: &str,
) -> Result<Option<InstallationGeoState>, AppError> {
    conn.query_row(
        "SELECT last_seen_ip, country_code, latitude, longitude,
                geo_candidate_country_code, geo_candidate_country, geo_candidate_region, geo_candidate_city,
                geo_candidate_latitude, geo_candidate_longitude, geo_candidate_hits,
                geo_candidate_first_seen_at, geo_last_changed_at
         FROM installations WHERE id = ?1",
        params![installation_id],
        |row| {
            Ok(InstallationGeoState {
                last_seen_ip: row.get(0)?,
                country_code: row.get(1)?,
                latitude: row.get(2)?,
                longitude: row.get(3)?,
                geo_candidate_country_code: row.get(4)?,
                geo_candidate_latitude: row.get(8)?,
                geo_candidate_longitude: row.get(9)?,
                geo_candidate_hits: row.get(10)?,
                geo_candidate_first_seen_at: row
                    .get::<_, Option<String>>(11)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
                geo_last_changed_at: row
                    .get::<_, Option<String>>(12)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
            })
        },
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query installation geo state failed: {e}")))
}

fn touch_installation_presence(
    conn: &Connection,
    installation_id: &str,
    metadata: &ClientMetadata,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE installations
         SET last_seen_at = ?2,
             last_seen_ip = COALESCE(?3, last_seen_ip),
             country_code = COALESCE(?4, country_code)
         WHERE id = ?1",
        params![
            installation_id,
            now.to_rfc3339(),
            metadata.ip.as_deref(),
            metadata.country_code.as_deref(),
        ],
    )
    .map_err(|e| AppError::Internal(format!("update installation failed: {e}")))?;
    Ok(())
}

fn should_refresh_installation_geo(installation: &Installation, next_ip: Option<&str>) -> bool {
    let Some(next_ip) = next_ip.map(str::trim).filter(|v| !v.is_empty()) else {
        return false;
    };
    installation.last_seen_ip.as_deref() != Some(next_ip)
        || installation.latitude.is_none()
        || installation.longitude.is_none()
}

fn prefer_dashboard_share(candidate: &ShareView, existing: &ShareView) -> bool {
    let candidate_active = candidate.active_lease_count > 0 || candidate.share_status == "active";
    let existing_active = existing.active_lease_count > 0 || existing.share_status == "active";
    if candidate_active != existing_active {
        return candidate_active;
    }
    if candidate.created_at != existing.created_at {
        return candidate.created_at > existing.created_at;
    }
    candidate.share_id > existing.share_id
}

fn deduplicate_dashboard_installations(
    installations: Vec<Installation>,
    leases_by_installation: &HashMap<String, Vec<TunnelLease>>,
    active_share_subdomains_by_installation: &HashMap<String, HashSet<String>>,
) -> Vec<Installation> {
    let mut deduped = Vec::with_capacity(installations.len());
    let mut seen = HashMap::<String, usize>::new();

    for installation in installations {
        let key = installation.public_key.clone();
        match seen.entry(key) {
            Entry::Vacant(entry) => {
                entry.insert(deduped.len());
                deduped.push(installation);
            }
            Entry::Occupied(entry) => {
                let existing = &mut deduped[*entry.get()];
                if prefer_dashboard_installation(
                    &installation,
                    existing,
                    leases_by_installation,
                    active_share_subdomains_by_installation,
                ) {
                    *existing = installation;
                }
            }
        }
    }

    deduped.sort_by(|a, b| b.last_seen_at.cmp(&a.last_seen_at));
    deduped
}

fn prefer_dashboard_installation(
    candidate: &Installation,
    existing: &Installation,
    leases_by_installation: &HashMap<String, Vec<TunnelLease>>,
    active_share_subdomains_by_installation: &HashMap<String, HashSet<String>>,
) -> bool {
    let candidate_has_share = active_share_subdomains_by_installation
        .get(&candidate.id)
        .map(|subdomains| !subdomains.is_empty())
        .unwrap_or(false);
    let existing_has_share = active_share_subdomains_by_installation
        .get(&existing.id)
        .map(|subdomains| !subdomains.is_empty())
        .unwrap_or(false);
    if candidate_has_share != existing_has_share {
        return candidate_has_share;
    }

    let candidate_active_lease_count = leases_by_installation
        .get(&candidate.id)
        .map(|leases| leases.len())
        .unwrap_or(0);
    let existing_active_lease_count = leases_by_installation
        .get(&existing.id)
        .map(|leases| leases.len())
        .unwrap_or(0);
    if candidate_active_lease_count != existing_active_lease_count {
        return candidate_active_lease_count > existing_active_lease_count;
    }

    if candidate.last_seen_at != existing.last_seen_at {
        return candidate.last_seen_at > existing.last_seen_at;
    }

    candidate.created_at > existing.created_at
}

fn haversine_distance_km(
    lat1: Option<f64>,
    lon1: Option<f64>,
    lat2: Option<f64>,
    lon2: Option<f64>,
) -> Option<f64> {
    let (lat1, lon1, lat2, lon2) = (lat1?, lon1?, lat2?, lon2?);
    let to_rad = |deg: f64| deg.to_radians();
    let dlat = to_rad(lat2 - lat1);
    let dlon = to_rad(lon2 - lon1);
    let lat1 = to_rad(lat1);
    let lat2 = to_rad(lat2);
    let a = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    Some(6371.0 * c)
}

fn persist_candidate_geo(
    conn: &Connection,
    installation_id: &str,
    geo: &GeoLookupResult,
    hits: i64,
    first_seen_at: DateTime<Utc>,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE installations
         SET geo_candidate_country_code = ?2,
             geo_candidate_country = ?3,
             geo_candidate_region = ?4,
             geo_candidate_city = ?5,
             geo_candidate_latitude = ?6,
             geo_candidate_longitude = ?7,
             geo_candidate_hits = ?8,
             geo_candidate_first_seen_at = ?9
         WHERE id = ?1",
        params![
            installation_id,
            geo.country_code,
            geo.country,
            geo.region,
            geo.city,
            geo.latitude,
            geo.longitude,
            hits,
            first_seen_at.to_rfc3339(),
        ],
    )
    .map_err(|e| AppError::Internal(format!("update installation candidate geo failed: {e}")))?;
    Ok(())
}

fn persist_stable_geo(
    conn: &Connection,
    installation_id: &str,
    geo: &GeoLookupResult,
    changed_at: DateTime<Utc>,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE installations
         SET country_code = COALESCE(?2, country_code),
             country = COALESCE(?3, country),
             region = COALESCE(?4, region),
             city = COALESCE(?5, city),
             latitude = COALESCE(?6, latitude),
             longitude = COALESCE(?7, longitude),
             geo_candidate_country_code = NULL,
             geo_candidate_country = NULL,
             geo_candidate_region = NULL,
             geo_candidate_city = NULL,
             geo_candidate_latitude = NULL,
             geo_candidate_longitude = NULL,
             geo_candidate_hits = 0,
             geo_candidate_first_seen_at = NULL,
             geo_last_changed_at = ?8
         WHERE id = ?1",
        params![
            installation_id,
            geo.country_code,
            geo.country,
            geo.region,
            geo.city,
            geo.latitude,
            geo.longitude,
            changed_at.to_rfc3339(),
        ],
    )
    .map_err(|e| AppError::Internal(format!("update installation stable geo failed: {e}")))?;
    Ok(())
}

async fn lookup_ip_im_geo(ip: &str) -> Option<GeoLookupResult> {
    let url = format!("https://ip.im/{ip}");
    let client = reqwest::Client::builder()
        .user_agent("portr-rs/0.1")
        .timeout(StdDuration::from_secs(3))
        .build()
        .ok()?;
    let response = timeout(StdDuration::from_secs(4), client.get(url).send())
        .await
        .ok()?
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = response.text().await.ok()?;
    parse_ip_im_geo(&body)
}

fn parse_ip_im_geo(body: &str) -> Option<GeoLookupResult> {
    let mut result = GeoLookupResult {
        country_code: None,
        country: None,
        region: None,
        city: None,
        latitude: None,
        longitude: None,
    };

    for raw_line in body.lines() {
        let line = raw_line.trim();
        if let Some(value) = line.strip_prefix("Country:") {
            let value = value.trim();
            if value.len() == 2 && value.chars().all(|ch| ch.is_ascii_alphabetic()) {
                result.country_code = Some(value.to_ascii_uppercase());
            } else if !value.is_empty() {
                result.country = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Region:") {
            let value = value.trim();
            if !value.is_empty() {
                result.region = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("City:") {
            let value = value.trim();
            if !value.is_empty() {
                result.city = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Loc:") {
            let value = value.trim();
            if let Some((lat, lon)) = value.split_once(',') {
                result.latitude = lat.trim().parse().ok();
                result.longitude = lon.trim().parse().ok();
            }
        }
    }

    if result.latitude.is_none() || result.longitude.is_none() {
        return None;
    }
    Some(result)
}

fn list_leases(conn: &Connection) -> Result<Vec<TunnelLease>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, installation_id, connection_id, subdomain, tunnel_type, ssh_username,
                    ssh_password, issued_at, expires_at, used_at, share_json
             FROM leases ORDER BY issued_at DESC",
        )
        .map_err(|e| AppError::Internal(format!("prepare leases failed: {e}")))?;
    let rows = stmt
        .query_map([], map_lease_row)
        .map_err(|e| AppError::Internal(format!("query leases failed: {e}")))?;
    collect_rows(rows)
}

fn list_shares(conn: &Connection) -> Result<Vec<(String, ShareDescriptor, usize)>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT s.installation_id, s.share_id, s.share_name, s.description, s.for_sale, COALESCE(s.subdomain, '-'), s.share_token, s.app_type, s.provider_id,
                    s.enabled_claude, s.enabled_codex, s.enabled_gemini,
                    s.token_limit, s.tokens_used, s.requests_count, s.share_status, s.created_at, s.expires_at,
                    (
                        SELECT COUNT(*) FROM leases l
                        WHERE json_extract(l.share_json, '$.shareId') = s.share_id
                          AND l.expires_at > ?1
                    ) AS active_lease_count
             FROM shares s
             ORDER BY s.share_name ASC",
        )
        .map_err(|e| AppError::Internal(format!("prepare shares failed: {e}")))?;
    let rows = stmt
        .query_map(params![Utc::now().to_rfc3339()], |row| {
            Ok((
                row.get(0)?,
                ShareDescriptor {
                    share_id: row.get(1)?,
                    share_name: row.get(2)?,
                    description: row.get(3)?,
                    for_sale: row.get(4)?,
                    subdomain: row.get(5)?,
                    share_token: row.get(6)?,
                    app_type: row.get(7)?,
                    provider_id: row.get(8)?,
                    support: ShareSupport {
                        claude: row.get::<_, i64>(9)? != 0,
                        codex: row.get::<_, i64>(10)? != 0,
                        gemini: row.get::<_, i64>(11)? != 0,
                    },
                    token_limit: row.get(12)?,
                    tokens_used: row.get(13)?,
                    requests_count: row.get(14)?,
                    share_status: row.get(15)?,
                    created_at: row.get(16)?,
                    expires_at: row.get(17)?,
                },
                row.get::<_, i64>(18)? as usize,
            ))
        })
        .map_err(|e| AppError::Internal(format!("query shares failed: {e}")))?;
    collect_rows(rows)
}

fn normalize_share_description(description: Option<String>) -> Result<Option<String>, AppError> {
    let Some(description) = description else {
        return Ok(None);
    };
    let trimmed = description.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > 200 {
        return Err(AppError::BadRequest(
            "share description must be 200 characters or fewer".into(),
        ));
    }
    Ok(Some(trimmed.to_string()))
}

fn normalize_share_for_sale(value: &str) -> Result<String, AppError> {
    match value.trim() {
        "No" => Ok("No".to_string()),
        "Yes" => Ok("Yes".to_string()),
        _ => Err(AppError::BadRequest(
            "share for_sale must be Yes or No".into(),
        )),
    }
}

fn mask_share_token(token: &str) -> String {
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return "***".to_string();
    };
    let last = token.chars().last().unwrap_or(first);
    format!("{first}***{last}")
}

fn list_recent_share_request_logs(
    conn: &Connection,
    per_share_limit: usize,
) -> Result<Vec<ShareRequestLogEntry>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT request_id, share_id, share_name, provider_id, provider_name, app_type, model,
                    request_model, status_code, latency_ms, first_token_ms, input_tokens,
                    output_tokens, cache_read_tokens, cache_creation_tokens, is_streaming,
                    session_id, created_at
             FROM (
                 SELECT request_id, share_id, share_name, provider_id, provider_name, app_type, model,
                        request_model, status_code, latency_ms, first_token_ms, input_tokens,
                        output_tokens, cache_read_tokens, cache_creation_tokens, is_streaming,
                        session_id, created_at,
                        ROW_NUMBER() OVER (PARTITION BY share_id ORDER BY created_at DESC) AS row_num
                 FROM share_request_logs
             )
             WHERE row_num <= ?1
             ORDER BY created_at DESC",
        )
        .map_err(|e| AppError::Internal(format!("prepare recent share request logs failed: {e}")))?;
    let rows = stmt
        .query_map(params![per_share_limit as i64], |row| {
            Ok(ShareRequestLogEntry {
                request_id: row.get(0)?,
                share_id: row.get(1)?,
                share_name: row.get(2)?,
                provider_id: row.get(3)?,
                provider_name: row.get(4)?,
                app_type: row.get(5)?,
                model: row.get(6)?,
                request_model: row.get(7)?,
                status_code: row.get::<_, i64>(8)? as u16,
                latency_ms: row.get::<_, i64>(9)? as u64,
                first_token_ms: row.get::<_, Option<i64>>(10)?.map(|v| v as u64),
                input_tokens: row.get::<_, i64>(11)? as u32,
                output_tokens: row.get::<_, i64>(12)? as u32,
                cache_read_tokens: row.get::<_, i64>(13)? as u32,
                cache_creation_tokens: row.get::<_, i64>(14)? as u32,
                is_streaming: row.get::<_, i64>(15)? != 0,
                session_id: row.get(16)?,
                created_at: row.get(17)?,
            })
        })
        .map_err(|e| AppError::Internal(format!("query recent share request logs failed: {e}")))?;
    let logs = collect_rows(rows)?;
    Ok(deduplicate_recent_share_request_logs(logs))
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct RecentShareLogFingerprint {
    share_id: String,
    created_at: i64,
    model: String,
    request_model: String,
    status_code: u16,
    latency_ms: u64,
    first_token_ms: Option<u64>,
    input_tokens: u32,
    output_tokens: u32,
    cache_read_tokens: u32,
    cache_creation_tokens: u32,
    is_streaming: bool,
    session_id: Option<String>,
}

fn deduplicate_recent_share_request_logs(
    logs: Vec<ShareRequestLogEntry>,
) -> Vec<ShareRequestLogEntry> {
    let mut deduped = Vec::with_capacity(logs.len());
    let mut seen = HashMap::<RecentShareLogFingerprint, usize>::new();

    for log in logs {
        let fingerprint = RecentShareLogFingerprint {
            share_id: log.share_id.clone(),
            created_at: log.created_at,
            model: log.model.clone(),
            request_model: log.request_model.clone(),
            status_code: log.status_code,
            latency_ms: log.latency_ms,
            first_token_ms: log.first_token_ms,
            input_tokens: log.input_tokens,
            output_tokens: log.output_tokens,
            cache_read_tokens: log.cache_read_tokens,
            cache_creation_tokens: log.cache_creation_tokens,
            is_streaming: log.is_streaming,
            session_id: log.session_id.clone(),
        };

        match seen.entry(fingerprint) {
            Entry::Vacant(entry) => {
                entry.insert(deduped.len());
                deduped.push(log);
            }
            Entry::Occupied(entry) => {
                let existing = &mut deduped[*entry.get()];
                if prefer_share_request_log(&log, existing) {
                    *existing = log;
                }
            }
        }
    }

    deduped
}

fn prefer_share_request_log(
    candidate: &ShareRequestLogEntry,
    existing: &ShareRequestLogEntry,
) -> bool {
    let candidate_name = candidate.provider_name.trim();
    let existing_name = existing.provider_name.trim();
    let candidate_has_display_name =
        !candidate_name.is_empty() && candidate_name != candidate.provider_id;
    let existing_has_display_name =
        !existing_name.is_empty() && existing_name != existing.provider_id;
    if candidate_has_display_name != existing_has_display_name {
        return candidate_has_display_name;
    }

    let candidate_model_score = usize::from(!candidate.model.trim().is_empty())
        + usize::from(!candidate.request_model.trim().is_empty());
    let existing_model_score = usize::from(!existing.model.trim().is_empty())
        + usize::from(!existing.request_model.trim().is_empty());
    if candidate_model_score != existing_model_score {
        return candidate_model_score > existing_model_score;
    }

    candidate.request_id > existing.request_id
}

fn list_health_checks(
    conn: &Connection,
    minutes: usize,
) -> Result<HashMap<String, Vec<HealthCheckEntry>>, AppError> {
    let current_bucket = Utc::now().timestamp().div_euclid(60);
    let cutoff = (current_bucket - (minutes as i64 - 1)) * 60;
    let mut stmt = conn
        .prepare(
            "SELECT share_id, checked_at, is_healthy
             FROM share_health_checks
             WHERE checked_at >= ?1
             ORDER BY checked_at ASC",
        )
        .map_err(|e| AppError::Internal(format!("prepare health checks failed: {e}")))?;
    let rows = stmt
        .query_map(params![cutoff], |row| {
            Ok((
                row.get::<_, String>(0)?,
                HealthCheckEntry {
                    checked_at: row.get(1)?,
                    is_healthy: row.get::<_, i64>(2)? != 0,
                },
            ))
        })
        .map_err(|e| AppError::Internal(format!("query health checks failed: {e}")))?;
    let mut map: HashMap<String, Vec<HealthCheckEntry>> = HashMap::new();
    for row in rows {
        let (share_id, entry) =
            row.map_err(|e| AppError::Internal(format!("read health check row failed: {e}")))?;
        map.entry(share_id).or_default().push(entry);
    }
    Ok(map)
}

fn list_online_minutes_24h(conn: &Connection) -> Result<HashMap<String, usize>, AppError> {
    let cutoff = Utc::now().timestamp() - 24 * 60 * 60;
    let mut stmt = conn
        .prepare(
            "SELECT share_id, COUNT(DISTINCT checked_at / 60) AS online_minutes
             FROM share_health_checks
             WHERE checked_at >= ?1
             GROUP BY share_id",
        )
        .map_err(|e| AppError::Internal(format!("prepare online minutes failed: {e}")))?;
    let rows = stmt
        .query_map(params![cutoff], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })
        .map_err(|e| AppError::Internal(format!("query online minutes failed: {e}")))?;
    let mut map = HashMap::new();
    for row in rows {
        let (share_id, online_minutes) =
            row.map_err(|e| AppError::Internal(format!("read online minute row failed: {e}")))?;
        map.insert(share_id, online_minutes);
    }
    Ok(map)
}

fn map_lease_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TunnelLease> {
    let share_json: Option<String> = row.get(10)?;
    Ok(TunnelLease {
        id: row.get(0)?,
        installation_id: row.get(1)?,
        connection_id: row.get(2)?,
        subdomain: row.get(3)?,
        tunnel_type: row.get(4)?,
        ssh_username: row.get(5)?,
        ssh_password: row.get(6)?,
        issued_at: parse_dt_sql(&row.get::<_, String>(7)?)?,
        expires_at: parse_dt_sql(&row.get::<_, String>(8)?)?,
        used_at: row
            .get::<_, Option<String>>(9)?
            .map(|value| parse_dt_sql(&value))
            .transpose()?,
        share: share_json
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    10,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
    })
}

fn parse_dt_sql(value: &str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>, AppError> {
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Internal(format!("collect rows failed: {e}")))
}

fn normalize_subdomain(value: &str) -> Result<String, AppError> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() {
        return Err(AppError::BadRequest("subdomain is required".into()));
    }
    if value.len() < 3 || value.len() > 63 {
        return Err(AppError::BadRequest("invalid subdomain".into()));
    }
    if value.starts_with('-') || value.ends_with('-') {
        return Err(AppError::BadRequest("invalid subdomain".into()));
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(AppError::BadRequest("invalid subdomain".into()));
    }
    Ok(value)
}

fn ensure_subdomain_allowed(value: &str) -> Result<(), AppError> {
    const RESERVED: &[&str] = &["admin", "api", "www", "cdn-cgi"];
    if RESERVED.contains(&value) {
        return Err(AppError::Conflict("subdomain is reserved".into()));
    }
    Ok(())
}

fn get_share_owned_subdomain(
    conn: &Connection,
    installation_id: &str,
    share_id: &str,
) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT subdomain FROM shares WHERE installation_id = ?1 AND share_id = ?2",
        params![installation_id, share_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query owned subdomain failed: {e}")))
}

fn map_share_constraint_error(err: rusqlite::Error) -> AppError {
    let text = err.to_string();
    if text.contains("UNIQUE constraint failed: shares.subdomain")
        || text.contains("idx_shares_subdomain_unique")
    {
        AppError::Conflict("subdomain already claimed".into())
    } else {
        AppError::Internal(format!("upsert share failed: {text}"))
    }
}

fn verify_signature(public_key: &str, input: &IssueLeaseRequest) -> Result<(), AppError> {
    let key_bytes = base64::engine::general_purpose::STANDARD
        .decode(public_key)
        .map_err(|_| AppError::Unauthorized("invalid stored public key".into()))?;
    let key_array: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| AppError::Unauthorized("invalid public key length".into()))?;
    let verifying_key = VerifyingKey::from_bytes(&key_array)
        .map_err(|_| AppError::Unauthorized("invalid public key".into()))?;

    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(&input.signature)
        .map_err(|_| AppError::Unauthorized("invalid signature".into()))?;
    let sig_array: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| AppError::Unauthorized("invalid signature length".into()))?;
    let signature = Signature::from_bytes(&sig_array);

    let payload = format!(
        "{}\n{}\n{}\n{}\n{}",
        input.installation_id,
        input.requested_subdomain,
        input.tunnel_type,
        input.timestamp_ms,
        input.nonce
    );
    verifying_key
        .verify(payload.as_bytes(), &signature)
        .map_err(|_| AppError::Unauthorized("signature verification failed".into()))
}
