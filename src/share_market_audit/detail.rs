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
        .map_err(db_error("begin Share rental audit detail read"))?;
    let value = detail_tx(&tx, &id, &q)?;
    tx.commit()
        .map_err(db_error("finish Share rental audit detail read"))?;
    Ok((no_store(), Json(value)))
}

pub(super) fn detail_tx(conn: &Connection, id: &str, q: &DetailQuery) -> Result<Value, AppError> {
    if id.len() > 200
        || q.event_cursor.as_ref().is_some_and(|s| s.len() > 200)
        || q.period_cursor.is_some_and(|n| n <= 0)
    {
        return Err(AppError::BadRequest(
            "invalid Share rental audit detail cursor".into(),
        ));
    }
    let mut record = conn
        .query_row(
            &format!("{FACTS}SELECT {COLUMNS} FROM records WHERE id=?1"),
            params![id],
            record_row,
        )
        .optional()
        .map_err(db_error("read Share rental audit detail"))?
        .ok_or_else(|| AppError::NotFound("Share rental audit record not found".into()))?;
    attach_money(conn, std::slice::from_mut(&mut record))?;
    let contract = record["contractId"].as_str();
    let (periods, periods_next) = periods_tx(conn, contract, q.period_cursor)?;
    let (events, events_next) = events_tx(conn, &record, q.event_cursor.as_deref())?;
    let trial = conn
        .query_row(
            "SELECT phase,token_allowance,token_budget,value_allowance_usd_micros,value_budget_usd_micros
             FROM market_share_trial_contracts WHERE contract_id=?1",
            params![contract],
            |r| {
                // A USD-denominated trial keeps both Token columns at 0 by
                // constraint; they are not a Token allowance.
                let usd = r.get::<_, Option<String>>(3)?;
                let token = |i: usize| -> crate::db::Result<Value> {
                    Ok(if usd.is_some() {
                        Value::Null
                    } else {
                        json!(r.get::<_, i64>(i)?.to_string())
                    })
                };
                Ok(json!({
                    "phase":r.get::<_,String>(0)?,"tokenAllowance":token(1)?,"tokenBudget":token(2)?,
                    "valueAllowanceUsdMicros":usd,"valueBudgetUsdMicros":r.get::<_,Option<String>>(4)?
                }))
            },
        )
        .optional()
        .map_err(db_error("read audited Share trial terms"))?;
    let number = |v: Option<i64>| v.map(|v| v.to_string());
    let settlement = conn
        .query_row(
            "SELECT status,json_extract(payload_json,'$.heldUnits'),json_extract(payload_json,'$.returnedUnits'),
               json_extract(payload_json,'$.compensationUnits'),json_extract(payload_json,'$.responsibility'),
               COALESCE(strftime('%Y-%m-%dT%H:%M:%fZ',cutoff_at),cutoff_at),json_extract(payload_json,'$.reason')
             FROM share_market_terminations WHERE id=?1",
            params![record["terminationId"].as_str()],
            |r| {
                Ok(json!({
                    "status":r.get::<_,String>(0)?,"heldUnits":number(r.get(1)?),
                    "returnedUnits":number(r.get(2)?),"compensationUnits":number(r.get(3)?),
                    "responsibility":r.get::<_,Option<String>>(4)?,"cutoffAt":r.get::<_,String>(5)?,
                    "reason":r.get::<_,Option<String>>(6)?
                }))
            },
        )
        .optional()
        .map_err(db_error("read audited Share termination facts"))?;
    Ok(
        json!({"record":record,"periods":periods,"periodsNextCursor":periods_next,"events":events,
        "eventsNextCursor":events_next,"trial":trial,"settlement":settlement,"serverNow":Utc::now().to_rfc3339()}),
    )
}

