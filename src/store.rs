use std::collections::{HashMap, HashSet, hash_map::Entry};
use std::sync::Arc;
use std::time::Duration as StdDuration;

use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rand::distributions::{Alphanumeric, DistString};
use resend_rs::Resend;
use resend_rs::types::CreateEmailBaseOptions;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tokio::time::timeout;
use uuid::Uuid;

use crate::ServerGeo;
use crate::config::Config;
use crate::error::AppError;
use crate::models::{
    AuthSession, AuthUser, BindInstallationOwnerEmailRequest, BindInstallationOwnerEmailResponse,
    ClientMetadata, DashboardClientView, DashboardMap, DashboardMapPoint, DashboardPresenceRequest,
    DashboardResponse, DashboardStats, GetInstallationOwnerEmailQuery,
    GetInstallationOwnerEmailResponse, HealthCheckEntry, Installation, InstallationView,
    IssueLeaseRequest, IssueLeaseResponse, LatLonPoint, LeaseView, PublicMapClientPoint,
    PublicMapPointsResponse, RefreshSessionRequest, RegisterInstallationRequest,
    RegisterInstallationResponse, RequestEmailCodeRequest, RequestEmailCodeResponse,
    SessionStatusResponse, ShareAppRuntimes, ShareBatchSyncRequest, ShareClaimSubdomainRequest,
    ShareDeleteRequest, ShareDescriptor, ShareHeartbeatRequest, ShareRequestLogBatchSyncRequest,
    ShareRequestLogEntry, ShareRequestLogFetchResponse, ShareRuntimeSnapshotResponse, ShareSupport,
    ShareSyncRequest, ShareUpstreamProvider, ShareView, TunnelLease, VerifyEmailCodeRequest,
    VerifyEmailCodeResponse,
};
use crate::proxy::ProxyRegistry;

const SHARE_REQUEST_LOG_RECOVERY_LIMIT: usize = 10;
const PUBLIC_MAP_CLIENT_ACTIVE_WINDOW_MINUTES: i64 = 5;
const ONLINE_WINDOW_MINUTES: usize = 24 * 60;
const SIGNED_REQUEST_MAX_SKEW_MS: i64 = 60_000;
const NONCE_RETENTION_SECS: i64 = 10 * 60;
const AUTH_CODE_DIGITS: usize = 6;
const AUTH_PURPOSE_LOGIN: &str = "login";

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BindOwnerEmailSignaturePayload<'a> {
    email: &'a str,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verification_token: Option<&'a str>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerificationRedeemResponse {
    ok: bool,
    email: String,
    purpose: String,
    verified_at: i64,
}

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
    share_log_recovery_attempts: Arc<Mutex<HashSet<String>>>,
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

#[derive(Debug, Default)]
pub struct CleanupResult {
    pub deleted_leases: usize,
    pub deleted_shares: usize,
    pub deleted_installations: usize,
    pub removed_routes: usize,
}

impl CleanupResult {
    pub fn has_changes(&self) -> bool {
        self.deleted_leases > 0
            || self.deleted_shares > 0
            || self.deleted_installations > 0
            || self.removed_routes > 0
    }
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
            share_log_recovery_attempts: Arc::new(Mutex::new(HashSet::new())),
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
            owner_email: None,
            owner_verified_at: None,
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
                id, public_key, platform, app_version, owner_email, owner_verified_at, last_seen_ip, country_code, country, region,
                city, latitude, longitude, geo_candidate_country_code, geo_candidate_country,
                geo_candidate_region, geo_candidate_city, geo_candidate_latitude,
                geo_candidate_longitude, geo_candidate_hits, geo_candidate_first_seen_at,
                geo_last_changed_at, created_at, last_seen_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
            params![
                installation.id,
                installation.public_key,
                installation.platform,
                installation.app_version,
                installation.owner_email,
                installation
                    .owner_verified_at
                    .map(|value| value.to_rfc3339()),
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

    pub async fn bind_installation_owner_email(
        &self,
        config: &Config,
        input: BindInstallationOwnerEmailRequest,
    ) -> Result<BindInstallationOwnerEmailResponse, AppError> {
        let email = normalize_email(&input.email)?;
        let now = Utc::now();
        let payload = BindOwnerEmailSignaturePayload {
            email: &email,
            verification_token: input
                .verification_token
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty()),
        };

        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?
            .ok_or_else(|| AppError::Unauthorized("installation not found".into()))?;
        verify_signed_share_request(
            &conn,
            &installation.public_key,
            &input.installation_id,
            "bind_installation_owner_email",
            &payload,
            input.timestamp_ms,
            &input.nonce,
            &input.signature,
        )?;

        if let Some(existing_owner_email) = installation.owner_email.as_deref() {
            if existing_owner_email != email {
                return Err(AppError::Conflict(
                    "this installation is locked to a different owner email".into(),
                ));
            }
            return Ok(BindInstallationOwnerEmailResponse {
                ok: true,
                owner_email: email,
                already_bound: true,
            });
        }
        drop(conn);

        let verification_token = payload.verification_token.ok_or_else(|| {
            AppError::Unauthorized(
                "verification token is required to bind installation owner".into(),
            )
        })?;
        let redeemed = redeem_verification_token(config, verification_token).await?;
        if !redeemed.ok || redeemed.purpose != AUTH_PURPOSE_LOGIN || redeemed.email != email {
            return Err(AppError::Unauthorized(
                "verification token does not match requested owner email".into(),
            ));
        }
        let verified_at = DateTime::<Utc>::from_timestamp(redeemed.verified_at, 0).unwrap_or(now);

        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE installations
             SET owner_email = ?2, owner_verified_at = ?3
             WHERE id = ?1
               AND (owner_email IS NULL OR owner_email = '' OR owner_email = ?2)",
            params![input.installation_id, email, verified_at.to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("bind installation owner email failed: {e}")))?;

