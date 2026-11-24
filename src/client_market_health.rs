//! Managed Host health ingestion, projections and enrollment.
use super::*;
use crate::db::OptionalExtension;
use crate::host_health_protocol::{self as wire, Report, Snapshot};
use axum::extract::DefaultBodyLimit;
use axum::response::{IntoResponse, Response};
use ed25519_dalek::{Signature, VerifyingKey};
use rand::Rng;
use serde_json::json;
use tracing::warn;
#[path = "client_market_health_bootstrap.rs"]
pub(crate) mod bootstrap;
#[path = "client_market_health_deployment.rs"]
mod deployment;
pub(super) use deployment::{
    claim_preparation, enroll, install_operation, remote_fence, uninstall,
};

fn database(error: impl std::fmt::Display) -> AppError {
    AppError::Internal(format!("Host health database: {error}"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub state: String,
    pub last_received_at: Option<String>,
    pub sampled_at: Option<String>,
    pub next_due_at: Option<String>,
    pub received24h: u32,
    pub observable24h: u32,
    pub consecutive_missed: u32,
    pub supervision: String,
    /// Without kernel time sync the reporter pauses after a reboot until time is confirmed.
    #[serde(default = "clock_synchronized_default")]
    pub clock_synchronized: bool,
    pub snapshot: Option<Snapshot>,
    pub diagnostic: Option<String>,
    #[serde(default)]
    pub machine_epoch: i64,
    #[serde(default)]
    pub deployment: DeploymentView,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentView {
    pub state: String,
    pub reason: Option<String>,
    pub can_bootstrap: bool,
    pub deployed_commit: Option<String>,
}

impl Default for DeploymentView {
    fn default() -> Self {
        Self {
            state: "not_enabled".into(),
            reason: None,
            can_bootstrap: false,
            deployed_commit: None,
        }
    }
}

fn clock_synchronized_default() -> bool {
    true
}

impl Summary {
    fn preparing() -> Self {
        Self {
            state: "not_enabled".into(),
            last_received_at: None,
            sampled_at: None,
            next_due_at: None,
            received24h: 0,
            observable24h: 0,
            consecutive_missed: 0,
            supervision: "unknown".into(),
            clock_synchronized: true,
            snapshot: None,
            diagnostic: None,
            machine_epoch: 1,
            deployment: DeploymentView::default(),
        }
    }
}

fn iso(seconds: i64) -> String {
    DateTime::<Utc>::from_timestamp(seconds, 0)
        .unwrap_or_default()
        .to_rfc3339()
}

pub(super) fn routes() -> Router<ServerState> {
    Router::new()
        .route(
            "/v1/client-market/host-heartbeats",
            post(receive).layer(DefaultBodyLimit::max(wire::MAX_BODY)),
        )
        .route("/v1/client-market/hosts/:id/health", get(details))
        .route("/v1/client-market/hosts/:id/health-calendar", get(calendar))
        .route("/v1/client-market/hosts/:id/health/retry", post(retry))
        .route(
            "/v1/client-market/hosts/:id/health/bootstrap",
            post(bootstrap::request),
        )
        .route(
            "/v1/client-market/hosts/:id/health/rotate-key",
            post(rotate),
        )
        .route(
            "/v1/client-market/hosts/:id/health/replace",
            post(replace_machine),
        )
}

fn error_response(status: StatusCode, code: &str, report: Option<&Report>, now: i64) -> Response {
    let mut body = json!({"code":code,"message":code,"routerTime":iso(now)});
    if let Some(report) = report {
        body["hostId"] = json!(report.host_id);
        body["machineEpoch"] = json!(report.machine_epoch);
        body["keyGeneration"] = json!(report.key_generation);
        body["utcHour"] = json!(report.utc_hour);
    }
    (status, [(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}

async fn receive(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Result<Bytes, axum::extract::rejection::BytesRejection>,
) -> Result<Response, AppError> {
    let now = Utc::now().timestamp();
    match body {
        Ok(body) => ingest(&state.store, headers, body, now).await,
        Err(error) => Ok(error_response(
            error.status(),
            if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
                "report_too_large"
            } else {
                "invalid_report"
            },
            None,
            now,
        )),
    }
}

async fn ingest(
    store: &AppStore,
    headers: HeaderMap,
    body: Bytes,
    now: i64,
) -> Result<Response, AppError> {
    if body.len() > wire::MAX_BODY {
        return Ok(error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "report_too_large",
            None,
            now,
        ));
    }
    let get = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    if get("X-CC-Report-Version") != Some("1") {
        return Ok(error_response(
            StatusCode::BAD_REQUEST,
            "unsupported_report_version",
            None,
            now,
        ));
    }
    let host_id = get("X-CC-Host-Id").filter(|id| !id.is_empty() && id.len() <= 64);
    let epoch = get("X-CC-Machine-Epoch")
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0);
    let generation = get("X-CC-Key-Generation")
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0);
    let (Some(host_id), Some(epoch), Some(generation)) = (host_id, epoch, generation) else {
        return Ok(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_identity",
            None,
            now,
        ));
    };
    let public_key = {
        let conn = store.conn.lock().await;
        conn.query_row(
            "SELECT public_key FROM client_host_agents WHERE host_id=?1 AND machine_epoch=?2 AND key_generation=?3
             UNION ALL SELECT public_key FROM client_host_revoked_keys WHERE host_id=?1 AND machine_epoch=?2 AND key_generation=?3 LIMIT 1",
            params![host_id, epoch, generation], |row| row.get::<_, Option<String>>(0),
        ).optional().map_err(database)?.flatten()
    };
    let valid_signature = public_key
        .as_deref()
        .and_then(|value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(value)
                .ok()
        })
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .and_then(|bytes| VerifyingKey::from_bytes(&bytes).ok())
        .zip(
            get("X-CC-Report-Signature")
                .and_then(|v| {
                    base64::engine::general_purpose::URL_SAFE_NO_PAD
                        .decode(v)
                        .ok()
                })
                .and_then(|bytes| Signature::from_slice(&bytes).ok()),
        )
        .is_some_and(|(key, signature)| {
            key.verify_strict(&wire::signing_message(&body), &signature)
                .is_ok()
        });
    if !valid_signature {
        return Ok(error_response(
            StatusCode::UNAUTHORIZED,
            "invalid_signature",
            None,
            now,
        ));
    }
    let report: Report = match serde_json::from_slice(&body) {
        Ok(report) => report,
        Err(_) => {
            return Ok(error_response(
                StatusCode::BAD_REQUEST,
                "invalid_report",
                None,
                now,
            ));
        }
    };
    if !report.validate()
        || get("X-CC-Host-Id") != Some(report.host_id.as_str())
        || get("X-CC-Machine-Epoch").and_then(|v| v.parse().ok()) != Some(report.machine_epoch)
        || get("X-CC-Key-Generation").and_then(|v| v.parse().ok()) != Some(report.key_generation)
    {
        return Ok(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_report",
            None,
            now,
        ));
    }
    let conn = store.conn.lock().await;
    let tx = conn.transaction().map_err(database)?;
    let binding: Option<(i64,i64,Option<String>,i64,Option<i64>)> = tx.query_row(
        "SELECT machine_epoch,key_generation,public_key,revoked,first_due_at FROM client_host_agents WHERE host_id=?1",
        params![report.host_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
    ).optional().map_err(database)?;
    let current = binding
        .as_ref()
        .is_some_and(|(epoch, generation, key, revoked, _)| {
            *epoch == report.machine_epoch as i64
                && *generation == report.key_generation as i64
                && *revoked == 0
                && key.as_ref() == public_key.as_ref()
        });
    if !current {
        let revoked: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM client_host_revoked_keys WHERE host_id=?1 AND machine_epoch=?2 AND key_generation=?3 AND public_key=?4)",
            params![report.host_id,report.machine_epoch as i64,report.key_generation as i64,public_key], |row| row.get(0),
        ).map_err(database)?;
        return Ok(error_response(
            if revoked {
                StatusCode::GONE
            } else {
                StatusCode::UNAUTHORIZED
            },
            if revoked {
                "host_agent_revoked"
            } else {
                "invalid_binding"
            },
            Some(&report),
            now,
        ));
    }
    let digest = hex::encode(Sha256::digest(&body));
    let existing: Option<String> = tx
        .query_row(
            "SELECT body_digest FROM client_host_hour_reports WHERE host_id=?1 AND utc_hour=?2",
            params![report.host_id, report.utc_hour],
            |row| row.get(0),
        )
        .optional()
        .map_err(database)?;
    if let Some(existing) = existing {
        if existing != digest {
            tx.execute("INSERT INTO client_host_report_conflicts(host_id,utc_hour,machine_epoch,last_seen_at) VALUES(?1,?2,?3,?4)
                ON CONFLICT(host_id,utc_hour) DO UPDATE SET last_seen_at=excluded.last_seen_at",
                params![report.host_id,report.utc_hour,report.machine_epoch as i64,now]).map_err(database)?;
            // Bumping the sequence supersedes an enrollment already in flight, which would
            // otherwise clear this request when it completes.
            tx.execute("UPDATE client_host_agents SET last_error='hour_conflict',maintenance_pending=1,operation_sequence=operation_sequence+1 WHERE host_id=?1",params![report.host_id]).map_err(database)?;
            tx.commit().map_err(database)?;
            return Ok(error_response(
                StatusCode::CONFLICT,
                "hour_conflict",
                Some(&report),
                now,
            ));
        }
        return Ok(error_response(
            StatusCode::OK,
            "accepted",
            Some(&report),
            now,
        ));
    }
    let first = binding
        .and_then(|b| b.4)
        .ok_or_else(|| AppError::Conflict("Host schedule is not ready".into()))?;
    let due = if report.utc_hour == first / 3600 {
        first
    } else {
        wire::due_at(&report.host_id, report.utc_hour)
    };
    let sampled = DateTime::parse_from_rfc3339(&report.sampled_at)
        .ok()
        .filter(|time| time.offset().local_minus_utc() == 0)
        .map(|t| t.timestamp());
    if report.utc_hour < first / 3600
        || now < (report.utc_hour * 3600).max(due - wire::EARLY_SECONDS)
        || now > due + wire::GRACE_SECONDS
        || sampled.is_none_or(|t| {
            t < due - wire::EARLY_SECONDS - 1
                || t > (report.utc_hour + 1) * 3600 + wire::EARLY_SECONDS
                || t > now + wire::EARLY_SECONDS
                || t < now - wire::GRACE_SECONDS - wire::EARLY_SECONDS
        })
    {
        return Ok(error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "report_outside_window",
            Some(&report),
            now,
        ));
    }
    let sampled = sampled.unwrap();
    let (public, lease) = sample_visibility(&tx, &report.host_id, sampled - 121, now)?;
    let report_json = serde_json::to_string(&report).map_err(database)?;
    tx.execute(
        "INSERT INTO client_host_hour_reports(host_id,utc_hour,machine_epoch,key_generation,body_digest,received_at,sampled_at,report_json,public_sample,lease_id)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![report.host_id,report.utc_hour,report.machine_epoch as i64,report.key_generation as i64,digest,now,sampled,report_json,public,lease],
    ).map_err(database)?;
    tx.execute(
        "UPDATE client_host_agents SET admitted_hour=COALESCE(admitted_hour,?2),legacy=0,readmission_pending=0,
          latest_hour=CASE WHEN latest_hour IS NULL OR latest_hour<?2 THEN ?2 ELSE latest_hour END,
          latest_sample_json=CASE WHEN latest_hour IS NULL OR latest_hour<?2 THEN ?3 ELSE latest_sample_json END,
          latest_received_at=CASE WHEN latest_hour IS NULL OR latest_hour<?2 THEN ?4 ELSE latest_received_at END,
          latest_public=CASE WHEN latest_hour IS NULL OR latest_hour<?2 THEN ?5 ELSE latest_public END,
          latest_lease_id=CASE WHEN latest_hour IS NULL OR latest_hour<?2 THEN ?6 ELSE latest_lease_id END
         WHERE host_id=?1",
        params![report.host_id,report.utc_hour,report_json,now,public,lease],
    ).map_err(database)?;
    // A replaced machine that was disabled or quarantined returns to that state.
    let admitted = tx.execute("UPDATE router_ssh_hosts SET status=COALESCE((SELECT resume_status FROM client_host_agents WHERE host_id=?1),'idle'),updated_at=?2 WHERE id=?1 AND status='onboarding'",
        params![report.host_id,iso(now)]).map_err(database)?;
    if admitted == 1 {
        tx.execute(
            "UPDATE client_host_agents SET resume_status=NULL WHERE host_id=?1",
            params![report.host_id],
        )
        .map_err(database)?;
    }
    refresh_summary(&tx, &report.host_id, now)?;
    tx.commit().map_err(database)?;
    Ok(error_response(
        StatusCode::OK,
        "accepted",
        Some(&report),
        now,
    ))
}

