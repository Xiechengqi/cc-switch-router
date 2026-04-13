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
    DashboardResponse, DashboardStats, Installation, InstallationView, IssueLeaseRequest,
    IssueLeaseResponse, LeaseView, RegisterInstallationRequest, RegisterInstallationResponse,
    ShareBatchSyncRequest, ShareDeleteRequest, ShareDescriptor, ShareSyncRequest, ShareView,
    TunnelLease,
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

        let subdomain = normalize_subdomain(&input.requested_subdomain)?;
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

    pub async fn dashboard_snapshot(&self) -> Result<DashboardResponse, AppError> {
        let conn = self.conn.lock().await;
        let now = Utc::now();

        let installations = list_installations(&conn)?;
        let leases = list_leases(&conn)?;
        let shares = list_shares(&conn)?;

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
                    let is_active = lease.expires_at > now;
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
            .map(
                |(installation_id, share, latest_subdomain, active_lease_count)| {
                    if active_lease_count > 0 {
                        active_share_ids.insert(share.share_id.clone());
                    }
                    ShareView {
                        share_id: share.share_id,
                        share_name: share.share_name,
                        share_token: share.share_token,
                        app_type: share.app_type,
                        provider_id: share.provider_id,
                        token_limit: share.token_limit,
                        tokens_used: share.tokens_used,
                        requests_count: share.requests_count,
                        share_status: share.share_status,
                        created_at: share.created_at,
                        expires_at: share.expires_at,
                        latest_subdomain,
                        installation_id,
                        active_lease_count,
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

        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit cleanup tx failed: {e}")))?;

        Ok((deleted_leases, deleted_shares))
    }

    async fn upsert_share(
        &self,
        installation_id: &str,
        share: ShareDescriptor,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
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
            share_id, installation_id, share_name, share_token, app_type, provider_id,
            token_limit, tokens_used, requests_count, share_status, created_at, expires_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
        ON CONFLICT(share_id) DO UPDATE SET
            installation_id = excluded.installation_id,
            share_name = excluded.share_name,
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
    .map_err(|e| AppError::Internal(format!("upsert share failed: {e}")))?;
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

        CREATE INDEX IF NOT EXISTS idx_leases_installation_id ON leases(installation_id);
        CREATE INDEX IF NOT EXISTS idx_leases_subdomain ON leases(subdomain);
        CREATE INDEX IF NOT EXISTS idx_shares_installation_id ON shares(installation_id);
        ",
    )
    .map_err(|e| AppError::Internal(format!("init schema failed: {e}")))?;
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

fn list_shares(
    conn: &Connection,
) -> Result<Vec<(String, ShareDescriptor, String, usize)>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT s.installation_id, s.share_id, s.share_name, s.share_token, s.app_type, s.provider_id,
                    s.token_limit, s.tokens_used, s.requests_count, s.share_status, s.created_at, s.expires_at,
                    COALESCE((
                        SELECT l.subdomain FROM leases l
                        WHERE json_extract(l.share_json, '$.shareId') = s.share_id
                        ORDER BY l.issued_at DESC
                        LIMIT 1
                    ), '-') AS latest_subdomain,
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
                    share_token: row.get(3)?,
                    app_type: row.get(4)?,
                    provider_id: row.get(5)?,
                    token_limit: row.get(6)?,
                    tokens_used: row.get(7)?,
                    requests_count: row.get(8)?,
                    share_status: row.get(9)?,
                    created_at: row.get(10)?,
                    expires_at: row.get(11)?,
                },
                row.get(12)?,
                row.get::<_, i64>(13)? as usize,
            ))
        })
        .map_err(|e| AppError::Internal(format!("query shares failed: {e}")))?;
    collect_rows(rows)
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
    if !value
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(AppError::BadRequest("invalid subdomain".into()));
    }
    Ok(value)
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
