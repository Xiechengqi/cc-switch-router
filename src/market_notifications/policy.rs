//! Live-state checks run before retrying instructions. Historical receipts
//! remain facts; stale requests for action must never be resurrected by mail.
use super::*;

pub(super) fn notice_current_tx(
    conn: &Connection,
    item: &Claimed,
    time: &str,
) -> Result<bool, AppError> {
    let field = |key: &str| item.payload.get(key).and_then(Value::as_str);
    match item.event_kind.as_str() {
        "c2c_order_updated" | "c2c_seller_changed" => super::c2c_notice_current_tx(conn,item,time),
        "topup_credited" => conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM market_topup_orders t JOIN market_completed_topup_evidence e ON e.id=t.id WHERE t.id=?1 AND t.user_id=?2 AND t.status='credited')",
            params![field("orderId"),item.recipient_user_id],|r|r.get(0)).map_err(db_error("recheck top-up receipt")),
        "topup_review_required" => conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM market_topup_orders t
                JOIN market_finance_reconciliation_cases c ON c.object_kind='topup' AND c.object_id=t.id AND c.status='open'
                WHERE t.id=?1 AND t.user_id=?2 AND t.status='review_required'
                AND (?3 IS NULL OR c.id=?3)
                AND NOT(c.code='topup_verification_failed' AND COALESCE(c.detail_code,'') IN
                    ('gateway_lookup_failed','gateway_status_unknown','create_result_unknown','gateway_unavailable')))",
            params![field("orderId"),item.recipient_user_id,field("reviewCaseId")],|r|r.get(0)).map_err(db_error("recheck payment review notice")),
        "wallet_low_runway" | "renewal_upcoming_shortfall" => {
            let Some(id)=field("contractId") else{return Ok(false);};
            let at=DateTime::parse_from_rfc3339(time).map_err(|_|AppError::Internal("invalid reminder time".into()))?.with_timezone(&Utc);
            crate::market_platform_contracts::funding_notice_current_tx(conn,&item.recipient_user_id,id,&item.event_kind,field("renewsAt"),at)
        }
        "contract_suspended" | "contract_resumed" | "contract_terminating_soon" | "contract_terminated" => {
            let expected=match item.event_kind.as_str() {"contract_resumed"=>"active","contract_terminated"=>"ended",_=>"suspended"};
            conn.query_row("SELECT EXISTS(SELECT 1 FROM market_platform_contracts WHERE id=?1 AND buyer_user_id=?2 AND status=?3
                AND (?4!='contract_terminating_soon' OR ?5='contract_terminating_soon:'||id||':'||suspended_at))",
                params![field("contractId"),item.recipient_user_id,expected,item.event_kind,field("notificationEventKey")],|r|r.get(0)).map_err(db_error("recheck service notice"))
        }
        _ => Ok(true),
    }
}

pub(super) fn related_count_tx(
    conn: &Connection,
    item: &Claimed,
    purpose: &str,
    time: &str,
) -> Result<usize, AppError> {
    let created = item
        .payload
        .get("notificationTime")
        .and_then(Value::as_str)
        .unwrap_or("");
    let rows = {
        let mut statement=conn.prepare("SELECT id,event_kind,payload_json,object_id FROM market_notification_outbox
            WHERE recipient_user_id=?1 AND
            ((?2='funding_summary' AND event_kind IN ('wallet_low_runway','renewal_upcoming_shortfall') AND substr(created_at,1,10)=substr(?3,1,10))
            OR (?2='service_recovery_summary' AND event_kind='contract_resumed'
                AND CAST(strftime('%s',created_at) AS INTEGER)/300=CAST(strftime('%s',?3) AS INTEGER)/300))")
            .map_err(db_error("prepare grouped notification"))?;
        statement
            .query_map(params![item.recipient_user_id, purpose, created], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(db_error("query grouped notification"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error("read grouped notification"))?
    };
    let mut contracts = std::collections::HashSet::new();
    for (id, kind, payload, contract) in rows {
        let other = Claimed {
            id,
            recipient_user_id: item.recipient_user_id.clone(),
            event_kind: kind,
            payload: serde_json::from_str(&payload).unwrap_or_else(|_| json!({})),
            mandatory: false,
            attempts: 0,
        };
        if notice_current_tx(conn, &other, time)? && !recipient_tx(conn, &other)?.opted_out {
            contracts.insert(contract);
        }
    }
    Ok(contracts.len())
}