fn sample_visibility(
    conn: &Connection,
    host: &str,
    earliest: i64,
    latest: i64,
) -> Result<(bool, Option<String>), AppError> {
    let mut query = conn
        .prepare(
            "SELECT installation_id,started_at,ended_at FROM client_host_tenancies WHERE host_id=?1
         AND started_at<=?3 AND (ended_at IS NULL OR ended_at>=?2)",
        )
        .map_err(database)?;
    let rows = query
        .query_map(params![host, earliest, latest], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })
        .map_err(database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database)?;
    let status: String = conn
        .query_row(
            "SELECT status FROM router_ssh_hosts WHERE id=?1",
            params![host],
            |row| row.get(0),
        )
        .map_err(database)?;
    if rows.is_empty() && matches!(status.as_str(), "idle" | "onboarding" | "maintaining") {
        return Ok((true, None));
    }
    if rows.len() == 1 {
        let (id, start, end) = &rows[0];
        if *start <= earliest && end.is_none_or(|end| end >= latest) {
            return Ok((false, Some(id.clone())));
        }
    }
    Ok((false, None))
}

fn unavailable(conn: &Connection, due: i64) -> Result<bool, AppError> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM client_host_receiver_outages WHERE started_at<=?1+30 AND ended_at>=?1-30)",
        params![due], |row|row.get(0)).map_err(database)
}

