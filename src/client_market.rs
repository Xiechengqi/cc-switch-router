use std::collections::{HashMap, HashSet};
use std::fs;
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path as AxumPath, Query, State};
use axum::http::{HeaderMap, header};
use axum::routing::{delete, get, post};
use base64::Engine;
use chrono::Utc;
use futures_util::{StreamExt, stream};
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
/// Legacy / umbrella cleanup phase (still accepted when resuming older jobs).
const JOB_PHASE_CLEANUP: &str = "cleanup_remote";
const JOB_PHASE_CLEANUP_STOP: &str = "cleanup_stop";
const JOB_PHASE_CLEANUP_WIPE: &str = "cleanup_wipe";
const JOB_PHASE_CLEANUP_PURGE: &str = "cleanup_purge";
const JOB_PHASE_COMPLETE: &str = "complete";
const JOB_PHASE_ROLLBACK: &str = "rollback";

const CLEANUP_FAILURE_SSH_TIMEOUT: &str = "cleanup_ssh_timeout";
const CLEANUP_FAILURE_STOP: &str = "cleanup_stop_failed";
const CLEANUP_FAILURE_WIPE: &str = "cleanup_wipe_failed";
const CLEANUP_FAILURE_PURGE: &str = "cleanup_purge_failed";
const CLEANUP_FAILURE_FINGERPRINT: &str = "cleanup_fingerprint_mismatch";
const CLEANUP_FAILURE_BINDING: &str = "cleanup_host_binding_mismatch";
const CLEANUP_FAILURE_GENERIC: &str = "cleanup_failed";

const CLEANUP_PURGE_ATTEMPTS: u32 = 3;
const CLEANUP_PURGE_RETRY_BASE: Duration = Duration::from_secs(1);
/// Draining hosts without an active cleanup job longer than this are repaired.
const STALE_DRAINING_AFTER: Duration = Duration::from_secs(10 * 60);
/// Unreachable hosts that look remotely clean are auto-healed after this age.
const UNREACHABLE_AUTO_HEAL_AFTER: Duration = Duration::from_secs(5 * 60);
/// Hosts claimed for provisioning but with no active job for longer than this are
/// stranded — the worker panicked, or the process died between claim and spawn.
/// `locked` has no other exit, so without this sweep the Host leaves the pool for
/// good. The active-job check is the real guard; this floor is set above the worst
/// case a create job can legitimately hold `locked`
/// (`SSH_INSTALL_TIMEOUT` + `PROVISION_POLL_TIMEOUT` = 20 min) so the sweep stays
/// safe even if that check ever regresses.
const STALE_LOCKED_AFTER: Duration = Duration::from_secs(25 * 60);
/// Reserved hosts whose quote is gone are returned to the pool after this age.
/// Quote TTL is 120s; this leaves generous headroom for a commit in flight.
const STALE_RESERVED_AFTER: Duration = Duration::from_secs(10 * 60);

const SUBDOMAIN_RESERVATION_TTL_MS: i64 = 30 * 60 * 1000;
const PROVISION_POLL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const PROVISION_POLL_INTERVAL: Duration = Duration::from_secs(15);
const PROVISION_SECRET_TTL: Duration = Duration::from_secs(15 * 60);
const PROVISION_REDEEM_RETRY_TTL: Duration = Duration::from_secs(2 * 60);
const SSH_VERIFY_TIMEOUT: Duration = Duration::from_secs(180);
const SSH_INSTALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const SSH_CLEANUP_TIMEOUT: Duration = Duration::from_secs(90);
const SSH_OUTPUT_LIMIT: usize = 64 * 1024;
const JOB_LOG_LIMIT: usize = 128 * 1024;
const MAX_SELECTION_ITEMS: usize = 100;
const MAX_NOTE_BYTES: usize = 500;

/// POSIX snippet used on remote hosts (Alpine ash or bash) to install market deps.
/// Most Client Market hosts are Alpine Docker images without bash/curl by default.
const REMOTE_ENSURE_CLIENT_MARKET_DEPS: &str = r#"
ensure_client_market_deps() {
  need_bash=0; need_curl=0; need_ping=0
  command -v bash >/dev/null 2>&1 || need_bash=1
  command -v curl >/dev/null 2>&1 || need_curl=1
  command -v ping >/dev/null 2>&1 || need_ping=1
  if [ "$need_bash$need_curl$need_ping" = "000" ]; then
    return 0
  fi
  if command -v apk >/dev/null 2>&1; then
    set --
    [ "$need_bash" -eq 1 ] && set -- "$@" bash
    [ "$need_curl" -eq 1 ] && set -- "$@" curl ca-certificates
    [ "$need_ping" -eq 1 ] && set -- "$@" iputils
    echo "installing host dependencies via apk: $*" >&2
    apk add --no-cache "$@" || return 1
    if [ "$need_curl" -eq 1 ] && command -v update-ca-certificates >/dev/null 2>&1; then
      update-ca-certificates >/dev/null 2>&1 || true
    fi
  elif command -v apt-get >/dev/null 2>&1; then
    set --
    [ "$need_bash" -eq 1 ] && set -- "$@" bash
    [ "$need_curl" -eq 1 ] && set -- "$@" curl ca-certificates
    [ "$need_ping" -eq 1 ] && set -- "$@" iputils-ping
    echo "installing host dependencies via apt-get: $*" >&2
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq || return 1
    apt-get install -y -qq "$@" || return 1
  else
    echo "missing required commands and no apk/apt-get available to install them" >&2
    [ "$need_bash" -eq 1 ] && echo "  - bash" >&2
    [ "$need_curl" -eq 1 ] && echo "  - curl" >&2
    [ "$need_ping" -eq 1 ] && echo "  - ping" >&2
    return 127
  fi
  command -v bash >/dev/null 2>&1 || { echo "required command still missing: bash" >&2; return 127; }
  command -v curl >/dev/null 2>&1 || { echo "required command still missing: curl" >&2; return 127; }
  command -v ping >/dev/null 2>&1 || { echo "required command still missing: ping" >&2; return 127; }
  return 0
}
"#;

/// Detect/stop a live cc-switch-server without matching this SSH checker's own cmdline.
/// Match the process COMMAND (e.g. `/usr/local/bin/cc-switch-server`), not a substring of
/// the remote `sh -c '...'` script which also contains that path.
const REMOTE_CC_SWITCH_SERVER_HELPERS: &str = r#"
cc_switch_server_pkill() {
  if command -v pkill >/dev/null 2>&1; then
    pkill cc-switch-server 2>/dev/null || true
    return 0
  fi
  for comm in /proc/[0-9]*/comm; do
    [ -r "$comm" ] || continue
    name=$(cat "$comm" 2>/dev/null) || continue
    [ "$name" = "cc-switch-server" ] || continue
    pid=${comm#/proc/}
    pid=${pid%/comm}
    kill "$pid" 2>/dev/null || true
  done
}
cc_switch_server_is_running() {
  if command -v ps >/dev/null 2>&1; then
    # BusyBox: PID USER TIME COMMAND...
    # GNU:     UID PID PPID C STIME TTY TIME CMD...
    # Only match argv0 being the binary — never the SSH helper shell cmdline.
    if ps -ef 2>/dev/null | awk '
      NR == 1 { next }
      {
        cmd = ""
        if ($1 ~ /^[0-9]+$/ && NF >= 4 && $3 ~ /^[0-9:.-]+$/) {
          cmd = $4
        } else if (NF >= 8) {
          cmd = $8
        } else {
          next
        }
        if (cmd == "cc-switch-server" || cmd ~ /\/cc-switch-server$/) exit 0
      }
      END { exit 1 }
    '; then
      return 0
    fi
  fi
  for comm in /proc/[0-9]*/comm; do
    [ -r "$comm" ] || continue
    name=$(cat "$comm" 2>/dev/null) || continue
    [ "$name" = "cc-switch-server" ] && return 0
  done
  return 1
}
cc_switch_server_stop() {
  if ! cc_switch_server_is_running; then
    return 0
  fi
  cc_switch_server_pkill
  attempt=0
  while [ "$attempt" -lt 3 ]; do
    sleep 3
    if ! cc_switch_server_is_running; then
      return 0
    fi
    cc_switch_server_pkill
    attempt=$((attempt + 1))
  done
  # Three spaced ps checks still saw the process.
  return 1
}
cc_switch_server_home() {
  home="${HOME:-}"
  if [ -z "$home" ]; then
    user="$(id -un 2>/dev/null || echo root)"
    home="$(getent passwd "$user" 2>/dev/null | cut -d: -f6 || true)"
  fi
  if [ -z "$home" ]; then
    home=/root
  fi
  printf '%s\n' "$home"
}
cc_switch_server_wipe_files() {
  home="$(cc_switch_server_home)"
  rm -f /usr/local/bin/cc-switch-server
  rm -rf "${home}/.cc-switch-server"
  # Unmatched globs must not abort under `set -e`.
  for path in "${home}"/.cc-switch-server.bak.*; do
    [ -e "$path" ] || continue
    rm -rf "$path"
  done
}
cc_switch_server_has_install_files() {
  home="$(cc_switch_server_home)"
  if [ -e /usr/local/bin/cc-switch-server ] || [ -e "${home}/.cc-switch-server" ]; then
    return 0
  fi
  for path in "${home}"/.cc-switch-server.bak.*; do
    [ -e "$path" ] || continue
    return 0
  done
  return 1
}
"#;
const MAX_PASSWORD_BYTES: usize = 1024;
const HOST_REGISTRATIONS_PER_OWNER_HOUR: u32 = 100;
const HOST_REGISTRATIONS_PER_TARGET_HOUR: u32 = 5;
const HOST_REGISTRATIONS_PER_SOURCE_HOUR: u32 = 120;

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
    ) -> Result<ProvisionTokenSecret, AppError> {
        self.prune();
        let secret = self.tokens.get_mut(token_hash).ok_or_else(|| {
            AppError::NotFound("provision credential not found or expired".into())
        })?;
        let expected = normalize_ip_for_compare(secret.host_ip);
        let actual = normalize_ip_for_compare(source_ip);
        if expected != actual {
            return Err(AppError::Unauthorized(format!(
                "provisioning host IP mismatch (expected {expected}, got {actual})"
            )));
        }
        secret.redeemed_at.get_or_insert_with(Instant::now);
        Ok(secret.clone())
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
        );

        CREATE TABLE IF NOT EXISTS client_market_host_import_jobs (
            id TEXT PRIMARY KEY,
            provider_id TEXT NOT NULL,
            owner_email TEXT NOT NULL,
            source_ip TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            completed_at TEXT
        );
        CREATE TABLE IF NOT EXISTS client_market_host_import_items (
            id TEXT PRIMARY KEY,
            job_id TEXT NOT NULL,
            position INTEGER NOT NULL,
            ip TEXT NOT NULL,
            port INTEGER NOT NULL,
            payload_json TEXT NOT NULL,
            status TEXT NOT NULL,
            host_id TEXT,
            error_message TEXT,
            attempts INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE(job_id, position),
            UNIQUE(job_id, ip, port)
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
    add_column_if_missing(conn, "router_ssh_hosts", "ip_intel_json", "TEXT")?;
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
            WHERE installation_id IS NOT NULL;
         CREATE INDEX IF NOT EXISTS idx_client_market_host_import_jobs_status
            ON client_market_host_import_jobs(status, updated_at);
         CREATE INDEX IF NOT EXISTS idx_client_market_host_import_items_pending
            ON client_market_host_import_items(job_id, status, position);",
    )?;
    crate::client_market_trade::init_schema(conn)?;
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
        .is_some_and(|value| {
            normalize_ip_for_compare(value) == normalize_ip_for_compare(source_ip)
        }))
}

