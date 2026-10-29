use super::*;

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DetailQuery {
    pub(super) period_cursor: Option<i64>,
    pub(super) event_cursor: Option<String>,
}

pub(super) async fn get(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<DetailQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::api::require_owner_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction()
        .map_err(db_error("begin rental audit detail read"))?;
    let value = detail_tx(&tx, &id, &q)?;
    tx.commit()
        .map_err(db_error("finish rental audit detail read"))?;
    Ok((no_store(), Json(value)))
}

pub(super) fn detail_tx(conn: &Connection, id: &str, q: &DetailQuery) -> Result<Value, AppError> {
    if id.len() > 200
        || q.event_cursor.as_ref().is_some_and(|s| s.len() > 200)
        || q.period_cursor.is_some_and(|n| n <= 0)
    {
        return Err(AppError::BadRequest(
            "invalid rental audit detail cursor".into(),
        ));
    }
    let mut record = conn
        .query_row(
            &format!("{FACTS}SELECT {COLUMNS} FROM records WHERE id=?1"),
            params![id],
            record_row,
        )
        .optional()
        .map_err(db_error("read rental audit detail"))?
        .ok_or_else(|| AppError::NotFound("rental audit record not found".into()))?;
    attach_money(conn, std::slice::from_mut(&mut record))?;
    let contract = record["contractId"].as_str();
    let (periods, periods_next) = periods_tx(conn, contract, q.period_cursor)?;
    let (events, events_next) = events_tx(conn, &record, q.event_cursor.as_deref())?;
    let trial_consumed:i64=conn.query_row("SELECT COALESCE(SUM(consumed_seconds),0) FROM market_platform_trial_segments WHERE contract_id=?1",
        params![contract],|r|r.get(0)).map_err(db_error("read audited trial consumption"))?;
    let settlement=conn.query_row(
        "SELECT held_units,returned_units,compensation_units,reason,responsibility,cutoff_at,cleanup_confirmed_at
         FROM client_market_terminations WHERE id=?1",params![record["terminationId"].as_str()],|r|Ok(json!({
            "heldUnits":r.get::<_,i64>(0)?.to_string(),"returnedUnits":r.get::<_,i64>(1)?.to_string(),
            "compensationUnits":r.get::<_,i64>(2)?.to_string(),"reason":r.get::<_,String>(3)?,"responsibility":r.get::<_,String>(4)?,
            "cutoffAt":r.get::<_,String>(5)?,"cleanupConfirmedAt":r.get::<_,Option<String>>(6)?
         }))).optional().map_err(db_error("read audited termination facts"))?;
    Ok(
        json!({"record":record,"periods":periods,"periodsNextCursor":periods_next,"events":events,
        "eventsNextCursor":events_next,"trialConsumedSeconds":trial_consumed,"settlement":settlement,"serverNow":Utc::now().to_rfc3339()}),
    )
}

fn periods_tx(
    conn: &Connection,
    contract: Option<&str>,
    cursor: Option<i64>,
) -> Result<(Vec<Value>, Option<i64>), AppError> {
    if let Some(cursor) = cursor {
        let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM market_platform_periods WHERE contract_id=?1 AND sequence=?2)",
            params![contract,cursor],|r|r.get(0)).map_err(db_error("validate audited period cursor"))?;
        if !exists {
            return Err(AppError::BadRequest("invalid audited period cursor".into()));
        }
    }
    let mut statement=conn.prepare("SELECT p.id,p.sequence,p.period_start,p.period_end,p.amount_units,p.fulfilled_units,p.refunded_units,
        p.termination_compensation_units,p.debit_journal_id,p.debited_at,p.status,t.rate_bps,
        (SELECT rate_bps FROM market_commission_periods fee WHERE fee.source_kind='monthly_period' AND fee.source_id=p.id)
        FROM market_platform_periods p LEFT JOIN client_termination_period_terms t ON t.period_id=p.id
        WHERE p.contract_id=?1 AND (?2 IS NULL OR p.sequence<?2) ORDER BY p.sequence DESC LIMIT 51")
        .map_err(db_error("prepare audited periods"))?;
    let mut rows=statement.query_map(params![contract,cursor],|r|Ok(json!({
        "id":r.get::<_,String>(0)?,"sequence":r.get::<_,i64>(1)?,"startedAt":r.get::<_,String>(2)?,"endedAt":r.get::<_,String>(3)?,
        "amountUnits":r.get::<_,i64>(4)?.to_string(),"serviceUnits":r.get::<_,i64>(5)?.to_string(),"returnedUnits":r.get::<_,i64>(6)?.to_string(),
        "compensationUnits":r.get::<_,i64>(7)?.to_string(),"journalId":r.get::<_,String>(8)?,"debitedAt":r.get::<_,String>(9)?,
        "status":r.get::<_,String>(10)?,"terminationRateBps":r.get::<_,Option<i64>>(11)?,"commissionRateBps":r.get::<_,Option<i64>>(12)?
    }))).map_err(db_error("query audited periods"))?.collect::<crate::db::Result<Vec<_>>>().map_err(db_error("read audited periods"))?;
    let next = if rows.len() > DETAIL_PAGE_SIZE {
        rows[DETAIL_PAGE_SIZE - 1]["sequence"].as_i64()
    } else {
        None
    };
    rows.truncate(DETAIL_PAGE_SIZE);
    Ok((rows, next))
}

