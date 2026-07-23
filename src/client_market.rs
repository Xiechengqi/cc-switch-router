use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use axum::Json;
use axum::Router;
use axum::extract::{ConnectInfo, Path as AxumPath, Query, State};
use axum::http::{HeaderMap, header};
use axum::routing::{delete, get, post};
use base64::Engine;
use chrono::Utc;
use rand::RngCore;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::ServerState;
use crate::config::Config;
use crate::error::AppError;
use crate::namespace::normalize_client_subdomain;
use crate::proxy::RouteAvailability;
use crate::store::AppStore;

pub const PROVISION_SOURCE_ROUTER_MARKET: &str = "router_market";

const HOST_STATUS_IDLE: &str = "idle";
const HOST_STATUS_ALLOCATED: &str = "allocated";
const HOST_STATUS_LOCKED: &str = "locked";
const HOST_STATUS_DRAINING: &str = "draining";
const HOST_STATUS_DISABLED: &str = "disabled";
const HOST_STATUS_UNREACHABLE: &str = "unreachable";
const HOST_STATUS_ABNORMAL: &str = "abnormal";

const HOST_HAS_RUNNING_SERVER_EXIT: i32 = 43;
const MAX_HOST_PROCESS_SKIP_ATTEMPTS: usize = 32;

const JOB_TYPE_CREATE: &str = "create";
const JOB_TYPE_CLEANUP: &str = "cleanup";

const JOB_STATUS_PENDING: &str = "pending";
const JOB_STATUS_RUNNING: &str = "running";
const JOB_STATUS_SUCCEEDED: &str = "succeeded";
const JOB_STATUS_FAILED: &str = "failed";

const JOB_PHASE_PENDING: &str = "pending";
const JOB_PHASE_LOCKED: &str = "locked";
const JOB_PHASE_INSTALLING: &str = "installing";
const JOB_PHASE_WAITING: &str = "waiting_for_client";
const JOB_PHASE_CLEANUP: &str = "cleanup_remote";
const JOB_PHASE_COMPLETE: &str = "complete";
const JOB_PHASE_ROLLBACK: &str = "rollback";

const SUBDOMAIN_RESERVATION_TTL_MS: i64 = 30 * 60 * 1000;
const PROVISION_POLL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const PROVISION_POLL_INTERVAL: Duration = Duration::from_secs(15);
const PROVISION_SECRET_TTL: Duration = Duration::from_secs(15 * 60);
const PROVISION_REDEEM_RETRY_TTL: Duration = Duration::from_secs(2 * 60);
const SSH_VERIFY_TIMEOUT: Duration = Duration::from_secs(45);
const SSH_INSTALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const SSH_CLEANUP_TIMEOUT: Duration = Duration::from_secs(90);
const SSH_OUTPUT_LIMIT: usize = 64 * 1024;
const JOB_LOG_LIMIT: usize = 128 * 1024;
const MAX_SELECTION_ITEMS: usize = 100;
const MAX_NOTE_BYTES: usize = 500;
const MAX_PASSWORD_BYTES: usize = 1024;
const HOST_REGISTRATIONS_PER_OWNER_HOUR: u32 = 20;
const HOST_REGISTRATIONS_PER_TARGET_HOUR: u32 = 5;
const HOST_REGISTRATIONS_PER_SOURCE_HOUR: u32 = 30;

static PROVISION_KNOWN_HOSTS_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug, Clone, Copy)]
enum SshHostKeyPolicy {
    AcceptNew,
    RequireKnown,
}

#[derive(Debug, Clone)]
pub struct ProvisionTokenSecret {
    pub password: String,
    pub owner_email: String,
    pub subdomain: String,
    pub job_id: String,
    pub host_ip: IpAddr,
    expires_at: Instant,
    redeemed_at: Option<Instant>,
}

#[derive(Debug, Default)]
pub struct ClientMarketJobSecrets {
    /// SHA-256 token hash -> one-time provisioning secret. Raw tokens are never retained.
    tokens: HashMap<String, ProvisionTokenSecret>,
    pending_passwords: HashMap<String, (String, Instant)>,
    owner_host_registration_buckets: HashMap<String, (i64, u32)>,
    target_host_registration_buckets: HashMap<String, (i64, u32)>,
    source_host_registration_buckets: HashMap<String, (i64, u32)>,
}

impl ClientMarketJobSecrets {
    fn prune(&mut self) {
        let now = Instant::now();
        self.tokens.retain(|_, secret| {
            secret.expires_at > now
                && secret.redeemed_at.is_none_or(|redeemed| {
                    now.duration_since(redeemed) < PROVISION_REDEEM_RETRY_TTL
                })
        });
        self.pending_passwords
            .retain(|_, (_, expires_at)| *expires_at > now);
    }

    pub fn insert_pending_password(&mut self, job_id: String, password: String) {
        self.prune();
        self.pending_passwords
            .insert(job_id, (password, Instant::now() + PROVISION_SECRET_TTL));
    }

    pub fn take_pending_password(&mut self, job_id: &str) -> Option<String> {
        self.prune();
        self.pending_passwords.remove(job_id).map(|value| value.0)
    }

    pub fn remove_job_secrets(&mut self, job_id: &str) {
        self.pending_passwords.remove(job_id);
        self.tokens.retain(|_, value| value.job_id != job_id);
    }

    pub fn insert_token_hash(&mut self, token_hash: String, secret: ProvisionTokenSecret) {
        self.prune();
        self.tokens.insert(token_hash, secret);
    }

    fn redeem_token(
        &mut self,
        token_hash: &str,
        source_ip: IpAddr,
    ) -> Option<ProvisionTokenSecret> {
        self.prune();
        let secret = self.tokens.get_mut(token_hash)?;
        if secret.host_ip != source_ip {
            return None;
        }
        secret.redeemed_at.get_or_insert_with(Instant::now);
        Some(secret.clone())
    }

    fn allow_host_registration(&mut self, owner: &str, target: IpAddr, source: IpAddr) -> bool {
        let hour = Utc::now().timestamp().div_euclid(3600);
        allow_rate_bucket(
            &mut self.owner_host_registration_buckets,
            owner.to_string(),
            hour,
            HOST_REGISTRATIONS_PER_OWNER_HOUR,
        ) && allow_rate_bucket(
            &mut self.target_host_registration_buckets,
            target.to_string(),
            hour,
            HOST_REGISTRATIONS_PER_TARGET_HOUR,
        ) && allow_rate_bucket(
            &mut self.source_host_registration_buckets,
            source.to_string(),
            hour,
            HOST_REGISTRATIONS_PER_SOURCE_HOUR,
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientMarketSchemaError {
    #[error("client market database error: {0}")]
    Database(#[from] rusqlite::Error),
}

pub fn init_schema(conn: &Connection) -> Result<(), ClientMarketSchemaError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS router_ssh_hosts (
            id TEXT PRIMARY KEY,
            ip TEXT NOT NULL,
            port INTEGER NOT NULL,
            host_owner_email TEXT NOT NULL,
            country_code TEXT,
            hostname TEXT,
            ssh_host_key_fingerprint TEXT,
            status TEXT NOT NULL,
            installation_id TEXT,
            last_verified_at TEXT,
            last_error TEXT,
            note TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE(ip, port)
        );
        CREATE INDEX IF NOT EXISTS idx_router_ssh_hosts_supply
            ON router_ssh_hosts(host_owner_email, status, country_code);

        CREATE TABLE IF NOT EXISTS provisioning_jobs (
            id TEXT PRIMARY KEY,
            type TEXT NOT NULL,
            host_id TEXT,
            host_owner_email TEXT,
            client_owner_email TEXT,
            selection_owners_json TEXT,
            selection_regions_json TEXT,
            subdomain TEXT,
            installation_id TEXT,
            status TEXT NOT NULL,
            phase TEXT NOT NULL DEFAULT 'pending',
            log_blob TEXT NOT NULL DEFAULT '',
            secret_ref TEXT,
            failure_code TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_provisioning_jobs_client
            ON provisioning_jobs(client_owner_email, status, updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_provisioning_jobs_host
            ON provisioning_jobs(host_id, status);

        CREATE TABLE IF NOT EXISTS subdomain_reservations (
            subdomain TEXT PRIMARY KEY COLLATE NOCASE,
            job_id TEXT NOT NULL,
            host_id TEXT,
            client_owner_email TEXT,
            installation_id TEXT,
            expires_at_ms INTEGER NOT NULL
        );",
    )?;
    add_column_if_missing(
        conn,
        "provisioning_jobs",
        "phase",
        "TEXT NOT NULL DEFAULT 'pending'",
    )?;
    add_column_if_missing(conn, "provisioning_jobs", "failure_code", "TEXT")?;
    add_column_if_missing(conn, "subdomain_reservations", "host_id", "TEXT")?;
    add_column_if_missing(conn, "subdomain_reservations", "client_owner_email", "TEXT")?;
    add_column_if_missing(conn, "subdomain_reservations", "installation_id", "TEXT")?;
    conn.execute(
        "UPDATE router_ssh_hosts SET status = ?1 WHERE status = 'provisioning'",
        params![HOST_STATUS_LOCKED],
    )?;
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_router_ssh_hosts_installation
            ON router_ssh_hosts(installation_id)
            WHERE installation_id IS NOT NULL;
         CREATE UNIQUE INDEX IF NOT EXISTS idx_provisioning_jobs_active_host
            ON provisioning_jobs(host_id)
            WHERE host_id IS NOT NULL AND status IN ('pending', 'running');
         CREATE INDEX IF NOT EXISTS idx_subdomain_reservations_job
            ON subdomain_reservations(job_id);
         CREATE UNIQUE INDEX IF NOT EXISTS idx_subdomain_reservations_installation
            ON subdomain_reservations(installation_id)
            WHERE installation_id IS NOT NULL;",
    )?;
    Ok(())
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), rusqlite::Error> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !columns.iter().any(|existing| existing == column) {
        conn.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition}"
        ))?;
    }
    Ok(())
}

#[derive(Debug)]
struct ActiveSubdomainReservation {
    job_id: String,
    host_id: Option<String>,
    client_owner_email: Option<String>,
    installation_id: Option<String>,
}

fn get_active_subdomain_reservation(
    conn: &Connection,
    subdomain: &str,
) -> Result<Option<ActiveSubdomainReservation>, AppError> {
    conn.execute(
        "DELETE FROM subdomain_reservations
         WHERE expires_at_ms <= ?1 AND installation_id IS NULL",
        params![Utc::now().timestamp_millis()],
    )
    .map_err(|e| AppError::Internal(format!("expire subdomain reservations failed: {e}")))?;
    conn.query_row(
        "SELECT job_id, host_id, client_owner_email, installation_id
         FROM subdomain_reservations
         WHERE subdomain = ?1 COLLATE NOCASE",
        params![subdomain],
        |row| {
            Ok(ActiveSubdomainReservation {
                job_id: row.get(0)?,
                host_id: row.get(1)?,
                client_owner_email: row.get(2)?,
                installation_id: row.get(3)?,
            })
        },
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("read subdomain reservation failed: {e}")))
}