fn reservation_has_active_create_job(
    conn: &Connection,
    reservation: &ActiveSubdomainReservation,
) -> Result<bool, AppError> {
    let Some(host_id) = reservation.host_id.as_deref() else {
        return Ok(false);
    };
    let active_job: Option<(String, String)> = conn
        .query_row(
            "SELECT status, phase FROM provisioning_jobs
             WHERE id = ?1 AND host_id = ?2 AND type = 'create'",
            params![reservation.job_id, host_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| AppError::Internal(format!("read reservation job failed: {e}")))?;
    Ok(active_job.is_some_and(|(status, phase)| {
        status == JOB_STATUS_RUNNING
            && matches!(
                phase.as_str(),
                JOB_PHASE_LOCKED | JOB_PHASE_INSTALLING | JOB_PHASE_WAITING
            )
    }))
}

/// Apply the global Client Market reservation to the public availability API.
/// Calls from the selected host are allowed through so the setup preflight can run;
/// every other installation sees the label as unavailable.
///
/// When peer IP cannot be trusted (e.g. cloudflared → localhost hides CF headers),
/// an unbound reservation with an active create job still passes preflight. Claim
/// remains gated by matching owner email + active job.
pub(crate) fn client_market_subdomain_available_to_source(
    conn: &Connection,
    subdomain: &str,
    installation_id: Option<&str>,
    source_ip: Option<&str>,
) -> Result<bool, AppError> {
    let Some(reservation) = get_active_subdomain_reservation(conn, subdomain)? else {
        return Ok(true);
    };
    let ip_ok = reservation_source_matches_host(conn, &reservation, source_ip)?;
    if !ip_ok {
        if reservation.installation_id.is_none()
            && reservation_has_active_create_job(conn, &reservation)?
        {
            return Ok(true);
        }
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
    if reservation.client_owner_email.as_deref() != Some(owner_email) {
        return Err(AppError::Conflict(
            "subdomain is reserved for another provisioning job".into(),
        ));
    }
    let ip_ok = reservation_source_matches_host(conn, &reservation, source_ip)?;
    if !ip_ok && !reservation_has_active_create_job(conn, &reservation)? {
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
    if !reservation_has_active_create_job(conn, &reservation)? {
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
        .route("/v1/client-market/hosts/export", get(export_hosts))
        .route("/v1/client-market/hosts/import", post(import_hosts))
        .route("/v1/client-market/hosts/test-ssh", post(test_host_ssh))
        .route("/v1/client-market/hosts/ip-info", post(lookup_host_ip_info))
        .route("/v1/client-market/supply-summary", get(supply_summary))
        .route("/v1/client-market/hosts/:id", delete(delete_host))
        .route("/v1/client-market/hosts/:id/reverify", post(reverify_host))
        .route("/v1/client-market/jobs/:id", get(get_job))
        .route(
            "/v1/client-market/clients/:installation_id/cleanup",
            post(cleanup_client),
        )
        .route(
            "/v1/client-market/clients/:installation_id/release",
            post(release_client),
        )
        .route(
            "/v1/client-market/clients/:installation_id/provider-cleanup",
            post(provider_cleanup_client),
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
    provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    host_owner_email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_cents: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rental_period_days: Option<i64>,
    offer_revision: i64,
    #[serde(default)]
    payment_method_kinds: Vec<String>,
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
    /// True when the current viewer may open Web Terminal for this host.
    #[serde(default)]
    can_web_terminal: bool,
    #[serde(default)]
    is_host_owner: bool,
    #[serde(default)]
    is_client_owner: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_verified_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ip_intel: Option<crate::ip_iq::HostIpIntel>,
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
    let viewer = extract_optional_session(&state, &headers).await?;
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
            // Host operations (port, SSH details, notes, etc.) are host-owner only.
            // Admins / Router owners do not get elevated market host privileges.
            let is_host_owner = viewer.as_ref().is_some_and(|session| {
                session_is_host_owner(
                    session,
                    host.provider_id.as_deref(),
                    &host.host_owner_email,
                )
            });
            let is_client_owner = viewer.as_ref().is_some_and(|session| {
                host.client_owner_user_id.as_deref() == Some(session.user_id.as_str())
            });
            let reveal_operations = is_host_owner;
            let reveal_installation = reveal_operations || is_client_owner;
            // Web Terminal is host-owner only — not admins or client owners.
            let can_web_terminal = is_host_owner;
            let ip_intel = host_ip_intel_for_viewer(
                host.ip_intel_json.as_deref(),
                &host.ip,
                reveal_operations,
            );
            // Market listings always show the full IP; only port stays owner-private.
            RouterSshHostView {
                id: host.id,
                provider_id: host.provider_id,
                ip: Some(host.ip.clone()),
                port: reveal_operations.then_some(host.port),
                host_owner_email: host.host_owner_email,
                price_cents: host.price_cents,
                rental_period_days: host.rental_period_days,
                offer_revision: host.offer_revision,
                payment_method_kinds: host.payment_method_kinds,
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
                can_web_terminal,
                is_host_owner,
                is_client_owner,
                last_verified_at: reveal_operations.then_some(host.last_verified_at).flatten(),
                last_error: reveal_operations.then_some(host.last_error).flatten(),
                note: reveal_operations.then_some(host.note).flatten(),
                ip_intel,
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
    /// Optional root password used only to install the provision public key.
    /// Never persisted; dropped when the request handler returns.
    root_password: Option<String>,
    price_cents: Option<i64>,
    rental_period_days: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HostTransferEntry {
    ip: String,
    port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    price_cents: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rental_period_days: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    informational_status: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HostTransferDocument {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exported_at: Option<String>,
    hosts: Vec<HostTransferEntry>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HostImportItemResult {
    ip: String,
    port: u16,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    host_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HostImportResponse {
    job_id: String,
    imported: usize,
    skipped: usize,
    failed: usize,
    items: Vec<HostImportItemResult>,
}

#[derive(Debug)]
struct HostImportJobWork {
    provider_id: String,
    owner_email: String,
    source_ip: IpAddr,
    items: Vec<HostImportItemWork>,
}

#[derive(Debug)]
struct HostImportItemWork {
    id: String,
    entry: HostTransferEntry,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TestHostSshRequest {
    ip: String,
    port: Option<u16>,
    root_password: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TestHostSshResponse {
    ok: bool,
}

async fn test_host_ssh(
    State(state): State<ServerState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<TestHostSshRequest>,
) -> Result<Json<TestHostSshResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
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
    if !state
        .client_market_job_secrets
        .lock()
        .await
        .allow_host_registration(&session.user_id, ip, source_ip)
    {
        return Err(AppError::TooManyRequests(
            "host verification rate limit exceeded".into(),
        ));
    }
    let known_hosts = known_hosts_path(&state.config);
    if let Some(password) = input.root_password.as_deref() {
        validate_root_password(password)?;
        ssh_test_login_with_password(&ip.to_string(), port, password, &known_hosts).await?;
    } else {
        ssh_test_login(&state, &ip.to_string(), port, &known_hosts).await?;
    }
    Ok(Json(TestHostSshResponse { ok: true }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostIpInfoRequest {
    ip: String,
}

async fn lookup_host_ip_info(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<HostIpInfoRequest>,
) -> Result<Json<crate::ip_iq::HostIpIntel>, AppError> {
    let _owner = require_session_email(&state, &headers).await?;
    let ip = parse_host_ip(&input.ip)?;
    let intel = crate::ip_iq::lookup_host_ip_intel(&state.config.ip_intel_endpoints, &ip.to_string()).await?;
    Ok(Json(intel))
}

async fn create_host(
    State(state): State<ServerState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<CreateHostRequest>,
) -> Result<Json<RouterSshHostView>, AppError> {
    let session = require_session(&state, &headers).await?;
    let owner = session.email.clone();
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
        .allow_host_registration(&session.user_id, ip, source_ip)
    {
        return Err(AppError::TooManyRequests(
            "host verification rate limit exceeded".into(),
        ));
    }
    let known_hosts = known_hosts_path(&state.config);
    if let Some(password) = input.root_password.as_deref() {
        validate_root_password(password)?;
        ssh_install_provision_key_with_password(
            &ip.to_string(),
            port,
            password,
            &state.provision_ssh_authorized_keys_line,
            &known_hosts,
        )
        .await?;
    }
    let (hostname, fingerprint) =
        ssh_verify_host(&state, &ip.to_string(), port, &known_hosts).await?;
    let intel = crate::ip_iq::lookup_host_ip_intel(&state.config.ip_intel_endpoints, &ip.to_string()).await?;
    let intel_json = serde_json::to_string(&intel)
        .map_err(|e| AppError::Internal(format!("serialize host ip intel failed: {e}")))?;
    let (price_cents, rental_period_days) =
        crate::client_market_trade::validate_offer(input.price_cents, input.rental_period_days)?;
    if price_cents.is_some() {
        let conn = state.store.conn.lock().await;
        crate::client_market_trade::require_payment_profile_for_offer(&conn, &session.user_id)?;
    }
    state
        .store
        .client_market_ensure_provider(&session.user_id, &owner)
        .await?;
    let host = state
        .store
        .client_market_insert_host_for_provider(
            &session.user_id,
            &owner,
            &ip.to_string(),
            port,
            Some(&intel.country_code),
            hostname.as_deref(),
            fingerprint.as_deref(),
            input.note.as_deref(),
            Some(&intel_json),
            price_cents,
            rental_period_days,
        )
        .await?;
    Ok(Json(host_to_view(host, true)))
}

async fn export_hosts(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<HostTransferDocument>, AppError> {
    let session = require_session(&state, &headers).await?;
    let hosts = state
        .store
        .client_market_list_hosts(None, None, None)
        .await?;
    let hosts = hosts
        .into_iter()
        .filter(|host| {
            session_is_host_owner(
                &session,
                host.provider_id.as_deref(),
                &host.host_owner_email,
            )
        })
        .map(|host| HostTransferEntry {
            ip: host.ip,
            port: host.port,
            note: host.note,
            price_cents: host.price_cents,
            rental_period_days: host.rental_period_days,
            expected_fingerprint: host.ssh_host_key_fingerprint,
            informational_status: Some(host.status),
        })
        .collect();
    Ok(Json(HostTransferDocument {
        version: 1,
        exported_at: Some(Utc::now().to_rfc3339()),
        hosts,
    }))
}

async fn import_hosts(
    State(state): State<ServerState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<HostImportResponse>, AppError> {
    const MAX_IMPORT_BYTES: usize = 1024 * 1024;
    const MAX_IMPORT_HOSTS: usize = 100;
    if body.len() > MAX_IMPORT_BYTES {
        return Err(AppError::BadRequest(
            "Host import exceeds the 1 MB limit".into(),
        ));
    }
    let mut document: HostTransferDocument = serde_json::from_slice(&body).map_err(|_| {
        AppError::BadRequest("Host import must be a valid versioned JSON document".into())
    })?;
    if document.version != 1 {
        return Err(AppError::BadRequest(
            "unsupported Host import version".into(),
        ));
    }
    if document.hosts.is_empty() || document.hosts.len() > MAX_IMPORT_HOSTS {
        return Err(AppError::BadRequest(
            "Host import must contain 1 to 100 Hosts".into(),
        ));
    }
    let mut endpoints = HashSet::new();
    document
        .hosts
        .retain(|host| endpoints.insert((host.ip.trim().to_string(), host.port)));
    let session = require_session(&state, &headers).await?;
    let source_ip = crate::client_meta::extract_client_metadata(&headers, addr)
        .ip
        .as_deref()
        .and_then(|value| value.parse::<IpAddr>().ok())
        .unwrap_or_else(|| addr.ip());
    state
        .store
        .client_market_ensure_provider(&session.user_id, &session.email)
        .await?;
    let job_id = state
        .store
        .client_market_create_host_import_job(
            &session.user_id,
            &session.email,
            source_ip,
            &document.hosts,
        )
        .await?;
    let runner_state = state.clone();
    let runner_job_id = job_id.clone();
    let response =
        tokio::spawn(async move { process_host_import_job(&runner_state, &runner_job_id).await })
            .await
            .map_err(|error| {
                AppError::Internal(format!("Host import worker stopped: {error}"))
            })??;
    Ok(Json(response))
}

async fn process_host_import_job(
    state: &ServerState,
    job_id: &str,
) -> Result<HostImportResponse, AppError> {
    let work = state
        .store
        .client_market_claim_host_import_job(job_id)
        .await?;
    let results = stream::iter(work.items.into_iter().map(|item| {
        let state = state.clone();
        let provider_id = work.provider_id.clone();
        let owner_email = work.owner_email.clone();
        async move {
            let result = import_one_host(
                state.clone(),
                provider_id,
                owner_email,
                work.source_ip,
                item.entry,
            )
            .await;
            state
                .store
                .client_market_finish_host_import_item(&item.id, &result)
                .await?;
            Ok::<_, AppError>(result)
        }
    }))
    .buffer_unordered(5)
    .collect::<Vec<Result<HostImportItemResult, AppError>>>()
    .await;
    for result in results {
        result?;
    }
    state
        .store
        .client_market_complete_host_import_job(job_id)
        .await
}

async fn import_one_host(
    state: ServerState,
    provider_id: String,
    owner_email: String,
    source_ip: IpAddr,
    entry: HostTransferEntry,
) -> HostImportItemResult {
    let ip_label = entry.ip.trim().to_string();
    let port = entry.port;
    let result = async {
        let ip = parse_host_ip(&ip_label)?;
        if port == 0 {
            return Err(AppError::BadRequest(
                "SSH port must be greater than zero".into(),
            ));
        }
        if entry
            .note
            .as_ref()
            .is_some_and(|note| note.len() > MAX_NOTE_BYTES)
        {
            return Err(AppError::BadRequest(
                "Host note cannot exceed 500 bytes".into(),
            ));
        }
        let (price, period) = crate::client_market_trade::validate_offer(
            entry.price_cents,
            entry.rental_period_days,
        )?;
        if price.is_some() {
            let conn = state.store.conn.lock().await;
            crate::client_market_trade::require_payment_profile_for_offer(&conn, &provider_id)?;
        }
        if let Some(existing_provider) = state
            .store
            .client_market_endpoint_provider(&ip.to_string(), port)
            .await?
        {
            if existing_provider == provider_id {
                return Ok(None);
            }
            return Err(AppError::Conflict(
                "Host endpoint is registered by another Provider".into(),
            ));
        }
        if !state
            .client_market_job_secrets
            .lock()
            .await
            .allow_host_registration(&provider_id, ip, source_ip)
        {
            return Err(AppError::TooManyRequests(
                "Host verification rate limit exceeded".into(),
            ));
        }
        let known_hosts = known_hosts_path(&state.config);
        let (hostname, fingerprint) =
            ssh_verify_host(&state, &ip.to_string(), port, &known_hosts).await?;
        if let Some(expected) = entry
            .expected_fingerprint
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if fingerprint.as_deref() != Some(expected) {
                return Err(AppError::Conflict(
                    "SSH Host fingerprint does not match the exported value".into(),
                ));
            }
        }
        let intel = crate::ip_iq::lookup_host_ip_intel(&state.config.ip_intel_endpoints, &ip.to_string()).await?;
        let intel_json = serde_json::to_string(&intel).map_err(|error| {
            AppError::Internal(format!("serialize imported Host IP data failed: {error}"))
        })?;
        let host = state
            .store
            .client_market_insert_host_for_provider(
                &provider_id,
                &owner_email,
                &ip.to_string(),
                port,
                Some(&intel.country_code),
                hostname.as_deref(),
                fingerprint.as_deref(),
                entry.note.as_deref(),
                Some(&intel_json),
                price,
                period,
            )
            .await?;
        Ok(Some(host.id))
    }
    .await;
    match result {
        Ok(Some(host_id)) => HostImportItemResult {
            ip: ip_label,
            port,
            status: "imported".into(),
            host_id: Some(host_id),
            error: None,
        },
        Ok(None) => HostImportItemResult {
            ip: ip_label,
            port,
            status: "skipped".into(),
            host_id: None,
            error: None,
        },
        Err(error) => HostImportItemResult {
            ip: ip_label,
            port,
            status: "failed".into(),
            host_id: None,
            error: Some(error.to_string()),
        },
    }
}

async fn reverify_host(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<RouterSshHostView>, AppError> {
    let session = require_session(&state, &headers).await?;
    let host = state
        .store
        .client_market_get_host_for_operator(&id, &session)
        .await?;
    if !matches!(
        host.status.as_str(),
        HOST_STATUS_UNREACHABLE | HOST_STATUS_DISABLED | HOST_STATUS_IDLE | HOST_STATUS_ABNORMAL
    ) {
        return Err(AppError::Conflict(
            "host cannot be reverified in its current state".into(),
        ));
    }
    // Unreachable / abnormal hosts often still carry an installation_id after a
    // failed cleanup. Host owners must be able to recover without admin help.
    if host.installation_id.is_some()
        && !matches!(
            host.status.as_str(),
            HOST_STATUS_UNREACHABLE | HOST_STATUS_ABNORMAL
        )
    {
        return Err(AppError::Conflict(
            "host still has an installation; retry client cleanup instead".into(),
        ));
    }
    let known_hosts = known_hosts_path(&state.config);
    let wipe_policy = if host.ssh_host_key_fingerprint.is_some() {
        SshHostKeyPolicy::RequireKnown
    } else {
        SshHostKeyPolicy::AcceptNew
    };
    // Wipe remnants first — reverify is a recovery path, not a "must already be clean" gate.
    ssh_wipe_cc_switch_server(&state, &host.ip, host.port, &known_hosts, wipe_policy).await?;
    let (hostname, fingerprint) =
        ssh_probe_host_identity(&state, &host.ip, host.port, &known_hosts).await?;
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
    let session = require_session(&state, &headers).await?;
    state
        .store
        .client_market_delete_host(&id, &session)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateClientResponse {
    job_id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CleanupClientRequest {
    reason: Option<String>,
    block_client_for_provider: Option<bool>,
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
    let session = require_session(&state, &headers).await?;
    let mut job = state
        .store
        .client_market_get_job_for_viewer(&id, &session)
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
    input: Option<Json<CleanupClientRequest>>,
) -> Result<Json<CreateClientResponse>, AppError> {
    cleanup_client_with_reason(state, headers, installation_id, input, None).await
}

async fn release_client(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(installation_id): AxumPath<String>,
) -> Result<Json<CreateClientResponse>, AppError> {
    cleanup_client_with_reason(
        state,
        headers,
        installation_id,
        None,
        Some("client_release"),
    )
    .await
}

async fn provider_cleanup_client(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(installation_id): AxumPath<String>,
    input: Option<Json<CleanupClientRequest>>,
) -> Result<Json<CreateClientResponse>, AppError> {
    cleanup_client_with_reason(
        state,
        headers,
        installation_id,
        input,
        Some("provider_release"),
    )
    .await
}

async fn cleanup_client_with_reason(
    state: ServerState,
    headers: HeaderMap,
    installation_id: String,
    input: Option<Json<CleanupClientRequest>>,
    default_reason: Option<&str>,
) -> Result<Json<CreateClientResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    let viewer = session.email.clone();
    let input = input.map(|Json(value)| value).unwrap_or_default();
    let reason = input
        .reason
        .as_deref()
        .unwrap_or(default_reason.unwrap_or("client_release"));
    if !matches!(
        reason,
        "client_release"
            | "provider_release"
            | "payment_not_received"
            | "host_maintenance"
            | "service_terminated"
            | "other"
    ) {
        return Err(AppError::BadRequest(
            "unsupported Client cleanup reason".into(),
        ));
    }
    let subdomain = state
        .store
        .client_market_subdomain_for_installation(&installation_id)
        .await?;
    // Session admins are not elevated: only the Host owner or Client owner may cleanup.
    let job_id = state
        .store
        .client_market_begin_cleanup_job_with_context(
            &installation_id,
            Some(&session.user_id),
            &viewer,
            false,
            reason,
            input.block_client_for_provider,
        )
        .await?;
    if let Some(subdomain) = subdomain.as_deref() {
        state.proxy.remove_route(subdomain).await;
    }
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
        secrets.redeem_token(&token_hash, source_ip)?
    };
    state
        .store
        .client_market_validate_token_redemption(
            &secret.job_id,
            &token_hash,
            &normalize_ip_for_compare(source_ip).to_string(),
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

pub(crate) async fn run_create_job(state: ServerState, job_id: String) -> Result<(), AppError> {
    let result = run_create_job_inner(&state, &job_id).await;
    if let Err(ref error) = result {
        handle_create_job_failure(&state, &job_id, error).await;
    }
    if let Err(error) = state.store.client_market_sync_batch_for_job(&job_id).await {
        warn!(job_id = %job_id, error = %error, "failed to synchronize Client Market batch state");
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

pub async fn reconcile_interrupted_host_import_jobs(state: ServerState) -> Result<(), AppError> {
    let job_ids = state
        .store
        .client_market_interrupted_host_import_jobs()
        .await?;
    if job_ids.is_empty() {
        return Ok(());
    }
    info!(
        count = job_ids.len(),
        "reconciling interrupted Client Market Host imports"
    );
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(2));
    for job_id in job_ids {
        let runner_state = state.clone();
        let semaphore = semaphore.clone();
        tokio::spawn(async move {
            let Ok(_permit) = semaphore.acquire_owned().await else {
                return;
            };
            if let Err(error) = process_host_import_job(&runner_state, &job_id).await {
                warn!(job_id = %job_id, error = %error, "interrupted Host import recovery failed");
            }
        });
    }
    Ok(())
}

/// Periodic repair for stuck draining / remotely-clean unreachable hosts.
pub async fn reconcile_stale_market_hosts(state: ServerState) -> Result<(), AppError> {
    let draining = state
        .store
        .client_market_list_hosts_by_status(HOST_STATUS_DRAINING)
        .await?;
    let now = Utc::now();
    for host in draining {
        let updated = chrono::DateTime::parse_from_rfc3339(&host.updated_at)
            .ok()
            .map(|ts| ts.with_timezone(&Utc));
        let Some(updated) = updated else {
            continue;
        };
        if now.signed_duration_since(updated)
            < chrono::Duration::from_std(STALE_DRAINING_AFTER)
                .unwrap_or(chrono::Duration::minutes(10))
        {
            continue;
        }
        let has_active = state
            .store
            .client_market_host_has_active_job(&host.id)
            .await?;
        if has_active {
            continue;
        }
        let Some(installation_id) = host.installation_id.clone() else {
            warn!(
                host_id = %host.id,
                "stale draining host has no installation; marking unreachable"
            );
            match state
                .store
                .client_market_force_host_status(
                    &host.id,
                    HOST_STATUS_DRAINING,
                    HOST_STATUS_UNREACHABLE,
                    "stale_draining_without_installation",
                )
                .await
            {
                Ok(false) => info!(
                    host_id = %host.id,
                    "stale draining host changed concurrently; leaving it to the winning job"
                ),
                Ok(true) => {}
                Err(err) => warn!(
                    host_id = %host.id,
                    error = %err,
                    "failed to mark stale draining host unreachable"
                ),
            }
            continue;
        };
        info!(
            host_id = %host.id,
            installation_id = %installation_id,
            "spawning cleanup for stale draining host"
        );
        match state
            .store
            .client_market_begin_system_cleanup_job(&installation_id, "stale_draining_recovery")
            .await
        {
            Ok(job_id) => {
                let runner_state = state.clone();
                tokio::spawn(async move {
                    if let Err(err) = run_cleanup_job(runner_state, job_id.clone()).await {
                        error!(job_id = %job_id, error = %err, "stale draining cleanup failed");
                    }
                });
            }
            Err(err) => {
                warn!(
                    host_id = %host.id,
                    error = %err,
                    "failed to begin cleanup for stale draining host"
                );
            }
        }
    }

    let unreachable = state
        .store
        .client_market_list_hosts_by_status(HOST_STATUS_UNREACHABLE)
        .await?;
    for host in unreachable {
        let Some(installation_id) = host.installation_id.clone() else {
            continue;
        };
        let updated = chrono::DateTime::parse_from_rfc3339(&host.updated_at)
            .ok()
            .map(|ts| ts.with_timezone(&Utc));
        let Some(updated) = updated else {
            continue;
        };
        if now.signed_duration_since(updated)
            < chrono::Duration::from_std(UNREACHABLE_AUTO_HEAL_AFTER)
                .unwrap_or(chrono::Duration::minutes(5))
        {
            continue;
        }
        match ssh_remote_is_market_clean(&state, &host).await {
            Ok(true) => {
                info!(
                    host_id = %host.id,
                    installation_id = %installation_id,
                    "auto-healing unreachable host that is remotely clean"
                );
                if let Some(subdomain) = host.client_subdomain.as_deref() {
                    state.proxy.remove_route(subdomain).await;
                } else if let Ok(Some(subdomain)) = state
                    .store
                    .client_market_subdomain_for_installation(&installation_id)
                    .await
                {
                    state.proxy.remove_route(&subdomain).await;
                }
                if let Err(err) = state
                    .store
                    .purge_installation_for_client_market(&installation_id)
                    .await
                {
                    warn!(
                        host_id = %host.id,
                        error = %err,
                        "auto-heal purge failed"
                    );
                    continue;
                }
                if let Err(err) = state
                    .store
                    .client_market_complete_host_reverify(
                        &host.id,
                        host.hostname.as_deref(),
                        host.ssh_host_key_fingerprint.as_deref(),
                    )
                    .await
                {
                    warn!(
                        host_id = %host.id,
                        error = %err,
                        "auto-heal reverify finalize failed"
                    );
                }
            }
            Ok(false) => {}
            Err(err) => {
                warn!(
                    host_id = %host.id,
                    error = %err,
                    "auto-heal remote probe failed"
                );
            }
        }
    }

    reconcile_stranded_locked_hosts(&state, now).await?;
    reconcile_stranded_reserved_hosts(&state, now).await?;
    Ok(())
}

/// `locked` is entered when a Host is claimed for provisioning and is normally left
/// by the create job. If the process died between the claim and the job starting —
/// or the worker panicked — nothing else moves the Host, and it silently leaves the
/// supply pool forever. Reclaim it once no job can possibly still own it.
async fn reconcile_stranded_locked_hosts(
    state: &ServerState,
    now: chrono::DateTime<Utc>,
) -> Result<(), AppError> {
    let locked = state
        .store
        .client_market_list_hosts_by_status(HOST_STATUS_LOCKED)
        .await?;
    for host in locked {
        let Some(updated) = chrono::DateTime::parse_from_rfc3339(&host.updated_at)
            .ok()
            .map(|ts| ts.with_timezone(&Utc))
        else {
            continue;
        };
        if now.signed_duration_since(updated)
            < chrono::Duration::from_std(STALE_LOCKED_AFTER)
                .unwrap_or(chrono::Duration::minutes(25))
        {
            continue;
        }
        if state
            .store
            .client_market_host_has_active_job(&host.id)
            .await?
        {
            continue;
        }
        // Route to `unreachable` rather than `idle`: the Host may carry a partial
        // install from the job that died, so it must pass reverify before it can be
        // handed to another renter.
        match state
            .store
            .client_market_force_host_status(
                &host.id,
                HOST_STATUS_LOCKED,
                HOST_STATUS_UNREACHABLE,
                "stale_locked_without_active_job",
            )
            .await
        {
            Ok(true) => warn!(
                host_id = %host.id,
                "reclaimed host stranded in locked state"
            ),
            Ok(false) => {}
            Err(err) => warn!(
                host_id = %host.id,
                error = %err,
                "failed to reclaim stranded locked host"
            ),
        }
    }
    Ok(())
}

/// `reserved` is held by a live allocation quote. Quotes are expired opportunistically
/// (only when some quote endpoint is called), so a restart with quotes outstanding can
/// strand Hosts indefinitely if no further quote traffic arrives.
async fn reconcile_stranded_reserved_hosts(
    state: &ServerState,
    now: chrono::DateTime<Utc>,
) -> Result<(), AppError> {
    let reserved = state
        .store
        .client_market_list_hosts_by_status(crate::client_market_trade::HOST_STATUS_RESERVED)
        .await?;
    for host in reserved {
        let Some(updated) = chrono::DateTime::parse_from_rfc3339(&host.updated_at)
            .ok()
            .map(|ts| ts.with_timezone(&Utc))
        else {
            continue;
        };
        if now.signed_duration_since(updated)
            < chrono::Duration::from_std(STALE_RESERVED_AFTER)
                .unwrap_or(chrono::Duration::minutes(10))
        {
            continue;
        }
        if state
            .store
            .client_market_host_has_live_quote(&host.id)
            .await?
        {
            continue;
        }
        // No live quote can claim it, and a reserved Host was never touched remotely,
        // so returning it straight to the pool is safe.
        match state
            .store
            .client_market_force_host_status(
                &host.id,
                crate::client_market_trade::HOST_STATUS_RESERVED,
                HOST_STATUS_IDLE,
                "",
            )
            .await
        {
            Ok(true) => warn!(
                host_id = %host.id,
                "returned host stranded in reserved state to the pool"
            ),
            Ok(false) => {}
            Err(err) => warn!(
                host_id = %host.id,
                error = %err,
                "failed to return stranded reserved host"
            ),
        }
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
                    &router_public_url(&state.config),
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
    let host = if let Some(host_id) = job.host_id.as_deref() {
        let host = state
            .store
            .client_market_get_host(host_id)
            .await?
            .ok_or_else(|| AppError::NotFound("quoted Host no longer exists".into()))?;
        if host.status != HOST_STATUS_LOCKED {
            return Err(AppError::Conflict(
                "quoted Host is no longer locked for this job".into(),
            ));
        }
        let known_hosts = known_hosts_path(&state.config);
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
            Ok(false) => host,
            Ok(true) => {
                state
                    .store
                    .client_market_mark_host_abnormal_and_detach_job(
                        job_id,
                        &host.id,
                        "cc-switch-server started after the allocation quote",
                    )
                    .await?;
                return Err(AppError::Conflict(
                    "quoted Host started cc-switch-server after reservation; no fallback Host was selected".into(),
                ));
            }
            Err(error) => {
                state
                    .store
                    .client_market_mark_host_unreachable_and_detach_job(
                        job_id,
                        &host.id,
                        &format!("quoted Host process check failed: {error}"),
                    )
                    .await?;
                return Err(error);
            }
        }
    } else {
        claim_idle_host_without_running_server(state, job_id, &job).await?
    };
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
    let owner_email = job.client_owner_email.clone().unwrap_or_default();
    let subdomain = job.subdomain.clone().unwrap_or_default();
    if owner_email.is_empty() || subdomain.is_empty() {
        return Err(AppError::Internal(
            "create job is missing owner email or subdomain".into(),
        ));
    }
    let router_url = router_public_url(&state.config);
    let install_cmd = format!(
        "{deps}\
         set -eu; \
         ensure_client_market_deps; \
         mkdir -p /usr/local/bin /tmp; \
         script=$(mktemp); trap 'rm -f \"$script\"' EXIT; \
         curl --fail --silent --show-error --location --max-time 120 {} -o \"$script\"; \
         bash \"$script\" {} {} --password-stdin {} disableWebTerminal",
        shell_quote(&format!("{router_url}/install-client.sh")),
        shell_quote(&router_url),
        shell_quote(&owner_email),
        shell_quote(&subdomain),
        deps = REMOTE_ENSURE_CLIENT_MARKET_DEPS,
    );
    let password_stdin = format!("{password}\n").into_bytes();
    let install_result = ssh_run_remote_with_input(
        &state.provision_ssh_key_path,
        &known_hosts_path(&state.config),
        &host.ip,
        host.port,
        &install_cmd,
        Some(password_stdin),
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
            &router_public_url(&state.config),
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
    let _ = state
        .store
        .client_market_append_job_log(job_id, &format!("provisioning error: {error}\n"))
        .await;
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

pub(crate) async fn run_cleanup_job(state: ServerState, job_id: String) -> Result<(), AppError> {
    let result = run_cleanup_job_inner(&state, &job_id, false).await;
    if let Err(ref error) = result {
        handle_cleanup_job_failure(&state, &job_id, error).await;
    }
    result
}

fn is_cleanup_phase(phase: &str) -> bool {
    matches!(
        phase,
        JOB_PHASE_CLEANUP
            | JOB_PHASE_CLEANUP_STOP
            | JOB_PHASE_CLEANUP_WIPE
            | JOB_PHASE_CLEANUP_PURGE
    )
}

fn classify_cleanup_failure(error: &AppError) -> &'static str {
    let detail = error.to_string();
    let lower = detail.to_ascii_lowercase();
    if lower.contains("fingerprint") {
        CLEANUP_FAILURE_FINGERPRINT
    } else if lower.contains("binding") || lower.contains("installation mismatch") {
        CLEANUP_FAILURE_BINDING
    } else if lower.contains("exceeded its execution timeout") || lower.contains("timeout") {
        CLEANUP_FAILURE_SSH_TIMEOUT
    } else if lower.contains("failed to stop")
        || lower.contains("respawned")
        || lower.contains("process is already running")
    {
        CLEANUP_FAILURE_STOP
    } else if lower.contains("failed to remove") || lower.contains("installation files") {
        CLEANUP_FAILURE_WIPE
    } else if lower.contains("purge") {
        CLEANUP_FAILURE_PURGE
    } else {
        CLEANUP_FAILURE_GENERIC
    }
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
        || !is_cleanup_phase(&job.phase)
    {
        return Err(AppError::Conflict(
            "cleanup job is not runnable in its current state".into(),
        ));
    }
    let host_id = job
        .host_id
        .clone()
        .ok_or_else(|| AppError::Internal("cleanup job missing host".into()))?;
    let installation_id = job
        .installation_id
        .clone()
        .ok_or_else(|| AppError::Internal("cleanup job missing installation".into()))?;
    let host = state
        .store
        .client_market_get_host(&host_id)
        .await?
        .ok_or_else(|| AppError::NotFound("host not found".into()))?;
    if host.status != HOST_STATUS_DRAINING {
        return Err(AppError::Conflict("cleanup host is not draining".into()));
    }
    // Safety: refuse to wipe if host no longer points at this installation.
    if host.installation_id.as_deref() != Some(installation_id.as_str()) {
        return Err(AppError::Conflict(
            "cleanup host installation binding mismatch".into(),
        ));
    }
    if let (Some(job_sub), Some(host_sub)) =
        (job.subdomain.as_deref(), host.client_subdomain.as_deref())
    {
        if !job_sub.eq_ignore_ascii_case(host_sub) {
            return Err(AppError::Conflict(
                "cleanup host subdomain binding mismatch".into(),
            ));
        }
    }

    // Drop the public route before touching the remote box. Previously this happened
    // only in the purge phase, so a cleanup that failed during stop or wipe left the
    // tunnel serving traffic while billing had already moved the subscription to
    // releasing/release_failed — the renter kept working on a Host they were no longer
    // being charged for. Removal is idempotent, so the purge phase below can repeat it.
    if let Some(subdomain) = job.subdomain.as_deref() {
        state.proxy.remove_route(subdomain).await;
        state
            .store
            .client_market_append_job_log(
                job_id,
                &format!("proxy route removed before teardown: {subdomain}\n"),
            )
            .await?;
    }

    let mut phase = job.phase.clone();
    if matches!(
        phase.as_str(),
        JOB_PHASE_CLEANUP | JOB_PHASE_CLEANUP_STOP | JOB_PHASE_CLEANUP_WIPE
    ) {
        if phase == JOB_PHASE_CLEANUP {
            state
                .store
                .client_market_set_job_phase(job_id, JOB_PHASE_CLEANUP_STOP)
                .await?;
            phase = JOB_PHASE_CLEANUP_STOP.to_string();
        }
        if phase == JOB_PHASE_CLEANUP_STOP {
            state
                .store
                .client_market_append_job_log(job_id, "phase: stop cc-switch-server\n")
                .await?;
            ssh_stop_cc_switch_server(state, &host).await?;
            state
                .store
                .client_market_append_job_log(
                    job_id,
                    "remote process stopped (or already stopped)\n",
                )
                .await?;
            state
                .store
                .client_market_set_job_phase(job_id, JOB_PHASE_CLEANUP_WIPE)
                .await?;
            phase = JOB_PHASE_CLEANUP_WIPE.to_string();
        }
        if phase == JOB_PHASE_CLEANUP_WIPE {
            state
                .store
                .client_market_append_job_log(job_id, "phase: wipe install files\n")
                .await?;
            ssh_wipe_cc_switch_files(state, &host).await?;
            state
                .store
                .client_market_append_job_log(
                    job_id,
                    "remote client files removed (binary, data, backups)\n",
                )
                .await?;
            state
                .store
                .client_market_set_job_phase(job_id, JOB_PHASE_CLEANUP_PURGE)
                .await?;
            phase = JOB_PHASE_CLEANUP_PURGE.to_string();
        }
    }

    if phase != JOB_PHASE_CLEANUP_PURGE {
        state
            .store
            .client_market_set_job_phase(job_id, JOB_PHASE_CLEANUP_PURGE)
            .await?;
    }
    state
        .store
        .client_market_append_job_log(job_id, "phase: purge router installation\n")
        .await?;
    if let Some(subdomain) = job.subdomain.as_deref() {
        state.proxy.remove_route(subdomain).await;
        state
            .store
            .client_market_append_job_log(job_id, &format!("proxy route removed: {subdomain}\n"))
            .await?;
    }
    let mut last_purge_error = None;
    for attempt in 1..=CLEANUP_PURGE_ATTEMPTS {
        match state
            .store
            .purge_installation_for_client_market(&installation_id)
            .await
        {
            Ok(()) => {
                last_purge_error = None;
                break;
            }
            Err(err) => {
                warn!(
                    job_id = %job_id,
                    attempt,
                    error = %err,
                    "cleanup purge attempt failed"
                );
                state
                    .store
                    .client_market_append_job_log(
                        job_id,
                        &format!(
                            "purge attempt {attempt}/{CLEANUP_PURGE_ATTEMPTS} failed: {err}\n"
                        ),
                    )
                    .await?;
                last_purge_error = Some(err);
                if attempt < CLEANUP_PURGE_ATTEMPTS {
                    tokio::time::sleep(CLEANUP_PURGE_RETRY_BASE * attempt).await;
                }
            }
        }
    }
    if let Some(err) = last_purge_error {
        return Err(AppError::Internal(format!(
            "purge installation failed after {CLEANUP_PURGE_ATTEMPTS} attempts: {err}"
        )));
    }
    state
        .store
        .client_market_append_job_log(
            job_id,
            "installation purged from router; marking host idle\n",
        )
        .await?;
    state
        .store
        .client_market_finish_cleanup_job(job_id, &host_id)
        .await?;
    Ok(())
}

async fn handle_cleanup_job_failure(state: &ServerState, job_id: &str, error: &AppError) {
    warn!(job_id = %job_id, error = %error, "client market cleanup failed");
    let detail = error.to_string();
    let failure_code = classify_cleanup_failure(error);
    let _ = state
        .store
        .client_market_append_job_log(
            job_id,
            &format!("cleanup error [{failure_code}]: {detail}\n"),
        )
        .await;
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
    // Remote already wiped but DB purge failed: keep a precise code so UI can guide reverify.
    let last_error = if failure_code == CLEANUP_FAILURE_PURGE {
        format!("{failure_code}: remote wipe ok; {detail}")
            .chars()
            .take(500)
            .collect::<String>()
    } else {
        format!("{failure_code}: {detail}")
            .chars()
            .take(500)
            .collect::<String>()
    };
    let guidance = match failure_code {
        CLEANUP_FAILURE_PURGE => {
            "cleanup failed after remote wipe; use Retry cleanup or Re-verify to finish\n"
        }
        CLEANUP_FAILURE_SSH_TIMEOUT | CLEANUP_FAILURE_STOP | CLEANUP_FAILURE_WIPE => {
            "cleanup failed on the host; retry cleanup, or re-verify after manual cleanup\n"
        }
        CLEANUP_FAILURE_FINGERPRINT | CLEANUP_FAILURE_BINDING => {
            "cleanup blocked by host safety checks; operator intervention is required\n"
        }
        _ => "cleanup failed; host remains unavailable until retry or re-verify\n",
    };
    if let Err(finalize_error) = state
        .store
        .client_market_fail_cleanup_job(job_id, host_id, &last_error, guidance)
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

async fn ssh_test_login(
    state: &ServerState,
    ip: &str,
    port: u16,
    known_hosts: &Path,
) -> Result<(), AppError> {
    let _known_hosts_guard = PROVISION_KNOWN_HOSTS_LOCK.lock().await;
    let output = ssh_run_remote_with_input(
        &state.provision_ssh_key_path,
        known_hosts,
        ip,
        port,
        "set -eu; printf 'ok\\n'",
        None,
        SSH_VERIFY_TIMEOUT,
        SshHostKeyPolicy::AcceptNew,
    )
    .await?;
    if !output.lines().any(|line| line.trim() == "ok") {
        return Err(AppError::BadRequest(
            "ssh login test succeeded but returned an unexpected response".into(),
        ));
    }
    Ok(())
}

fn validate_root_password(password: &str) -> Result<(), AppError> {
    if password.is_empty() {
        return Err(AppError::BadRequest("root password is required".into()));
    }
    if password.len() > MAX_PASSWORD_BYTES {
        return Err(AppError::BadRequest(
            "root password cannot exceed 1024 bytes".into(),
        ));
    }
    if password.chars().any(char::is_control) {
        return Err(AppError::BadRequest(
            "root password must not contain control characters".into(),
        ));
    }
    Ok(())
}

fn install_provision_key_remote_command(authorized_keys_line: &str) -> String {
    format!(
        "set -eu; \
         mkdir -p \"$HOME/.ssh\"; \
         chmod 700 \"$HOME/.ssh\"; \
         touch \"$HOME/.ssh/authorized_keys\"; \
         chmod 600 \"$HOME/.ssh/authorized_keys\"; \
         line={line}; \
         if ! grep -qxF \"$line\" \"$HOME/.ssh/authorized_keys\" 2>/dev/null; then \
           printf '%s\\n' \"$line\" >> \"$HOME/.ssh/authorized_keys\"; \
         fi; \
         printf 'ok\\n'",
        line = shell_quote(authorized_keys_line),
    )
}

async fn ssh_test_login_with_password(
    ip: &str,
    port: u16,
    password: &str,
    known_hosts: &Path,
) -> Result<(), AppError> {
    let _known_hosts_guard = PROVISION_KNOWN_HOSTS_LOCK.lock().await;
    let output = ssh_run_remote_with_password(
        password,
        known_hosts,
        ip,
        port,
        "set -eu; printf 'ok\\n'",
        SSH_VERIFY_TIMEOUT,
        SshHostKeyPolicy::AcceptNew,
    )
    .await?;
    if !output.lines().any(|line| line.trim() == "ok") {
        return Err(AppError::BadRequest(
            "ssh password login test succeeded but returned an unexpected response".into(),
        ));
    }
    Ok(())
}

async fn ssh_install_provision_key_with_password(
    ip: &str,
    port: u16,
    password: &str,
    authorized_keys_line: &str,
    known_hosts: &Path,
) -> Result<(), AppError> {
    let _known_hosts_guard = PROVISION_KNOWN_HOSTS_LOCK.lock().await;
    let command = install_provision_key_remote_command(authorized_keys_line);
    let output = ssh_run_remote_with_password(
        password,
        known_hosts,
        ip,
        port,
        &command,
        SSH_VERIFY_TIMEOUT,
        SshHostKeyPolicy::AcceptNew,
    )
    .await?;
    if !output.lines().any(|line| line.trim() == "ok") {
        return Err(AppError::BadRequest(
            "provision SSH key install succeeded but returned an unexpected response".into(),
        ));
    }
    Ok(())
}

struct PasswordAskpassMaterial {
    dir: PathBuf,
}

impl PasswordAskpassMaterial {
    fn create(password: &str) -> Result<(Self, PathBuf), AppError> {
        let dir =
            std::env::temp_dir().join(format!("cc-switch-router-ssh-askpass-{}", Uuid::new_v4()));
        fs::create_dir(&dir)
            .map_err(|e| AppError::Internal(format!("create ssh askpass directory failed: {e}")))?;
        let material = Self { dir: dir.clone() };
        let password_path = dir.join("password");
        let askpass_path = dir.join("askpass.sh");
        fs::write(&password_path, password.as_bytes()).map_err(|e| {
            AppError::Internal(format!("write ssh askpass password file failed: {e}"))
        })?;
        fs::set_permissions(&password_path, fs::Permissions::from_mode(0o600)).map_err(|e| {
            AppError::Internal(format!("chmod ssh askpass password file failed: {e}"))
        })?;
        let script = format!(
            "#!/bin/sh\nexec cat {}\n",
            shell_quote(&password_path.display().to_string())
        );
        fs::write(&askpass_path, script)
            .map_err(|e| AppError::Internal(format!("write ssh askpass script failed: {e}")))?;
        fs::set_permissions(&askpass_path, fs::Permissions::from_mode(0o700))
            .map_err(|e| AppError::Internal(format!("chmod ssh askpass script failed: {e}")))?;
        Ok((material, askpass_path))
    }
}

impl Drop for PasswordAskpassMaterial {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

async fn ssh_run_remote_with_password(
    password: &str,
    known_hosts: &Path,
    ip: &str,
    port: u16,
    remote_command: &str,
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
    let (_askpass_material, askpass_path) = PasswordAskpassMaterial::create(password)?;
    let wrapped_command = format!(
        "export PATH=\"/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin${{PATH:+:$PATH}}\"; \
         if command -v bash >/dev/null 2>&1; then \
           exec bash --noprofile --norc -c {}; \
         else \
           exec sh -c {}; \
         fi",
        shell_quote(remote_command),
        shell_quote(remote_command)
    );
    let target = format!("root@{ip}");
    let mut command = Command::new("ssh");
    command
        .env("SSH_ASKPASS", &askpass_path)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("DISPLAY", "cc-switch-router:0")
        .env_remove("SSH_AUTH_SOCK")
        .arg("-F")
        .arg("/dev/null")
        .arg("-T")
        .arg("-p")
        .arg(port.to_string())
        .arg("-o")
        .arg("BatchMode=no")
        .arg("-o")
        .arg("NumberOfPasswordPrompts=1")
        .arg("-o")
        .arg("PubkeyAuthentication=no")
        .arg("-o")
        .arg("PreferredAuthentications=password")
        .arg("-o")
        .arg("KbdInteractiveAuthentication=no")
        .arg("-o")
        .arg("ChallengeResponseAuthentication=no")
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
        .arg(&wrapped_command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|e| AppError::ServiceUnavailable(format!("start ssh command failed: {e}")))?;
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
        let detail = sanitize_ssh_password_error(&format!("{stdout}{stderr}"));
        return Err(AppError::BadRequest(format!(
            "ssh password auth failed ({}): {detail}",
            status
        )));
    }
    Ok(format!("{stdout}{stderr}"))
}

fn sanitize_ssh_password_error(detail: &str) -> String {
    let mut output = String::new();
    for line in detail.lines().take(40) {
        if job_log_line_looks_sensitive(line) {
            output.push_str("[sensitive output redacted]\n");
            continue;
        }
        for character in line.chars().take(500) {
            if !character.is_control() || character == '\t' {
                output.push(character);
            }
        }
        output.push('\n');
        if output.len() >= 4 * 1024 {
            break;
        }
    }
    if output.trim().is_empty() {
        "authentication or remote command failed".into()
    } else {
        output
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
    if ssh_host_has_running_cc_switch_server(
        state,
        ip,
        port,
        known_hosts,
        SshHostKeyPolicy::AcceptNew,
    )
    .await?
    {
        return Err(AppError::Conflict(
            "host is already running cc-switch-server; stop the process before adding it to Client Market"
                .into(),
        ));
    }
    // Registering a host must not silently wipe an existing install.
    let output = ssh_run_remote_with_input(
        &state.provision_ssh_key_path,
        known_hosts,
        ip,
        port,
        &format!(
            "{helpers}\
             set -eu; \
             home=\"$(cc_switch_server_home)\"; \
             if cc_switch_server_has_install_files; then \
             echo \"host already contains a cc-switch-server installation under $home\" >&2; exit 42; fi; \
             uname -n",
            helpers = REMOTE_CC_SWITCH_SERVER_HELPERS,
        ),
        None,
        SSH_VERIFY_TIMEOUT,
        SshHostKeyPolicy::AcceptNew,
    )
    .await?;
    let hostname = parse_remote_hostname(&output)?;
    // Ensure package deps for later market installs, then pin host key.
    let _ = ssh_run_remote_with_input(
        &state.provision_ssh_key_path,
        known_hosts,
        ip,
        port,
        &format!(
            "{deps}\
             set -eu; \
             ensure_client_market_deps",
            deps = REMOTE_ENSURE_CLIENT_MARKET_DEPS,
        ),
        None,
        SSH_VERIFY_TIMEOUT,
        SshHostKeyPolicy::AcceptNew,
    )
    .await?;
    let fingerprint = ssh_fetch_host_fingerprint(ip, port, known_hosts).await?;
    Ok((Some(hostname), Some(fingerprint)))
}

async fn ssh_probe_host_identity(
    state: &ServerState,
    ip: &str,
    port: u16,
    known_hosts: &Path,
) -> Result<(Option<String>, Option<String>), AppError> {
    let _known_hosts_guard = PROVISION_KNOWN_HOSTS_LOCK.lock().await;
    let output = ssh_run_remote_with_input(
        &state.provision_ssh_key_path,
        known_hosts,
        ip,
        port,
        &format!(
            "{deps}\
             set -eu; \
             ensure_client_market_deps; \
             uname -n",
            deps = REMOTE_ENSURE_CLIENT_MARKET_DEPS,
        ),
        None,
        SSH_VERIFY_TIMEOUT,
        SshHostKeyPolicy::AcceptNew,
    )
    .await?;
    let hostname = parse_remote_hostname(&output)?;
    let fingerprint = ssh_fetch_host_fingerprint(ip, port, known_hosts).await?;
    Ok((Some(hostname), Some(fingerprint)))
}

fn parse_remote_hostname(output: &str) -> Result<String, AppError> {
    output
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
        .map(str::to_string)
        .ok_or_else(|| AppError::BadRequest("ssh hostname response was invalid".into()))
}

/// Stop cc-switch-server and remove install/data/backup files. Idempotent: succeeds when already clean.
async fn ssh_wipe_cc_switch_server(
    state: &ServerState,
    ip: &str,
    port: u16,
    known_hosts: &Path,
    host_key_policy: SshHostKeyPolicy,
) -> Result<(), AppError> {
    let command = format!(
        "{helpers}\
         set -eu; \
         if ! cc_switch_server_stop; then \
           echo 'failed to stop cc-switch-server' >&2; exit 43; \
         fi; \
         cc_switch_server_wipe_files; \
         if cc_switch_server_is_running; then \
           echo 'cc-switch-server respawned during wipe' >&2; exit 43; \
         fi; \
         if cc_switch_server_has_install_files; then \
           echo 'failed to remove cc-switch-server installation files' >&2; exit 44; \
         fi",
        helpers = REMOTE_CC_SWITCH_SERVER_HELPERS,
    );
    ssh_run_remote_with_input(
        &state.provision_ssh_key_path,
        known_hosts,
        ip,
        port,
        &command,
        None,
        SSH_CLEANUP_TIMEOUT,
        host_key_policy,
    )
    .await
    .map(|_| ())
}

async fn ssh_stop_cc_switch_server(
    state: &ServerState,
    host: &RouterSshHostRecord,
) -> Result<(), AppError> {
    let known_hosts = known_hosts_path(&state.config);
    require_pinned_host_fingerprint(host, &known_hosts).await?;
    let command = format!(
        "{helpers}\
         set -eu; \
         if ! cc_switch_server_stop; then \
           echo 'failed to stop cc-switch-server' >&2; exit 43; \
         fi",
        helpers = REMOTE_CC_SWITCH_SERVER_HELPERS,
    );
    ssh_run_remote_with_input(
        &state.provision_ssh_key_path,
        &known_hosts,
        &host.ip,
        host.port,
        &command,
        None,
        SSH_CLEANUP_TIMEOUT,
        SshHostKeyPolicy::RequireKnown,
    )
    .await
    .map(|_| ())
}

async fn ssh_wipe_cc_switch_files(
    state: &ServerState,
    host: &RouterSshHostRecord,
) -> Result<(), AppError> {
    let known_hosts = known_hosts_path(&state.config);
    require_pinned_host_fingerprint(host, &known_hosts).await?;
    let command = format!(
        "{helpers}\
         set -eu; \
         if ! cc_switch_server_stop; then \
           echo 'failed to stop cc-switch-server' >&2; exit 43; \
         fi; \
         cc_switch_server_wipe_files; \
         if cc_switch_server_has_install_files; then \
           echo 'failed to remove cc-switch-server installation files' >&2; exit 44; \
         fi",
        helpers = REMOTE_CC_SWITCH_SERVER_HELPERS,
    );
    ssh_run_remote_with_input(
        &state.provision_ssh_key_path,
        &known_hosts,
        &host.ip,
        host.port,
        &command,
        None,
        SSH_CLEANUP_TIMEOUT,
        SshHostKeyPolicy::RequireKnown,
    )
    .await
    .map(|_| ())
}

/// True when the remote host has no running process and no install/backup files.
async fn ssh_remote_is_market_clean(
    state: &ServerState,
    host: &RouterSshHostRecord,
) -> Result<bool, AppError> {
    // Auto-heal runs unattended and its success returns a Host to the allocation
    // pool. Trusting a fresh host key here would let whoever controls the IP during
    // the heal window present a clean machine and be adopted. A Host with no pinned
    // fingerprint must go through owner-driven reverify instead.
    let Some(_) = host.ssh_host_key_fingerprint.as_deref() else {
        return Err(AppError::Conflict(
            "host has no pinned SSH fingerprint; auto-heal requires owner reverify".into(),
        ));
    };
    let known_hosts = known_hosts_path(&state.config);
    let command = format!(
        "{helpers}\
         set -eu; \
         if cc_switch_server_is_running; then echo dirty; exit 0; fi; \
         if cc_switch_server_has_install_files; then echo dirty; exit 0; fi; \
         echo clean",
        helpers = REMOTE_CC_SWITCH_SERVER_HELPERS,
    );
    let output = ssh_run_remote_with_input(
        &state.provision_ssh_key_path,
        &known_hosts,
        &host.ip,
        host.port,
        &command,
        None,
        SSH_VERIFY_TIMEOUT,
        SshHostKeyPolicy::RequireKnown,
    )
    .await?;
    Ok(output.lines().any(|line| line.trim() == "clean"))
}

async fn ssh_cleanup_remote(
    state: &ServerState,
    host: &RouterSshHostRecord,
) -> Result<(), AppError> {
    let known_hosts = known_hosts_path(&state.config);
    require_pinned_host_fingerprint(host, &known_hosts).await?;
    ssh_wipe_cc_switch_server(
        state,
        &host.ip,
        host.port,
        &known_hosts,
        SshHostKeyPolicy::RequireKnown,
    )
    .await
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
            "{helpers}\
             set +e; \
             if cc_switch_server_is_running; then \
             echo 'cc-switch-server process is already running' >&2; exit {HOST_HAS_RUNNING_SERVER_EXIT}; \
             fi; exit 0",
            helpers = REMOTE_CC_SWITCH_SERVER_HELPERS,
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
                    .client_market_mark_host_abnormal_and_detach_job(job_id, &host.id, reason)
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
                    .client_market_mark_host_unreachable_and_detach_job(job_id, &host.id, &reason)
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
    // Non-interactive SSH often gets a stripped PATH. Prefer bash when present
    // (install-client.sh needs it); fall back to POSIX sh for Alpine before deps exist.
    let wrapped_command = format!(
        "export PATH=\"/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin${{PATH:+:$PATH}}\"; \
         if command -v bash >/dev/null 2>&1; then \
           exec bash --noprofile --norc -c {}; \
         else \
           exec sh -c {}; \
         fi",
        shell_quote(remote_command),
        shell_quote(remote_command)
    );
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
        .arg(&wrapped_command)
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
            || detail.contains("failed to stop cc-switch-server")
            || detail.contains("cc-switch-server respawned during wipe")
        {
            return Err(AppError::Conflict(
                "cc-switch-server process is already running".into(),
            ));
        }
        if code == Some(44)
            || detail.contains("failed to remove cc-switch-server installation files")
        {
            return Err(AppError::Conflict(
                "failed to remove cc-switch-server installation files".into(),
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

fn provision_token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn sanitize_job_log_chunk(chunk: &str) -> String {
    let mut output = String::new();
    for line in chunk.lines().take(200) {
        if job_log_line_looks_sensitive(line) {
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

fn job_log_line_looks_sensitive(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    // Keep prose like "provision token redemption failed" visible in job logs.
    // Only redact lines that look like they embed secret material.
    lower.contains("password=")
        || lower.contains("password:")
        || lower.contains("--password ")
        || lower.contains("token=")
        || lower.contains("authorization:")
        || lower.contains("bearer ")
}

fn normalize_ip_for_compare(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        other => other,
    }
}

fn parse_host_ip_intel(raw: Option<&str>) -> Option<crate::ip_iq::HostIpIntel> {
    raw.and_then(|value| serde_json::from_str(value).ok())
}

/// Non-owners still see market-useful ISP / risk / classification.
/// Full IP is public in the market list; geo coords and ownership stay redacted.
fn public_host_ip_intel(
    intel: crate::ip_iq::HostIpIntel,
    host_ip: &str,
) -> crate::ip_iq::HostIpIntel {
    crate::ip_iq::HostIpIntel {
        query: host_ip.to_string(),
        ip: Some(host_ip.to_string()),
        location: intel.location,
        score: None,
        level: intel.level,
        risk_score: None,
        risk_level: intel.risk_level,
        confidence: None,
        country_code: intel.country_code,
        country: intel.country,
        region: intel.region,
        city: intel.city,
        latitude: None,
        longitude: None,
        timezone: intel.timezone,
        asn: intel.asn,
        as_name: intel.as_name,
        isp: intel.isp,
        owner: None,
        network_type: intel.network_type,
        classification_type: intel.classification_type,
        proxy: intel.proxy,
        vpn: intel.vpn,
        hosting: intel.hosting,
        tor: intel.tor,
        source: intel.source,
    }
}

fn host_ip_intel_for_viewer(
    raw: Option<&str>,
    host_ip: &str,
    reveal: bool,
) -> Option<crate::ip_iq::HostIpIntel> {
    let intel = parse_host_ip_intel(raw)?;
    Some(if reveal {
        intel
    } else {
        public_host_ip_intel(intel, host_ip)
    })
}

fn host_to_view(host: RouterSshHostRecord, reveal: bool) -> RouterSshHostView {
    let ip_intel = host_ip_intel_for_viewer(host.ip_intel_json.as_deref(), &host.ip, reveal);
    RouterSshHostView {
        id: host.id,
        provider_id: host.provider_id,
        ip: Some(host.ip.clone()),
        port: reveal.then_some(host.port),
        host_owner_email: host.host_owner_email,
        price_cents: host.price_cents,
        rental_period_days: host.rental_period_days,
        offer_revision: host.offer_revision,
        payment_method_kinds: host.payment_method_kinds,
        country_code: host.country_code,
        hostname: host.hostname,
        ssh_host_key_fingerprint: reveal.then_some(host.ssh_host_key_fingerprint).flatten(),
        status: host.status,
        client_subdomain: host.client_subdomain,
        client_owner_email: reveal.then_some(host.client_owner_email).flatten(),
        installation_id: reveal.then_some(host.installation_id).flatten(),
        // Caller-specific; list_hosts sets this from the viewer session.
        can_web_terminal: false,
        is_host_owner: false,
        is_client_owner: false,
        last_verified_at: reveal.then_some(host.last_verified_at).flatten(),
        last_error: reveal.then_some(host.last_error).flatten(),
        note: reveal.then_some(host.note).flatten(),
        ip_intel,
        created_at: reveal.then_some(host.created_at),
        updated_at: reveal.then_some(host.updated_at),
    }
}

async fn require_session_email(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<String, AppError> {
    extract_optional_session(state, headers)
        .await?
        .map(|session| session.email)
        .ok_or_else(|| AppError::Unauthorized("authenticated owner session required".into()))
}

async fn require_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<crate::models::AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("authenticated owner session required".into()))
}

async fn extract_optional_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<Option<crate::models::AuthSession>, AppError> {
    crate::api::resolve_router_session(state, headers).await
}

#[derive(Debug, Clone)]
pub struct RouterSshHostRecord {
    pub id: String,
    pub provider_id: Option<String>,
    pub ip: String,
    pub port: u16,
    pub host_owner_email: String,
    pub price_cents: Option<i64>,
    pub rental_period_days: Option<i64>,
    pub offer_revision: i64,
    pub payment_method_kinds: Vec<String>,
    pub country_code: Option<String>,
    pub hostname: Option<String>,
    pub ssh_host_key_fingerprint: Option<String>,
    pub status: String,
    pub client_subdomain: Option<String>,
    pub client_owner_email: Option<String>,
    pub installation_id: Option<String>,
    pub client_owner_user_id: Option<String>,
    pub last_verified_at: Option<String>,
    pub last_error: Option<String>,
    pub note: Option<String>,
    pub ip_intel_json: Option<String>,
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

/// Host ownership for market ops: stable provider user id, legacy `email:` provider
/// key, or current host_owner_email matching the session.
pub(crate) fn session_is_host_owner(
    session: &crate::models::AuthSession,
    provider_id: Option<&str>,
    host_owner_email: &str,
) -> bool {
    if provider_id == Some(session.user_id.as_str()) {
        return true;
    }
    let Ok(session_email) = normalize_market_email(&session.email) else {
        return false;
    };
    let email_keyed = format!("email:{session_email}");
    if provider_id == Some(email_keyed.as_str()) {
        return true;
    }
    normalize_market_email(host_owner_email).is_ok_and(|host_email| host_email == session_email)
}

fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(", ")
}

impl AppStore {
    async fn client_market_create_host_import_job(
        &self,
        provider_id: &str,
        owner_email: &str,
        source_ip: IpAddr,
        entries: &[HostTransferEntry],
    ) -> Result<String, AppError> {
        let job_id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|error| {
            AppError::Internal(format!("begin Host import job failed: {error}"))
        })?;
        tx.execute(
            "INSERT INTO client_market_host_import_jobs (
                id, provider_id, owner_email, source_ip, status, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?5)",
            params![job_id, provider_id, owner_email, source_ip.to_string(), now],
        )
        .map_err(|error| AppError::Internal(format!("create Host import job failed: {error}")))?;
        for (position, entry) in entries.iter().enumerate() {
            let payload = serde_json::to_string(entry).map_err(|error| {
                AppError::Internal(format!("serialize Host import item failed: {error}"))
            })?;
            tx.execute(
                "INSERT INTO client_market_host_import_items (
                    id, job_id, position, ip, port, payload_json, status, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?7)",
                params![
                    Uuid::new_v4().to_string(),
                    job_id,
                    position as i64,
                    entry.ip.trim(),
                    i64::from(entry.port),
                    payload,
                    now
                ],
            )
            .map_err(|error| {
                AppError::Internal(format!("create Host import item failed: {error}"))
            })?;
        }
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit Host import job failed: {error}"))
        })?;
        Ok(job_id)
    }

    async fn client_market_claim_host_import_job(
        &self,
        job_id: &str,
    ) -> Result<HostImportJobWork, AppError> {
        let now = Utc::now().to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|error| {
            AppError::Internal(format!("begin Host import claim failed: {error}"))
        })?;
        let (provider_id, owner_email, source_ip, status): (String, String, String, String) = tx
            .query_row(
                "SELECT provider_id, owner_email, source_ip, status
                 FROM client_market_host_import_jobs WHERE id = ?1",
                params![job_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("read Host import job failed: {error}")))?
            .ok_or_else(|| AppError::NotFound("Host import job not found".into()))?;
        if !matches!(status.as_str(), "pending" | "running" | "completed") {
            return Err(AppError::Conflict(
                "Host import job cannot be resumed".into(),
            ));
        }
        if status != "completed" {
            tx.execute(
                "UPDATE client_market_host_import_jobs
                 SET status = 'running', updated_at = ?2
                 WHERE id = ?1",
                params![job_id, now],
            )
            .map_err(|error| {
                AppError::Internal(format!("claim Host import job failed: {error}"))
            })?;
            tx.execute(
                "UPDATE client_market_host_import_items
                 SET status = 'running', attempts = attempts + 1, updated_at = ?2
                 WHERE job_id = ?1 AND status IN ('pending', 'running')",
                params![job_id, now],
            )
            .map_err(|error| {
                AppError::Internal(format!("claim Host import items failed: {error}"))
            })?;
        }
        let items = {
            let mut statement = tx
                .prepare(
                    "SELECT id, payload_json
                     FROM client_market_host_import_items
                     WHERE job_id = ?1 AND status = 'running'
                     ORDER BY position ASC",
                )
                .map_err(|error| {
                    AppError::Internal(format!("prepare Host import items failed: {error}"))
                })?;
            statement
                .query_map(params![job_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|error| {
                    AppError::Internal(format!("read Host import items failed: {error}"))
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    AppError::Internal(format!("decode Host import items failed: {error}"))
                })?
        };
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit Host import claim failed: {error}"))
        })?;
        let source_ip = source_ip.parse::<IpAddr>().map_err(|error| {
            AppError::Internal(format!("stored Host import source IP is invalid: {error}"))
        })?;
        let items = items
            .into_iter()
            .map(|(id, payload)| {
                serde_json::from_str(&payload)
                    .map(|entry| HostImportItemWork { id, entry })
                    .map_err(|error| {
                        AppError::Internal(format!("stored Host import item is invalid: {error}"))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(HostImportJobWork {
            provider_id,
            owner_email,
            source_ip,
            items,
        })
    }

    async fn client_market_finish_host_import_item(
        &self,
        item_id: &str,
        result: &HostImportItemResult,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                "UPDATE client_market_host_import_items
                 SET status = ?2, host_id = ?3, error_message = ?4, updated_at = ?5
                 WHERE id = ?1 AND status = 'running'",
                params![
                    item_id,
                    result.status,
                    result.host_id,
                    result.error,
                    Utc::now().to_rfc3339()
                ],
            )
            .map_err(|error| {
                AppError::Internal(format!("finish Host import item failed: {error}"))
            })?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "Host import item is no longer running".into(),
            ));
        }
        Ok(())
    }

    async fn client_market_complete_host_import_job(
        &self,
        job_id: &str,
    ) -> Result<HostImportResponse, AppError> {
        let mut conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|error| {
            AppError::Internal(format!("begin Host import completion failed: {error}"))
        })?;
        let unfinished: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM client_market_host_import_items
                 WHERE job_id = ?1 AND status IN ('pending', 'running')",
                params![job_id],
                |row| row.get(0),
            )
            .map_err(|error| {
                AppError::Internal(format!("count unfinished Host imports failed: {error}"))
            })?;
        if unfinished != 0 {
            return Err(AppError::Conflict(
                "Host import job still has unfinished items".into(),
            ));
        }
        let now = Utc::now().to_rfc3339();
        let changed = tx
            .execute(
                "UPDATE client_market_host_import_jobs
                 SET status = 'completed', updated_at = ?2, completed_at = COALESCE(completed_at, ?2)
                 WHERE id = ?1 AND status IN ('pending', 'running', 'completed')",
                params![job_id, now],
            )
            .map_err(|error| {
                AppError::Internal(format!("complete Host import job failed: {error}"))
            })?;
        if changed != 1 {
            return Err(AppError::NotFound("Host import job not found".into()));
        }
        let items = {
            let mut statement = tx
                .prepare(
                    "SELECT ip, port, status, host_id, error_message
                     FROM client_market_host_import_items
                     WHERE job_id = ?1 ORDER BY position ASC",
                )
                .map_err(|error| {
                    AppError::Internal(format!("prepare Host import result failed: {error}"))
                })?;
            statement
                .query_map(params![job_id], |row| {
                    Ok(HostImportItemResult {
                        ip: row.get(0)?,
                        port: row.get(1)?,
                        status: row.get(2)?,
                        host_id: row.get(3)?,
                        error: row.get(4)?,
                    })
                })
                .map_err(|error| {
                    AppError::Internal(format!("read Host import result failed: {error}"))
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    AppError::Internal(format!("decode Host import result failed: {error}"))
                })?
        };
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit Host import completion failed: {error}"))
        })?;
        let imported = items
            .iter()
            .filter(|item| item.status == "imported")
            .count();
        let skipped = items.iter().filter(|item| item.status == "skipped").count();
        let failed = items.len().saturating_sub(imported + skipped);
        Ok(HostImportResponse {
            job_id: job_id.to_string(),
            imported,
            skipped,
            failed,
            items,
        })
    }

    async fn client_market_interrupted_host_import_jobs(&self) -> Result<Vec<String>, AppError> {
        let conn = self.conn.lock().await;
        let mut statement = conn
            .prepare(
                "SELECT id FROM client_market_host_import_jobs
                 WHERE status IN ('pending', 'running') ORDER BY created_at ASC",
            )
            .map_err(|error| {
                AppError::Internal(format!("prepare interrupted Host imports failed: {error}"))
            })?;
        statement
            .query_map([], |row| row.get(0))
            .map_err(|error| {
                AppError::Internal(format!("read interrupted Host imports failed: {error}"))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                AppError::Internal(format!("decode interrupted Host imports failed: {error}"))
            })
    }

    pub async fn client_market_endpoint_provider(
        &self,
        ip: &str,
        port: u16,
    ) -> Result<Option<String>, AppError> {
        let conn = self.conn.lock().await;
        conn.query_row(
            "SELECT COALESCE(provider_id, 'email:' || LOWER(host_owner_email))
             FROM router_ssh_hosts WHERE ip = ?1 AND port = ?2",
            params![ip, port],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| AppError::Internal(format!("lookup Host endpoint failed: {error}")))
    }

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
                    h.ip_intel_json, t.subdomain,
                    COALESCE(NULLIF(TRIM(t.owner_email), ''), NULLIF(TRIM(i.owner_email), '')),
                    s.client_user_id, h.provider_id, h.price_cents, h.rental_period_days, h.offer_revision,
                    COALESCE((SELECT methods_json FROM account_payment_profiles p
                              WHERE p.user_id = h.provider_id), '[]')
             FROM router_ssh_hosts h
             LEFT JOIN installation_client_tunnels t ON t.installation_id = h.installation_id
             LEFT JOIN installations i ON i.id = h.installation_id
             LEFT JOIN client_market_subscriptions s ON s.installation_id = h.installation_id
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
        ip_intel_json: Option<&str>,
    ) -> Result<RouterSshHostRecord, AppError> {
        let owner = normalize_market_email(owner_email)?;
        let provider_id = {
            let conn = self.conn.lock().await;
            conn.query_row(
                "SELECT id FROM users WHERE email_normalized = ?1",
                params![owner],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("resolve Host Provider failed: {e}")))?
            .unwrap_or_else(|| format!("email:{owner}"))
        };
        self.client_market_insert_host_for_provider(
            &provider_id,
            &owner,
            ip,
            port,
            country_code,
            hostname,
            fingerprint,
            note,
            ip_intel_json,
            None,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn client_market_insert_host_for_provider(
        &self,
        provider_id: &str,
        owner_email: &str,
        ip: &str,
        port: u16,
        country_code: Option<&str>,
        hostname: Option<&str>,
        fingerprint: Option<&str>,
        note: Option<&str>,
        ip_intel_json: Option<&str>,
        price_cents: Option<i64>,
        rental_period_days: Option<i64>,
    ) -> Result<RouterSshHostRecord, AppError> {
        let owner = normalize_market_email(owner_email)?;
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4().to_string();
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO host_provider_profiles (provider_id, owner_email, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(provider_id) DO UPDATE SET owner_email = excluded.owner_email, updated_at = excluded.updated_at",
            params![provider_id, owner, now],
        )
        .map_err(|e| AppError::Internal(format!("ensure Host Provider failed: {e}")))?;
        conn.execute(
            "INSERT INTO router_ssh_hosts (
                id, provider_id, ip, port, host_owner_email, country_code, hostname, ssh_host_key_fingerprint,
                status, installation_id, last_verified_at, last_error, note, ip_intel_json,
                price_cents, rental_period_days, offer_revision, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10, NULL, ?11, ?12,
                       ?13, ?14, 1, ?10, ?10)",
            params![
                id,
                provider_id,
                ip,
                port,
                owner,
                country_code,
                hostname,
                fingerprint,
                HOST_STATUS_IDLE,
                now,
                note,
                ip_intel_json,
                price_cents,
                rental_period_days,
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
        session: &crate::models::AuthSession,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let host = get_router_ssh_host(&conn, id)?
            .ok_or_else(|| AppError::NotFound("host not found".into()))?;
        if !session_is_host_owner(
            session,
            host.provider_id.as_deref(),
            &host.host_owner_email,
        ) {
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
        if host.installation_id.is_some() {
            return Err(AppError::Conflict(
                "host still has an installation; cleanup or reverify it before deletion".into(),
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
        session: &crate::models::AuthSession,
    ) -> Result<JobView, AppError> {
        let job = self
            .client_market_get_job_record(job_id)
            .await?
            .ok_or_else(|| AppError::NotFound("job not found".into()))?;
        let conn = self.conn.lock().await;
        let allowed = conn
            .query_row(
                "SELECT CASE WHEN j.client_owner_user_id = ?2 OR h.provider_id = ?2 THEN 1 ELSE 0 END
                 FROM provisioning_jobs j
                 LEFT JOIN router_ssh_hosts h ON h.id = j.host_id
                 WHERE j.id = ?1",
                params![job_id, session.user_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("authorize provisioning job failed: {error}")))?
            .is_some_and(|allowed| allowed != 0);
        if !allowed {
            return Err(AppError::Forbidden("not allowed to view this job".into()));
        }
        drop(conn);
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

    pub async fn client_market_set_job_phase(
        &self,
        job_id: &str,
        phase: &str,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                "UPDATE provisioning_jobs
                 SET phase = ?2, updated_at = ?3
                 WHERE id = ?1 AND status = 'running'",
                params![job_id, phase, Utc::now().to_rfc3339()],
            )
            .map_err(|e| AppError::Internal(format!("set provisioning job phase failed: {e}")))?;
        if changed != 1 {
            return Err(AppError::Conflict("provisioning job is not running".into()));
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
            return Err(AppError::Unauthorized(
                "provision credential rejected for this host IP or job state".into(),
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
               AND NOT EXISTS (
                   SELECT 1 FROM host_provider_client_blocks b
                   WHERE b.provider_id = router_ssh_hosts.provider_id
                     AND b.client_user_id = (
                         SELECT client_owner_user_id FROM provisioning_jobs WHERE id = ?
                     )
                     AND b.lifted_at IS NULL
               )
             ORDER BY RANDOM()
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
        values.push(&job_id);
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

    pub async fn client_market_list_hosts_by_status(
        &self,
        status: &str,
    ) -> Result<Vec<RouterSshHostRecord>, AppError> {
        let conn = self.conn.lock().await;
        let mut statement = conn
            .prepare(
                "SELECT h.id, h.ip, h.port, h.host_owner_email, h.country_code, h.hostname,
                        h.ssh_host_key_fingerprint, h.status, h.installation_id,
                        h.last_verified_at, h.last_error, h.note, h.created_at, h.updated_at,
                        h.ip_intel_json, t.subdomain,
                        COALESCE(NULLIF(TRIM(t.owner_email), ''), NULLIF(TRIM(i.owner_email), '')),
                        s.client_user_id, h.provider_id, h.price_cents, h.rental_period_days, h.offer_revision,
                        COALESCE((SELECT methods_json FROM account_payment_profiles p
                                  WHERE p.user_id = h.provider_id), '[]')
                 FROM router_ssh_hosts h
                 LEFT JOIN installation_client_tunnels t ON t.installation_id = h.installation_id
                 LEFT JOIN installations i ON i.id = h.installation_id
                 LEFT JOIN client_market_subscriptions s ON s.installation_id = h.installation_id
                 WHERE h.status = ?1
                 ORDER BY h.updated_at ASC",
            )
            .map_err(|e| AppError::Internal(format!("prepare hosts by status failed: {e}")))?;
        let rows = statement
            .query_map(params![status], map_router_ssh_host_row)
            .map_err(|e| AppError::Internal(format!("query hosts by status failed: {e}")))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Internal(format!("read hosts by status failed: {e}")))
    }

    pub async fn client_market_host_has_active_job(&self, host_id: &str) -> Result<bool, AppError> {
        let conn = self.conn.lock().await;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM provisioning_jobs
                 WHERE host_id = ?1 AND status IN ('pending', 'running')",
                params![host_id],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Internal(format!("count active host jobs failed: {e}")))?;
        Ok(count > 0)
    }

    /// True when an unexpired `active` allocation quote still holds this host, i.e.
    /// the `reserved` status is legitimate and must not be reclaimed.
    pub async fn client_market_host_has_live_quote(&self, host_id: &str) -> Result<bool, AppError> {
        let conn = self.conn.lock().await;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*)
                 FROM client_market_allocation_quote_items i
                 JOIN client_market_allocation_quotes q ON q.id = i.quote_id
                 WHERE i.host_id = ?1 AND q.status = 'active' AND q.expires_at > ?2",
                params![host_id, Utc::now().to_rfc3339()],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Internal(format!("count live host quotes failed: {e}")))?;
        Ok(count > 0)
    }

    /// Compare-and-set a host status. Returns `false` when the host is no longer in
    /// `expected_status`, which means a concurrent job already moved it — the caller
    /// should skip rather than overwrite. Blind writes here previously let the
    /// reconcile loop clobber a cleanup job that was mid-commit.
    pub async fn client_market_force_host_status(
        &self,
        host_id: &str,
        expected_status: &str,
        status: &str,
        last_error: &str,
    ) -> Result<bool, AppError> {
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                "UPDATE router_ssh_hosts
                 SET status = ?2, last_error = ?3, updated_at = ?4
                 WHERE id = ?1 AND status = ?5",
                params![
                    host_id,
                    status,
                    last_error,
                    Utc::now().to_rfc3339(),
                    expected_status
                ],
            )
            .map_err(|e| AppError::Internal(format!("force host status failed: {e}")))?;
        Ok(changed == 1)
    }

    pub async fn client_market_get_host_for_operator(
        &self,
        id: &str,
        session: &crate::models::AuthSession,
    ) -> Result<RouterSshHostRecord, AppError> {
        let conn = self.conn.lock().await;
        let host = get_router_ssh_host(&conn, id)?
            .ok_or_else(|| AppError::NotFound("host not found".into()))?;
        if !session_is_host_owner(
            session,
            host.provider_id.as_deref(),
            &host.host_owner_email,
        ) {
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
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(|e| AppError::Internal(format!("begin complete host reverify failed: {e}")))?;
        let now = Utc::now().to_rfc3339();
        let detached_installation_id = tx
            .query_row(
                "SELECT installation_id FROM router_ssh_hosts WHERE id = ?1",
                params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("read reverified Host state failed: {e}")))?
            .flatten();
        let changed = tx
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
        if let Some(installation_id) = detached_installation_id.as_deref() {
            let has_subscription: i64 = tx
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM client_market_subscriptions WHERE installation_id = ?1
                    )",
                    params![installation_id],
                    |row| row.get(0),
                )
                .map_err(|e| {
                    AppError::Internal(format!("check reverified Host subscription failed: {e}"))
                })?;
            if has_subscription != 0 {
                crate::client_market_trade::cleanup_finished_tx(
                    &tx,
                    installation_id,
                    id,
                    Utc::now(),
                )?;
            }
        }
        tx.commit().map_err(|e| {
            AppError::Internal(format!("commit complete host reverify failed: {e}"))
        })?;
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
        let tx = conn
            .transaction()
            .map_err(|e| AppError::Internal(format!("begin quarantine host tx failed: {e}")))?;
        let now = Utc::now().to_rfc3339();
        let clipped = if reason.chars().count() > 500 {
            format!("{}…", reason.chars().take(497).collect::<String>())
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
            // Successful remote rollback returns the host to the idle pool — do not
            // leave a stale machine failure code on an otherwise allocatable host.
            let host_last_error: Option<&str> = if release_to_idle {
                None
            } else {
                Some(failure_code)
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
                        host_last_error,
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
        dashboard_url: &str,
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
        crate::client_market_trade::complete_provisioning_tx(
            &tx,
            job_id,
            host_id,
            installation_id,
            dashboard_url,
            Utc::now(),
        )?;
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
        let actor_user_id = {
            let conn = self.conn.lock().await;
            conn.query_row(
                "SELECT COALESCE(
                    (SELECT h.provider_id FROM router_ssh_hosts h
                     WHERE h.installation_id = ?1
                       AND LOWER(h.host_owner_email) = LOWER(?2)),
                    (SELECT s.client_user_id FROM client_market_subscriptions s
                     WHERE s.installation_id = ?1
                       AND LOWER(s.client_owner_email) = LOWER(?2)),
                    (SELECT u.id FROM users u WHERE u.email_normalized = LOWER(?2))
                 )",
                params![installation_id, viewer],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(|error| {
                AppError::Internal(format!("resolve cleanup actor identity failed: {error}"))
            })?
        };
        self.client_market_begin_cleanup_job_with_context(
            installation_id,
            actor_user_id.as_deref(),
            &viewer,
            is_admin,
            "operator_release",
            None,
        )
        .await
    }

    pub async fn client_market_begin_system_cleanup_job(
        &self,
        installation_id: &str,
        reason: &str,
    ) -> Result<String, AppError> {
        self.client_market_begin_cleanup_job_with_context(
            installation_id,
            None,
            "router-system@internal.invalid",
            true,
            reason,
            Some(false),
        )
        .await
    }

    pub async fn client_market_begin_cleanup_job_with_context(
        &self,
        installation_id: &str,
        actor_user_id: Option<&str>,
        viewer_email: &str,
        is_admin: bool,
        reason: &str,
        block_client_for_provider: Option<bool>,
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
        let host = tx
            .query_row(
                "SELECT id, provider_id, host_owner_email, status
                 FROM router_ssh_hosts WHERE installation_id = ?1",
                params![installation_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("lookup provision host failed: {e}")))?
            .ok_or_else(|| AppError::NotFound("provision host not found".into()))?;
        let subscription_owner: Option<(String, String)> = tx
            .query_row(
                "SELECT client_user_id, client_owner_email
                 FROM client_market_subscriptions WHERE installation_id = ?1",
                params![installation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("read cleanup billing owner failed: {error}"))
            })?;
        if reason == "payment_deadline_expired" {
            let still_expired: i64 = tx
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM client_market_subscriptions
                        WHERE installation_id = ?1 AND status = 'payment_due'
                          AND payment_deadline IS NOT NULL AND payment_deadline <= ?2
                    )",
                    params![installation_id, Utc::now().to_rfc3339()],
                    |row| row.get(0),
                )
                .map_err(|error| {
                    AppError::Internal(format!(
                        "recheck expired billing before cleanup failed: {error}"
                    ))
                })?;
            if still_expired == 0 {
                return Err(AppError::Conflict(
                    "the Client payment state changed before automatic release".into(),
                ));
            }
        }
        let is_host_owner = actor_user_id.is_some_and(|user_id| host.1.as_deref() == Some(user_id))
            || normalize_market_email(&viewer)
                .ok()
                .and_then(|viewer_email| {
                    normalize_market_email(&host.2)
                        .ok()
                        .map(|host_email| host_email == viewer_email)
                })
                .unwrap_or(false);
        let is_client_owner = actor_user_id.is_some_and(|user_id| {
            subscription_owner
                .as_ref()
                .is_some_and(|owner| owner.0 == user_id)
        }) || subscription_owner.as_ref().is_some_and(|owner| {
            normalize_market_email(&viewer)
                .ok()
                .and_then(|viewer_email| {
                    normalize_market_email(&owner.1)
                        .ok()
                        .map(|client_email| client_email == viewer_email)
                })
                .unwrap_or(false)
        });
        // `is_admin` here means Router system automation only (no session actor).
        // Human admins / Router owners are never elevated for market Host cleanup.
        let system_operator = is_admin && actor_user_id.is_none();
        if !system_operator && !is_host_owner && !is_client_owner {
            return Err(AppError::Forbidden(
                "not allowed to cleanup this client".into(),
            ));
        }
        if is_host_owner {
            if let Some(provider_id) = host.1.as_deref() {
                tx.execute(
                    "INSERT INTO host_provider_profiles
                        (provider_id, owner_email, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?3)
                     ON CONFLICT(provider_id) DO UPDATE SET
                        owner_email = excluded.owner_email,
                        updated_at = excluded.updated_at",
                    params![provider_id, viewer, Utc::now().to_rfc3339()],
                )
                .map_err(|error| {
                    AppError::Internal(format!("sync cleanup Provider profile failed: {error}"))
                })?;
            }
        }
        if !matches!(
            host.3.as_str(),
            HOST_STATUS_ALLOCATED | HOST_STATUS_UNREACHABLE | HOST_STATUS_DRAINING
        ) {
            return Err(AppError::Conflict(
                "client host is already being cleaned or is unavailable".into(),
            ));
        }
        if host.3 == HOST_STATUS_DRAINING {
            let active: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM provisioning_jobs
                     WHERE host_id = ?1 AND status IN ('pending', 'running')",
                    params![host.0],
                    |row| row.get(0),
                )
                .map_err(|e| {
                    AppError::Internal(format!("count active cleanup jobs failed: {e}"))
                })?;
            if active > 0 {
                return Err(AppError::Conflict(
                    "client host already has an active cleanup job".into(),
                ));
            }
        }
        let job_id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let client_owner_email = subscription_owner
            .as_ref()
            .map(|owner| owner.1.as_str())
            .unwrap_or(owner_email.as_str());
        let should_block_client = is_host_owner
            && reason == "payment_not_received"
            && block_client_for_provider.unwrap_or(true)
            && subscription_owner
                .as_ref()
                .is_some_and(|owner| host.1.as_deref() != Some(owner.0.as_str()));
        tx.execute(
            "INSERT INTO provisioning_jobs (
                id, type, host_id, host_owner_email, client_owner_email,
                selection_owners_json, selection_regions_json, subdomain, installation_id,
                status, phase, log_blob, secret_ref, failure_code, created_at, updated_at,
                cleanup_reason
             ) VALUES (?1, ?2, ?3, ?4, ?5, '[]', '[]', ?6, ?7, ?8, ?9, '', NULL, NULL, ?10, ?10, ?11)",
            params![
                job_id,
                JOB_TYPE_CLEANUP,
                host.0,
                host.2,
                client_owner_email,
                subdomain,
                installation_id,
                JOB_STATUS_PENDING,
                JOB_PHASE_CLEANUP_STOP,
                now,
                reason,
            ],
        )
        .map_err(|e| AppError::Internal(format!("insert cleanup job failed: {e}")))?;
        let changed = tx
            .execute(
                "UPDATE router_ssh_hosts
                 SET status = ?2, updated_at = ?3
                 WHERE id = ?1 AND status IN ('allocated', 'unreachable', 'draining') AND installation_id = ?4",
                params![host.0, HOST_STATUS_DRAINING, now, installation_id],
            )
            .map_err(|e| AppError::Internal(format!("mark cleanup host draining failed: {e}")))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "client host cleanup raced with another operation".into(),
            ));
        }
        crate::client_market_trade::cleanup_started_tx(
            &tx,
            installation_id,
            &host.0,
            actor_user_id,
            Some(&viewer),
            reason,
            should_block_client,
            Utc::now(),
        )?;
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
        let installation_id: String = tx
            .query_row(
                "SELECT installation_id FROM provisioning_jobs
                 WHERE id = ?1 AND host_id = ?2 AND type = 'cleanup'",
                params![job_id, host_id],
                |row| row.get(0),
            )
            .map_err(|e| {
                AppError::Internal(format!("read completed cleanup installation failed: {e}"))
            })?;
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
        crate::client_market_trade::cleanup_finished_tx(
            &tx,
            &installation_id,
            host_id,
            Utc::now(),
        )?;
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
            crate::client_market_trade::cleanup_finished_tx(
                &tx,
                &installation_id,
                host_id,
                Utc::now(),
            )?;
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
        crate::client_market_trade::cleanup_failed_tx(
            &tx,
            &installation_id,
            host_id,
            failure_code,
            Utc::now(),
        )?;
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
                h.ip_intel_json, t.subdomain,
                COALESCE(NULLIF(TRIM(t.owner_email), ''), NULLIF(TRIM(i.owner_email), '')),
                s.client_user_id, h.provider_id, h.price_cents, h.rental_period_days, h.offer_revision,
                COALESCE((SELECT methods_json FROM account_payment_profiles p
                          WHERE p.user_id = h.provider_id), '[]')
         FROM router_ssh_hosts h
         LEFT JOIN installation_client_tunnels t ON t.installation_id = h.installation_id
         LEFT JOIN installations i ON i.id = h.installation_id
         LEFT JOIN client_market_subscriptions s ON s.installation_id = h.installation_id
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
    let methods_json: String = row.get(22)?;
    let mut payment_method_kinds =
        serde_json::from_str::<Vec<crate::client_market_trade::PaymentMethod>>(&methods_json)
            .unwrap_or_default()
            .into_iter()
            .map(|method| method.kind)
            .collect::<Vec<_>>();
    payment_method_kinds.sort();
    payment_method_kinds.dedup();
    Ok(RouterSshHostRecord {
        id: row.get(0)?,
        ip: row.get(1)?,
        port: row.get::<_, i64>(2)? as u16,
        host_owner_email: row.get(3)?,
        country_code: row.get(4)?,
        hostname: row.get(5)?,
        ssh_host_key_fingerprint: row.get(6)?,
        status: row.get(7)?,
        installation_id: row.get(8)?,
        last_verified_at: row.get(9)?,
        last_error: row.get(10)?,
        note: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        ip_intel_json: row.get(14)?,
        client_subdomain: row.get(15)?,
        client_owner_email: row.get(16)?,
        client_owner_user_id: row.get(17)?,
        provider_id: row.get(18)?,
        price_cents: row.get(19)?,
        rental_period_days: row.get(20)?,
        offer_revision: row.get(21)?,
        payment_method_kinds,
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
            ip_intel_endpoints: Vec::new(),
            verification_service_base_url: "https://tokenswitch.org".into(),
            verification_service_api_key: None,
            router_owner_email: None,
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
                None,
            )
            .await
            .expect("insert host")
    }

    async fn ensure_payment_profile(store: &AppStore, user_id: &str, email: &str) {
        use crate::client_market_trade::PaymentMethod;
        store
            .client_market_update_payment_profile(
                &market_session(user_id, email),
                &[PaymentMethod {
                    kind: "alipay".into(),
                    account: Some("13800000000".into()),
                    qr_image_url: None,
                    asset_url: None,
                    token: None,
                    chain: None,
                    address: None,
                    instructions: None,
                }],
            )
            .await
            .expect("configure test payment profile");
    }

    fn market_session(user_id: &str, email: &str) -> crate::models::AuthSession {
        let now = Utc::now();
        crate::models::AuthSession {
            session_id: format!("session-{user_id}"),
            user_id: user_id.to_string(),
            email: email.to_string(),
            installation_id: format!("browser-{user_id}"),
            access_token_hash: format!("access-{user_id}"),
            refresh_token_hash: format!("refresh-{user_id}"),
            access_expires_at: now + chrono::Duration::hours(1),
            refresh_expires_at: now + chrono::Duration::days(30),
            created_at: now,
            last_used_at: now,
        }
    }

    async fn add_provider_host(
        store: &AppStore,
        provider_id: &str,
        email: &str,
        ip: &str,
        country: &str,
        price_cents: Option<i64>,
        rental_period_days: Option<i64>,
    ) -> RouterSshHostRecord {
        store
            .client_market_insert_host_for_provider(
                provider_id,
                email,
                ip,
                22,
                Some(country),
                Some("market-host"),
                Some("SHA256:market-test"),
                None,
                None,
                price_cents,
                rental_period_days,
            )
            .await
            .expect("insert Provider Host")
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
             ) VALUES (?1, ?2, 'linux', 'test', ?3, ?4, ?4, ?4, NULL, NULL)",
            params![
                installation_id,
                format!("test-public-key-{installation_id}"),
                owner,
                now
            ],
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
    async fn host_import_jobs_persist_item_progress_and_resume_only_unfinished_work() {
        let (store, _, root) = test_store("host-import-resume");
        let entries = vec![
            HostTransferEntry {
                ip: "203.0.113.10".into(),
                port: 22,
                note: Some("first".into()),
                price_cents: None,
                rental_period_days: None,
                expected_fingerprint: Some("SHA256:first".into()),
                informational_status: Some("idle".into()),
            },
            HostTransferEntry {
                ip: "203.0.113.11".into(),
                port: 2222,
                note: Some("second".into()),
                price_cents: Some(500),
                rental_period_days: Some(4),
                expected_fingerprint: Some("SHA256:second".into()),
                informational_status: Some("allocated".into()),
            },
        ];
        let job_id = store
            .client_market_create_host_import_job(
                "provider-import",
                "provider@example.com",
                "198.51.100.8".parse().unwrap(),
                &entries,
            )
            .await
            .expect("persist Host import job");
        assert_eq!(
            store
                .client_market_interrupted_host_import_jobs()
                .await
                .unwrap(),
            vec![job_id.clone()]
        );

        let first_claim = store
            .client_market_claim_host_import_job(&job_id)
            .await
            .expect("claim Host import job");
        assert_eq!(first_claim.provider_id, "provider-import");
        assert_eq!(first_claim.owner_email, "provider@example.com");
        assert_eq!(first_claim.items.len(), 2);
        let first_item_id = first_claim.items[0].id.clone();
        let second_item_id = first_claim.items[1].id.clone();
        drop(first_claim);

        let resumed = store
            .client_market_claim_host_import_job(&job_id)
            .await
            .expect("resume running Host import job");
        assert_eq!(resumed.items.len(), 2);
        store
            .client_market_finish_host_import_item(
                &first_item_id,
                &HostImportItemResult {
                    ip: entries[0].ip.clone(),
                    port: entries[0].port,
                    status: "imported".into(),
                    host_id: Some("host-imported".into()),
                    error: None,
                },
            )
            .await
            .expect("persist imported item");
        store
            .client_market_finish_host_import_item(
                &second_item_id,
                &HostImportItemResult {
                    ip: entries[1].ip.clone(),
                    port: entries[1].port,
                    status: "failed".into(),
                    host_id: None,
                    error: Some("fingerprint mismatch".into()),
                },
            )
            .await
            .expect("persist failed item");

        let completed = store
            .client_market_complete_host_import_job(&job_id)
            .await
            .expect("complete Host import job");
        assert_eq!(completed.job_id, job_id);
        assert_eq!(completed.imported, 1);
        assert_eq!(completed.skipped, 0);
        assert_eq!(completed.failed, 1);
        assert_eq!(completed.items[0].host_id.as_deref(), Some("host-imported"));
        assert_eq!(
            completed.items[1].error.as_deref(),
            Some("fingerprint mismatch")
        );
        assert!(
            store
                .client_market_interrupted_host_import_jobs()
                .await
                .unwrap()
                .is_empty()
        );

        let completed_claim = store
            .client_market_claim_host_import_job(&job_id)
            .await
            .expect("read completed Host import job");
        assert!(completed_claim.items.is_empty());
        let reread = store
            .client_market_complete_host_import_job(&job_id)
            .await
            .expect("reread completed Host import result");
        assert_eq!(reread.imported, 1);
        assert_eq!(reread.failed, 1);
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn owner_region_selection_randomly_uses_only_matching_hosts() {
        let (store, _, root) = test_store("selection");
        let wrong_region = add_host(&store, "one@example.com", "198.18.0.1", "FR").await;
        let wrong_owner = add_host(&store, "three@example.com", "198.18.0.2", "US").await;
        let matching_de = add_host(&store, "two@example.com", "198.18.0.3", "DE").await;
        let matching_us = add_host(&store, "one@example.com", "198.18.0.4", "US").await;
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
        assert!([matching_de.id.as_str(), matching_us.id.as_str()].contains(&claimed.id.as_str()));
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
        assert_eq!(job.host_id.as_deref(), Some(claimed.id.as_str()));
        assert_eq!(
            job.host_owner_email.as_deref(),
            Some(claimed.host_owner_email.as_str())
        );
        let unclaimed_matching_id = if claimed.id == matching_de.id {
            &matching_us.id
        } else {
            &matching_de.id
        };
        assert_eq!(
            store
                .client_market_get_host(unclaimed_matching_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            HOST_STATUS_IDLE
        );
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
            // Peer IP may differ from the SSH host address (cloudflared / dual-stack).
            // Owner + active create job is enough to authorize claim.
            assert!(
                client_market_subdomain_available_to_source(
                    &conn,
                    "bound-client",
                    None,
                    Some("198.18.2.2"),
                )
                .unwrap(),
                "unbound active reservation should pass preflight even with mismatched peer IP"
            );
            let tx = conn.transaction().unwrap();
            authorize_client_market_subdomain_claim(
                &tx,
                "bound-client",
                "installation-bound",
                "client@example.com",
                Some("198.18.2.2"),
            )
            .expect("bind reservation without matching peer IP");
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
                .unwrap(),
                "after binding, mismatched peer IP must not unlock another caller"
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
                "https://binding-job.router.test",
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
                .is_err()
        );
        assert_eq!(
            secrets.redeem_token(&token_hash, host_ip).unwrap().password,
            "not-persisted"
        );
        secrets.tokens.get_mut(&token_hash).unwrap().expires_at =
            Instant::now() - Duration::from_secs(1);
        assert!(secrets.redeem_token(&token_hash, host_ip).is_err());
        assert!(!sanitize_job_log_chunk(&format!("token={raw_token}")).contains(&raw_token));
        assert!(
            sanitize_job_log_chunk("Provision token redemption failed\n")
                .contains("Provision token redemption failed")
        );
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
        let reusable_after = store
            .client_market_get_host(&reusable.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reusable_after.status, HOST_STATUS_IDLE);
        assert_eq!(reusable_after.last_error, None);

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
                .client_market_begin_cleanup_job(
                    "cleanup-installation",
                    "stranger@example.com",
                    false,
                )
                .await,
            Err(AppError::Forbidden(_))
        ));
        let job_id = store
            .client_market_begin_cleanup_job("cleanup-installation", "host@example.com", false)
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

    #[tokio::test]
    async fn reverify_releases_failed_subscription_and_allows_host_reuse() {
        let (store, _config, root) = test_store("reverify-subscription-reuse");
        let host = add_provider_host(
            &store,
            "provider-reuse",
            "provider@example.com",
            "198.18.5.3",
            "US",
            Some(500),
            Some(30),
        )
        .await;
        let now = Utc::now().to_rfc3339();
        {
            let conn = store.conn.lock().await;
            insert_installation(&conn, "failed-release-client", "renter@example.com");
            conn.execute(
                "UPDATE installations SET provision_source = ?2, provision_host_id = ?3 WHERE id = ?1",
                params!["failed-release-client", PROVISION_SOURCE_ROUTER_MARKET, host.id],
            )
            .unwrap();
            insert_tunnel_and_public_host(
                &conn,
                "failed-release-client",
                "renter@example.com",
                "failed-release-client",
            );
            conn.execute(
                "UPDATE router_ssh_hosts
                 SET status = 'unreachable', installation_id = 'failed-release-client'
                 WHERE id = ?1",
                params![host.id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO client_market_subscriptions
                    (installation_id, host_id, provider_id, host_owner_email,
                     client_user_id, client_owner_email, status, price_cents,
                     rental_period_days, offer_revision, created_at, updated_at)
                 VALUES ('failed-release-client', ?1, 'provider-reuse', 'provider@example.com',
                         'renter-reuse', 'renter@example.com', 'release_failed', 500,
                         30, 1, ?2, ?2)",
                params![host.id, now],
            )
            .unwrap();
        }

        store
            .purge_installation_for_client_market("failed-release-client")
            .await
            .unwrap();
        let recovered = store
            .client_market_complete_host_reverify(&host.id, Some("reused-host"), None)
            .await
            .expect("complete Host recovery after purge");
        assert_eq!(recovered.status, HOST_STATUS_IDLE);
        assert!(recovered.installation_id.is_none());
        store
            .client_market_assert_creation_allowed(&market_session(
                "renter-reuse",
                "renter@example.com",
            ))
            .await
            .expect("released recovery no longer blocks the Client owner");
        {
            let conn = store.conn.lock().await;
            let status: String = conn
                .query_row(
                    "SELECT status FROM client_market_subscriptions
                     WHERE installation_id = 'failed-release-client'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(status, "released");
            conn.execute(
                "INSERT INTO client_market_subscriptions
                    (installation_id, host_id, provider_id, host_owner_email,
                     client_user_id, client_owner_email, status, offer_revision,
                     created_at, updated_at)
                 VALUES ('replacement-client', ?1, 'provider-reuse', 'provider@example.com',
                         'replacement-renter', 'replacement@example.com', 'active', 1, ?2, ?2)",
                params![host.id, Utc::now().to_rfc3339()],
            )
            .expect("released subscription must not reserve the Host forever");
        }

        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn public_host_views_hide_operational_and_owner_details() {
        let host = RouterSshHostRecord {
            id: "host-id".into(),
            provider_id: Some("provider-id".into()),
            ip: "203.0.113.9".into(),
            port: 2222,
            host_owner_email: "host@example.com".into(),
            price_cents: Some(500),
            rental_period_days: Some(30),
            offer_revision: 1,
            payment_method_kinds: vec!["alipay".into()],
            country_code: Some("US".into()),
            hostname: Some("host.example".into()),
            ssh_host_key_fingerprint: Some("SHA256:secret".into()),
            status: HOST_STATUS_ALLOCATED.into(),
            client_subdomain: Some("public-client".into()),
            client_owner_email: Some("client@example.com".into()),
            installation_id: Some("installation-id".into()),
            client_owner_user_id: Some("client-id".into()),
            last_verified_at: Some("verified-at".into()),
            last_error: Some("diagnostic".into()),
            note: Some("operator note".into()),
            ip_intel_json: Some(
                r#"{"query":"203.0.113.9","location":"United States · Texas","riskLevel":"轻微风险","countryCode":"US","asn":"AS401701","isp":"Cognetcloud INC","classificationType":"IDC 机房 IP","source":"iq"}"#
                    .into(),
            ),
            created_at: "created-at".into(),
            updated_at: "updated-at".into(),
        };
        let public = serde_json::to_value(host_to_view(host.clone(), false)).unwrap();
        for key in [
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
        // Full IP is public; only port and operational fields stay owner-private.
        assert_eq!(
            public.get("ip").and_then(|value| value.as_str()),
            Some("203.0.113.9")
        );
        let public_intel = public
            .get("ipIntel")
            .expect("public view should expose ip intel");
        assert_eq!(
            public_intel.get("isp").and_then(|value| value.as_str()),
            Some("Cognetcloud INC")
        );
        assert_eq!(
            public_intel.get("asn").and_then(|value| value.as_str()),
            Some("AS401701")
        );
        assert_eq!(
            public_intel
                .get("riskLevel")
                .and_then(|value| value.as_str()),
            Some("轻微风险")
        );
        assert_eq!(
            public_intel.get("ip").and_then(|value| value.as_str()),
            Some("203.0.113.9")
        );
        assert!(public_intel.get("latitude").is_none());
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

    #[tokio::test]
    async fn allocation_quotes_enforce_limits_release_hosts_and_commit_once() {
        use crate::client_market_trade::CreateQuoteRequest;

        let (store, _config, root) = test_store("trade-quotes");
        let first = add_provider_host(
            &store,
            "provider-1",
            "provider@example.com",
            "198.18.20.1",
            "US",
            Some(500),
            Some(30),
        )
        .await;
        let second = add_provider_host(
            &store,
            "provider-1",
            "provider@example.com",
            "198.18.20.2",
            "US",
            Some(500),
            Some(30),
        )
        .await;
        let client = market_session("client-1", "client@example.com");

        assert!(matches!(
            store
                .client_market_create_quote(
                    &client,
                    CreateQuoteRequest {
                        provider_ids: vec!["provider-1".into()],
                        country_codes: vec!["US".into()],
                        count: 3,
                        host_id: None,
                    },
                )
                .await,
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            store
                .client_market_create_quote(
                    &client,
                    CreateQuoteRequest {
                        provider_ids: Vec::new(),
                        country_codes: Vec::new(),
                        count: 2,
                        host_id: Some(first.id.clone()),
                    },
                )
                .await,
            Err(AppError::BadRequest(_))
        ));

        let quote = store
            .client_market_create_quote(
                &client,
                CreateQuoteRequest {
                    provider_ids: vec!["provider-1".into()],
                    country_codes: vec!["US".into()],
                    count: 2,
                    host_id: None,
                },
            )
            .await
            .expect("create two-Host quote");
        assert_eq!(quote.items.len(), 2);
        let provider = market_session("provider-1", "provider@example.com");
        ensure_payment_profile(&store, "provider-1", "provider@example.com").await;
        let changed_offer = store
            .client_market_update_host_offer(&first.id, &provider, Some(900), Some(60))
            .await
            .expect("Provider may change an offer while a quote is reserved");
        assert_eq!(changed_offer.offer_revision, 2);
        assert_eq!(
            store
                .client_market_get_host(&first.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            crate::client_market_trade::HOST_STATUS_RESERVED
        );
        let stale_prepared = quote
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                (
                    item.id.clone(),
                    format!("stale-quoted-client-{index}"),
                    "secret-password".into(),
                    item.offer_revision,
                )
            })
            .collect::<Vec<_>>();
        assert!(matches!(
            store
                .client_market_commit_quote(&quote.id, &client, &stale_prepared)
                .await,
            Err(AppError::Conflict(_))
        ));
        store
            .client_market_cancel_quote(&quote.id, &client)
            .await
            .expect("cancel quote");
        store
            .client_market_cancel_quote(&quote.id, &client)
            .await
            .expect("repeat quote cancellation");
        for host_id in [&first.id, &second.id] {
            assert_eq!(
                store
                    .client_market_get_host(host_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .status,
                HOST_STATUS_IDLE
            );
        }

        let expiring = store
            .client_market_create_quote(
                &client,
                CreateQuoteRequest {
                    provider_ids: Vec::new(),
                    country_codes: Vec::new(),
                    count: 1,
                    host_id: Some(second.id.clone()),
                },
            )
            .await
            .expect("create expiring quote");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE client_market_allocation_quotes SET expires_at = ?2 WHERE id = ?1",
                params![
                    expiring.id,
                    (Utc::now() - chrono::Duration::seconds(1)).to_rfc3339()
                ],
            )
            .unwrap();
        }
        store
            .client_market_assert_creation_allowed(&client)
            .await
            .expect("expire stale quote");
        assert_eq!(
            store
                .client_market_get_host(&second.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            HOST_STATUS_IDLE
        );

        let fixed = store
            .client_market_create_quote(
                &client,
                CreateQuoteRequest {
                    provider_ids: Vec::new(),
                    country_codes: Vec::new(),
                    count: 1,
                    host_id: Some(first.id.clone()),
                },
            )
            .await
            .expect("create fixed Host quote");
        let prepared = vec![(
            fixed.items[0].id.clone(),
            "quoted-client".into(),
            "secret".into(),
            fixed.items[0].offer_revision,
        )];
        let committed = store
            .client_market_commit_quote(&fixed.id, &client, &prepared)
            .await
            .expect("commit quote");
        assert_eq!(committed.job_ids.len(), 1);
        assert!(matches!(
            store
                .client_market_commit_quote(&fixed.id, &client, &prepared)
                .await,
            Err(AppError::Gone(_))
        ));
        assert_eq!(
            store
                .client_market_get_host(&first.id)
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
    async fn quote_commit_rechecks_provider_block_and_cancel_releases_host() {
        use crate::client_market_trade::CreateQuoteRequest;

        let (store, _config, root) = test_store("trade-quote-provider-block");
        let host = add_provider_host(
            &store,
            "provider-blocked-quote",
            "provider@example.com",
            "198.18.20.3",
            "US",
            Some(500),
            Some(30),
        )
        .await;
        let client = market_session("client-blocked-quote", "client@example.com");
        let quote = store
            .client_market_create_quote(
                &client,
                CreateQuoteRequest {
                    provider_ids: Vec::new(),
                    country_codes: Vec::new(),
                    count: 1,
                    host_id: Some(host.id.clone()),
                },
            )
            .await
            .expect("create quote before Provider block");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "INSERT INTO host_provider_client_blocks
                    (provider_id, client_user_id, client_owner_email, reason, created_at)
                 VALUES (?1, ?2, ?3, 'payment_not_received', ?4)",
                params![
                    "provider-blocked-quote",
                    client.user_id,
                    client.email,
                    Utc::now().to_rfc3339(),
                ],
            )
            .unwrap();
        }
        let prepared = vec![(
            quote.items[0].id.clone(),
            "blocked-quote-client".into(),
            "secret".into(),
            quote.items[0].offer_revision,
        )];
        assert!(matches!(
            store
                .client_market_commit_quote(&quote.id, &client, &prepared)
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
            crate::client_market_trade::HOST_STATUS_RESERVED
        );
        store
            .client_market_cancel_quote(&quote.id, &client)
            .await
            .expect("cancel blocked quote");
        assert_eq!(
            store
                .client_market_get_host(&host.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            HOST_STATUS_IDLE
        );

        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn offer_updates_freeze_paid_period_and_payment_declaration_is_idempotent() {
        use crate::client_market_trade::PaymentMethod;

        let (store, _config, root) = test_store("trade-offer-payment");
        let host = add_provider_host(
            &store,
            "provider-stable",
            "provider-old@example.com",
            "198.18.21.1",
            "US",
            Some(500),
            Some(30),
        )
        .await;
        let now = Utc::now();
        let frozen_end = now + chrono::Duration::days(2);
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "INSERT INTO client_market_subscriptions
                    (installation_id, host_id, provider_id, host_owner_email,
                     client_user_id, client_owner_email, status, price_cents,
                     rental_period_days, offer_revision, last_declared_at,
                     current_period_end, created_at, updated_at)
                 VALUES ('paid-client', ?1, 'provider-stable', 'provider-old@example.com',
                         'client-stable', 'client-old@example.com', 'active', 500,
                         30, 1, ?2, ?3, ?4, ?4)",
                params![
                    host.id,
                    (now - chrono::Duration::days(28)).to_rfc3339(),
                    frozen_end.to_rfc3339(),
                    now.to_rfc3339(),
                ],
            )
            .unwrap();
        }

        let provider = market_session("provider-stable", "provider-new@example.com");
        assert!(matches!(
            store
                .client_market_update_host_offer(&host.id, &provider, Some(900), Some(60))
                .await,
            Err(AppError::BadRequest(message))
                if message.contains("configure payment details on the Account page")
        ));

        let client = market_session("client-stable", "client-new@example.com");
        let billing_without_profile = store
            .client_market_billing_for_viewer("paid-client", &client)
            .await
            .expect("load billing before Provider configures payment details");
        assert!(billing_without_profile.payment_profile_updated_at.is_none());
        let payment_profile = store
            .client_market_update_payment_profile(
                &provider,
                &[PaymentMethod {
                    kind: "crypto".into(),
                    account: None,
                    qr_image_url: None,
                    asset_url: None,
                    token: Some("USDC".into()),
                    chain: Some("base".into()),
                    address: Some("0x0123456789abcdef0123456789abcdef01234567".into()),
                    instructions: None,
                }],
            )
            .await
            .expect("configure Provider payment details");
        let updated_offer = store
            .client_market_update_host_offer(&host.id, &provider, Some(900), Some(60))
            .await
            .expect("update Host offer with stable Provider identity");
        assert_eq!(updated_offer.offer_revision, 2);
        let unchanged_offer = store
            .client_market_update_host_offer(&host.id, &provider, Some(900), Some(60))
            .await
            .expect("save unchanged Host offer");
        assert_eq!(unchanged_offer.offer_revision, 2);
        assert!(matches!(
            store.client_market_assert_creation_allowed(&client).await,
            Err(AppError::Conflict(_))
        ));
        let (status, period_end, invoice_id): (String, String, String) = {
            let conn = store.conn.lock().await;
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM client_market_email_deliveries
                     WHERE kind = 'host_offer_changed'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                1
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM client_market_offer_events
                     WHERE host_id = ?1 AND offer_revision = 2",
                    params![host.id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                1
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM client_market_email_deliveries d
                     JOIN client_market_email_events e ON e.id = d.event_id
                     WHERE d.kind = 'host_offer_changed'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                1
            );
            conn.query_row(
                "SELECT s.status, s.current_period_end, i.id
                 FROM client_market_subscriptions s
                 JOIN client_market_invoices i ON i.installation_id = s.installation_id
                 WHERE s.installation_id = 'paid-client' AND i.status = 'open'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap()
        };
        assert_eq!(status, "payment_due");
        assert_eq!(
            parse_market_time(&period_end),
            parse_market_time(&frozen_end.to_rfc3339())
        );
        let billing_with_profile = store
            .client_market_billing_for_viewer("paid-client", &client)
            .await
            .expect("reload billing after payment details change");
        {
            let conn = store.conn.lock().await;
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM account_payment_methods
                     WHERE profile_user_id = 'provider-stable' AND enabled = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                1
            );
        }
        assert_eq!(
            billing_with_profile.payment_profile_updated_at.as_deref(),
            Some(payment_profile.updated_at.as_str())
        );
        assert!(matches!(
            store
                .client_market_declare_paid("paid-client", &invoice_id, 2, None, None, &client)
                .await,
            Err(AppError::Conflict(_))
        ));
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE client_market_invoices SET deadline_at = ?2 WHERE id = ?1",
                params![
                    invoice_id,
                    (Utc::now() - chrono::Duration::seconds(1)).to_rfc3339()
                ],
            )
            .unwrap();
        }
        assert!(matches!(
            store
                .client_market_declare_paid(
                    "paid-client",
                    &invoice_id,
                    1,
                    Some(&payment_profile.updated_at),
                    None,
                    &client,
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        assert!(matches!(
            store
                .client_market_declare_paid(
                    "paid-client",
                    &invoice_id,
                    2,
                    Some(&payment_profile.updated_at),
                    None,
                    &client,
                )
                .await,
            Err(AppError::Gone(_))
        ));
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE client_market_invoices SET deadline_at = ?2 WHERE id = ?1",
                params![
                    invoice_id,
                    (Utc::now() + chrono::Duration::days(1)).to_rfc3339()
                ],
            )
            .unwrap();
        }
        // An echoed amount that disagrees with the invoice must be rejected even when
        // the revision matches — the client displayed a stale number.
        assert!(
            matches!(
                store
                    .client_market_declare_paid(
                        "paid-client",
                        &invoice_id,
                        2,
                        Some(&payment_profile.updated_at),
                        Some(1),
                        &client,
                    )
                    .await,
                Err(AppError::Conflict(_))
            ),
            "mismatched amount_cents_confirmed must be refused"
        );
        store
            .client_market_declare_paid(
                "paid-client",
                &invoice_id,
                2,
                Some(&payment_profile.updated_at),
                None,
                &client,
            )
            .await
            .expect("declare payment");
        store
            .client_market_declare_paid(
                "paid-client",
                &invoice_id,
                2,
                Some(&payment_profile.updated_at),
                None,
                &client,
            )
            .await
            .expect("repeat payment declaration");
        let (next_end, declarations, owner_email, declaration_events): (String, i64, String, i64) = {
            let conn = store.conn.lock().await;
            let subscription = conn
                .query_row(
                    "SELECT current_period_end, client_owner_email
                     FROM client_market_subscriptions WHERE installation_id = 'paid-client'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .unwrap();
            let count = conn
                .query_row(
                    "SELECT COUNT(*) FROM client_market_payment_declarations WHERE invoice_id = ?1",
                    params![invoice_id],
                    |row| row.get(0),
                )
                .unwrap();
            let event_count = conn
                .query_row(
                    "SELECT COUNT(*) FROM client_market_subscription_events
                     WHERE installation_id = 'paid-client' AND event_type = 'payment_declared'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            (subscription.0, count, subscription.1, event_count)
        };
        assert_eq!(
            parse_market_time(&next_end),
            parse_market_time(&frozen_end.to_rfc3339()) + chrono::Duration::days(60)
        );
        assert_eq!(declarations, 1);
        assert_eq!(declaration_events, 1);
        assert_eq!(owner_email, "client-new@example.com");
        let billing = store
            .client_market_billing_for_viewer("paid-client", &client)
            .await
            .expect("load billing after email change");
        assert!(billing.is_client_owner);
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn provider_supply_groups_by_stable_identity_and_uses_current_email() {
        let (store, _config, root) = test_store("provider-supply-stable-identity");
        let first = add_provider_host(
            &store,
            "provider-stable",
            "provider-old@example.com",
            "198.18.24.1",
            "US",
            None,
            None,
        )
        .await;
        let second = add_provider_host(
            &store,
            "provider-stable",
            "provider-old@example.com",
            "198.18.24.2",
            "CA",
            None,
            None,
        )
        .await;
        store
            .client_market_ensure_provider("provider-stable", "provider-new@example.com")
            .await
            .expect("sync Provider profile");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE router_ssh_hosts
                 SET host_owner_email = 'provider-stale@example.com', last_error = 'observed error'
                 WHERE id = ?1",
                params![first.id],
            )
            .unwrap();
            conn.execute(
                "UPDATE router_ssh_hosts
                 SET price_cents = 500, rental_period_days = 30, status = 'allocated'
                 WHERE id = ?1",
                params![second.id],
            )
            .unwrap();
            let now = Utc::now();
            for (installation_id, host_id, client_user_id, created_at) in [
                (
                    "external-old",
                    "observation-host-old",
                    "external-old-user",
                    now - chrono::Duration::days(31),
                ),
                (
                    "external-trial",
                    "observation-host-trial",
                    "external-trial-user",
                    now - chrono::Duration::days(1),
                ),
                (
                    "provider-self",
                    "observation-host-self",
                    "provider-stable",
                    now - chrono::Duration::days(40),
                ),
            ] {
                conn.execute(
                    "INSERT INTO client_market_subscriptions
                        (installation_id, host_id, provider_id, host_owner_email,
                         client_user_id, client_owner_email, status, price_cents,
                         rental_period_days, offer_revision, last_declared_at,
                         created_at, updated_at)
                     VALUES (?1, ?2, 'provider-stable', 'provider-new@example.com',
                             ?3, 'client@example.com', 'active', 500, 30, 1, ?4, ?5, ?5)",
                    params![
                        installation_id,
                        host_id,
                        client_user_id,
                        now.to_rfc3339(),
                        created_at.to_rfc3339(),
                    ],
                )
                .unwrap();
            }
            conn.execute(
                "INSERT INTO host_provider_daily_stats
                    (provider_id, stat_date, host_total, idle_total, allocated_total,
                     external_client_total, online_samples, observed_samples,
                     anomalous_host_samples, host_samples, updated_at)
                 VALUES ('provider-stable', ?1, 2, 1, 1, 2, 8, 10, 5, 10, ?2)",
                params![now.format("%Y-%m-%d").to_string(), now.to_rfc3339()],
            )
            .unwrap();
        }

        let supply = store
            .client_market_provider_supply(Some("provider-new@example.com"))
            .await
            .expect("load Provider supply");
        assert_eq!(supply.providers.len(), 1);
        assert_eq!(supply.providers[0].provider_id, "provider-stable");
        assert_eq!(supply.providers[0].owner_email, "provider-new@example.com");
        assert_eq!(supply.providers[0].host_total, 2);
        assert_eq!(supply.providers[0].idle_total, 1);
        assert_eq!(supply.providers[0].allocated_total, 1);
        assert_eq!(supply.providers[0].free_host_total, 1);
        assert_eq!(supply.providers[0].paid_host_total, 1);
        assert_eq!(supply.providers[0].paid_allocated_total, 1);
        assert_eq!(supply.providers[0].external_client_owner_total, 2);
        assert_eq!(supply.providers[0].external_clients_over_3_days, 1);
        assert_eq!(supply.providers[0].external_clients_over_30_days, 1);
        assert_eq!(supply.providers[0].online_rate_30d, Some(0.8));
        assert_eq!(supply.providers[0].anomalous_host_rate, 0.5);
        assert!(supply.providers[0].official);
        assert_eq!(
            supply.official_provider_id.as_deref(),
            Some("provider-stable")
        );

        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn official_provider_stays_first_with_zero_host_capacity() {
        let (store, _config, root) = test_store("official-provider-zero-capacity");
        store
            .client_market_ensure_provider("official-provider", "official@example.com")
            .await
            .expect("create official Provider profile");
        add_provider_host(
            &store,
            "other-provider",
            "other@example.com",
            "198.18.24.3",
            "US",
            None,
            None,
        )
        .await;

        let supply = store
            .client_market_provider_supply(Some("official@example.com"))
            .await
            .expect("load Provider supply");
        assert_eq!(
            supply.official_provider_id.as_deref(),
            Some("official-provider")
        );
        assert_eq!(supply.providers[0].provider_id, "official-provider");
        assert!(supply.providers[0].official);
        assert_eq!(supply.providers[0].host_total, 0);
        assert_eq!(supply.providers[0].idle_total, 0);
        assert_eq!(supply.providers[0].free_host_total, 0);

        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn provider_supply_counts_drifted_free_forever_hosts_as_idle() {
        let (store, _config, root) = test_store("provider-supply-drifted-free-idle");
        store
            .client_market_ensure_provider("provider-stable", "provider@example.com")
            .await
            .expect("create Provider profile");
        let paid = add_provider_host(
            &store,
            "provider-stable",
            "provider@example.com",
            "198.18.25.1",
            "US",
            Some(500),
            Some(30),
        )
        .await;
        let free = add_provider_host(
            &store,
            "provider-stable",
            "provider@example.com",
            "198.18.25.2",
            "JP",
            None,
            None,
        )
        .await;
        {
            let conn = store.conn.lock().await;
            // Simulate legacy/drifted provider_id that never got healed by a paid offer edit.
            conn.execute(
                "UPDATE router_ssh_hosts
                 SET provider_id = 'email:provider@example.com', price_cents = NULL, rental_period_days = NULL
                 WHERE id = ?1",
                params![free.id],
            )
            .unwrap();
            conn.execute(
                "UPDATE router_ssh_hosts SET status = 'idle' WHERE id IN (?1, ?2)",
                params![paid.id, free.id],
            )
            .unwrap();
        }

        let supply = store
            .client_market_provider_supply(Some("provider@example.com"))
            .await
            .expect("load Provider supply");
        let provider = supply
            .providers
            .iter()
            .find(|item| item.provider_id == "provider-stable")
            .expect("stable provider");
        assert_eq!(provider.host_total, 2);
        assert_eq!(provider.idle_total, 2);
        assert_eq!(provider.free_host_total, 1);
        assert_eq!(provider.paid_host_total, 1);
        let japan = provider
            .countries
            .iter()
            .find(|country| country.code == "JP")
            .expect("Japan capacity from free host");
        assert_eq!(japan.idle, 1);
        assert_eq!(japan.total, 1);

        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn provider_supply_counts_null_provider_id_free_forever_hosts_as_idle() {
        let (store, _config, root) = test_store("provider-supply-null-provider-free-idle");
        store
            .client_market_ensure_provider("provider-stable", "provider@example.com")
            .await
            .expect("create Provider profile");
        let free = add_provider_host(
            &store,
            "provider-stable",
            "provider@example.com",
            "198.18.25.3",
            "US",
            None,
            None,
        )
        .await;
        {
            let conn = store.conn.lock().await;
            // Live us01 shape: provider_id was added nullable and never backfilled.
            // SQL `NULL != canonical_id` is unknown, so the old heal skipped these rows.
            conn.execute(
                "UPDATE router_ssh_hosts
                 SET provider_id = NULL, price_cents = NULL, rental_period_days = NULL, status = 'idle'
                 WHERE id = ?1",
                params![free.id],
            )
            .unwrap();
            let still_null: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM router_ssh_hosts WHERE id = ?1 AND provider_id IS NULL",
                    params![free.id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(still_null, 1);
        }

        let supply = store
            .client_market_provider_supply(Some("provider@example.com"))
            .await
            .expect("load Provider supply");
        let provider = supply
            .providers
            .iter()
            .find(|item| item.provider_id == "provider-stable")
            .expect("stable provider");
        assert_eq!(provider.host_total, 1);
        assert_eq!(provider.idle_total, 1);
        assert_eq!(provider.free_host_total, 1);
        let us = provider
            .countries
            .iter()
            .find(|country| country.code == "US")
            .expect("US capacity from null-provider free host");
        assert_eq!(us.idle, 1);
        assert_eq!(us.total, 1);

        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn billing_reconcile_opens_one_renewal_and_reports_overdue_client() {
        let (store, _config, root) = test_store("trade-renewal");
        let host = add_provider_host(
            &store,
            "provider-renewal",
            "provider@example.com",
            "198.18.22.1",
            "US",
            Some(700),
            Some(30),
        )
        .await;
        let now = Utc::now();
        let period_end = now + chrono::Duration::days(2);
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "INSERT INTO client_market_subscriptions
                    (installation_id, host_id, provider_id, host_owner_email,
                     client_user_id, client_owner_email, status, price_cents,
                     rental_period_days, offer_revision, current_period_end,
                     created_at, updated_at)
                 VALUES ('renewal-client', ?1, 'provider-renewal', 'provider@example.com',
                         'renewal-user', 'renewal@example.com', 'active', 700,
                         30, 1, ?2, ?3, ?3)",
                params![host.id, period_end.to_rfc3339(), now.to_rfc3339()],
            )
            .unwrap();
        }
        assert!(
            store
                .client_market_reconcile_trade_state(now)
                .await
                .expect("open renewal")
                .is_empty()
        );
        assert!(
            store
                .client_market_reconcile_trade_state(now + chrono::Duration::minutes(1))
                .await
                .expect("repeat renewal reconciliation")
                .is_empty()
        );
        {
            let conn = store.conn.lock().await;
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM client_market_invoices
                     WHERE installation_id = 'renewal-client' AND status = 'open'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1);
        }
        let expired = store
            .client_market_reconcile_trade_state(period_end + chrono::Duration::seconds(1))
            .await
            .expect("report overdue Client");
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].installation_id, "renewal-client");
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn automatic_release_rechecks_payment_state_inside_cleanup_transaction() {
        let (store, _config, root) = test_store("trade-release-payment-race");
        let host = add_provider_host(
            &store,
            "provider-race",
            "provider@example.com",
            "198.18.22.2",
            "US",
            Some(700),
            Some(30),
        )
        .await;
        let now = Utc::now();
        {
            let conn = store.conn.lock().await;
            insert_installation(&conn, "race-client", "renter@example.com");
            conn.execute(
                "UPDATE installations SET provision_source = ?2, provision_host_id = ?3 WHERE id = ?1",
                params!["race-client", PROVISION_SOURCE_ROUTER_MARKET, host.id],
            )
            .unwrap();
            insert_tunnel_and_public_host(
                &conn,
                "race-client",
                "renter@example.com",
                "race-client",
            );
            conn.execute(
                "UPDATE router_ssh_hosts SET status = 'allocated', installation_id = 'race-client'
                 WHERE id = ?1",
                params![host.id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO client_market_subscriptions
                    (installation_id, host_id, provider_id, host_owner_email,
                     client_user_id, client_owner_email, status, price_cents,
                     rental_period_days, offer_revision, current_period_end,
                     payment_deadline, created_at, updated_at)
                 VALUES ('race-client', ?1, 'provider-race', 'provider@example.com',
                         'renter-race', 'renter@example.com', 'active', 700,
                         30, 1, ?2, NULL, ?3, ?3)",
                params![
                    host.id,
                    (now + chrono::Duration::days(30)).to_rfc3339(),
                    now.to_rfc3339(),
                ],
            )
            .unwrap();
        }

        assert!(matches!(
            store
                .client_market_begin_system_cleanup_job("race-client", "payment_deadline_expired")
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
            HOST_STATUS_ALLOCATED
        );

        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE client_market_subscriptions
                 SET status = 'payment_due', payment_deadline = ?2 WHERE installation_id = ?1",
                params![
                    "race-client",
                    (Utc::now() - chrono::Duration::seconds(1)).to_rfc3339()
                ],
            )
            .unwrap();
        }
        store
            .client_market_begin_system_cleanup_job("race-client", "payment_deadline_expired")
            .await
            .expect("start automatic cleanup while payment is still overdue");

        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn provider_cleanup_and_blocking_use_stable_identity() {
        let (store, _config, root) = test_store("trade-provider-block");
        let host = add_provider_host(
            &store,
            "provider-block",
            "provider-old@example.com",
            "198.18.23.1",
            "US",
            Some(500),
            Some(30),
        )
        .await;
        let now = Utc::now().to_rfc3339();
        {
            let conn = store.conn.lock().await;
            insert_installation(&conn, "blocked-client", "client-old@example.com");
            conn.execute(
                "UPDATE installations SET provision_source = ?2, provision_host_id = ?3 WHERE id = ?1",
                params!["blocked-client", PROVISION_SOURCE_ROUTER_MARKET, host.id],
            )
            .unwrap();
            insert_tunnel_and_public_host(
                &conn,
                "blocked-client",
                "client-old@example.com",
                "blocked-client",
            );
            conn.execute(
                "UPDATE router_ssh_hosts SET status = 'allocated', installation_id = 'blocked-client'
                 WHERE id = ?1",
                params![host.id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO client_market_subscriptions
                    (installation_id, host_id, provider_id, host_owner_email,
                     client_user_id, client_owner_email, status, price_cents,
                     rental_period_days, offer_revision, payment_deadline,
                     created_at, updated_at)
                 VALUES ('blocked-client', ?1, 'provider-block', 'provider-old@example.com',
                         'client-block', 'client-old@example.com', 'payment_due', 500,
                         30, 1, ?2, ?3, ?3)",
                params![
                    host.id,
                    (Utc::now() + chrono::Duration::days(3)).to_rfc3339(),
                    now
                ],
            )
            .unwrap();
        }
        store
            .client_market_begin_cleanup_job_with_context(
                "blocked-client",
                Some("provider-block"),
                "provider-new@example.com",
                false,
                "payment_not_received",
                Some(true),
            )
            .await
            .expect("Provider cleanup after email change");
        let blocks = store
            .client_market_provider_blocks("provider-block")
            .await
            .unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].client_user_id, "client-block");
        {
            let conn = store.conn.lock().await;
            let mut statement = conn
                .prepare(
                    "SELECT recipient FROM client_market_email_deliveries
                     WHERE idempotency_key LIKE 'cleanup-started:blocked-client:%'
                     ORDER BY recipient",
                )
                .unwrap();
            let recipients = statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(
                recipients,
                vec![
                    "client-old@example.com".to_string(),
                    "provider-new@example.com".to_string(),
                ]
            );
        }
        let provider = market_session("provider-block", "provider-new@example.com");
        store
            .client_market_lift_provider_block(&provider, "client-block")
            .await
            .expect("lift Provider block");
        assert!(
            store
                .client_market_provider_blocks("provider-block")
                .await
                .unwrap()
                .is_empty()
        );
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn provider_release_and_self_rental_never_create_payment_blocks() {
        let (store, _config, root) = test_store("trade-provider-release-no-block");
        let provider_id = "provider-release";
        let provider_email = "provider@example.com";
        let first = add_provider_host(
            &store,
            provider_id,
            provider_email,
            "198.18.25.1",
            "US",
            Some(500),
            Some(30),
        )
        .await;
        let second = add_provider_host(
            &store,
            provider_id,
            provider_email,
            "198.18.25.2",
            "US",
            Some(500),
            Some(30),
        )
        .await;
        let now = Utc::now().to_rfc3339();
        {
            let conn = store.conn.lock().await;
            for (installation_id, host_id, client_user_id, client_email) in [
                (
                    "provider-release-client",
                    first.id.as_str(),
                    "renter",
                    "renter@example.com",
                ),
                (
                    "provider-self-client",
                    second.id.as_str(),
                    provider_id,
                    provider_email,
                ),
            ] {
                insert_installation(&conn, installation_id, client_email);
                conn.execute(
                    "UPDATE installations SET provision_source = ?2, provision_host_id = ?3 WHERE id = ?1",
                    params![installation_id, PROVISION_SOURCE_ROUTER_MARKET, host_id],
                )
                .unwrap();
                insert_tunnel_and_public_host(
                    &conn,
                    installation_id,
                    client_email,
                    installation_id,
                );
                conn.execute(
                    "UPDATE router_ssh_hosts SET status = 'allocated', installation_id = ?2 WHERE id = ?1",
                    params![host_id, installation_id],
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO client_market_subscriptions
                        (installation_id, host_id, provider_id, host_owner_email,
                         client_user_id, client_owner_email, status, price_cents,
                         rental_period_days, offer_revision, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', 500, 30, 1, ?7, ?7)",
                    params![
                        installation_id,
                        host_id,
                        provider_id,
                        provider_email,
                        client_user_id,
                        client_email,
                        now,
                    ],
                )
                .unwrap();
            }
        }

        store
            .client_market_begin_cleanup_job_with_context(
                "provider-release-client",
                Some(provider_id),
                provider_email,
                false,
                "provider_release",
                Some(true),
            )
            .await
            .expect("ordinary Provider release");
        store
            .client_market_begin_cleanup_job_with_context(
                "provider-self-client",
                Some(provider_id),
                provider_email,
                false,
                "payment_not_received",
                Some(true),
            )
            .await
            .expect("self-rental release");
        assert!(
            store
                .client_market_provider_blocks(provider_id)
                .await
                .unwrap()
                .is_empty()
        );

        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn disabled_email_delivery_does_not_claim_or_consume_attempts() {
        let (store, _config, root) = test_store("trade-email-disabled");
        let now = Utc::now();
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "INSERT INTO client_market_email_deliveries
                    (id, kind, recipient, subject, html_body, text_body, idempotency_key,
                     status, attempts, next_attempt_at, created_at, updated_at)
                 VALUES ('pending-email', 'test', 'owner@example.com', 'subject', '<p>body</p>',
                         'body', 'test:pending-email', 'pending', 0, ?1, ?1, ?1)",
                params![now.to_rfc3339()],
            )
            .unwrap();
        }

        assert!(
            store
                .client_market_claim_email("test-worker", now, false)
                .await
                .unwrap()
                .is_none()
        );
        {
            let conn = store.conn.lock().await;
            let state: (String, i64) = conn
                .query_row(
                    "SELECT status, attempts FROM client_market_email_deliveries
                     WHERE id = 'pending-email'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(state, ("pending".into(), 0));
        }
        assert!(
            store
                .client_market_claim_email("test-worker", now, true)
                .await
                .unwrap()
                .is_some()
        );

        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    fn parse_market_time(value: &str) -> chrono::DateTime<Utc> {
        chrono::DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
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

    #[test]
    fn validate_root_password_rejects_empty_control_and_oversized() {
        assert!(validate_root_password("ok").is_ok());
        assert!(validate_root_password("").is_err());
        assert!(validate_root_password("bad\npass").is_err());
        assert!(validate_root_password(&"x".repeat(MAX_PASSWORD_BYTES + 1)).is_err());
        assert!(validate_root_password(&"x".repeat(MAX_PASSWORD_BYTES)).is_ok());
    }

    #[test]
    fn install_provision_key_remote_command_is_idempotent_and_quotes_line() {
        let command = install_provision_key_remote_command(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAItest cc-switch-router-provision",
        );
        assert!(command.contains("mkdir -p \"$HOME/.ssh\""));
        assert!(command.contains("authorized_keys"));
        assert!(command.contains("grep -qxF"));
        assert!(
            command
                .contains("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAItest cc-switch-router-provision")
        );
        assert!(command.contains("printf 'ok\\n'"));
        let with_quote = install_provision_key_remote_command("line'with'quote");
        assert!(with_quote.contains("'\"'\"'"));
    }

    #[test]
    fn create_host_request_deserializes_optional_root_password() {
        let with_password: CreateHostRequest = serde_json::from_str(
            r#"{"ip":"8.8.8.8","port":22,"note":"n","rootPassword":"secret"}"#,
        )
        .unwrap();
        assert_eq!(with_password.root_password.as_deref(), Some("secret"));
        let without: CreateHostRequest = serde_json::from_str(r#"{"ip":"8.8.8.8"}"#).unwrap();
        assert!(without.root_password.is_none());
        let test_req: TestHostSshRequest =
            serde_json::from_str(r#"{"ip":"8.8.8.8","rootPassword":"pw"}"#).unwrap();
        assert_eq!(test_req.root_password.as_deref(), Some("pw"));
    }

    #[test]
    fn classify_cleanup_failure_maps_known_cases() {
        assert_eq!(
            classify_cleanup_failure(&AppError::ServiceUnavailable(
                "ssh command exceeded its execution timeout".into()
            )),
            CLEANUP_FAILURE_SSH_TIMEOUT
        );
        assert_eq!(
            classify_cleanup_failure(&AppError::Conflict(
                "failed to stop cc-switch-server".into()
            )),
            CLEANUP_FAILURE_STOP
        );
        assert_eq!(
            classify_cleanup_failure(&AppError::Conflict(
                "failed to remove cc-switch-server installation files".into()
            )),
            CLEANUP_FAILURE_WIPE
        );
        assert_eq!(
            classify_cleanup_failure(&AppError::Internal(
                "purge installation failed after 3 attempts: db locked".into()
            )),
            CLEANUP_FAILURE_PURGE
        );
        assert_eq!(
            classify_cleanup_failure(&AppError::Conflict(
                "cleanup host installation binding mismatch".into()
            )),
            CLEANUP_FAILURE_BINDING
        );
        assert_eq!(
            classify_cleanup_failure(&AppError::Conflict(
                "ssh host key fingerprint does not match the registered host".into()
            )),
            CLEANUP_FAILURE_FINGERPRINT
        );
    }

    /// `release_failed` had no exit: nothing transitioned a subscription out of it
    /// while `ensure_creation_allowed_tx` treated it as a hard block, so one failed
    /// remote cleanup locked the renter out of creating Clients permanently.
    #[tokio::test]
    async fn force_release_clears_the_release_failed_deadlock() {
        let (store, _config, root) = test_store("trade-force-release");
        let host = add_provider_host(
            &store,
            "provider-stuck",
            "provider@example.com",
            "198.18.30.1",
            "US",
            Some(500),
            Some(30),
        )
        .await;
        let now = Utc::now();
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "INSERT INTO client_market_subscriptions
                    (installation_id, host_id, provider_id, host_owner_email,
                     client_user_id, client_owner_email, status, price_cents,
                     rental_period_days, offer_revision, current_period_end,
                     created_at, updated_at)
                 VALUES ('stuck-client', ?1, 'provider-stuck', 'provider@example.com',
                         'client-stuck', 'client@example.com', 'release_failed', 500,
                         30, 1, ?2, ?3, ?3)",
                params![
                    host.id,
                    (now - chrono::Duration::days(1)).to_rfc3339(),
                    now.to_rfc3339(),
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO client_market_invoices
                    (id, installation_id, sequence, amount_cents, rental_period_days,
                     offer_revision, status, deadline_at, opened_at)
                 VALUES ('stuck-invoice', 'stuck-client', 1, 500, 30, 1, 'open', ?1, ?2)",
                params![
                    (now + chrono::Duration::days(1)).to_rfc3339(),
                    now.to_rfc3339()
                ],
            )
            .unwrap();
        }

        let client = market_session("client-stuck", "client@example.com");
        assert!(
            store.client_market_assert_creation_allowed(&client).await.is_err(),
            "release_failed must block creation before the force release"
        );

        let outcome = store
            .client_market_force_release_subscription(
                "stuck-client",
                "admin-user",
                "admin@example.com",
            )
            .await
            .expect("force release a wedged subscription");
        assert_eq!(outcome.previous_status, "release_failed");
        assert_eq!(outcome.status, "released");
        assert_eq!(outcome.canceled_invoices, 1);

        store
            .client_market_assert_creation_allowed(&client)
            .await
            .expect("creation gate must clear after the force release");

        // Second call is a conflict, not a silent double-release.
        assert!(matches!(
            store
                .client_market_force_release_subscription(
                    "stuck-client",
                    "admin-user",
                    "admin@example.com"
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    /// A Provider must not be able to rent their own Host: it would inflate their
    /// public allocation stats. Ownership is matched by provider_id and by owner
    /// email, because provider_id can drift from the registering account.
    #[tokio::test]
    async fn self_dealing_quotes_are_refused() {
        use crate::client_market_trade::CreateQuoteRequest;

        let (store, _config, root) = test_store("trade-self-deal");
        let host = add_provider_host(
            &store,
            "provider-self",
            "self@example.com",
            "198.18.31.1",
            "US",
            Some(500),
            Some(30),
        )
        .await;

        let owner = market_session("provider-self", "self@example.com");
        assert!(
            matches!(
                store
                    .client_market_create_quote(
                        &owner,
                        CreateQuoteRequest {
                            provider_ids: vec!["provider-self".into()],
                            country_codes: vec!["US".into()],
                            count: 1,
                            host_id: Some(host.id.clone()),
                        }
                    )
                    .await,
                Err(AppError::Conflict(_))
            ),
            "a Provider must not quote their own Host by id"
        );
        assert!(
            store
                .client_market_create_quote(
                    &owner,
                    CreateQuoteRequest {
                        provider_ids: vec!["provider-self".into()],
                        country_codes: vec!["US".into()],
                        count: 1,
                        host_id: None,
                    }
                )
                .await
                .is_err(),
            "random selection must not pick the caller's own Host"
        );

        // A different account can still rent it, so the guard is not over-broad.
        let renter = market_session("client-other", "other@example.com");
        store
            .client_market_create_quote(
                &renter,
                CreateQuoteRequest {
                    provider_ids: vec!["provider-self".into()],
                    country_codes: vec!["US".into()],
                    count: 1,
                    host_id: Some(host.id.clone()),
                },
            )
            .await
            .expect("a third-party renter must still be able to quote the Host");
        let _ = std::fs::remove_dir_all(root);
    }

    /// Quotes expire opportunistically, so a restart with quotes outstanding could
    /// strand Hosts in `reserved` with no live quote to release them.
    #[tokio::test]
    async fn stranded_reserved_host_is_detected_as_quoteless() {
        let (store, _config, root) = test_store("trade-stranded-reserved");
        let host = add_provider_host(
            &store,
            "provider-stranded",
            "provider@example.com",
            "198.18.32.1",
            "US",
            Some(500),
            Some(30),
        )
        .await;
        assert!(
            !store
                .client_market_host_has_live_quote(&host.id)
                .await
                .expect("query live quotes"),
            "a Host with no quote rows must report no live quote"
        );

        store
            .client_market_force_host_status(
                &host.id,
                HOST_STATUS_IDLE,
                crate::client_market_trade::HOST_STATUS_RESERVED,
                "",
            )
            .await
            .expect("move host to reserved");

        // CAS must refuse when the observed status is stale.
        assert!(
            !store
                .client_market_force_host_status(
                    &host.id,
                    HOST_STATUS_IDLE,
                    HOST_STATUS_UNREACHABLE,
                    "",
                )
                .await
                .expect("cas call must not error"),
            "compare-and-set must refuse when the host already moved"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
