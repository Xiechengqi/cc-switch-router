use std::collections::{HashMap, HashSet};
use std::fs;
use std::future::Future;
use std::io::Write as _;
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::db::{Connection, OptionalExtension, TransactionBehavior, params};
use anyhow::{Context as _, bail};
use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path as AxumPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::{delete, get, post};
use base64::Engine;
use chrono::{DateTime, Utc};
use futures_util::{StreamExt, stream};
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
const CLEANUP_FAILURE_SSH_UNREACHABLE: &str = "cleanup_ssh_unreachable";
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
/// Delay before each unattended clean-host probe. Five failed probes are terminal;
/// Providers retain the explicit retry, reverify, and permanent-retirement actions.
const CLEANUP_RECOVERY_BACKOFF: [Duration; 5] = [
    Duration::from_secs(5 * 60),
    Duration::from_secs(15 * 60),
    Duration::from_secs(60 * 60),
    Duration::from_secs(6 * 60 * 60),
    Duration::from_secs(24 * 60 * 60),
];
const CLEANUP_RECOVERY_CLAIM_LIMIT: usize = 4;
const CLEANUP_RECOVERY_CLAIM_LEASE: Duration = Duration::from_secs(10 * 60);
const HOST_REPROBE_CLAIM_LIMIT: usize = 4;
const HOST_REPROBE_CLAIM_LEASE: Duration = Duration::from_secs(5 * 60);
const HOST_REPROBE_BACKOFF: [Duration; 4] = [
    Duration::from_secs(15 * 60),
    Duration::from_secs(60 * 60),
    Duration::from_secs(6 * 60 * 60),
    Duration::from_secs(24 * 60 * 60),
];
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
const SSH_HOST_KEY_SCAN_TIMEOUT: Duration = Duration::from_secs(12);
const SSH_HOST_KEY_SCAN_CONNECT_TIMEOUT_SECS: &str = "5";
const SSH_INSTALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const SSH_CLEANUP_TIMEOUT: Duration = Duration::from_secs(90);
const JOB_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const JOB_HEARTBEAT_STALE_AFTER: Duration = Duration::from_secs(90);
const JOB_WATCHDOG_STALE_AFTER: Duration = Duration::from_secs(15 * 60);
const CREATE_JOB_MAX_RUNTIME: Duration = Duration::from_secs(30 * 60);
const CLEANUP_JOB_MAX_RUNTIME: Duration = Duration::from_secs(10 * 60);
const JOB_LEASE_REVOKED_MESSAGE: &str = "job execution lease was revoked";
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
/// Match process argv0 (e.g. `/usr/local/bin/cc-switch-server`) or truncated Linux
/// `/proc/*/comm` (`cc-switch-serve`, 15-char limit) — never a substring of the remote
/// `sh -c '...'` helper script which also contains that path as text.
const REMOTE_CC_SWITCH_SERVER_HELPERS: &str = r#"
cc_switch_server_list_pids() {
  for dir in /proc/[0-9]*; do
    [ -d "$dir" ] || continue
    pid=${dir#/proc/}
    case "$pid" in
      ''|*[!0-9]*) continue ;;
    esac
    [ "$pid" -eq "$$" ] 2>/dev/null && continue
    cmdline="$dir/cmdline"
    [ -r "$cmdline" ] || continue
    argv0=$(tr '\0' '\n' < "$cmdline" 2>/dev/null | head -n 1)
    case "$argv0" in
      cc-switch-server|*/cc-switch-server)
        printf '%s\n' "$pid"
        continue
        ;;
    esac
    if [ -r "$dir/comm" ]; then
      name=$(cat "$dir/comm" 2>/dev/null) || continue
      # Linux TASK_COMM_LEN is 15; "cc-switch-server" truncates to "cc-switch-serve".
      case "$name" in
        cc-switch-server|cc-switch-serve)
          printf '%s\n' "$pid"
          ;;
      esac
    fi
  done
}
cc_switch_server_pkill() {
  pids=$(cc_switch_server_list_pids | sort -u)
  if [ -z "$pids" ]; then
    return 0
  fi
  for pid in $pids; do
    kill "$pid" 2>/dev/null || true
  done
  sleep 1
  pids=$(cc_switch_server_list_pids | sort -u)
  for pid in $pids; do
    kill -9 "$pid" 2>/dev/null || true
  done
}
cc_switch_server_is_running() {
  pids=$(cc_switch_server_list_pids)
  [ -n "$pids" ]
}
cc_switch_server_stop_supervisor() {
  if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
    systemctl disable --now cc-switch-server.service >/dev/null 2>&1 || true
  fi
  if command -v rc-service >/dev/null 2>&1; then
    rc-service cc-switch-server stop >/dev/null 2>&1 || true
  fi
  if command -v rc-update >/dev/null 2>&1; then
    rc-update del cc-switch-server default >/dev/null 2>&1 || true
  fi
}
cc_switch_server_stop() {
  cc_switch_server_stop_supervisor
  if ! cc_switch_server_is_running; then
    return 0
  fi
  cc_switch_server_pkill
  attempt=0
  while [ "$attempt" -lt 5 ]; do
    sleep 2
    if ! cc_switch_server_is_running; then
      return 0
    fi
    cc_switch_server_pkill
    attempt=$((attempt + 1))
  done
  echo "cc-switch-server still running after stop attempts: $(cc_switch_server_list_pids | tr '\n' ' ')" >&2
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
  cc_switch_server_stop_supervisor
  rm -f /etc/systemd/system/cc-switch-server.service
  rm -f /etc/init.d/cc-switch-server
  if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
    systemctl daemon-reload >/dev/null 2>&1 || true
    systemctl reset-failed cc-switch-server.service >/dev/null 2>&1 || true
  fi
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
    config.data_dir.join("client_market_ssh_known_hosts")
}

pub fn router_public_url(config: &Config) -> String {
    let scheme = if config.use_localhost {
        "http"
    } else {
        "https"
    };
    format!("{scheme}://{}", config.tunnel_domain.trim_end_matches('/'))
}