fn reservation_source_matches_host(
    conn: &Connection,
    reservation: &ActiveSubdomainReservation,
    source_ip: Option<&str>,
) -> Result<bool, AppError> {
    let Some(host_id) = reservation.host_id.as_deref() else {
        return Ok(false);
    };
    let Some(source_ip) = source_ip.and_then(|value| value.parse::<IpAddr>().ok()) else {
        return Ok(false);
    };
    let host_ip: Option<String> = conn
        .query_row(
            "SELECT ip FROM router_ssh_hosts WHERE id = ?1",
            params![host_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| AppError::Internal(format!("read reservation host failed: {e}")))?;
    Ok(host_ip
        .and_then(|value| value.parse::<IpAddr>().ok())
        .is_some_and(|value| value == source_ip))
}

/// Apply the global Client Market reservation to the public availability API.
/// Calls from the selected host are allowed through so the setup preflight can run;
/// every other installation sees the label as unavailable.
pub(crate) fn client_market_subdomain_available_to_source(
    conn: &Connection,
    subdomain: &str,
    installation_id: Option<&str>,
    source_ip: Option<&str>,
) -> Result<bool, AppError> {
    let Some(reservation) = get_active_subdomain_reservation(conn, subdomain)? else {
        return Ok(true);
    };
    if !reservation_source_matches_host(conn, &reservation, source_ip)? {
        return Ok(false);
    }
    Ok(match reservation.installation_id.as_deref() {
        Some(bound) => installation_id.is_some_and(|candidate| candidate == bound),
        None => true,
    })
}

/// Authorize and atomically bind a reserved label during tunnel claim. This must be
/// called inside the same transaction that creates the public-host/tunnel rows.
pub(crate) fn authorize_client_market_subdomain_claim(
    conn: &Connection,
    subdomain: &str,
    installation_id: &str,
    owner_email: &str,
    source_ip: Option<&str>,
) -> Result<(), AppError> {
    let Some(reservation) = get_active_subdomain_reservation(conn, subdomain)? else {
        return Ok(());
    };
    if reservation.client_owner_email.as_deref() != Some(owner_email)
        || !reservation_source_matches_host(conn, &reservation, source_ip)?
    {
        return Err(AppError::Conflict(
            "subdomain is reserved for another provisioning job".into(),
        ));
    }
    if let Some(bound) = reservation.installation_id.as_deref() {
        if bound == installation_id {
            return Ok(());
        }
        return Err(AppError::Conflict(
            "subdomain reservation is already bound to another installation".into(),
        ));
    }
    let host_id = reservation
        .host_id
        .as_deref()
        .ok_or_else(|| AppError::Conflict("subdomain reservation has no selected host".into()))?;
    let active_job: Option<(String, String)> = conn
        .query_row(
            "SELECT status, phase FROM provisioning_jobs
             WHERE id = ?1 AND host_id = ?2 AND type = 'create'",
            params![reservation.job_id, host_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| AppError::Internal(format!("read reservation job failed: {e}")))?;
    if !active_job.is_some_and(|(status, phase)| {
        status == JOB_STATUS_RUNNING
            && matches!(
                phase.as_str(),
                JOB_PHASE_LOCKED | JOB_PHASE_INSTALLING | JOB_PHASE_WAITING
            )
    }) {
        return Err(AppError::Conflict(
            "subdomain provisioning job is no longer active".into(),
        ));
    }
    let bound = conn
        .execute(
            "UPDATE subdomain_reservations
             SET installation_id = ?2, expires_at_ms = ?3
             WHERE job_id = ?1 AND installation_id IS NULL",
            params![
                reservation.job_id,
                installation_id,
                Utc::now().timestamp_millis() + SUBDOMAIN_RESERVATION_TTL_MS,
            ],
        )
        .map_err(|e| AppError::Internal(format!("bind subdomain reservation failed: {e}")))?;
    if bound != 1 {
        return Err(AppError::Conflict(
            "subdomain reservation binding raced".into(),
        ));
    }
    let now = Utc::now().to_rfc3339();
    let tagged = conn
        .execute(
            "UPDATE installations
             SET provision_source = ?2, provision_host_id = ?3
             WHERE id = ?1",
            params![installation_id, PROVISION_SOURCE_ROUTER_MARKET, host_id],
        )
        .map_err(|e| AppError::Internal(format!("tag provisioned installation failed: {e}")))?;
    if tagged != 1 {
        return Err(AppError::NotFound(
            "reserved installation was not found".into(),
        ));
    }
    let host_bound = conn
        .execute(
            "UPDATE router_ssh_hosts
             SET installation_id = ?2, updated_at = ?3
             WHERE id = ?1 AND status = 'locked' AND installation_id IS NULL",
            params![host_id, installation_id, now],
        )
        .map_err(|e| {
            AppError::Internal(format!("bind installation to provision host failed: {e}"))
        })?;
    if host_bound != 1 {
        return Err(AppError::Conflict(
            "provision host installation binding raced".into(),
        ));
    }
    let job_bound = conn
        .execute(
            "UPDATE provisioning_jobs
             SET installation_id = ?2, phase = ?3, updated_at = ?4
             WHERE id = ?1 AND type = 'create' AND host_id = ?5
               AND status = 'running' AND installation_id IS NULL
               AND phase IN ('locked', 'installing', 'waiting_for_client')",
            params![
                reservation.job_id,
                installation_id,
                JOB_PHASE_WAITING,
                now,
                host_id,
            ],
        )
        .map_err(|e| AppError::Internal(format!("bind installation to job failed: {e}")))?;
    if job_bound != 1 {
        return Err(AppError::Conflict(
            "provisioning job installation binding raced".into(),
        ));
    }
    Ok(())
}

pub fn known_hosts_path(config: &Config) -> PathBuf {
    config
        .db_path
        .parent()
        .map(|dir| dir.join("client_market_ssh_known_hosts"))
        .unwrap_or_else(|| PathBuf::from("./data/client_market_ssh_known_hosts"))
}

pub fn router_public_url(config: &Config) -> String {
    let scheme = if config.use_localhost {
        "http"
    } else {
        "https"
    };
    format!("{scheme}://{}", config.tunnel_domain.trim_end_matches('/'))
}

fn client_public_url(config: &Config, subdomain: &str) -> String {
    let scheme = if config.use_localhost {
        "http"
    } else {
        "https"
    };
    format!(
        "{scheme}://{subdomain}.{}",
        config.tunnel_domain.trim_end_matches('/')
    )
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route(
            "/v1/client-market/provision-ssh-key",
            get(get_provision_ssh_key),
        )
        .route("/v1/client-market/hosts", get(list_hosts).post(create_host))
        .route("/v1/client-market/supply-summary", get(supply_summary))
        .route("/v1/client-market/hosts/:id", delete(delete_host))
        .route("/v1/client-market/hosts/:id/reverify", post(reverify_host))
        .route("/v1/client-market/clients", post(create_client))
        .route("/v1/client-market/jobs/:id", get(get_job))
        .route(
            "/v1/client-market/clients/:installation_id/cleanup",
            post(cleanup_client),
        )
        .route(
            "/v1/client-market/provision-tokens/redeem",
            post(redeem_provision_token),
        )
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProvisionSshKeyResponse {
    public_key: String,
    authorized_keys_line: String,
}

async fn get_provision_ssh_key(
    State(state): State<ServerState>,
) -> Result<Json<ProvisionSshKeyResponse>, AppError> {
    Ok(Json(ProvisionSshKeyResponse {
        public_key: state.provision_ssh_public_key.clone(),
        authorized_keys_line: state.provision_ssh_authorized_keys_line.clone(),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListHostsQuery {
    owner_email: Option<String>,
    country: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RouterSshHostView {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    host_owner_email: String,
    country_code: Option<String>,
    hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ssh_host_key_fingerprint: Option<String>,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_subdomain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_owner_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    installation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_verified_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<String>,
}

async fn list_hosts(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ListHostsQuery>,
) -> Result<Json<Vec<RouterSshHostView>>, AppError> {
    let viewer = extract_optional_session_email(&state, &headers).await?;
    let is_admin = if let Some(ref email) = viewer {
        state.dynamic.read().await.is_admin(email)
    } else {
        false
    };
    let hosts = state
        .store
        .client_market_list_hosts(
            query.owner_email.as_deref(),
            query.country.as_deref(),
            query.status.as_deref(),
        )
        .await?;
    let views = hosts
        .into_iter()
        .map(|host| {
            let reveal_operations = is_admin
                || viewer
                    .as_deref()
                    .is_some_and(|v| v.eq_ignore_ascii_case(&host.host_owner_email));
            let is_client_owner = viewer.as_deref().is_some_and(|value| {
                host.client_owner_email
                    .as_deref()
                    .is_some_and(|owner| value.eq_ignore_ascii_case(owner))
            });
            let reveal_installation = reveal_operations || is_client_owner;
            RouterSshHostView {
                id: host.id,
                ip: reveal_operations.then_some(host.ip),
                port: reveal_operations.then_some(host.port),
                host_owner_email: host.host_owner_email,
                country_code: host.country_code,
                hostname: host.hostname,
                ssh_host_key_fingerprint: reveal_operations
                    .then_some(host.ssh_host_key_fingerprint)
                    .flatten(),
                status: host.status,
                client_subdomain: host.client_subdomain,
                client_owner_email: reveal_installation
                    .then_some(host.client_owner_email)
                    .flatten(),
                installation_id: reveal_installation
                    .then_some(host.installation_id)
                    .flatten(),
                last_verified_at: reveal_operations.then_some(host.last_verified_at).flatten(),
                last_error: reveal_operations.then_some(host.last_error).flatten(),
                note: reveal_operations.then_some(host.note).flatten(),
                created_at: reveal_operations.then_some(host.created_at),
                updated_at: reveal_operations.then_some(host.updated_at),
            }
        })
        .collect();
    Ok(Json(views))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupplySummaryEntry {
    pub host_owner_email: String,
    pub country_code: Option<String>,
    pub idle_count: i64,
    pub total_count: i64,
}

async fn supply_summary(
    State(state): State<ServerState>,
) -> Result<Json<Vec<SupplySummaryEntry>>, AppError> {
    Ok(Json(state.store.client_market_supply_summary().await?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateHostRequest {
    ip: String,
    port: Option<u16>,
    note: Option<String>,
}

async fn create_host(
    State(state): State<ServerState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<CreateHostRequest>,
) -> Result<Json<RouterSshHostView>, AppError> {
    let owner = require_session_email(&state, &headers).await?;
    let ip = parse_host_ip(&input.ip)?;
    let source_ip = crate::client_meta::extract_client_metadata(&headers, addr)
        .ip
        .as_deref()
        .and_then(|value| value.parse::<IpAddr>().ok())
        .unwrap_or_else(|| addr.ip());
    let port = input.port.unwrap_or(22);
    if port == 0 {
        return Err(AppError::BadRequest(
            "ssh port must be greater than zero".into(),
        ));
    }
    if input
        .note
        .as_ref()
        .is_some_and(|note| note.len() > MAX_NOTE_BYTES)
    {
        return Err(AppError::BadRequest(
            "host note cannot exceed 500 bytes".into(),
        ));
    }
    if !state
        .client_market_job_secrets
        .lock()
        .await
        .allow_host_registration(&owner, ip, source_ip)
    {
        return Err(AppError::TooManyRequests(
            "host verification rate limit exceeded".into(),
        ));
    }
    let (hostname, fingerprint) = ssh_verify_host(
        &state,
        &ip.to_string(),
        port,
        &known_hosts_path(&state.config),
    )
    .await?;
    let country_code = state
        .store
        .lookup_geo_country_code_for_ip(&ip.to_string())
        .await
        .filter(|code| code.len() == 2)
        .ok_or_else(|| {
            AppError::ServiceUnavailable("could not determine host country; retry later".into())
        })?;
    let host = state
        .store
        .client_market_insert_host(
            &owner,
            &ip.to_string(),
            port,
            Some(&country_code),
            hostname.as_deref(),
            fingerprint.as_deref(),
            input.note.as_deref(),
        )
        .await?;
    Ok(Json(host_to_view(host, true)))
}

async fn reverify_host(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<RouterSshHostView>, AppError> {
    let viewer = require_session_email(&state, &headers).await?;
    let is_admin = state.dynamic.read().await.is_admin(&viewer);
    let host = state
        .store
        .client_market_get_host_for_operator(&id, &viewer, is_admin)
        .await?;
    if host.installation_id.is_some() && !is_admin {
        return Err(AppError::Conflict(
            "host still has an installation; retry client cleanup instead".into(),
        ));
    }
    if !matches!(
        host.status.as_str(),
        HOST_STATUS_UNREACHABLE
            | HOST_STATUS_DISABLED
            | HOST_STATUS_IDLE
            | HOST_STATUS_ABNORMAL
    ) {
        return Err(AppError::Conflict(
            "host cannot be reverified in its current state".into(),
        ));
    }
    let (hostname, fingerprint) = ssh_verify_host(
        &state,
        &host.ip,
        host.port,
        &known_hosts_path(&state.config),
    )
    .await?;
    if host
        .ssh_host_key_fingerprint
        .as_deref()
        .is_some_and(|expected| {
            fingerprint
                .as_deref()
                .is_none_or(|actual| actual != expected)
        })
    {
        return Err(AppError::Conflict(
            "ssh host key fingerprint changed; operator intervention is required".into(),
        ));
    }
    if let Some(installation_id) = host.installation_id.as_deref() {
        if let Some(subdomain) = state
            .store
            .client_market_subdomain_for_installation(installation_id)
            .await?
        {
            state.proxy.remove_route(&subdomain).await;
        }
        state
            .store
            .purge_installation_for_client_market(installation_id)
            .await?;
    }
    let updated = state
        .store
        .client_market_complete_host_reverify(&id, hostname.as_deref(), fingerprint.as_deref())
        .await?;
    Ok(Json(host_to_view(updated, true)))
}

async fn delete_host(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let viewer = require_session_email(&state, &headers).await?;
    let is_admin = state.dynamic.read().await.is_admin(&viewer);
    state
        .store
        .client_market_delete_host(&id, &viewer, is_admin)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateClientRequest {
    host_owner_emails: Vec<String>,
    country_codes: Vec<String>,
    subdomain: String,
    password: String,
    count: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateClientResponse {
    job_id: String,
}

async fn create_client(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<CreateClientRequest>,
) -> Result<Json<CreateClientResponse>, AppError> {
    let client_owner = require_session_email(&state, &headers).await?;
    if input.count.unwrap_or(1) != 1 {
        return Err(AppError::BadRequest(
            "only a single client (count=1) is supported".into(),
        ));
    }
    let subdomain = normalize_client_subdomain(&input.subdomain)
        .map_err(|message| AppError::BadRequest(message.into()))?;
    let password_len = input.password.chars().count();
    if password_len < 8
        || input.password.len() > MAX_PASSWORD_BYTES
        || input.password.chars().any(char::is_control)
    {
        return Err(AppError::BadRequest(
            "password must be at least 8 characters, at most 1024 bytes, and contain no control characters".into(),
        ));
    }
    if input.host_owner_emails.is_empty() || input.country_codes.is_empty() {
        return Err(AppError::BadRequest(
            "hostOwnerEmails and countryCodes are required".into(),
        ));
    }
    let job_id = Uuid::new_v4().to_string();
    state
        .store
        .client_market_create_job(
            &job_id,
            JOB_TYPE_CREATE,
            &client_owner,
            &input.host_owner_emails,
            &input.country_codes,
            &subdomain,
            None,
        )
        .await?;
    {
        let mut secrets = state.client_market_job_secrets.lock().await;
        secrets.insert_pending_password(job_id.clone(), input.password);
    }
    let runner_state = state.clone();
    let response_job_id = job_id.clone();
    let spawn_job_id = job_id.clone();
    tokio::spawn(async move {
        if let Err(err) = run_create_job(runner_state, spawn_job_id).await {
            error!(job_id = %response_job_id, error = %err, "client market create job failed");
        }
    });
    Ok(Json(CreateClientResponse { job_id }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    pub id: String,
    pub job_type: String,
    pub host_id: Option<String>,
    pub host_owner_email: Option<String>,
    pub client_owner_email: Option<String>,
    pub subdomain: Option<String>,
    pub installation_id: Option<String>,
    pub status: String,
    pub phase: String,
    pub failure_code: Option<String>,
    pub country_code: Option<String>,
    pub client_url: Option<String>,
    pub log: String,
    pub created_at: String,
    pub updated_at: String,
}

async fn get_job(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<JobView>, AppError> {
    let viewer = require_session_email(&state, &headers).await?;
    let is_admin = state.dynamic.read().await.is_admin(&viewer);
    let mut job = state
        .store
        .client_market_get_job_for_viewer(&id, &viewer, is_admin)
        .await?;
    job.client_url = job
        .subdomain
        .as_deref()
        .map(|subdomain| client_public_url(&state.config, subdomain));
    Ok(Json(job))
}

async fn cleanup_client(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(installation_id): AxumPath<String>,
) -> Result<Json<CreateClientResponse>, AppError> {
    let viewer = require_session_email(&state, &headers).await?;
    let is_admin = state.dynamic.read().await.is_admin(&viewer);
    let job_id = state
        .store
        .client_market_begin_cleanup_job(&installation_id, &viewer, is_admin)
        .await?;
    let runner_state = state.clone();
    let response_job_id = job_id.clone();
    let spawn_job_id = job_id.clone();
    tokio::spawn(async move {
        if let Err(err) = run_cleanup_job(runner_state, spawn_job_id).await {
            error!(job_id = %response_job_id, error = %err, "client market cleanup job failed");
        }
    });
    Ok(Json(CreateClientResponse { job_id }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProvisionTokenResponse {
    router_url: String,
    owner_email: String,
    password: String,
    subdomain: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RedeemProvisionTokenRequest {
    token: String,
}

async fn redeem_provision_token(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(input): Json<RedeemProvisionTokenRequest>,
) -> Result<
    (
        [(header::HeaderName, &'static str); 1],
        Json<ProvisionTokenResponse>,
    ),
    AppError,
> {
    if input.token.len() < 32 || input.token.len() > 256 {
        return Err(AppError::NotFound(
            "provision token not found or expired".into(),
        ));
    }
    let token_hash = provision_token_hash(&input.token);
    let metadata = crate::client_meta::extract_client_metadata(&headers, addr);
    let source_ip = metadata
        .ip
        .as_deref()
        .and_then(|value| value.parse::<IpAddr>().ok())
        .ok_or_else(|| AppError::Unauthorized("provision token source is unavailable".into()))?;
    let secret = {
        let mut secrets = state.client_market_job_secrets.lock().await;
        secrets.redeem_token(&token_hash, source_ip)
    }
    .ok_or_else(|| AppError::NotFound("provision token not found or expired".into()))?;
    state
        .store
        .client_market_validate_token_redemption(
            &secret.job_id,
            &token_hash,
            &source_ip.to_string(),
        )
        .await?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(ProvisionTokenResponse {
            router_url: router_public_url(&state.config),
            owner_email: secret.owner_email,
            password: secret.password,
            subdomain: secret.subdomain,
        }),
    ))
}

async fn run_create_job(state: ServerState, job_id: String) -> Result<(), AppError> {
    let result = run_create_job_inner(&state, &job_id).await;
    if let Err(ref error) = result {
        handle_create_job_failure(&state, &job_id, error).await;
    }
    result
}

pub async fn reconcile_interrupted_jobs(state: ServerState) -> Result<(), AppError> {
    let jobs = state.store.client_market_interrupted_jobs().await?;
    if jobs.is_empty() {
        return Ok(());
    }
    info!(
        count = jobs.len(),
        "reconciling interrupted client market jobs"
    );
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
    for job in jobs {
        let runner_state = state.clone();
        let semaphore = semaphore.clone();
        tokio::spawn(async move {
            let Ok(_permit) = semaphore.acquire_owned().await else {
                return;
            };
            match job.job_type.as_str() {
                JOB_TYPE_CREATE => resume_interrupted_create_job(&runner_state, job).await,
                JOB_TYPE_CLEANUP => {
                    let job_id = job.id.clone();
                    let result = run_cleanup_job_inner(
                        &runner_state,
                        &job_id,
                        job.status == JOB_STATUS_RUNNING,
                    )
                    .await;
                    if let Err(ref error) = result {
                        handle_cleanup_job_failure(&runner_state, &job_id, error).await;
                    }
                }
                _ => {
                    let _ = runner_state
                        .store
                        .client_market_fail_job(
                            &job.id,
                            "interrupted job has an unsupported type\n",
                        )
                        .await;
                }
            }
        });
    }
    Ok(())
}

async fn resume_interrupted_create_job(state: &ServerState, job: ProvisioningJobRecord) {
    if job.status == JOB_STATUS_RUNNING
        && job.phase == JOB_PHASE_WAITING
        && let (Some(host_id), Some(installation_id), Some(subdomain)) = (
            job.host_id.as_deref(),
            job.installation_id.as_deref(),
            job.subdomain.as_deref(),
        )
    {
        let resumed = async {
            let ready_id = poll_for_installation(
                state,
                &job.id,
                subdomain,
                PROVISION_POLL_TIMEOUT,
                PROVISION_POLL_INTERVAL,
            )
            .await?;
            if ready_id != installation_id {
                return Err(AppError::Conflict(
                    "interrupted job installation binding changed".into(),
                ));
            }
            state
                .store
                .client_market_complete_create_job(
                    &job.id,
                    host_id,
                    installation_id,
                    PROVISION_SOURCE_ROUTER_MARKET,
                )
                .await
        }
        .await;
        if resumed.is_ok() {
            let _ = state
                .store
                .client_market_append_job_log(
                    &job.id,
                    "provisioning recovered after router restart\n",
                )
                .await;
            return;
        }
    }
    let error = AppError::ServiceUnavailable(
        "provisioning was interrupted before it could safely resume".into(),
    );
    handle_create_job_failure(state, &job.id, &error).await;
}

async fn run_create_job_inner(state: &ServerState, job_id: &str) -> Result<(), AppError> {
    state
        .store
        .client_market_append_job_log(job_id, "starting provisioning job\n")
        .await?;
    state
        .store
        .client_market_start_job(job_id, JOB_TYPE_CREATE)
        .await?;
    let job = state
        .store
        .client_market_get_job_record(job_id)
        .await?
        .ok_or_else(|| AppError::NotFound("job not found".into()))?;
    let host = claim_idle_host_without_running_server(state, job_id, &job).await?;
    state
        .store
        .client_market_append_job_log(job_id, "reserved one matching host\n")
        .await?;
    require_pinned_host_fingerprint(&host, &known_hosts_path(&state.config)).await?;
    let password = state
        .client_market_job_secrets
        .lock()
        .await
        .take_pending_password(job_id)
        .ok_or_else(|| {
            AppError::ServiceUnavailable(
                "provisioning secret expired or was lost during router restart".into(),
            )
        })?;
    let token = new_provision_token();
    let token_hash = provision_token_hash(&token);
    let host_ip = host
        .ip
        .parse::<IpAddr>()
        .map_err(|_| AppError::Internal("selected host has an invalid IP address".into()))?;
    state
        .store
        .client_market_activate_token(job_id, &token_hash)
        .await?;
    {
        state
            .client_market_job_secrets
            .lock()
            .await
            .insert_token_hash(
                token_hash.clone(),
                ProvisionTokenSecret {
                    password,
                    owner_email: job.client_owner_email.clone().unwrap_or_default(),
                    subdomain: job.subdomain.clone().unwrap_or_default(),
                    job_id: job_id.to_string(),
                    host_ip,
                    expires_at: Instant::now() + PROVISION_SECRET_TTL,
                    redeemed_at: None,
                },
            );
    }
    let router_url = router_public_url(&state.config);
    let ip_family = if host_ip.is_ipv4() { "4" } else { "6" };
    let install_cmd = format!(
        "set -eu; script=$(mktemp); trap 'rm -f \"$script\"' EXIT; \
         curl --fail --silent --show-error --location --max-time 120 {} -o \"$script\"; \
         CC_SWITCH_PROVISION_IP_FAMILY={} bash \"$script\" --provision-token-stdin {} disableWebTerminal",
        shell_quote(&format!("{router_url}/install-client.sh")),
        ip_family,
        shell_quote(&router_url),
    );
    let install_result = ssh_run_remote_with_input(
        &state.provision_ssh_key_path,
        &known_hosts_path(&state.config),
        &host.ip,
        host.port,
        &install_cmd,
        Some(format!("{token}\n").into_bytes()),
        SSH_INSTALL_TIMEOUT,
        SshHostKeyPolicy::RequireKnown,
    )
    .await;
    state
        .client_market_job_secrets
        .lock()
        .await
        .remove_job_secrets(job_id);
    let install_output = install_result?;
    if !install_output.trim().is_empty() {
        state
            .store
            .client_market_append_job_log(
                job_id,
                &format!("remote installer output:\n{install_output}"),
            )
            .await?;
    }
    state.store.client_market_finish_installer(job_id).await?;
    state
        .store
        .client_market_append_job_log(job_id, "remote installer completed; waiting for tunnel\n")
        .await?;
    let subdomain = job.subdomain.clone().unwrap_or_default();
    let installation_id = poll_for_installation(
        state,
        job_id,
        &subdomain,
        PROVISION_POLL_TIMEOUT,
        PROVISION_POLL_INTERVAL,
    )
    .await?;
    state
        .store
        .client_market_complete_create_job(
            job_id,
            &host.id,
            &installation_id,
            PROVISION_SOURCE_ROUTER_MARKET,
        )
        .await?;
    state
        .store
        .client_market_append_job_log(
            job_id,
            "client tunnel is online and provisioning is complete\n",
        )
        .await?;
    info!(
        job_id = %job_id,
        host_id = %host.id,
        installation_id = %installation_id,
        "client market provisioning succeeded"
    );
    Ok(())
}

async fn handle_create_job_failure(state: &ServerState, job_id: &str, error: &AppError) {
    warn!(job_id = %job_id, error = %error, "rolling back failed client market provisioning");
    state
        .client_market_job_secrets
        .lock()
        .await
        .remove_job_secrets(job_id);
    let Ok(Some(job)) = state.store.client_market_get_job_record(job_id).await else {
        return;
    };
    if matches!(
        job.status.as_str(),
        JOB_STATUS_SUCCEEDED | JOB_STATUS_FAILED
    ) {
        return;
    }
    let _ = state.store.client_market_mark_rollback(job_id).await;
    let mut release_to_idle = job.host_id.is_none();
    if let Some(host_id) = job.host_id.as_deref() {
        match state.store.client_market_get_host(host_id).await {
            Ok(Some(host)) => match ssh_cleanup_remote(state, &host).await {
                Ok(()) => {
                    let installation = match job
                        .installation_id
                        .clone()
                        .or_else(|| host.installation_id.clone())
                    {
                        Some(installation_id) => Ok(Some(installation_id)),
                        None => state.store.client_market_bound_installation(job_id).await,
                    };
                    release_to_idle = match installation {
                        Ok(Some(installation_id)) => {
                            if let Some(subdomain) = job
                                .subdomain
                                .as_deref()
                                .or(host.client_subdomain.as_deref())
                            {
                                state.proxy.remove_route(subdomain).await;
                            }
                            match state
                                .store
                                .purge_installation_for_client_market(&installation_id)
                                .await
                            {
                                Ok(()) => true,
                                Err(purge_error) => {
                                    warn!(
                                        job_id = %job_id,
                                        installation_id = %installation_id,
                                        error = %purge_error,
                                        "failed to purge installation during provisioning rollback"
                                    );
                                    false
                                }
                            }
                        }
                        Ok(None) => true,
                        Err(lookup_error) => {
                            warn!(
                                job_id = %job_id,
                                error = %lookup_error,
                                "failed to resolve installation during provisioning rollback"
                            );
                            false
                        }
                    };
                }
                Err(cleanup_error) => {
                    warn!(
                        job_id = %job_id,
                        host_id = %host_id,
                        error = %cleanup_error,
                        "remote provisioning rollback failed"
                    );
                }
            },
            Ok(None) => {
                warn!(job_id = %job_id, host_id = %host_id, "provisioning rollback host disappeared");
            }
            Err(host_error) => {
                warn!(job_id = %job_id, host_id = %host_id, error = %host_error, "failed to load provisioning rollback host");
            }
        }
    }
    let message = if release_to_idle {
        "provisioning failed; remote rollback completed\n"
    } else {
        "provisioning failed; host requires operator verification before reuse\n"
    };
    if let Err(finalize_error) = state
        .store
        .client_market_finalize_create_failure(
            job_id,
            job.host_id.as_deref(),
            release_to_idle,
            "provisioning_failed",
            message,
        )
        .await
    {
        error!(job_id = %job_id, error = %finalize_error, "failed to persist provisioning rollback");
    }
}

async fn run_cleanup_job(state: ServerState, job_id: String) -> Result<(), AppError> {
    let result = run_cleanup_job_inner(&state, &job_id, false).await;
    if let Err(ref error) = result {
        handle_cleanup_job_failure(&state, &job_id, error).await;
    }
    result
}

async fn run_cleanup_job_inner(
    state: &ServerState,
    job_id: &str,
    resume_running: bool,
) -> Result<(), AppError> {
    state
        .store
        .client_market_append_job_log(job_id, "starting cleanup job\n")
        .await?;
    if !resume_running {
        state
            .store
            .client_market_start_job(job_id, JOB_TYPE_CLEANUP)
            .await?;
    }
    let job = state
        .store
        .client_market_get_job_record(job_id)
        .await?
        .ok_or_else(|| AppError::NotFound("job not found".into()))?;
    if job.job_type != JOB_TYPE_CLEANUP
        || job.status != JOB_STATUS_RUNNING
        || job.phase != JOB_PHASE_CLEANUP
    {
        return Err(AppError::Conflict(
            "cleanup job is not runnable in its current state".into(),
        ));
    }
    let host_id = job
        .host_id
        .clone()
        .ok_or_else(|| AppError::Internal("cleanup job missing host".into()))?;
    let host = state
        .store
        .client_market_get_host(&host_id)
        .await?
        .ok_or_else(|| AppError::NotFound("host not found".into()))?;
    if host.status != HOST_STATUS_DRAINING {
        return Err(AppError::Conflict("cleanup host is not draining".into()));
    }
    ssh_cleanup_remote(state, &host).await?;
    state
        .store
        .client_market_append_job_log(job_id, "remote client files removed\n")
        .await?;
    let installation_id = job
        .installation_id
        .as_deref()
        .ok_or_else(|| AppError::Internal("cleanup job missing installation".into()))?;
    if let Some(subdomain) = job.subdomain.as_deref() {
        state.proxy.remove_route(subdomain).await;
    }
    state
        .store
        .purge_installation_for_client_market(installation_id)
        .await?;
    state
        .store
        .client_market_finish_cleanup_job(job_id, &host_id)
        .await?;
    Ok(())
}

async fn handle_cleanup_job_failure(state: &ServerState, job_id: &str, error: &AppError) {
    warn!(job_id = %job_id, error = %error, "client market cleanup failed");
    let Ok(Some(job)) = state.store.client_market_get_job_record(job_id).await else {
        return;
    };
    if matches!(
        job.status.as_str(),
        JOB_STATUS_SUCCEEDED | JOB_STATUS_FAILED
    ) {
        return;
    }
    let Some(host_id) = job.host_id.as_deref() else {
        let _ = state
            .store
            .client_market_fail_job(job_id, "cleanup failed before a host was resolved\n")
            .await;
        return;
    };
    if let Err(finalize_error) = state
        .store
        .client_market_fail_cleanup_job(
            job_id,
            host_id,
            "cleanup_failed",
            "cleanup failed; host remains unavailable until operator verification\n",
        )
        .await
    {
        error!(job_id = %job_id, error = %finalize_error, "failed to persist cleanup failure");
    }
}

async fn poll_for_installation(
    state: &ServerState,
    job_id: &str,
    subdomain: &str,
    timeout: Duration,
    interval: Duration,
) -> Result<String, AppError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(id) = state.store.client_market_ready_installation(job_id).await? {
            let route_online = state
                .proxy
                .route_availability(subdomain, Duration::ZERO)
                .await
                .is_some_and(|snapshot| snapshot.state == RouteAvailability::Active);
            if route_online {
                return Ok(id);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(AppError::ServiceUnavailable(
                "timed out waiting for client installation".into(),
            ));
        }
        tokio::time::sleep(interval).await;
    }
}

async fn ssh_verify_host(
    state: &ServerState,
    ip: &str,
    port: u16,
    known_hosts: &Path,
) -> Result<(Option<String>, Option<String>), AppError> {
    let _known_hosts_guard = PROVISION_KNOWN_HOSTS_LOCK.lock().await;
    // Reject hosts that already run cc-switch-server (same check used when claiming).
    if ssh_host_has_running_cc_switch_server(state, ip, port, known_hosts, SshHostKeyPolicy::AcceptNew)
        .await?
    {
        return Err(AppError::Conflict(
            "host is already running cc-switch-server; stop the process before adding it to Client Market"
                .into(),
        ));
    }
    let output = ssh_run_remote_with_input(
        &state.provision_ssh_key_path,
        known_hosts,
        ip,
        port,
        "set -eu; command -v pgrep >/dev/null; command -v curl >/dev/null; \
             command -v bash >/dev/null; command -v python3 >/dev/null; \
             if [ -e /usr/local/bin/cc-switch-server ] || [ -e \"$HOME/.cc-switch-server\" ]; then \
             echo 'host already contains a cc-switch-server installation' >&2; exit 42; fi; \
             if find \"$HOME\" -maxdepth 1 -name '.cc-switch-server.bak.*' -print -quit | grep -q .; then \
             echo 'host contains a cc-switch-server backup' >&2; exit 42; fi; hostname",
        None,
        SSH_VERIFY_TIMEOUT,
        SshHostKeyPolicy::AcceptNew,
    )
    .await?;
    let hostname = output
        .lines()
        .next()
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 253
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        })
        .ok_or_else(|| AppError::BadRequest("ssh hostname response was invalid".into()))?;
    let fingerprint = ssh_fetch_host_fingerprint(ip, port, known_hosts).await?;
    Ok((Some(hostname.to_string()), Some(fingerprint)))
}

/// `ps -ef | grep cc-switch-server | grep -v grep` — exit 0 means a process is running.
async fn ssh_host_has_running_cc_switch_server(
    state: &ServerState,
    ip: &str,
    port: u16,
    known_hosts: &Path,
    host_key_policy: SshHostKeyPolicy,
) -> Result<bool, AppError> {
    let result = ssh_run_remote_with_input(
        &state.provision_ssh_key_path,
        known_hosts,
        ip,
        port,
        &format!(
            "set +e; ps -ef | grep cc-switch-server | grep -v grep >/dev/null 2>&1; \
             status=$?; if [ \"$status\" -eq 0 ]; then \
             echo 'cc-switch-server process is already running' >&2; exit {HOST_HAS_RUNNING_SERVER_EXIT}; \
             fi; exit 0"
        ),
        None,
        SSH_VERIFY_TIMEOUT,
        host_key_policy,
    )
    .await;
    match result {
        Ok(_) => Ok(false),
        Err(AppError::Conflict(message))
            if message.contains("cc-switch-server process is already running") =>
        {
            Ok(true)
        }
        Err(other) => Err(other),
    }
}

async fn claim_idle_host_without_running_server(
    state: &ServerState,
    job_id: &str,
    job: &ProvisioningJobRecord,
) -> Result<RouterSshHostRecord, AppError> {
    let subdomain = job.subdomain.clone().unwrap_or_default();
    let known_hosts = known_hosts_path(&state.config);
    for attempt in 1..=MAX_HOST_PROCESS_SKIP_ATTEMPTS {
        let host = state
            .store
            .client_market_claim_idle_host(
                job_id,
                &job.selection_owners,
                &job.selection_regions,
                &subdomain,
            )
            .await?;
        require_pinned_host_fingerprint(&host, &known_hosts).await?;
        match ssh_host_has_running_cc_switch_server(
            state,
            &host.ip,
            host.port,
            &known_hosts,
            SshHostKeyPolicy::RequireKnown,
        )
        .await
        {
            Ok(false) => return Ok(host),
            Ok(true) => {
                let reason = "cc-switch-server process is already running; host marked abnormal";
                state
                    .store
                    .client_market_mark_host_abnormal_and_detach_job(
                        job_id,
                        &host.id,
                        reason,
                    )
                    .await?;
                state
                    .store
                    .client_market_append_job_log(
                        job_id,
                        &format!(
                            "skipped host {} ({}) because cc-switch-server is running (attempt {attempt})\n",
                            host.id, host.ip
                        ),
                    )
                    .await?;
            }
            Err(error) => {
                let reason = format!("host process check failed: {error}");
                state
                    .store
                    .client_market_mark_host_unreachable_and_detach_job(
                        job_id,
                        &host.id,
                        &reason,
                    )
                    .await?;
                state
                    .store
                    .client_market_append_job_log(
                        job_id,
                        &format!(
                            "skipped host {} ({}) after process check failure (attempt {attempt}): {error}\n",
                            host.id, host.ip
                        ),
                    )
                    .await?;
            }
        }
    }
    Err(AppError::ServiceUnavailable(
        "no idle host without a running cc-switch-server matched the selection".into(),
    ))
}

async fn ssh_fetch_host_fingerprint(
    ip: &str,
    port: u16,
    known_hosts: &Path,
) -> Result<String, AppError> {
    let target = if port == 22 {
        ip.to_string()
    } else {
        format!("[{ip}]:{port}")
    };
    let output = Command::new("ssh-keygen")
        .args(["-F", &target, "-f", known_hosts.to_string_lossy().as_ref()])
        .output()
        .await
        .map_err(|e| AppError::Internal(format!("ssh-keygen failed: {e}")))?;
    if !output.status.success() {
        return Err(AppError::Internal(
            "could not locate the verified host key in known_hosts".into(),
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let _hosts = fields.next();
        let _algorithm = fields.next();
        let Some(encoded_key) = fields.next() else {
            continue;
        };
        let Ok(key_blob) = base64::engine::general_purpose::STANDARD.decode(encoded_key) else {
            continue;
        };
        let digest = Sha256::digest(key_blob);
        return Ok(format!(
            "SHA256:{}",
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest)
        ));
    }
    Err(AppError::Internal(
        "could not read host key fingerprint from known_hosts".into(),
    ))
}

async fn ssh_cleanup_remote(
    state: &ServerState,
    host: &RouterSshHostRecord,
) -> Result<(), AppError> {
    require_pinned_host_fingerprint(host, &known_hosts_path(&state.config)).await?;
    let command = "set -eu; \
        if pgrep -f '^/usr/local/bin/cc-switch-server( |$)' >/dev/null 2>&1; then \
          pkill -TERM -f '^/usr/local/bin/cc-switch-server( |$)' || true; \
          i=0; while pgrep -f '^/usr/local/bin/cc-switch-server( |$)' >/dev/null 2>&1 && [ \"$i\" -lt 20 ]; do sleep 1; i=$((i + 1)); done; \
          if pgrep -f '^/usr/local/bin/cc-switch-server( |$)' >/dev/null 2>&1; then pkill -KILL -f '^/usr/local/bin/cc-switch-server( |$)' || true; sleep 1; fi; \
        fi; \
        if pgrep -f '^/usr/local/bin/cc-switch-server( |$)' >/dev/null 2>&1; then exit 43; fi; \
        rm -f /usr/local/bin/cc-switch-server; \
        rm -rf \"$HOME/.cc-switch-server\" \"$HOME\"/.cc-switch-server.bak.*";
    ssh_run_remote_with_input(
        &state.provision_ssh_key_path,
        &known_hosts_path(&state.config),
        &host.ip,
        host.port,
        command,
        None,
        SSH_CLEANUP_TIMEOUT,
        SshHostKeyPolicy::RequireKnown,
    )
    .await
    .map(|_| ())
}

async fn require_pinned_host_fingerprint(
    host: &RouterSshHostRecord,
    known_hosts: &Path,
) -> Result<(), AppError> {
    let actual = ssh_fetch_host_fingerprint(&host.ip, host.port, known_hosts).await?;
    if host
        .ssh_host_key_fingerprint
        .as_deref()
        .is_some_and(|expected| expected != actual)
    {
        return Err(AppError::Conflict(
            "ssh host key fingerprint does not match the registered host".into(),
        ));
    }
    Ok(())
}

async fn read_bounded<R>(mut reader: R, limit: usize) -> std::io::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut retained = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(retained.len());
        if remaining > 0 {
            retained.extend_from_slice(&buffer[..read.min(remaining)]);
        }
        truncated |= read > remaining;
    }
    if truncated {
        retained.extend_from_slice(b"\n[output truncated]\n");
    }
    Ok(retained)
}

async fn ssh_run_remote_with_input(
    key_path: &Path,
    known_hosts: &Path,
    ip: &str,
    port: u16,
    remote_command: &str,
    stdin: Option<Vec<u8>>,
    timeout: Duration,
    host_key_policy: SshHostKeyPolicy,
) -> Result<String, AppError> {
    if let Some(parent) = known_hosts.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            AppError::Internal(format!(
                "create provisioning known_hosts directory failed: {e}"
            ))
        })?;
    }
    let target = format!("root@{ip}");
    let mut command = Command::new("ssh");
    command
        .arg("-F")
        .arg("/dev/null")
        .arg("-T")
        .arg("-i")
        .arg(key_path)
        .arg("-p")
        .arg(port.to_string())
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("IdentitiesOnly=yes")
        .arg("-o")
        .arg("PasswordAuthentication=no")
        .arg("-o")
        .arg("KbdInteractiveAuthentication=no")
        .arg("-o")
        .arg("ChallengeResponseAuthentication=no")
        .arg("-o")
        .arg("PreferredAuthentications=publickey")
        .arg("-o")
        .arg(match host_key_policy {
            SshHostKeyPolicy::AcceptNew => "StrictHostKeyChecking=accept-new",
            SshHostKeyPolicy::RequireKnown => "StrictHostKeyChecking=yes",
        })
        .arg("-o")
        .arg(format!("UserKnownHostsFile={}", known_hosts.display()))
        .arg("-o")
        .arg("GlobalKnownHostsFile=/dev/null")
        .arg("-o")
        .arg("UpdateHostKeys=no")
        .arg("-o")
        .arg("ConnectTimeout=30")
        .arg("-o")
        .arg("ServerAliveInterval=10")
        .arg("-o")
        .arg("ServerAliveCountMax=3")
        .arg("-o")
        .arg("LogLevel=ERROR")
        .arg(&target)
        .arg(remote_command)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|e| AppError::ServiceUnavailable(format!("start ssh command failed: {e}")))?;
    if let Some(input) = stdin {
        let mut child_stdin = child
            .stdin
            .take()
            .ok_or_else(|| AppError::Internal("ssh stdin was not available".into()))?;
        child_stdin
            .write_all(&input)
            .await
            .map_err(|e| AppError::ServiceUnavailable(format!("write ssh input failed: {e}")))?;
        child_stdin
            .shutdown()
            .await
            .map_err(|e| AppError::ServiceUnavailable(format!("close ssh input failed: {e}")))?;
    }
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Internal("ssh stdout was not available".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::Internal("ssh stderr was not available".into()))?;
    let completed = tokio::time::timeout(timeout, async {
        let (status, stdout, stderr) = tokio::join!(
            child.wait(),
            read_bounded(stdout, SSH_OUTPUT_LIMIT),
            read_bounded(stderr, SSH_OUTPUT_LIMIT),
        );
        (status, stdout, stderr)
    })
    .await;
    let (status, stdout, stderr) = match completed {
        Ok(result) => result,
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(AppError::ServiceUnavailable(
                "ssh command exceeded its execution timeout".into(),
            ));
        }
    };
    let status = status
        .map_err(|e| AppError::ServiceUnavailable(format!("wait for ssh command failed: {e}")))?;
    let stdout =
        stdout.map_err(|e| AppError::ServiceUnavailable(format!("read ssh stdout failed: {e}")))?;
    let stderr =
        stderr.map_err(|e| AppError::ServiceUnavailable(format!("read ssh stderr failed: {e}")))?;
    let stdout = String::from_utf8_lossy(&stdout);
    let stderr = String::from_utf8_lossy(&stderr);
    if !status.success() {
        let code = status.code();
        let detail = format!("{stdout}{stderr}");
        if code == Some(HOST_HAS_RUNNING_SERVER_EXIT)
            || detail.contains("cc-switch-server process is already running")
        {
            return Err(AppError::Conflict(
                "cc-switch-server process is already running".into(),
            ));
        }
        return Err(AppError::BadRequest(format!(
            "ssh failed ({}): {detail}",
            status
        )));
    }
    Ok(format!("{stdout}{stderr}"))
}

fn parse_host_ip(value: &str) -> Result<IpAddr, AppError> {
    let trimmed = value.trim();
    let ip = trimmed
        .parse()
        .map_err(|_| AppError::BadRequest("invalid ip address".into()))?;
    if !is_public_routable_ip(ip) {
        return Err(AppError::BadRequest(
            "host ip must be a publicly routable address".into(),
        ));
    }
    Ok(ip)
}

fn is_public_routable_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, d] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 192 && b == 0 && c == 2)
                || (a == 192 && b == 168)
                || (a == 192 && b == 88 && c == 99)
                || (a == 198 && (b == 18 || b == 19))
                || (a == 198 && b == 51 && c == 100)
                || (a == 203 && b == 0 && c == 113)
                || a >= 224
                || (a == 255 && b == 255 && c == 255 && d == 255))
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return is_public_routable_ip(IpAddr::V4(mapped));
            }
            let segments = ip.segments();
            let is_current_global_unicast = segments[0] & 0xe000 == 0x2000;
            let is_teredo = segments[0] == 0x2001 && segments[1] == 0;
            let is_benchmarking = segments[0] == 0x2001 && segments[1] == 2 && segments[2] == 0;
            let is_documentation = segments[0] == 0x2001 && segments[1] == 0x0db8;
            let is_orchid =
                segments[0] == 0x2001 && matches!(segments[1] & 0xfff0, 0x0010 | 0x0020);
            let is_6to4 = segments[0] == 0x2002;
            is_current_global_unicast
                && !(ip.is_unspecified()
                    || ip.is_loopback()
                    || ip.is_multicast()
                    || ip.is_unique_local()
                    || ip.is_unicast_link_local()
                    || is_teredo
                    || is_benchmarking
                    || is_documentation
                    || is_orchid
                    || is_6to4)
        }
    }
}

fn allow_rate_bucket<K: std::hash::Hash + Eq>(
    buckets: &mut HashMap<K, (i64, u32)>,
    key: K,
    bucket: i64,
    limit: u32,
) -> bool {
    if buckets.len() > 4096 {
        buckets.retain(|_, (existing, _)| *existing >= bucket - 1);
    }
    let entry = buckets.entry(key).or_insert((bucket, 0));
    if entry.0 != bucket {
        *entry = (bucket, 0);
    }
    if entry.1 >= limit {
        return false;
    }
    entry.1 += 1;
    true
}

fn new_provision_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn provision_token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn sanitize_job_log_chunk(chunk: &str) -> String {
    let mut output = String::new();
    for line in chunk.lines().take(200) {
        let lower = line.to_ascii_lowercase();
        if [
            "password",
            "token",
            "authorization",
            "bearer",
            "provision-tokens",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
        {
            output.push_str("[sensitive output redacted]\n");
            continue;
        }
        for character in line.chars().take(2000) {
            if !character.is_control() || character == '\t' {
                output.push(character);
            }
        }
        output.push('\n');
        if output.len() >= 16 * 1024 {
            output.truncate(16 * 1024);
            break;
        }
    }
    output
}

fn host_to_view(host: RouterSshHostRecord, reveal: bool) -> RouterSshHostView {
    RouterSshHostView {
        id: host.id,
        ip: reveal.then_some(host.ip),
        port: reveal.then_some(host.port),
        host_owner_email: host.host_owner_email,
        country_code: host.country_code,
        hostname: host.hostname,
        ssh_host_key_fingerprint: reveal.then_some(host.ssh_host_key_fingerprint).flatten(),
        status: host.status,
        client_subdomain: host.client_subdomain,
        client_owner_email: reveal.then_some(host.client_owner_email).flatten(),
        installation_id: reveal.then_some(host.installation_id).flatten(),
        last_verified_at: reveal.then_some(host.last_verified_at).flatten(),
        last_error: reveal.then_some(host.last_error).flatten(),
        note: reveal.then_some(host.note).flatten(),
        created_at: reveal.then_some(host.created_at),
        updated_at: reveal.then_some(host.updated_at),
    }
}

async fn require_session_email(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<String, AppError> {
    extract_optional_session_email(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("authenticated owner session required".into()))
}

async fn extract_optional_session_email(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<Option<String>, AppError> {
    if let Some(session) = crate::api::resolve_router_session(state, headers).await? {
        return Ok(Some(session.email));
    }
    Ok(None)
}

#[derive(Debug, Clone)]
pub struct RouterSshHostRecord {
    pub id: String,
    pub ip: String,
    pub port: u16,
    pub host_owner_email: String,
    pub country_code: Option<String>,
    pub hostname: Option<String>,
    pub ssh_host_key_fingerprint: Option<String>,
    pub status: String,
    pub client_subdomain: Option<String>,
    pub client_owner_email: Option<String>,
    pub installation_id: Option<String>,
    pub last_verified_at: Option<String>,
    pub last_error: Option<String>,
    pub note: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct ProvisioningJobRecord {
    pub id: String,
    pub job_type: String,
    pub host_id: Option<String>,
    pub host_owner_email: Option<String>,
    pub client_owner_email: Option<String>,
    pub selection_owners: Vec<String>,
    pub selection_regions: Vec<String>,
    pub subdomain: Option<String>,
    pub installation_id: Option<String>,
    pub status: String,
    pub phase: String,
    pub log_blob: String,
    pub secret_ref: Option<String>,
    pub failure_code: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn normalize_market_email(value: &str) -> Result<String, AppError> {
    let email = value.trim().to_ascii_lowercase();
    if email.len() > 254 || email.chars().any(char::is_control) {
        return Err(AppError::BadRequest("invalid email".into()));
    }
    let Some((local, domain)) = email.split_once('@') else {
        return Err(AppError::BadRequest("invalid email".into()));
    };
    if local.is_empty() || domain.is_empty() || !domain.contains('.') {
        return Err(AppError::BadRequest("invalid email".into()));
    }
    Ok(email)
}

fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(", ")
}

impl AppStore {
    pub async fn client_market_list_hosts(
        &self,
        owner_email: Option<&str>,
        country: Option<&str>,
        status: Option<&str>,
    ) -> Result<Vec<RouterSshHostRecord>, AppError> {
        let conn = self.conn.lock().await;
        let mut sql = String::from(
            "SELECT h.id, h.ip, h.port, h.host_owner_email, h.country_code, h.hostname,
                    h.ssh_host_key_fingerprint, h.status, h.installation_id,
                    h.last_verified_at, h.last_error, h.note, h.created_at, h.updated_at,
                    t.subdomain, t.owner_email
             FROM router_ssh_hosts h
             LEFT JOIN installation_client_tunnels t ON t.installation_id = h.installation_id
             WHERE 1=1",
        );
        let mut binds: Vec<String> = Vec::new();
        if let Some(owner) = owner_email {
            binds.push(normalize_market_email(owner)?);
            sql.push_str(&format!(" AND h.host_owner_email = ?{}", binds.len()));
        }
        if let Some(country) = country {
            binds.push(country.trim().to_ascii_uppercase());
            sql.push_str(&format!(" AND h.country_code = ?{}", binds.len()));
        }
        if let Some(status) = status {
            binds.push(status.trim().to_string());
            sql.push_str(&format!(" AND h.status = ?{}", binds.len()));
        }
        sql.push_str(" ORDER BY h.updated_at DESC");
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AppError::Internal(format!("prepare list hosts failed: {e}")))?;
        let params: Vec<&dyn rusqlite::ToSql> = binds
            .iter()
            .map(|value| value as &dyn rusqlite::ToSql)
            .collect();
        let rows = stmt
            .query_map(params.as_slice(), map_router_ssh_host_row)
            .map_err(|e| AppError::Internal(format!("query hosts failed: {e}")))?;
        collect_host_rows(rows)
    }

    pub async fn client_market_supply_summary(&self) -> Result<Vec<SupplySummaryEntry>, AppError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT host_owner_email, country_code,
                        SUM(CASE WHEN status = 'idle' THEN 1 ELSE 0 END) AS idle_count,
                        COUNT(*) AS total_count
                 FROM router_ssh_hosts
                 GROUP BY host_owner_email, country_code
                 ORDER BY host_owner_email, country_code",
            )
            .map_err(|e| AppError::Internal(format!("prepare supply summary failed: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(SupplySummaryEntry {
                    host_owner_email: row.get(0)?,
                    country_code: row.get(1)?,
                    idle_count: row.get(2)?,
                    total_count: row.get(3)?,
                })
            })
            .map_err(|e| AppError::Internal(format!("query supply summary failed: {e}")))?;
        let mut output = Vec::new();
        for row in rows {
            output
                .push(row.map_err(|e| AppError::Internal(format!("read supply row failed: {e}")))?);
        }
        Ok(output)
    }

    pub async fn client_market_insert_host(
        &self,
        owner_email: &str,
        ip: &str,
        port: u16,
        country_code: Option<&str>,
        hostname: Option<&str>,
        fingerprint: Option<&str>,
        note: Option<&str>,
    ) -> Result<RouterSshHostRecord, AppError> {
        let owner = normalize_market_email(owner_email)?;
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4().to_string();
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO router_ssh_hosts (
                id, ip, port, host_owner_email, country_code, hostname, ssh_host_key_fingerprint,
                status, installation_id, last_verified_at, last_error, note, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, NULL, ?10, ?9, ?9)",
            params![
                id,
                ip,
                port,
                owner,
                country_code,
                hostname,
                fingerprint,
                HOST_STATUS_IDLE,
                now,
                note,
            ],
        )
        .map_err(|e| {
            if e.to_string().contains("UNIQUE constraint failed") {
                AppError::Conflict("host with this ip and port already exists".into())
            } else {
                AppError::Internal(format!("insert router ssh host failed: {e}"))
            }
        })?;
        get_router_ssh_host(&conn, &id)?.ok_or_else(|| {
            AppError::Internal("inserted router ssh host could not be read back".into())
        })
    }

    pub async fn client_market_delete_host(
        &self,
        id: &str,
        viewer_email: &str,
        is_admin: bool,
    ) -> Result<(), AppError> {
        let viewer = normalize_market_email(viewer_email)?;
        let conn = self.conn.lock().await;
        let host = get_router_ssh_host(&conn, id)?
            .ok_or_else(|| AppError::NotFound("host not found".into()))?;
        if host.host_owner_email != viewer && !is_admin {
            return Err(AppError::Forbidden(
                "not allowed to delete this host".into(),
            ));
        }
        if host.status != HOST_STATUS_IDLE
            && host.status != HOST_STATUS_DISABLED
            && host.status != HOST_STATUS_ABNORMAL
        {
            return Err(AppError::Conflict(
                "host must be idle, disabled, or abnormal before deletion".into(),
            ));
        }
        conn.execute("DELETE FROM router_ssh_hosts WHERE id = ?1", params![id])
            .map_err(|e| AppError::Internal(format!("delete host failed: {e}")))?;
        Ok(())
    }

    pub async fn client_market_create_job(
        &self,
        job_id: &str,
        job_type: &str,
        client_owner_email: &str,
        host_owners: &[String],
        regions: &[String],
        subdomain: &str,
        installation_id: Option<&str>,
    ) -> Result<(), AppError> {
        let client_owner = normalize_market_email(client_owner_email)?;
        let mut owners: Vec<String> = host_owners
            .iter()
            .map(|value| normalize_market_email(value))
            .collect::<Result<Vec<_>, _>>()?;
        owners.sort_unstable();
        owners.dedup();
        let mut regions: Vec<String> = regions
            .iter()
            .map(|value| value.trim().to_ascii_uppercase())
            .filter(|value| !value.is_empty())
            .collect();
        if regions
            .iter()
            .any(|value| value.len() != 2 || !value.bytes().all(|byte| byte.is_ascii_uppercase()))
        {
            return Err(AppError::BadRequest(
                "country codes must be two ASCII letters".into(),
            ));
        }
        regions.sort_unstable();
        regions.dedup();
        let now = Utc::now().to_rfc3339();
        let owners_json = serde_json::to_string(&owners)
            .map_err(|e| AppError::Internal(format!("encode owners failed: {e}")))?;
        let regions_json = serde_json::to_string(&regions)
            .map_err(|e| AppError::Internal(format!("encode regions failed: {e}")))?;
        if owners.is_empty()
            || owners.len() > MAX_SELECTION_ITEMS
            || regions.is_empty()
            || regions.len() > MAX_SELECTION_ITEMS
        {
            return Err(AppError::BadRequest(
                "owner and region selections must each contain 1 to 100 values".into(),
            ));
        }
        let mut conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|e| {
            AppError::Internal(format!("begin provisioning job transaction failed: {e}"))
        })?;
        tx.execute(
            "DELETE FROM subdomain_reservations
             WHERE expires_at_ms <= ?1 AND installation_id IS NULL",
            params![Utc::now().timestamp_millis()],
        )
        .map_err(|e| AppError::Internal(format!("expire subdomain reservations failed: {e}")))?;
        let active_jobs: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM provisioning_jobs
                 WHERE client_owner_email = ?1 AND status IN ('pending', 'running')",
                params![client_owner],
                |row| row.get(0),
            )
            .map_err(|e| {
                AppError::Internal(format!("count active provisioning jobs failed: {e}"))
            })?;
        if active_jobs >= 5 {
            return Err(AppError::Conflict(
                "too many active client provisioning jobs".into(),
            ));
        }
        let existing_host: Option<String> = tx
            .query_row(
                "SELECT label FROM public_hosts
                 WHERE label = ?1 COLLATE NOCASE",
                params![subdomain],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("check subdomain catalog failed: {e}")))?;
        if existing_host.is_some() {
            return Err(AppError::Conflict("subdomain is already in use".into()));
        }
        let reservation_owner: Option<String> = tx
            .query_row(
                "SELECT job_id FROM subdomain_reservations
                 WHERE subdomain = ?1 COLLATE NOCASE",
                params![subdomain],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("check subdomain reservation failed: {e}")))?;
        if reservation_owner.is_some() {
            return Err(AppError::Conflict("subdomain is reserved".into()));
        }
        tx.execute(
            "INSERT INTO provisioning_jobs (
                id, type, host_id, host_owner_email, client_owner_email,
                selection_owners_json, selection_regions_json, subdomain, installation_id,
                status, phase, log_blob, secret_ref, failure_code, created_at, updated_at
             ) VALUES (?1, ?2, NULL, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?9, '', NULL, NULL, ?10, ?10)",
            params![
                job_id,
                job_type,
                client_owner,
                owners_json,
                regions_json,
                subdomain,
                installation_id,
                JOB_STATUS_PENDING,
                JOB_PHASE_PENDING,
                now,
            ],
        )
        .map_err(|e| AppError::Internal(format!("insert provisioning job failed: {e}")))?;
        tx.execute(
            "INSERT INTO subdomain_reservations (
                subdomain, job_id, host_id, client_owner_email, installation_id, expires_at_ms
             ) VALUES (?1, ?2, NULL, ?3, NULL, ?4)",
            params![
                subdomain,
                job_id,
                client_owner,
                Utc::now().timestamp_millis() + SUBDOMAIN_RESERVATION_TTL_MS,
            ],
        )
        .map_err(|e| AppError::Internal(format!("reserve subdomain failed: {e}")))?;
        tx.commit().map_err(|e| {
            AppError::Internal(format!("commit provisioning job transaction failed: {e}"))
        })?;
        Ok(())
    }

    pub async fn client_market_get_job_for_viewer(
        &self,
        job_id: &str,
        viewer_email: &str,
        is_admin: bool,
    ) -> Result<JobView, AppError> {
        let viewer = normalize_market_email(viewer_email)?;
        let job = self
            .client_market_get_job_record(job_id)
            .await?
            .ok_or_else(|| AppError::NotFound("job not found".into()))?;
        let allowed = is_admin
            || job.client_owner_email.as_deref() == Some(viewer.as_str())
            || job.host_owner_email.as_deref() == Some(viewer.as_str());
        if !allowed {
            return Err(AppError::Forbidden("not allowed to view this job".into()));
        }
        let country_code = if let Some(host_id) = job.host_id.as_deref() {
            self.client_market_get_host(host_id)
                .await?
                .and_then(|host| host.country_code)
        } else {
            None
        };
        Ok(JobView {
            id: job.id,
            job_type: job.job_type,
            host_id: job.host_id,
            host_owner_email: job.host_owner_email,
            client_owner_email: job.client_owner_email,
            subdomain: job.subdomain,
            installation_id: job.installation_id,
            status: job.status,
            phase: job.phase,
            failure_code: job.failure_code,
            country_code,
            client_url: None,
            log: job.log_blob,
            created_at: job.created_at,
            updated_at: job.updated_at,
        })
    }

    pub async fn client_market_get_job_record(
        &self,
        job_id: &str,
    ) -> Result<Option<ProvisioningJobRecord>, AppError> {
        let conn = self.conn.lock().await;
        conn.query_row(
            "SELECT id, type, host_id, host_owner_email, client_owner_email,
                    selection_owners_json, selection_regions_json, subdomain, installation_id,
                    status, phase, log_blob, secret_ref, failure_code, created_at, updated_at
             FROM provisioning_jobs WHERE id = ?1",
            params![job_id],
            map_provisioning_job_row,
        )
        .optional()
        .map_err(|e| AppError::Internal(format!("query job failed: {e}")))
    }

    pub async fn client_market_interrupted_jobs(
        &self,
    ) -> Result<Vec<ProvisioningJobRecord>, AppError> {
        let conn = self.conn.lock().await;
        let mut statement = conn
            .prepare(
                "SELECT id, type, host_id, host_owner_email, client_owner_email,
                        selection_owners_json, selection_regions_json, subdomain, installation_id,
                        status, phase, log_blob, secret_ref, failure_code, created_at, updated_at
                 FROM provisioning_jobs
                 WHERE status IN ('pending', 'running')
                 ORDER BY created_at ASC",
            )
            .map_err(|e| AppError::Internal(format!("prepare interrupted jobs failed: {e}")))?;
        let rows = statement
            .query_map([], map_provisioning_job_row)
            .map_err(|e| AppError::Internal(format!("query interrupted jobs failed: {e}")))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Internal(format!("read interrupted job failed: {e}")))
    }

    pub async fn client_market_append_job_log(
        &self,
        job_id: &str,
        chunk: &str,
    ) -> Result<(), AppError> {
        let chunk = sanitize_job_log_chunk(chunk);
        if chunk.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE provisioning_jobs
             SET log_blob = substr(COALESCE(log_blob, '') || ?2, -?3), updated_at = ?4
             WHERE id = ?1",
            params![job_id, chunk, JOB_LOG_LIMIT as i64, Utc::now().to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("append job log failed: {e}")))?;
        Ok(())
    }

    pub async fn client_market_start_job(
        &self,
        job_id: &str,
        expected_type: &str,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                "UPDATE provisioning_jobs
                 SET status = ?3, updated_at = ?4
                 WHERE id = ?1 AND type = ?2 AND status = 'pending'",
                params![
                    job_id,
                    expected_type,
                    JOB_STATUS_RUNNING,
                    Utc::now().to_rfc3339()
                ],
            )
            .map_err(|e| AppError::Internal(format!("start provisioning job failed: {e}")))?;
        if changed != 1 {
            return Err(AppError::Conflict("provisioning job is not pending".into()));
        }
        Ok(())
    }

    pub async fn client_market_set_running_phase(
        &self,
        job_id: &str,
        expected_phase: &str,
        next_phase: &str,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                "UPDATE provisioning_jobs
                 SET phase = ?3, updated_at = ?4
                 WHERE id = ?1 AND status = 'running' AND phase = ?2",
                params![job_id, expected_phase, next_phase, Utc::now().to_rfc3339()],
            )
            .map_err(|e| AppError::Internal(format!("advance provisioning job failed: {e}")))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "provisioning job phase changed concurrently".into(),
            ));
        }
        Ok(())
    }

    pub async fn client_market_mark_rollback(&self, job_id: &str) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE provisioning_jobs
             SET phase = ?2, secret_ref = NULL, updated_at = ?3
             WHERE id = ?1 AND status = 'running'",
            params![job_id, JOB_PHASE_ROLLBACK, Utc::now().to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("mark provisioning rollback failed: {e}")))?;
        Ok(())
    }

    pub async fn client_market_activate_token(
        &self,
        job_id: &str,
        token_hash: &str,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                "UPDATE provisioning_jobs
                 SET secret_ref = ?2, phase = ?3, updated_at = ?4
                 WHERE id = ?1 AND status = 'running' AND phase = ?5 AND host_id IS NOT NULL",
                params![
                    job_id,
                    token_hash,
                    JOB_PHASE_INSTALLING,
                    Utc::now().to_rfc3339(),
                    JOB_PHASE_LOCKED,
                ],
            )
            .map_err(|e| AppError::Internal(format!("activate provision token failed: {e}")))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "provisioning job cannot activate a token".into(),
            ));
        }
        Ok(())
    }

    pub async fn client_market_finish_installer(&self, job_id: &str) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                "UPDATE provisioning_jobs
                 SET secret_ref = NULL, phase = ?2, updated_at = ?3
                 WHERE id = ?1 AND status = 'running' AND phase IN (?4, ?2)",
                params![
                    job_id,
                    JOB_PHASE_WAITING,
                    Utc::now().to_rfc3339(),
                    JOB_PHASE_INSTALLING,
                ],
            )
            .map_err(|e| AppError::Internal(format!("finish remote installer failed: {e}")))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "provisioning job is no longer installing".into(),
            ));
        }
        Ok(())
    }

    pub async fn client_market_validate_token_redemption(
        &self,
        job_id: &str,
        token_hash: &str,
        source_ip: &str,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let valid: Option<i64> = conn
            .query_row(
                "SELECT 1
                 FROM provisioning_jobs j
                 JOIN router_ssh_hosts h ON h.id = j.host_id
                 WHERE j.id = ?1 AND j.status = 'running'
                   AND j.phase IN ('installing', 'waiting_for_client')
                   AND j.secret_ref = ?2 AND h.ip = ?3 AND h.status = 'locked'",
                params![job_id, token_hash, source_ip],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("validate provision token failed: {e}")))?;
        if valid.is_none() {
            return Err(AppError::NotFound(
                "provision token not found or expired".into(),
            ));
        }
        Ok(())
    }

    pub async fn client_market_fail_job(&self, job_id: &str, log: &str) -> Result<(), AppError> {
        self.client_market_append_job_log(job_id, log).await?;
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE provisioning_jobs
             SET status = ?2, phase = ?3, secret_ref = NULL, updated_at = ?4
             WHERE id = ?1 AND status IN ('pending', 'running')",
            params![
                job_id,
                JOB_STATUS_FAILED,
                JOB_PHASE_COMPLETE,
                Utc::now().to_rfc3339()
            ],
        )
        .map_err(|e| AppError::Internal(format!("fail provisioning job failed: {e}")))?;
        conn.execute(
            "DELETE FROM subdomain_reservations WHERE job_id = ?1",
            params![job_id],
        )
        .map_err(|e| AppError::Internal(format!("release failed job reservation failed: {e}")))?;
        Ok(())
    }

    pub async fn client_market_claim_idle_host(
        &self,
        job_id: &str,
        owners: &[String],
        regions: &[String],
        subdomain: &str,
    ) -> Result<RouterSshHostRecord, AppError> {
        if owners.is_empty() || regions.is_empty() {
            return Err(AppError::BadRequest(
                "host owner and region filters required".into(),
            ));
        }
        let expires_at_ms = Utc::now().timestamp_millis() + SUBDOMAIN_RESERVATION_TTL_MS;
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(|e| AppError::Internal(format!("begin claim host tx failed: {e}")))?;
        tx.execute(
            "DELETE FROM subdomain_reservations
             WHERE expires_at_ms < ?1 AND installation_id IS NULL",
            params![Utc::now().timestamp_millis()],
        )
        .ok();
        let job = get_provisioning_job(&tx, job_id)?
            .ok_or_else(|| AppError::NotFound("provisioning job not found".into()))?;
        if job.job_type != JOB_TYPE_CREATE
            || !matches!(job.status.as_str(), JOB_STATUS_PENDING | JOB_STATUS_RUNNING)
            || job.host_id.is_some()
            || job.subdomain.as_deref() != Some(subdomain)
        {
            return Err(AppError::Conflict(
                "provisioning job cannot claim a host in its current state".into(),
            ));
        }
        let reserved_by: Option<String> = tx
            .query_row(
                "SELECT job_id FROM subdomain_reservations WHERE subdomain = ?1 COLLATE NOCASE",
                params![subdomain],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("query subdomain reservation failed: {e}")))?;
        if reserved_by.as_deref() != Some(job_id) {
            return Err(AppError::Conflict(
                "subdomain reservation does not belong to this job".into(),
            ));
        }
        let existing_host: Option<String> = tx
            .query_row(
                "SELECT label FROM public_hosts
                 WHERE label = ?1 COLLATE NOCASE",
                params![subdomain],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("query public host subdomain failed: {e}")))?;
        if existing_host.is_some() {
            return Err(AppError::Conflict("subdomain already in use".into()));
        }
        let owner_placeholders = placeholders(owners.len());
        let region_placeholders = placeholders(regions.len());
        let sql = format!(
            "SELECT id FROM router_ssh_hosts
             WHERE status = '{HOST_STATUS_IDLE}'
               AND host_owner_email IN ({owner_placeholders})
               AND country_code IN ({region_placeholders})
             ORDER BY updated_at ASC
             LIMIT 1"
        );
        let mut query = tx
            .prepare(&sql)
            .map_err(|e| AppError::Internal(format!("prepare claim host failed: {e}")))?;
        let mut values: Vec<&dyn rusqlite::ToSql> = Vec::new();
        for owner in owners {
            values.push(owner);
        }
        for region in regions {
            values.push(region);
        }
        let host_id: Option<String> = query
            .query_row(values.as_slice(), |row| row.get(0))
            .optional()
            .map_err(|e| AppError::Internal(format!("select idle host failed: {e}")))?;
        let host_id = host_id
            .ok_or_else(|| AppError::ServiceUnavailable("no idle host matches selection".into()))?;
        let now = Utc::now().to_rfc3339();
        let updated = tx
            .execute(
                "UPDATE router_ssh_hosts
                 SET status = ?2, updated_at = ?3
                 WHERE id = ?1 AND status = ?4",
                params![host_id, HOST_STATUS_LOCKED, now, HOST_STATUS_IDLE],
            )
            .map_err(|e| AppError::Internal(format!("mark host provisioning failed: {e}")))?;
        drop(query);
        if updated != 1 {
            return Err(AppError::Conflict("host claim raced, retry".into()));
        }
        let attached = tx
            .execute(
                "UPDATE provisioning_jobs
             SET host_id = ?2,
                 host_owner_email = (SELECT host_owner_email FROM router_ssh_hosts WHERE id = ?2),
                 status = ?3,
                 phase = ?4,
                 updated_at = ?5
             WHERE id = ?1 AND host_id IS NULL AND status IN ('pending', 'running')",
                params![job_id, host_id, JOB_STATUS_RUNNING, JOB_PHASE_LOCKED, now],
            )
            .map_err(|e| AppError::Internal(format!("attach host to job failed: {e}")))?;
        if attached != 1 {
            return Err(AppError::Conflict("provisioning job claim raced".into()));
        }
        let reservation_updated = tx
            .execute(
                "UPDATE subdomain_reservations
             SET host_id = ?2, expires_at_ms = ?3
             WHERE job_id = ?1 AND installation_id IS NULL",
                params![job_id, host_id, expires_at_ms],
            )
            .map_err(|e| AppError::Internal(format!("bind host reservation failed: {e}")))?;
        if reservation_updated != 1 {
            return Err(AppError::Conflict(
                "subdomain reservation binding raced".into(),
            ));
        }
        let host = get_router_ssh_host(&tx, &host_id)?
            .ok_or_else(|| AppError::Internal("claimed host missing".into()))?;
        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit claim host failed: {e}")))?;
        Ok(host)
    }

    pub async fn client_market_get_host(
        &self,
        id: &str,
    ) -> Result<Option<RouterSshHostRecord>, AppError> {
        let conn = self.conn.lock().await;
        get_router_ssh_host(&conn, id)
    }

    pub async fn client_market_get_host_for_operator(
        &self,
        id: &str,
        viewer_email: &str,
        is_admin: bool,
    ) -> Result<RouterSshHostRecord, AppError> {
        let viewer = normalize_market_email(viewer_email)?;
        let conn = self.conn.lock().await;
        let host = get_router_ssh_host(&conn, id)?
            .ok_or_else(|| AppError::NotFound("host not found".into()))?;
        if !is_admin && host.host_owner_email != viewer {
            return Err(AppError::Forbidden(
                "not allowed to operate this host".into(),
            ));
        }
        Ok(host)
    }

    pub async fn client_market_complete_host_reverify(
        &self,
        id: &str,
        hostname: Option<&str>,
        fingerprint: Option<&str>,
    ) -> Result<RouterSshHostRecord, AppError> {
        let conn = self.conn.lock().await;
        let now = Utc::now().to_rfc3339();
        let changed = conn
            .execute(
                "UPDATE router_ssh_hosts
                 SET status = ?2, hostname = ?3, ssh_host_key_fingerprint = ?4,
                     installation_id = NULL, last_verified_at = ?5,
                     last_error = NULL, updated_at = ?5
                 WHERE id = ?1
                   AND (installation_id IS NULL OR NOT EXISTS (
                       SELECT 1 FROM installations i
                       WHERE i.id = router_ssh_hosts.installation_id
                   ))
                   AND status IN ('idle', 'disabled', 'unreachable', 'abnormal')
                   AND NOT EXISTS (
                       SELECT 1 FROM provisioning_jobs j
                       WHERE j.host_id = router_ssh_hosts.id
                         AND j.status IN ('pending', 'running')
                   )",
                params![id, HOST_STATUS_IDLE, hostname, fingerprint, now],
            )
            .map_err(|e| AppError::Internal(format!("complete host reverify failed: {e}")))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "host changed while it was being reverified".into(),
            ));
        }
        get_router_ssh_host(&conn, id)?
            .ok_or_else(|| AppError::Internal("reverified host disappeared".into()))
    }

    pub async fn client_market_mark_host_abnormal_and_detach_job(
        &self,
        job_id: &str,
        host_id: &str,
        reason: &str,
    ) -> Result<(), AppError> {
        self.client_market_quarantine_host_and_detach_job(
            job_id,
            host_id,
            HOST_STATUS_ABNORMAL,
            reason,
        )
        .await
    }

    pub async fn client_market_mark_host_unreachable_and_detach_job(
        &self,
        job_id: &str,
        host_id: &str,
        reason: &str,
    ) -> Result<(), AppError> {
        self.client_market_quarantine_host_and_detach_job(
            job_id,
            host_id,
            HOST_STATUS_UNREACHABLE,
            reason,
        )
        .await
    }

    async fn client_market_quarantine_host_and_detach_job(
        &self,
        job_id: &str,
        host_id: &str,
        status: &str,
        reason: &str,
    ) -> Result<(), AppError> {
        let mut conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|e| {
            AppError::Internal(format!("begin quarantine host tx failed: {e}"))
        })?;
        let now = Utc::now().to_rfc3339();
        let clipped = if reason.len() > 500 {
            format!("{}…", &reason[..497])
        } else {
            reason.to_string()
        };
        let updated = tx
            .execute(
                "UPDATE router_ssh_hosts
                 SET status = ?2, last_error = ?3, updated_at = ?4
                 WHERE id = ?1 AND status = ?5",
                params![host_id, status, clipped, now, HOST_STATUS_LOCKED],
            )
            .map_err(|e| AppError::Internal(format!("quarantine host failed: {e}")))?;
        if updated != 1 {
            return Err(AppError::Conflict(
                "host is no longer locked by this provisioning job".into(),
            ));
        }
        let detached = tx
            .execute(
                "UPDATE provisioning_jobs
                 SET host_id = NULL,
                     host_owner_email = NULL,
                     phase = ?2,
                     updated_at = ?3
                 WHERE id = ?1 AND host_id = ?4 AND status IN ('pending', 'running')",
                params![job_id, JOB_PHASE_PENDING, now, host_id],
            )
            .map_err(|e| AppError::Internal(format!("detach host from job failed: {e}")))?;
        if detached != 1 {
            return Err(AppError::Conflict(
                "provisioning job host detach raced".into(),
            ));
        }
        tx.execute(
            "UPDATE subdomain_reservations
             SET host_id = NULL
             WHERE job_id = ?1 AND installation_id IS NULL",
            params![job_id],
        )
        .map_err(|e| AppError::Internal(format!("clear reservation host failed: {e}")))?;
        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit quarantine host failed: {e}")))?;
        Ok(())
    }

    pub async fn client_market_subdomain_for_installation(
        &self,
        installation_id: &str,
    ) -> Result<Option<String>, AppError> {
        let conn = self.conn.lock().await;
        conn.query_row(
            "SELECT subdomain FROM installation_client_tunnels WHERE installation_id = ?1",
            params![installation_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| AppError::Internal(format!("read client subdomain failed: {e}")))
    }

    pub async fn client_market_ready_installation(
        &self,
        job_id: &str,
    ) -> Result<Option<String>, AppError> {
        let conn = self.conn.lock().await;
        conn.query_row(
            "SELECT j.installation_id
             FROM provisioning_jobs j
             JOIN subdomain_reservations r
               ON r.job_id = j.id AND r.installation_id = j.installation_id
             JOIN installation_client_tunnels t
               ON t.installation_id = j.installation_id
              AND t.subdomain = j.subdomain COLLATE NOCASE
              AND t.owner_email = j.client_owner_email
              AND t.enabled = 1
             JOIN public_hosts p
               ON p.kind = 'client' AND p.subject_id = j.installation_id
              AND p.label = j.subdomain COLLATE NOCASE
              AND p.lifecycle = 'active'
             JOIN installation_setup_completions c
               ON c.installation_id = j.installation_id AND c.source = 'explicit'
             WHERE j.id = ?1 AND j.status = 'running'
               AND j.phase = 'waiting_for_client'",
            params![job_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| {
            AppError::Internal(format!(
                "check provisioned installation readiness failed: {e}"
            ))
        })
    }

    pub async fn client_market_bound_installation(
        &self,
        job_id: &str,
    ) -> Result<Option<String>, AppError> {
        let conn = self.conn.lock().await;
        conn.query_row(
            "SELECT installation_id FROM subdomain_reservations
             WHERE job_id = ?1 AND installation_id IS NOT NULL",
            params![job_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| AppError::Internal(format!("read bound provisioned installation failed: {e}")))
    }

    pub async fn client_market_finalize_create_failure(
        &self,
        job_id: &str,
        host_id: Option<&str>,
        release_to_idle: bool,
        failure_code: &str,
        log: &str,
    ) -> Result<(), AppError> {
        let chunk = sanitize_job_log_chunk(log);
        let mut conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|e| {
            AppError::Internal(format!("begin create failure transaction failed: {e}"))
        })?;
        let now = Utc::now().to_rfc3339();
        if let Some(host_id) = host_id {
            let status = if release_to_idle {
                HOST_STATUS_IDLE
            } else {
                HOST_STATUS_UNREACHABLE
            };
            let changed = tx
                .execute(
                    "UPDATE router_ssh_hosts
                     SET status = ?2,
                         installation_id = CASE WHEN ?3 = 1 THEN NULL ELSE installation_id END,
                         last_error = ?4,
                         updated_at = ?5
                     WHERE id = ?1 AND status IN ('locked', 'draining', 'unreachable')
                       AND EXISTS (
                           SELECT 1 FROM provisioning_jobs j
                           WHERE j.id = ?6 AND j.host_id = router_ssh_hosts.id
                             AND j.status IN ('pending', 'running')
                       )",
                    params![
                        host_id,
                        status,
                        i64::from(release_to_idle),
                        failure_code,
                        now,
                        job_id,
                    ],
                )
                .map_err(|e| {
                    AppError::Internal(format!("mark failed provision host failed: {e}"))
                })?;
            if changed != 1 {
                return Err(AppError::Conflict(
                    "provision host is not owned by the active job".into(),
                ));
            }
        }
        let changed = tx
            .execute(
                "UPDATE provisioning_jobs
                 SET status = ?2, phase = ?3, secret_ref = NULL, failure_code = ?4,
                     log_blob = substr(COALESCE(log_blob, '') || ?5, -?6), updated_at = ?7
                 WHERE id = ?1 AND status IN ('pending', 'running')",
                params![
                    job_id,
                    JOB_STATUS_FAILED,
                    JOB_PHASE_COMPLETE,
                    failure_code,
                    chunk,
                    JOB_LOG_LIMIT as i64,
                    now,
                ],
            )
            .map_err(|e| {
                AppError::Internal(format!("finalize failed provisioning job failed: {e}"))
            })?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "provisioning job is already terminal".into(),
            ));
        }
        tx.execute(
            "DELETE FROM subdomain_reservations WHERE job_id = ?1",
            params![job_id],
        )
        .map_err(|e| AppError::Internal(format!("release failed reservation failed: {e}")))?;
        tx.commit().map_err(|e| {
            AppError::Internal(format!("commit create failure transaction failed: {e}"))
        })?;
        Ok(())
    }

    pub async fn client_market_complete_create_job(
        &self,
        job_id: &str,
        host_id: &str,
        installation_id: &str,
        provision_source: &str,
    ) -> Result<(), AppError> {
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(|e| AppError::Internal(format!("begin complete job tx failed: {e}")))?;
        let now = Utc::now().to_rfc3339();
        let ready: Option<i64> = tx
            .query_row(
                "SELECT 1
                 FROM provisioning_jobs j
                 JOIN subdomain_reservations r
                   ON r.job_id = j.id AND r.installation_id = ?3
                 JOIN installation_client_tunnels t
                   ON t.installation_id = ?3 AND t.subdomain = j.subdomain COLLATE NOCASE
                  AND t.owner_email = j.client_owner_email AND t.enabled = 1
                 JOIN public_hosts p
                   ON p.kind = 'client' AND p.subject_id = ?3
                  AND p.label = j.subdomain COLLATE NOCASE AND p.lifecycle = 'active'
                 JOIN installation_setup_completions c
                   ON c.installation_id = ?3 AND c.source = 'explicit'
                 WHERE j.id = ?1 AND j.host_id = ?2 AND j.status = 'running'
                   AND j.phase = 'waiting_for_client'",
                params![job_id, host_id, installation_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| {
                AppError::Internal(format!("verify completed provisioning job failed: {e}"))
            })?;
        if ready.is_none() {
            return Err(AppError::Conflict(
                "provisioned installation is not ready to complete".into(),
            ));
        }
        let tagged = tx
            .execute(
                "UPDATE installations
             SET provision_source = ?2, provision_host_id = ?3
             WHERE id = ?1",
                params![installation_id, provision_source, host_id],
            )
            .map_err(|e| AppError::Internal(format!("tag installation provision failed: {e}")))?;
        if tagged != 1 {
            return Err(AppError::NotFound(
                "provisioned installation not found".into(),
            ));
        }
        let host_changed = tx
            .execute(
                "UPDATE router_ssh_hosts
             SET status = ?2, installation_id = ?3, last_error = NULL, updated_at = ?4
             WHERE id = ?1 AND status = 'locked' AND installation_id = ?3",
                params![host_id, HOST_STATUS_ALLOCATED, installation_id, now],
            )
            .map_err(|e| AppError::Internal(format!("mark host allocated failed: {e}")))?;
        if host_changed != 1 {
            return Err(AppError::Conflict(
                "provision host is no longer locked by this installation".into(),
            ));
        }
        let job_changed = tx
            .execute(
                "UPDATE provisioning_jobs
             SET status = ?2, phase = ?3, installation_id = ?4, secret_ref = NULL,
                 failure_code = NULL, updated_at = ?5
             WHERE id = ?1 AND status = 'running' AND phase = 'waiting_for_client'",
                params![
                    job_id,
                    JOB_STATUS_SUCCEEDED,
                    JOB_PHASE_COMPLETE,
                    installation_id,
                    now,
                ],
            )
            .map_err(|e| AppError::Internal(format!("complete job failed: {e}")))?;
        if job_changed != 1 {
            return Err(AppError::Conflict(
                "provisioning job completion raced".into(),
            ));
        }
        tx.execute(
            "DELETE FROM subdomain_reservations WHERE job_id = ?1 AND installation_id = ?2",
            params![job_id, installation_id],
        )
        .map_err(|e| AppError::Internal(format!("release completed reservation failed: {e}")))?;
        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit complete job failed: {e}")))?;
        Ok(())
    }

    pub async fn client_market_begin_cleanup_job(
        &self,
        installation_id: &str,
        viewer_email: &str,
        is_admin: bool,
    ) -> Result<String, AppError> {
        let viewer = normalize_market_email(viewer_email)?;
        let mut conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|e| {
            AppError::Internal(format!("begin cleanup job transaction failed: {e}"))
        })?;
        let provision_source: Option<String> = tx
            .query_row(
                "SELECT provision_source FROM installations WHERE id = ?1",
                params![installation_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("read installation failed: {e}")))?
            .flatten();
        if provision_source.as_deref() != Some(PROVISION_SOURCE_ROUTER_MARKET) {
            return Err(AppError::BadRequest(
                "installation is not a router market client".into(),
            ));
        }
        let tunnel: Option<(String, String)> = tx
            .query_row(
                "SELECT owner_email, subdomain FROM installation_client_tunnels WHERE installation_id = ?1",
                params![installation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("read tunnel owner failed: {e}")))?;
        let (owner_email, subdomain) =
            tunnel.ok_or_else(|| AppError::NotFound("client not found".into()))?;
        if owner_email != viewer && !is_admin {
            return Err(AppError::Forbidden(
                "not allowed to cleanup this client".into(),
            ));
        }
        let host = tx
            .query_row(
                "SELECT id, host_owner_email, status FROM router_ssh_hosts WHERE installation_id = ?1",
                params![installation_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("lookup provision host failed: {e}")))?
            .ok_or_else(|| AppError::NotFound("provision host not found".into()))?;
        if !matches!(
            host.2.as_str(),
            HOST_STATUS_ALLOCATED | HOST_STATUS_UNREACHABLE
        ) {
            return Err(AppError::Conflict(
                "client host is already being cleaned or is unavailable".into(),
            ));
        }
        let job_id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        tx.execute(
            "INSERT INTO provisioning_jobs (
                id, type, host_id, host_owner_email, client_owner_email,
                selection_owners_json, selection_regions_json, subdomain, installation_id,
                status, phase, log_blob, secret_ref, failure_code, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, '[]', '[]', ?6, ?7, ?8, ?9, '', NULL, NULL, ?10, ?10)",
            params![
                job_id,
                JOB_TYPE_CLEANUP,
                host.0,
                host.1,
                owner_email,
                subdomain,
                installation_id,
                JOB_STATUS_PENDING,
                JOB_PHASE_CLEANUP,
                now,
            ],
        )
        .map_err(|e| AppError::Internal(format!("insert cleanup job failed: {e}")))?;
        let changed = tx
            .execute(
                "UPDATE router_ssh_hosts
                 SET status = ?2, updated_at = ?3
                 WHERE id = ?1 AND status IN ('allocated', 'unreachable') AND installation_id = ?4",
                params![host.0, HOST_STATUS_DRAINING, now, installation_id],
            )
            .map_err(|e| AppError::Internal(format!("mark cleanup host draining failed: {e}")))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "client host cleanup raced with another operation".into(),
            ));
        }
        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit cleanup job failed: {e}")))?;
        Ok(job_id)
    }

    pub async fn client_market_finish_cleanup_job(
        &self,
        job_id: &str,
        host_id: &str,
    ) -> Result<(), AppError> {
        let mut conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|e| {
            AppError::Internal(format!("begin finish cleanup transaction failed: {e}"))
        })?;
        let now = Utc::now().to_rfc3339();
        let host_changed = tx
            .execute(
                "UPDATE router_ssh_hosts
             SET status = ?2, installation_id = NULL, last_error = NULL, updated_at = ?3
             WHERE id = ?1 AND status = 'draining'",
                params![host_id, HOST_STATUS_IDLE, now],
            )
            .map_err(|e| AppError::Internal(format!("reset host after cleanup failed: {e}")))?;
        if host_changed != 1 {
            return Err(AppError::Conflict("cleanup host is not draining".into()));
        }
        let job_changed = tx
            .execute(
                "UPDATE provisioning_jobs
             SET status = ?2, phase = ?3, failure_code = NULL, updated_at = ?4
             WHERE id = ?1 AND status = 'running' AND type = 'cleanup' AND host_id = ?5",
                params![
                    job_id,
                    JOB_STATUS_SUCCEEDED,
                    JOB_PHASE_COMPLETE,
                    now,
                    host_id
                ],
            )
            .map_err(|e| AppError::Internal(format!("complete cleanup job failed: {e}")))?;
        if job_changed != 1 {
            return Err(AppError::Conflict("cleanup job completion raced".into()));
        }
        tx.commit().map_err(|e| {
            AppError::Internal(format!("commit finish cleanup transaction failed: {e}"))
        })?;
        Ok(())
    }

    pub async fn client_market_fail_cleanup_job(
        &self,
        job_id: &str,
        host_id: &str,
        failure_code: &str,
        log: &str,
    ) -> Result<(), AppError> {
        let chunk = sanitize_job_log_chunk(log);
        let mut conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|e| {
            AppError::Internal(format!("begin fail cleanup transaction failed: {e}"))
        })?;
        let now = Utc::now().to_rfc3339();
        let job: Option<(Option<String>, String)> = tx
            .query_row(
                "SELECT installation_id, status
                 FROM provisioning_jobs
                 WHERE id = ?1 AND type = 'cleanup' AND host_id = ?2",
                params![job_id, host_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("read failed cleanup job failed: {e}")))?;
        let (installation_id, job_status) = job.ok_or_else(|| {
            AppError::Conflict("cleanup job is not bound to the supplied host".into())
        })?;
        let installation_id = installation_id
            .ok_or_else(|| AppError::Internal("cleanup job missing installation".into()))?;
        let host: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT status, installation_id FROM router_ssh_hosts WHERE id = ?1",
                params![host_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("read failed cleanup host failed: {e}")))?;
        let (host_status, host_installation_id) =
            host.ok_or_else(|| AppError::NotFound("cleanup host not found".into()))?;
        let installation_exists = tx
            .query_row(
                "SELECT 1 FROM installations WHERE id = ?1",
                params![installation_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("check cleanup installation failed: {e}")))?
            .is_some();
        if !installation_exists {
            if job_status == JOB_STATUS_SUCCEEDED {
                if host_status == HOST_STATUS_IDLE && host_installation_id.is_none() {
                    return Ok(());
                }
                return Err(AppError::Conflict(
                    "completed cleanup job has inconsistent host state".into(),
                ));
            }
            if !matches!(job_status.as_str(), JOB_STATUS_PENDING | JOB_STATUS_RUNNING) {
                return Err(AppError::Conflict("cleanup job is already terminal".into()));
            }
            let host_changed = tx
                .execute(
                    "UPDATE router_ssh_hosts
                 SET status = ?2, installation_id = NULL, last_error = NULL, updated_at = ?3
                 WHERE id = ?1 AND status IN ('draining', 'unreachable')
                   AND installation_id = ?4
                   AND EXISTS (
                       SELECT 1 FROM provisioning_jobs j
                       WHERE j.id = ?5 AND j.type = 'cleanup'
                         AND j.host_id = router_ssh_hosts.id
                         AND j.status IN ('pending', 'running')
                   )",
                    params![host_id, HOST_STATUS_IDLE, now, installation_id, job_id],
                )
                .map_err(|e| {
                    AppError::Internal(format!("recover purged cleanup host failed: {e}"))
                })?;
            if host_changed != 1 {
                return Err(AppError::Conflict(
                    "purged cleanup host changed concurrently".into(),
                ));
            }
            let job_changed = tx
                .execute(
                    "UPDATE provisioning_jobs
                 SET status = ?2, phase = ?3, failure_code = NULL, updated_at = ?4
                 WHERE id = ?1 AND status IN ('pending', 'running')
                   AND type = 'cleanup' AND host_id = ?5 AND installation_id = ?6",
                    params![
                        job_id,
                        JOB_STATUS_SUCCEEDED,
                        JOB_PHASE_COMPLETE,
                        now,
                        host_id,
                        installation_id,
                    ],
                )
                .map_err(|e| {
                    AppError::Internal(format!("recover purged cleanup job failed: {e}"))
                })?;
            if job_changed != 1 {
                return Err(AppError::Conflict(
                    "purged cleanup job changed concurrently".into(),
                ));
            }
            tx.commit().map_err(|e| {
                AppError::Internal(format!("commit recovered cleanup job failed: {e}"))
            })?;
            return Ok(());
        }
        if job_status == JOB_STATUS_FAILED {
            if host_status == HOST_STATUS_UNREACHABLE
                && host_installation_id.as_deref() == Some(installation_id.as_str())
            {
                return Ok(());
            }
            return Err(AppError::Conflict(
                "failed cleanup job has inconsistent host state".into(),
            ));
        }
        if !matches!(job_status.as_str(), JOB_STATUS_PENDING | JOB_STATUS_RUNNING) {
            return Err(AppError::Conflict("cleanup job is already terminal".into()));
        }
        let host_changed = tx
            .execute(
                "UPDATE router_ssh_hosts
             SET status = ?2, last_error = ?3, updated_at = ?4
             WHERE id = ?1 AND status = 'draining' AND installation_id = ?5
               AND EXISTS (
                   SELECT 1 FROM provisioning_jobs j
                   WHERE j.id = ?6 AND j.type = 'cleanup'
                     AND j.host_id = router_ssh_hosts.id
                     AND j.status IN ('pending', 'running')
               )",
                params![
                    host_id,
                    HOST_STATUS_UNREACHABLE,
                    failure_code,
                    now,
                    installation_id,
                    job_id,
                ],
            )
            .map_err(|e| {
                AppError::Internal(format!("mark cleanup host unreachable failed: {e}"))
            })?;
        if host_changed != 1 {
            return Err(AppError::Conflict(
                "cleanup host changed concurrently".into(),
            ));
        }
        let job_changed = tx
            .execute(
                "UPDATE provisioning_jobs
             SET status = ?2, phase = ?3, failure_code = ?4,
                 log_blob = substr(COALESCE(log_blob, '') || ?5, -?6), updated_at = ?7
             WHERE id = ?1 AND status IN ('pending', 'running')
               AND type = 'cleanup' AND host_id = ?8 AND installation_id = ?9",
                params![
                    job_id,
                    JOB_STATUS_FAILED,
                    JOB_PHASE_COMPLETE,
                    failure_code,
                    chunk,
                    JOB_LOG_LIMIT as i64,
                    now,
                    host_id,
                    installation_id,
                ],
            )
            .map_err(|e| AppError::Internal(format!("fail cleanup job failed: {e}")))?;
        if job_changed != 1 {
            return Err(AppError::Conflict(
                "cleanup job changed concurrently".into(),
            ));
        }
        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit failed cleanup job failed: {e}")))?;
        Ok(())
    }
}