fn refresh_summary(conn: &Connection, host: &str, now: i64) -> Result<(), AppError> {
    let (first,admitted,latest,received,json,supervision,diagnostic,clock_synchronized): (Option<i64>,Option<i64>,Option<i64>,Option<i64>,Option<String>,String,Option<String>,bool) =
        conn.query_row("SELECT first_due_at,admitted_hour,latest_hour,latest_received_at,latest_sample_json,supervision,last_error,clock_synchronized FROM client_host_agents WHERE host_id=?1",
            params![host],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?))).map_err(database)?;
    let Some(first) = first else { return Ok(()) };
    let hour = now / 3600;
    let due = |h| {
        if h == first / 3600 {
            first
        } else {
            wire::due_at(host, h)
        }
    };
    let decided = if now >= due(hour) + wire::GRACE_SECONDS {
        hour
    } else if now >= due(hour - 1) + wire::GRACE_SECONDS {
        hour - 1
    } else {
        hour - 2
    };
    let mut reports = HashSet::new();
    let mut stmt=conn.prepare("SELECT utc_hour FROM client_host_hour_reports WHERE host_id=?1 AND machine_epoch=(SELECT machine_epoch FROM client_host_agents WHERE host_id=?1) AND utc_hour BETWEEN ?2 AND ?3").map_err(database)?;
    for row in stmt
        .query_map(params![host, decided - 23, decided], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(database)?
    {
        reports.insert(row.map_err(database)?);
    }
    let mut received_count = 0;
    let mut observed = 0;
    let mut consecutive = 0;
    let mut counting = true;
    let start = admitted.unwrap_or(i64::MAX).max(decided - 23);
    for h in (start..=decided).rev() {
        if reports.contains(&h) {
            received_count += 1;
            observed += 1;
            counting = false
        } else if unavailable(conn, due(h))? {
            counting = false
        } else {
            observed += 1;
            if counting {
                consecutive += 1
            }
        }
    }
    let healthy = latest.is_some_and(|h| h >= decided);
    let state = if admitted.is_none() {
        if now > first + wire::GRACE_SECONDS {
            "first_overdue"
        } else {
            "awaiting_first"
        }
    } else if healthy {
        "healthy"
    } else if unavailable(conn, due(decided))? {
        "unknown"
    } else {
        "missed"
    };
    let mut report = json.and_then(|value| serde_json::from_str::<Report>(&value).ok());
    if received.is_some_and(|time| now - time > 7 * 86400) {
        if let Some(report) = report.as_mut() {
            report.snapshot.hide_dynamic("expired");
            conn.execute(
                "UPDATE client_host_agents SET latest_sample_json=?2 WHERE host_id=?1",
                params![host, serde_json::to_string(report).map_err(database)?],
            )
            .map_err(database)?;
        }
    }
    let summary = Summary {
        state: state.into(),
        last_received_at: received.map(iso),
        sampled_at: report.as_ref().map(|r| r.sampled_at.clone()),
        next_due_at: Some(iso(if now < first {
            first
        } else if now < due(hour) {
            due(hour)
        } else {
            due(hour + 1)
        })),
        received24h: received_count,
        observable24h: observed,
        consecutive_missed: consecutive,
        supervision,
        clock_synchronized,
        snapshot: report.map(|mut r| {
            if received.is_some_and(|t| now - t > 7 * 86400) {
                r.snapshot.hide_dynamic("expired")
            }
            r.snapshot
        }),
        diagnostic,
        machine_epoch: 1,
        deployment: DeploymentView::default(),
    };
    let deadline = [hour - 1, hour, hour + 1]
        .into_iter()
        .filter(|h| *h >= first / 3600)
        .map(|h| due(h) + wire::GRACE_SECONDS)
        .filter(|deadline| *deadline > now)
        .min()
        .unwrap_or(first + wire::GRACE_SECONDS);
    conn.execute(
        "UPDATE client_host_agents SET summary_json=?2,next_deadline=?3 WHERE host_id=?1",
        params![
            host,
            serde_json::to_string(&summary).map_err(database)?,
            deadline
        ],
    )
    .map_err(database)?;
    Ok(())
}

pub(crate) fn public_summary(conn: &Connection, host: &str) -> Result<Option<Summary>, AppError> {
    Ok(project_views(conn, None, Some(host), false)?.remove(host))
}

pub(super) async fn views(
    state: &ServerState,
    viewer: Option<&crate::models::AuthSession>,
    host_filter: Option<&str>,
) -> Result<HashMap<String, Summary>, AppError> {
    let operations = if let Some(viewer) = viewer {
        state.dynamic.read().await.is_admin(&viewer.email)
    } else {
        false
    };
    let conn = state.store.conn.lock().await;
    project_views(&conn, viewer, host_filter, operations)
}

#[derive(Debug)]
struct DeploymentFacts {
    epoch: i64,
    registered: bool,
    first: Option<i64>,
    legacy: bool,
    revoked: bool,
    admitted: Option<i64>,
    preparation: Option<String>,
    bootstrap: Option<String>,
    bootstrap_error: Option<String>,
    deployed_commit: Option<String>,
    installation: Option<String>,
    eligible_identity: bool,
    no_running_job: bool,
}

fn project_views(
    conn: &Connection,
    viewer: Option<&crate::models::AuthSession>,
    host_filter: Option<&str>,
    operations: bool,
) -> Result<HashMap<String, Summary>, AppError> {
    let predicate = if host_filter.is_some() {
        "a.host_id=?1"
    } else {
        "?1 IS NULL"
    };
    let mut query=conn.prepare(&format!(
        "SELECT a.host_id,a.summary_json,a.latest_public,a.latest_lease_id,h.status,h.provider_id,s.installation_id,s.client_user_id,COALESCE(a.last_error,p.last_error),a.revoked,
         a.machine_epoch,a.public_key IS NOT NULL,a.first_due_at,a.legacy,a.admitted_hour,p.status,b.status,b.last_error,a.deployed_commit,h.installation_id,
         a.replacement_pending=0 AND a.readmission_pending=0 AND COALESCE(a.last_error,'') NOT IN ('rotation_requested','hour_conflict'),
         CASE WHEN a.legacy=1 AND h.status='allocated' THEN NOT EXISTS(SELECT 1 FROM provisioning_jobs j WHERE j.host_id=h.id AND j.status IN ('pending','running')) ELSE 0 END
         FROM client_host_agents a JOIN router_ssh_hosts h ON h.id=a.host_id
         LEFT JOIN client_market_subscriptions s ON s.host_id=a.host_id AND s.status IN ('active','billing_suspended')
         LEFT JOIN client_binary_preparations p ON p.host_id=a.host_id
         LEFT JOIN client_host_agent_bootstraps b ON b.host_id=a.host_id AND b.installation_id=h.installation_id
           AND b.machine_epoch=a.machine_epoch AND b.key_generation=a.key_generation WHERE {predicate}")).map_err(database)?;
    let rows = query
        .query_map(params![host_filter], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<String>>(8)?,
                DeploymentFacts {
                    epoch: r.get(10)?,
                    registered: r.get(11)?,
                    first: r.get(12)?,
                    legacy: r.get(13)?,
                    admitted: r.get(14)?,
                    preparation: r.get(15)?,
                    bootstrap: r.get(16)?,
                    bootstrap_error: r.get(17)?,
                    deployed_commit: r.get(18)?,
                    installation: r.get(19)?,
                    eligible_identity: r.get(20)?,
                    no_running_job: r.get(21)?,
                    revoked: r.get(9)?,
                },
            ))
        })
        .map_err(database)?;
    let mut out = HashMap::new();
    for row in rows {
        let (
            host,
            raw,
            public,
            lease,
            status,
            provider,
            current_lease,
            tenant,
            preparation_error,
            facts,
        ) = row.map_err(database)?;
        let mut summary = raw
            .map(|v| serde_json::from_str::<Summary>(&v))
            .transpose()
            .map_err(database)?
            .unwrap_or_else(Summary::preparing);
        let own = viewer.is_some_and(|v| provider.as_deref() == Some(v.user_id.as_str()));
        let has_report = summary.last_received_at.is_some();
        let deployed = facts.deployed_commit.is_some()
            || facts.bootstrap.as_deref() == Some("succeeded")
            || facts.preparation.as_deref() == Some("ready");
        let phase = if facts.revoked {
            "paused"
        } else if has_report {
            "active"
        } else if facts.bootstrap.as_deref() == Some("running") {
            "installing"
        } else if facts.bootstrap.as_deref() == Some("pending") {
            "queued"
        } else if facts.bootstrap.as_deref() == Some("failed") {
            "failed"
        } else if facts.registered && facts.first.is_some() && deployed {
            "awaiting_first"
        } else if status == "allocated" {
            "not_enabled"
        } else if facts.preparation.as_deref() == Some("running") {
            "installing"
        } else if facts.preparation.as_deref() == Some("failed") {
            "failed"
        } else if matches!(status.as_str(), "onboarding" | "idle" | "maintaining") {
            "queued"
        } else {
            "not_enabled"
        };
        if !has_report {
            summary.state = match phase {
                "awaiting_first" => {
                    if facts
                        .first
                        .is_some_and(|t| Utc::now().timestamp() > t + wire::GRACE_SECONDS)
                    {
                        "first_overdue"
                    } else {
                        "awaiting_first"
                    }
                }
                "queued" | "installing" => "deploying",
                "failed" => "deployment_failed",
                _ => "not_enabled",
            }
            .into();
            if !matches!(phase, "awaiting_first") {
                summary.next_due_at = None;
            }
        }
        if facts.revoked {
            summary.state = "revoked".into()
        }
        summary.machine_epoch = facts.epoch;
        summary.deployment = DeploymentView {
            state: phase.into(),
            reason: if phase == "not_enabled" && status == "allocated" {
                Some("rental_not_enrolled".into())
            } else {
                None
            },
            can_bootstrap: own
                && status == "allocated"
                && facts.legacy
                && facts.admitted.is_none()
                && !facts.revoked
                && facts.eligible_identity
                && facts.no_running_job
                && current_lease.is_some()
                && current_lease == facts.installation
                && (!facts.registered || facts.bootstrap.is_some())
                && !matches!(facts.bootstrap.as_deref(), Some("pending" | "running")),
            deployed_commit: facts.deployed_commit,
        };
        let current_tenant = viewer.is_some_and(|v| tenant.as_deref() == Some(v.user_id.as_str()))
            && lease.is_some()
            && lease == current_lease;
        if !(public == 1 && status == "idle" || current_tenant || operations) {
            if let Some(snapshot) = &mut summary.snapshot {
                snapshot.hide_dynamic("private")
            }
        }
        if own || operations {
            if !has_report && facts.bootstrap.as_deref() == Some("failed") {
                summary.diagnostic = facts.bootstrap_error
            }
            if summary.diagnostic.is_none() {
                summary.diagnostic = preparation_error
            }
        } else {
            summary.diagnostic = None;
        }
        out.insert(host, summary);
    }
    Ok(out)
}

