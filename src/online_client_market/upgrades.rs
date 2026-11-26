use super::*;
use crate::models::{InstallationUpgradeTaskReportPayload, UpgradeInstallationStatusResponse};

fn installation(conn: &Connection, lease: &str, buyer: &AuthSession) -> Result<String, AppError> {
    conn.query_row(
        "SELECT r.installation_id FROM online_client_leases r
         JOIN online_client_listings l ON l.installation_id = r.installation_id
         WHERE r.id = ?1 AND r.buyer_id = ?2 AND r.tenant_email = ?3
           AND r.state = 'active' AND r.grant_until > unixepoch()
           AND r.management_version = 2 AND l.exclusive_management",
        params![lease, buyer.user_id, buyer.email],
        |row| row.get(0),
    )
    .optional()
    .market()?
    .ok_or_else(missing)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Start {
    operation_id: Uuid,
}
pub(super) async fn start(
    State(s): State<ServerState>,
    Path(lease): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Start>,
) -> Reply {
    let buyer = actor(&s, &headers).await?;
    let task = format!("lease-{lease}-{}", input.operation_id.simple());
    let id = {
        let c = s.store.conn.lock().await;
        let id = installation(&c, &lease, &buyer)?;
        let known: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM installation_upgrade_tasks
             WHERE installation_id = ?1 AND lease_id = ?2 AND task_id = ?3)",
                params![id, lease, task],
                |row| row.get(0),
            )
            .market()?;
        if known {
            return Ok(Json(json!({"ok":true,"taskId":task})));
        }
        id
    };
    let commit = crate::api::validate_client_upgrade(&s).await?;
    let reply: Value = control(
        &s,
        &id,
        "/_ctl/market/upgrade",
        json!({
            "leaseId": lease, "tenantEmail": buyer.email, "buyerId": buyer.user_id,
            "taskId": task, "targetCommit": commit, "restartAfter": true,
        }),
        30,
    )
    .await?;
    if reply["taskId"].as_str() != Some(task.as_str()) {
        return Err(conflict("Client returned an unexpected task"));
    }
    let c = s.store.conn.lock().await;
    // A signed task report may have arrived before the start acknowledgement.
    c.execute(
        "INSERT INTO installation_upgrade_tasks
            (installation_id, task_id, lease_id, requested_by_email, status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?5)
         ON CONFLICT(installation_id, task_id) DO UPDATE SET requested_by_email = excluded.requested_by_email",
        params![id, task, lease, buyer.email, Utc::now().to_rfc3339()],
    ).market()?;
    installation(&c, &lease, &buyer)?;
    Ok(Json(reply))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Status {
    task_id: Option<String>,
}
pub(super) async fn status(
    State(s): State<ServerState>,
    Path(lease): Path<String>,
    headers: HeaderMap,
    Query(query): Query<Status>,
) -> Result<Json<UpgradeInstallationStatusResponse>, AppError> {
    let buyer = actor(&s, &headers).await?;
    let id = {
        let c = s.store.conn.lock().await;
        installation(&c, &lease, &buyer)?
    };
    if query
        .task_id
        .as_deref()
        .is_some_and(|id| !valid_task(&lease, id))
    {
        return Err(missing());
    }
    let current = s
        .store
        .installation_upgrade_status_scoped(
            &id,
            query.task_id.as_deref(),
            &buyer.email,
            Some((&lease, &buyer.user_id)),
        )
        .await;
    let known = match current {
        Ok(view) if view.status == "running" && view.status_sync != "reported" => Some(view),
        Ok(view) => return Ok(Json(view)),
        Err(AppError::NotFound(_)) => None,
        Err(err) => return Err(err),
    };
    let expected = known
        .as_ref()
        .map(|v| v.task_id.as_str())
        .or(query.task_id.as_deref());
    let report: Result<InstallationUpgradeTaskReportPayload, _> = control(
        &s,
        &id,
        "/_ctl/market/upgrade-status",
        json!({
            "leaseId": lease, "buyerId": buyer.user_id,
            "tenantEmail": buyer.email, "taskId": expected,
        }),
        10,
    )
    .await;
    let c = s.store.conn.lock().await;
    installation(&c, &lease, &buyer)?;
    let report = match report {
        Ok(report)
            if valid_task(&lease, &report.task_id)
                && expected.is_none_or(|id| id == report.task_id) =>
        {
            report
        }
        _ => return known.map(Json).ok_or_else(missing),
    };
    let task = report.task_id.clone();
    crate::store::validate_installation_upgrade_task_report(&report)?;
    c.execute(
        "INSERT OR IGNORE INTO installation_upgrade_tasks
            (installation_id, task_id, lease_id, requested_by_email, status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?5)",
        params![id, task, lease, buyer.email, report.updated_at],
    )
    .market()?;
    drop(c);
    s.store
        .reconcile_installation_upgrade_status_from_client(&id, &task, report)
        .await?;
    Ok(Json(
        s.store
            .installation_upgrade_status_scoped(
                &id,
                Some(&task),
                &buyer.email,
                Some((&lease, &buyer.user_id)),
            )
            .await?,
    ))
}

fn valid_task(lease: &str, task: &str) -> bool {
    task.strip_prefix(&format!("lease-{lease}-"))
        .is_some_and(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
}
