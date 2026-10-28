//! Read-only work projections. They never approve cleanup, move funds, or infer
//! an old rental's parties from the Host's current installation/owner.
use super::*;
use std::collections::HashMap;

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Permissions {
    buyer: bool,
    provider: bool,
    owner: bool,
    dispute: bool,
    withdraw: bool,
    waive: bool,
    respond: bool,
    resolve: bool,
    retry_cleanup: bool,
}

struct Facts {
    id: String,
    host: String,
    buyer: String,
    provider: String,
    status: String,
    responsibility: String,
    cap: i64,
    compensation: i64,
    held: i64,
    credited: i64,
    cleaned: bool,
    unlock: Option<String>,
    disputed: bool,
    opened: Option<String>,
    responded: bool,
    retry_cleanup: bool,
}

// Only small, indexed facts are loaded for summary/counts. Claims, emails,
// financial event bodies and one detail request per Host are deliberately absent.
const FACTS: &str = "SELECT t.id,t.installation_id,t.host_id,t.buyer_user_id,t.provider_user_id,
    t.status,t.responsibility,t.compensation_cap_units,t.compensation_units,t.held_units,
    COALESCE(c.credited_units,0),t.cleanup_confirmed_at IS NOT NULL,c.unlock_at,
    t.dispute_used,t.dispute_opened_at,t.provider_responded_at IS NOT NULL,t.cutoff_at,
    EXISTS(SELECT 1 FROM router_ssh_hosts h WHERE h.id=t.host_id
        AND h.installation_id=t.installation_id AND h.provider_id=t.provider_user_id
        AND h.status IN ('allocated','unreachable','draining'))
    FROM client_market_terminations t LEFT JOIN client_termination_compensations c ON c.id=t.id";