#[derive(Debug, Deserialize)]
struct CalendarQuery {
    #[serde(default)]
    days: Option<u16>,
}

fn calendar_days(
    conn: &Connection,
    host: &str,
    now: i64,
    days: i64,
) -> Result<Vec<serde_json::Value>, AppError> {
    let (epoch, admitted, first): (i64, Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT machine_epoch,admitted_hour,first_due_at FROM client_host_agents WHERE host_id=?1",
            params![host],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(database)?
        .unwrap_or((1, None, None));
    let days = days.clamp(1, 7);
    let today_hour = now.div_euclid(86_400) * 24;
    let start_hour = today_hour - (days - 1) * 24;
    let mut received = HashSet::new();
    if admitted.is_some() {
        let mut statement = conn
            .prepare(
                "SELECT utc_hour FROM client_host_hour_reports WHERE host_id=?1 AND machine_epoch=?2 AND utc_hour>=?3",
            )
            .map_err(database)?;
        for hour in statement
            .query_map(params![host, epoch, start_hour], |row| row.get::<_, i64>(0))
            .map_err(database)?
        {
            received.insert(hour.map_err(database)?);
        }
    }
    let mut days_out = Vec::new();
    for day_start in (start_hour..=today_hour).step_by(24) {
        let mut got = 0u32;
        let mut observed = 0u32;
        for hour in day_start..day_start + 24 {
            let due = first
                .filter(|first| first / 3600 == hour)
                .unwrap_or_else(|| wire::due_at(host, hour));
            if admitted.is_none_or(|admitted| hour < admitted) || now < due + wire::GRACE_SECONDS {
                continue;
            }
            if unavailable(conn, due)? {
                continue;
            }
            observed += 1;
            if received.contains(&hour) {
                got += 1;
            }
        }
        let date = DateTime::<Utc>::from_timestamp(day_start * 3600, 0)
            .unwrap_or_default()
            .date_naive();
        days_out.push(json!({
            "date": date.to_string(),
            "receivedHours": got,
            "observableHours": observed,
            "receivedRate": (observed > 0).then(|| (f64::from(got) * 1000.0 / f64::from(observed)).round() / 10.0),
        }));
    }
    Ok(days_out)
}

/// Hourly reports are deleted after seven days, so the calendar cannot exceed that window.
async fn calendar(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<CalendarQuery>,
) -> Result<Response, AppError> {
    let viewer = extract_optional_session(&state, &headers).await?;
    let operations = if let Some(viewer) = &viewer {
        state.dynamic.read().await.is_admin(&viewer.email)
    } else {
        false
    };
    let host = state
        .store
        .client_market_get_host(&id)
        .await?
        .ok_or_else(|| AppError::NotFound("Host not found".into()))?;
    if matches!(host.status.as_str(), "onboarding" | "maintaining")
        && !operations
        && !viewer
            .as_ref()
            .is_some_and(|viewer| session_is_host_owner(viewer, host.provider_id.as_deref()))
    {
        return Err(AppError::NotFound("Host not available".into()));
    }
    let conn = state.store.conn.lock().await;
    let operator = viewer
        .as_ref()
        .is_some_and(|viewer| session_is_host_owner(viewer, host.provider_id.as_deref()));
    if !operator && !operations && unavailable_paid_host_ids(&conn)?.contains(&id) {
        return Err(AppError::NotFound("Host not available".into()));
    }
    let days = i64::from(query.days.unwrap_or(7).clamp(1, 7));
    let now = Utc::now().timestamp();
    let days_out = calendar_days(&conn, &id, now, days)?;
    let start_hour = now.div_euclid(86_400) * 24 - (days - 1) * 24;
    let start_date = DateTime::<Utc>::from_timestamp(start_hour * 3600, 0)
        .unwrap_or_default()
        .date_naive();
    let end_date = DateTime::<Utc>::from_timestamp(now.div_euclid(86_400) * 86_400, 0)
        .unwrap_or_default()
        .date_naive();
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({
            "hostId": id,
            "timezone": "UTC",
            "startDate": start_date.to_string(),
            "endDate": end_date.to_string(),
            "days": days_out,
        })),
    )
        .into_response())
}