        Ok(BindInstallationOwnerEmailResponse {
            ok: true,
            owner_email: email,
            already_bound: false,
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

    pub async fn count_sent_emails_last_24h(&self) -> Result<usize, AppError> {
        let cutoff = (Utc::now() - Duration::hours(24)).to_rfc3339();
        let conn = self.conn.lock().await;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM email_send_logs
                 WHERE status = 'sent'
                   AND created_at >= ?1",
                params![cutoff],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Internal(format!("count sent emails failed: {e}")))?;
        Ok(count as usize)
    }

    pub async fn request_email_code(
        &self,
        config: &Config,
        resend: Option<&Resend>,
        input: RequestEmailCodeRequest,
        metadata: ClientMetadata,
    ) -> Result<RequestEmailCodeResponse, AppError> {
        let email = normalize_email(&input.email)?;
        let now = Utc::now();
        {
            let conn = self.conn.lock().await;
            let installation = get_installation(&conn, &input.installation_id)?
                .ok_or_else(|| AppError::Unauthorized("installation not found".into()))?;
            verify_signed_payload(
                &installation.public_key,
                &input.installation_id,
                "auth_request_code",
                &serde_json::json!({ "email": email, "purpose": AUTH_PURPOSE_LOGIN }),
                input.timestamp_ms,
                &input.nonce,
                &input.signature,
            )?;
            consume_request_nonce(
                &conn,
                &input.installation_id,
                "auth_request_code",
                &input.nonce,
                now,
            )?;
            enforce_auth_send_limits(
                &conn,
                config,
                &email,
                &input.installation_id,
                &metadata,
                now,
            )?;
        }

        let code = generate_numeric_code(AUTH_CODE_DIGITS);
        let resend = resend.ok_or_else(|| AppError::Internal("resend is not configured".into()))?;
        let provider_message_id =
            send_login_code_email(resend, config, &email, &code, config.auth_code_ttl_secs).await?;

        let expires_at = now + Duration::seconds(config.auth_code_ttl_secs);
        let resend_available_at = now + Duration::seconds(config.auth_code_cooldown_secs);
        let code_hash = hash_token(&format!("{email}:{code}"));
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE email_login_challenges
             SET consumed_at = ?2
             WHERE email_normalized = ?1
               AND purpose = ?3
               AND consumed_at IS NULL",
            params![email, now.to_rfc3339(), AUTH_PURPOSE_LOGIN],
        )
        .map_err(|e| AppError::Internal(format!("expire old auth challenges failed: {e}")))?;
        conn.execute(
            "INSERT INTO email_login_challenges (
                id, email_normalized, installation_id, purpose, code_hash, expires_at,
                consumed_at, attempt_count, resend_available_at, created_ip, created_user_agent, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, 0, ?7, ?8, NULL, ?9)",
            params![
                Uuid::new_v4().to_string(),
                email,
                input.installation_id,
                AUTH_PURPOSE_LOGIN,
                code_hash,
                expires_at.to_rfc3339(),
                resend_available_at.to_rfc3339(),
                metadata.ip,
                now.to_rfc3339(),
            ],
        )
        .map_err(|e| AppError::Internal(format!("insert auth challenge failed: {e}")))?;
        conn.execute(
            "INSERT INTO email_send_logs (
                id, email_type, to_email, provider_message_id, status, error_message, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6)",
            params![
                Uuid::new_v4().to_string(),
                "login_code",
                email,
                provider_message_id,
                "sent",
                now.to_rfc3339(),
            ],
        )
        .map_err(|e| AppError::Internal(format!("insert email send log failed: {e}")))?;

        Ok(RequestEmailCodeResponse {
            ok: true,
            cooldown_secs: config.auth_code_cooldown_secs,
            masked_destination: mask_email(&email),
        })
    }

    pub async fn verify_email_code(
        &self,
        config: &Config,
        input: VerifyEmailCodeRequest,
    ) -> Result<VerifyEmailCodeResponse, AppError> {
        let email = normalize_email(&input.email)?;
        let code = input.code.trim();
        if code.len() != AUTH_CODE_DIGITS || !code.chars().all(|ch| ch.is_ascii_digit()) {
            return Err(AppError::Unauthorized("invalid verification code".into()));
        }

        let now = Utc::now();
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?
            .ok_or_else(|| AppError::Unauthorized("installation not found".into()))?;
        let locked_owner_email = get_installation_owner_email(&conn, &installation.id)?;
        if let Some(ref owner_email) = locked_owner_email {
            if owner_email != &email {
                return Err(AppError::Conflict(
                    "this installation is locked to a different owner email".into(),
                ));
            }
        }

        let challenge = get_latest_active_email_challenge(
            &conn,
            &email,
            &input.installation_id,
            AUTH_PURPOSE_LOGIN,
            now,
        )?
        .ok_or_else(|| AppError::Unauthorized("verification code expired or not found".into()))?;

        if challenge.attempt_count >= config.auth_max_verify_attempts {
            return Err(AppError::TooManyRequests(
                "too many invalid verification attempts".into(),
            ));
        }

        let expected_hash = hash_token(&format!("{email}:{code}"));
        if expected_hash != challenge.code_hash {
            conn.execute(
                "UPDATE email_login_challenges
                 SET attempt_count = attempt_count + 1
                 WHERE id = ?1",
                params![challenge.id],
            )
            .map_err(|e| AppError::Internal(format!("update auth attempts failed: {e}")))?;
            return Err(AppError::Unauthorized("invalid verification code".into()));
        }

        conn.execute(
            "UPDATE email_login_challenges SET consumed_at = ?2 WHERE id = ?1",
            params![challenge.id, now.to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("consume auth challenge failed: {e}")))?;

        let user = upsert_user_by_email(&conn, &email, now)?;
        let access_token = generate_secret(48);
        let refresh_token = generate_secret(64);
        let access_expires_at = now + Duration::seconds(config.auth_session_ttl_secs);
        let refresh_expires_at = now + Duration::seconds(config.auth_refresh_ttl_secs);
        let session = AuthSession {
            session_id: Uuid::new_v4().to_string(),
            user_id: user.id.clone(),
            email: user.email.clone(),
            installation_id: installation.id.clone(),
            access_token_hash: hash_token(&access_token),
            refresh_token_hash: hash_token(&refresh_token),
            access_expires_at,
            refresh_expires_at,
            created_at: now,
            last_used_at: now,
        };
        persist_session(&conn, &session)?;

        Ok(VerifyEmailCodeResponse {
            user,
            access_token,
            refresh_token,
            expires_at: access_expires_at,
            refresh_expires_at,
        })
    }

    pub async fn refresh_session(
        &self,
        config: &Config,
        input: RefreshSessionRequest,
    ) -> Result<VerifyEmailCodeResponse, AppError> {
        let now = Utc::now();
        let refresh_hash = hash_token(input.refresh_token.trim());
        let conn = self.conn.lock().await;
        let current = get_session_by_refresh_hash(&conn, &refresh_hash)?
            .ok_or_else(|| AppError::Unauthorized("refresh session not found".into()))?;
        if current.refresh_expires_at < now {
            return Err(AppError::Unauthorized("refresh session expired".into()));
        }
        if current.installation_id != input.installation_id {
            return Err(AppError::Unauthorized(
                "refresh session installation mismatch".into(),
            ));
        }

        let user = get_user_by_id(&conn, &current.user_id)?
            .ok_or_else(|| AppError::Unauthorized("user not found".into()))?;
        let access_token = generate_secret(48);
        let refresh_token = generate_secret(64);
        let access_expires_at = now + Duration::seconds(config.auth_session_ttl_secs);
        let refresh_expires_at = now + Duration::seconds(config.auth_refresh_ttl_secs);
        conn.execute(
            "UPDATE user_sessions
             SET access_token_hash = ?2,
                 refresh_token_hash = ?3,
                 access_expires_at = ?4,
                 refresh_expires_at = ?5,
                 last_used_at = ?6,
                 revoked_at = NULL
             WHERE id = ?1",
            params![
                current.session_id,
                hash_token(&access_token),
                hash_token(&refresh_token),
                access_expires_at.to_rfc3339(),
                refresh_expires_at.to_rfc3339(),
                now.to_rfc3339(),
            ],
        )
        .map_err(|e| AppError::Internal(format!("refresh session failed: {e}")))?;

        Ok(VerifyEmailCodeResponse {
            user,
            access_token,
            refresh_token,
            expires_at: access_expires_at,
            refresh_expires_at,
        })
    }

    pub async fn session_status(
        &self,
        access_token: Option<&str>,
        installation_id: Option<&str>,
    ) -> Result<SessionStatusResponse, AppError> {
        let owner_email = if let Some(installation_id) = installation_id {
            let conn = self.conn.lock().await;
            get_installation_owner_email(&conn, installation_id)?
        } else {
            None
        };

        let Some(access_token) = access_token.map(str::trim).filter(|v| !v.is_empty()) else {
            return Ok(SessionStatusResponse {
                authenticated: false,
                user: None,
                expires_at: None,
                installation_owner_email: owner_email,
            });
        };

        let session = self
            .resolve_session_by_access_token(access_token)
            .await?
            .ok_or_else(|| AppError::Unauthorized("session not found".into()))?;
        Ok(SessionStatusResponse {
            authenticated: true,
            user: Some(AuthUser {
                id: session.user_id,
                email: session.email,
            }),
            expires_at: Some(session.access_expires_at),
            installation_owner_email: owner_email,
        })
    }

    pub async fn get_installation_owner_email_status(
        &self,
        query: GetInstallationOwnerEmailQuery,
    ) -> Result<GetInstallationOwnerEmailResponse, AppError> {
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &query.installation_id)?
            .ok_or_else(|| AppError::Unauthorized("installation not found".into()))?;
        verify_signed_share_request(
            &conn,
            &installation.public_key,
            &query.installation_id,
            "get_installation_owner_email",
            &serde_json::json!({}),
            query.timestamp_ms,
            &query.nonce,
            &query.signature,
        )?;
        Ok(GetInstallationOwnerEmailResponse {
            ok: true,
            owner_email: installation.owner_email,
        })
    }

    pub async fn resolve_session_by_access_token(
        &self,
        access_token: &str,
    ) -> Result<Option<AuthSession>, AppError> {
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let Some(session) = get_session_by_access_hash(&conn, &hash_token(access_token))? else {
            return Ok(None);
        };
        if session.access_expires_at < now {
            return Ok(None);
        }
        conn.execute(
            "UPDATE user_sessions SET last_used_at = ?2 WHERE id = ?1",
            params![session.session_id, now.to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("touch session failed: {e}")))?;
        Ok(Some(session))
    }

    pub async fn issue_lease(
        &self,
        config: &Config,
        proxy: &ProxyRegistry,
        input: IssueLeaseRequest,
        metadata: ClientMetadata,
        _current_user_email: Option<&str>,
    ) -> Result<IssueLeaseResponse, AppError> {
        let now = Utc::now();
        let skew = (now.timestamp_millis() - input.timestamp_ms).abs();
        if skew > SIGNED_REQUEST_MAX_SKEW_MS {
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

        let normalized_share = if let Some(mut share) = input.share.clone() {
            let existing_owner_email = {
                let conn = self.conn.lock().await;
                get_share_owner_email(&conn, &share.share_id)?
            };
            let bound_owner_email = {
                let conn = self.conn.lock().await;
                require_installation_owner_email(&conn, &input.installation_id)?
            };
            enforce_share_owner(
                &mut share,
                existing_owner_email.as_deref(),
                &bound_owner_email,
            )?;
            Some(share)
        } else {
            None
        };

        if let Some(share) = normalized_share.clone() {
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
            share: normalized_share,
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
            ssh_host_fingerprint: None,
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
        _current_user_email: &str,
    ) -> Result<(), AppError> {
        {
            let conn = self.conn.lock().await;
            let installation = get_installation(&conn, &input.installation_id)?;
            let Some(installation) = installation else {
                return Err(AppError::Unauthorized("installation not found".into()));
            };
            verify_signed_share_request(
                &conn,
                &installation.public_key,
                &input.installation_id,
                "share_sync",
                &input.share,
                input.timestamp_ms,
                &input.nonce,
                &input.signature,
            )?;
            let should_refresh_geo =
                should_refresh_installation_geo(&installation, metadata.ip.as_deref());
            touch_installation_presence(&conn, &input.installation_id, &metadata, Utc::now())?;
            drop(conn);
            if should_refresh_geo {
                self.refresh_installation_geo(&input.installation_id, &metadata.ip, false)
                    .await?;
            }
        }
        let existing_owner_email = {
            let conn = self.conn.lock().await;
            get_share_owner_email(&conn, &input.share.share_id)?
        };
        let bound_owner_email = {
            let conn = self.conn.lock().await;
            require_installation_owner_email(&conn, &input.installation_id)?
        };
        let mut share = input.share;
        enforce_share_owner(
            &mut share,
            existing_owner_email.as_deref(),
            &bound_owner_email,
        )?;
        self.upsert_share(&input.installation_id, share).await
    }

    pub async fn claim_share_subdomain(
        &self,
        input: ShareClaimSubdomainRequest,
        metadata: ClientMetadata,
        _current_user_email: &str,
    ) -> Result<(), AppError> {
        let subdomain = normalize_subdomain(&input.share.subdomain)?;
        ensure_subdomain_allowed(&subdomain)?;
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?;
        let Some(installation) = installation else {
            return Err(AppError::Unauthorized("installation not found".into()));
        };
        verify_signed_share_request(
            &conn,
            &installation.public_key,
            &input.installation_id,
            "share_claim_subdomain",
            &input.share,
            input.timestamp_ms,
            &input.nonce,
            &input.signature,
        )?;
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
        let existing_owner_email = get_share_owner_email(&tx, &share.share_id)?;
        let bound_owner_email = require_installation_owner_email(&tx, &input.installation_id)?;
        enforce_share_owner(
            &mut share,
            existing_owner_email.as_deref(),
            &bound_owner_email,
        )?;
        share.subdomain = subdomain;
        release_reclaimable_subdomain_claim(
            &tx,
            &input.installation_id,
            &share.share_id,
            share.owner_email.as_deref(),
            &share.subdomain,
        )?;
        upsert_share_tx(&tx, &input.installation_id, share)?;
        tx.commit().map_err(map_share_constraint_error)?;
        Ok(())
    }

    pub async fn delete_share(
        &self,
        input: ShareDeleteRequest,
        _current_user_email: &str,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?;
        let Some(installation) = installation else {
            return Err(AppError::Unauthorized("installation not found".into()));
        };
        let delete_payload = serde_json::json!({ "shareId": &input.share_id });
        verify_signed_share_request(
            &conn,
            &installation.public_key,
            &input.installation_id,
            "share_delete",
            &delete_payload,
            input.timestamp_ms,
            &input.nonce,
            &input.signature,
        )?;
        let owner_email = get_share_owner_email(&conn, &input.share_id)?;
        let bound_owner_email = require_installation_owner_email(&conn, &input.installation_id)?;
        if owner_email.as_deref() != Some(bound_owner_email.as_str()) {
            return Err(AppError::Unauthorized(
                "only share owner can delete share".into(),
            ));
        }
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
        _current_user_email: &str,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?;
        let Some(installation) = installation else {
            return Err(AppError::Unauthorized("installation not found".into()));
        };
        verify_signed_share_request(
            &conn,
            &installation.public_key,
            &input.installation_id,
            "share_batch_sync",
            &input.ops,
            input.timestamp_ms,
            &input.nonce,
            &input.signature,
        )?;
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
        let bound_owner_email = require_installation_owner_email(&tx, &input.installation_id)?;
        for op in input.ops {
            match op.kind.as_str() {
                "upsert" => {
                    let mut share = op.share.ok_or_else(|| {
                        AppError::BadRequest("share is required for upsert".into())
                    })?;
                    let existing_owner_email = get_share_owner_email(&tx, &share.share_id)?;
                    enforce_share_owner(
                        &mut share,
                        existing_owner_email.as_deref(),
                        &bound_owner_email,
                    )?;
                    upsert_share_tx(&tx, &input.installation_id, share)?;
                }
                "delete" => {
                    let share_id = op.share_id.ok_or_else(|| {
                        AppError::BadRequest("shareId is required for delete".into())
                    })?;
                    let owner_email = get_share_owner_email(&tx, &share_id)?;
                    if owner_email.as_deref() != Some(bound_owner_email.as_str()) {
                        return Err(AppError::Unauthorized(
                            "only share owner can delete share".into(),
                        ));
                    }
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
        _current_user_email: &str,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?;
        let Some(installation) = installation else {
            return Err(AppError::Unauthorized("installation not found".into()));
        };
        verify_signed_share_request(
            &conn,
            &installation.public_key,
            &input.installation_id,
            "share_request_logs_batch_sync",
            &input.logs,
            input.timestamp_ms,
            &input.nonce,
            &input.signature,
        )?;
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
        let bound_owner_email = require_installation_owner_email(&tx, &input.installation_id)?;
        for log in input.logs {
            let owner_email = get_share_owner_email(&tx, &log.share_id)?;
            if owner_email.as_deref() != Some(bound_owner_email.as_str()) {
                return Err(AppError::Unauthorized(
                    "only share owner can sync request logs".into(),
                ));
            }
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
        viewer_email: Option<&str>,
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
            if share.share_status == "active" && active_subdomains.contains(&share.subdomain) {
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
            if is_active {
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
            }
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
                let active_lease_count = usize::from(
                    share.share_status == "active" && active_subdomains.contains(&share.subdomain),
                );
                let recent_requests = logs_by_share
                    .get(&share.share_id)
                    .cloned()
                    .unwrap_or_default();
                let health_checks = health_by_share
                    .get(&share.share_id)
                    .cloned()
                    .unwrap_or_default();
                let online_minutes_24h = online_by_share.get(&share.share_id).copied().unwrap_or(0);
                let online_rate_24h =
                    ((online_minutes_24h as f64 / ONLINE_WINDOW_MINUTES as f64) * 100.0).min(100.0);
                let can_view_secret = share_visible_to_email(&share, viewer_email);
                let can_manage = can_manage_share(&share, viewer_email);
                ShareView {
                    share_id: share.share_id,
                    share_name: share.share_name,
                    owner_email: share.owner_email,
                    shared_with_emails: if can_manage {
                        share.shared_with_emails
                    } else {
                        Vec::new()
                    },
                    description: share.description,
                    for_sale: share.for_sale,
                    subdomain: share.subdomain,
                    share_token: if can_view_secret {
                        share.share_token
                    } else {
                        mask_share_token(&share.share_token)
                    },
                    app_type: share.app_type,
                    can_view_secret,
                    can_manage,
                    provider_id: share.provider_id,
                    token_limit: share.token_limit,
                    parallel_limit: share.parallel_limit,
                    tokens_used: share.tokens_used,
                    requests_count: share.requests_count,
                    share_status: share.share_status,
                    created_at: share.created_at,
                    expires_at: share.expires_at,
                    support: share.support,
                    upstream_provider: share.upstream_provider,
                    app_runtimes: share.app_runtimes,
                    installation_id,
                    active_lease_count,
                    online_minutes_24h,
                    online_rate_24h,
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
            .filter(|client| client.share.is_some())
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
        let missing_shares = {
            let mut attempted = self.share_log_recovery_attempts.lock().await;
            missing_shares
                .into_iter()
                .filter(|(_, share_id, _)| {
                    if attempted.contains(share_id) {
                        return false;
                    }
                    attempted.insert(share_id.clone());
                    true
                })
                .collect::<Vec<_>>()
        };

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

    pub async fn cleanup_expired_data(
        &self,
        config: &Config,
        proxy: &ProxyRegistry,
    ) -> Result<CleanupResult, AppError> {
        let cutoff = (Utc::now() - Duration::seconds(config.lease_retention_secs)).to_rfc3339();
        let stale_cutoff = (Utc::now() - Duration::seconds(config.client_stale_secs)).to_rfc3339();
        let (mut result, stale_subdomains) = {
            let conn = self.conn.lock().await;
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| AppError::Internal(format!("begin cleanup tx failed: {e}")))?;

            let stale_subdomains = {
                let mut stmt = tx
                    .prepare(
                        "SELECT DISTINCT subdomain
                         FROM shares
                         WHERE installation_id IN (
                             SELECT id FROM installations WHERE last_seen_at < ?1
                         )
                           AND subdomain IS NOT NULL
                           AND subdomain != ''
                           AND subdomain != '-'",
                    )
                    .map_err(|e| AppError::Internal(format!("prepare stale routes failed: {e}")))?;
                let rows = stmt
                    .query_map(params![stale_cutoff], |row| row.get::<_, String>(0))
                    .map_err(|e| AppError::Internal(format!("query stale routes failed: {e}")))?;
                collect_rows(rows)?
            };

            let deleted_leases = tx
                .execute(
                    "DELETE FROM leases
                     WHERE expires_at < ?1
                       AND (used_at IS NULL OR used_at < ?1)",
                    params![cutoff],
                )
                .map_err(|e| AppError::Internal(format!("delete expired leases failed: {e}")))?
                as usize;

            tx.execute(
                "DELETE FROM share_health_checks
                     WHERE share_id IN (
                         SELECT share_id
                         FROM shares
                         WHERE installation_id IN (
                             SELECT id FROM installations WHERE last_seen_at < ?1
                         )
                     )",
                params![stale_cutoff],
            )
            .map_err(|e| AppError::Internal(format!("delete stale share health failed: {e}")))?;

            let deleted_stale_shares = tx
                .execute(
                    "DELETE FROM shares
                     WHERE installation_id IN (
                         SELECT id FROM installations WHERE last_seen_at < ?1
                     )",
                    params![stale_cutoff],
                )
                .map_err(|e| {
                    AppError::Internal(format!("delete stale client shares failed: {e}"))
                })? as usize;

            let deleted_stale_leases = tx
                .execute(
                    "DELETE FROM leases
                     WHERE installation_id IN (
                         SELECT id FROM installations WHERE last_seen_at < ?1
                     )",
                    params![stale_cutoff],
                )
                .map_err(|e| {
                    AppError::Internal(format!("delete stale client leases failed: {e}"))
                })? as usize;

            let deleted_installations = tx
                .execute(
                    "DELETE FROM installations WHERE last_seen_at < ?1",
                    params![stale_cutoff],
                )
                .map_err(|e| {
                    AppError::Internal(format!("delete stale installations failed: {e}"))
                })? as usize;

            let deleted_old_shares = tx
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
                .map_err(|e| {
                    AppError::Internal(format!("delete stale request logs failed: {e}"))
                })?;

            tx.execute(
                "DELETE FROM request_nonces
                 WHERE created_at < ?1",
                params![(Utc::now() - Duration::seconds(NONCE_RETENTION_SECS)).to_rfc3339()],
            )
            .map_err(|e| AppError::Internal(format!("delete stale request nonces failed: {e}")))?;
            tx.execute(
                "DELETE FROM email_login_challenges
                 WHERE expires_at < ?1 OR consumed_at IS NOT NULL",
                params![cutoff],
            )
            .map_err(|e| AppError::Internal(format!("delete stale auth challenges failed: {e}")))?;
            tx.execute(
                "DELETE FROM user_sessions
                 WHERE refresh_expires_at < ?1 OR revoked_at IS NOT NULL",
                params![cutoff],
            )
            .map_err(|e| AppError::Internal(format!("delete stale user sessions failed: {e}")))?;

            tx.commit()
                .map_err(|e| AppError::Internal(format!("commit cleanup tx failed: {e}")))?;

            (
                CleanupResult {
                    deleted_leases: deleted_leases + deleted_stale_leases,
                    deleted_shares: deleted_stale_shares + deleted_old_shares,
                    deleted_installations,
                    removed_routes: 0,
                },
                stale_subdomains,
            )
        };

        let mut removed_routes = 0;
        for subdomain in stale_subdomains {
            proxy.remove_route(&subdomain).await;
            removed_routes += 1;
        }
        result.removed_routes = removed_routes;

        Ok(result)
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
                   AND share_status = 'active'
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
                "SELECT DISTINCT i.id, i.latitude, i.longitude, i.country_code
                 FROM installations i
                 INNER JOIN shares s ON s.installation_id = i.id
                 WHERE i.last_seen_at >= ?1
                   AND s.share_status = 'active'
                 ORDER BY i.last_seen_at DESC",
            )
            .map_err(|e| AppError::Internal(format!("prepare public map clients failed: {e}")))?;
        let rows = stmt
            .query_map(params![active_cutoff], |row| {
                let lat = row.get::<_, Option<f64>>(1)?;
                let lon = row.get::<_, Option<f64>>(2)?;
                let country_code = row.get::<_, Option<String>>(3)?;
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
        let mut grouped_clients = HashMap::<String, PublicMapClientPoint>::new();
        let mut client_count = 0usize;
        for point in collect_rows(rows)?.into_iter().flatten() {
            client_count += 1;
            let key = format!("{:.6},{:.6}", point.lat, point.lon);
            grouped_clients
                .entry(key)
                .and_modify(|existing| existing.count += 1)
                .or_insert(PublicMapClientPoint {
                    lat: point.lat,
                    lon: point.lon,
                    count: 1,
                });
        }
        let mut clients = grouped_clients.into_values().collect::<Vec<_>>();
        clients.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.lat.total_cmp(&b.lat))
                .then_with(|| a.lon.total_cmp(&b.lon))
        });

        Ok(PublicMapPointsResponse {
            server: server_geo
                .lat
                .zip(server_geo.lon)
                .map(|(lat, lon)| LatLonPoint { lat, lon }),
            client_count,
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

    pub async fn record_share_runtime_snapshot(
        &self,
        snapshot: ShareRuntimeSnapshotResponse,
    ) -> Result<(), AppError> {
        let app_runtimes_json = serde_json::to_string(&snapshot.app_runtimes)
            .map_err(|e| AppError::Internal(format!("serialize app runtimes failed: {e}")))?;
        let refreshed_at = DateTime::<Utc>::from_timestamp(snapshot.queried_at, 0)
            .unwrap_or_else(Utc::now)
            .to_rfc3339();

        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE shares
             SET enabled_claude = ?2,
                 enabled_codex = ?3,
                 enabled_gemini = ?4,
                 app_runtimes_json = ?5,
                 runtime_refreshed_at = ?6
             WHERE share_id = ?1",
            params![
                snapshot.share_id,
                i64::from(snapshot.support.claude as u8),
                i64::from(snapshot.support.codex as u8),
                i64::from(snapshot.support.gemini as u8),
                app_runtimes_json,
                refreshed_at,
            ],
        )
        .map_err(|e| AppError::Internal(format!("update share runtime snapshot failed: {e}")))?;
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

pub async fn fetch_share_runtime_snapshot_from_route(
    config: &Config,
    client: &reqwest::Client,
    subdomain: &str,
) -> Result<ShareRuntimeSnapshotResponse, AppError> {
    let url = format!("{}/_portr/share-runtime", config.tunnel_url(subdomain));
    let response = client
        .get(&url)
        .header("X-Portr-Probe", "1")
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("fetch share runtime failed: {e}")))?;

    if !response.status().is_success() {
        return Err(AppError::Internal(format!(
            "fetch share runtime failed with status {}",
            response.status()
        )));
    }

    response
        .json::<ShareRuntimeSnapshotResponse>()
        .await
        .map_err(|e| AppError::Internal(format!("decode share runtime failed: {e}")))
}

fn upsert_share_tx(
    conn: &Connection,
    installation_id: &str,
    share: ShareDescriptor,
) -> Result<(), AppError> {
    let description = normalize_share_description(share.description.clone())?;
    let for_sale = normalize_share_for_sale(&share.for_sale)?;
    let upstream_provider_json = share
        .upstream_provider
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| AppError::Internal(format!("serialize upstream provider failed: {e}")))?;
    let shared_with_emails_json = serde_json::to_string(&share.shared_with_emails)
        .map_err(|e| AppError::Internal(format!("serialize shared_with_emails failed: {e}")))?;
    conn.execute(
        "INSERT INTO shares (
            share_id, installation_id, share_name, owner_email, shared_with_emails_json, description, for_sale, subdomain, share_token, app_type, provider_id,
            enabled_claude, enabled_codex, enabled_gemini,
            token_limit, parallel_limit, tokens_used, requests_count, share_status, created_at, expires_at, upstream_provider_json, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)
        ON CONFLICT(share_id) DO UPDATE SET
            installation_id = excluded.installation_id,
            share_name = excluded.share_name,
            owner_email = excluded.owner_email,
            shared_with_emails_json = excluded.shared_with_emails_json,
            description = excluded.description,
            for_sale = excluded.for_sale,
            subdomain = excluded.subdomain,
            share_token = excluded.share_token,
            app_type = excluded.app_type,
            provider_id = excluded.provider_id,
            enabled_claude = shares.enabled_claude,
            enabled_codex = shares.enabled_codex,
            enabled_gemini = shares.enabled_gemini,
            token_limit = excluded.token_limit,
            parallel_limit = excluded.parallel_limit,
            tokens_used = excluded.tokens_used,
            requests_count = excluded.requests_count,
            share_status = excluded.share_status,
            created_at = excluded.created_at,
            expires_at = excluded.expires_at,
            upstream_provider_json = shares.upstream_provider_json,
            app_runtimes_json = shares.app_runtimes_json,
            runtime_refreshed_at = shares.runtime_refreshed_at,
            updated_at = excluded.updated_at",
        params![
            share.share_id,
            installation_id,
            share.share_name,
            share.owner_email,
            shared_with_emails_json,
            description,
            for_sale,
            share.subdomain,
            share.share_token,
            share.app_type,
            share.provider_id,
            i64::from(share.support.claude as u8),
            i64::from(share.support.codex as u8),
            i64::from(share.support.gemini as u8),
            share.token_limit,
            share.parallel_limit,
            share.tokens_used,
            share.requests_count,
            share.share_status,
            share.created_at,
            share.expires_at,
            upstream_provider_json,
            Utc::now().to_rfc3339(),
        ],
    )
    .map_err(map_share_constraint_error)?;
    Ok(())
}

fn upsert_share_request_log_tx(
    conn: &Connection,
    installation_id: &str,
    log: ShareRequestLogEntry,
) -> Result<(), AppError> {
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
            owner_email TEXT,
            owner_verified_at TEXT,
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
            owner_email TEXT,
            shared_with_emails_json TEXT NOT NULL DEFAULT '[]',
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
            parallel_limit INTEGER NOT NULL DEFAULT 3,
            tokens_used INTEGER NOT NULL,
            requests_count INTEGER NOT NULL,
            share_status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            upstream_provider_json TEXT,
            app_runtimes_json TEXT,
            runtime_refreshed_at TEXT,
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

        CREATE TABLE IF NOT EXISTS email_send_logs (
            id TEXT PRIMARY KEY,
            email_type TEXT NOT NULL,
            to_email TEXT NOT NULL,
            provider_message_id TEXT,
            status TEXT NOT NULL,
            error_message TEXT,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS request_nonces (
            installation_id TEXT NOT NULL,
            action TEXT NOT NULL,
            nonce TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (installation_id, action, nonce)
        );

        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            email_normalized TEXT NOT NULL UNIQUE,
            status TEXT NOT NULL DEFAULT 'active',
            created_at TEXT NOT NULL,
            last_login_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS email_login_challenges (
            id TEXT PRIMARY KEY,
            email_normalized TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            purpose TEXT NOT NULL,
            code_hash TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            consumed_at TEXT,
            attempt_count INTEGER NOT NULL DEFAULT 0,
            resend_available_at TEXT NOT NULL,
            created_ip TEXT,
            created_user_agent TEXT,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS user_sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            access_token_hash TEXT NOT NULL UNIQUE,
            refresh_token_hash TEXT NOT NULL UNIQUE,
            access_expires_at TEXT NOT NULL,
            refresh_expires_at TEXT NOT NULL,
            revoked_at TEXT,
            created_at TEXT NOT NULL,
            last_used_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_leases_installation_id ON leases(installation_id);
        CREATE INDEX IF NOT EXISTS idx_leases_subdomain ON leases(subdomain);
        CREATE INDEX IF NOT EXISTS idx_shares_installation_id ON shares(installation_id);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_shares_subdomain_unique ON shares(subdomain) WHERE subdomain IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_share_request_logs_share_id ON share_request_logs(share_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_share_health_checks ON share_health_checks(share_id, checked_at DESC);
        CREATE INDEX IF NOT EXISTS idx_dashboard_presence_last_seen ON dashboard_presence(last_seen_at DESC);
        CREATE INDEX IF NOT EXISTS idx_email_send_logs_created_at ON email_send_logs(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_request_nonces_created_at ON request_nonces(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_auth_challenges_email ON email_login_challenges(email_normalized, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_auth_challenges_installation ON email_login_challenges(installation_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_user_sessions_user_id ON user_sessions(user_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_user_sessions_installation ON user_sessions(installation_id, created_at DESC);
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
    if !columns.iter().any(|name| name == "owner_email") {
        conn.execute("ALTER TABLE installations ADD COLUMN owner_email TEXT", [])
            .map_err(|e| {
                AppError::Internal(format!("add installations owner_email failed: {e}"))
            })?;
    }
    if !columns.iter().any(|name| name == "owner_verified_at") {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN owner_verified_at TEXT",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!("add installations owner_verified_at failed: {e}"))
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
    if !columns.iter().any(|name| name == "owner_email") {
        conn.execute("ALTER TABLE shares ADD COLUMN owner_email TEXT", [])
            .map_err(|e| AppError::Internal(format!("add shares owner_email failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "shared_with_emails_json") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN shared_with_emails_json TEXT NOT NULL DEFAULT '[]'",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!("add shares shared_with_emails_json failed: {e}"))
        })?;
    }
    if !columns.iter().any(|name| name == "for_sale") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN for_sale TEXT NOT NULL DEFAULT 'No'",
            [],
        )
        .map_err(|e| AppError::Internal(format!("add shares for_sale failed: {e}")))?;
    }
    conn.execute(
        "CREATE TABLE IF NOT EXISTS email_send_logs (
            id TEXT PRIMARY KEY,
            email_type TEXT NOT NULL,
            to_email TEXT NOT NULL,
            provider_message_id TEXT,
            status TEXT NOT NULL,
            error_message TEXT,
            created_at TEXT NOT NULL
        )",
        [],
    )
    .map_err(|e| AppError::Internal(format!("create email_send_logs table failed: {e}")))?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_email_send_logs_created_at ON email_send_logs(created_at DESC)",
        [],
    )
    .map_err(|e| AppError::Internal(format!("create email_send_logs index failed: {e}")))?;
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
    if !columns.iter().any(|name| name == "upstream_provider_json") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN upstream_provider_json TEXT",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!("add shares upstream_provider_json failed: {e}"))
        })?;
    }
    if !columns.iter().any(|name| name == "app_runtimes_json") {
        conn.execute("ALTER TABLE shares ADD COLUMN app_runtimes_json TEXT", [])
            .map_err(|e| AppError::Internal(format!("add shares app_runtimes_json failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "parallel_limit") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN parallel_limit INTEGER NOT NULL DEFAULT 3",
            [],
        )
        .map_err(|e| AppError::Internal(format!("add shares parallel_limit failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "runtime_refreshed_at") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN runtime_refreshed_at TEXT",
            [],
        )
        .map_err(|e| AppError::Internal(format!("add shares runtime_refreshed_at failed: {e}")))?;
    }
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_shares_subdomain_unique ON shares(subdomain) WHERE subdomain IS NOT NULL",
        [],
    )
    .map_err(|e| AppError::Internal(format!("create subdomain unique index failed: {e}")))?;
    conn.execute(
        "UPDATE installations
         SET owner_email = (
                 SELECT s.owner_email
                 FROM shares s
                 WHERE s.installation_id = installations.id
                   AND s.owner_email IS NOT NULL
                   AND s.owner_email != ''
                 ORDER BY s.created_at DESC
                 LIMIT 1
             ),
             owner_verified_at = COALESCE(owner_verified_at, last_seen_at)
         WHERE (owner_email IS NULL OR owner_email = '')
           AND EXISTS (
                 SELECT 1
                 FROM shares s
                 WHERE s.installation_id = installations.id
                   AND s.owner_email IS NOT NULL
                   AND s.owner_email != ''
             )",
        [],
    )
    .map_err(|e| AppError::Internal(format!("backfill installation owner email failed: {e}")))?;
    Ok(())
}

fn get_installation(
    conn: &Connection,
    installation_id: &str,
) -> Result<Option<Installation>, AppError> {
    conn.query_row(
        "SELECT id, public_key, platform, app_version, owner_email, owner_verified_at, last_seen_ip, country_code, country, region, city, latitude, longitude,
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
                owner_email: row.get(4)?,
                owner_verified_at: row
                    .get::<_, Option<String>>(5)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
                last_seen_ip: row.get(6)?,
                country_code: row.get(7)?,
                country: row.get(8)?,
                region: row.get(9)?,
                city: row.get(10)?,
                latitude: row.get(11)?,
                longitude: row.get(12)?,
                geo_candidate_country_code: row.get(13)?,
                geo_candidate_country: row.get(14)?,
                geo_candidate_region: row.get(15)?,
                geo_candidate_city: row.get(16)?,
                geo_candidate_latitude: row.get(17)?,
                geo_candidate_longitude: row.get(18)?,
                geo_candidate_hits: row.get(19)?,
                geo_candidate_first_seen_at: row
                    .get::<_, Option<String>>(20)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
                geo_last_changed_at: row
                    .get::<_, Option<String>>(21)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
                created_at: parse_dt_sql(&row.get::<_, String>(22)?)?,
                last_seen_at: parse_dt_sql(&row.get::<_, String>(23)?)?,
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
            "SELECT id, public_key, platform, app_version, owner_email, owner_verified_at, last_seen_ip, country_code, country, region, city, latitude, longitude,
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
                owner_email: row.get(4)?,
                owner_verified_at: row
                    .get::<_, Option<String>>(5)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
                last_seen_ip: row.get(6)?,
                country_code: row.get(7)?,
                country: row.get(8)?,
                region: row.get(9)?,
                city: row.get(10)?,
                latitude: row.get(11)?,
                longitude: row.get(12)?,
                geo_candidate_country_code: row.get(13)?,
                geo_candidate_country: row.get(14)?,
                geo_candidate_region: row.get(15)?,
                geo_candidate_city: row.get(16)?,
                geo_candidate_latitude: row.get(17)?,
                geo_candidate_longitude: row.get(18)?,
                geo_candidate_hits: row.get(19)?,
                geo_candidate_first_seen_at: row
                    .get::<_, Option<String>>(20)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
                geo_last_changed_at: row
                    .get::<_, Option<String>>(21)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
                created_at: parse_dt_sql(&row.get::<_, String>(22)?)?,
                last_seen_at: parse_dt_sql(&row.get::<_, String>(23)?)?,
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
    let candidate_active = candidate.share_status == "active";
    let existing_active = existing.share_status == "active";
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
                    s.owner_email, s.shared_with_emails_json,
                    s.enabled_claude, s.enabled_codex, s.enabled_gemini,
                    s.token_limit, s.parallel_limit, s.tokens_used, s.requests_count, s.share_status, s.created_at, s.expires_at, s.upstream_provider_json, s.app_runtimes_json,
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
                    owner_email: row.get(9)?,
                    shared_with_emails: parse_string_vec(row.get(10)?)?,
                    support: ShareSupport {
                        claude: row.get::<_, i64>(11)? != 0,
                        codex: row.get::<_, i64>(12)? != 0,
                        gemini: row.get::<_, i64>(13)? != 0,
                    },
                    token_limit: row.get(14)?,
                    parallel_limit: row.get(15)?,
                    tokens_used: row.get(16)?,
                    requests_count: row.get(17)?,
                    share_status: row.get(18)?,
                    created_at: row.get(19)?,
                    expires_at: row.get(20)?,
                    upstream_provider: parse_upstream_provider(row.get(21)?)?,
                    app_runtimes: parse_app_runtimes(row.get(22)?)?,
                },
                row.get::<_, i64>(23)? as usize,
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
        "Free" => Ok("Free".to_string()),
        _ => Err(AppError::BadRequest(
            "share for_sale must be Yes, No, or Free".into(),
        )),
    }
}

fn parse_upstream_provider(
    value: Option<String>,
) -> Result<Option<ShareUpstreamProvider>, rusqlite::Error> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&value).map(Some).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn parse_app_runtimes(value: Option<String>) -> Result<ShareAppRuntimes, rusqlite::Error> {
    let Some(value) = value else {
        return Ok(ShareAppRuntimes::default());
    };
    if value.trim().is_empty() {
        return Ok(ShareAppRuntimes::default());
    }
    serde_json::from_str(&value).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
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
        map.insert(share_id, online_minutes.min(ONLINE_WINDOW_MINUTES));
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

fn get_share_owner_email(conn: &Connection, share_id: &str) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT owner_email FROM shares WHERE share_id = ?1",
        params![share_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query share owner email failed: {e}")))
}

fn find_share_claim_by_subdomain(
    conn: &Connection,
    subdomain: &str,
) -> Result<Option<(String, String, Option<String>)>, AppError> {
    conn.query_row(
        "SELECT share_id, installation_id, owner_email FROM shares WHERE subdomain = ?1",
        params![subdomain],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query subdomain claim failed: {e}")))
}

fn release_reclaimable_subdomain_claim(
    conn: &Connection,
    incoming_installation_id: &str,
    incoming_share_id: &str,
    incoming_owner_email: Option<&str>,
    subdomain: &str,
) -> Result<(), AppError> {
    let Some((existing_share_id, existing_installation_id, existing_owner_email)) =
        find_share_claim_by_subdomain(conn, subdomain)?
    else {
        return Ok(());
    };

    if existing_share_id == incoming_share_id {
        return Ok(());
    }

    let same_installation = existing_installation_id == incoming_installation_id;
    let same_owner = existing_owner_email.as_deref() == incoming_owner_email;
    if !same_installation && !same_owner {
        return Ok(());
    }

    conn.execute(
        "DELETE FROM share_request_logs WHERE share_id = ?1",
        params![existing_share_id],
    )
    .map_err(|e| AppError::Internal(format!("delete replaced share request logs failed: {e}")))?;
    conn.execute(
        "DELETE FROM share_health_checks WHERE share_id = ?1",
        params![existing_share_id],
    )
    .map_err(|e| AppError::Internal(format!("delete replaced share health checks failed: {e}")))?;
    conn.execute(
        "DELETE FROM shares WHERE share_id = ?1",
        params![existing_share_id],
    )
    .map_err(|e| AppError::Internal(format!("delete replaced share claim failed: {e}")))?;
    Ok(())
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

fn verify_signed_share_request<T: Serialize>(
    conn: &Connection,
    public_key: &str,
    installation_id: &str,
    action: &str,
    payload: &T,
    timestamp_ms: i64,
    nonce: &str,
    signature: &str,
) -> Result<(), AppError> {
    let now = Utc::now();
    let skew = (now.timestamp_millis() - timestamp_ms).abs();
    if skew > SIGNED_REQUEST_MAX_SKEW_MS {
        return Err(AppError::Unauthorized("stale signed request".into()));
    }

    verify_signed_payload(
        public_key,
        installation_id,
        action,
        payload,
        timestamp_ms,
        nonce,
        signature,
    )?;
    consume_request_nonce(conn, installation_id, action, nonce, now)
}

fn consume_request_nonce(
    conn: &Connection,
    installation_id: &str,
    action: &str,
    nonce: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO request_nonces (installation_id, action, nonce, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![installation_id, action, nonce, now.to_rfc3339()],
    )
    .map_err(|err| {
        let text = err.to_string();
        if text.contains("UNIQUE constraint failed")
            || text.contains("request_nonces.installation_id")
        {
            AppError::Unauthorized("nonce already used".into())
        } else {
            AppError::Internal(format!("store request nonce failed: {text}"))
        }
    })?;
    Ok(())
}

fn verify_signed_payload<T: Serialize>(
    public_key: &str,
    installation_id: &str,
    action: &str,
    payload: &T,
    timestamp_ms: i64,
    nonce: &str,
    signature: &str,
) -> Result<(), AppError> {
    let key_bytes = base64::engine::general_purpose::STANDARD
        .decode(public_key)
        .map_err(|_| AppError::Unauthorized("invalid stored public key".into()))?;
    let key_array: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| AppError::Unauthorized("invalid public key length".into()))?;
    let verifying_key = VerifyingKey::from_bytes(&key_array)
        .map_err(|_| AppError::Unauthorized("invalid public key".into()))?;

    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(signature)
        .map_err(|_| AppError::Unauthorized("invalid signature".into()))?;
    let sig_array: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| AppError::Unauthorized("invalid signature length".into()))?;
    let signature = Signature::from_bytes(&sig_array);

    let payload_json = serde_json::to_string(payload)
        .map_err(|_| AppError::Unauthorized("invalid signed payload".into()))?;
    let payload = format!(
        "{}\n{}\n{}\n{}\n{}",
        installation_id, action, payload_json, timestamp_ms, nonce
    );
    verifying_key
        .verify(payload.as_bytes(), &signature)
        .map_err(|_| AppError::Unauthorized("signature verification failed".into()))
}

async fn redeem_verification_token(
    config: &Config,
    verification_token: &str,
) -> Result<VerificationRedeemResponse, AppError> {
    let url = format!(
        "{}/v1/verification/email/redeem",
        config.verification_service_base_url.trim_end_matches('/')
    );
    let client = reqwest::Client::builder()
        .timeout(StdDuration::from_secs(20))
        .build()
        .map_err(|e| AppError::Internal(format!("create verification client failed: {e}")))?;
    let mut request = client.post(&url).json(&serde_json::json!({
        "verificationToken": verification_token,
        "purpose": AUTH_PURPOSE_LOGIN,
    }));
    if let Some(api_key) = config.verification_service_api_key.as_deref() {
        request = request.bearer_auth(api_key);
    }
    let response = request.send().await.map_err(|e| {
        AppError::Internal(format!("redeem verification token request failed: {e}"))
    })?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| format!("HTTP {status}"));
        return Err(AppError::Unauthorized(format!(
            "redeem verification token failed: {body}"
        )));
    }
    response
        .json::<VerificationRedeemResponse>()
        .await
        .map_err(|e| AppError::Internal(format!("parse verification redeem response failed: {e}")))
}

#[derive(Debug, Clone)]
struct EmailLoginChallenge {
    id: String,
    code_hash: String,
    attempt_count: i64,
}

fn normalize_email(value: &str) -> Result<String, AppError> {
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

fn normalize_email_list(values: &[String], owner_email: &str) -> Vec<String> {
    let mut result = Vec::new();
    for value in values {
        if let Ok(email) = normalize_email(value) {
            if email == owner_email || result.contains(&email) {
                continue;
            }
            result.push(email);
        }
    }
    result
}

fn mask_email(email: &str) -> String {
    let Some((local, domain)) = email.split_once('@') else {
        return "***".into();
    };
    let mut chars = local.chars();
    let first = chars.next().unwrap_or('*');
    let last = local.chars().last().unwrap_or(first);
    format!("{first}***{last}@{domain}")
}

fn hash_token(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
}

fn generate_secret(len: usize) -> String {
    Alphanumeric.sample_string(&mut rand::thread_rng(), len)
}

fn generate_numeric_code(len: usize) -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| char::from(b'0' + rng.gen_range(0..10)))
        .collect()
}

fn parse_string_vec(value: Option<String>) -> Result<Vec<String>, rusqlite::Error> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&value).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

async fn send_login_code_email(
    resend: &Resend,
    config: &Config,
    email: &str,
    code: &str,
    ttl_secs: i64,
) -> Result<Option<String>, AppError> {
    let from = config
        .resend_from
        .as_deref()
        .ok_or_else(|| AppError::Internal("resend from address is not configured".into()))?;
    let html = format!(
        "<div style=\"font-family:Arial,sans-serif\"><p>Your verification code is:</p><p style=\"font-size:28px;font-weight:700;letter-spacing:6px\">{code}</p><p>This code expires in {} minutes.</p></div>",
        (ttl_secs / 60).max(1)
    );
    let mut message =
        CreateEmailBaseOptions::new(from, [email], "Your TokenSwitch verification code")
            .with_html(&html);
    if let Some(reply_to) = config.resend_reply_to.as_deref() {
        message = message.with_reply(reply_to);
    }
    let response = resend
        .emails
        .send(message)
        .await
        .map_err(|e| AppError::Internal(format!("send verification email failed: {e}")))?;
    Ok(Some(response.id.to_string()))
}

fn enforce_auth_send_limits(
    conn: &Connection,
    config: &Config,
    email: &str,
    installation_id: &str,
    metadata: &ClientMetadata,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let hour_cutoff = (now - Duration::hours(1)).to_rfc3339();
    if let Some(next_allowed_at) = latest_challenge_cooldown(conn, email, installation_id)? {
        if next_allowed_at > now {
            return Err(AppError::TooManyRequests(format!(
                "verification email cooldown active, retry in {}s",
                (next_allowed_at - now).num_seconds().max(1)
            )));
        }
    }

    let email_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM email_login_challenges
             WHERE email_normalized = ?1 AND created_at >= ?2",
            params![email, hour_cutoff],
            |row| row.get(0),
        )
        .map_err(|e| AppError::Internal(format!("count auth email requests failed: {e}")))?;
    if email_count >= config.auth_email_hourly_limit {
        return Err(AppError::TooManyRequests(
            "email verification rate limit exceeded".into(),
        ));
    }

    let installation_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM email_login_challenges
             WHERE installation_id = ?1 AND created_at >= ?2",
            params![installation_id, hour_cutoff],
            |row| row.get(0),
        )
        .map_err(|e| AppError::Internal(format!("count installation auth requests failed: {e}")))?;
    if installation_count >= config.auth_installation_hourly_limit {
        return Err(AppError::TooManyRequests(
            "installation verification rate limit exceeded".into(),
        ));
    }

    if let Some(ip) = metadata.ip.as_deref() {
        let ip_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM email_login_challenges
                 WHERE created_ip = ?1 AND created_at >= ?2",
                params![ip, hour_cutoff],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Internal(format!("count ip auth requests failed: {e}")))?;
        if ip_count >= config.auth_ip_hourly_limit {
            return Err(AppError::TooManyRequests(
                "ip verification rate limit exceeded".into(),
            ));
        }
    }

    Ok(())
}

fn latest_challenge_cooldown(
    conn: &Connection,
    email: &str,
    installation_id: &str,
) -> Result<Option<DateTime<Utc>>, AppError> {
    conn.query_row(
        "SELECT resend_available_at
         FROM email_login_challenges
         WHERE email_normalized = ?1
           AND installation_id = ?2
         ORDER BY created_at DESC
         LIMIT 1",
        params![email, installation_id],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query latest challenge cooldown failed: {e}")))?
    .map(|value| {
        parse_dt_sql(&value).map_err(|e| AppError::Internal(format!("parse cooldown failed: {e}")))
    })
    .transpose()
}

fn get_latest_active_email_challenge(
    conn: &Connection,
    email: &str,
    installation_id: &str,
    purpose: &str,
    now: DateTime<Utc>,
) -> Result<Option<EmailLoginChallenge>, AppError> {
    conn.query_row(
        "SELECT id, code_hash, attempt_count
         FROM email_login_challenges
         WHERE email_normalized = ?1
           AND installation_id = ?2
           AND purpose = ?3
           AND consumed_at IS NULL
           AND expires_at >= ?4
         ORDER BY created_at DESC
         LIMIT 1",
        params![email, installation_id, purpose, now.to_rfc3339()],
        |row| {
            Ok(EmailLoginChallenge {
                id: row.get(0)?,
                code_hash: row.get(1)?,
                attempt_count: row.get(2)?,
            })
        },
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query auth challenge failed: {e}")))
}

fn upsert_user_by_email(
    conn: &Connection,
    email: &str,
    now: DateTime<Utc>,
) -> Result<AuthUser, AppError> {
    if let Some(user) = get_user_by_email(conn, email)? {
        conn.execute(
            "UPDATE users SET last_login_at = ?2 WHERE id = ?1",
            params![user.id, now.to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("update user login failed: {e}")))?;
        return Ok(user);
    }
    let user = AuthUser {
        id: Uuid::new_v4().to_string(),
        email: email.to_string(),
    };
    conn.execute(
        "INSERT INTO users (id, email_normalized, status, created_at, last_login_at)
         VALUES (?1, ?2, 'active', ?3, ?4)",
        params![user.id, user.email, now.to_rfc3339(), now.to_rfc3339()],
    )
    .map_err(|e| AppError::Internal(format!("insert user failed: {e}")))?;
    Ok(user)
}

fn get_user_by_email(conn: &Connection, email: &str) -> Result<Option<AuthUser>, AppError> {
    conn.query_row(
        "SELECT id, email_normalized FROM users WHERE email_normalized = ?1",
        params![email],
        |row| {
            Ok(AuthUser {
                id: row.get(0)?,
                email: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query user by email failed: {e}")))
}

fn get_user_by_id(conn: &Connection, user_id: &str) -> Result<Option<AuthUser>, AppError> {
    conn.query_row(
        "SELECT id, email_normalized FROM users WHERE id = ?1",
        params![user_id],
        |row| {
            Ok(AuthUser {
                id: row.get(0)?,
                email: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query user by id failed: {e}")))
}

fn persist_session(conn: &Connection, session: &AuthSession) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO user_sessions (
            id, user_id, installation_id, access_token_hash, refresh_token_hash,
            access_expires_at, refresh_expires_at, revoked_at, created_at, last_used_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?9)",
        params![
            session.session_id,
            session.user_id,
            session.installation_id,
            session.access_token_hash,
            session.refresh_token_hash,
            session.access_expires_at.to_rfc3339(),
            session.refresh_expires_at.to_rfc3339(),
            session.created_at.to_rfc3339(),
            session.last_used_at.to_rfc3339(),
        ],
    )
    .map_err(|e| AppError::Internal(format!("persist session failed: {e}")))?;
    Ok(())
}

fn map_auth_session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuthSession> {
    Ok(AuthSession {
        session_id: row.get(0)?,
        user_id: row.get(1)?,
        installation_id: row.get(2)?,
        access_token_hash: row.get(3)?,
        refresh_token_hash: row.get(4)?,
        access_expires_at: parse_dt_sql(&row.get::<_, String>(5)?)?,
        refresh_expires_at: parse_dt_sql(&row.get::<_, String>(6)?)?,
        created_at: parse_dt_sql(&row.get::<_, String>(7)?)?,
        last_used_at: parse_dt_sql(&row.get::<_, String>(8)?)?,
        email: row.get(9)?,
    })
}

fn get_session_by_access_hash(
    conn: &Connection,
    access_hash: &str,
) -> Result<Option<AuthSession>, AppError> {
    conn.query_row(
        "SELECT s.id, s.user_id, s.installation_id, s.access_token_hash, s.refresh_token_hash,
                s.access_expires_at, s.refresh_expires_at, s.created_at, s.last_used_at, u.email_normalized
         FROM user_sessions s
         INNER JOIN users u ON u.id = s.user_id
         WHERE s.access_token_hash = ?1 AND s.revoked_at IS NULL",
        params![access_hash],
        map_auth_session_row,
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query session by access hash failed: {e}")))
}

fn get_session_by_refresh_hash(
    conn: &Connection,
    refresh_hash: &str,
) -> Result<Option<AuthSession>, AppError> {
    conn.query_row(
        "SELECT s.id, s.user_id, s.installation_id, s.access_token_hash, s.refresh_token_hash,
                s.access_expires_at, s.refresh_expires_at, s.created_at, s.last_used_at, u.email_normalized
         FROM user_sessions s
         INNER JOIN users u ON u.id = s.user_id
         WHERE s.refresh_token_hash = ?1 AND s.revoked_at IS NULL",
        params![refresh_hash],
        map_auth_session_row,
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query session by refresh hash failed: {e}")))
}

fn get_installation_owner_email(
    conn: &Connection,
    installation_id: &str,
) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT owner_email
         FROM installations
         WHERE id = ?1
           AND owner_email IS NOT NULL
           AND owner_email != ''
         LIMIT 1",
        params![installation_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query installation owner email failed: {e}")))
}

fn require_installation_owner_email(
    conn: &Connection,
    installation_id: &str,
) -> Result<String, AppError> {
    get_installation_owner_email(conn, installation_id)?.ok_or_else(|| {
        AppError::Unauthorized("installation owner email binding is required".into())
    })
}

fn share_visible_to_email(share: &ShareDescriptor, viewer_email: Option<&str>) -> bool {
    let Some(viewer_email) = viewer_email else {
        return false;
    };
    share.owner_email.as_deref() == Some(viewer_email)
        || share
            .shared_with_emails
            .iter()
            .any(|email| email == viewer_email)
}

fn can_manage_share(share: &ShareDescriptor, viewer_email: Option<&str>) -> bool {
    share.owner_email.as_deref() == viewer_email
}

fn enforce_share_owner(
    share: &mut ShareDescriptor,
    existing_owner_email: Option<&str>,
    current_user_email: &str,
) -> Result<(), AppError> {
    let current_user_email = normalize_email(current_user_email)?;
    if let Some(existing_owner_email) = existing_owner_email {
        if existing_owner_email != current_user_email {
            return Err(AppError::Unauthorized("share owner mismatch".into()));
        }
    }
    share.owner_email = Some(current_user_email.clone());
    share.share_name = current_user_email.clone();
    share.shared_with_emails = normalize_email_list(&share.shared_with_emails, &current_user_email);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::models::ShareSyncOperation;
    use crate::proxy::ProxyRegistry;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::path::PathBuf;

    fn test_config(name: &str) -> Config {
        let db_path = std::env::temp_dir().join(format!("portr-rs-{name}-{}.db", Uuid::new_v4()));
        Config {
            api_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8787),
            ssh_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 2222),
            tunnel_domain: "127.0.0.1:8787".into(),
            ssh_public_addr: String::new(),
            use_localhost: true,
            lease_ttl_secs: 60,
            db_path,
            host_key_path: std::env::temp_dir()
                .join(format!("portr-rs-{name}-{}.key", Uuid::new_v4())),
            cleanup_interval_secs: 300,
            lease_retention_secs: 7 * 24 * 60 * 60,
            client_stale_secs: 60 * 60,
            resend_api_key: None,
            resend_from: None,
            resend_reply_to: None,
            auth_code_ttl_secs: 600,
            auth_code_cooldown_secs: 60,
            auth_session_ttl_secs: 7 * 24 * 60 * 60,
            auth_refresh_ttl_secs: 30 * 24 * 60 * 60,
            auth_max_verify_attempts: 8,
            auth_email_hourly_limit: 10,
            auth_ip_hourly_limit: 30,
            auth_installation_hourly_limit: 15,
            verification_service_base_url: "https://tokenswitch.org".into(),
            verification_service_api_key: None,
        }
    }

    async fn setup_store(name: &str) -> (AppStore, Config) {
        let config = test_config(name);
        let store = AppStore::new(&config).expect("create store");
        (store, config)
    }

    async fn insert_installation(store: &AppStore, installation_id: &str) {
        let now = Utc::now().to_rfc3339();
        let conn = store.conn.lock().await;
        conn.execute(
            "INSERT INTO installations (
                id, public_key, platform, app_version, owner_email, owner_verified_at, created_at, last_seen_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                installation_id,
                format!("pk-{installation_id}"),
                "macOS",
                "1.0.0",
                "owner@example.com",
                now,
                now,
                now,
            ],
        )
        .expect("insert installation");
    }

    async fn set_installation_country_code(
        store: &AppStore,
        installation_id: &str,
        country_code: &str,
    ) {
        let conn = store.conn.lock().await;
        conn.execute(
            "UPDATE installations SET country_code = ?2 WHERE id = ?1",
            params![installation_id, country_code],
        )
        .expect("update installation country_code");
    }

    async fn mark_installation_last_seen(
        store: &AppStore,
        installation_id: &str,
        value: DateTime<Utc>,
    ) {
        let conn = store.conn.lock().await;
        conn.execute(
            "UPDATE installations SET last_seen_at = ?2 WHERE id = ?1",
            params![installation_id, value.to_rfc3339()],
        )
        .expect("update installation last_seen_at");
    }

    async fn insert_share(
        store: &AppStore,
        installation_id: &str,
        share_id: &str,
        subdomain: &str,
        share_status: &str,
    ) {
        let now = Utc::now();
        let expires = now + Duration::hours(1);
        let conn = store.conn.lock().await;
        conn.execute(
            "INSERT INTO shares (
                share_id, installation_id, share_name, owner_email, shared_with_emails_json,
                description, for_sale, subdomain, share_token, app_type, provider_id,
                enabled_claude, enabled_codex, enabled_gemini, token_limit, parallel_limit,
                tokens_used, requests_count, share_status, created_at, expires_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, '[]', NULL, 'No', ?5, 'token', 'proxy', NULL, 1, 1, 1, 1000, 3, 0, 0, ?6, ?7, ?8, ?7)",
            params![
                share_id,
                installation_id,
                format!("share-{share_id}"),
                "owner@example.com",
                subdomain,
                share_status,
                now.to_rfc3339(),
                expires.to_rfc3339(),
            ],
        )
        .expect("insert share");
    }

    async fn insert_health_check(store: &AppStore, share_id: &str, checked_at: i64) {
        let conn = store.conn.lock().await;
        conn.execute(
            "INSERT INTO share_health_checks (share_id, checked_at, is_healthy) VALUES (?1, ?2, 1)",
            params![share_id, checked_at],
        )
        .expect("insert health check");
    }

    async fn insert_signed_installation(store: &AppStore, installation_id: &str) -> SigningKey {
        let signing_key = SigningKey::generate(&mut OsRng);
        let public_key = base64::engine::general_purpose::STANDARD
            .encode(signing_key.verifying_key().to_bytes());
        let now = Utc::now().to_rfc3339();
        let conn = store.conn.lock().await;
        conn.execute(
            "INSERT INTO installations (
                id, public_key, platform, app_version, owner_email, owner_verified_at, created_at, last_seen_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                installation_id,
                public_key,
                "macOS",
                "1.0.0",
                "owner@example.com",
                now,
                now,
                now
            ],
        )
        .expect("insert signed installation");
        signing_key
    }

    fn sign_test_payload<T: Serialize>(
        signing_key: &SigningKey,
        installation_id: &str,
        action: &str,
        payload: &T,
        timestamp_ms: i64,
        nonce: &str,
    ) -> String {
        let payload_json = serde_json::to_string(payload).expect("serialize test payload");
        let body = format!("{installation_id}\n{action}\n{payload_json}\n{timestamp_ms}\n{nonce}");
        let signature = signing_key.sign(body.as_bytes());
        base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
    }

    #[tokio::test]
    async fn list_share_route_targets_only_returns_active_shares() {
        let (store, config) = setup_store("route-targets").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-active", "active-sub", "active").await;
        insert_share(&store, "inst-1", "share-paused", "paused-sub", "paused").await;

        let targets = store
            .list_share_route_targets()
            .await
            .expect("list route targets");
        let subdomains = targets
            .into_iter()
            .map(|target| target.subdomain)
            .collect::<Vec<_>>();

        assert_eq!(subdomains, vec!["active-sub".to_string()]);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn claim_share_subdomain_accepts_valid_signature_and_rejects_replay_and_tamper() {
        let (store, config) = setup_store("signed-share-claim").await;
        let signing_key = insert_signed_installation(&store, "inst-signed").await;

        let share = ShareDescriptor {
            share_id: "share-1".into(),
            share_name: "Signed Share".into(),
            owner_email: Some("owner@example.com".into()),
            shared_with_emails: vec![],
            description: None,
            for_sale: "No".into(),
            subdomain: "signed-sub".into(),
            share_token: "token-12345678".into(),
            app_type: "proxy".into(),
            provider_id: None,
            token_limit: 1000,
            parallel_limit: 3,
            tokens_used: 0,
            requests_count: 0,
            share_status: "paused".into(),
            created_at: Utc::now().to_rfc3339(),
            expires_at: (Utc::now() + Duration::hours(1)).to_rfc3339(),
            support: ShareSupport::default(),
            upstream_provider: None,
            app_runtimes: ShareAppRuntimes::default(),
        };

        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            &signing_key,
            "inst-signed",
            "share_claim_subdomain",
            &share,
            timestamp_ms,
            &nonce,
        );

        let request = ShareClaimSubdomainRequest {
            installation_id: "inst-signed".into(),
            timestamp_ms,
            nonce: nonce.clone(),
            signature: signature.clone(),
            share: share.clone(),
        };

        store
            .claim_share_subdomain(
                request,
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect("valid signed share claim");

        let replay_err = store
            .claim_share_subdomain(
                ShareClaimSubdomainRequest {
                    installation_id: "inst-signed".into(),
                    timestamp_ms,
                    nonce: nonce.clone(),
                    signature: signature.clone(),
                    share: share.clone(),
                },
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect_err("replay should fail");
        assert!(replay_err.to_string().contains("nonce already used"));

        let tampered_share = ShareDescriptor {
            subdomain: "signed-sub-tampered".into(),
            ..share
        };
        let tampered_err = store
            .claim_share_subdomain(
                ShareClaimSubdomainRequest {
                    installation_id: "inst-signed".into(),
                    timestamp_ms: Utc::now().timestamp_millis(),
                    nonce: Uuid::new_v4().to_string(),
                    signature,
                    share: tampered_share,
                },
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect_err("tampered payload should fail");
        assert!(
            tampered_err
                .to_string()
                .contains("signature verification failed")
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn claim_share_subdomain_allows_same_owner_to_reclaim_existing_subdomain() {
        let (store, config) = setup_store("signed-share-reclaim-owner").await;
        insert_share(&store, "inst-old", "share-old", "owner-sub", "paused").await;
        let signing_key = insert_signed_installation(&store, "inst-new").await;

        let share = ShareDescriptor {
            share_id: "share-new".into(),
            share_name: "owner@example.com".into(),
            owner_email: Some("owner@example.com".into()),
            shared_with_emails: vec![],
            description: None,
            for_sale: "No".into(),
            subdomain: "owner-sub".into(),
            share_token: "token-12345678".into(),
            app_type: "proxy".into(),
            provider_id: None,
            token_limit: 1000,
            parallel_limit: 3,
            tokens_used: 0,
            requests_count: 0,
            share_status: "paused".into(),
            created_at: Utc::now().to_rfc3339(),
            expires_at: (Utc::now() + Duration::hours(1)).to_rfc3339(),
            support: ShareSupport::default(),
            upstream_provider: None,
            app_runtimes: ShareAppRuntimes::default(),
        };

        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            &signing_key,
            "inst-new",
            "share_claim_subdomain",
            &share,
            timestamp_ms,
            &nonce,
        );

        store
            .claim_share_subdomain(
                ShareClaimSubdomainRequest {
                    installation_id: "inst-new".into(),
                    timestamp_ms,
                    nonce,
                    signature,
                    share: share.clone(),
                },
                ClientMetadata {
                    ip: None,
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect("claim reclaimed subdomain");

        let conn = store.conn.lock().await;
        let rows: Vec<(String, String, String)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT share_id, installation_id, subdomain
                     FROM shares
                     WHERE subdomain = 'owner-sub'",
                )
                .expect("prepare reclaimed subdomain query");
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .expect("query reclaimed subdomain rows")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect reclaimed subdomain rows")
        };
        assert_eq!(
            rows,
            vec![("share-new".into(), "inst-new".into(), "owner-sub".into())]
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn claim_share_subdomain_allows_same_installation_to_replace_deleted_share_claim() {
        let (store, config) = setup_store("signed-share-reclaim-installation").await;
        let signing_key = insert_signed_installation(&store, "inst-same").await;
        insert_share(&store, "inst-same", "share-old", "reused-sub", "paused").await;

        let share = ShareDescriptor {
            share_id: "share-new".into(),
            share_name: "owner@example.com".into(),
            owner_email: Some("different@example.com".into()),
            shared_with_emails: vec![],
            description: None,
            for_sale: "No".into(),
            subdomain: "reused-sub".into(),
            share_token: "token-12345678".into(),
            app_type: "proxy".into(),
            provider_id: None,
            token_limit: 1000,
            parallel_limit: 3,
            tokens_used: 0,
            requests_count: 0,
            share_status: "paused".into(),
            created_at: Utc::now().to_rfc3339(),
            expires_at: (Utc::now() + Duration::hours(1)).to_rfc3339(),
            support: ShareSupport::default(),
            upstream_provider: None,
            app_runtimes: ShareAppRuntimes::default(),
        };

        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            &signing_key,
            "inst-same",
            "share_claim_subdomain",
            &share,
            timestamp_ms,
            &nonce,
        );

        store
            .claim_share_subdomain(
                ShareClaimSubdomainRequest {
                    installation_id: "inst-same".into(),
                    timestamp_ms,
                    nonce,
                    signature,
                    share: share.clone(),
                },
                ClientMetadata {
                    ip: None,
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect("claim reclaimed subdomain for same installation");

        let conn = store.conn.lock().await;
        let rows: Vec<(String, String, String)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT share_id, installation_id, subdomain
                     FROM shares
                     WHERE subdomain = 'reused-sub'",
                )
                .expect("prepare reclaimed installation subdomain query");
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .expect("query reclaimed installation subdomain rows")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect reclaimed installation subdomain rows")
        };
        assert_eq!(
            rows,
            vec![("share-new".into(), "inst-same".into(), "reused-sub".into())]
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn batch_sync_shares_requires_valid_signature() {
        let (store, config) = setup_store("signed-share-batch").await;
        let signing_key = insert_signed_installation(&store, "inst-batch").await;

        let share = ShareDescriptor {
            share_id: "share-batch-1".into(),
            share_name: "Batch Share".into(),
            owner_email: Some("owner@example.com".into()),
            shared_with_emails: vec![],
            description: Some("signed batch sync".into()),
            for_sale: "No".into(),
            subdomain: "batch-sub".into(),
            share_token: "token-batch-123".into(),
            app_type: "proxy".into(),
            provider_id: None,
            token_limit: 2048,
            parallel_limit: 3,
            tokens_used: 12,
            requests_count: 3,
            share_status: "active".into(),
            created_at: Utc::now().to_rfc3339(),
            expires_at: (Utc::now() + Duration::hours(2)).to_rfc3339(),
            support: ShareSupport {
                claude: true,
                codex: true,
                gemini: false,
            },
            upstream_provider: None,
            app_runtimes: ShareAppRuntimes::default(),
        };
        let ops = vec![ShareSyncOperation {
            kind: "upsert".into(),
            share: Some(share.clone()),
            share_id: None,
        }];

        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            &signing_key,
            "inst-batch",
            "share_batch_sync",
            &ops,
            timestamp_ms,
            &nonce,
        );

        store
            .batch_sync_shares(
                ShareBatchSyncRequest {
                    installation_id: "inst-batch".into(),
                    timestamp_ms,
                    nonce,
                    signature,
                    ops,
                },
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect("valid signed batch sync");

        let conn = store.conn.lock().await;
        let synced: (String, String, i64) = conn
            .query_row(
                "SELECT share_name, subdomain, token_limit FROM shares
                 WHERE installation_id = ?1 AND share_id = ?2",
                params!["inst-batch", "share-batch-1"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("query synced share");
        drop(conn);
        assert_eq!(synced.0, "owner@example.com");
        assert_eq!(synced.1, "batch-sub");
        assert_eq!(synced.2, 2048);

        let tampered_ops = vec![ShareSyncOperation {
            kind: "upsert".into(),
            share: Some(ShareDescriptor {
                share_name: "Batch Share Tampered".into(),
                ..share
            }),
            share_id: None,
        }];
        let tampered_err = store
            .batch_sync_shares(
                ShareBatchSyncRequest {
                    installation_id: "inst-batch".into(),
                    timestamp_ms: Utc::now().timestamp_millis(),
                    nonce: Uuid::new_v4().to_string(),
                    signature: sign_test_payload(
                        &signing_key,
                        "inst-batch",
                        "share_batch_sync",
                        &vec![ShareSyncOperation {
                            kind: "upsert".into(),
                            share: Some(ShareDescriptor {
                                share_name: "Different".into(),
                                ..tampered_ops[0].share.clone().expect("share")
                            }),
                            share_id: None,
                        }],
                        Utc::now().timestamp_millis(),
                        &Uuid::new_v4().to_string(),
                    ),
                    ops: tampered_ops,
                },
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect_err("tampered batch sync should fail");
        assert!(
            tampered_err
                .to_string()
                .contains("signature verification failed")
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn batch_sync_share_request_logs_requires_valid_signature() {
        let (store, config) = setup_store("signed-log-batch").await;
        let signing_key = insert_signed_installation(&store, "inst-logs").await;
        insert_share(&store, "inst-logs", "share-log-1", "log-sub", "active").await;

        let logs = vec![ShareRequestLogEntry {
            request_id: "req-1".into(),
            share_id: "share-log-1".into(),
            share_name: "Log Share".into(),
            provider_id: "provider-1".into(),
            provider_name: "Provider One".into(),
            app_type: "codex".into(),
            model: "gpt-5".into(),
            request_model: "gpt-5".into(),
            status_code: 200,
            latency_ms: 1234,
            first_token_ms: Some(222),
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            is_streaming: true,
            session_id: Some("session-1".into()),
            created_at: Utc::now().timestamp(),
        }];

        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            &signing_key,
            "inst-logs",
            "share_request_logs_batch_sync",
            &logs,
            timestamp_ms,
            &nonce,
        );

        store
            .batch_sync_share_request_logs(
                ShareRequestLogBatchSyncRequest {
                    installation_id: "inst-logs".into(),
                    timestamp_ms,
                    nonce: nonce.clone(),
                    signature: signature.clone(),
                    logs: logs.clone(),
                },
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect("valid signed request log batch sync");

        let conn = store.conn.lock().await;
        let stored_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM share_request_logs
                 WHERE installation_id = ?1 AND request_id = ?2",
                params!["inst-logs", "req-1"],
                |row| row.get(0),
            )
            .expect("count synced request logs");
        drop(conn);
        assert_eq!(stored_count, 1);

        let replay_err = store
            .batch_sync_share_request_logs(
                ShareRequestLogBatchSyncRequest {
                    installation_id: "inst-logs".into(),
                    timestamp_ms,
                    nonce,
                    signature,
                    logs: logs.clone(),
                },
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect_err("replayed log sync should fail");
        assert!(replay_err.to_string().contains("nonce already used"));

        let tampered_logs = vec![ShareRequestLogEntry {
            status_code: 500,
            ..logs[0].clone()
        }];
        let bad_signature = sign_test_payload(
            &signing_key,
            "inst-logs",
            "share_request_logs_batch_sync",
            &logs,
            Utc::now().timestamp_millis(),
            &Uuid::new_v4().to_string(),
        );
        let tampered_err = store
            .batch_sync_share_request_logs(
                ShareRequestLogBatchSyncRequest {
                    installation_id: "inst-logs".into(),
                    timestamp_ms: Utc::now().timestamp_millis(),
                    nonce: Uuid::new_v4().to_string(),
                    signature: bad_signature,
                    logs: tampered_logs,
                },
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect_err("tampered log sync should fail");
        assert!(
            tampered_err
                .to_string()
                .contains("signature verification failed")
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn dashboard_snapshot_does_not_count_paused_share_as_active() {
        let (store, config) = setup_store("dashboard-paused").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-paused", "paused-sub", "paused").await;

        let server_geo = ServerGeo {
            lat: None,
            lon: None,
        };
        let proxy = ProxyRegistry::default();
        let snapshot = store
            .dashboard_snapshot(&config, &server_geo, &proxy, None)
            .await
            .expect("dashboard snapshot");

        assert_eq!(snapshot.stats.clients, 1);
        assert_eq!(snapshot.stats.active_shares, 0);
        assert_eq!(snapshot.stats.active_leases, 0);
        assert_eq!(snapshot.clients.len(), 1);
        assert_eq!(
            snapshot.clients[0]
                .share
                .as_ref()
                .expect("share view")
                .share_status,
            "paused"
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn public_map_points_returns_total_client_count_alongside_deduplicated_points() {
        let (store, config) = setup_store("public-map-client-count").await;
        for installation_id in ["inst-1", "inst-2", "inst-3"] {
            insert_installation(&store, installation_id).await;
            set_installation_country_code(&store, installation_id, "JP").await;
        }
        insert_share(&store, "inst-1", "share-1", "sub-1", "active").await;
        insert_share(&store, "inst-2", "share-2", "sub-2", "active").await;
        insert_share(&store, "inst-3", "share-3", "sub-3", "active").await;

        let server_geo = ServerGeo {
            lat: Some(35.6895),
            lon: Some(139.692),
        };
        let points = store
            .public_map_points(&server_geo)
            .await
            .expect("public map points");

        assert_eq!(points.client_count, 3);
        assert_eq!(points.clients.len(), 1);
        assert_eq!(points.clients[0].lat, 36.2);
        assert_eq!(points.clients[0].lon, 138.25);
        assert_eq!(points.clients[0].count, 3);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn cleanup_removes_clients_and_shares_after_one_hour_without_report() {
        let (store, config) = setup_store("cleanup-stale-client").await;
        insert_installation(&store, "inst-stale").await;
        insert_installation(&store, "inst-fresh").await;
        insert_share(&store, "inst-stale", "share-stale", "stale-sub", "active").await;
        insert_share(&store, "inst-fresh", "share-fresh", "fresh-sub", "active").await;
        mark_installation_last_seen(&store, "inst-stale", Utc::now() - Duration::hours(2)).await;

        let proxy = ProxyRegistry::default();
        proxy
            .set_route("stale-sub".into(), "127.0.0.1:1234".into(), None, None, -1)
            .await;
        proxy
            .set_route("fresh-sub".into(), "127.0.0.1:5678".into(), None, None, -1)
            .await;

        let result = store
            .cleanup_expired_data(&config, &proxy)
            .await
            .expect("cleanup stale client");

        assert_eq!(result.deleted_installations, 1);
        assert_eq!(result.deleted_shares, 1);
        assert_eq!(result.removed_routes, 1);

        let conn = store.conn.lock().await;
        let stale_installations: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM installations WHERE id = 'inst-stale'",
                [],
                |row| row.get(0),
            )
            .expect("count stale installations");
        let stale_shares: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM shares WHERE share_id = 'share-stale'",
                [],
                |row| row.get(0),
            )
            .expect("count stale shares");
        let fresh_shares: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM shares WHERE share_id = 'share-fresh'",
                [],
                |row| row.get(0),
            )
            .expect("count fresh shares");
        drop(conn);

        assert_eq!(stale_installations, 0);
        assert_eq!(stale_shares, 0);
        assert_eq!(fresh_shares, 1);
        let active_subdomains = proxy.active_subdomains().await;
        assert!(!active_subdomains.contains(&"stale-sub".to_string()));
        assert!(active_subdomains.contains(&"fresh-sub".to_string()));

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn online_minutes_24h_is_capped_to_one_day() {
        let (store, config) = setup_store("online-minutes-cap").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-1", "share-sub", "active").await;

        let now = Utc::now().timestamp();
        for minute_offset in 0..=ONLINE_WINDOW_MINUTES {
            insert_health_check(&store, "share-1", now - (minute_offset as i64 * 60)).await;
        }

        let conn = store.conn.lock().await;
        let online = list_online_minutes_24h(&conn).expect("list online minutes");
        drop(conn);

        assert_eq!(online.get("share-1"), Some(&ONLINE_WINDOW_MINUTES));

        let server_geo = ServerGeo {
            lat: None,
            lon: None,
        };
        let proxy = ProxyRegistry::default();
        let snapshot = store
            .dashboard_snapshot(&config, &server_geo, &proxy, None)
            .await
            .expect("dashboard snapshot");
        let share = snapshot.clients[0].share.as_ref().expect("share view");
        assert_eq!(share.online_minutes_24h, ONLINE_WINDOW_MINUTES);
        assert_eq!(share.online_rate_24h, 100.0);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }
}