fn facts_row(r: &crate::db::Row<'_>) -> crate::db::Result<Facts> {
    Ok(Facts {
        id: r.get(0)?,
        host: r.get(2)?,
        buyer: r.get(3)?,
        provider: r.get(4)?,
        status: r.get(5)?,
        responsibility: r.get(6)?,
        cap: r.get(7)?,
        compensation: r.get(8)?,
        held: r.get(9)?,
        credited: r.get(10)?,
        cleaned: r.get(11)?,
        unlock: r.get(12)?,
        disputed: r.get(13)?,
        opened: r.get(14)?,
        responded: r.get(15)?,
        retry_cleanup: r.get(17)?,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Projection {
    permissions: Permissions,
    attention: &'static str,
    action_required: bool,
    response_deadline_at: Option<String>,
}

fn project(f: &Facts, user: &str, owner: bool, at: &str) -> Result<Projection, AppError> {
    let now = time(at)?;
    let deadline = f
        .opened
        .as_deref()
        .map(time)
        .transpose()?
        .map(|v| v + Duration::days(3));
    let unresolved = f.disputed && f.responsibility == "undetermined";
    let due = unresolved && !f.responded && deadline.is_some_and(|v| now >= v);
    let remaining = if !f.cleaned || f.status == "finance_pending" {
        f.cap
    } else if f.compensation > 0 {
        f.compensation
    } else {
        f.held
    };
    let window_open = f
        .unlock
        .as_deref()
        .map(time)
        .transpose()?
        .is_none_or(|v| now < v);
    let decidable = !due
        && f.status != "finance_pending"
        && remaining > 0
        && f.credited == 0
        && (window_open || f.status == "disputed");
    let buyer = f.buyer == user;
    let provider = f.provider == user;
    let permissions = Permissions {
        buyer,
        provider,
        owner,
        dispute: buyer
            && f.status != "finance_pending"
            && !f.disputed
            && f.cap > 0
            && f.credited == 0
            && (!f.cleaned || f.compensation > 0)
            && window_open,
        withdraw: buyer && unresolved && decidable,
        waive: provider && decidable,
        // A formal reply is still possible during a finance hold: it moves no
        // money and must not take away the Provider's contractual response right.
        respond: provider && unresolved && !f.responded && !due && remaining > 0,
        resolve: owner && decidable,
        retry_cleanup: provider && f.status == "cleanup_failed" && f.retry_cleanup,
    };
    let (attention, action_required) = if permissions.respond {
        ("provider_response", true)
    } else if owner && f.status == "finance_pending" {
        ("finance_review", true)
    } else if owner && unresolved && f.responded && permissions.resolve {
        ("owner_decision", true)
    } else if permissions.retry_cleanup {
        ("cleanup_failed", true)
    } else if due {
        ("timeout_pending", false)
    } else if unresolved {
        (
            if f.responded {
                "waiting_owner"
            } else {
                "waiting_provider"
            },
            false,
        )
    } else {
        (
            match f.status.as_str() {
                "cleanup_failed" => "cleanup_failed",
                "finance_pending" => "finance_review",
                "completed" => "completed",
                _ => "processing",
            },
            false,
        )
    };
    Ok(Projection {
        permissions,
        attention,
        action_required,
        response_deadline_at: deadline.map(|v| v.to_rfc3339()),
    })
}

fn fact_tx(conn: &Connection, id: &str) -> Result<Facts, AppError> {
    conn.query_row(&format!("{FACTS} WHERE t.id=?1"), params![id], facts_row)
        .optional()
        .map_err(db_error("read Client termination work state"))?
        .ok_or_else(|| AppError::NotFound("termination not found".into()))
}

fn authorize(f: &Facts, user: &str, owner: bool) -> Result<(), AppError> {
    if owner || f.buyer == user || f.provider == user {
        Ok(())
    } else {
        Err(AppError::Forbidden(
            "not allowed to view this termination".into(),
        ))
    }
}

fn view_tx(
    conn: &Connection,
    f: &Facts,
    user: &str,
    owner: bool,
    at: &str,
) -> Result<serde_json::Value, AppError> {
    authorize(f, user, owner)?;
    let mut value = serde_json::to_value(by_id_tx(conn, &f.id)?)
        .map_err(|_| AppError::Internal("serialize Client termination".into()))?;
    let projection = serde_json::to_value(project(f, user, owner, at)?)
        .map_err(|_| AppError::Internal("serialize Client termination actions".into()))?;
    value
        .as_object_mut()
        .unwrap()
        .extend(projection.as_object().unwrap().clone());
    value["serverNow"] = json!(at);
    Ok(value)
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DetailQuery {
    event_cursor: Option<String>,
}

pub(super) async fn detail(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<DetailQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = session(&state, &headers).await?;
    let owner = crate::api::is_finance_owner(&state.config, &actor.email);
    let conn = state.store.conn.lock().await;
    Ok((
        no_store(),
        Json(detail_tx(
            &conn,
            &id,
            &actor.user_id,
            owner,
            &Utc::now().to_rfc3339(),
            q.event_cursor.as_deref(),
        )?),
    ))
}

fn detail_tx(
    conn: &Connection,
    id: &str,
    user: &str,
    owner: bool,
    at: &str,
    cursor: Option<&str>,
) -> Result<serde_json::Value, AppError> {
    let f = fact_tx(conn, id)?;
    let mut value = view_tx(conn, &f, user, owner, at)?;
    let position: Option<(i64, String)> = cursor.map(|c| conn.query_row(
        "SELECT version,created_at FROM client_termination_events WHERE id=?1 AND termination_id=?2",
        params![c,id], |r| Ok((r.get(0)?,r.get(1)?))).optional()
        .map_err(db_error("read Client termination event cursor"))?
        .ok_or_else(|| AppError::BadRequest("invalid termination event cursor".into()))).transpose()?;
    let mut statement = conn.prepare("SELECT id,action,actor_user_id,detail_json,created_at FROM client_termination_events
        WHERE termination_id=?1 AND (?2 IS NULL OR version<?3 OR (version=?3 AND (created_at<?4 OR (created_at=?4 AND id<?2))))
        ORDER BY version DESC,created_at DESC,id DESC LIMIT 51").map_err(db_error("prepare Client termination events"))?;
    let events = statement.query_map(params![id,cursor,position.as_ref().map(|p|p.0),position.as_ref().map(|p|&p.1)], |r| {
        let detail: String=r.get(3)?;
        let detail: serde_json::Value=serde_json::from_str(&detail).unwrap_or_default();
        let actor: Option<String>=r.get(2)?;
        Ok(json!({"id":r.get::<_,String>(0)?,"action":r.get::<_,String>(1)?,
            "actorRole":match actor.as_deref() { None=>"system",Some(v) if v==f.buyer=>"buyer",Some(v) if v==f.provider=>"provider",_=>"owner" },
            "note":detail.get("note").and_then(|v|v.as_str()),"awardUnits":detail.get("awardUnits").and_then(|v|v.as_str()),
            "createdAt":r.get::<_,String>(4)?}))
    }).map_err(db_error("query Client termination events"))?.collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read Client termination events"))?;
    value["eventsNextCursor"] = if events.len() > 50 {
        events[49]["id"].clone()
    } else {
        json!(null)
    };
    value["events"] = json!(events.into_iter().take(50).collect::<Vec<_>>());
    Ok(value)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HostSummary {
    host_id: String,
    termination_id: String,
    attention: &'static str,
    action_required_count: usize,
    response_deadline_at: Option<String>,
}
fn priority(p: &Projection) -> u8 {
    match (p.action_required, p.attention) {
        (true, "provider_response") => 0,
        (true, "finance_review") => 1,
        (true, "owner_decision") => 2,
        (true, _) => 3,
        (false, "waiting_provider" | "waiting_owner" | "timeout_pending") => 4,
        (false, "completed") => 6,
        _ => 5,
    }
}
fn summary_tx(
    conn: &Connection,
    user: &str,
    owner: bool,
    at: &str,
) -> Result<serde_json::Value, AppError> {
    // A parameterized Owner OR would make every ordinary user's background
    // poll scan everyone else's history instead of using the Provider index.
    // Select only static predicates here; all identities stay bound parameters.
    let scope = if owner {
        "?2=1"
    } else {
        "?2=0 AND t.provider_user_id=?1"
    };
    let latest_scope = if owner {
        "?2=1"
    } else {
        "latest.provider_user_id=?1"
    };
    let mut statement = conn
        .prepare(&format!(
            "{FACTS} WHERE {scope}
        AND (t.status!='completed' OR t.id=(SELECT latest.id FROM client_market_terminations latest
            WHERE latest.host_id=t.host_id AND {latest_scope}
            ORDER BY latest.cutoff_at DESC,latest.id DESC LIMIT 1))
        ORDER BY t.cutoff_at DESC,t.id DESC"
        ))
        .map_err(db_error("prepare Client termination summary"))?;
    let facts = statement
        .query_map(params![user, owner], facts_row)
        .map_err(db_error("query Client termination summary"))?;
    let mut hosts: HashMap<String, (HostSummary, u8)> = HashMap::new();
    let mut count = 0;
    for f in facts {
        let f = f.map_err(db_error("read Client termination summary"))?;
        let p = project(&f, user, owner, at)?;
        count += usize::from(p.action_required);
        let rank = priority(&p);
        let entry = hosts.entry(f.host.clone()).or_insert_with(|| {
            (
                HostSummary {
                    host_id: f.host.clone(),
                    termination_id: f.id.clone(),
                    attention: p.attention,
                    action_required_count: 0,
                    response_deadline_at: p.response_deadline_at.clone(),
                },
                rank,
            )
        });
        entry.0.action_required_count += usize::from(p.action_required);
        if rank < entry.1 || (rank == 0 && p.response_deadline_at < entry.0.response_deadline_at) {
            entry.0.termination_id = f.id;
            entry.0.attention = p.attention;
            entry.0.response_deadline_at = p.response_deadline_at;
            entry.1 = rank;
        }
    }
    let mut hosts = hosts.into_values().map(|v| v.0).collect::<Vec<_>>();
    hosts.sort_by(|a, b| a.host_id.cmp(&b.host_id));
    Ok(json!({"hosts":hosts,"actionRequiredCount":count,"canAdjudicate":owner,"serverNow":at}))
}
pub(super) async fn summary(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((
        no_store(),
        Json(summary_tx(
            &conn,
            &actor.user_id,
            crate::api::is_finance_owner(&state.config, &actor.email),
            &Utc::now().to_rfc3339(),
        )?),
    ))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RecordsQuery {
    host_id: Option<String>,
    cursor: Option<String>,
    #[serde(default)]
    pending_only: bool,
}
fn records_tx(
    conn: &Connection,
    user: &str,
    owner: bool,
    q: &RecordsQuery,
    at: &str,
) -> Result<serde_json::Value, AppError> {
    let cursor_at: Option<String>=q.cursor.as_deref().map(|c| conn.query_row(
        "SELECT cutoff_at FROM client_market_terminations WHERE id=?1 AND (?3=1 OR provider_user_id=?2) AND (?4 IS NULL OR host_id=?4)",
        params![c,user,owner,q.host_id],|r|r.get(0)).optional().map_err(db_error("read Client work cursor"))?
        .ok_or_else(||AppError::BadRequest("invalid termination cursor".into()))).transpose()?;
    // Pending candidates are open records only. Filter exact deadlines in Rust
    // (SQLite julianday rounds sub-milliseconds), before applying the page limit.
    let limit = if q.pending_only { "" } else { " LIMIT 51" };
    let scope = if owner {
        "?2=1"
    } else {
        "?2=0 AND t.provider_user_id=?1"
    };
    let mut statement = conn
        .prepare(&format!(
            "{FACTS} WHERE {scope}
        AND (?3 IS NULL OR t.host_id=?3) AND (?4=0 OR t.status!='completed')
        AND (?5 IS NULL OR t.cutoff_at<?6 OR (t.cutoff_at=?6 AND t.id<?5))
        ORDER BY t.cutoff_at DESC,t.id DESC{limit}"
        ))
        .map_err(db_error("prepare Client work records"))?;
    let candidates = statement
        .query_map(
            params![user, owner, q.host_id, q.pending_only, q.cursor, cursor_at],
            facts_row,
        )
        .map_err(db_error("query Client work records"))?;
    let mut items = Vec::new();
    for f in candidates {
        let f = f.map_err(db_error("read Client work records"))?;
        if q.pending_only && !project(&f, user, owner, at)?.action_required {
            continue;
        }
        items.push(view_tx(conn, &f, user, owner, at)?);
        if items.len() == 51 {
            break;
        }
    }
    let next = if items.len() > 50 {
        items[49]["id"].clone()
    } else {
        json!(null)
    };
    items.truncate(50);
    Ok(json!({"items":items,"nextCursor":next,"serverNow":at}))
}
pub(super) async fn records(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(q): Query<RecordsQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((
        no_store(),
        Json(records_tx(
            &conn,
            &actor.user_id,
            crate::api::is_finance_owner(&state.config, &actor.email),
            &q,
            &Utc::now().to_rfc3339(),
        )?),
    ))
}

pub(super) fn notify_started_tx(
    conn: &Connection,
    t: &Termination,
    at: &str,
) -> Result<(), AppError> {
    conn.execute("INSERT OR IGNORE INTO user_notification_events
        (id,scope,recipient_email,category,event_kind,object_kind,object_id,payload_json,created_at_ms)
        SELECT ?1,'user',email_normalized,'client','client_termination_started','client_termination',?2,?3,?4
        FROM users WHERE id=?5 AND id!=?6",params![format!("termination:{}:started:provider",t.id),t.id,
        json!({"terminationId":t.id,"hostId":t.host_id,"clientLabel":t.client_subdomain.as_deref().unwrap_or(&t.installation_id)}).to_string(),
        time(at)?.timestamp_millis(),t.provider_user_id,t.buyer_user_id]).map_err(db_error("notify Provider of immediate Client exit"))?;
    Ok(())
}
pub(super) fn notify_owner_ready_tx(
    conn: &Connection,
    t: &Termination,
    at: &str,
) -> Result<(), AppError> {
    // Visibility is Owner-scoped, independently of the administrator list.
    // Categorize this as a Client action so an explicitly configured finance
    // Owner can follow it even when they are not also a general admin.
    conn.execute("INSERT OR IGNORE INTO user_notification_events
        (id,scope,category,event_kind,object_kind,object_id,payload_json,created_at_ms)
        VALUES (?1,'owner','client','client_termination_decision_required','client_termination',?2,?3,?4)",
        params![format!("termination:{}:decision_required",t.id),t.id,
        json!({"terminationId":t.id,"hostId":t.host_id,"clientLabel":t.client_subdomain.as_deref().unwrap_or(&t.installation_id)}).to_string(),
        time(at)?.timestamp_millis()]).map_err(db_error("notify Owner of Client compensation decision"))?;
    Ok(())
}

#[cfg(test)]
mod tests;