async fn details(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, AppError> {
    let viewer = extract_optional_session(&state, &headers).await?;
    let operations = if let Some(viewer) = &viewer {
        state.dynamic.read().await.is_admin(&viewer.email)
    } else {
        false
    };
    let host = state
        .store
        .client_market_get_host(&id)
        .await?
        .ok_or_else(|| AppError::NotFound("Host not found".into()))?;
    if matches!(host.status.as_str(), "onboarding" | "maintaining")
        && !operations
        && !viewer
            .as_ref()
            .is_some_and(|v| session_is_host_owner(v, host.provider_id.as_deref()))
    {
        return Err(AppError::NotFound("Host not available".into()));
    }
    let mut projections = views(&state, viewer.as_ref(), Some(&id)).await?;
    let summary = projections.remove(&id);
    let conn = state.store.conn.lock().await;
    let operator = viewer
        .as_ref()
        .is_some_and(|v| session_is_host_owner(v, host.provider_id.as_deref()));
    if !operator && !operations && unavailable_paid_host_ids(&conn)?.contains(&id) {
        return Err(AppError::NotFound("Host not available".into()));
    }
    let current_lease: Option<String> = if let Some(viewer) = &viewer {
        conn.query_row("SELECT installation_id FROM client_market_subscriptions WHERE host_id=?1 AND client_user_id=?2 AND status IN ('active','billing_suspended')",params![id,viewer.user_id],|row|row.get(0)).optional().map_err(database)?
    } else {
        None
    };
    let (epoch,admitted,first):(i64,Option<i64>,Option<i64>)=conn.query_row("SELECT machine_epoch,admitted_hour,first_due_at FROM client_host_agents WHERE host_id=?1",params![id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(database)?.unwrap_or((1,None,None));
    let now = Utc::now().timestamp();
    let mut hours = Vec::new();
    if let Some(admitted) = admitted {
        let mut q=conn.prepare("SELECT utc_hour,received_at,report_json,public_sample,lease_id FROM client_host_hour_reports WHERE host_id=?1 AND machine_epoch=?2 AND utc_hour>=?3").map_err(database)?;
        let got = q
            .query_map(params![id, epoch, now / 3600 - 168], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    (
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, bool>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ),
                ))
            })
            .map_err(database)?
            .collect::<Result<HashMap<_, _>, _>>()
            .map_err(database)?;
        for hour in (admitted.max(now / 3600 - 167)..=now / 3600).rev() {
            let due = if first.is_some_and(|first| first / 3600 == hour) {
                first.unwrap()
            } else {
                wire::due_at(&id, hour)
            };
            let state = if got.contains_key(&hour) {
                "received"
            } else if now <= due + wire::GRACE_SECONDS {
                "pending"
            } else if unavailable(&conn, due)? {
                "unknown"
            } else {
                "missed"
            };
            let sample = got.get(&hour).and_then(|(_, raw, public, lease)| {
                serde_json::from_str::<Report>(raw).ok().map(|mut report| {
                    if !(*public && host.status == "idle"
                        || lease.is_some() && *lease == current_lease
                        || operations)
                    {
                        report.snapshot.hide_dynamic("private")
                    }
                    report.snapshot
                })
            });
            hours.push(json!({"utcHour":hour,"dueAt":iso(due),"state":state,"receivedAt":got.get(&hour).map(|r|iso(r.0)),"snapshot":sample}));
        }
    }
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"hostHealth":summary,"hours":hours,"machineEpoch":epoch})),
    )
        .into_response())
}

async fn retry(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    let host = state
        .store
        .client_market_get_host(&id)
        .await?
        .ok_or_else(|| AppError::NotFound("Host not found".into()))?;
    if !session_is_host_owner(&session, host.provider_id.as_deref()) {
        return Err(AppError::Forbidden("Host owner required".into()));
    }
    if !matches!(host.status.as_str(), "onboarding" | "maintaining" | "idle") {
        return Err(AppError::Conflict(
            "Host must be onboarding, maintaining or idle".into(),
        ));
    }
    binary::queue(&state, &id).await;
    Ok(Json(json!({"queued":true})))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RotateKeyRequest {
    #[serde(default)]
    compromised: bool,
}

async fn rotate(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    input: Option<Json<RotateKeyRequest>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    let host = state
        .store
        .client_market_get_host(&id)
        .await?
        .ok_or_else(|| AppError::NotFound("Host not found".into()))?;
    if !session_is_host_owner(&session, host.provider_id.as_deref()) {
        return Err(AppError::Forbidden("Host owner required".into()));
    }
    let compromised = input.is_some_and(|input| input.0.compromised);
    let conn = state.store.conn.lock().await;
    let tx = conn.transaction().map_err(database)?;
    if compromised {
        tx.execute("INSERT OR IGNORE INTO client_host_revoked_keys SELECT host_id,machine_epoch,key_generation,public_key,?2 FROM client_host_agents WHERE host_id=?1 AND public_key IS NOT NULL",params![id,Utc::now().timestamp()]).map_err(database)?;
        tx.execute(
            "UPDATE client_host_agents SET revoked=1,readmission_pending=1 WHERE host_id=?1",
            params![id],
        )
        .map_err(database)?;
        tx.execute(
            "UPDATE router_ssh_hosts SET status='maintaining' WHERE id=?1 AND status='idle'",
            params![id],
        )
        .map_err(database)?;
    }
    tx.execute("UPDATE client_host_agents SET maintenance_pending=1,last_error='rotation_requested',operation_sequence=operation_sequence+1 WHERE host_id=?1",params![id]).map_err(database)?;
    tx.commit().map_err(database)?;
    drop(conn);
    binary::queue(&state, &id).await;
    Ok(Json(json!({"queued":true})))
}

pub(super) fn reusable_status(conn: &Connection, host: &str) -> Result<&'static str, AppError> {
    let admitted:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM client_host_agents WHERE host_id=?1 AND (admitted_hour IS NOT NULL OR legacy=1) AND revoked=0 AND readmission_pending=0)",params![host],|row|row.get(0)).map_err(database)?;
    Ok(if admitted { "idle" } else { "maintaining" })
}

pub(super) async fn settle(state: &ServerState) -> Result<(), AppError> {
    let conn = state.store.conn.lock().await;
    let now = Utc::now().timestamp();
    let tx = conn.transaction().map_err(database)?;
    let ids = {
        let mut q=tx.prepare("SELECT host_id FROM client_host_agents WHERE first_due_at IS NOT NULL AND next_deadline<=?1 ORDER BY next_deadline LIMIT 128").map_err(database)?;
        q.query_map(params![now], |row| row.get::<_, String>(0))
            .map_err(database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database)?
    };
    for id in ids {
        refresh_summary(&tx, &id, now)?
    }
    tx.execute("DELETE FROM client_host_hour_reports WHERE rowid IN (SELECT rowid FROM client_host_hour_reports WHERE received_at<?1 LIMIT 512)",params![now-7*86400]).map_err(database)?;
    tx.execute(
        "DELETE FROM client_host_report_conflicts WHERE last_seen_at<?1",
        params![now - 7 * 86400],
    )
    .map_err(database)?;
    tx.execute(
        "DELETE FROM client_host_receiver_outages WHERE ended_at<?1",
        params![now - 8 * 86400],
    )
    .map_err(database)?;
    tx.commit().map_err(database)
}