fn periods_tx(
    conn: &Connection,
    contract: Option<&str>,
    cursor: Option<i64>,
) -> Result<(Vec<Value>, Option<i64>), AppError> {
    if let Some(cursor) = cursor {
        let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM market_platform_periods WHERE contract_id=?1 AND sequence=?2)",
            params![contract,cursor],|r|r.get(0)).map_err(db_error("validate audited Share period cursor"))?;
        if !exists {
            return Err(AppError::BadRequest(
                "invalid audited Share period cursor".into(),
            ));
        }
    }
    // Delivered service is time delivery plus quota value recognised on an
    // immediate termination; both are persisted on the period itself.
    let mut statement=conn.prepare("SELECT p.id,p.sequence,p.period_start,p.period_end,p.amount_units,
        p.fulfilled_units+p.share_quota_units,p.refunded_units,p.termination_compensation_units,p.debit_journal_id,
        p.debited_at,p.status,t.rate_bps,
        (SELECT rate_bps FROM market_commission_periods fee WHERE fee.source_kind='monthly_period' AND fee.source_id=p.id)
        FROM market_platform_periods p LEFT JOIN share_termination_period_terms t ON t.period_id=p.id
        WHERE p.contract_id=?1 AND (?2 IS NULL OR p.sequence<?2) ORDER BY p.sequence DESC LIMIT 51")
        .map_err(db_error("prepare audited Share periods"))?;
    let mut rows=statement.query_map(params![contract,cursor],|r|Ok(json!({
        "id":r.get::<_,String>(0)?,"sequence":r.get::<_,i64>(1)?,"startedAt":r.get::<_,String>(2)?,"endedAt":r.get::<_,String>(3)?,
        "amountUnits":r.get::<_,i64>(4)?.to_string(),"serviceUnits":r.get::<_,i64>(5)?.to_string(),"returnedUnits":r.get::<_,i64>(6)?.to_string(),
        "compensationUnits":r.get::<_,i64>(7)?.to_string(),"journalId":r.get::<_,String>(8)?,"debitedAt":r.get::<_,String>(9)?,
        "status":r.get::<_,String>(10)?,"terminationRateBps":r.get::<_,Option<i64>>(11)?,"commissionRateBps":r.get::<_,Option<i64>>(12)?
    }))).map_err(db_error("query audited Share periods"))?.collect::<crate::db::Result<Vec<_>>>().map_err(db_error("read audited Share periods"))?;
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
    // Termination events store only a user ID. Name the actor only when it is
    // one of the frozen transaction parties; never borrow a current identity.
    // Notes are exposed only for the human-written compensation decisions.
    let cte = "WITH parties AS (
        SELECT buyer_user_id,buyer_email,provider_user_id,provider_email
        FROM share_market_rental_identities WHERE id=?1
    ), events AS (
        SELECT 'e:'||e.id AS id,e.event_type AS action,e.actor_email,
          COALESCE(strftime('%Y-%m-%dT%H:%M:%fZ',e.created_at),e.created_at) AS at,NULL AS note
        FROM share_market_events e WHERE e.subscription_id=?1
        UNION ALL SELECT 't:'||e.id,e.action,
          (SELECT CASE WHEN e.actor_user_id=p.buyer_user_id THEN p.buyer_email
                       WHEN e.actor_user_id=p.provider_user_id THEN p.provider_email END FROM parties p),
          COALESCE(strftime('%Y-%m-%dT%H:%M:%fZ',e.created_at),e.created_at),
          CASE WHEN e.action IN ('respond','waive','resolve','withdraw','timeout') AND json_valid(e.detail_json)
               THEN json_extract(e.detail_json,'$.note') END
        FROM share_termination_events e WHERE e.termination_id=?2
        UNION ALL SELECT 'order:'||?1,'order_committed',NULL,?3,NULL
    ) ";
    let mut args = vec![
        SqlValue::Text(record["id"].as_str().unwrap_or_default().into()),
        record["terminationId"]
            .as_str()
            .map(|v| SqlValue::Text(v.into()))
            .unwrap_or(SqlValue::Null),
        SqlValue::Text(record["createdAt"].as_str().unwrap_or_default().into()),
    ];
    let mut predicate = "1=1".to_string();
    if let Some(cursor) = cursor {
        let mut lookup = args.clone();
        lookup.push(SqlValue::Text(cursor.into()));
        let at: String = conn
            .query_row(
                &format!("{cte}SELECT at FROM events WHERE id=?4"),
                lookup,
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error("validate audited Share event cursor"))?
            .ok_or_else(|| AppError::BadRequest("invalid audited Share event cursor".into()))?;
        args.push(SqlValue::Text(at));
        args.push(SqlValue::Text(cursor.into()));
        predicate = "(at<?4 OR (at=?4 AND id<?5))".into();
    }
    let mut statement=conn.prepare(&format!("{cte}SELECT id,action,actor_email,at,note FROM events WHERE {predicate} ORDER BY at DESC,id DESC LIMIT 51"))
        .map_err(db_error("prepare audited Share events"))?;
    let mut rows=statement.query_map(args,|r|Ok(json!({
        "id":r.get::<_,String>(0)?,"action":safe_action(&r.get::<_,String>(1)?),"actorEmail":r.get::<_,Option<String>>(2)?,
        "createdAt":r.get::<_,String>(3)?,"note":r.get::<_,Option<String>>(4)?.map(|s|s.chars().take(2000).collect::<String>())
    }))).map_err(db_error("query audited Share events"))?.collect::<crate::db::Result<Vec<_>>>().map_err(db_error("read audited Share events"))?;
    let next = if rows.len() > DETAIL_PAGE_SIZE {
        rows[DETAIL_PAGE_SIZE - 1]["id"].clone()
    } else {
        Value::Null
    };
    rows.truncate(DETAIL_PAGE_SIZE);
    Ok((rows, next))
}

/// Allowlisted action vocabulary. Termination actions use the Client audit's
/// names so both audits share translations; anything else is `updated`.
pub(super) fn safe_action(value: &str) -> &str {
    match value {
        "return_unused" | "return_compensation" => "returned",
        "automatic_credit" => "credited",
        "share_termination" => "awarded",
        "share_quota" => "quota_delivered",
        "respond" => "provider_response",
        "waive" => "provider_waiver",
        "resolve" => "owner_decision",
        "withdraw" => "buyer_withdrawal",
        "timeout" => "timeout_waiver",
        "order_committed"
        | "accepted"
        | "settled"
        | "seat_rented"
        | "entitlement_activated"
        | "entitlement_failed"
        | "entitlement_grant_retried"
        | "entitlement_revoke_requested"
        | "entitlement_contract_violation"
        | "service_activation_rejected"
        | "service_term_expiring"
        | "service_term_expired"
        | "subscription_released"
        | "rental_cancelled_at_period_end"
        | "owner_revoke_requested"
        | "supplier_termination_requested"
        | "revoke_failed"
        | "control_dead_letter_requeued"
        | "billing_suspension_requested"
        | "billing_suspension_failed"
        | "billing_suspended"
        | "billing_resume_requested"
        | "billing_resume_failed"
        | "billing_resumed"
        | "contract_integrity_violated"
        | "contract_integrity_repair_started"
        | "contract_integrity_restored" => value,
        _ => "updated",
    }
}