fn get_router_ssh_host(
    conn: &Connection,
    id: &str,
) -> Result<Option<RouterSshHostRecord>, AppError> {
    conn.query_row(
        "SELECT h.id, h.ip, h.port, h.host_owner_email, h.country_code, h.hostname,
                h.ssh_host_key_fingerprint, h.status, h.installation_id,
                h.last_verified_at, h.last_error, h.note, h.created_at, h.updated_at,
                t.subdomain, t.owner_email
         FROM router_ssh_hosts h
         LEFT JOIN installation_client_tunnels t ON t.installation_id = h.installation_id
         WHERE h.id = ?1",
        params![id],
        map_router_ssh_host_row,
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("get host failed: {e}")))
}

fn get_provisioning_job(
    conn: &Connection,
    job_id: &str,
) -> Result<Option<ProvisioningJobRecord>, AppError> {
    conn.query_row(
        "SELECT id, type, host_id, host_owner_email, client_owner_email,
                    selection_owners_json, selection_regions_json, subdomain, installation_id,
                    status, phase, log_blob, secret_ref, failure_code, created_at, updated_at
             FROM provisioning_jobs WHERE id = ?1",
        params![job_id],
        map_provisioning_job_row,
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("get job failed: {e}")))
}

fn map_router_ssh_host_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RouterSshHostRecord> {
    Ok(RouterSshHostRecord {
        id: row.get(0)?,
        ip: row.get(1)?,
        port: row.get::<_, i64>(2)? as u16,
        host_owner_email: row.get(3)?,
        country_code: row.get(4)?,
        hostname: row.get(5)?,
        ssh_host_key_fingerprint: row.get(6)?,
        status: row.get(7)?,
        client_subdomain: row.get(14)?,
        client_owner_email: row.get(15)?,
        installation_id: row.get(8)?,
        last_verified_at: row.get(9)?,
        last_error: row.get(10)?,
        note: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

fn map_provisioning_job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProvisioningJobRecord> {
    let owners_json: String = row.get(5)?;
    let regions_json: String = row.get(6)?;
    let selection_owners: Vec<String> = serde_json::from_str(&owners_json).unwrap_or_default();
    let selection_regions: Vec<String> = serde_json::from_str(&regions_json).unwrap_or_default();
    Ok(ProvisioningJobRecord {
        id: row.get(0)?,
        job_type: row.get(1)?,
        host_id: row.get(2)?,
        host_owner_email: row.get(3)?,
        client_owner_email: row.get(4)?,
        selection_owners,
        selection_regions,
        subdomain: row.get(7)?,
        installation_id: row.get(8)?,
        status: row.get(9)?,
        phase: row.get(10)?,
        log_blob: row.get(11)?,
        secret_ref: row.get(12)?,
        failure_code: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
    })
}

fn collect_host_rows(
    rows: rusqlite::MappedRows<
        '_,
        impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<RouterSshHostRecord>,
    >,
) -> Result<Vec<RouterSshHostRecord>, AppError> {
    let mut output = Vec::new();
    for row in rows {
        output.push(row.map_err(|e| AppError::Internal(format!("read host row failed: {e}")))?);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use super::*;
    use crate::config::{ClientNotificationSettings, MetricsConfig};
    use crate::namespace::PublicHostKind;
    use crate::public_hosts::{
        NewPublicHost, PublicHostLifecycle, claim as claim_public_host,
        get_by_label as get_public_host,
    };

    fn test_config(name: &str) -> (Config, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "cc-switch-router-client-market-{name}-{}",
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create client market test directory");
        let config = Config {
            api_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            ssh_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            tunnel_domain: "router.test".into(),
            ssh_public_addr: String::new(),
            use_localhost: true,
            lease_ttl_secs: 60,
            db_path: root.join("router.db"),
            host_key_path: root.join("host-key"),
            provision_ssh_private_key_path: root.join("provision-key"),
            provision_ssh_public_key_path: root.join("provision-key.pub"),
            cleanup_interval_secs: 300,
            lease_retention_secs: 24 * 60 * 60,
            request_log_retention_days: 30,
            client_stale_secs: 60 * 60,
            client_installation_retention_secs: 6 * 60 * 60,
            paused_share_stale_secs: 60 * 60,
            resend_api_key: None,
            resend_from: None,
            resend_from_name: None,
            resend_reply_to: None,
            client_notifications: ClientNotificationSettings::default(),
            auth_code_ttl_secs: 600,
            auth_code_cooldown_secs: 60,
            auth_session_ttl_secs: 7 * 24 * 60 * 60,
            auth_refresh_ttl_secs: 30 * 24 * 60 * 60,
            auth_max_verify_attempts: 8,
            auth_email_hourly_limit: 10,
            auth_ip_hourly_limit: 30,
            auth_installation_hourly_limit: 15,
            ip_blacklist: String::new(),
            free_share_ip_parallel_limit: 1,
            verification_service_base_url: "https://tokenswitch.org".into(),
            verification_service_api_key: None,
            admin_emails: HashSet::new(),
            telegram_bot_token: None,
            telegram_chat_id: None,
            telegram_topic_id: None,
            telegram_notify_all: false,
            telegram_notify_admin: false,
            board_max_len: 1000,
            board_guest_per_hour: 5,
            board_user_per_hour: 30,
            board_pin_limit: 3,
            board_guest_self_delete_secs: 300,
            ux_telemetry_enabled: false,
            ux_telemetry_retention_days: 7,
            footer_telegram_url: crate::config::DEFAULT_FOOTER_TELEGRAM_URL.to_string(),
            metrics: MetricsConfig {
                enabled: false,
                db_path: root.join("metrics.db"),
                retention_days: 7,
                sample_interval_secs: 5,
            },
        };
        (config, root)
    }

    fn test_store(name: &str) -> (AppStore, Config, PathBuf) {
        let (config, root) = test_config(name);
        let store = AppStore::new(&config).expect("create client market test store");
        (store, config, root)
    }

    async fn add_host(
        store: &AppStore,
        owner: &str,
        ip: &str,
        country: &str,
    ) -> RouterSshHostRecord {
        store
            .client_market_insert_host(
                owner,
                ip,
                22,
                Some(country),
                Some("test-host"),
                Some("SHA256:test"),
                Some("test note"),
            )
            .await
            .expect("insert host")
    }

    async fn create_started_job(
        store: &AppStore,
        job_id: &str,
        client_owner: &str,
        owners: &[&str],
        regions: &[&str],
        subdomain: &str,
    ) {
        store
            .client_market_create_job(
                job_id,
                JOB_TYPE_CREATE,
                client_owner,
                &owners
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect::<Vec<_>>(),
                &regions
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect::<Vec<_>>(),
                subdomain,
                None,
            )
            .await
            .expect("create job");
        store
            .client_market_start_job(job_id, JOB_TYPE_CREATE)
            .await
            .expect("start job");
    }

    fn insert_installation(conn: &Connection, installation_id: &str, owner: &str) {
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO installations (
                id, public_key, platform, app_version, owner_email, owner_verified_at,
                created_at, last_seen_at, provision_source, provision_host_id
             ) VALUES (?1, 'test-public-key', 'linux', 'test', ?2, ?3, ?3, ?3, NULL, NULL)",
            params![installation_id, owner, now],
        )
        .expect("insert installation");
    }

    fn insert_tunnel_and_public_host(
        conn: &Connection,
        installation_id: &str,
        owner: &str,
        subdomain: &str,
    ) {
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO installation_client_tunnels (
                installation_id, owner_email, subdomain, enabled, created_at, updated_at,
                last_seen_at
             ) VALUES (?1, ?2, ?3, 1, ?4, ?4, ?4)",
            params![installation_id, owner, subdomain, now],
        )
        .expect("insert client tunnel");
        claim_public_host(
            conn,
            NewPublicHost {
                label: subdomain,
                route_id: installation_id,
                kind: PublicHostKind::Client,
                subject_id: installation_id,
                installation_id: Some(installation_id),
                target_lane_id: installation_id,
            },
        )
        .expect("claim public client host");
    }

    fn insert_setup_completion(conn: &Connection, installation_id: &str, source: &str) {
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO installation_setup_completions (
                installation_id, setup_id, source, password_hint, notification_status,
                event_id, completed_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, NULL, 'suppressed_disabled', NULL, ?4, ?4, ?4)",
            params![installation_id, Uuid::new_v4().to_string(), source, now],
        )
        .expect("insert setup completion");
    }

    #[tokio::test]
    async fn owner_region_selection_uses_all_bindings_and_oldest_matching_host() {
        let (store, _, root) = test_store("selection");
        let wrong_region = add_host(&store, "one@example.com", "198.18.0.1", "FR").await;
        let wrong_owner = add_host(&store, "three@example.com", "198.18.0.2", "US").await;
        let selected = add_host(&store, "two@example.com", "198.18.0.3", "DE").await;
        let newer = add_host(&store, "one@example.com", "198.18.0.4", "US").await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE router_ssh_hosts SET updated_at = '2020-01-01T00:00:00Z' WHERE id = ?1",
                params![selected.id],
            )
            .unwrap();
            conn.execute(
                "UPDATE router_ssh_hosts SET updated_at = '2021-01-01T00:00:00Z' WHERE id = ?1",
                params![newer.id],
            )
            .unwrap();
        }
        create_started_job(
            &store,
            "selection-job",
            "client@example.com",
            &["one@example.com", "two@example.com"],
            &["US", "DE"],
            "selection-client",
        )
        .await;

        let claimed = store
            .client_market_claim_idle_host(
                "selection-job",
                &["one@example.com".into(), "two@example.com".into()],
                &["US".into(), "DE".into()],
                "selection-client",
            )
            .await
            .expect("claim matching host");
        assert_eq!(claimed.id, selected.id);
        assert_eq!(claimed.status, HOST_STATUS_LOCKED);
        assert_eq!(
            store
                .client_market_get_host(&wrong_region.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            HOST_STATUS_IDLE
        );
        assert_eq!(
            store
                .client_market_get_host(&wrong_owner.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            HOST_STATUS_IDLE
        );
        let job = store
            .client_market_get_job_record("selection-job")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.host_id.as_deref(), Some(selected.id.as_str()));
        assert_eq!(job.host_owner_email.as_deref(), Some("two@example.com"));
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn concurrent_jobs_can_lock_a_single_host_only_once() {
        let (store, _, root) = test_store("concurrent-claim");
        let host = add_host(&store, "host@example.com", "198.18.1.1", "US").await;
        create_started_job(
            &store,
            "claim-job-a",
            "client@example.com",
            &["host@example.com"],
            &["US"],
            "claim-client-a",
        )
        .await;
        create_started_job(
            &store,
            "claim-job-b",
            "client@example.com",
            &["host@example.com"],
            &["US"],
            "claim-client-b",
        )
        .await;

        let first_store = store.clone();
        let second_store = store.clone();
        let owners = vec!["host@example.com".to_string()];
        let regions = vec!["US".to_string()];
        let (first, second) = tokio::join!(
            first_store.client_market_claim_idle_host(
                "claim-job-a",
                &owners,
                &regions,
                "claim-client-a",
            ),
            second_store.client_market_claim_idle_host(
                "claim-job-b",
                &owners,
                &regions,
                "claim-client-b",
            )
        );
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        assert_eq!(
            store
                .client_market_get_host(&host.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            HOST_STATUS_LOCKED
        );
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn reservation_binds_only_matching_owner_and_host_then_requires_setup_receipt() {
        let (store, _, root) = test_store("reservation-binding");
        let host = add_host(&store, "host@example.com", "198.18.2.1", "US").await;
        create_started_job(
            &store,
            "binding-job",
            "client@example.com",
            &["host@example.com"],
            &["US"],
            "bound-client",
        )
        .await;
        store
            .client_market_claim_idle_host(
                "binding-job",
                &["host@example.com".into()],
                &["US".into()],
                "bound-client",
            )
            .await
            .unwrap();

        {
            let mut conn = store.conn.lock().await;
            insert_installation(&conn, "installation-bound", "client@example.com");
            assert!(matches!(
                authorize_client_market_subdomain_claim(
                    &conn,
                    "bound-client",
                    "installation-bound",
                    "other@example.com",
                    Some("198.18.2.1"),
                ),
                Err(AppError::Conflict(_))
            ));
            assert!(matches!(
                authorize_client_market_subdomain_claim(
                    &conn,
                    "bound-client",
                    "installation-bound",
                    "client@example.com",
                    Some("198.18.2.2"),
                ),
                Err(AppError::Conflict(_))
            ));
            let tx = conn.transaction().unwrap();
            authorize_client_market_subdomain_claim(
                &tx,
                "bound-client",
                "installation-bound",
                "client@example.com",
                Some("198.18.2.1"),
            )
            .expect("bind reservation");
            tx.commit().unwrap();

            assert!(
                client_market_subdomain_available_to_source(
                    &conn,
                    "bound-client",
                    Some("installation-bound"),
                    Some("198.18.2.1"),
                )
                .unwrap()
            );
            assert!(
                !client_market_subdomain_available_to_source(
                    &conn,
                    "bound-client",
                    Some("another-installation"),
                    Some("198.18.2.1"),
                )
                .unwrap()
            );
            assert!(
                !client_market_subdomain_available_to_source(
                    &conn,
                    "bound-client",
                    Some("installation-bound"),
                    Some("198.18.2.2"),
                )
                .unwrap()
            );

            insert_tunnel_and_public_host(
                &conn,
                "installation-bound",
                "client@example.com",
                "bound-client",
            );
        }

        assert_eq!(
            store
                .client_market_ready_installation("binding-job")
                .await
                .unwrap(),
            None
        );
        {
            let conn = store.conn.lock().await;
            insert_setup_completion(&conn, "installation-bound", "legacy_fallback");
        }
        assert_eq!(
            store
                .client_market_ready_installation("binding-job")
                .await
                .unwrap(),
            None
        );
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE installation_setup_completions SET source = 'explicit'
                 WHERE installation_id = 'installation-bound'",
                [],
            )
            .unwrap();
        }
        assert_eq!(
            store
                .client_market_ready_installation("binding-job")
                .await
                .unwrap()
                .as_deref(),
            Some("installation-bound")
        );
        store
            .client_market_complete_create_job(
                "binding-job",
                &host.id,
                "installation-bound",
                PROVISION_SOURCE_ROUTER_MARKET,
            )
            .await
            .unwrap();
        let completed_host = store
            .client_market_get_host(&host.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed_host.status, HOST_STATUS_ALLOCATED);
        assert_eq!(
            completed_host.installation_id.as_deref(),
            Some("installation-bound")
        );
        let job = store
            .client_market_get_job_record("binding-job")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.status, JOB_STATUS_SUCCEEDED);
        assert_eq!(job.phase, JOB_PHASE_COMPLETE);
        assert_eq!(
            store
                .installation_provision_source("installation-bound")
                .await
                .unwrap()
                .as_deref(),
            Some(PROVISION_SOURCE_ROUTER_MARKET)
        );
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn provision_tokens_are_hashed_source_bound_and_expiring() {
        let (store, _, root) = test_store("provision-token");
        add_host(&store, "host@example.com", "198.18.3.1", "US").await;
        create_started_job(
            &store,
            "token-job",
            "client@example.com",
            &["host@example.com"],
            &["US"],
            "token-client",
        )
        .await;
        store
            .client_market_claim_idle_host(
                "token-job",
                &["host@example.com".into()],
                &["US".into()],
                "token-client",
            )
            .await
            .unwrap();
        let raw_token = "A".repeat(43);
        let token_hash = provision_token_hash(&raw_token);
        store
            .client_market_activate_token("token-job", &token_hash)
            .await
            .unwrap();
        store
            .client_market_validate_token_redemption("token-job", &token_hash, "198.18.3.1")
            .await
            .unwrap();
        assert!(
            store
                .client_market_validate_token_redemption("token-job", &token_hash, "198.18.3.2")
                .await
                .is_err()
        );
        let persisted_secret: String = {
            let conn = store.conn.lock().await;
            conn.query_row(
                "SELECT secret_ref FROM provisioning_jobs WHERE id = 'token-job'",
                [],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(persisted_secret, token_hash);
        assert_ne!(persisted_secret, raw_token);

        let host_ip: IpAddr = "198.18.3.1".parse().unwrap();
        let mut secrets = ClientMarketJobSecrets::default();
        secrets.insert_token_hash(
            token_hash.clone(),
            ProvisionTokenSecret {
                password: "not-persisted".into(),
                owner_email: "client@example.com".into(),
                subdomain: "token-client".into(),
                job_id: "token-job".into(),
                host_ip,
                expires_at: Instant::now() + Duration::from_secs(60),
                redeemed_at: None,
            },
        );
        assert!(
            secrets
                .redeem_token(&token_hash, "198.18.3.2".parse().unwrap())
                .is_none()
        );
        assert_eq!(
            secrets.redeem_token(&token_hash, host_ip).unwrap().password,
            "not-persisted"
        );
        secrets.tokens.get_mut(&token_hash).unwrap().expires_at =
            Instant::now() - Duration::from_secs(1);
        assert!(secrets.redeem_token(&token_hash, host_ip).is_none());
        assert!(!sanitize_job_log_chunk(&format!("token={raw_token}")).contains(&raw_token));
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn create_failure_releases_only_successfully_rolled_back_hosts() {
        let (store, _, root) = test_store("create-failure");
        let reusable = add_host(&store, "host@example.com", "198.18.4.1", "US").await;
        create_started_job(
            &store,
            "failure-job-idle",
            "client@example.com",
            &["host@example.com"],
            &["US"],
            "failure-client-idle",
        )
        .await;
        store
            .client_market_claim_idle_host(
                "failure-job-idle",
                &["host@example.com".into()],
                &["US".into()],
                "failure-client-idle",
            )
            .await
            .unwrap();
        store
            .client_market_finalize_create_failure(
                "failure-job-idle",
                Some(&reusable.id),
                true,
                "installer_failed",
                "rollback complete",
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .client_market_get_host(&reusable.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            HOST_STATUS_IDLE
        );

        let quarantined = add_host(&store, "quarantine-host@example.com", "198.18.4.2", "US").await;
        create_started_job(
            &store,
            "failure-job-unreachable",
            "client@example.com",
            &["quarantine-host@example.com"],
            &["US"],
            "failure-client-unreachable",
        )
        .await;
        store
            .client_market_claim_idle_host(
                "failure-job-unreachable",
                &["quarantine-host@example.com".into()],
                &["US".into()],
                "failure-client-unreachable",
            )
            .await
            .unwrap();
        store
            .client_market_finalize_create_failure(
                "failure-job-unreachable",
                Some(&quarantined.id),
                false,
                "rollback_failed",
                "operator verification required",
            )
            .await
            .unwrap();
        let quarantined = store
            .client_market_get_host(&quarantined.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(quarantined.status, HOST_STATUS_UNREACHABLE);
        assert_eq!(quarantined.last_error.as_deref(), Some("rollback_failed"));
        let reservations: i64 = {
            let conn = store.conn.lock().await;
            conn.query_row("SELECT COUNT(*) FROM subdomain_reservations", [], |row| {
                row.get(0)
            })
            .unwrap()
        };
        assert_eq!(reservations, 0);
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn cleanup_recovery_after_purge_is_idempotent_and_keeps_label_tombstoned() {
        let (store, config, root) = test_store("cleanup-recovery");
        let host = add_host(&store, "host@example.com", "198.18.5.1", "US").await;
        let unrelated_host = add_host(&store, "other@example.com", "198.18.5.2", "US").await;
        {
            let conn = store.conn.lock().await;
            insert_installation(&conn, "cleanup-installation", "client@example.com");
            conn.execute(
                "UPDATE installations
                 SET provision_source = ?2, provision_host_id = ?3
                 WHERE id = ?1",
                params![
                    "cleanup-installation",
                    PROVISION_SOURCE_ROUTER_MARKET,
                    host.id
                ],
            )
            .unwrap();
            insert_tunnel_and_public_host(
                &conn,
                "cleanup-installation",
                "client@example.com",
                "cleanup-client",
            );
            conn.execute(
                "UPDATE router_ssh_hosts
                 SET status = 'allocated', installation_id = 'cleanup-installation'
                 WHERE id = ?1",
                params![host.id],
            )
            .unwrap();
        }
        assert!(matches!(
            store
                .client_market_begin_cleanup_job("cleanup-installation", "host@example.com", false,)
                .await,
            Err(AppError::Forbidden(_))
        ));
        let job_id = store
            .client_market_begin_cleanup_job("cleanup-installation", "client@example.com", false)
            .await
            .unwrap();
        assert_eq!(
            store
                .client_market_get_host(&host.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            HOST_STATUS_DRAINING
        );
        store
            .client_market_start_job(&job_id, JOB_TYPE_CLEANUP)
            .await
            .unwrap();
        assert!(matches!(
            store
                .client_market_fail_cleanup_job(
                    &job_id,
                    &unrelated_host.id,
                    "wrong_host",
                    "must not mutate an unrelated host",
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        assert_eq!(
            store
                .client_market_get_host(&host.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            HOST_STATUS_DRAINING
        );
        assert_eq!(
            store
                .client_market_get_host(&unrelated_host.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            HOST_STATUS_IDLE
        );

        store
            .purge_installation_for_client_market("cleanup-installation")
            .await
            .unwrap();
        store
            .client_market_fail_cleanup_job(
                &job_id,
                &host.id,
                "post_purge_crash",
                "recover after purge",
            )
            .await
            .unwrap();
        store
            .client_market_fail_cleanup_job(
                &job_id,
                &host.id,
                "post_purge_crash",
                "idempotent retry",
            )
            .await
            .unwrap();
        let recovered_host = store
            .client_market_get_host(&host.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(recovered_host.status, HOST_STATUS_IDLE);
        assert!(recovered_host.installation_id.is_none());
        let job = store
            .client_market_get_job_record(&job_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.status, JOB_STATUS_SUCCEEDED);
        assert_eq!(job.phase, JOB_PHASE_COMPLETE);
        {
            let conn = store.conn.lock().await;
            assert_eq!(
                get_public_host(&conn, "cleanup-client")
                    .unwrap()
                    .unwrap()
                    .lifecycle,
                PublicHostLifecycle::Tombstoned
            );
        }
        let availability = store
            .check_client_tunnel_subdomain_availability(
                &config,
                "cleanup-client",
                None,
                Some("198.18.5.1"),
            )
            .await
            .unwrap();
        assert!(!availability.available);
        assert_eq!(availability.reason.as_deref(), Some("previously_claimed"));
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn public_host_views_hide_operational_and_owner_details() {
        let host = RouterSshHostRecord {
            id: "host-id".into(),
            ip: "203.0.113.9".into(),
            port: 2222,
            host_owner_email: "host@example.com".into(),
            country_code: Some("US".into()),
            hostname: Some("host.example".into()),
            ssh_host_key_fingerprint: Some("SHA256:secret".into()),
            status: HOST_STATUS_ALLOCATED.into(),
            client_subdomain: Some("public-client".into()),
            client_owner_email: Some("client@example.com".into()),
            installation_id: Some("installation-id".into()),
            last_verified_at: Some("verified-at".into()),
            last_error: Some("diagnostic".into()),
            note: Some("operator note".into()),
            created_at: "created-at".into(),
            updated_at: "updated-at".into(),
        };
        let public = serde_json::to_value(host_to_view(host.clone(), false)).unwrap();
        for key in [
            "ip",
            "port",
            "sshHostKeyFingerprint",
            "clientOwnerEmail",
            "installationId",
            "lastVerifiedAt",
            "lastError",
            "note",
            "createdAt",
            "updatedAt",
        ] {
            assert!(public.get(key).is_none(), "public view leaked {key}");
        }
        assert_eq!(
            public
                .get("clientSubdomain")
                .and_then(|value| value.as_str()),
            Some("public-client")
        );
        let private = serde_json::to_value(host_to_view(host, true)).unwrap();
        assert_eq!(
            private.get("ip").and_then(|value| value.as_str()),
            Some("203.0.113.9")
        );
        assert_eq!(
            private
                .get("clientOwnerEmail")
                .and_then(|value| value.as_str()),
            Some("client@example.com")
        );
    }

    #[test]
    fn host_ip_validation_rejects_non_public_ranges() {
        for value in [
            "127.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "169.254.1.1",
            "172.16.0.1",
            "192.168.0.1",
            "192.0.2.1",
            "198.51.100.1",
            "203.0.113.1",
            "192.88.99.1",
            "::1",
            "::ffff:192.168.0.1",
            "fc00::1",
            "fe80::1",
            "64:ff9b::c0a8:1",
            "2001::1",
            "2001:2::1",
            "2001:10::1",
            "2001:db8::1",
            "2002:c0a8:101::1",
        ] {
            assert!(
                parse_host_ip(value).is_err(),
                "accepted reserved IP {value}"
            );
        }
        assert_eq!(
            parse_host_ip("8.8.8.8").unwrap(),
            "8.8.8.8".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            parse_host_ip("2606:4700:4700::1111").unwrap(),
            "2606:4700:4700::1111".parse::<IpAddr>().unwrap()
        );
    }
}
