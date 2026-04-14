use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rand::distributions::{Alphanumeric, DistString};
use rusqlite::{Connection, OptionalExtension, params};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::config::Config;
use crate::error::AppError;
use crate::models::{
    DashboardResponse, DashboardStats, HealthCheckEntry, Installation, InstallationView,
    IssueLeaseRequest, IssueLeaseResponse, LeaseView, RegisterInstallationRequest,
    RegisterInstallationResponse, ShareBatchSyncRequest, ShareClaimSubdomainRequest,
    ShareDeleteRequest, ShareDescriptor, ShareHeartbeatRequest, ShareRequestLogBatchSyncRequest,
    ShareRequestLogEntry, ShareSyncRequest, ShareView, TunnelLease,
};
use crate::proxy::ProxyRegistry;

#[derive(Clone)]
pub struct AppStore {
    conn: Arc<Mutex<Connection>>,
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
    ) -> Result<RegisterInstallationResponse, AppError> {
        if input.public_key.trim().is_empty() {
            return Err(AppError::BadRequest("public_key is required".into()));
        }
        let now = Utc::now();
        let installation = Installation {
            id: Uuid::new_v4().to_string(),
            public_key: input.public_key,
            platform: input.platform,
            app_version: input.app_version,
            created_at: now,
            last_seen_at: now,
        };
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO installations (id, public_key, platform, app_version, created_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                installation.id,
                installation.public_key,
                installation.platform,
                installation.app_version,
                installation.created_at.to_rfc3339(),
                installation.last_seen_at.to_rfc3339(),
            ],
        )
        .map_err(|e| AppError::Internal(format!("insert installation failed: {e}")))?;
        Ok(RegisterInstallationResponse {
            installation_id: installation.id,
        })
    }

    pub async fn issue_lease(
        &self,
        config: &Config,
        proxy: &ProxyRegistry,
        input: IssueLeaseRequest,
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
            conn.execute(
                "UPDATE installations SET last_seen_at = ?2 WHERE id = ?1",
                params![input.installation_id, now.to_rfc3339()],
            )
            .map_err(|e| AppError::Internal(format!("update installation failed: {e}")))?;
            installation
        };

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

    pub async fn sync_share(&self, input: ShareSyncRequest) -> Result<(), AppError> {
        {
            let conn = self.conn.lock().await;
            let exists = get_installation(&conn, &input.installation_id)?.is_some();
            if !exists {
                return Err(AppError::Unauthorized("installation not found".into()));
            }
        }
        self.upsert_share(&input.installation_id, input.share).await
    }

    pub async fn claim_share_subdomain(
        &self,
        input: ShareClaimSubdomainRequest,
    ) -> Result<(), AppError> {
        let subdomain = normalize_subdomain(&input.share.subdomain)?;
        ensure_subdomain_allowed(&subdomain)?;
        let conn = self.conn.lock().await;
        let exists = get_installation(&conn, &input.installation_id)?.is_some();
        if !exists {
            return Err(AppError::Unauthorized("installation not found".into()));
        }

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

    pub async fn batch_sync_shares(&self, input: ShareBatchSyncRequest) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let exists = get_installation(&conn, &input.installation_id)?.is_some();
        if !exists {
            return Err(AppError::Unauthorized("installation not found".into()));
        }

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
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let exists = get_installation(&conn, &input.installation_id)?.is_some();
        if !exists {
            return Err(AppError::Unauthorized("installation not found".into()));
        }

        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AppError::Internal(format!("begin request log batch sync tx failed: {e}")))?;
        for log in input.logs {
            upsert_share_request_log_tx(&tx, &input.installation_id, log)?;
        }
        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit request log batch sync failed: {e}")))?;
        Ok(())
    }

    pub async fn dashboard_snapshot(
        &self,
        proxy: &ProxyRegistry,
    ) -> Result<DashboardResponse, AppError> {
        let active_subdomains = proxy.active_subdomains().await.into_iter().collect::<HashSet<_>>();
        let conn = self.conn.lock().await;
        let now = Utc::now();

        let installations = list_installations(&conn)?;
        let leases = list_leases(&conn)?;
        let shares = list_shares(&conn)?;
        let health_by_share = list_health_checks(&conn, 10)?;
        let recent_logs = list_recent_share_request_logs(&conn, 8)?;
        let logs_by_share = recent_logs
            .into_iter()
            .fold(HashMap::<String, Vec<ShareRequestLogEntry>>::new(), |mut acc, log| {
                acc.entry(log.share_id.clone()).or_default().push(log);
                acc
            });

        let mut leases_by_installation: HashMap<String, Vec<TunnelLease>> = HashMap::new();
        for lease in leases {
            leases_by_installation
                .entry(lease.installation_id.clone())
                .or_default()
                .push(lease);
        }

        let mut installation_views = Vec::new();
        for installation in installations {
            let mut lease_views = Vec::new();
            let mut active_lease_count = 0usize;
            if let Some(items) = leases_by_installation.get(&installation.id) {
                for lease in items {
                    let is_active = active_subdomains.contains(&lease.subdomain);
                    if is_active {
                        active_lease_count += 1;
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
            installation_views.push(InstallationView {
                id: installation.id,
                platform: installation.platform,
                app_version: installation.app_version,
                created_at: installation.created_at,
                last_seen_at: installation.last_seen_at,
                active_lease_count,
                leases: lease_views,
            });
        }
        installation_views.sort_by(|a, b| b.last_seen_at.cmp(&a.last_seen_at));

        let mut active_share_ids = HashSet::new();
        let share_views = shares
            .into_iter()
            .map(|(installation_id, share, _active_lease_count)| {
                    let active_lease_count =
                        usize::from(active_subdomains.contains(&share.subdomain));
                    if active_lease_count > 0 {
                        active_share_ids.insert(share.share_id.clone());
                    }
                    let recent_requests = logs_by_share
                        .get(&share.share_id)
                        .cloned()
                        .unwrap_or_default();
                    let health_checks = health_by_share
                        .get(&share.share_id)
                        .cloned()
                        .unwrap_or_default();
                    ShareView {
                        share_id: share.share_id,
                        share_name: share.share_name,
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
                        installation_id,
                        active_lease_count,
                        recent_requests,
                        health_checks,
                    }
                },
            )
            .collect::<Vec<_>>();

        Ok(DashboardResponse {
            generated_at: now,
            stats: DashboardStats {
                installations: installation_views.len(),
                shares: share_views.len(),
                active_leases: installation_views
                    .iter()
                    .map(|installation| installation.active_lease_count)
                    .sum(),
                active_shares: active_share_ids.len(),
            },
            installations: installation_views,
            shares: share_views,
        })
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
                params![DateTime::parse_from_rfc3339(&cutoff)
                    .map(|dt| dt.timestamp())
                    .unwrap_or_default()],
            )
            .map_err(|e| AppError::Internal(format!("delete stale request logs failed: {e}")))?;

        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit cleanup tx failed: {e}")))?;

        Ok((deleted_leases, deleted_shares))
    }

    /// Record a heartbeat reported by cc-switch for a share.
    pub async fn record_share_heartbeat(
        &self,
        input: ShareHeartbeatRequest,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let exists = get_installation(&conn, &input.installation_id)?.is_some();
        if !exists {
            return Err(AppError::Unauthorized("installation not found".into()));
        }
        let now = Utc::now().timestamp();
        conn.execute(
            "INSERT INTO share_health_checks (share_id, checked_at, is_healthy) VALUES (?1, ?2, 1)",
            params![input.share_id, now],
        )
        .map_err(|e| AppError::Internal(format!("insert heartbeat failed: {e}")))?;
        // Prune entries older than 15 minutes.
        conn.execute(
            "DELETE FROM share_health_checks WHERE checked_at < ?1",
            params![now - 900],
        )
        .map_err(|e| AppError::Internal(format!("prune heartbeats failed: {e}")))?;
        Ok(())
    }

    async fn upsert_share(
        &self,
        installation_id: &str,
        mut share: ShareDescriptor,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let existing_subdomain = get_share_owned_subdomain(&conn, installation_id, &share.share_id)?
            .ok_or_else(|| AppError::Conflict("share subdomain is not claimed".into()))?;
        share.subdomain = existing_subdomain;
        upsert_share_tx(&conn, installation_id, share)?;
        Ok(())
    }
}

fn upsert_share_tx(
    conn: &Connection,
    installation_id: &str,
    share: ShareDescriptor,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO shares (
            share_id, installation_id, share_name, subdomain, share_token, app_type, provider_id,
            token_limit, tokens_used, requests_count, share_status, created_at, expires_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
        ON CONFLICT(share_id) DO UPDATE SET
            installation_id = excluded.installation_id,
            share_name = excluded.share_name,
            subdomain = excluded.subdomain,
            share_token = excluded.share_token,
            app_type = excluded.app_type,
            provider_id = excluded.provider_id,
            token_limit = excluded.token_limit,
            tokens_used = excluded.tokens_used,
            requests_count = excluded.requests_count,
            share_status = excluded.share_status,
            created_at = excluded.created_at,
            expires_at = excluded.expires_at,
            updated_at = excluded.updated_at",
        params![
            share.share_id,
            installation_id,
            share.share_name,
            share.subdomain,
            share.share_token,
            share.app_type,
            share.provider_id,
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
            subdomain TEXT,
            share_token TEXT NOT NULL,
            app_type TEXT NOT NULL,
            provider_id TEXT,
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

        CREATE INDEX IF NOT EXISTS idx_leases_installation_id ON leases(installation_id);
        CREATE INDEX IF NOT EXISTS idx_leases_subdomain ON leases(subdomain);
        CREATE INDEX IF NOT EXISTS idx_shares_installation_id ON shares(installation_id);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_shares_subdomain_unique ON shares(subdomain) WHERE subdomain IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_share_request_logs_share_id ON share_request_logs(share_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_share_health_checks ON share_health_checks(share_id, checked_at DESC);
        ",
    )
    .map_err(|e| AppError::Internal(format!("init schema failed: {e}")))?;
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
        "SELECT id, public_key, platform, app_version, created_at, last_seen_at
         FROM installations WHERE id = ?1",
        params![installation_id],
        |row| {
            Ok(Installation {
                id: row.get(0)?,
                public_key: row.get(1)?,
                platform: row.get(2)?,
                app_version: row.get(3)?,
                created_at: parse_dt_sql(&row.get::<_, String>(4)?)?,
                last_seen_at: parse_dt_sql(&row.get::<_, String>(5)?)?,
            })
        },
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query installation failed: {e}")))
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
            "SELECT id, public_key, platform, app_version, created_at, last_seen_at
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
                created_at: parse_dt_sql(&row.get::<_, String>(4)?)?,
                last_seen_at: parse_dt_sql(&row.get::<_, String>(5)?)?,
            })
        })
        .map_err(|e| AppError::Internal(format!("query installations failed: {e}")))?;
    collect_rows(rows)
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
            "SELECT s.installation_id, s.share_id, s.share_name, COALESCE(s.subdomain, '-'), s.share_token, s.app_type, s.provider_id,
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
                    subdomain: row.get(3)?,
                    share_token: row.get(4)?,
                    app_type: row.get(5)?,
                    provider_id: row.get(6)?,
                    token_limit: row.get(7)?,
                    tokens_used: row.get(8)?,
                    requests_count: row.get(9)?,
                    share_status: row.get(10)?,
                    created_at: row.get(11)?,
                    expires_at: row.get(12)?,
                },
                row.get::<_, i64>(13)? as usize,
            ))
        })
        .map_err(|e| AppError::Internal(format!("query shares failed: {e}")))?;
    collect_rows(rows)
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
    collect_rows(rows)
}

fn list_health_checks(conn: &Connection, limit: usize) -> Result<HashMap<String, Vec<HealthCheckEntry>>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT share_id, checked_at, is_healthy
             FROM (
                 SELECT share_id, checked_at, is_healthy,
                        ROW_NUMBER() OVER (PARTITION BY share_id ORDER BY checked_at DESC) AS rn
                 FROM share_health_checks
             )
             WHERE rn <= ?1
             ORDER BY checked_at ASC",
        )
        .map_err(|e| AppError::Internal(format!("prepare health checks failed: {e}")))?;
    let rows = stmt
        .query_map(params![limit as i64], |row| {
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
        let (share_id, entry) = row.map_err(|e| AppError::Internal(format!("read health check row failed: {e}")))?;
        map.entry(share_id).or_default().push(entry);
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