pub(crate) async fn observe(
    state: ServerState,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<(), AppError> {
    let instance = Uuid::new_v4().to_string();
    {
        let conn = state.store.conn.lock().await;
        let now = Utc::now().timestamp();
        let old: Option<i64> = conn
            .query_row(
                "SELECT COALESCE(stopped_at,last_seen_at) FROM client_host_receiver WHERE id=1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(database)?;
        if let Some(old) = old {
            conn.execute(
                "INSERT OR IGNORE INTO client_host_receiver_outages VALUES(?1,?2)",
                params![old, now],
            )
            .map_err(database)?;
        }
        conn.execute("INSERT INTO client_host_receiver(id,instance_id,last_seen_at) VALUES(1,?1,?2)
            ON CONFLICT(id) DO UPDATE SET instance_id=excluded.instance_id,last_seen_at=excluded.last_seen_at,stopped_at=NULL",params![instance,now]).map_err(database)?;
    }
    loop {
        if *shutdown.borrow() {
            break;
        }
        state
            .store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE client_host_receiver SET last_seen_at=?2 WHERE id=1 AND instance_id=?1",
                params![instance, Utc::now().timestamp()],
            )
            .map_err(database)?;
        // A settlement failure must not stop the liveness tick: restarting the observer
        // would record a fake receiver outage and mark due Hosts as platform-unknown.
        if let Err(error) = settle(&state).await {
            warn!(%error, "Host health settlement failed; retrying on the next tick");
        }
        tokio::select! {_=tokio::time::sleep(Duration::from_secs(10))=>{},_=shutdown.changed()=>{break}}
    }
    state
        .store
        .conn
        .lock()
        .await
        .execute(
            "UPDATE client_host_receiver SET stopped_at=?2 WHERE id=1 AND instance_id=?1",
            params![instance, Utc::now().timestamp()],
        )
        .map_err(database)?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplaceMachineRequest {
    expected_machine_epoch: i64,
}

async fn replace_machine(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(input): Json<ReplaceMachineRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    let host = state
        .store
        .client_market_get_host_for_operator(&id, &session)
        .await?;
    if !session_is_host_owner(&session, host.provider_id.as_deref()) {
        return Err(AppError::Forbidden("Host owner required".into()));
    }
    {
        let conn = state.store.conn.lock().await;
        let tx = conn.transaction().map_err(database)?;
        // A repeated replacement during onboarding keeps the state saved by the first one.
        tx.execute("UPDATE client_host_agents SET resume_status=COALESCE((SELECT CASE WHEN status IN ('disabled','abnormal') THEN status END FROM router_ssh_hosts WHERE id=?1),resume_status) WHERE host_id=?1 AND machine_epoch=?2",
            params![id,input.expected_machine_epoch]).map_err(database)?;
        let changed=tx.execute("UPDATE router_ssh_hosts SET status='maintaining' WHERE id=?1 AND status IN ('idle','onboarding','disabled','abnormal') AND installation_id IS NULL AND EXISTS(SELECT 1 FROM client_host_agents WHERE host_id=?1 AND machine_epoch=?2)",
            params![id,input.expected_machine_epoch]).map_err(database)?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "Host generation or availability changed".into(),
            ));
        }
        tx.execute("INSERT OR IGNORE INTO client_host_revoked_keys SELECT host_id,machine_epoch,key_generation,public_key,?2 FROM client_host_agents WHERE host_id=?1 AND public_key IS NOT NULL",params![id,Utc::now().timestamp()]).map_err(database)?;
        tx.execute("UPDATE client_host_agents SET replacement_pending=1,maintenance_pending=1,revoked=1,last_error='replacement_pending',operation_sequence=operation_sequence+1 WHERE host_id=?1",params![id]).map_err(database)?;
        tx.commit().map_err(database)?;
    }
    binary::queue(&state, &id).await;
    Ok(Json(json!({"queued":true})))
}