fn events_tx(
    conn: &Connection,
    record: &Value,
    cursor: Option<&str>,
) -> Result<(Vec<Value>, Value), AppError> {
    // Subscription events already mirror the general audit events: do NOT union
    // both tables and display each action twice. Never project detail_json/logs.
    let cte = "WITH events AS (
        SELECT 'r:'||id AS id,event_type AS action,actor_email,
          strftime('%Y-%m-%dT%H:%M:%fZ',created_at) AS at,NULL AS note
        FROM client_market_subscription_events WHERE installation_id=?1
        UNION ALL SELECT 't:'||id,action,NULL,strftime('%Y-%m-%dT%H:%M:%fZ',created_at),
          CASE WHEN action IN ('provider_waiver','owner_decision','provider_response','buyer_withdrawal','timeout_waiver')
                    AND json_valid(detail_json) THEN json_extract(detail_json,'$.note') END
        FROM client_termination_events WHERE termination_id=?2
        UNION ALL SELECT 'order:'||?3,'order_committed',NULL,?4,NULL
    ) ";
    let mut args = vec![
        record["installationId"]
            .as_str()
            .map(|v| SqlValue::Text(v.into()))
            .unwrap_or(SqlValue::Null),
        record["terminationId"]
            .as_str()
            .map(|v| SqlValue::Text(v.into()))
            .unwrap_or(SqlValue::Null),
        SqlValue::Text(record["id"].as_str().unwrap_or_default().into()),
        SqlValue::Text(record["createdAt"].as_str().unwrap_or_default().into()),
    ];
    let mut predicate = "1=1".to_string();
    if let Some(cursor) = cursor {
        let mut lookup = args.clone();
        lookup.push(SqlValue::Text(cursor.into()));
        let at: String = conn
            .query_row(
                &format!("{cte}SELECT at FROM events WHERE id=?5"),
                lookup,
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error("validate audited event cursor"))?
            .ok_or_else(|| AppError::BadRequest("invalid audited event cursor".into()))?;
        args.push(SqlValue::Text(at));
        args.push(SqlValue::Text(cursor.into()));
        predicate = "(at<?5 OR (at=?5 AND id<?6))".into();
    }
    let mut statement=conn.prepare(&format!("{cte}SELECT id,action,actor_email,at,note FROM events WHERE {predicate} ORDER BY at DESC,id DESC LIMIT 51"))
        .map_err(db_error("prepare audited events"))?;
    let mut rows=statement.query_map(args,|r|Ok(json!({
        "id":r.get::<_,String>(0)?,"action":safe_action(&r.get::<_,String>(1)?),"actorEmail":r.get::<_,Option<String>>(2)?,
        "createdAt":r.get::<_,String>(3)?,"note":r.get::<_,Option<String>>(4)?.map(|s|s.chars().take(2000).collect::<String>())
    }))).map_err(db_error("query audited events"))?.collect::<crate::db::Result<Vec<_>>>().map_err(db_error("read audited events"))?;
    let next = if rows.len() > DETAIL_PAGE_SIZE {
        rows[DETAIL_PAGE_SIZE - 1]["id"].clone()
    } else {
        Value::Null
    };
    rows.truncate(DETAIL_PAGE_SIZE);
    Ok((rows, next))
}

fn safe_action(value: &str) -> &str {
    match value {
        "return_unused" | "return_compensation" | "return_disputed" => "returned",
        "automatic_credit" => "credited",
        "order_committed"
        | "client_provisioned"
        | "cleanup_started"
        | "cleanup_finished"
        | "cleanup_failed"
        | "cleanup_confirmed"
        | "subscription_force_released"
        | "subscription_release_finalized"
        | "free_period_expiring"
        | "free_period_expired"
        | "accepted"
        | "provider_waiver"
        | "owner_decision"
        | "provider_response"
        | "buyer_withdrawal"
        | "timeout_waiver"
        | "disputed"
        | "awarded"
        | "returned"
        | "credited" => value,
        _ => "updated",
    }
}