pub(crate) fn client_public_url(config: &Config, subdomain: &str) -> String {
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
        .route(
            "/v1/client-market/hosts/import/:id",
            get(get_host_import_job),
        )
        .route("/v1/client-market/hosts/test-ssh", post(test_host_ssh))
        .route("/v1/client-market/hosts/ip-info", post(lookup_host_ip_info))
        .route("/v1/client-market/supply-summary", get(supply_summary))
        .route("/v1/client-market/hosts/:id", delete(delete_host))
        .route(
            "/v1/client-market/hosts/:id/retire-unreachable",
            post(retire_unreachable_host),
        )
        .route("/v1/client-market/hosts/:id/reverify", post(reverify_host))
        .route(
            "/v1/client-market/hosts/:id/ssh-host-key/scan",
            post(scan_host_ssh_key),
        )
        .route(
            "/v1/client-market/hosts/:id/ssh-host-key/rotate",
            post(rotate_host_ssh_key),
        )
        .route(
            "/v1/client-market/hosts/:id/recovery/pause",
            post(pause_host_recovery),
        )
        .route(
            "/v1/client-market/hosts/:id/recovery/resume",
            post(resume_host_recovery),
        )
        .route(
            "/v1/client-market/hosts/:id/recovery/retry",
            post(retry_host_recovery),
        )
        .route("/v1/client-market/jobs/:id", get(get_job))
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
    daily_rate_minor: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    free_duration_days: Option<u32>,
    offer_revision: i64,
    #[serde(default)]
    payment_method_kinds: Vec<String>,
    #[serde(default)]
    contacts: Vec<crate::client_market_trade::PaymentContact>,
    #[serde(default)]
    seller_approval_required: bool,
    eligibility: crate::market_access::MarketEligibilityView,
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
    #[serde(default)]
    can_control_recovery: bool,
    #[serde(default)]
    can_retire_unreachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery: Option<crate::client_market_recovery::ClientMarketRecoveryView>,
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

fn host_seller_approval_required(
    has_viewer: bool,
    is_host_owner: bool,
    is_idle: bool,
    provider_id: Option<&str>,
    daily_rate_minor: Option<i64>,
    access_by_scope: &HashMap<(String, String), bool>,
) -> bool {
    has_viewer
        && !is_host_owner
        && is_idle
        && provider_id.is_some_and(|provider_id| {
            let key = (
                provider_id.to_string(),
                crate::market_access::pricing_kind_for_rate(daily_rate_minor).to_string(),
            );
            access_by_scope.get(&key) == Some(&false)
        })
}

async fn list_hosts(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ListHostsQuery>,
) -> Result<Json<Vec<RouterSshHostView>>, AppError> {
    let viewer = extract_optional_session(&state, &headers).await?;
    let viewer_is_admin = if let Some(session) = viewer.as_ref() {
        state.dynamic.read().await.is_admin(&session.email)
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
    let recovery_host_ids = if state.config.client_market_recovery_enabled {
        viewer
            .as_ref()
            .map(|session| {
                hosts
                    .iter()
                    .filter(|host| {
                        viewer_is_admin
                            || session_is_host_owner(session, host.provider_id.as_deref())
                            || host.client_owner_user_id.as_deref()
                                == Some(session.user_id.as_str())
                    })
                    .map(|host| host.id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let recovery_by_host = state
        .store
        .client_market_recovery_views_for_hosts(&recovery_host_ids)
        .await?;
    let retirable_host_ids = if viewer.is_some() {
        state.store.client_market_retirable_host_ids().await?
    } else {
        HashSet::new()
    };
    let terminal_authorized_host_ids = if let Some(session) = viewer.as_ref() {
        state
            .store
            .client_market_provider_terminal_authorized_host_ids(&session.user_id)
            .await?
    } else {
        HashSet::new()
    };
    let mut access_by_scope = HashMap::new();
    let mut eligibility_by_scope = HashMap::new();
    if let Some(session) = viewer.as_ref() {
        let conn = state.store.conn.lock().await;
        for host in hosts.iter().filter(|host| host.status == HOST_STATUS_IDLE) {
            let Some(provider_id) = host.provider_id.as_deref() else {
                continue;
            };
            let pricing_kind = crate::market_access::pricing_kind_for_rate(host.daily_rate_minor);
            let access_key = (provider_id.to_string(), pricing_kind.to_string());
            let eligibility_key = (
                provider_id.to_string(),
                pricing_kind.to_string(),
                host.currency
                    .clone()
                    .map(|value| value.to_ascii_uppercase()),
            );
            if eligibility_by_scope.contains_key(&eligibility_key) {
                continue;
            }
            let eligibility = crate::market_access::market_eligibility_tx(
                &conn,
                provider_id,
                &session.user_id,
                &session.email,
                crate::market_access::PRODUCT_CLIENT_HOST,
                host.daily_rate_minor,
                host.currency
                    .as_deref()
                    .or_else(|| host.daily_rate_minor.map(|_| "USD")),
            )?;
            access_by_scope
                .entry(access_key)
                .or_insert(eligibility.status != "access_required");
            eligibility_by_scope.insert(eligibility_key, eligibility);
        }
    }
    let views = hosts
        .into_iter()
        .map(|host| {
            // Host operations (port, SSH details, notes, etc.) are host-owner only.
            // Admins / Router owners do not get elevated market host privileges.
            let is_host_owner = viewer
                .as_ref()
                .is_some_and(|session| session_is_host_owner(session, host.provider_id.as_deref()));
            let is_client_owner = viewer.as_ref().is_some_and(|session| {
                host.client_owner_user_id.as_deref() == Some(session.user_id.as_str())
            });
            let reveal_operations = is_host_owner;
            let reveal_installation = reveal_operations || is_client_owner;
            let reveal_recovery_detail = is_host_owner || viewer_is_admin;
            let can_control_recovery =
                state.config.client_market_recovery_enabled && reveal_recovery_detail;
            let can_retire_unreachable = is_host_owner && retirable_host_ids.contains(&host.id);
            let recovery = if reveal_installation || viewer_is_admin {
                recovery_by_host.get(&host.id).cloned().map(|mut recovery| {
                    if !reveal_recovery_detail {
                        recovery.blocked_reason = None;
                    }
                    recovery
                })
            } else {
                None
            };
            // An allocated machine belongs to the renter's security boundary. Its Provider
            // needs an explicit, unexpired renter authorization before opening a root shell.
            let can_web_terminal = is_host_owner
                && (host_is_unallocated_for_terminal(&host)
                    || terminal_authorized_host_ids.contains(&host.id));
            let seller_approval_required = host_seller_approval_required(
                viewer.is_some(),
                is_host_owner,
                host.status == HOST_STATUS_IDLE,
                host.provider_id.as_deref(),
                host.daily_rate_minor,
                &access_by_scope,
            );
            let eligibility = if viewer.is_none() {
                crate::market_access::MarketEligibilityView::login_required()
            } else if is_host_owner || host.status != HOST_STATUS_IDLE {
                crate::market_access::MarketEligibilityView::allowed()
            } else if let Some(provider_id) = host.provider_id.as_deref() {
                eligibility_by_scope
                    .get(&(
                        provider_id.to_string(),
                        crate::market_access::pricing_kind_for_rate(host.daily_rate_minor)
                            .to_string(),
                        host.currency
                            .clone()
                            .map(|value| value.to_ascii_uppercase()),
                    ))
                    .cloned()
                    .unwrap_or_else(crate::market_access::MarketEligibilityView::allowed)
            } else {
                crate::market_access::MarketEligibilityView::allowed()
            };
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
                daily_rate_minor: host.daily_rate_minor,
                currency: host
                    .currency
                    .clone()
                    .or_else(|| host.daily_rate_minor.map(|_| "USD".into())),
                free_duration_days: host.free_duration_days,
                offer_revision: host.offer_revision,
                payment_method_kinds: host.payment_method_kinds,
                contacts: host.contacts,
                seller_approval_required,
                eligibility,
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
                can_control_recovery,
                can_retire_unreachable,
                recovery,
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
    daily_rate_minor: Option<i64>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    free_duration_days: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HostTransferEntry {
    ip: String,
    port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    daily_rate_minor: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    currency: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    free_duration_days: Option<u32>,
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
    status: String,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SshHostKeyInspection {
    host_id: String,
    endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    stored_fingerprint: Option<String>,
    observed_fingerprint: String,
    observed_key_type: String,
    changed: bool,
    confirmation_required: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RotateHostSshKeyRequest {
    #[serde(default)]
    expected_current_fingerprint: Option<String>,
    confirmed_fingerprint: String,
    #[serde(default)]
    verified_from_host_console: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RotateHostSshKeyResponse {
    host: RouterSshHostView,
    inspection: SshHostKeyInspection,
}

#[derive(Debug, Clone)]
struct ObservedSshHostKey {
    target: String,
    key_type: String,
    encoded_key: String,
    fingerprint: String,
}

struct HostFingerprintRotation<'a> {
    expected_fingerprint: Option<&'a str>,
    fingerprint: &'a str,
    key_type: &'a str,
    actor_user_id: &'a str,
    actor_email: &'a str,
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
    let intel =
        crate::ip_iq::lookup_host_ip_intel(&state.config.ip_intel_endpoints, &ip.to_string())
            .await?;
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
    let intel =
        crate::ip_iq::lookup_host_ip_intel(&state.config.ip_intel_endpoints, &ip.to_string())
            .await?;
    let intel_json = serde_json::to_string(&intel)
        .map_err(|e| AppError::Internal(format!("serialize host ip intel failed: {e}")))?;
    let daily_rate_minor = crate::client_market_trade::validate_offer(input.daily_rate_minor)?;
    let currency =
        crate::client_market_trade::normalize_offer_currency(daily_rate_minor, input.currency)?;
    let free_duration_days = crate::client_market_trade::validate_free_duration_days(
        daily_rate_minor,
        input.free_duration_days,
    )?;
    if daily_rate_minor.is_some() {
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
            daily_rate_minor,
            currency.as_deref(),
            free_duration_days,
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
        .filter(|host| session_is_host_owner(&session, host.provider_id.as_deref()))
        .map(|host| HostTransferEntry {
            ip: host.ip,
            port: host.port,
            note: host.note,
            daily_rate_minor: host.daily_rate_minor,
            currency: host.currency,
            free_duration_days: host.free_duration_days,
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
) -> Result<(StatusCode, Json<HostImportResponse>), AppError> {
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
    tokio::spawn(async move {
        if let Err(error) = process_host_import_job(&runner_state, &runner_job_id).await {
            warn!(job_id = %runner_job_id, error = %error, "Host import worker failed");
        }
    });
    let response = state
        .store
        .client_market_host_import_job(&job_id, &session)
        .await?;
    Ok((StatusCode::ACCEPTED, Json(response)))
}

async fn get_host_import_job(
    State(state): State<ServerState>,
    AxumPath(job_id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<HostImportResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .client_market_host_import_job(&job_id, &session)
            .await?,
    ))
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
        let daily_rate_minor = crate::client_market_trade::validate_offer(entry.daily_rate_minor)?;
        let currency =
            crate::client_market_trade::normalize_offer_currency(daily_rate_minor, entry.currency)?;
        let free_duration_days = crate::client_market_trade::validate_free_duration_days(
            daily_rate_minor,
            entry.free_duration_days,
        )?;
        if daily_rate_minor.is_some() {
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
        let intel =
            crate::ip_iq::lookup_host_ip_intel(&state.config.ip_intel_endpoints, &ip.to_string())
                .await?;
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
                daily_rate_minor,
                currency.as_deref(),
                free_duration_days,
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

async fn pause_host_recovery(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<crate::client_market_recovery::ClientMarketRecoveryView>, AppError> {
    control_host_recovery(
        &state,
        &headers,
        &id,
        crate::client_market_recovery::RecoveryControlAction::Pause,
    )
    .await
    .map(Json)
}

async fn resume_host_recovery(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<crate::client_market_recovery::ClientMarketRecoveryView>, AppError> {
    control_host_recovery(
        &state,
        &headers,
        &id,
        crate::client_market_recovery::RecoveryControlAction::Resume,
    )
    .await
    .map(Json)
}

async fn retry_host_recovery(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<crate::client_market_recovery::ClientMarketRecoveryView>, AppError> {
    control_host_recovery(
        &state,
        &headers,
        &id,
        crate::client_market_recovery::RecoveryControlAction::Retry,
    )
    .await
    .map(Json)
}

async fn control_host_recovery(
    state: &ServerState,
    headers: &HeaderMap,
    host_id: &str,
    action: crate::client_market_recovery::RecoveryControlAction,
) -> Result<crate::client_market_recovery::ClientMarketRecoveryView, AppError> {
    let session = require_session(state, headers).await?;
    crate::client_market_recovery::control_recovery(state, &session, host_id, action).await
}

async fn scan_host_ssh_key(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<SshHostKeyInspection>, AppError> {
    let session = require_session(&state, &headers).await?;
    let _known_hosts_guard = PROVISION_KNOWN_HOSTS_LOCK.lock().await;
    let host = state
        .store
        .client_market_get_host_for_operator(&id, &session)
        .await?;
    let observed = ssh_scan_host_key(
        &host.ip,
        host.port,
        host.ssh_host_key_fingerprint.as_deref(),
    )
    .await?;
    let inspection = ssh_host_key_inspection(&host, &observed);
    if !inspection.confirmation_required {
        ensure_strict_known_host_entry(&state.config, &host, &observed).await?;
    }
    Ok(Json(inspection))
}

async fn rotate_host_ssh_key(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(input): Json<RotateHostSshKeyRequest>,
) -> Result<Json<RotateHostSshKeyResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    let expected_current =
        normalized_optional_fingerprint(input.expected_current_fingerprint.as_deref());
    let confirmed = normalize_confirmed_ssh_fingerprint(&input.confirmed_fingerprint)?;
    if !input.verified_from_host_console {
        return Err(AppError::BadRequest(
            "confirm that the fingerprint was verified from the Host console".into(),
        ));
    }
    let initial_host = state
        .store
        .client_market_get_host_for_operator(&id, &session)
        .await?;
    require_host_key_rotation_idle(&state, &initial_host).await?;
    require_expected_host_fingerprint(&initial_host, expected_current.as_deref())?;

    let _known_hosts_guard = PROVISION_KNOWN_HOSTS_LOCK.lock().await;
    let host = state
        .store
        .client_market_get_host_for_operator(&id, &session)
        .await?;
    require_host_key_rotation_idle(&state, &host).await?;
    require_expected_host_fingerprint(&host, expected_current.as_deref())?;

    let observed = ssh_scan_host_key(
        &host.ip,
        host.port,
        host.ssh_host_key_fingerprint.as_deref(),
    )
    .await?;
    let inspection = ssh_host_key_inspection(&host, &observed);
    if observed.fingerprint != confirmed {
        return Err(AppError::coded_conflict(
            "SSH_HOST_KEY_CHANGED_DURING_CONFIRMATION",
            "the observed SSH host fingerprint does not match the confirmed fingerprint",
            serde_json::to_value(&inspection).unwrap_or_else(|_| serde_json::json!({})),
        ));
    }

    if normalized_optional_fingerprint(host.ssh_host_key_fingerprint.as_deref()).as_deref()
        == Some(confirmed.as_str())
    {
        ensure_strict_known_host_entry(&state.config, &host, &observed).await?;
        return Ok(Json(RotateHostSshKeyResponse {
            host: host_to_view(host, true),
            inspection,
        }));
    }

    let known_hosts = known_hosts_path(&state.config);
    let snapshot = install_known_host_entry(&known_hosts, &observed).await?;
    let updated = match state
        .store
        .client_market_rotate_host_fingerprint(
            &host.id,
            HostFingerprintRotation {
                expected_fingerprint: host.ssh_host_key_fingerprint.as_deref(),
                fingerprint: &observed.fingerprint,
                key_type: &observed.key_type,
                actor_user_id: &session.user_id,
                actor_email: &session.email,
            },
        )
        .await
    {
        Ok(updated) => updated,
        Err(error) => {
            if let Err(rollback_error) = restore_known_hosts_snapshot(&known_hosts, &snapshot) {
                return Err(AppError::Internal(format!(
                    "rotate SSH host fingerprint failed ({error}); restoring known_hosts also failed: {rollback_error}"
                )));
            }
            return Err(error);
        }
    };
    let updated_inspection = ssh_host_key_inspection(&updated, &observed);
    info!(
        host_id = %updated.id,
        endpoint = %observed.target,
        old_fingerprint = ?host.ssh_host_key_fingerprint,
        new_fingerprint = %observed.fingerprint,
        "client market SSH host fingerprint rotated"
    );
    Ok(Json(RotateHostSshKeyResponse {
        host: host_to_view(updated, true),
        inspection: updated_inspection,
    }))
}

async fn delete_host(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    state.store.client_market_delete_host(&id, &session).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetireUnreachableHostResponse {
    pub host_id: String,
    pub installation_id: String,
    pub previous_subscription_status: String,
    pub status: String,
    #[serde(skip)]
    subdomain: Option<String>,
}

async fn retire_unreachable_host(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<RetireUnreachableHostResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    let outcome = state
        .store
        .client_market_retire_unreachable_host(&id, &session)
        .await?;
    if let Some(subdomain) = outcome.subdomain.as_deref() {
        state.proxy.remove_route(subdomain).await;
    }
    Ok(Json(outcome))
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
    deny_client_access: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
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

async fn release_client(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(installation_id): AxumPath<String>,
) -> Result<Json<CreateClientResponse>, AppError> {
    start_client_cleanup(
        state,
        headers,
        installation_id,
        "client_release",
        false,
        "client",
    )
    .await
}

async fn provider_cleanup_client(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(installation_id): AxumPath<String>,
    input: Option<Json<CleanupClientRequest>>,
) -> Result<Json<CreateClientResponse>, AppError> {
    let input = input.map(|Json(value)| value).unwrap_or_default();
    let reason = input.reason.as_deref().unwrap_or("provider_release");
    if !matches!(
        reason,
        "provider_release" | "host_maintenance" | "service_terminated" | "other"
    ) {
        return Err(AppError::BadRequest(
            "unsupported Provider cleanup reason".into(),
        ));
    }
    start_client_cleanup(
        state,
        headers,
        installation_id,
        reason,
        input.deny_client_access.unwrap_or(false),
        "provider",
    )
    .await
}

async fn start_client_cleanup(
    state: ServerState,
    headers: HeaderMap,
    installation_id: String,
    reason: &str,
    deny_client_access: bool,
    required_role: &str,
) -> Result<Json<CreateClientResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    let viewer = session.email.clone();
    let subdomain = state
        .store
        .client_market_subdomain_for_installation(&installation_id)
        .await?;
    // Session admins are not elevated: only the Host owner or Client owner may cleanup.
    let job_id = {
        let _recovery_guard = state.client_market_recovery.lock(&installation_id).await;
        state
            .store
            .client_market_begin_cleanup_job_with_context(
                &installation_id,
                Some(&session.user_id),
                &viewer,
                false,
                Some(required_role),
                reason,
                Some(deny_client_access),
            )
            .await?
    };
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

pub(crate) async fn terminate_for_billing(
    state: &ServerState,
    installation_id: &str,
    reason: &str,
) -> Result<(), AppError> {
    let status = {
        let conn = state.store.conn.lock().await;
        conn.query_row(
            "SELECT status FROM client_market_subscriptions WHERE installation_id = ?1",
            params![installation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| {
            AppError::Internal(format!(
                "read Client subscription for billing termination failed: {error}"
            ))
        })?
    };
    if status
        .as_deref()
        .is_none_or(|status| matches!(status, "released" | "releasing" | "release_failed"))
    {
        return Ok(());
    }
    let subdomain = state
        .store
        .client_market_subdomain_for_installation(installation_id)
        .await?;
    let begin_cleanup = {
        let _recovery_guard = state.client_market_recovery.lock(installation_id).await;
        state
            .store
            .client_market_begin_system_cleanup_job(installation_id, reason)
            .await
    };
    let job_id = match begin_cleanup {
        Ok(job_id) => job_id,
        Err(AppError::Conflict(_)) => {
            let conn = state.store.conn.lock().await;
            let releasing = conn
                .query_row(
                    "SELECT 1 FROM client_market_subscriptions
                     WHERE installation_id = ?1
                       AND status IN ('released', 'releasing', 'release_failed')",
                    params![installation_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(|error| {
                    AppError::Internal(format!(
                        "recheck Client billing termination failed: {error}"
                    ))
                })?
                .is_some();
            if releasing {
                return Ok(());
            }
            return Err(AppError::Conflict(
                "Client billing termination raced with another operation".into(),
            ));
        }
        Err(error) => return Err(error),
    };
    if let Some(subdomain) = subdomain.as_deref() {
        state.proxy.remove_route(subdomain).await;
    }
    let runner_state = state.clone();
    tokio::spawn(async move {
        if let Err(error) = run_cleanup_job(runner_state, job_id.clone()).await {
            error!(job_id = %job_id, error = %error, "Client billing termination cleanup failed");
        }
    });
    Ok(())
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
    let result = run_job_with_lease(
        &state,
        &job_id,
        JOB_TYPE_CREATE,
        false,
        CREATE_JOB_MAX_RUNTIME,
        run_create_job_inner(&state, &job_id),
    )
    .await;
    if let Err(ref error) = result
        && !job_lease_was_revoked(error)
    {
        handle_create_job_failure(&state, &job_id, error).await;
    }
    if let Err(error) = state.store.client_market_sync_batch_for_job(&job_id).await {
        warn!(job_id = %job_id, error = %error, "failed to synchronize Client Market batch state");
    }
    result
}

pub(crate) async fn run_job_with_lease<F>(
    state: &ServerState,
    job_id: &str,
    expected_type: &str,
    resume_running: bool,
    max_runtime: Duration,
    operation: F,
) -> Result<(), AppError>
where
    F: Future<Output = Result<(), AppError>>,
{
    let worker_id = Uuid::new_v4().to_string();
    state
        .store
        .client_market_claim_job_execution(
            job_id,
            expected_type,
            &worker_id,
            resume_running,
            max_runtime,
        )
        .await?;
    let mut heartbeat = tokio::time::interval(JOB_HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    heartbeat.tick().await;
    let deadline = tokio::time::sleep(max_runtime);
    tokio::pin!(deadline);
    tokio::pin!(operation);
    loop {
        tokio::select! {
            result = &mut operation => return result,
            _ = heartbeat.tick() => {
                if !state.store.client_market_heartbeat_job(job_id, &worker_id).await? {
                    return Err(AppError::Conflict(JOB_LEASE_REVOKED_MESSAGE.into()));
                }
            }
            _ = &mut deadline => {
                return Err(AppError::ServiceUnavailable(format!(
                    "{expected_type} job exceeded its runtime deadline"
                )));
            }
        }
    }
}

pub(crate) fn job_lease_was_revoked(error: &AppError) -> bool {
    matches!(error, AppError::Conflict(message) if message == JOB_LEASE_REVOKED_MESSAGE)
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
                    let result = run_cleanup_job_with_mode(
                        &runner_state,
                        &job_id,
                        job.status == JOB_STATUS_RUNNING,
                    )
                    .await;
                    if let Err(ref error) = result
                        && !job_lease_was_revoked(error)
                    {
                        handle_cleanup_job_failure(&runner_state, &job_id, error).await;
                    }
                }
                crate::client_market_recovery::JOB_TYPE_RECOVER => {
                    crate::client_market_recovery::resume_interrupted_job(runner_state, job).await;
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
    reconcile_expired_job_leases(&state, Utc::now()).await?;
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
        let begin_cleanup = {
            let _recovery_guard = state.client_market_recovery.lock(&installation_id).await;
            state
                .store
                .client_market_begin_system_cleanup_job(&installation_id, "stale_draining_recovery")
                .await
        };
        match begin_cleanup {
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

    let cleanup_recovery_claims = state
        .store
        .client_market_claim_due_cleanup_recoveries(now, CLEANUP_RECOVERY_CLAIM_LIMIT)
        .await?;
    let cleanup_recovery_results = stream::iter(cleanup_recovery_claims.into_iter().map(|claim| {
        let runner_state = state.clone();
        async move { reconcile_cleanup_recovery_claim(runner_state, claim).await }
    }))
    .buffer_unordered(CLEANUP_RECOVERY_CLAIM_LIMIT)
    .collect::<Vec<_>>()
    .await;
    for result in cleanup_recovery_results {
        if let Err(error) = result {
            warn!(error = %error, "cleanup recovery probe failed");
        }
    }

    let reprobe_claims = state
        .store
        .client_market_claim_due_quarantined_host_reprobes(now, HOST_REPROBE_CLAIM_LIMIT)
        .await?;
    let reprobe_results = stream::iter(reprobe_claims.into_iter().map(|claim| {
        let runner_state = state.clone();
        async move { reconcile_quarantined_host_reprobe(runner_state, claim).await }
    }))
    .buffer_unordered(HOST_REPROBE_CLAIM_LIMIT)
    .collect::<Vec<_>>()
    .await;
    for result in reprobe_results {
        if let Err(error) = result {
            warn!(error = %error, "quarantined Host reprobe failed");
        }
    }

    reconcile_stranded_locked_hosts(&state, now).await?;
    reconcile_stranded_reserved_hosts(&state.store, now).await?;
    Ok(())
}

fn quarantined_host_reprobe_next_at(now: DateTime<Utc>, completed_attempts: u32) -> DateTime<Utc> {
    let index = completed_attempts
        .saturating_sub(1)
        .min((HOST_REPROBE_BACKOFF.len() - 1) as u32) as usize;
    now + chrono::Duration::from_std(HOST_REPROBE_BACKOFF[index])
        .unwrap_or(chrono::Duration::hours(24))
}

async fn reconcile_quarantined_host_reprobe(
    state: ServerState,
    claim: QuarantinedHostReprobeClaim,
) -> Result<(), AppError> {
    let Some(host) = state.store.client_market_get_host(&claim.host_id).await? else {
        return Ok(());
    };
    if host.installation_id.is_some()
        || !matches!(
            host.status.as_str(),
            HOST_STATUS_UNREACHABLE | HOST_STATUS_ABNORMAL
        )
    {
        return Ok(());
    }
    match ssh_remote_is_market_clean(&state, &host).await {
        Ok(true) => {
            state
                .store
                .client_market_complete_host_reverify(
                    &host.id,
                    host.hostname.as_deref(),
                    host.ssh_host_key_fingerprint.as_deref(),
                )
                .await?;
            info!(host_id = %host.id, attempt = claim.attempt_count, "returned clean quarantined Host to idle");
            Ok(())
        }
        Ok(false) => {
            state
                .store
                .client_market_finish_quarantined_host_reprobe(
                    &claim,
                    "remote_not_clean",
                    true,
                    Utc::now(),
                )
                .await
        }
        Err(error) => {
            let outcome = format!("probe_failed:{}", classify_cleanup_failure(&error));
            let retry = !cleanup_recovery_requires_manual_intervention(&outcome);
            state
                .store
                .client_market_finish_quarantined_host_reprobe(&claim, &outcome, retry, Utc::now())
                .await?;
            Err(error)
        }
    }
}

async fn reconcile_expired_job_leases(
    state: &ServerState,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let jobs = state
        .store
        .client_market_claim_expired_job_leases(now)
        .await?;
    for job in jobs {
        let runner_state = state.clone();
        tokio::spawn(async move {
            let error = AppError::ServiceUnavailable(
                "Client Market job stopped heartbeating or exceeded its deadline".into(),
            );
            warn!(job_id = %job.id, job_type = %job.job_type, "expiring stale Client Market job lease");
            match job.job_type.as_str() {
                JOB_TYPE_CREATE => handle_create_job_failure(&runner_state, &job.id, &error).await,
                JOB_TYPE_CLEANUP => {
                    handle_cleanup_job_failure(&runner_state, &job.id, &error).await
                }
                crate::client_market_recovery::JOB_TYPE_RECOVER => {
                    crate::client_market_recovery::expire_stale_job(&runner_state, &job.id).await;
                }
                _ => {
                    let _ = runner_state
                        .store
                        .client_market_fail_job(&job.id, "unsupported stale Client Market job\n")
                        .await;
                }
            }
        });
    }
    Ok(())
}

async fn reconcile_cleanup_recovery_claim(
    state: ServerState,
    claim: CleanupRecoveryClaim,
) -> Result<(), AppError> {
    let _recovery_guard = state
        .client_market_recovery
        .lock(&claim.installation_id)
        .await;
    let Some(host) = state.store.client_market_get_host(&claim.host_id).await? else {
        return Ok(());
    };
    if host.status != HOST_STATUS_UNREACHABLE
        || host.installation_id.as_deref() != Some(claim.installation_id.as_str())
    {
        return Ok(());
    }
    match ssh_remote_is_market_clean(&state, &host).await {
        Ok(true) => {
            if let Some(subdomain) = host.client_subdomain.as_deref() {
                state.proxy.remove_route(subdomain).await;
            }
            match state
                .store
                .client_market_finalize_clean_unreachable_host(
                    &claim.host_id,
                    &claim.installation_id,
                )
                .await
            {
                Ok(()) => {
                    info!(
                        host_id = %claim.host_id,
                        installation_id = %claim.installation_id,
                        attempt = claim.attempt_count,
                        "returned remotely clean unreachable Host to the idle pool"
                    );
                    Ok(())
                }
                Err(error) => {
                    state
                        .store
                        .client_market_finish_cleanup_recovery_attempt(
                            &claim,
                            &format!("router_finalize_failed: {error}"),
                            Utc::now(),
                        )
                        .await?;
                    Err(error)
                }
            }
        }
        Ok(false) => {
            state
                .store
                .client_market_finish_cleanup_recovery_attempt(
                    &claim,
                    "remote_not_clean",
                    Utc::now(),
                )
                .await
        }
        Err(error) => {
            let outcome = format!("probe_failed:{}", classify_cleanup_failure(&error));
            state
                .store
                .client_market_finish_cleanup_recovery_attempt(&claim, &outcome, Utc::now())
                .await?;
            Err(error)
        }
    }
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
    store: &AppStore,
    now: chrono::DateTime<Utc>,
) -> Result<(), AppError> {
    let reserved = store
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
        if store.client_market_host_has_live_quote(&host.id).await? {
            continue;
        }
        // No live quote can claim it, and a reserved Host was never touched remotely,
        // so returning it straight to the pool is safe.
        match store
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
        let resumed = run_job_with_lease(
            state,
            &job.id,
            JOB_TYPE_CREATE,
            true,
            CREATE_JOB_MAX_RUNTIME,
            async {
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
            },
        )
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
        if resumed
            .as_ref()
            .is_err_and(|error| job_lease_was_revoked(error))
        {
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
    let result = run_cleanup_job_with_mode(&state, &job_id, false).await;
    if let Err(ref error) = result
        && !job_lease_was_revoked(error)
    {
        handle_cleanup_job_failure(&state, &job_id, error).await;
    }
    result
}

async fn run_cleanup_job_with_mode(
    state: &ServerState,
    job_id: &str,
    resume_running: bool,
) -> Result<(), AppError> {
    run_job_with_lease(
        state,
        job_id,
        JOB_TYPE_CLEANUP,
        resume_running,
        CLEANUP_JOB_MAX_RUNTIME,
        run_cleanup_job_inner(state, job_id),
    )
    .await
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
    if lower.contains("fingerprint")
        || lower.contains("no pinned ssh")
        || lower.contains("remote host identification has changed")
        || lower.contains("host key verification failed")
    {
        CLEANUP_FAILURE_FINGERPRINT
    } else if lower.contains("binding") || lower.contains("installation mismatch") {
        CLEANUP_FAILURE_BINDING
    } else if cleanup_error_is_ssh_unreachable(&lower) {
        CLEANUP_FAILURE_SSH_UNREACHABLE
    } else if lower.contains("exceeded its execution timeout") || lower.contains("timeout") {
        CLEANUP_FAILURE_SSH_TIMEOUT
    } else if lower.contains("failed to stop")
        || lower.contains("respawned")
        || lower.contains("still running after wipe")
        || lower.contains("still running after stop")
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

fn cleanup_error_is_ssh_unreachable(lower: &str) -> bool {
    [
        "connection refused",
        "connection timed out",
        "operation timed out",
        "network is unreachable",
        "no route to host",
        "could not resolve hostname",
        "temporary failure in name resolution",
        "name or service not known",
        "connection reset by peer",
        "connection closed by remote host",
        "connection closed by ",
        "kex_exchange_identification: connection closed",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn cleanup_recovery_next_at(now: DateTime<Utc>, completed_attempts: u32) -> Option<DateTime<Utc>> {
    CLEANUP_RECOVERY_BACKOFF
        .get(completed_attempts as usize)
        .and_then(|delay| chrono::Duration::from_std(*delay).ok())
        .map(|delay| now + delay)
}

fn cleanup_recovery_requires_manual_intervention(outcome: &str) -> bool {
    outcome.contains(CLEANUP_FAILURE_FINGERPRINT) || outcome.contains(CLEANUP_FAILURE_BINDING)
}

async fn run_cleanup_job_inner(state: &ServerState, job_id: &str) -> Result<(), AppError> {
    state
        .store
        .client_market_append_job_log(job_id, "starting cleanup job\n")
        .await?;
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
            let stop_output = ssh_stop_cc_switch_server(state, &host).await?;
            if !stop_output.trim().is_empty() {
                state
                    .store
                    .client_market_append_job_log(job_id, &format!("{}\n", stop_output.trim_end()))
                    .await?;
            }
            state
                .store
                .client_market_append_job_log(
                    job_id,
                    "remote process stopped (verified not running)\n",
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
            let wipe_output = ssh_wipe_cc_switch_files(state, &host).await?;
            if !wipe_output.trim().is_empty() {
                state
                    .store
                    .client_market_append_job_log(job_id, &format!("{}\n", wipe_output.trim_end()))
                    .await?;
            }
            state
                .store
                .client_market_append_job_log(
                    job_id,
                    "remote client files removed; process verified stopped\n",
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
    let detail = crate::store::client_chat::sanitize_system_event_text(&error.to_string());
    warn!(job_id = %job_id, error = %detail, "client market cleanup failed");
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
        CLEANUP_FAILURE_SSH_UNREACHABLE
        | CLEANUP_FAILURE_SSH_TIMEOUT
        | CLEANUP_FAILURE_STOP
        | CLEANUP_FAILURE_WIPE => {
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ProvisionSshRotationStage {
    Distributing,
    Activated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProvisionSshRotationState {
    version: u32,
    stage: ProvisionSshRotationStage,
    old_public_key: String,
    new_public_key: String,
    created_at: String,
}

#[derive(Debug, Clone)]
pub struct ProvisionSshRotationReport {
    pub host_count: usize,
}

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn atomic_write_file_mode(path: &Path, bytes: &[u8], mode: u32) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent directory: {}", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create directory failed: {}", parent.display()))?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("rotation-state");
    let temporary = parent.join(format!(".{name}.{}.tmp", Uuid::new_v4()));
    let result = (|| -> anyhow::Result<()> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(mode)
            .open(&temporary)
            .with_context(|| format!("create temporary file failed: {}", temporary.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("write temporary file failed: {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("sync temporary file failed: {}", temporary.display()))?;
        drop(file);
        fs::set_permissions(&temporary, fs::Permissions::from_mode(mode)).with_context(|| {
            format!(
                "set temporary file permissions failed: {}",
                temporary.display()
            )
        })?;
        fs::rename(&temporary, path)
            .with_context(|| format!("replace file failed: {}", path.display()))?;
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .with_context(|| format!("sync directory failed: {}", parent.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn write_provision_rotation_state(
    path: &Path,
    state: &ProvisionSshRotationState,
) -> anyhow::Result<()> {
    let mut serialized = serde_json::to_vec_pretty(state)?;
    serialized.push(b'\n');
    atomic_write_file_mode(path, &serialized, 0o600)
}

fn activate_provision_ssh_candidate(
    private_key_path: &Path,
    public_key_path: &Path,
    candidate_private_path: &Path,
    candidate_public_path: &Path,
) -> anyhow::Result<()> {
    let previous_private_path = path_with_suffix(private_key_path, ".previous");
    let previous_public_path = path_with_suffix(public_key_path, ".previous");
    if previous_private_path.exists() || previous_public_path.exists() {
        bail!(
            "previous provisioning SSH key backup already exists; inspect {} before retrying",
            previous_private_path.display()
        );
    }
    fs::copy(private_key_path, &previous_private_path).with_context(|| {
        format!(
            "back up active provisioning SSH private key failed: {}",
            previous_private_path.display()
        )
    })?;
    fs::set_permissions(&previous_private_path, fs::Permissions::from_mode(0o600))?;
    fs::copy(public_key_path, &previous_public_path).with_context(|| {
        format!(
            "back up active provisioning SSH public key failed: {}",
            previous_public_path.display()
        )
    })?;
    fs::set_permissions(&previous_public_path, fs::Permissions::from_mode(0o644))?;

    fs::rename(candidate_private_path, private_key_path).with_context(|| {
        format!(
            "activate candidate provisioning SSH private key failed: {}",
            private_key_path.display()
        )
    })?;
    if let Err(error) = fs::rename(candidate_public_path, public_key_path) {
        let _ = fs::copy(private_key_path, candidate_private_path);
        let _ = fs::copy(&previous_private_path, private_key_path);
        return Err(error).with_context(|| {
            format!(
                "activate candidate provisioning SSH public key failed: {}",
                public_key_path.display()
            )
        });
    }
    crate::provision_ssh::require_provision_ssh_keys(private_key_path, public_key_path)?;
    if let Some(parent) = private_key_path.parent() {
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .with_context(|| {
                format!(
                    "sync provisioning key directory failed: {}",
                    parent.display()
                )
            })?;
    }
    Ok(())
}

fn remove_old_provision_key_remote_command(
    old_public_key: &str,
    new_authorized_keys_line: &str,
) -> anyhow::Result<String> {
    let mut old_parts = old_public_key.split_whitespace();
    let old_algorithm = old_parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("old provisioning public key has no algorithm"))?;
    let old_body = old_parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("old provisioning public key has no body"))?;
    let mut new_parts = new_authorized_keys_line.split_whitespace();
    let new_algorithm = new_parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("new provisioning public key has no algorithm"))?;
    let new_body = new_parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("new provisioning public key has no body"))?;
    Ok(format!(
        "set -eu; \
         file=\"$HOME/.ssh/authorized_keys\"; \
         test -f \"$file\"; \
         tmp=\"$HOME/.ssh/.authorized_keys.rotate.$$\"; \
         trap 'rm -f \"$tmp\"' EXIT HUP INT TERM; \
         awk -v oa={old_algorithm} -v ob={old_body} -v na={new_algorithm} -v nb={new_body} \
           'function haskey(a,b, i) {{ for (i=1; i<NF; i++) if ($i==a && $(i+1)==b) return 1; return 0 }} \
            !haskey(oa,ob) && !haskey(na,nb) {{ print }}' \"$file\" > \"$tmp\"; \
         printf '%s\\n' {new_line} >> \"$tmp\"; \
         chmod 600 \"$tmp\"; \
         mv \"$tmp\" \"$file\"; \
         trap - EXIT HUP INT TERM; \
         printf 'CC_SWITCH_PROVISION_KEY_ROTATED=1\\n'",
        old_algorithm = shell_quote(old_algorithm),
        old_body = shell_quote(old_body),
        new_algorithm = shell_quote(new_algorithm),
        new_body = shell_quote(new_body),
        new_line = shell_quote(new_authorized_keys_line),
    ))
}

pub async fn rotate_provision_ssh_key_offline(
    config: &Config,
    store: &AppStore,
) -> anyhow::Result<ProvisionSshRotationReport> {
    let private_key_path = &config.provision_ssh_private_key_path;
    let public_key_path = &config.provision_ssh_public_key_path;
    let candidate_private_path = path_with_suffix(private_key_path, ".next");
    let candidate_public_path = path_with_suffix(public_key_path, ".next");
    let rotation_state_path = path_with_suffix(private_key_path, ".rotation.json");
    let previous_private_path = path_with_suffix(private_key_path, ".previous");
    let previous_public_path = path_with_suffix(public_key_path, ".previous");

    let mut rotation = if rotation_state_path.is_file() {
        serde_json::from_slice::<ProvisionSshRotationState>(
            &fs::read(&rotation_state_path).with_context(|| {
                format!(
                    "read provisioning SSH rotation state failed: {}",
                    rotation_state_path.display()
                )
            })?,
        )?
    } else {
        crate::provision_ssh::require_provision_ssh_keys(private_key_path, public_key_path)?;
        if candidate_private_path.exists()
            || candidate_public_path.exists()
            || previous_private_path.exists()
            || previous_public_path.exists()
        {
            bail!(
                "stale provisioning SSH rotation files exist beside {}; inspect them before starting",
                private_key_path.display()
            );
        }
        crate::provision_ssh::require_provision_ssh_keys(
            &candidate_private_path,
            &candidate_public_path,
        )?;
        let state = ProvisionSshRotationState {
            version: 1,
            stage: ProvisionSshRotationStage::Distributing,
            old_public_key: crate::provision_ssh::public_key_openssh_from_public_path(
                public_key_path,
            )?,
            new_public_key: crate::provision_ssh::public_key_openssh_from_public_path(
                &candidate_public_path,
            )?,
            created_at: Utc::now().to_rfc3339(),
        };
        write_provision_rotation_state(&rotation_state_path, &state)?;
        state
    };
    if rotation.version != 1 {
        bail!(
            "unsupported provisioning SSH rotation state version {}",
            rotation.version
        );
    }

    let active_derived = crate::provision_ssh::derive_public_key(private_key_path)?;
    if rotation.stage == ProvisionSshRotationStage::Distributing
        && active_derived == rotation.new_public_key
    {
        atomic_write_file_mode(
            public_key_path,
            format!("{} cc-switch-router-provision\n", rotation.new_public_key).as_bytes(),
            0o644,
        )?;
        rotation.stage = ProvisionSshRotationStage::Activated;
        write_provision_rotation_state(&rotation_state_path, &rotation)?;
    }

    let hosts = store.client_market_list_hosts(None, None, None).await?;
    let known_hosts = known_hosts_path(config);
    if rotation.stage == ProvisionSshRotationStage::Distributing {
        if active_derived != rotation.old_public_key {
            bail!("active provisioning SSH key changed while rotation was staged");
        }
        crate::provision_ssh::require_provision_ssh_keys(
            &candidate_private_path,
            &candidate_public_path,
        )?;
        let candidate_line = format!(
            "{} cc-switch-router-provision-next",
            rotation.new_public_key
        );
        let install_command = install_provision_key_remote_command(&candidate_line);
        for host in &hosts {
            if host.ssh_host_key_fingerprint.is_none() {
                bail!(
                    "Host {} has no pinned SSH fingerprint; reverify it before rotating keys",
                    host.id
                );
            }
            require_pinned_host_fingerprint(host, &known_hosts).await?;
            ssh_run_remote_with_input(
                private_key_path,
                &known_hosts,
                &host.ip,
                host.port,
                &install_command,
                None,
                SSH_VERIFY_TIMEOUT,
                SshHostKeyPolicy::RequireKnown,
            )
            .await?;
            let verified = ssh_run_remote_with_input(
                &candidate_private_path,
                &known_hosts,
                &host.ip,
                host.port,
                "printf 'CC_SWITCH_PROVISION_KEY_CANDIDATE=1\\n'",
                None,
                SSH_VERIFY_TIMEOUT,
                SshHostKeyPolicy::RequireKnown,
            )
            .await?;
            if !verified
                .lines()
                .any(|line| line.trim() == "CC_SWITCH_PROVISION_KEY_CANDIDATE=1")
            {
                bail!(
                    "candidate provisioning SSH key verification failed for Host {}",
                    host.id
                );
            }
            store
                .client_market_record_audit_event(
                    host.installation_id.as_deref(),
                    Some(&host.id),
                    None,
                    None,
                    "provision_ssh_rotation_candidate_verified",
                    serde_json::json!({}),
                )
                .await?;
        }
        activate_provision_ssh_candidate(
            private_key_path,
            public_key_path,
            &candidate_private_path,
            &candidate_public_path,
        )?;
        rotation.stage = ProvisionSshRotationStage::Activated;
        write_provision_rotation_state(&rotation_state_path, &rotation)?;
    }

    crate::provision_ssh::require_provision_ssh_keys(private_key_path, public_key_path)?;
    if crate::provision_ssh::derive_public_key(private_key_path)? != rotation.new_public_key {
        bail!("activated provisioning SSH key does not match rotation state");
    }
    let active_line = format!("{} cc-switch-router-provision", rotation.new_public_key);
    let cleanup_command =
        remove_old_provision_key_remote_command(&rotation.old_public_key, &active_line)?;
    for host in &hosts {
        if host.ssh_host_key_fingerprint.is_none() {
            bail!(
                "Host {} has no pinned SSH fingerprint; cannot finish key cleanup",
                host.id
            );
        }
        require_pinned_host_fingerprint(host, &known_hosts).await?;
        let output = ssh_run_remote_with_input(
            private_key_path,
            &known_hosts,
            &host.ip,
            host.port,
            &cleanup_command,
            None,
            SSH_VERIFY_TIMEOUT,
            SshHostKeyPolicy::RequireKnown,
        )
        .await?;
        if !output
            .lines()
            .any(|line| line.trim() == "CC_SWITCH_PROVISION_KEY_ROTATED=1")
        {
            bail!(
                "old provisioning SSH key cleanup failed for Host {}",
                host.id
            );
        }
        store
            .client_market_record_audit_event(
                host.installation_id.as_deref(),
                Some(&host.id),
                None,
                None,
                "provision_ssh_rotation_completed",
                serde_json::json!({}),
            )
            .await?;
    }

    for path in [
        previous_private_path,
        previous_public_path,
        candidate_private_path,
        candidate_public_path,
        rotation_state_path,
    ] {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("remove rotation artifact failed: {}", path.display())
                });
            }
        }
    }
    Ok(ProvisionSshRotationReport {
        host_count: hosts.len(),
    })
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
    // Leftover install files are fine: install-client.sh backs up $HOME/.cc-switch-server
    // and reinstalls the binary when the host is later claimed for provisioning.
    let output = ssh_run_remote_with_input(
        &state.provision_ssh_key_path,
        known_hosts,
        ip,
        port,
        "set -eu; uname -n",
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
) -> Result<String, AppError> {
    let known_hosts = known_hosts_path(&state.config);
    require_pinned_host_fingerprint(host, &known_hosts).await?;
    let command = format!(
        "{helpers}\
         set -eu; \
         pids=$(cc_switch_server_list_pids | sort -u | tr '\\n' ' '); \
         echo \"detected cc-switch-server pids: ${{pids:-none}}\"; \
         if ! cc_switch_server_stop; then \
           echo 'failed to stop cc-switch-server' >&2; exit 43; \
         fi; \
         echo 'post-stop running=no'",
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
}

async fn ssh_wipe_cc_switch_files(
    state: &ServerState,
    host: &RouterSshHostRecord,
) -> Result<String, AppError> {
    let known_hosts = known_hosts_path(&state.config);
    require_pinned_host_fingerprint(host, &known_hosts).await?;
    let command = format!(
        "{helpers}\
         set -eu; \
         if ! cc_switch_server_stop; then \
           echo 'failed to stop cc-switch-server' >&2; exit 43; \
         fi; \
         cc_switch_server_wipe_files; \
         if cc_switch_server_is_running; then \
           echo 'cc-switch-server still running after wipe' >&2; exit 43; \
         fi; \
         if cc_switch_server_has_install_files; then \
           echo 'failed to remove cc-switch-server installation files' >&2; exit 44; \
         fi; \
         echo 'post-wipe running=no files=removed'",
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClientRecoveryRemoteOutcome {
    AlreadyRunning,
    Started { method: String },
    MissingBinary,
    MissingConfig,
    StartFailed { method: String },
}

/// Check and, only when absent, start the Client Market process. The command is
/// idempotent and keeps strict SSH host verification enabled for unattended recovery.
pub(crate) async fn ssh_recover_market_client_process(
    state: &ServerState,
    host: &RouterSshHostRecord,
) -> Result<ClientRecoveryRemoteOutcome, AppError> {
    let known_hosts = known_hosts_path(&state.config);
    require_pinned_host_fingerprint(host, &known_hosts).await?;
    let command = format!(
        "{helpers}\
         set +e; \
         if cc_switch_server_is_running; then \
           echo 'CC_SWITCH_RECOVERY=already_running'; exit 0; \
         fi; \
         if [ ! -x /usr/local/bin/cc-switch-server ]; then \
           echo 'CC_SWITCH_RECOVERY=missing_binary'; exit 0; \
         fi; \
         home=$(cc_switch_server_home); \
         if [ ! -r \"$home/.cc-switch-server/server.json\" ]; then \
           echo 'CC_SWITCH_RECOVERY=missing_config'; exit 0; \
         fi; \
         method=nohup; started=0; \
         if command -v systemctl >/dev/null 2>&1 \
              && [ -d /run/systemd/system ] \
              && systemctl cat cc-switch-server.service >/dev/null 2>&1; then \
           method=systemd; \
           systemctl start cc-switch-server.service >/dev/null 2>&1 && started=1; \
         elif command -v rc-service >/dev/null 2>&1 \
              && [ -x /etc/init.d/cc-switch-server ]; then \
           method=openrc; \
           rc-service cc-switch-server start >/dev/null 2>&1 && started=1; \
         else \
           cd \"$home\" >/dev/null 2>&1 \
             && nohup /usr/local/bin/cc-switch-server </dev/null >/dev/null 2>&1 & \
           started=1; \
         fi; \
         if [ \"$started\" -ne 1 ]; then \
           echo \"CC_SWITCH_RECOVERY=start_failed:$method\"; exit 0; \
         fi; \
         sleep 2; \
         if cc_switch_server_is_running; then \
           echo \"CC_SWITCH_RECOVERY=started:$method\"; \
         else \
           echo \"CC_SWITCH_RECOVERY=start_failed:$method\"; \
         fi",
        helpers = REMOTE_CC_SWITCH_SERVER_HELPERS,
    );
    let output = ssh_run_remote_with_input_connect_timeout(
        &state.provision_ssh_key_path,
        &known_hosts,
        &host.ip,
        host.port,
        &command,
        None,
        Duration::from_secs(45),
        SshHostKeyPolicy::RequireKnown,
        10,
    )
    .await?;
    parse_client_recovery_remote_outcome(&output)
}

fn parse_client_recovery_remote_outcome(
    output: &str,
) -> Result<ClientRecoveryRemoteOutcome, AppError> {
    let marker = output
        .lines()
        .rev()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("CC_SWITCH_RECOVERY="))
        .ok_or_else(|| {
            AppError::ServiceUnavailable(
                "recovery command completed without a valid result marker".into(),
            )
        })?;
    match marker {
        "already_running" => Ok(ClientRecoveryRemoteOutcome::AlreadyRunning),
        "missing_binary" => Ok(ClientRecoveryRemoteOutcome::MissingBinary),
        "missing_config" => Ok(ClientRecoveryRemoteOutcome::MissingConfig),
        value if value.starts_with("started:") => Ok(ClientRecoveryRemoteOutcome::Started {
            method: recovery_method(value.trim_start_matches("started:"))?,
        }),
        value if value.starts_with("start_failed:") => {
            Ok(ClientRecoveryRemoteOutcome::StartFailed {
                method: recovery_method(value.trim_start_matches("start_failed:"))?,
            })
        }
        _ => Err(AppError::ServiceUnavailable(
            "recovery command returned an unknown result marker".into(),
        )),
    }
}

fn recovery_method(value: &str) -> Result<String, AppError> {
    matches!(value, "systemd" | "openrc" | "nohup")
        .then(|| value.to_string())
        .ok_or_else(|| AppError::ServiceUnavailable("recovery method marker is invalid".into()))
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

fn normalized_optional_fingerprint(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_confirmed_ssh_fingerprint(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    let encoded = value.strip_prefix("SHA256:").ok_or_else(|| {
        AppError::BadRequest("confirmed fingerprint must use the SHA256:<base64> format".into())
    })?;
    let digest = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(encoded)
        .map_err(|_| AppError::BadRequest("confirmed SSH fingerprint is invalid".into()))?;
    if digest.len() != 32 {
        return Err(AppError::BadRequest(
            "confirmed SSH fingerprint is invalid".into(),
        ));
    }
    Ok(value.to_string())
}

fn ssh_known_hosts_target(ip: &str, port: u16) -> String {
    if port == 22 {
        ip.to_string()
    } else {
        format!("[{ip}]:{port}")
    }
}

fn ssh_host_key_inspection(
    host: &RouterSshHostRecord,
    observed: &ObservedSshHostKey,
) -> SshHostKeyInspection {
    let stored_fingerprint =
        normalized_optional_fingerprint(host.ssh_host_key_fingerprint.as_deref());
    let confirmation_required =
        stored_fingerprint.as_deref() != Some(observed.fingerprint.as_str());
    SshHostKeyInspection {
        host_id: host.id.clone(),
        endpoint: observed.target.clone(),
        changed: stored_fingerprint.is_some() && confirmation_required,
        confirmation_required,
        stored_fingerprint,
        observed_fingerprint: observed.fingerprint.clone(),
        observed_key_type: observed.key_type.clone(),
    }
}

fn require_expected_host_fingerprint(
    host: &RouterSshHostRecord,
    expected: Option<&str>,
) -> Result<(), AppError> {
    let current = normalized_optional_fingerprint(host.ssh_host_key_fingerprint.as_deref());
    if current.as_deref() != expected {
        return Err(AppError::coded_conflict(
            "SSH_HOST_KEY_STATE_CHANGED",
            "the stored SSH host fingerprint changed; scan the host again",
            serde_json::json!({ "storedFingerprint": current }),
        ));
    }
    Ok(())
}

async fn require_host_key_rotation_idle(
    state: &ServerState,
    host: &RouterSshHostRecord,
) -> Result<(), AppError> {
    if host_key_rotation_status_is_busy(&host.status)
        || state
            .store
            .client_market_host_has_active_job(&host.id)
            .await?
    {
        return Err(AppError::Conflict(
            "SSH host fingerprint cannot be changed while a Host operation is active".into(),
        ));
    }
    Ok(())
}

fn host_key_rotation_status_is_busy(status: &str) -> bool {
    matches!(
        status,
        "reserved" | HOST_STATUS_LOCKED | HOST_STATUS_DRAINING
    )
}

pub(crate) async fn prepare_web_terminal_host(
    state: &ServerState,
    authorized_host: &RouterSshHostRecord,
) -> Result<RouterSshHostRecord, AppError> {
    let _known_hosts_guard = PROVISION_KNOWN_HOSTS_LOCK.lock().await;
    let host = state
        .store
        .client_market_get_host(&authorized_host.id)
        .await?
        .ok_or_else(|| AppError::NotFound("host not found".into()))?;
    let observed = ssh_scan_host_key(
        &host.ip,
        host.port,
        host.ssh_host_key_fingerprint.as_deref(),
    )
    .await?;
    let inspection = ssh_host_key_inspection(&host, &observed);
    if inspection.confirmation_required {
        return Err(AppError::coded_conflict(
            "SSH_HOST_KEY_CONFIRMATION_REQUIRED",
            "SSH host identity confirmation is required before opening Web Terminal",
            serde_json::to_value(&inspection).unwrap_or_else(|_| serde_json::json!({})),
        ));
    }
    ensure_strict_known_host_entry(&state.config, &host, &observed).await?;
    Ok(host)
}

async fn ssh_scan_host_key(
    ip: &str,
    port: u16,
    stored_fingerprint: Option<&str>,
) -> Result<ObservedSshHostKey, AppError> {
    let mut command = Command::new("ssh-keyscan");
    command
        .arg("-T")
        .arg(SSH_HOST_KEY_SCAN_CONNECT_TIMEOUT_SECS)
        .arg("-p")
        .arg(port.to_string())
        .arg("-t")
        .arg("ed25519,ecdsa,rsa")
        .arg(ip)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(SSH_HOST_KEY_SCAN_TIMEOUT, command.output())
        .await
        .map_err(|_| AppError::ServiceUnavailable("SSH host key scan timed out".into()))?
        .map_err(|error| {
            AppError::ServiceUnavailable(format!("start SSH host key scan failed: {error}"))
        })?;
    if output.stdout.len() > 64 * 1024 || output.stderr.len() > 64 * 1024 {
        return Err(AppError::ServiceUnavailable(
            "SSH host key scan returned too much data".into(),
        ));
    }
    let stdout = String::from_utf8(output.stdout).map_err(|_| {
        AppError::ServiceUnavailable("SSH host key scan returned invalid text".into())
    })?;
    let target = ssh_known_hosts_target(ip, port);
    parse_ssh_keyscan_output(&stdout, &target, stored_fingerprint).ok_or_else(|| {
        warn!(
            exit_status = ?output.status.code(),
            stderr_bytes = output.stderr.len(),
            "SSH host key scan returned no supported key"
        );
        AppError::ServiceUnavailable(
            "SSH host key scan failed: the host returned no supported key".into(),
        )
    })
}

fn parse_ssh_keyscan_output(
    output: &str,
    target: &str,
    stored_fingerprint: Option<&str>,
) -> Option<ObservedSshHostKey> {
    let mut candidates = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(scanned_target) = fields.next() else {
            continue;
        };
        if !scanned_target
            .split(',')
            .any(|candidate| candidate == target)
        {
            continue;
        }
        let Some(key_type) = fields.next() else {
            continue;
        };
        let Some(encoded_key) = fields.next() else {
            continue;
        };
        if fields.next().is_some() {
            continue;
        }
        let priority = match key_type {
            "ssh-ed25519" => 0,
            value if value.starts_with("ecdsa-sha2-") => 1,
            "ssh-rsa" => 2,
            _ => continue,
        };
        let Ok(key_blob) = base64::engine::general_purpose::STANDARD.decode(encoded_key) else {
            continue;
        };
        if key_blob.is_empty() || key_blob.len() > 16 * 1024 {
            continue;
        }
        let digest = Sha256::digest(key_blob);
        let fingerprint = format!(
            "SHA256:{}",
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest)
        );
        candidates.push((
            priority,
            ObservedSshHostKey {
                target: target.to_string(),
                key_type: key_type.to_string(),
                encoded_key: encoded_key.to_string(),
                fingerprint,
            },
        ));
    }
    if let Some(stored_fingerprint) = normalized_optional_fingerprint(stored_fingerprint)
        && let Some((_, matching)) = candidates
            .iter()
            .find(|(_, candidate)| candidate.fingerprint == stored_fingerprint)
    {
        return Some(matching.clone());
    }
    candidates.sort_by_key(|(priority, _)| *priority);
    candidates.into_iter().next().map(|(_, key)| key)
}

#[derive(Debug)]
struct KnownHostsSnapshot {
    existed: bool,
    bytes: Vec<u8>,
}

fn read_known_hosts_snapshot(path: &Path) -> Result<KnownHostsSnapshot, AppError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                return Err(AppError::Internal(
                    "Client Market known_hosts path is not a regular file".into(),
                ));
            }
            Ok(KnownHostsSnapshot {
                existed: true,
                bytes: fs::read(path).map_err(|error| {
                    AppError::Internal(format!("read Client Market known_hosts failed: {error}"))
                })?,
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(KnownHostsSnapshot {
            existed: false,
            bytes: Vec::new(),
        }),
        Err(error) => Err(AppError::Internal(format!(
            "inspect Client Market known_hosts failed: {error}"
        ))),
    }
}

fn atomic_write_known_hosts(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    let parent = path.parent().ok_or_else(|| {
        AppError::Internal("Client Market known_hosts path has no parent directory".into())
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        AppError::Internal(format!(
            "create Client Market known_hosts directory failed: {error}"
        ))
    })?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("known_hosts");
    let temporary = parent.join(format!(".{name}.{}.tmp", Uuid::new_v4()));
    let result = (|| -> Result<(), AppError> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| {
                AppError::Internal(format!("create known_hosts temporary file failed: {error}"))
            })?;
        file.write_all(bytes).map_err(|error| {
            AppError::Internal(format!("write known_hosts temporary file failed: {error}"))
        })?;
        file.sync_all().map_err(|error| {
            AppError::Internal(format!("sync known_hosts temporary file failed: {error}"))
        })?;
        drop(file);
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600)).map_err(|error| {
            AppError::Internal(format!("secure known_hosts temporary file failed: {error}"))
        })?;
        fs::rename(&temporary, path).map_err(|error| {
            AppError::Internal(format!("replace Client Market known_hosts failed: {error}"))
        })?;
        if let Err(error) = fs::File::open(parent).and_then(|directory| directory.sync_all()) {
            warn!(%error, "failed to sync Client Market known_hosts directory");
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn ssh_keygen_backup_path(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(".old");
    PathBuf::from(value)
}

struct TemporaryKnownHostsPath {
    path: PathBuf,
}

impl TemporaryKnownHostsPath {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryKnownHostsPath {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(ssh_keygen_backup_path(&self.path));
    }
}

async fn install_known_host_entry(
    path: &Path,
    observed: &ObservedSshHostKey,
) -> Result<KnownHostsSnapshot, AppError> {
    let snapshot = read_known_hosts_snapshot(path)?;
    let parent = path.parent().ok_or_else(|| {
        AppError::Internal("Client Market known_hosts path has no parent directory".into())
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        AppError::Internal(format!(
            "create Client Market known_hosts directory failed: {error}"
        ))
    })?;
    let candidate = TemporaryKnownHostsPath::new(
        parent.join(format!(".known_hosts.rotate.{}.tmp", Uuid::new_v4())),
    );
    atomic_write_known_hosts(candidate.path(), &snapshot.bytes)?;
    let output = Command::new("ssh-keygen")
        .arg("-R")
        .arg(&observed.target)
        .arg("-f")
        .arg(candidate.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| AppError::Internal(format!("start ssh-keygen failed: {error}")))?;
    if !output.status.success() {
        return Err(AppError::Internal(format!(
            "remove stale known_hosts entry failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let mut next = fs::read(candidate.path()).map_err(|error| {
        AppError::Internal(format!(
            "read rotated known_hosts candidate failed: {error}"
        ))
    })?;
    if !next.is_empty() && !next.ends_with(b"\n") {
        next.push(b'\n');
    }
    next.extend_from_slice(
        format!(
            "{} {} {}\n",
            observed.target, observed.key_type, observed.encoded_key
        )
        .as_bytes(),
    );
    atomic_write_known_hosts(path, &next)?;
    Ok(snapshot)
}

fn restore_known_hosts_snapshot(
    path: &Path,
    snapshot: &KnownHostsSnapshot,
) -> Result<(), AppError> {
    if snapshot.existed {
        return atomic_write_known_hosts(path, &snapshot.bytes);
    }
    match fs::remove_file(path) {
        Ok(()) => {
            if let Some(parent) = path.parent()
                && let Err(error) =
                    fs::File::open(parent).and_then(|directory| directory.sync_all())
            {
                warn!(%error, "failed to sync restored Client Market known_hosts directory");
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppError::Internal(format!(
            "remove newly-created Client Market known_hosts failed: {error}"
        ))),
    }
}

async fn ensure_strict_known_host_entry(
    config: &Config,
    host: &RouterSshHostRecord,
    observed: &ObservedSshHostKey,
) -> Result<(), AppError> {
    let known_hosts = known_hosts_path(config);
    if ssh_fetch_host_fingerprint(&host.ip, host.port, &known_hosts)
        .await
        .is_ok_and(|fingerprint| fingerprint == observed.fingerprint)
    {
        return Ok(());
    }
    install_known_host_entry(&known_hosts, observed).await?;
    Ok(())
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
    ssh_run_remote_with_input_connect_timeout(
        key_path,
        known_hosts,
        ip,
        port,
        remote_command,
        stdin,
        timeout,
        host_key_policy,
        30,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn ssh_run_remote_with_input_connect_timeout(
    key_path: &Path,
    known_hosts: &Path,
    ip: &str,
    port: u16,
    remote_command: &str,
    stdin: Option<Vec<u8>>,
    timeout: Duration,
    host_key_policy: SshHostKeyPolicy,
    connect_timeout_secs: u64,
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
        .arg(format!("ConnectTimeout={connect_timeout_secs}"))
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
        daily_rate_minor: host.daily_rate_minor,
        currency: host.currency,
        free_duration_days: host.free_duration_days,
        offer_revision: host.offer_revision,
        payment_method_kinds: host.payment_method_kinds,
        contacts: host.contacts,
        seller_approval_required: false,
        eligibility: crate::market_access::MarketEligibilityView::allowed(),
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
        can_control_recovery: false,
        can_retire_unreachable: false,
        recovery: None,
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
    pub daily_rate_minor: Option<i64>,
    pub currency: Option<String>,
    pub free_duration_days: Option<u32>,
    pub offer_revision: i64,
    pub payment_method_kinds: Vec<String>,
    pub contacts: Vec<crate::client_market_trade::PaymentContact>,
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
    pub started_at: Option<String>,
    pub heartbeat_at: Option<String>,
    pub deadline_at: Option<String>,
    pub worker_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CleanupRecoveryClaim {
    host_id: String,
    installation_id: String,
    attempt_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QuarantinedHostReprobeClaim {
    host_id: String,
    attempt_count: u32,
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

/// Host ownership for market operations is bound to the stable Provider user id.
pub(crate) fn session_is_host_owner(
    session: &crate::models::AuthSession,
    provider_id: Option<&str>,
) -> bool {
    provider_id == Some(session.user_id.as_str())
}

pub(crate) fn host_is_unallocated_for_terminal(host: &RouterSshHostRecord) -> bool {
    host.installation_id.is_none()
        && host.client_owner_user_id.is_none()
        && matches!(host.status.as_str(), "idle" | "abnormal" | "unreachable")
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
        let conn = self.conn.lock().await;
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
        let conn = self.conn.lock().await;
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
        let conn = self.conn.lock().await;
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
            status: "completed".into(),
            imported,
            skipped,
            failed,
            items,
        })
    }

    async fn client_market_host_import_job(
        &self,
        job_id: &str,
        session: &crate::models::AuthSession,
    ) -> Result<HostImportResponse, AppError> {
        let conn = self.conn.lock().await;
        let (provider_id, status): (String, String) = conn
            .query_row(
                "SELECT provider_id, status FROM client_market_host_import_jobs WHERE id = ?1",
                params![job_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("read Host import job failed: {error}")))?
            .ok_or_else(|| AppError::NotFound("Host import job not found".into()))?;
        if provider_id != session.user_id {
            return Err(AppError::Forbidden(
                "Host import job belongs to another account".into(),
            ));
        }
        let items = conn
            .prepare(
                "SELECT ip, port, status, host_id, error_message
                 FROM client_market_host_import_items
                 WHERE job_id = ?1 ORDER BY position ASC",
            )
            .and_then(|mut statement| {
                statement
                    .query_map(params![job_id], |row| {
                        Ok(HostImportItemResult {
                            ip: row.get(0)?,
                            port: row.get(1)?,
                            status: row.get(2)?,
                            host_id: row.get(3)?,
                            error: row.get(4)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(|error| {
                AppError::Internal(format!("read Host import items failed: {error}"))
            })?;
        let imported = items
            .iter()
            .filter(|item| item.status == "imported")
            .count();
        let skipped = items.iter().filter(|item| item.status == "skipped").count();
        let failed = items.iter().filter(|item| item.status == "failed").count();
        Ok(HostImportResponse {
            job_id: job_id.to_string(),
            status,
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
                    s.client_user_id, h.provider_id, h.daily_rate_minor, h.offer_revision,
                    COALESCE((SELECT methods_json FROM account_payment_profiles p
                              WHERE p.user_id = h.provider_id), '[]'),
                    COALESCE((SELECT contacts_json FROM account_payment_profiles p
                              WHERE p.user_id = h.provider_id), '[]'),
                    NULLIF(TRIM(h.currency), ''), h.free_duration_days
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
        let params: Vec<&dyn crate::db::ToSql> = binds
            .iter()
            .map(|value| value as &dyn crate::db::ToSql)
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
        daily_rate_minor: Option<i64>,
        currency: Option<&str>,
        free_duration_days: Option<u32>,
    ) -> Result<RouterSshHostRecord, AppError> {
        let owner = normalize_market_email(owner_email)?;
        let daily_rate_minor = crate::client_market_trade::validate_offer(daily_rate_minor)?;
        let free_duration_days = crate::client_market_trade::validate_free_duration_days(
            daily_rate_minor,
            free_duration_days,
        )?;
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4().to_string();
        let conn = self.conn.lock().await;
        if daily_rate_minor.is_some() {
            crate::market_billing::require_supplier_profile_tx(
                &conn,
                provider_id,
                currency
                    .ok_or_else(|| AppError::Internal("paid Host currency is missing".into()))?,
            )?;
        }
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
                daily_rate_minor, currency, free_duration_days, offer_revision, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10, NULL, ?11, ?12,
                       ?13, ?14, ?15, 1, ?10, ?10)",
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
                daily_rate_minor,
                currency,
                free_duration_days.map(i64::from),
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
        if !session_is_host_owner(session, host.provider_id.as_deref()) {
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

    pub async fn client_market_retire_unreachable_host(
        &self,
        id: &str,
        session: &crate::models::AuthSession,
    ) -> Result<RetireUnreachableHostResponse, AppError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin unreachable Host retirement failed: {error}"))
            })?;
        type RetirementRow = (
            String,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
            Option<String>,
            Option<i64>,
            Option<String>,
            Option<String>,
            Option<String>,
        );
        let row: RetirementRow = tx
            .query_row(
                "SELECT h.status, h.installation_id, h.provider_id, h.host_owner_email,
                        i.provision_source, i.provision_host_id, t.enabled, t.subdomain,
                        s.status, s.host_id
                 FROM router_ssh_hosts h
                 LEFT JOIN installations i ON i.id = h.installation_id
                 LEFT JOIN installation_client_tunnels t ON t.installation_id = h.installation_id
                 LEFT JOIN client_market_subscriptions s ON s.installation_id = h.installation_id
                 WHERE h.id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("load unreachable Host retirement failed: {error}"))
            })?
            .ok_or_else(|| AppError::NotFound("host not found".into()))?;
        let (
            host_status,
            installation_id,
            provider_id,
            _host_owner_email,
            provision_source,
            provision_host_id,
            tunnel_enabled,
            subdomain,
            subscription_status,
            subscription_host_id,
        ) = row;
        if !session_is_host_owner(session, provider_id.as_deref()) {
            return Err(AppError::Forbidden(
                "only the Host Provider may permanently remove this lost Host".into(),
            ));
        }
        if host_status != HOST_STATUS_UNREACHABLE {
            return Err(AppError::Conflict(
                "only an unreachable Host can be permanently removed without SSH cleanup".into(),
            ));
        }
        let installation_id = installation_id.ok_or_else(|| {
            AppError::Conflict("unreachable Host no longer has an installation binding".into())
        })?;
        if provision_source.as_deref() != Some(PROVISION_SOURCE_ROUTER_MARKET)
            || provision_host_id.as_deref() != Some(id)
        {
            return Err(AppError::Conflict(
                "Host installation binding is not managed by Client Market".into(),
            ));
        }
        if tunnel_enabled != Some(0) {
            return Err(AppError::Conflict(
                "Client tunnel must be disabled before permanently removing the Host".into(),
            ));
        }
        let subscription_status = subscription_status.ok_or_else(|| {
            AppError::Conflict("Host has no Client Market subscription to finalize".into())
        })?;
        if subscription_host_id.as_deref() != Some(id)
            || !matches!(
                subscription_status.as_str(),
                "releasing" | "release_failed" | "released"
            )
        {
            return Err(AppError::Conflict(
                "Host subscription has not entered the release state".into(),
            ));
        }
        let active_jobs: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM provisioning_jobs
                 WHERE host_id = ?1 AND status IN ('pending', 'running')",
                params![id],
                |row| row.get(0),
            )
            .map_err(|error| {
                AppError::Internal(format!("check active Host jobs failed: {error}"))
            })?;
        if active_jobs > 0 {
            return Err(AppError::Conflict(
                "Host still has an active job; wait for it to finish before permanent removal"
                    .into(),
            ));
        }

        let now = Utc::now();
        let now_text = now.to_rfc3339();
        crate::market_billing::terminate_contract_tx(
            &tx,
            "client_host",
            &installation_id,
            "unreachable_host_retired",
            &now_text,
        )?;
        let subscription_changed = tx
            .execute(
                "UPDATE client_market_subscriptions
                 SET status = 'released', released_at = COALESCE(released_at, ?3), updated_at = ?3
                 WHERE installation_id = ?1 AND host_id = ?2
                   AND status IN ('releasing', 'release_failed', 'released')",
                params![installation_id, id, now_text],
            )
            .map_err(|error| {
                AppError::Internal(format!(
                    "finalize retired Host subscription failed: {error}"
                ))
            })?;
        if subscription_changed != 1 {
            return Err(AppError::Conflict(
                "Host subscription changed concurrently; retry".into(),
            ));
        }
        crate::client_market_trade::insert_audit_tx(
            &tx,
            Some(&installation_id),
            Some(id),
            Some(&session.user_id),
            Some(&session.email),
            "unreachable_host_retired",
            serde_json::json!({
                "previousStatus": subscription_status,
                "remoteCleanupBypassed": true,
            }),
            now,
        )?;
        crate::public_hosts::tombstone_subject(
            &tx,
            crate::namespace::PublicHostKind::Client,
            &installation_id,
        )
        .map_err(|error| {
            AppError::Internal(format!("tombstone retired Client route failed: {error}"))
        })?;
        crate::store::purge_installation_data_tx(&tx, &installation_id)?;
        let host_deleted = tx
            .execute(
                "DELETE FROM router_ssh_hosts
                 WHERE id = ?1 AND status = 'unreachable' AND installation_id = ?2",
                params![id, installation_id],
            )
            .map_err(|error| {
                AppError::Internal(format!("delete retired unreachable Host failed: {error}"))
            })?;
        if host_deleted != 1 {
            return Err(AppError::Conflict(
                "unreachable Host changed concurrently; retry".into(),
            ));
        }
        tx.commit().map_err(|error| {
            AppError::Internal(format!(
                "commit unreachable Host retirement failed: {error}"
            ))
        })?;
        Ok(RetireUnreachableHostResponse {
            host_id: id.to_string(),
            installation_id,
            previous_subscription_status: subscription_status,
            status: "retired".into(),
            subdomain,
        })
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
        let conn = self.conn.lock().await;
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
                    status, phase, log_blob, secret_ref, failure_code, created_at, updated_at,
                    started_at, heartbeat_at, deadline_at, worker_id
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
                        status, phase, log_blob, secret_ref, failure_code, created_at, updated_at,
                        started_at, heartbeat_at, deadline_at, worker_id
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

    async fn client_market_claim_expired_job_leases(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<ProvisioningJobRecord>, AppError> {
        let stale_before = now
            - chrono::Duration::from_std(JOB_HEARTBEAT_STALE_AFTER).map_err(|error| {
                AppError::Internal(format!("invalid heartbeat window: {error}"))
            })?;
        let watchdog_stale_before = now
            - chrono::Duration::from_std(JOB_WATCHDOG_STALE_AFTER).map_err(|error| {
                AppError::Internal(format!("invalid watchdog finalizer window: {error}"))
            })?;
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin stale job claim failed: {error}"))
            })?;
        let candidates = {
            let mut statement = tx
                .prepare(
                    "SELECT id, type, host_id, host_owner_email, client_owner_email,
                            selection_owners_json, selection_regions_json, subdomain, installation_id,
                            status, phase, log_blob, secret_ref, failure_code, created_at, updated_at,
                            started_at, heartbeat_at, deadline_at, worker_id
                     FROM provisioning_jobs
                     WHERE status = 'running' AND worker_id IS NOT NULL
                       AND (
                         (worker_id NOT LIKE 'watchdog:%'
                          AND ((deadline_at IS NOT NULL AND deadline_at <= ?1)
                               OR (heartbeat_at IS NOT NULL AND heartbeat_at <= ?2)))
                         OR (worker_id LIKE 'watchdog:%'
                             AND COALESCE(heartbeat_at, updated_at) <= ?3)
                       )
                     ORDER BY CASE WHEN worker_id LIKE 'watchdog:%'
                                   THEN COALESCE(heartbeat_at, updated_at)
                                   ELSE COALESCE(deadline_at, heartbeat_at) END,
                              created_at
                     LIMIT 32",
                )
                .map_err(|error| AppError::Internal(format!("prepare stale jobs failed: {error}")))?;
            statement
                .query_map(
                    params![
                        now.to_rfc3339(),
                        stale_before.to_rfc3339(),
                        watchdog_stale_before.to_rfc3339()
                    ],
                    map_provisioning_job_row,
                )
                .map_err(|error| AppError::Internal(format!("query stale jobs failed: {error}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::Internal(format!("read stale jobs failed: {error}")))?
        };
        let mut claimed = Vec::with_capacity(candidates.len());
        for job in candidates {
            let previous_worker = job.worker_id.as_deref().unwrap_or_default();
            let watchdog_worker = format!("watchdog:{}", Uuid::new_v4());
            let changed = tx
                .execute(
                    "UPDATE provisioning_jobs
                     SET worker_id = ?3, heartbeat_at = ?4, updated_at = ?4
                     WHERE id = ?1 AND status = 'running' AND worker_id = ?2
                       AND (
                         (worker_id NOT LIKE 'watchdog:%'
                          AND ((deadline_at IS NOT NULL AND deadline_at <= ?4)
                               OR (heartbeat_at IS NOT NULL AND heartbeat_at <= ?5)))
                         OR (worker_id LIKE 'watchdog:%'
                             AND COALESCE(heartbeat_at, updated_at) <= ?6)
                       )",
                    params![
                        job.id,
                        previous_worker,
                        watchdog_worker,
                        now.to_rfc3339(),
                        stale_before.to_rfc3339(),
                        watchdog_stale_before.to_rfc3339(),
                    ],
                )
                .map_err(|error| AppError::Internal(format!("claim stale job failed: {error}")))?;
            if changed == 1 {
                claimed.push(job);
            }
        }
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit stale job claims failed: {error}"))
        })?;
        Ok(claimed)
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

    pub(crate) async fn client_market_claim_job_execution(
        &self,
        job_id: &str,
        expected_type: &str,
        worker_id: &str,
        resume_running: bool,
        max_runtime: Duration,
    ) -> Result<(), AppError> {
        let now = Utc::now();
        let deadline = now
            + chrono::Duration::from_std(max_runtime)
                .map_err(|error| AppError::Internal(format!("invalid job runtime: {error}")))?;
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                "UPDATE provisioning_jobs
                 SET status = 'running', started_at = COALESCE(started_at, ?5),
                     heartbeat_at = ?5, deadline_at = ?6, worker_id = ?3, updated_at = ?5
                 WHERE id = ?1 AND type = ?2
                   AND (status = 'pending' OR (?4 = 1 AND status = 'running'))",
                params![
                    job_id,
                    expected_type,
                    worker_id,
                    i64::from(resume_running),
                    now.to_rfc3339(),
                    deadline.to_rfc3339(),
                ],
            )
            .map_err(|error| AppError::Internal(format!("claim job execution failed: {error}")))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "provisioning job cannot be claimed for execution".into(),
            ));
        }
        Ok(())
    }

    pub(crate) async fn client_market_heartbeat_job(
        &self,
        job_id: &str,
        worker_id: &str,
    ) -> Result<bool, AppError> {
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                "UPDATE provisioning_jobs SET heartbeat_at = ?3, updated_at = ?3
                 WHERE id = ?1 AND status = 'running' AND worker_id = ?2
                   AND deadline_at > ?3",
                params![job_id, worker_id, Utc::now().to_rfc3339()],
            )
            .map_err(|error| AppError::Internal(format!("heartbeat job failed: {error}")))?;
        Ok(changed == 1)
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
        let conn = self.conn.lock().await;
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
        let client_identity: (Option<String>, String) = tx
            .query_row(
                "SELECT client_owner_user_id, client_owner_email
                 FROM provisioning_jobs WHERE id = ?1",
                params![job_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| AppError::Internal(format!("read claim client identity failed: {e}")))?;
        let client_user_id = client_identity.0.ok_or_else(|| {
            AppError::Conflict("provisioning job has no authenticated client owner".into())
        })?;
        let sql = format!(
            "SELECT id, provider_id FROM router_ssh_hosts
             WHERE status = '{HOST_STATUS_IDLE}'
               AND daily_rate_minor IS NULL
               AND host_owner_email IN ({owner_placeholders})
               AND country_code IN ({region_placeholders})
             ORDER BY RANDOM()"
        );
        let mut query = tx
            .prepare(&sql)
            .map_err(|e| AppError::Internal(format!("prepare claim host failed: {e}")))?;
        let mut values: Vec<&dyn crate::db::ToSql> = Vec::new();
        for owner in owners {
            values.push(owner);
        }
        for region in regions {
            values.push(region);
        }
        let candidates = query
            .query_map(values.as_slice(), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .map_err(|e| AppError::Internal(format!("select idle hosts failed: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Internal(format!("read idle hosts failed: {e}")))?;
        drop(query);
        let mut host_id = None;
        for (candidate_id, provider_id) in candidates {
            let Some(provider_id) = provider_id else {
                continue;
            };
            if crate::market_access::product_access_allowed_tx(
                &tx,
                &provider_id,
                &client_user_id,
                &client_identity.1,
                crate::market_access::PRODUCT_CLIENT_HOST,
                crate::market_access::PRICING_FREE,
            )? {
                host_id = Some(candidate_id);
                break;
            }
        }
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
                        s.client_user_id, h.provider_id, h.daily_rate_minor, h.offer_revision,
                        COALESCE((SELECT methods_json FROM account_payment_profiles p
                                  WHERE p.user_id = h.provider_id), '[]'),
                        COALESCE((SELECT contacts_json FROM account_payment_profiles p
                                  WHERE p.user_id = h.provider_id), '[]'),
                        NULLIF(TRIM(h.currency), ''), h.free_duration_days
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

    async fn client_market_retirable_host_ids(&self) -> Result<HashSet<String>, AppError> {
        let conn = self.conn.lock().await;
        let mut statement = conn
            .prepare(
                "SELECT h.id
                 FROM router_ssh_hosts h
                 JOIN installations i ON i.id = h.installation_id
                 JOIN installation_client_tunnels t ON t.installation_id = i.id
                 JOIN client_market_subscriptions s ON s.installation_id = i.id
                 WHERE h.status = 'unreachable'
                   AND i.provision_source = ?1 AND i.provision_host_id = h.id
                   AND t.enabled = 0 AND s.host_id = h.id
                   AND s.status IN ('releasing', 'release_failed', 'released')
                   AND NOT EXISTS (
                       SELECT 1 FROM provisioning_jobs j
                       WHERE j.host_id = h.id AND j.status IN ('pending', 'running')
                   )",
            )
            .map_err(|error| {
                AppError::Internal(format!("prepare retirable Host list failed: {error}"))
            })?;
        let rows = statement
            .query_map(params![PROVISION_SOURCE_ROUTER_MARKET], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|error| {
                AppError::Internal(format!("query retirable Host list failed: {error}"))
            })?;
        rows.collect::<Result<HashSet<_>, _>>().map_err(|error| {
            AppError::Internal(format!("read retirable Host list failed: {error}"))
        })
    }

    async fn client_market_claim_due_cleanup_recoveries(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<CleanupRecoveryClaim>, AppError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin cleanup recovery claim failed: {error}"))
            })?;
        tx.execute(
            "DELETE FROM client_market_cleanup_recovery_state
             WHERE NOT EXISTS (
                 SELECT 1
                 FROM router_ssh_hosts h
                 JOIN installations i ON i.id = h.installation_id
                 JOIN installation_client_tunnels t ON t.installation_id = i.id
                 JOIN client_market_subscriptions s ON s.installation_id = i.id
                 WHERE h.id = client_market_cleanup_recovery_state.host_id
                   AND i.id = client_market_cleanup_recovery_state.installation_id
                   AND h.status = 'unreachable' AND t.enabled = 0
                   AND s.host_id = h.id
                   AND s.status IN ('releasing', 'release_failed', 'released')
             )",
            [],
        )
        .map_err(|error| {
            AppError::Internal(format!("prune cleanup recovery state failed: {error}"))
        })?;
        let now_text = now.to_rfc3339();
        tx.execute(
            "UPDATE client_market_cleanup_recovery_state
             SET next_attempt_at = NULL, stopped_at = ?1,
                 last_outcome = 'probe_interrupted', updated_at = ?1
             WHERE stopped_at IS NULL AND attempt_count >= 5
               AND last_outcome = 'probing'
               AND next_attempt_at IS NOT NULL AND next_attempt_at <= ?1",
            params![now_text],
        )
        .map_err(|error| {
            AppError::Internal(format!("stop interrupted cleanup recovery failed: {error}"))
        })?;
        let claim_lease_until = chrono::Duration::from_std(CLEANUP_RECOVERY_CLAIM_LEASE)
            .ok()
            .map(|lease| (now + lease).to_rfc3339())
            .expect("cleanup recovery claim lease is valid");
        let due = {
            let mut statement = tx
                .prepare(
                    "SELECT r.host_id, r.installation_id, r.attempt_count
                     FROM client_market_cleanup_recovery_state r
                     JOIN router_ssh_hosts h ON h.id = r.host_id
                     JOIN installation_client_tunnels t
                       ON t.installation_id = r.installation_id
                     JOIN client_market_subscriptions s
                       ON s.installation_id = r.installation_id
                     WHERE r.stopped_at IS NULL
                       AND r.next_attempt_at IS NOT NULL AND r.next_attempt_at <= ?1
                       AND r.attempt_count < 5
                       AND h.status = 'unreachable'
                       AND h.installation_id = r.installation_id
                       AND t.enabled = 0
                       AND s.host_id = h.id
                       AND s.status IN ('releasing', 'release_failed', 'released')
                       AND NOT EXISTS (
                           SELECT 1 FROM provisioning_jobs j
                           WHERE j.host_id = h.id AND j.status IN ('pending', 'running')
                       )
                     ORDER BY r.next_attempt_at, r.host_id
                     LIMIT ?2",
                )
                .map_err(|error| {
                    AppError::Internal(format!("prepare cleanup recovery claims failed: {error}"))
                })?;
            let rows = statement
                .query_map(params![now_text, limit as i64], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .map_err(|error| {
                    AppError::Internal(format!("query cleanup recovery claims failed: {error}"))
                })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(|error| {
                AppError::Internal(format!("read cleanup recovery claims failed: {error}"))
            })?
        };
        let mut claimed = Vec::with_capacity(due.len());
        for (host_id, installation_id, previous_attempt_count) in due {
            let next_attempt_count = previous_attempt_count.saturating_add(1).min(5);
            let changed = tx
                .execute(
                    "UPDATE client_market_cleanup_recovery_state
                     SET attempt_count = ?4, next_attempt_at = ?6,
                         last_attempt_at = ?3, last_outcome = 'probing', updated_at = ?3
                     WHERE host_id = ?1 AND installation_id = ?2
                       AND attempt_count = ?5 AND stopped_at IS NULL
                       AND next_attempt_at IS NOT NULL AND next_attempt_at <= ?3",
                    params![
                        host_id,
                        installation_id,
                        now_text,
                        next_attempt_count,
                        previous_attempt_count,
                        claim_lease_until,
                    ],
                )
                .map_err(|error| {
                    AppError::Internal(format!("claim cleanup recovery failed: {error}"))
                })?;
            if changed == 1 {
                claimed.push(CleanupRecoveryClaim {
                    host_id,
                    installation_id,
                    attempt_count: u32::try_from(next_attempt_count).unwrap_or(5),
                });
            }
        }
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit cleanup recovery claims failed: {error}"))
        })?;
        Ok(claimed)
    }

    async fn client_market_finish_cleanup_recovery_attempt(
        &self,
        claim: &CleanupRecoveryClaim,
        outcome: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        let outcome = crate::store::client_chat::sanitize_system_event_text(outcome)
            .chars()
            .take(500)
            .collect::<String>();
        let next_attempt_at = (!cleanup_recovery_requires_manual_intervention(&outcome))
            .then(|| cleanup_recovery_next_at(now, claim.attempt_count))
            .flatten();
        let stopped_at = next_attempt_at.is_none().then(|| now.to_rfc3339());
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                "UPDATE client_market_cleanup_recovery_state
             SET next_attempt_at = ?4, last_outcome = ?5, stopped_at = ?6, updated_at = ?7
             WHERE host_id = ?1 AND installation_id = ?2 AND attempt_count = ?3
               AND last_outcome = 'probing' AND stopped_at IS NULL",
                params![
                    claim.host_id,
                    claim.installation_id,
                    i64::from(claim.attempt_count),
                    next_attempt_at.map(|value| value.to_rfc3339()),
                    outcome,
                    stopped_at,
                    now.to_rfc3339(),
                ],
            )
            .map_err(|error| {
                AppError::Internal(format!("finish cleanup recovery attempt failed: {error}"))
            })?;
        if changed == 0 {
            tracing::debug!(
                host_id = %claim.host_id,
                installation_id = %claim.installation_id,
                attempt = claim.attempt_count,
                "discarded stale cleanup recovery result"
            );
        }
        Ok(())
    }

    async fn client_market_claim_due_quarantined_host_reprobes(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<QuarantinedHostReprobeClaim>, AppError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let now_text = now.to_rfc3339();
        let lease_until = (now
            + chrono::Duration::from_std(HOST_REPROBE_CLAIM_LEASE).map_err(|error| {
                AppError::Internal(format!("invalid Host reprobe lease: {error}"))
            })?)
        .to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!(
                    "begin quarantined Host reprobe claim failed: {error}"
                ))
            })?;
        tx.execute(
            "DELETE FROM client_market_host_reprobe_state
             WHERE NOT EXISTS (
                 SELECT 1 FROM router_ssh_hosts h
                 WHERE h.id = client_market_host_reprobe_state.host_id
                   AND h.installation_id IS NULL
                   AND h.status IN ('unreachable', 'abnormal')
             )",
            [],
        )
        .map_err(|error| AppError::Internal(format!("prune Host reprobe state failed: {error}")))?;
        tx.execute(
            "INSERT OR IGNORE INTO client_market_host_reprobe_state
                (host_id, attempt_count, next_attempt_at, lease_until, last_outcome, updated_at)
             SELECT h.id, 0, ?1, NULL, NULL, ?1
             FROM router_ssh_hosts h
             WHERE h.installation_id IS NULL
               AND h.status IN ('unreachable', 'abnormal')
               AND NOT EXISTS (
                   SELECT 1 FROM provisioning_jobs j
                   WHERE j.host_id = h.id AND j.status IN ('pending', 'running')
               )",
            params![now_text],
        )
        .map_err(|error| AppError::Internal(format!("seed Host reprobe state failed: {error}")))?;
        let due = {
            let mut statement = tx
                .prepare(
                    "SELECT r.host_id, r.attempt_count
                     FROM client_market_host_reprobe_state r
                     JOIN router_ssh_hosts h ON h.id = r.host_id
                     WHERE h.installation_id IS NULL
                       AND h.status IN ('unreachable', 'abnormal')
                       AND r.next_attempt_at IS NOT NULL AND r.next_attempt_at <= ?1
                       AND (r.lease_until IS NULL OR r.lease_until <= ?1)
                       AND NOT EXISTS (
                           SELECT 1 FROM provisioning_jobs j
                           WHERE j.host_id = h.id AND j.status IN ('pending', 'running')
                       )
                     ORDER BY r.next_attempt_at, r.host_id LIMIT ?2",
                )
                .map_err(|error| {
                    AppError::Internal(format!("prepare Host reprobes failed: {error}"))
                })?;
            statement
                .query_map(params![now_text, limit as i64], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(|error| {
                    AppError::Internal(format!("query Host reprobes failed: {error}"))
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    AppError::Internal(format!("read Host reprobes failed: {error}"))
                })?
        };
        let mut claimed = Vec::with_capacity(due.len());
        for (host_id, previous_attempts) in due {
            let next_attempt = previous_attempts.saturating_add(1);
            let changed = tx
                .execute(
                    "UPDATE client_market_host_reprobe_state
                     SET attempt_count = ?2, lease_until = ?3, last_outcome = 'probing', updated_at = ?4
                     WHERE host_id = ?1 AND attempt_count = ?5
                       AND next_attempt_at IS NOT NULL AND next_attempt_at <= ?4
                       AND (lease_until IS NULL OR lease_until <= ?4)",
                    params![host_id, next_attempt, lease_until, now_text, previous_attempts],
                )
                .map_err(|error| AppError::Internal(format!("claim Host reprobe failed: {error}")))?;
            if changed == 1 {
                claimed.push(QuarantinedHostReprobeClaim {
                    host_id,
                    attempt_count: u32::try_from(next_attempt).unwrap_or(u32::MAX),
                });
            }
        }
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit Host reprobe claims failed: {error}"))
        })?;
        Ok(claimed)
    }

    async fn client_market_finish_quarantined_host_reprobe(
        &self,
        claim: &QuarantinedHostReprobeClaim,
        outcome: &str,
        retry: bool,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        let outcome = crate::store::client_chat::sanitize_system_event_text(outcome)
            .chars()
            .take(500)
            .collect::<String>();
        let next_attempt_at =
            retry.then(|| quarantined_host_reprobe_next_at(now, claim.attempt_count).to_rfc3339());
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE client_market_host_reprobe_state
             SET next_attempt_at = ?3, lease_until = NULL, last_outcome = ?4, updated_at = ?5
             WHERE host_id = ?1 AND attempt_count = ?2 AND last_outcome = 'probing'",
            params![
                claim.host_id,
                i64::from(claim.attempt_count),
                next_attempt_at,
                outcome,
                now.to_rfc3339(),
            ],
        )
        .map_err(|error| AppError::Internal(format!("finish Host reprobe failed: {error}")))?;
        Ok(())
    }

    async fn client_market_finalize_clean_unreachable_host(
        &self,
        host_id: &str,
        installation_id: &str,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin clean Host recovery failed: {error}"))
            })?;
        let eligible: Option<i64> = tx
            .query_row(
                "SELECT 1
                 FROM router_ssh_hosts h
                 JOIN installations i ON i.id = h.installation_id
                 JOIN installation_client_tunnels t ON t.installation_id = i.id
                 WHERE h.id = ?1 AND i.id = ?2
                   AND h.status = 'unreachable' AND t.enabled = 0
                   AND i.provision_source = ?3 AND i.provision_host_id = h.id
                   AND NOT EXISTS (
                       SELECT 1 FROM provisioning_jobs j
                       WHERE j.host_id = h.id AND j.status IN ('pending', 'running')
                   )",
                params![host_id, installation_id, PROVISION_SOURCE_ROUTER_MARKET],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("validate clean Host recovery failed: {error}"))
            })?;
        if eligible.is_none() {
            return Err(AppError::Conflict(
                "unreachable Host changed before cleanup recovery completed".into(),
            ));
        }
        let has_subscription: i64 = tx
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM client_market_subscriptions
                    WHERE installation_id = ?1 AND host_id = ?2
                      AND status IN ('releasing', 'release_failed', 'released')
                 )",
                params![installation_id, host_id],
                |row| row.get(0),
            )
            .map_err(|error| {
                AppError::Internal(format!("check clean Host subscription failed: {error}"))
            })?;
        if has_subscription != 0 {
            crate::client_market_trade::cleanup_finished_tx(
                &tx,
                installation_id,
                host_id,
                Utc::now(),
            )?;
        }
        crate::public_hosts::tombstone_subject(
            &tx,
            crate::namespace::PublicHostKind::Client,
            installation_id,
        )
        .map_err(|error| {
            AppError::Internal(format!("tombstone recovered Client route failed: {error}"))
        })?;
        crate::store::purge_installation_data_tx(&tx, installation_id)?;
        let now = Utc::now().to_rfc3339();
        let changed = tx
            .execute(
                "UPDATE router_ssh_hosts
                 SET status = 'idle', installation_id = NULL, last_error = NULL, updated_at = ?3
                 WHERE id = ?1 AND status = 'unreachable' AND installation_id = ?2",
                params![host_id, installation_id, now],
            )
            .map_err(|error| {
                AppError::Internal(format!(
                    "return clean recovered Host to idle failed: {error}"
                ))
            })?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "unreachable Host changed while cleanup recovery completed".into(),
            ));
        }
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit clean Host recovery failed: {error}"))
        })?;
        Ok(())
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
        if !session_is_host_owner(session, host.provider_id.as_deref()) {
            return Err(AppError::Forbidden(
                "not allowed to operate this host".into(),
            ));
        }
        Ok(host)
    }

    async fn client_market_rotate_host_fingerprint(
        &self,
        id: &str,
        rotation: HostFingerprintRotation<'_>,
    ) -> Result<RouterSshHostRecord, AppError> {
        let conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|error| {
            AppError::Internal(format!(
                "begin SSH host fingerprint rotation failed: {error}"
            ))
        })?;
        let host = get_router_ssh_host(&tx, id)?
            .ok_or_else(|| AppError::NotFound("host not found".into()))?;
        if host.ssh_host_key_fingerprint.as_deref() != rotation.expected_fingerprint {
            return Err(AppError::Conflict(
                "the stored SSH host fingerprint changed; scan the host again".into(),
            ));
        }
        let active_jobs: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM provisioning_jobs
                 WHERE host_id = ?1 AND status IN ('pending', 'running')",
                params![id],
                |row| row.get(0),
            )
            .map_err(|error| {
                AppError::Internal(format!(
                    "check active jobs before SSH host fingerprint rotation failed: {error}"
                ))
            })?;
        if host_key_rotation_status_is_busy(&host.status) || active_jobs > 0 {
            return Err(AppError::Conflict(
                "SSH host fingerprint cannot be changed while a Host operation is active".into(),
            ));
        }
        let now = Utc::now();
        let now_text = now.to_rfc3339();
        let changed = tx
            .execute(
                "UPDATE router_ssh_hosts
                 SET ssh_host_key_fingerprint = ?3, last_verified_at = ?4, updated_at = ?4
                 WHERE id = ?1
                   AND ((ssh_host_key_fingerprint IS NULL AND ?2 IS NULL)
                        OR ssh_host_key_fingerprint = ?2)",
                params![
                    id,
                    rotation.expected_fingerprint,
                    rotation.fingerprint,
                    now_text
                ],
            )
            .map_err(|error| {
                AppError::Internal(format!("update SSH host fingerprint failed: {error}"))
            })?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "the stored SSH host fingerprint changed; scan the host again".into(),
            ));
        }
        crate::client_market_trade::insert_audit_tx(
            &tx,
            host.installation_id.as_deref(),
            Some(id),
            Some(rotation.actor_user_id),
            Some(rotation.actor_email),
            "host_ssh_fingerprint_rotated",
            serde_json::json!({
                "endpoint": ssh_known_hosts_target(&host.ip, host.port),
                "keyType": rotation.key_type,
                "oldFingerprint": host.ssh_host_key_fingerprint,
                "newFingerprint": rotation.fingerprint,
            }),
            now,
        )?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!(
                "commit SSH host fingerprint rotation failed: {error}"
            ))
        })?;
        get_router_ssh_host(&conn, id)?
            .ok_or_else(|| AppError::Internal("rotated Host disappeared".into()))
    }

    pub async fn client_market_complete_host_reverify(
        &self,
        id: &str,
        hostname: Option<&str>,
        fingerprint: Option<&str>,
    ) -> Result<RouterSshHostRecord, AppError> {
        let conn = self.conn.lock().await;
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
        tx.execute(
            "DELETE FROM client_market_host_reprobe_state WHERE host_id = ?1",
            params![id],
        )
        .map_err(|e| AppError::Internal(format!("clear Host reprobe state failed: {e}")))?;
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
        let conn = self.conn.lock().await;
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
        let conn = self.conn.lock().await;
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
        let conn = self.conn.lock().await;
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
            None,
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
            None,
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
        required_role: Option<&str>,
        reason: &str,
        deny_client_access: Option<bool>,
    ) -> Result<String, AppError> {
        let viewer = normalize_market_email(viewer_email)?;
        let conn = self.conn.lock().await;
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
        match required_role {
            Some("client") if !is_client_owner => {
                return Err(AppError::Forbidden(
                    "only the Client owner may release this rental".into(),
                ));
            }
            Some("provider") if !is_host_owner => {
                return Err(AppError::Forbidden(
                    "only the Host Provider may clean this rental".into(),
                ));
            }
            Some("client" | "provider") | None => {}
            Some(_) => {
                return Err(AppError::Internal(
                    "invalid Client cleanup role constraint".into(),
                ));
            }
        }
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
        let now = Utc::now().to_rfc3339();
        tx.execute(
            "UPDATE provisioning_jobs
             SET status = 'failed', phase = 'complete', failure_code = 'recovery_cancelled',
                 log_blob = substr(COALESCE(log_blob, '') || 'recovery cancelled by cleanup\n', -131072),
                 updated_at = ?3
             WHERE host_id = ?1 AND installation_id = ?2 AND type = 'recover'
               AND status IN ('pending', 'running')",
            params![host.0, installation_id, now],
        )
        .map_err(|error| {
            AppError::Internal(format!("cancel recovery before cleanup failed: {error}"))
        })?;
        tx.execute(
            "DELETE FROM client_market_recovery_state
             WHERE installation_id = ?1 AND host_id = ?2",
            params![installation_id, host.0],
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "clear recovery state before cleanup failed: {error}"
            ))
        })?;
        tx.execute(
            "DELETE FROM client_market_cleanup_recovery_state
             WHERE installation_id = ?1 AND host_id = ?2",
            params![installation_id, host.0],
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "clear cleanup recovery state before retry failed: {error}"
            ))
        })?;
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
        let client_owner_email = subscription_owner
            .as_ref()
            .map(|owner| owner.1.as_str())
            .unwrap_or(owner_email.as_str());
        let should_deny_client = required_role == Some("provider")
            && is_host_owner
            && deny_client_access.unwrap_or(false)
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
            should_deny_client,
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
        let conn = self.conn.lock().await;
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
        let failure_code = crate::store::client_chat::sanitize_system_event_text(failure_code);
        let failure_code = failure_code.as_str();
        let chunk = sanitize_job_log_chunk(log);
        let conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|e| {
            AppError::Internal(format!("begin fail cleanup transaction failed: {e}"))
        })?;
        let now_at = Utc::now();
        let now = now_at.to_rfc3339();
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
        let requires_manual = cleanup_recovery_requires_manual_intervention(failure_code);
        let next_attempt_at = (!requires_manual)
            .then(|| cleanup_recovery_next_at(now_at, 0))
            .flatten()
            .map(|value| value.to_rfc3339());
        let stopped_at = requires_manual.then(|| now.clone());
        tx.execute(
            "INSERT INTO client_market_cleanup_recovery_state (
                host_id, installation_id, attempt_count, next_attempt_at,
                last_attempt_at, last_outcome, stopped_at, updated_at
             ) VALUES (?1, ?2, 0, ?3, NULL, ?4, ?5, ?6)
             ON CONFLICT(host_id) DO UPDATE SET
                installation_id = excluded.installation_id,
                attempt_count = 0,
                next_attempt_at = excluded.next_attempt_at,
                last_attempt_at = NULL,
                last_outcome = excluded.last_outcome,
                stopped_at = excluded.stopped_at,
                updated_at = excluded.updated_at",
            params![
                host_id,
                installation_id,
                next_attempt_at,
                failure_code,
                stopped_at,
                now,
            ],
        )
        .map_err(|error| {
            AppError::Internal(format!("schedule failed cleanup recovery failed: {error}"))
        })?;
        crate::client_market_trade::cleanup_failed_tx(
            &tx,
            &installation_id,
            host_id,
            failure_code,
            now_at,
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
                s.client_user_id, h.provider_id, h.daily_rate_minor, h.offer_revision,
                COALESCE((SELECT methods_json FROM account_payment_profiles p
                          WHERE p.user_id = h.provider_id), '[]'),
                COALESCE((SELECT contacts_json FROM account_payment_profiles p
                          WHERE p.user_id = h.provider_id), '[]'),
                NULLIF(TRIM(h.currency), ''), h.free_duration_days
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
                    status, phase, log_blob, secret_ref, failure_code, created_at, updated_at,
                    started_at, heartbeat_at, deadline_at, worker_id
             FROM provisioning_jobs WHERE id = ?1",
        params![job_id],
        map_provisioning_job_row,
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("get job failed: {e}")))
}

fn map_router_ssh_host_row(row: &crate::db::Row<'_>) -> crate::db::Result<RouterSshHostRecord> {
    let methods_json: String = row.get(21)?;
    let contacts_json: String = row.get(22)?;
    let currency: Option<String> = row.get(23)?;
    let free_duration_days = row
        .get::<_, Option<i64>>(24)?
        .and_then(|value| u32::try_from(value).ok());
    let payment_methods =
        serde_json::from_str::<Vec<crate::client_market_trade::PaymentMethod>>(&methods_json)
            .unwrap_or_default();
    let mut payment_method_kinds = payment_methods
        .iter()
        .map(|method| method.kind.clone())
        .collect::<Vec<_>>();
    payment_method_kinds.sort();
    payment_method_kinds.dedup();
    let contacts: Vec<crate::client_market_trade::PaymentContact> =
        serde_json::from_str(&contacts_json).unwrap_or_default();
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
        daily_rate_minor: row.get(19)?,
        offer_revision: row.get(20)?,
        payment_method_kinds,
        contacts,
        currency,
        free_duration_days,
    })
}

fn map_provisioning_job_row(row: &crate::db::Row<'_>) -> crate::db::Result<ProvisioningJobRecord> {
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
        started_at: row.get(16)?,
        heartbeat_at: row.get(17)?,
        deadline_at: row.get(18)?,
        worker_id: row.get(19)?,
    })
}

fn collect_host_rows(
    rows: crate::db::MappedRows<
        '_,
        impl FnMut(&crate::db::Row<'_>) -> crate::db::Result<RouterSshHostRecord>,
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

    const TEST_ED25519_KEY_OLD: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAIGjWI8jfRRbxMZjdFDfgRlaHpRZPf7qs4odSbL41WQ1m";
    const TEST_ED25519_KEY_NEW: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAIGjWI8jfRRbxMZjdFDfgRlaHpRZPf7qs4odSbL41WQ1n";
    const TEST_ED25519_KEY_OTHER: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAIGjWI8jfRRbxMZjdFDfgRlaHpRZPf7qs4odSbL41WQ1o";

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
            data_dir: root.clone(),
            database: crate::config::DatabaseConfig::local(root.join("router.db")),
            host_key_path: root.join("host-key"),
            provision_ssh_private_key_path: root.join("provision-key"),
            provision_ssh_public_key_path: root.join("provision-key.pub"),
            cleanup_interval_secs: 300,
            lease_retention_secs: 24 * 60 * 60,
            request_log_retention_days: 30,
            client_stale_secs: 60 * 60,
            client_installation_retention_secs: 6 * 60 * 60,
            paused_share_stale_secs: 60 * 60,
            client_market_recovery_enabled: true,
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
            auth_source_hourly_limit: 15,
            ip_blacklist: String::new(),
            free_share_ip_parallel_limit: 1,
            market_usd_cny_rate_micros: crate::market_billing::DEFAULT_USD_CNY_RATE_MICROS,
            ip_intel_endpoints: Vec::new(),
            verification_service_base_url: "https://tokenswitch.org".into(),
            verification_service_api_key: None,
            router_owner_email: None,
            admin_emails: HashSet::new(),
            ux_telemetry_enabled: false,
            ux_telemetry_retention_days: 7,
            footer_telegram_url: crate::config::DEFAULT_FOOTER_TELEGRAM_URL.to_string(),
            metrics: MetricsConfig {
                enabled: false,
                db_path: root.join("metrics.db"),
                retention_days: 7,
                sample_interval_secs: 5,
                alerting: crate::config::AlertingSettings::default(),
            },
            clock_health: crate::config::ClockHealthConfig::default(),
        };
        (config, root)
    }

    fn test_store(name: &str) -> (AppStore, Config, PathBuf) {
        let (config, root) = test_config(name);
        let store = AppStore::new(&config).expect("create client market test store");
        (store, config, root)
    }

    fn test_ssh_fingerprint(encoded_key: &str) -> String {
        let blob = base64::engine::general_purpose::STANDARD
            .decode(encoded_key)
            .expect("decode test SSH key");
        format!(
            "SHA256:{}",
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(blob))
        )
    }

    #[test]
    fn ssh_keyscan_parser_prefers_ed25519_and_preserves_a_pinned_algorithm() {
        let target = "[203.0.113.10]:22222";
        let rsa_key = base64::engine::general_purpose::STANDARD.encode(b"test-rsa-key");
        let ecdsa_key = base64::engine::general_purpose::STANDARD.encode(b"test-ecdsa-key");
        let output = format!(
            "# ssh-keyscan comment\n\
             {target} ssh-rsa {rsa_key}\n\
             wrong.example ssh-ed25519 {TEST_ED25519_KEY_OTHER}\n\
             {target} ecdsa-sha2-nistp256 {ecdsa_key}\n\
             {target} ssh-ed25519 {TEST_ED25519_KEY_NEW}\n"
        );

        let preferred = parse_ssh_keyscan_output(&output, target, None)
            .expect("select the preferred SSH host key");
        assert_eq!(preferred.target, target);
        assert_eq!(preferred.key_type, "ssh-ed25519");
        assert_eq!(preferred.encoded_key, TEST_ED25519_KEY_NEW);

        let rsa_fingerprint = test_ssh_fingerprint(&rsa_key);
        let pinned_rsa = parse_ssh_keyscan_output(&output, target, Some(&rsa_fingerprint))
            .expect("retain a pinned RSA host key");
        assert_eq!(pinned_rsa.key_type, "ssh-rsa");
        assert_eq!(pinned_rsa.fingerprint, rsa_fingerprint);

        let ecdsa_fingerprint = test_ssh_fingerprint(&ecdsa_key);
        let pinned_ecdsa = parse_ssh_keyscan_output(&output, target, Some(&ecdsa_fingerprint))
            .expect("retain a pinned ECDSA host key");
        assert_eq!(pinned_ecdsa.key_type, "ecdsa-sha2-nistp256");
        assert_eq!(pinned_ecdsa.fingerprint, ecdsa_fingerprint);

        assert!(
            parse_ssh_keyscan_output(
                &format!("wrong.example ssh-ed25519 {TEST_ED25519_KEY_NEW}\n"),
                target,
                None,
            )
            .is_none()
        );
    }

    #[test]
    fn confirmed_ssh_fingerprint_requires_an_openssh_sha256_digest() {
        let valid = format!(
            "SHA256:{}",
            base64::engine::general_purpose::STANDARD_NO_PAD.encode([7_u8; 32])
        );
        assert_eq!(
            normalize_confirmed_ssh_fingerprint(&format!("  {valid}  ")).unwrap(),
            valid
        );

        for invalid in [
            "",
            "MD5:00:11:22",
            "SHA256:not-base64!",
            "SHA256:c2hvcnQ",
            "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        ] {
            assert!(
                matches!(
                    normalize_confirmed_ssh_fingerprint(invalid),
                    Err(AppError::BadRequest(_))
                ),
                "fingerprint should be rejected: {invalid}"
            );
        }
    }

    #[tokio::test]
    async fn known_hosts_rotation_replaces_hashed_entry_and_restores_snapshots() {
        let (_config, root) = test_config("known-hosts-rotation");
        let path = root.join("known_hosts");
        let target = "[203.0.113.10]:22222";
        let original = format!(
            "{target} ssh-ed25519 {TEST_ED25519_KEY_OLD}\n\
             other.example ssh-ed25519 {TEST_ED25519_KEY_OTHER}\n"
        );
        atomic_write_known_hosts(&path, original.as_bytes()).expect("write known_hosts fixture");
        let hashed = std::process::Command::new("ssh-keygen")
            .arg("-H")
            .arg("-f")
            .arg(&path)
            .output()
            .expect("start ssh-keygen hash");
        assert!(
            hashed.status.success(),
            "hash known_hosts fixture: {}",
            String::from_utf8_lossy(&hashed.stderr)
        );
        let _ = fs::remove_file(ssh_keygen_backup_path(&path));
        let original_hashed = fs::read(&path).expect("read hashed known_hosts fixture");
        assert!(!String::from_utf8_lossy(&original_hashed).contains(target));

        let observed = ObservedSshHostKey {
            target: target.into(),
            key_type: "ssh-ed25519".into(),
            encoded_key: TEST_ED25519_KEY_NEW.into(),
            fingerprint: test_ssh_fingerprint(TEST_ED25519_KEY_NEW),
        };
        let snapshot = install_known_host_entry(&path, &observed)
            .await
            .expect("rotate known_hosts entry");
        assert!(snapshot.existed);
        assert_eq!(snapshot.bytes, original_hashed);

        let rotated = fs::read_to_string(&path).expect("read rotated known_hosts");
        assert!(rotated.contains(TEST_ED25519_KEY_NEW));
        assert!(rotated.contains(TEST_ED25519_KEY_OTHER));
        assert!(!rotated.contains(TEST_ED25519_KEY_OLD));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let lookup = std::process::Command::new("ssh-keygen")
            .arg("-F")
            .arg(target)
            .arg("-f")
            .arg(&path)
            .output()
            .expect("look up rotated Host");
        assert!(lookup.status.success());
        assert!(String::from_utf8_lossy(&lookup.stdout).contains(TEST_ED25519_KEY_NEW));

        restore_known_hosts_snapshot(&path, &snapshot).expect("restore known_hosts snapshot");
        assert_eq!(fs::read(&path).unwrap(), original_hashed);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let new_path = root.join("new_known_hosts");
        let new_snapshot = install_known_host_entry(&new_path, &observed)
            .await
            .expect("install first known_hosts entry");
        assert!(!new_snapshot.existed);
        assert!(new_path.exists());
        restore_known_hosts_snapshot(&new_path, &new_snapshot)
            .expect("remove known_hosts created after snapshot");
        assert!(!new_path.exists());

        let leftovers = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("known_hosts.rotate") || name.ends_with(".old"))
            .collect::<Vec<_>>();
        assert!(
            leftovers.is_empty(),
            "temporary files remain: {leftovers:?}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn host_fingerprint_rotation_is_cas_audited_and_rejects_busy_hosts() {
        let (store, _config, root) = test_store("host-key-cas");
        let host = add_provider_host(
            &store,
            "provider-host-key",
            "provider@example.com",
            "198.18.44.1",
            "US",
            None,
        )
        .await;
        let old_fingerprint = host.ssh_host_key_fingerprint.clone().unwrap();
        let new_fingerprint = test_ssh_fingerprint(TEST_ED25519_KEY_NEW);

        let updated = store
            .client_market_rotate_host_fingerprint(
                &host.id,
                HostFingerprintRotation {
                    expected_fingerprint: Some(&old_fingerprint),
                    fingerprint: &new_fingerprint,
                    key_type: "ssh-ed25519",
                    actor_user_id: "provider-host-key",
                    actor_email: "provider@example.com",
                },
            )
            .await
            .expect("rotate pinned Host fingerprint");
        assert_eq!(
            updated.ssh_host_key_fingerprint.as_deref(),
            Some(new_fingerprint.as_str())
        );
        assert!(updated.last_verified_at.is_some());

        assert!(matches!(
            store
                .client_market_rotate_host_fingerprint(
                    &host.id,
                    HostFingerprintRotation {
                        expected_fingerprint: Some(&old_fingerprint),
                        fingerprint: &test_ssh_fingerprint(TEST_ED25519_KEY_OTHER),
                        key_type: "ssh-ed25519",
                        actor_user_id: "stale-actor",
                        actor_email: "stale@example.com",
                    },
                )
                .await,
            Err(AppError::Conflict(_))
        ));

        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE router_ssh_hosts SET status = 'reserved' WHERE id = ?1",
                params![host.id],
            )
            .unwrap();
        }
        assert!(matches!(
            store
                .client_market_rotate_host_fingerprint(
                    &host.id,
                    HostFingerprintRotation {
                        expected_fingerprint: Some(&new_fingerprint),
                        fingerprint: &test_ssh_fingerprint(TEST_ED25519_KEY_OTHER),
                        key_type: "ssh-ed25519",
                        actor_user_id: "provider-host-key",
                        actor_email: "provider@example.com",
                    },
                )
                .await,
            Err(AppError::Conflict(_))
        ));

        {
            let conn = store.conn.lock().await;
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE router_ssh_hosts SET status = 'idle' WHERE id = ?1",
                params![host.id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO provisioning_jobs
                    (id, type, host_id, status, phase, log_blob, created_at, updated_at)
                 VALUES ('host-key-job', 'create', ?1, 'running', 'locked', '', ?2, ?2)",
                params![host.id, now],
            )
            .unwrap();
        }
        assert!(matches!(
            store
                .client_market_rotate_host_fingerprint(
                    &host.id,
                    HostFingerprintRotation {
                        expected_fingerprint: Some(&new_fingerprint),
                        fingerprint: &test_ssh_fingerprint(TEST_ED25519_KEY_OTHER),
                        key_type: "ssh-ed25519",
                        actor_user_id: "provider-host-key",
                        actor_email: "provider@example.com",
                    },
                )
                .await,
            Err(AppError::Conflict(_))
        ));

        {
            let conn = store.conn.lock().await;
            let (count, actor_user_id, actor_email, detail_json): (
                i64,
                Option<String>,
                Option<String>,
                String,
            ) = conn
                .query_row(
                    "SELECT COUNT(*), actor_user_id, actor_email, detail_json
                     FROM client_market_audit_events
                     WHERE host_id = ?1 AND event_type = 'host_ssh_fingerprint_rotated'",
                    params![host.id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .unwrap();
            assert_eq!(count, 1);
            assert_eq!(actor_user_id.as_deref(), Some("provider-host-key"));
            assert_eq!(actor_email.as_deref(), Some("provider@example.com"));
            let detail: serde_json::Value = serde_json::from_str(&detail_json).unwrap();
            assert_eq!(detail["endpoint"], "198.18.44.1");
            assert_eq!(detail["keyType"], "ssh-ed25519");
            assert_eq!(detail["oldFingerprint"], old_fingerprint);
            assert_eq!(detail["newFingerprint"], new_fingerprint);
            let stored: String = conn
                .query_row(
                    "SELECT ssh_host_key_fingerprint FROM router_ssh_hosts WHERE id = ?1",
                    params![host.id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(stored, new_fingerprint);
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn schema_excludes_legacy_prepaid_billing_contract() {
        let conn = Connection::open_in_memory().expect("open in-memory database");
        crate::schema::apply(&conn).expect("initialize database baseline");

        for table in [
            "client_market_invoices",
            "client_market_payment_declarations",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    params![table],
                    |row| row.get(0),
                )
                .expect("inspect legacy Client Market table");
            assert_eq!(count, 0, "legacy table {table} must not be created");
        }

        for (table, legacy_columns) in [
            (
                "router_ssh_hosts",
                &["price_cents", "rental_period_days"][..],
            ),
            (
                "client_market_allocation_quote_items",
                &["price_cents", "rental_period_days"][..],
            ),
            (
                "client_market_subscriptions",
                &[
                    "price_cents",
                    "rental_period_days",
                    "last_declared_at",
                    "current_period_end",
                    "payment_deadline",
                ][..],
            ),
        ] {
            let mut statement = conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .expect("inspect Client Market schema");
            let columns = statement
                .query_map([], |row| row.get::<_, String>(1))
                .expect("query Client Market columns")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect Client Market columns");
            for column in legacy_columns {
                assert!(
                    !columns.iter().any(|existing| existing == column),
                    "legacy column {table}.{column} must not be created"
                );
            }
        }
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
                None,
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
            auth_source_kind: "auth_device".into(),
            auth_source_id: format!("browser-{user_id}"),
            access_token_hash: format!("access-{user_id}"),
            refresh_token_hash: format!("refresh-{user_id}"),
            access_expires_at: now + chrono::Duration::hours(1),
            refresh_expires_at: now + chrono::Duration::days(30),
            created_at: now,
            last_used_at: now,
        }
    }

    #[test]
    fn host_ownership_requires_the_stable_provider_user_id() {
        let session = market_session("provider-stable", "provider@example.com");
        assert!(session_is_host_owner(&session, Some("provider-stable")));
        assert!(!session_is_host_owner(
            &session,
            Some("email:provider@example.com")
        ));
        assert!(!session_is_host_owner(&session, None));
    }

    async fn add_provider_host(
        store: &AppStore,
        provider_id: &str,
        email: &str,
        ip: &str,
        country: &str,
        daily_rate_minor: Option<i64>,
    ) -> RouterSshHostRecord {
        let session = market_session(provider_id, email);
        let now = Utc::now().to_rfc3339();
        {
            let conn = store.conn.lock().await;
            crate::market_access::configure_open_test_policy(&conn, &session, "USD", 50_000, &now);
        }
        if daily_rate_minor.is_some() {
            ensure_payment_profile(store, provider_id, email).await;
            store
                .market_billing_update_supplier_profile(&session, "USD", 24)
                .await
                .expect("configure test USD payment grace");
        }
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
                daily_rate_minor,
                daily_rate_minor.map(|_| "USD"),
                None,
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
            .client_market_bind_job_user(
                job_id,
                &format!("test-client:{}", client_owner.to_ascii_lowercase()),
            )
            .await
            .expect("bind test Client owner");
        {
            let conn = store.conn.lock().await;
            let now = Utc::now().to_rfc3339();
            for owner in owners {
                let provider_id: String = conn
                    .query_row(
                        "SELECT provider_id FROM host_provider_profiles WHERE owner_email = ?1",
                        params![owner.to_ascii_lowercase()],
                        |row| row.get(0),
                    )
                    .expect("resolve test Host Provider");
                crate::market_access::configure_open_test_policy(
                    &conn,
                    &market_session(&provider_id, owner),
                    "USD",
                    50_000,
                    &now,
                );
            }
        }
        store
            .client_market_start_job(job_id, JOB_TYPE_CREATE)
            .await
            .expect("start job");
    }

    async fn insert_running_job_with_watchdog(
        store: &AppStore,
        job_id: &str,
        heartbeat_at: DateTime<Utc>,
    ) {
        let now = Utc::now();
        store
            .conn
            .lock()
            .await
            .execute(
                "INSERT INTO provisioning_jobs (
                    id, type, selection_owners_json, selection_regions_json,
                    status, phase, log_blob, created_at, updated_at,
                    started_at, heartbeat_at, deadline_at, worker_id
                 ) VALUES (?1, 'create', '[]', '[]', 'running', 'pending', '',
                           ?2, ?3, ?2, ?3, ?4, ?5)",
                params![
                    job_id,
                    (now - chrono::Duration::minutes(30)).to_rfc3339(),
                    heartbeat_at.to_rfc3339(),
                    (now - chrono::Duration::minutes(1)).to_rfc3339(),
                    format!("watchdog:{job_id}"),
                ],
            )
            .expect("insert watchdog-owned job");
    }

    #[tokio::test]
    async fn active_watchdog_finalizer_is_not_reclaimed() {
        let (store, _, root) = test_store("active-watchdog-finalizer");
        let now = Utc::now();
        insert_running_job_with_watchdog(&store, "active-watchdog-job", now).await;

        assert!(
            store
                .client_market_claim_expired_job_leases(now)
                .await
                .expect("scan active watchdog finalizer")
                .is_empty()
        );
        let worker_id = store
            .client_market_get_job_record("active-watchdog-job")
            .await
            .expect("read active watchdog job")
            .expect("active watchdog job exists")
            .worker_id;
        assert_eq!(worker_id.as_deref(), Some("watchdog:active-watchdog-job"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn stale_watchdog_finalizer_can_be_reclaimed_once() {
        let (store, _, root) = test_store("stale-watchdog-finalizer");
        let now = Utc::now();
        let stale_at = now
            - chrono::Duration::from_std(JOB_WATCHDOG_STALE_AFTER).expect("valid watchdog timeout")
            - chrono::Duration::seconds(1);
        insert_running_job_with_watchdog(&store, "stale-watchdog-job", stale_at).await;

        let claimed = store
            .client_market_claim_expired_job_leases(now)
            .await
            .expect("reclaim stale watchdog finalizer");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].id, "stale-watchdog-job");
        assert!(
            store
                .client_market_claim_expired_job_leases(now)
                .await
                .expect("avoid duplicate watchdog finalizer")
                .is_empty()
        );
        let worker_id = store
            .client_market_get_job_record("stale-watchdog-job")
            .await
            .expect("read reclaimed watchdog job")
            .expect("reclaimed watchdog job exists")
            .worker_id
            .expect("reclaimed watchdog worker");
        assert!(worker_id.starts_with("watchdog:"));
        assert_ne!(worker_id, "watchdog:stale-watchdog-job");
        let _ = std::fs::remove_dir_all(root);
    }

    fn insert_installation(conn: &Connection, installation_id: &str, owner: &str) {
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO installations (
                id, public_key, platform, app_version, owner_email, owner_verified_at,
                created_at, last_seen_at, client_activated_at, control_secret_b64,
                provision_source, provision_host_id
             ) VALUES (?1, ?2, 'linux', 'test', ?3, ?4, ?4, ?4, ?4, ?5, NULL, NULL)",
            params![
                installation_id,
                format!("test-public-key-{installation_id}"),
                owner,
                now,
                format!("test-control-secret-{installation_id}"),
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
                daily_rate_minor: None,
                currency: None,
                free_duration_days: Some(1),
                expected_fingerprint: Some("SHA256:first".into()),
                informational_status: Some("idle".into()),
            },
            HostTransferEntry {
                ip: "203.0.113.11".into(),
                port: 2222,
                note: Some("second".into()),
                daily_rate_minor: Some(500),
                currency: Some("USD".into()),
                free_duration_days: None,
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
        let owner = market_session("provider-import", "provider@example.com");
        let pending = store
            .client_market_host_import_job(&job_id, &owner)
            .await
            .expect("read pending Host import job");
        assert_eq!(pending.status, "pending");
        assert_eq!(pending.items.len(), 2);
        assert_eq!(pending.failed, 0);
        let other = market_session("provider-other", "provider@example.com");
        assert!(matches!(
            store.client_market_host_import_job(&job_id, &other).await,
            Err(AppError::Forbidden(_))
        ));
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
        assert_eq!(completed.status, "completed");
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
            let conn = store.conn.lock().await;
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
        {
            let conn = store.conn.lock().await;
            let (event_type, payload_json): (String, String) = conn
                .query_row(
                    "SELECT event_type, payload_json
                     FROM client_chat_system_outbox
                     WHERE installation_id = 'cleanup-installation'
                     ORDER BY created_at DESC LIMIT 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            let payload: serde_json::Value = serde_json::from_str(&payload_json).unwrap();
            assert_eq!(event_type, "cleanup_started");
            assert_eq!(payload["clientLabel"], "cleanup-client");
            assert_eq!(payload["clientOwnerEmail"], "client@example.com");
            assert_eq!(payload["providerEmail"], "host@example.com");
            assert_eq!(payload["status"], "releasing");
        }
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
                     client_user_id, client_owner_email, status, daily_rate_minor,
                     offer_revision, created_at, updated_at)
                 VALUES ('failed-release-client', ?1, 'provider-reuse', 'provider@example.com',
                         'renter-reuse', 'renter@example.com', 'release_failed', 500,
                         1, ?2, ?2)",
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
    fn host_approval_flag_only_marks_authenticated_non_owners_without_access() {
        let access = HashMap::from([
            (("allowed-provider".to_string(), "free".to_string()), true),
            (("blocked-provider".to_string(), "free".to_string()), false),
        ]);
        assert!(host_seller_approval_required(
            true,
            false,
            true,
            Some("blocked-provider"),
            None,
            &access,
        ));
        assert!(!host_seller_approval_required(
            false,
            false,
            true,
            Some("blocked-provider"),
            None,
            &access,
        ));
        assert!(!host_seller_approval_required(
            true,
            true,
            true,
            Some("blocked-provider"),
            None,
            &access,
        ));
        assert!(!host_seller_approval_required(
            true,
            false,
            true,
            Some("allowed-provider"),
            None,
            &access,
        ));
        assert!(!host_seller_approval_required(
            true,
            false,
            false,
            Some("blocked-provider"),
            None,
            &access,
        ));
        assert!(!host_seller_approval_required(
            true, false, true, None, None, &access,
        ));
    }

    #[test]
    fn public_host_views_hide_operational_and_owner_details() {
        let host = RouterSshHostRecord {
            id: "host-id".into(),
            provider_id: Some("provider-id".into()),
            ip: "203.0.113.9".into(),
            port: 2222,
            host_owner_email: "host@example.com".into(),
            daily_rate_minor: Some(500),
            currency: Some("USD".into()),
            free_duration_days: None,
            offer_revision: 1,
            payment_method_kinds: vec!["alipay".into()],
            contacts: vec![],
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
        assert!(public.get("paymentMethods").is_none());
        assert_eq!(
            public
                .get("sellerApprovalRequired")
                .and_then(|value| value.as_bool()),
            Some(false)
        );
        assert_eq!(
            public
                .get("eligibility")
                .and_then(|value| value.get("status"))
                .and_then(|value| value.as_str()),
            Some("allowed")
        );
        assert_eq!(
            public
                .get("eligibility")
                .and_then(|value| value.get("allowed"))
                .and_then(|value| value.as_bool()),
            Some(true)
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
        assert!(private.get("paymentMethods").is_none());
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
            None,
        )
        .await;
        let second = add_provider_host(
            &store,
            "provider-1",
            "provider@example.com",
            "198.18.20.2",
            "US",
            None,
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
        store
            .market_billing_update_supplier_profile(&provider, "USD", 24)
            .await
            .expect("configure Provider payment grace");
        assert!(matches!(
            store
                .client_market_update_host_offer(
                    &first.id,
                    &provider,
                    Some(900),
                    Some("USD".into()),
                    None,
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        let reserved_host = store
            .client_market_get_host(&first.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reserved_host.status,
            crate::client_market_trade::HOST_STATUS_RESERVED
        );
        assert_eq!(reserved_host.offer_revision, 1);
        {
            let conn = store.conn.lock().await;
            assert_eq!(
                conn.execute(
                    "UPDATE router_ssh_hosts SET offer_revision = offer_revision + 1 WHERE id = ?1",
                    params![first.id],
                )
                .expect("simulate an out-of-band offer revision change"),
                1
            );
        }
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
        let changed_offer = store
            .client_market_update_host_offer(
                &first.id,
                &provider,
                Some(900),
                Some("USD".into()),
                None,
            )
            .await
            .expect("Provider may change an offer after the quote is cancelled");
        assert_eq!(changed_offer.offer_revision, 3);

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
        let replayed = store
            .client_market_commit_quote(&fixed.id, &client, &prepared)
            .await
            .expect("replay identical quote commit");
        assert_eq!(replayed.batch_id, committed.batch_id);
        assert_eq!(replayed.job_ids, committed.job_ids);
        assert!(replayed.replayed);
        assert!(matches!(
            store
                .client_market_commit_quote_idempotent(
                    &fixed.id,
                    &client,
                    &format!("quote:{}", fixed.id),
                    "different-request-fingerprint",
                    &prepared,
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        let batch = store
            .client_market_batch(&committed.batch_id, &client)
            .await
            .expect("batch owner reads committed jobs");
        assert_eq!(batch.jobs.len(), 1);
        let stranger = market_session("client-stranger", "stranger@example.com");
        assert!(matches!(
            store
                .client_market_batch(&committed.batch_id, &stranger)
                .await,
            Err(AppError::Forbidden(_))
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
    async fn quote_commit_rechecks_provider_access_and_cancel_releases_host() {
        use crate::client_market_trade::CreateQuoteRequest;

        let (store, _config, root) = test_store("trade-quote-provider-block");
        let host = add_provider_host(
            &store,
            "provider-blocked-quote",
            "provider@example.com",
            "198.18.20.3",
            "US",
            Some(500),
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
            .expect("create quote before Provider access changes");
        {
            let conn = store.conn.lock().await;
            crate::market_access::set_product_access_decision_tx(
                &conn,
                "provider-blocked-quote",
                "provider@example.com",
                &client.user_id,
                &client.email,
                crate::market_access::PRODUCT_CLIENT_HOST,
                crate::market_access::PRICING_PAID,
                crate::market_access::DECISION_DENY,
                "provider-blocked-quote",
                &Utc::now().to_rfc3339(),
            )
            .expect("deny quoted Client access");
        }
        let prepared = vec![(
            quote.items[0].id.clone(),
            "blocked-quote-client".into(),
            "secret".into(),
            quote.items[0].offer_revision,
        )];
        let error = store
            .client_market_commit_quote(&quote.id, &client, &prepared)
            .await
            .expect_err("recheck Provider access before committing quote");
        assert_eq!(
            error.code(),
            Some(crate::market_access::ERROR_MARKET_ACCESS_REQUIRED)
        );
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
    async fn host_offer_updates_only_affect_future_postpaid_contracts() {
        let (store, _config, root) = test_store("trade-offer-postpaid");
        let host = add_provider_host(
            &store,
            "provider-stable",
            "provider-old@example.com",
            "198.18.21.1",
            "US",
            Some(500),
        )
        .await;
        let now = Utc::now().to_rfc3339();
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "INSERT INTO client_market_subscriptions
                    (installation_id, host_id, provider_id, host_owner_email,
                     client_user_id, client_owner_email, status, daily_rate_minor,
                     currency, offer_revision, created_at, updated_at)
                 VALUES ('paid-client', ?1, 'provider-stable', 'provider-old@example.com',
                         'client-stable', 'client-old@example.com', 'active', 500,
                         'USD', 1, ?2, ?2)",
                params![host.id, now],
            )
            .expect("insert active postpaid subscription");
        }

        let provider = market_session("provider-stable", "provider-old@example.com");
        let updated = store
            .client_market_update_host_offer(
                &host.id,
                &provider,
                Some(900),
                Some("USD".into()),
                None,
            )
            .await
            .expect("update daily Host rate");
        assert_eq!(updated.offer_revision, 2);
        assert_eq!(updated.daily_rate_minor, Some(900));
        let unchanged = store
            .client_market_update_host_offer(
                &host.id,
                &provider,
                Some(900),
                Some("USD".into()),
                None,
            )
            .await
            .expect("save unchanged daily Host rate");
        assert_eq!(unchanged.offer_revision, 2);

        let conn = store.conn.lock().await;
        let frozen: (Option<i64>, Option<String>, i64) = conn
            .query_row(
                "SELECT daily_rate_minor, currency, offer_revision
                 FROM client_market_subscriptions WHERE installation_id = 'paid-client'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read frozen Client contract offer");
        assert_eq!(frozen, (Some(500), Some("USD".into()), 1));
        drop(conn);
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
        )
        .await;
        let second = add_provider_host(
            &store,
            "provider-stable",
            "provider-old@example.com",
            "198.18.24.2",
            "CA",
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
                 SET daily_rate_minor = 500, status = 'allocated'
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
                         client_user_id, client_owner_email, status, daily_rate_minor,
                         offer_revision,
                         created_at, updated_at)
                     VALUES (?1, ?2, 'provider-stable', 'provider-new@example.com',
                             ?3, 'client@example.com', 'active', 500, 1, ?4, ?4)",
                    params![
                        installation_id,
                        host_id,
                        client_user_id,
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
    async fn provider_supply_counts_drifted_permanent_free_hosts_as_idle() {
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
        )
        .await;
        let free = add_provider_host(
            &store,
            "provider-stable",
            "provider@example.com",
            "198.18.25.2",
            "JP",
            None,
        )
        .await;
        {
            let conn = store.conn.lock().await;
            // Simulate legacy/drifted provider_id that never got healed by a paid offer edit.
            conn.execute(
                "UPDATE router_ssh_hosts
                 SET provider_id = 'email:provider@example.com', daily_rate_minor = NULL
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
    async fn provider_supply_counts_null_provider_id_permanent_free_hosts_as_idle() {
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
        )
        .await;
        {
            let conn = store.conn.lock().await;
            // Live us01 shape: provider_id was added nullable and never backfilled.
            // SQL `NULL != canonical_id` is unknown, so the old heal skipped these rows.
            conn.execute(
                "UPDATE router_ssh_hosts
                 SET provider_id = NULL, daily_rate_minor = NULL, status = 'idle'
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
    async fn pool_allocation_quotes_only_select_free_hosts() {
        use crate::client_market_trade::CreateQuoteRequest;

        let (store, _config, root) = test_store("trade-quotes-free-only");
        let _paid = add_provider_host(
            &store,
            "provider-free-only",
            "provider@example.com",
            "198.18.41.1",
            "US",
            Some(500),
        )
        .await;
        let free = add_provider_host(
            &store,
            "provider-free-only",
            "provider@example.com",
            "198.18.41.2",
            "US",
            None,
        )
        .await;
        let client = market_session("client-free-only", "client@example.com");

        assert!(
            matches!(
                store
                    .client_market_create_quote(
                        &client,
                        CreateQuoteRequest {
                            provider_ids: vec!["provider-free-only".into()],
                            country_codes: vec!["US".into()],
                            count: 2,
                            host_id: None,
                        },
                    )
                    .await,
                Err(AppError::ServiceUnavailable(_))
            ),
            "pool quotes must not pad capacity with paid Hosts"
        );

        let quote = store
            .client_market_create_quote(
                &client,
                CreateQuoteRequest {
                    provider_ids: vec!["provider-free-only".into()],
                    country_codes: vec!["US".into()],
                    count: 1,
                    host_id: None,
                },
            )
            .await
            .expect("pool quote selects the free Host");
        assert_eq!(quote.items.len(), 1);
        assert_eq!(quote.items[0].host_id, free.id);
        assert!(quote.items[0].daily_rate_minor.is_none());

        store
            .client_market_cancel_quote(&quote.id, &client)
            .await
            .expect("cancel free quote");
        let paid_quote = store
            .client_market_create_quote(
                &client,
                CreateQuoteRequest {
                    provider_ids: Vec::new(),
                    country_codes: Vec::new(),
                    count: 1,
                    host_id: Some(_paid.id.clone()),
                },
            )
            .await
            .expect("fixed Host quotes may still target paid Hosts");
        assert_eq!(paid_quote.items[0].host_id, _paid.id);
        assert_eq!(paid_quote.items[0].daily_rate_minor, Some(500));

        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn pool_allocation_quotes_sample_across_free_hosts() {
        use crate::client_market_trade::CreateQuoteRequest;
        use std::collections::HashSet;

        let (store, _config, root) = test_store("trade-quotes-random-sample");
        for index in 0..6 {
            add_provider_host(
                &store,
                "provider-random",
                "provider@example.com",
                &format!("198.18.42.{index}"),
                "US",
                None,
            )
            .await;
        }
        let client = market_session("client-random", "client@example.com");
        let mut seen = HashSet::new();
        for _ in 0..12 {
            let quote = store
                .client_market_create_quote(
                    &client,
                    CreateQuoteRequest {
                        provider_ids: vec!["provider-random".into()],
                        country_codes: vec!["US".into()],
                        count: 1,
                        host_id: None,
                    },
                )
                .await
                .expect("sample free Host");
            seen.insert(quote.items[0].host_id.clone());
            store
                .client_market_cancel_quote(&quote.id, &client)
                .await
                .expect("release sampled Host");
        }
        assert!(
            seen.len() > 1,
            "repeated pool quotes should sample more than one free Host, got {seen:?}"
        );

        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn provider_supply_bootstraps_profiles_for_orphan_host_owners() {
        let (store, _config, root) = test_store("provider-supply-orphan-owner");
        store
            .client_market_ensure_provider("official-provider", "official@example.com")
            .await
            .expect("create official Provider");
        let _official = add_provider_host(
            &store,
            "official-provider",
            "official@example.com",
            "198.18.40.1",
            "DE",
            None,
        )
        .await;
        // Live shape: Host rows exist for a non-official owner with NULL provider_id and
        // no host_provider_profiles row, so Create Client only ever listed the official owner.
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "INSERT INTO users (id, email_normalized, status, created_at, last_login_at)
                 VALUES ('non-official-user', 'peer@example.com', 'active', ?1, ?1)",
                params![Utc::now().to_rfc3339()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO router_ssh_hosts (
                    id, provider_id, ip, port, host_owner_email, country_code, hostname,
                    ssh_host_key_fingerprint, status, installation_id, last_verified_at, last_error,
                    note, ip_intel_json, daily_rate_minor, offer_revision, created_at, updated_at
                 ) VALUES (
                    'orphan-host-1', NULL, '198.18.40.2', 22, 'peer@example.com', 'US', 'peer-host',
                    'SHA256:peer', 'idle', NULL, NULL, NULL, NULL, NULL, NULL, 1, ?1, ?1
                 )",
                params![Utc::now().to_rfc3339()],
            )
            .unwrap();
        }

        let supply = store
            .client_market_provider_supply(Some("official@example.com"))
            .await
            .expect("load Provider supply");
        assert!(
            supply
                .providers
                .iter()
                .any(|provider| provider.provider_id == "official-provider" && provider.official),
            "official Provider must remain listed"
        );
        let peer = supply
            .providers
            .iter()
            .find(|provider| provider.owner_email == "peer@example.com")
            .expect("non-official Host owner must appear in supply");
        assert_eq!(peer.provider_id, "non-official-user");
        assert!(!peer.official);
        assert_eq!(peer.host_total, 1);
        assert_eq!(peer.idle_total, 1);
        let us = peer
            .countries
            .iter()
            .find(|country| country.code == "US")
            .expect("orphan Host country must feed region capacity");
        assert_eq!(us.idle, 1);
        assert_eq!(us.total, 1);

        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn trade_reconcile_leaves_postpaid_subscriptions_to_market_billing() {
        let (store, _config, root) = test_store("trade-postpaid");
        let host = add_provider_host(
            &store,
            "provider-postpaid",
            "provider@example.com",
            "198.18.22.1",
            "US",
            Some(700),
        )
        .await;
        let now = Utc::now();
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "INSERT INTO client_market_subscriptions
                    (installation_id, host_id, provider_id, host_owner_email,
                     client_user_id, client_owner_email, status, daily_rate_minor,
                     currency, offer_revision,
                     created_at, updated_at)
                 VALUES ('postpaid-client', ?1, 'provider-postpaid', 'provider@example.com',
                         'postpaid-user', 'postpaid@example.com', 'active', 700,
                         'USD', 1, ?2, ?2)",
                params![host.id, now.to_rfc3339()],
            )
            .unwrap();
        }
        store
            .client_market_reconcile_trade_state(now)
            .await
            .expect("reconcile market quotes");
        store
            .client_market_reconcile_trade_state(now + chrono::Duration::days(60))
            .await
            .expect("repeat market quote reconciliation");
        {
            let conn = store.conn.lock().await;
            let status: String = conn
                .query_row(
                    "SELECT status FROM client_market_subscriptions
                     WHERE installation_id = 'postpaid-client'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(status, "active");
        }
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn free_host_duration_is_bounded_and_paid_offers_reject_it() {
        assert_eq!(
            crate::client_market_trade::validate_free_duration_days(None, Some(1)).unwrap(),
            Some(1)
        );
        assert_eq!(
            crate::client_market_trade::validate_free_duration_days(None, None).unwrap(),
            None
        );
        assert!(matches!(
            crate::client_market_trade::validate_free_duration_days(None, Some(0)),
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            crate::client_market_trade::validate_free_duration_days(
                None,
                Some(crate::client_market_trade::MAX_FREE_DURATION_DAYS + 1),
            ),
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            crate::client_market_trade::validate_free_duration_days(Some(500), Some(1)),
            Err(AppError::BadRequest(_))
        ));
    }

    #[tokio::test]
    async fn free_host_duration_is_frozen_in_quote_and_starts_at_activation() {
        use crate::client_market_trade::CreateQuoteRequest;

        let (store, _config, root) = test_store("free-duration-quote-snapshot");
        let host = add_provider_host(
            &store,
            "provider-free-duration",
            "provider@example.com",
            "198.18.52.1",
            "US",
            None,
        )
        .await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE router_ssh_hosts SET free_duration_days = 7 WHERE id = ?1",
                params![host.id],
            )
            .unwrap();
        }
        let client = market_session("client-free-duration", "client@example.com");
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
            .expect("create fixed-duration free Host quote");
        assert_eq!(quote.items[0].free_duration_days, Some(7));
        let committed = store
            .client_market_commit_quote(
                &quote.id,
                &client,
                &[(
                    quote.items[0].id.clone(),
                    "duration-snapshot".into(),
                    "secret".into(),
                    quote.items[0].offer_revision,
                )],
            )
            .await
            .expect("commit fixed-duration free Host quote");
        let activated_at = Utc::now();
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE router_ssh_hosts SET free_duration_days = 30 WHERE id = ?1",
                params![host.id],
            )
            .unwrap();
            let tx = conn.transaction().unwrap();
            crate::client_market_trade::complete_provisioning_tx(
                &tx,
                &committed.job_ids[0],
                &host.id,
                "duration-snapshot-installation",
                "https://client.example.test",
                activated_at,
            )
            .expect("activate quoted free Host subscription");
            tx.commit().unwrap();
        }
        let conn = store.conn.lock().await;
        let (free_duration_days, stored_activated_at, expires_at): (i64, String, String) = conn
            .query_row(
                "SELECT free_duration_days, activated_at, expires_at
                 FROM client_market_subscriptions
                 WHERE installation_id = 'duration-snapshot-installation'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(free_duration_days, 7);
        assert_eq!(
            chrono::DateTime::parse_from_rfc3339(&stored_activated_at)
                .unwrap()
                .with_timezone(&Utc),
            activated_at
        );
        assert_eq!(
            chrono::DateTime::parse_from_rfc3339(&expires_at)
                .unwrap()
                .with_timezone(&Utc),
            activated_at + chrono::Duration::days(7)
        );
        drop(conn);
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn free_host_expiry_is_retryable_once_and_permanent_access_does_not_expire() {
        let (store, _config, root) = test_store("free-duration-expiry");
        let expiring_host = add_provider_host(
            &store,
            "provider-expiring",
            "provider@example.com",
            "198.18.53.1",
            "US",
            None,
        )
        .await;
        let permanent_host = add_provider_host(
            &store,
            "provider-permanent",
            "permanent@example.com",
            "198.18.53.2",
            "US",
            None,
        )
        .await;
        let now = Utc::now();
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "INSERT INTO client_market_subscriptions
                    (installation_id, host_id, provider_id, host_owner_email,
                     client_user_id, client_owner_email, status, daily_rate_minor,
                     currency, free_duration_days, offer_revision, activated_at, expires_at,
                     created_at, updated_at)
                 VALUES ('expired-free-client', ?1, 'provider-expiring', 'provider@example.com',
                         'client-expiring', 'client@example.com', 'active', NULL, NULL, 1, 1,
                         ?2, ?3, ?2, ?2),
                        ('permanent-free-client', ?4, 'provider-permanent', 'permanent@example.com',
                         'client-permanent', 'client2@example.com', 'active', NULL, NULL, NULL, 1,
                         ?2, NULL, ?2, ?2)",
                params![
                    expiring_host.id,
                    now.to_rfc3339(),
                    (now - chrono::Duration::seconds(1)).to_rfc3339(),
                    permanent_host.id,
                ],
            )
            .unwrap();
        }
        assert_eq!(
            store
                .client_market_reconcile_trade_state(now)
                .await
                .unwrap(),
            vec!["expired-free-client".to_string()]
        );
        {
            let conn = store.conn.lock().await;
            let tx = conn.transaction().unwrap();
            crate::client_market_trade::cleanup_started_tx(
                &tx,
                "expired-free-client",
                &expiring_host.id,
                None,
                None,
                "free_period_expired",
                false,
                now,
            )
            .unwrap();
            tx.commit().unwrap();
        }
        assert!(
            store
                .client_market_reconcile_trade_state(now + chrono::Duration::days(400))
                .await
                .unwrap()
                .is_empty()
        );
        let conn = store.conn.lock().await;
        let expired_events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM client_market_subscription_events
                 WHERE installation_id = 'expired-free-client'
                   AND event_type = 'free_period_expired'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(expired_events, 1);
        let permanent_status: String = conn
            .query_row(
                "SELECT status FROM client_market_subscriptions
                 WHERE installation_id = 'permanent-free-client'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(permanent_status, "active");
        drop(conn);
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
                     client_user_id, client_owner_email, status, daily_rate_minor,
                     offer_revision,
                     created_at, updated_at)
                 VALUES ('blocked-client', ?1, 'provider-block', 'provider-old@example.com',
                         'client-block', 'client-old@example.com', 'active', 500,
                         1, ?2, ?2)",
                params![host.id, now],
            )
            .unwrap();
        }
        store
            .client_market_begin_cleanup_job_with_context(
                "blocked-client",
                Some("provider-block"),
                "provider-new@example.com",
                false,
                Some("provider"),
                "provider_release",
                Some(true),
            )
            .await
            .expect("Provider cleanup after email change");
        {
            let conn = store.conn.lock().await;
            let provider_email: String = conn
                .query_row(
                    "SELECT owner_email FROM host_provider_profiles WHERE provider_id = ?1",
                    params!["provider-block"],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(provider_email, "provider-new@example.com");
            assert!(
                !crate::market_access::product_access_allowed_tx(
                    &conn,
                    "provider-block",
                    "client-block",
                    "client-old@example.com",
                    crate::market_access::PRODUCT_CLIENT_HOST,
                    crate::market_access::PRICING_PAID,
                )
                .expect("read denied Client access")
            );
            crate::market_access::set_product_access_decision_tx(
                &conn,
                "provider-block",
                "provider-new@example.com",
                "client-block",
                "client-old@example.com",
                crate::market_access::PRODUCT_CLIENT_HOST,
                crate::market_access::PRICING_PAID,
                crate::market_access::DECISION_ALLOW,
                "provider-block",
                &Utc::now().to_rfc3339(),
            )
            .expect("allow Client access again");
        }
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn provider_release_without_deny_and_self_rental_never_denies_access() {
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
        )
        .await;
        let second = add_provider_host(
            &store,
            provider_id,
            provider_email,
            "198.18.25.2",
            "US",
            Some(500),
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
                         client_user_id, client_owner_email, status, daily_rate_minor,
                         offer_revision, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', 500, 1, ?7, ?7)",
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
                Some("provider"),
                "provider_release",
                Some(false),
            )
            .await
            .expect("ordinary Provider release");
        {
            let conn = store.conn.lock().await;
            assert!(
                crate::market_access::product_access_allowed_tx(
                    &conn,
                    provider_id,
                    "renter",
                    "renter@example.com",
                    crate::market_access::PRODUCT_CLIENT_HOST,
                    crate::market_access::PRICING_PAID,
                )
                .expect("ordinary release preserves Client access")
            );
        }
        store
            .client_market_begin_cleanup_job_with_context(
                "provider-self-client",
                Some(provider_id),
                provider_email,
                false,
                Some("provider"),
                "provider_release",
                Some(true),
            )
            .await
            .expect("self-rental release");
        {
            let conn = store.conn.lock().await;
            assert!(
                crate::market_access::product_access_allowed_tx(
                    &conn,
                    provider_id,
                    provider_id,
                    provider_email,
                    crate::market_access::PRODUCT_CLIENT_HOST,
                    crate::market_access::PRICING_PAID,
                )
                .expect("self-rental access remains allowed")
            );
        }

        drop(store);
        let _ = std::fs::remove_dir_all(root);
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
    fn remote_helpers_detect_linux_comm_truncation() {
        assert!(
            REMOTE_CC_SWITCH_SERVER_HELPERS.contains("cc-switch-serve"),
            "Linux TASK_COMM_LEN truncates cc-switch-server to cc-switch-serve"
        );
        assert!(
            REMOTE_CC_SWITCH_SERVER_HELPERS.contains("cc_switch_server_list_pids"),
            "cleanup must list pids via cmdline/comm before kill"
        );
        assert!(
            REMOTE_CC_SWITCH_SERVER_HELPERS.contains("kill -9"),
            "stop must escalate to SIGKILL when SIGTERM is ignored"
        );
        assert!(
            REMOTE_CC_SWITCH_SERVER_HELPERS
                .contains("systemctl disable --now cc-switch-server.service"),
            "cleanup must disable systemd before killing the process"
        );
        assert!(
            REMOTE_CC_SWITCH_SERVER_HELPERS.contains("rc-update del cc-switch-server default"),
            "cleanup must disable OpenRC before killing the process"
        );
    }

    #[test]
    fn recovery_remote_markers_are_strictly_parsed() {
        assert_eq!(
            parse_client_recovery_remote_outcome("noise\nCC_SWITCH_RECOVERY=already_running\n")
                .unwrap(),
            ClientRecoveryRemoteOutcome::AlreadyRunning
        );
        assert_eq!(
            parse_client_recovery_remote_outcome("CC_SWITCH_RECOVERY=started:systemd\n").unwrap(),
            ClientRecoveryRemoteOutcome::Started {
                method: "systemd".into()
            }
        );
        assert_eq!(
            parse_client_recovery_remote_outcome("CC_SWITCH_RECOVERY=start_failed:openrc\n")
                .unwrap(),
            ClientRecoveryRemoteOutcome::StartFailed {
                method: "openrc".into()
            }
        );
        assert!(
            parse_client_recovery_remote_outcome("CC_SWITCH_RECOVERY=started:unknown\n").is_err()
        );
        assert!(parse_client_recovery_remote_outcome("unstructured output").is_err());
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
                "cc-switch-server still running after wipe".into()
            )),
            CLEANUP_FAILURE_STOP
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
        assert_eq!(
            classify_cleanup_failure(&AppError::BadRequest(
                "ssh failed (exit status: 255): connect to host 192.0.2.10 port 22: Connection refused"
                    .into()
            )),
            CLEANUP_FAILURE_SSH_UNREACHABLE
        );
        assert_eq!(
            classify_cleanup_failure(&AppError::BadRequest(
                "WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! Host key verification failed"
                    .into()
            )),
            CLEANUP_FAILURE_FINGERPRINT
        );
    }

    #[test]
    fn cleanup_recovery_backoff_is_bounded_and_terminal() {
        let now = Utc::now();
        let expected_seconds = [5 * 60, 15 * 60, 60 * 60, 6 * 60 * 60, 24 * 60 * 60];
        for (completed_attempts, expected) in expected_seconds.into_iter().enumerate() {
            let next = cleanup_recovery_next_at(now, completed_attempts as u32)
                .expect("retry remains scheduled");
            assert_eq!((next - now).num_seconds(), expected);
        }
        assert!(cleanup_recovery_next_at(now, 5).is_none());
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
        )
        .await;
        let now = Utc::now();
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "INSERT INTO client_market_subscriptions
                    (installation_id, host_id, provider_id, host_owner_email,
                     client_user_id, client_owner_email, status, daily_rate_minor,
                     offer_revision,
                     created_at, updated_at)
                 VALUES ('stuck-client', ?1, 'provider-stuck', 'provider@example.com',
                         'client-stuck', 'client@example.com', 'release_failed', 500,
                         1, ?2, ?2)",
                params![host.id, now.to_rfc3339()],
            )
            .unwrap();
        }

        let client = market_session("client-stuck", "client@example.com");
        assert!(
            store
                .client_market_assert_creation_allowed(&client)
                .await
                .is_err(),
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

    #[tokio::test]
    async fn client_owner_can_finalize_only_a_wedged_release_without_an_active_cleanup() {
        let (store, _config, root) = test_store("trade-owner-finalize-release");
        let host = add_provider_host(
            &store,
            "provider-owner-finalize",
            "provider@example.com",
            "198.18.30.2",
            "US",
            None,
        )
        .await;
        let now = Utc::now().to_rfc3339();
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "INSERT INTO client_market_subscriptions
                    (installation_id, host_id, provider_id, host_owner_email,
                     client_user_id, client_owner_email, status, offer_revision,
                     created_at, updated_at)
                 VALUES ('owner-stuck-client', ?1, 'provider-owner-finalize',
                         'provider@example.com', 'owner-stuck', 'owner@example.com',
                         'releasing', 1, ?2, ?2)",
                params![host.id, now],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO provisioning_jobs
                    (id, type, host_id, installation_id, status, phase, log_blob,
                     created_at, updated_at)
                 VALUES ('owner-stuck-cleanup', 'cleanup', ?1, 'owner-stuck-client',
                         'running', 'cleanup_stop', '', ?2, ?2)",
                params![host.id, now],
            )
            .unwrap();
        }

        let owner = market_session("owner-stuck", "owner@example.com");
        assert!(matches!(
            store
                .client_market_finalize_release_for_owner(
                    "owner-stuck-client",
                    &market_session("stranger", "stranger@example.com")
                )
                .await,
            Err(AppError::Forbidden(_))
        ));
        assert!(matches!(
            store
                .client_market_finalize_release_for_owner("owner-stuck-client", &owner)
                .await,
            Err(AppError::Conflict(_))
        ));
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE provisioning_jobs SET status = 'failed', phase = 'complete'
                 WHERE id = 'owner-stuck-cleanup'",
                [],
            )
            .unwrap();
        }
        let outcome = store
            .client_market_finalize_release_for_owner("owner-stuck-client", &owner)
            .await
            .expect("owner finalizes a cleanup with no active job");
        assert_eq!(outcome.previous_status, "releasing");
        assert_eq!(outcome.status, "released");
        assert_eq!(
            store
                .client_market_get_host(&host.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            HOST_STATUS_IDLE,
            "finalizing the rental must not change Host disposition"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn cleanup_recovery_state_backs_off_stops_and_manual_retry_resets_it() {
        let (store, _config, root) = test_store("cleanup-recovery-backoff");
        let host = add_provider_host(
            &store,
            "provider-cleanup-backoff",
            "provider@example.com",
            "198.18.30.3",
            "US",
            None,
        )
        .await;
        let now = Utc::now().to_rfc3339();
        {
            let conn = store.conn.lock().await;
            insert_installation(&conn, "cleanup-backoff-client", "client@example.com");
            conn.execute(
                "UPDATE installations SET provision_source = ?2, provision_host_id = ?3
                 WHERE id = ?1",
                params![
                    "cleanup-backoff-client",
                    PROVISION_SOURCE_ROUTER_MARKET,
                    host.id
                ],
            )
            .unwrap();
            insert_tunnel_and_public_host(
                &conn,
                "cleanup-backoff-client",
                "client@example.com",
                "cleanup-backoff-client",
            );
            conn.execute(
                "UPDATE installation_client_tunnels SET enabled = 0
                 WHERE installation_id = 'cleanup-backoff-client'",
                [],
            )
            .unwrap();
            conn.execute(
                "UPDATE router_ssh_hosts
                 SET status = 'draining', installation_id = 'cleanup-backoff-client'
                 WHERE id = ?1",
                params![host.id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO client_market_subscriptions
                    (installation_id, host_id, provider_id, host_owner_email,
                     client_user_id, client_owner_email, status, offer_revision,
                     created_at, updated_at)
                 VALUES ('cleanup-backoff-client', ?1, 'provider-cleanup-backoff',
                         'provider@example.com', 'client-backoff', 'client@example.com',
                         'releasing', 1, ?2, ?2)",
                params![host.id, now],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO provisioning_jobs
                    (id, type, host_id, host_owner_email, client_owner_email, subdomain,
                     installation_id, status, phase, log_blob, created_at, updated_at)
                 VALUES ('cleanup-backoff-job', 'cleanup', ?1, 'provider@example.com',
                         'client@example.com', 'cleanup-backoff-client',
                         'cleanup-backoff-client', 'running', 'cleanup_stop', '', ?2, ?2)",
                params![host.id, now],
            )
            .unwrap();
        }
        store
            .client_market_fail_cleanup_job(
                "cleanup-backoff-job",
                &host.id,
                CLEANUP_FAILURE_SSH_UNREACHABLE,
                "connection refused",
            )
            .await
            .expect("persist failed cleanup and initial retry");

        let mut due_at = {
            let conn = store.conn.lock().await;
            let (attempt_count, next_attempt_at): (i64, String) = conn
                .query_row(
                    "SELECT attempt_count, next_attempt_at
                     FROM client_market_cleanup_recovery_state WHERE host_id = ?1",
                    params![host.id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(attempt_count, 0);
            DateTime::parse_from_rfc3339(&next_attempt_at)
                .unwrap()
                .with_timezone(&Utc)
        };
        assert!(
            store
                .client_market_claim_due_cleanup_recoveries(
                    due_at - chrono::Duration::seconds(1),
                    1
                )
                .await
                .unwrap()
                .is_empty()
        );

        for expected_attempt in 1..=5 {
            let claims = store
                .client_market_claim_due_cleanup_recoveries(due_at, 1)
                .await
                .expect("claim due cleanup recovery");
            assert_eq!(claims.len(), 1);
            assert_eq!(claims[0].attempt_count, expected_attempt);
            store
                .client_market_finish_cleanup_recovery_attempt(
                    &claims[0],
                    "probe_failed:cleanup_ssh_unreachable",
                    due_at,
                )
                .await
                .expect("finish cleanup recovery attempt");
            store
                .client_market_finish_cleanup_recovery_attempt(
                    &claims[0],
                    "late_duplicate_result",
                    due_at + chrono::Duration::seconds(1),
                )
                .await
                .expect("discard duplicate cleanup recovery result");
            let conn = store.conn.lock().await;
            let (next, stopped, outcome): (Option<String>, Option<String>, String) = conn
                .query_row(
                    "SELECT next_attempt_at, stopped_at, last_outcome
                     FROM client_market_cleanup_recovery_state WHERE host_id = ?1",
                    params![host.id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(outcome, "probe_failed:cleanup_ssh_unreachable");
            if expected_attempt < 5 {
                assert!(stopped.is_none());
                due_at = DateTime::parse_from_rfc3339(next.as_deref().unwrap())
                    .unwrap()
                    .with_timezone(&Utc);
            } else {
                assert!(next.is_none());
                assert!(stopped.is_some());
            }
        }

        store
            .client_market_begin_cleanup_job(
                "cleanup-backoff-client",
                "provider@example.com",
                false,
            )
            .await
            .expect("manual cleanup retry remains available");
        let conn = store.conn.lock().await;
        let recovery_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM client_market_cleanup_recovery_state WHERE host_id = ?1",
                params![host.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(recovery_rows, 0);
        drop(conn);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn provider_can_permanently_retire_only_a_fenced_unreachable_host() {
        let (store, _config, root) = test_store("retire-unreachable-host");
        let host = add_provider_host(
            &store,
            "provider-retire-lost",
            "provider@example.com",
            "198.18.30.4",
            "US",
            None,
        )
        .await;
        let now = Utc::now().to_rfc3339();
        {
            let conn = store.conn.lock().await;
            insert_installation(&conn, "retire-lost-client", "client@example.com");
            conn.execute(
                "UPDATE installations SET provision_source = ?2, provision_host_id = ?3
                 WHERE id = ?1",
                params![
                    "retire-lost-client",
                    PROVISION_SOURCE_ROUTER_MARKET,
                    host.id
                ],
            )
            .unwrap();
            insert_tunnel_and_public_host(
                &conn,
                "retire-lost-client",
                "client@example.com",
                "retire-lost-client",
            );
            conn.execute(
                "UPDATE router_ssh_hosts
                 SET status = 'unreachable', installation_id = 'retire-lost-client'
                 WHERE id = ?1",
                params![host.id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO client_market_subscriptions
                    (installation_id, host_id, provider_id, host_owner_email,
                     client_user_id, client_owner_email, status, offer_revision,
                     created_at, updated_at)
                 VALUES ('retire-lost-client', ?1, 'provider-retire-lost',
                         'provider@example.com', 'client-retire-lost', 'client@example.com',
                         'release_failed', 1, ?2, ?2)",
                params![host.id, now],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO client_market_cleanup_recovery_state
                    (host_id, installation_id, attempt_count, next_attempt_at,
                     last_outcome, updated_at)
                 VALUES (?1, 'retire-lost-client', 1, ?2,
                         'probe_failed:cleanup_ssh_unreachable', ?2)",
                params![host.id, now],
            )
            .unwrap();
        }
        let provider = market_session("provider-retire-lost", "provider@example.com");
        assert!(matches!(
            store
                .client_market_retire_unreachable_host(
                    &host.id,
                    &market_session("stranger", "stranger@example.com")
                )
                .await,
            Err(AppError::Forbidden(_))
        ));
        assert!(matches!(
            store
                .client_market_retire_unreachable_host(&host.id, &provider)
                .await,
            Err(AppError::Conflict(_))
        ));
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE installation_client_tunnels SET enabled = 0
                 WHERE installation_id = 'retire-lost-client'",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO provisioning_jobs
                    (id, type, host_id, installation_id, status, phase, log_blob,
                     created_at, updated_at)
                 VALUES ('retire-lost-active-job', 'cleanup', ?1, 'retire-lost-client',
                         'running', 'cleanup_stop', '', ?2, ?2)",
                params![host.id, now],
            )
            .unwrap();
        }
        assert!(matches!(
            store
                .client_market_retire_unreachable_host(&host.id, &provider)
                .await,
            Err(AppError::Conflict(_))
        ));
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE provisioning_jobs SET status = 'failed', phase = 'complete'
                 WHERE id = 'retire-lost-active-job'",
                [],
            )
            .unwrap();
        }

        let outcome = store
            .client_market_retire_unreachable_host(&host.id, &provider)
            .await
            .expect("permanently retire fenced lost Host");
        assert_eq!(outcome.installation_id, "retire-lost-client");
        assert_eq!(outcome.previous_subscription_status, "release_failed");
        assert_eq!(outcome.status, "retired");
        assert_eq!(outcome.subdomain.as_deref(), Some("retire-lost-client"));
        assert!(
            store
                .client_market_get_host(&host.id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .installation_provision_source("retire-lost-client")
                .await
                .unwrap()
                .is_none()
        );
        {
            let conn = store.conn.lock().await;
            let subscription_status: String = conn
                .query_row(
                    "SELECT status FROM client_market_subscriptions
                     WHERE installation_id = 'retire-lost-client'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(subscription_status, "released");
            let recovery_rows: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM client_market_cleanup_recovery_state
                     WHERE host_id = ?1",
                    params![host.id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(recovery_rows, 0);
            assert_eq!(
                get_public_host(&conn, "retire-lost-client")
                    .unwrap()
                    .unwrap()
                    .lifecycle,
                PublicHostLifecycle::Tombstoned
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    /// Providers may allocate their own Hosts — self-host then self-use is supported.
    #[tokio::test]
    async fn providers_may_quote_their_own_hosts() {
        use crate::client_market_trade::CreateQuoteRequest;

        let (store, _config, root) = test_store("trade-self-use");
        let host = add_provider_host(
            &store,
            "provider-self",
            "self@example.com",
            "198.18.31.1",
            "US",
            None,
        )
        .await;
        let paid_host = add_provider_host(
            &store,
            "provider-self",
            "self@example.com",
            "198.18.31.2",
            "US",
            Some(500),
        )
        .await;

        let owner = market_session("provider-self", "self@example.com");
        let self_paid_quote = store
            .client_market_create_quote(
                &owner,
                CreateQuoteRequest {
                    provider_ids: vec!["provider-self".into()],
                    country_codes: vec!["US".into()],
                    count: 1,
                    host_id: Some(paid_host.id.clone()),
                },
            )
            .await
            .expect("a Provider must be able to quote their own Host by id");
        assert_eq!(self_paid_quote.items[0].host_id, paid_host.id);
        assert!(self_paid_quote.items[0].daily_rate_minor.is_none());
        assert!(self_paid_quote.items[0].currency.is_none());

        // Release the reserved Host so a follow-up quote can claim it again.
        {
            let conn = store.conn.lock().await;
            let past = (Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
            conn.execute(
                "UPDATE client_market_allocation_quotes SET status = 'expired', expires_at = ?1, updated_at = ?1",
                params![past],
            )
            .unwrap();
            conn.execute(
                "UPDATE router_ssh_hosts SET status = 'idle', updated_at = ?1 WHERE id = ?2",
                params![past, paid_host.id],
            )
            .unwrap();
        }

        let random = store
            .client_market_create_quote(
                &owner,
                CreateQuoteRequest {
                    provider_ids: vec!["provider-self".into()],
                    country_codes: vec!["US".into()],
                    count: 1,
                    host_id: None,
                },
            )
            .await
            .expect("random selection must include the caller's own free Host");
        assert_eq!(random.items[0].host_id, host.id);

        {
            let conn = store.conn.lock().await;
            let past = (Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
            conn.execute(
                "UPDATE client_market_allocation_quotes SET status = 'expired', expires_at = ?1, updated_at = ?1",
                params![past],
            )
            .unwrap();
            conn.execute(
                "UPDATE router_ssh_hosts SET status = 'idle', updated_at = ?1 WHERE id = ?2",
                params![past, host.id],
            )
            .unwrap();
        }

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
        let old = Utc::now() - chrono::Duration::minutes(11);
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE router_ssh_hosts SET updated_at = ?2 WHERE id = ?1",
                params![host.id, old.to_rfc3339()],
            )
            .expect("age orphaned reservation");
        }
        reconcile_stranded_reserved_hosts(&store, Utc::now())
            .await
            .expect("recover orphaned reserved Host");
        assert_eq!(
            store
                .client_market_get_host(&host.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            HOST_STATUS_IDLE
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