// The tests exercise the same signature and transaction path as the HTTP handler.

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn first_rented_host_report_preserves_the_rental_and_keeps_usage_private() {
        let store = AppStore::new_in_memory_for_tests().unwrap();
        host(&store, "bootstrap-host", 360100).await;
        {
            let conn = store.conn.lock().await;
            let owner = crate::market_finance_auth::test_session(&conn, "first-rental-owner");
            let buyer = crate::market_finance_auth::test_session(&conn, "first-rental-buyer");
            conn.execute(
                "UPDATE client_host_agents SET legacy=1 WHERE host_id='bootstrap-host'",
                [],
            )
            .unwrap();
            conn.execute("UPDATE router_ssh_hosts SET status='allocated',installation_id='live-rental',provider_id=?1 WHERE id='bootstrap-host'",params![owner.user_id]).unwrap();
            conn.execute("INSERT INTO client_market_subscriptions(installation_id,host_id,provider_id,host_owner_email,client_user_id,client_owner_email,status,offer_revision,created_at,updated_at)
                VALUES('live-rental','bootstrap-host',?1,?2,?3,?4,'active',7,?5,?5)",params![owner.user_id,owner.email,buyer.user_id,buyer.email,iso(350000)]).unwrap();
        }
        assert_eq!(
            send(&store, &report("bootstrap-host", 100, 360100), 360100).await,
            StatusCode::OK
        );
        let conn = store.conn.lock().await;
        let facts:(String,String,String,i64,bool)=conn.query_row("SELECT h.status,h.installation_id,s.status,s.offer_revision,a.legacy FROM router_ssh_hosts h JOIN client_market_subscriptions s ON s.host_id=h.id JOIN client_host_agents a ON a.host_id=h.id WHERE h.id='bootstrap-host'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap();
        assert_eq!(
            facts,
            (
                "allocated".into(),
                "live-rental".into(),
                "active".into(),
                7,
                false
            )
        );
        let public = public_summary(&conn, "bootstrap-host").unwrap().unwrap();
        assert!(
            public
                .snapshot
                .as_ref()
                .unwrap()
                .cpu_usage_pct
                .value
                .is_none()
        );
        assert_eq!(public.deployment.state, "active");
        assert!(!public.deployment.can_bootstrap);
    }

    #[tokio::test]
    async fn quote_reloads_the_frozen_health_and_enforces_buyer_identity() {
        let store = AppStore::new_in_memory_for_tests().unwrap();
        let id = "host-fixture";
        host(&store, id, 360100).await;
        assert_eq!(
            send(&store, &report(id, 100, 360100), 360100).await,
            StatusCode::OK
        );
        let (provider, buyer, other) = {
            let conn = store.conn.lock().await;
            let provider = crate::market_finance_auth::test_session(&conn, "health-quote-provider");
            let buyer = crate::market_finance_auth::test_session(&conn, "health-quote-buyer");
            let other = crate::market_finance_auth::test_session(&conn, "health-other-buyer");
            crate::market_access::configure_open_test_policy(
                &conn,
                &provider,
                "USD",
                50000,
                &Utc::now().to_rfc3339(),
            );
            (provider, buyer, other)
        };
        store
            .client_market_ensure_provider(&provider.user_id, &provider.email)
            .await
            .unwrap();
        store.conn.lock().await.execute("UPDATE router_ssh_hosts SET provider_id=?2,host_owner_email=?3,country_code='US' WHERE id=?1",params![id,provider.user_id,provider.email]).unwrap();
        let quote = store
            .client_market_create_quote(
                &buyer,
                crate::client_market_trade::CreateQuoteRequest {
                    provider_ids: vec![],
                    country_codes: vec![],
                    count: 1,
                    host_id: Some(id.into()),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            quote.items[0]
                .host_health
                .as_ref()
                .unwrap()
                .last_received_at,
            Some(iso(360100))
        );
        let due = wire::due_at(id, 101);
        assert_eq!(
            send(&store, &report(id, 101, due), due).await,
            StatusCode::OK
        );
        let frozen = store
            .client_market_quote_projection(&quote.id, &buyer)
            .await
            .unwrap();
        assert_eq!(
            frozen["items"][0]["hostHealth"]["lastReceivedAt"],
            iso(360100)
        );
        assert!(
            store
                .client_market_quote_projection(&quote.id, &other)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn projections_enforce_current_rental_and_hide_supplier_workload() {
        let store = AppStore::new_in_memory_for_tests().unwrap();
        let id = "host-fixture";
        host(&store, id, 360100).await;
        assert_eq!(
            send(&store, &report(id, 100, 360100), 360100).await,
            StatusCode::OK
        );
        let (owner, a, b) = {
            let conn = store.conn.lock().await;
            let owner = crate::market_finance_auth::test_session(&conn, "health-owner");
            let a = crate::market_finance_auth::test_session(&conn, "health-tenant-a");
            let b = crate::market_finance_auth::test_session(&conn, "health-tenant-b");
            conn.execute(
                "UPDATE router_ssh_hosts SET provider_id=?2,status='allocated' WHERE id=?1",
                params![id, owner.user_id],
            )
            .unwrap();
            conn.execute("INSERT INTO client_market_subscriptions(installation_id,host_id,provider_id,host_owner_email,client_user_id,client_owner_email,status,offer_revision,created_at,updated_at)
                VALUES('lease-a',?1,?2,?3,?4,?5,'active',1,?6,?6)",params![id,owner.user_id,owner.email,a.user_id,a.email,iso(360000)]).unwrap();
            (owner, a, b)
        };
        let due = wire::due_at(id, 101);
        assert_eq!(
            send(&store, &report(id, 101, due), due).await,
            StatusCode::OK
        );
        let conn = store.conn.lock().await;
        for viewer in [None, Some(&owner), Some(&b)] {
            assert!(
                project_views(&conn, viewer, Some(id), false).unwrap()[id]
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .cpu_usage_pct
                    .value
                    .is_none()
            );
        }
        assert_eq!(
            project_views(&conn, Some(&a), Some(id), false).unwrap()[id]
                .snapshot
                .as_ref()
                .unwrap()
                .cpu_usage_pct
                .value,
            Some(25.0)
        );
        conn.execute("UPDATE client_market_subscriptions SET status='released',released_at=?1 WHERE installation_id='lease-a'",params![iso(due+1)]).unwrap();
        conn.execute("INSERT INTO client_market_subscriptions(installation_id,host_id,provider_id,host_owner_email,client_user_id,client_owner_email,status,offer_revision,created_at,updated_at)
                VALUES('lease-b',?1,?2,?3,?4,?5,'active',1,?6,?6)",params![id,owner.user_id,owner.email,b.user_id,b.email,iso(due+2)]).unwrap();
        for viewer in [&a, &b] {
            assert!(
                project_views(&conn, Some(viewer), Some(id), false).unwrap()[id]
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .cpu_usage_pct
                    .value
                    .is_none()
            );
        }
        conn.execute(
            "UPDATE client_host_agents SET revoked=1 WHERE host_id=?1",
            params![id],
        )
        .unwrap();
        assert_eq!(
            project_views(&conn, None, Some(id), false).unwrap()[id].state,
            "revoked"
        );
    }

    #[tokio::test]
    async fn health_calendar_counts_received_hours_and_skips_grace() {
        let store = AppStore::new_in_memory_for_tests().unwrap();
        let hour = 480_012i64;
        let first = hour * 3600;
        host(&store, "host-fixture", first).await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE client_host_agents SET admitted_hour=?2 WHERE host_id='host-fixture'",
                params![hour],
            )
            .unwrap();
        }
        let report = report("host-fixture", hour, first);
        assert_eq!(send(&store, &report, first + 2).await, StatusCode::OK);
        let conn = store.conn.lock().await;
        let days = calendar_days(
            &conn,
            "host-fixture",
            first + 3 * 3600 + wire::GRACE_SECONDS,
            1,
        )
        .unwrap();
        let today = days.last().unwrap();
        assert_eq!(today["receivedHours"], 1);
        assert_eq!(today["observableHours"], 3);
        assert_eq!(today["receivedRate"], 33.3);
        let during_grace = calendar_days(&conn, "host-fixture", first + 60, 1).unwrap();
        assert_eq!(during_grace.last().unwrap()["observableHours"], 0);
    }

    #[tokio::test]
    async fn late_hour_grace_does_not_settle_at_the_hour_boundary() {
        let store = AppStore::new_in_memory_for_tests().unwrap();
        let first = 100 * 3600 + 3599;
        host(&store, "host-fixture", first).await;
        let report = report("host-fixture", 100, first);
        assert_eq!(send(&store, &report, first + 2).await, StatusCode::OK);
        let conn = store.conn.lock().await;
        refresh_summary(&conn, "host-fixture", 101 * 3600 + 1).unwrap();
        let summary = public_summary(&conn, "host-fixture").unwrap().unwrap();
        assert_eq!(summary.observable24h, 0);
        assert_eq!(summary.state, "healthy");
        refresh_summary(&conn, "host-fixture", first + wire::GRACE_SECONDS).unwrap();
        assert_eq!(
            public_summary(&conn, "host-fixture")
                .unwrap()
                .unwrap()
                .observable24h,
            1
        );
    }

    #[tokio::test]
    #[ignore = "explicit 10000 Host database workload"]
    async fn ten_thousand_host_summary_workload() {
        let store = AppStore::new_in_memory_for_tests().unwrap();
        let conn = store.conn.lock().await;
        conn.execute_batch(
            "BEGIN;
             WITH RECURSIVE n(v) AS (VALUES(1) UNION ALL SELECT v+1 FROM n WHERE v<10000)
             INSERT INTO router_ssh_hosts(id,ip,port,host_owner_email,status,created_at,updated_at)
             SELECT 'load-'||v,'load-'||v,22,'load@example.test','onboarding','2026-01-01','2026-01-01' FROM n;
             UPDATE client_host_agents SET first_due_at=49000*3600,admitted_hour=49000,latest_hour=49999,latest_received_at=49999*3600;
             WITH RECURSIVE h(v) AS (VALUES(49970) UNION ALL SELECT v+1 FROM h WHERE v<49999)
             INSERT INTO client_host_hour_reports(host_id,utc_hour,machine_epoch,key_generation,body_digest,received_at,sampled_at,report_json)
             SELECT a.host_id,h.v,1,1,'load',h.v*3600,h.v*3600,'{}' FROM client_host_agents a CROSS JOIN h WHERE h.v%24 NOT IN (0,1);
             COMMIT;"
        ).unwrap();
        let start = Instant::now();
        let mut times = Vec::new();
        for n in 1..=10000 {
            let tick = Instant::now();
            refresh_summary(&conn, &format!("load-{n}"), 50000 * 3600 - 1).unwrap();
            times.push(tick.elapsed().as_micros());
        }
        times.sort_unstable();
        let summary = public_summary(&conn, "load-5000").unwrap().unwrap();
        assert_eq!((summary.received24h, summary.observable24h), (22, 24));
        let bytes: i64 = conn
            .query_row(
                "SELECT page_count*page_size FROM pragma_page_count(),pragma_page_size()",
                [],
                |row| row.get(0),
            )
            .unwrap();
        eprintln!(
            "10000 Host summary recomputation {:?}, per-Host p95={}us p99={}us, synthetic database={} bytes",
            start.elapsed(),
            times[9499],
            times[9899],
            bytes
        );
    }

    use ed25519_dalek::{Signer, SigningKey};

    async fn host(store: &AppStore, id: &str, first: i64) {
        let conn = store.conn.lock().await;
        conn.execute(
            "INSERT INTO router_ssh_hosts(id,ip,port,host_owner_email,status,created_at,updated_at)
            VALUES(?1,?1,22,'host@example.test','onboarding',?2,?2)",
            params![id, iso(first)],
        )
        .unwrap();
        let key = SigningKey::from_bytes(&[7; 32]);
        conn.execute(
            "UPDATE client_host_agents SET first_due_at=?2,public_key=?3 WHERE host_id=?1",
            params![
                id,
                first,
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(key.verifying_key().as_bytes())
            ],
        )
        .unwrap();
    }

    fn report(id: &str, h: i64, due: i64) -> Report {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/host-health-v1.json")).unwrap();
        let mut report: Report = serde_json::from_str(fixture["body"].as_str().unwrap()).unwrap();
        report.host_id = id.to_string();
        report.utc_hour = h;
        report.sampled_at = iso(due);
        report
    }

    async fn send(store: &AppStore, report: &Report, now: i64) -> StatusCode {
        let body = serde_json::to_vec(report).unwrap();
        let signature = SigningKey::from_bytes(&[7; 32]).sign(&wire::signing_message(&body));
        let mut headers = HeaderMap::new();
        headers.insert("X-CC-Report-Version", "1".parse().unwrap());
        headers.insert("X-CC-Host-Id", report.host_id.parse().unwrap());
        headers.insert(
            "X-CC-Machine-Epoch",
            report.machine_epoch.to_string().parse().unwrap(),
        );
        headers.insert(
            "X-CC-Key-Generation",
            report.key_generation.to_string().parse().unwrap(),
        );
        headers.insert(
            "X-CC-Report-Signature",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(signature.to_bytes())
                .parse()
                .unwrap(),
        );
        ingest(store, headers, Bytes::from(body), now)
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn admission_is_atomic_and_replay_does_not_refresh() {
        let store = AppStore::new_in_memory_for_tests().unwrap();
        let first = 3600 * 100 + 3599;
        host(&store, "host-fixture", first).await;
        assert!(
            store
                .conn
                .lock()
                .await
                .execute(
                    "UPDATE router_ssh_hosts SET status='idle' WHERE id='host-fixture'",
                    []
                )
                .is_err()
        );
        let mut r = report("host-fixture", 100, first);
        assert_eq!(send(&store, &r, first + 2).await, StatusCode::OK);
        assert_eq!(
            store
                .client_market_get_host("host-fixture")
                .await
                .unwrap()
                .unwrap()
                .status,
            "idle"
        );
        assert_eq!(send(&store, &r, first + 10_000).await, StatusCode::OK);
        let received: i64 = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT received_at FROM client_host_hour_reports",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(received, first + 2);
        r.snapshot.effective_vcpu.value = Some(2.0);
        assert_eq!(send(&store, &r, first + 3).await, StatusCode::CONFLICT);
        let conn = store.conn.lock().await;
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM client_host_hour_reports", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM client_host_report_conflicts",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn invalid_window_unknown_values_revocation_and_rotation() {
        let store = AppStore::new_in_memory_for_tests().unwrap();
        let first = 360_100;
        host(&store, "host-fixture", first).await;
        let mut r = report("host-fixture", 100, first);
        assert_eq!(
            send(&store, &r, first + 301).await,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        r.snapshot.effective_vcpu = wire::Reading::unknown("scope_unverified");
        assert_eq!(send(&store, &r, first).await, StatusCode::OK);
        {
            let conn = store.conn.lock().await;
            conn.execute("INSERT INTO client_host_revoked_keys SELECT host_id,machine_epoch,key_generation,public_key,?1 FROM client_host_agents",params![first]).unwrap();
            conn.execute("UPDATE client_host_agents SET key_generation=2", [])
                .unwrap();
        }
        assert_eq!(send(&store, &r, first + 1).await, StatusCode::GONE);
        r.key_generation = 2;
        assert_eq!(send(&store, &r, first + 1).await, StatusCode::CONFLICT);
        r.utc_hour = 101;
        r.sampled_at = iso(wire::due_at("host-fixture", 101));
        assert_eq!(
            send(&store, &r, wire::due_at("host-fixture", 101)).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn counts_include_rented_hours_and_unknown_resets_consecutive() {
        let store = AppStore::new_in_memory_for_tests().unwrap();
        let id = "host-fixture";
        let first = wire::due_at(id, 100);
        host(&store, id, first).await;
        for h in 100..124 {
            if h == 121 || h == 123 {
                continue;
            }
            let due = wire::due_at(id, h);
            assert_eq!(send(&store, &report(id, h, due), due).await, StatusCode::OK);
        }
        let conn = store.conn.lock().await;
        let now = wire::due_at(id, 123) + wire::GRACE_SECONDS;
        refresh_summary(&conn, id, now).unwrap();
        let summary = public_summary(&conn, id).unwrap().unwrap();
        assert_eq!((summary.received24h, summary.observable24h), (22, 24));
        assert_eq!(summary.consecutive_missed, 1);
        conn.execute(
            "INSERT INTO client_host_receiver_outages VALUES(?1,?2)",
            params![wire::due_at(id, 122) - 1, wire::due_at(id, 122) + 1],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM client_host_hour_reports WHERE utc_hour=122",
            [],
        )
        .unwrap();
        refresh_summary(&conn, id, now).unwrap();
        let summary = public_summary(&conn, id).unwrap().unwrap();
        assert_eq!((summary.received24h, summary.observable24h), (21, 23));
        assert_eq!(summary.consecutive_missed, 1);
    }

    #[tokio::test]
    async fn delayed_rental_sample_stays_private_after_cleanup() {
        let store = AppStore::new_in_memory_for_tests().unwrap();
        host(&store, "host-fixture", 360_100).await;
        let conn = store.conn.lock().await;
        conn.execute(
            "INSERT INTO client_host_tenancies VALUES('rental-a','host-fixture',360000,360110)",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE client_host_agents SET admitted_hour=100 WHERE host_id='host-fixture'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE router_ssh_hosts SET status='idle' WHERE id='host-fixture'",
            [],
        )
        .unwrap();
        assert_eq!(
            sample_visibility(&conn, "host-fixture", 360000, 360112).unwrap(),
            (false, None)
        );
        assert_eq!(
            sample_visibility(&conn, "host-fixture", 360001, 360109).unwrap(),
            (false, Some("rental-a".into()))
        );
        assert_eq!(
            sample_visibility(&conn, "host-fixture", 360111, 360120).unwrap(),
            (true, None)
        );
    }

    #[tokio::test]
    async fn maintenance_and_reservation_have_one_winner() {
        let store = AppStore::new_in_memory_for_tests().unwrap();
        host(&store, "host-fixture", 360_100).await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE client_host_agents SET admitted_hour=100,maintenance_pending=1",
                [],
            )
            .unwrap();
            conn.execute("UPDATE router_ssh_hosts SET status='idle'", [])
                .unwrap();
            conn.execute("UPDATE client_binary_preparations SET status='ready'", [])
                .unwrap();
        }
        assert!(
            claim_preparation(&store, "host-fixture")
                .await
                .unwrap()
                .is_some()
        );
        let conn = store.conn.lock().await;
        assert_eq!(conn.execute("UPDATE router_ssh_hosts SET status='reserved' WHERE id='host-fixture' AND status='idle'",[]).unwrap(),0);
        conn.execute("UPDATE router_ssh_hosts SET status='reserved'", [])
            .unwrap();
        drop(conn);
        assert!(
            claim_preparation(&store, "host-fixture")
                .await
                .unwrap()
                .is_none()
        );
    }
}
