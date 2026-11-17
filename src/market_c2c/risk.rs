use super::*;
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BuyerLimits {
    pub max_minor: i64,
    pub max_active: i64,
    pub tier: &'static str,
    pub blocked_until: Option<String>,
    pub suspended: bool,
}
pub(super) fn buyer_limits_tx(
    conn: &Connection,
    user: &str,
    time: &str,
) -> Result<BuyerLimits, AppError> {
    let (age,total,count,peers):(f64,i64,i64,i64)=conn.query_row("SELECT MAX(0,COALESCE(julianday(?2)-julianday(u.created_at),0)), COALESCE((SELECT SUM(amount_units)/86400 FROM market_c2c_orders WHERE buyer_id=u.id AND status='released'),0),(SELECT COUNT(*) FROM market_c2c_orders WHERE buyer_id=u.id AND status='released'),(SELECT COUNT(DISTINCT seller_id) FROM market_c2c_orders WHERE buyer_id=u.id AND status='released') FROM users u WHERE id=?1",params![user,time],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(db_error("read C2C progressive limits"))?.unwrap_or((0.0,0,0,0));
    let (max_minor, max_active, tier) =
        if age >= 30.0 && total >= 200000 && count >= 10 && peers >= 3 {
            (500000, 2, "established")
        } else if age >= 7.0 && total >= 50000 && count >= 3 && peers >= 2 {
            (50000, 2, "regular")
        } else {
            (10000, 1, "new")
        };
    let false_claims:i64=conn.query_row("SELECT COUNT(*) FROM market_financial_risk_events e WHERE kind='c2c_false_payment' AND subject_id=?1 AND NOT EXISTS(SELECT 1 FROM market_financial_risk_events r WHERE r.kind='c2c_false_payment_cleared' AND r.subject_id=e.subject_id AND r.object_id=e.id)",params![user],|r|r.get(0)).map_err(db_error("read C2C false-payment decisions"))?;
    // A qualifying third strike starts a full 24-hour cooldown. Earlier
    // strikes aging out of today's window must not end that cooldown early.
    let triggered: Option<String> = conn
        .query_row(
            "SELECT e.created_at FROM market_financial_risk_events e
         WHERE e.subject_id=?1 AND e.kind='c2c_unpaid'
           AND julianday(e.created_at)>julianday(?2)-1
           AND julianday(e.created_at)<=julianday(?2)
           AND (SELECT COUNT(*) FROM market_financial_risk_events s
                WHERE s.subject_id=e.subject_id AND s.kind='c2c_unpaid'
                  AND julianday(s.created_at)>julianday(e.created_at)-1
                  AND julianday(s.created_at)<=julianday(e.created_at))>=3
         ORDER BY julianday(e.created_at) DESC LIMIT 1",
            params![user, time],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error("read C2C unpaid cooldown"))?;
    let blocked_until = triggered
        .map(|time| at(&time).map(|trigger| (trigger + Duration::hours(24)).to_rfc3339()))
        .transpose()?;
    Ok(BuyerLimits {
        max_minor,
        max_active,
        tier,
        blocked_until,
        suspended: false_claims > 0,
    })
}
fn require_buyer_tx(conn: &Connection, user: &str, time: &str) -> Result<BuyerLimits, AppError> {
    let limits = buyer_limits_tx(conn, user, time)?;
    if limits.suspended || limits.blocked_until.is_some() {
        return Err(problem(
            "C2C_BUYER_RESTRICTED",
            "new purchases are suspended after a verified false payment or repeated unpaid reservations; existing payment claims remain available",
        ));
    }
    Ok(limits)
}
pub(super) fn strike_tx(
    conn: &Connection,
    o: &Order,
    kind: &str,
    actor: &str,
    evidence: &str,
    time: &str,
) -> Result<(), AppError> {
    conn.execute("INSERT OR IGNORE INTO market_financial_risk_events(id,kind,subject_id,object_id,details_json,created_at) VALUES(?1,?2,?3,?4,?5,?6)",params![Uuid::new_v4().to_string(),kind,o.buyer_id,o.id,serde_json::json!({"actor":actor,"evidence":evidence}).to_string(),time]).map_err(db_error("record C2C abuse decision"))?;
    Ok(())
}
pub(super) fn admission_tx(
    conn: &Connection,
    buyer: &str,
    seller: &str,
    minor: i64,
    time: &str,
) -> Result<(), AppError> {
    let limits = require_buyer_tx(conn, buyer, time)?;
    if minor > limits.max_minor {
        return Err(problem(
            "C2C_BUYER_TIER_LIMIT",
            "amount exceeds this buyer's account-age and completed-volume limit",
        ));
    }
    let (count,amount):(i64,i64)=conn.query_row(&format!("SELECT COUNT(*),COALESCE(SUM(amount_units)/86400,0) FROM market_c2c_orders WHERE buyer_id=?1 AND status IN {ACTIVE}"),params![buyer],|r|Ok((r.get(0)?,r.get(1)?))).map_err(db_error("bound C2C buyer exposure"))?;
    if count >= limits.max_active
        || amount.saturating_add(minor) > limits.max_minor * limits.max_active
    {
        return Err(problem(
            "C2C_ORDER_LIMIT",
            "too many unfinished purchases or too much reserved credit",
        ));
    }
    if limits.tier == "new" {
        let (count,total):(i64,i64)=conn.query_row(&format!("SELECT COUNT(*),COALESCE(SUM(amount_units)/86400,0) FROM market_c2c_orders o WHERE o.seller_id=?1 AND o.status IN {ACTIVE} AND NOT EXISTS(SELECT 1 FROM market_c2c_orders r WHERE r.buyer_id=o.buyer_id AND r.status='released' AND EXISTS(SELECT 1 FROM users u WHERE u.id=r.buyer_id AND julianday(?2)-julianday(u.created_at)>=7) GROUP BY r.buyer_id HAVING SUM(r.amount_units)>=4320000000 AND COUNT(*)>=3 AND COUNT(DISTINCT r.seller_id)>=2)"),params![seller,time],|r|Ok((r.get(0)?,r.get(1)?))).map_err(db_error("bound C2C new-buyer seller exposure"))?;
        let posted = wallet_snapshot_tx(conn, seller)?.posted_units / 86400;
        let cap = (posted / 4).clamp(10000, 100000);
        if count >= 3 || total.saturating_add(minor) > cap {
            return Err(problem(
                "C2C_NEW_BUYER_EXPOSURE",
                "seller's reservations for new buyers are temporarily full",
            ));
        }
    }
    crate::market_risk::require_incoming_tx(conn, buyer, units_from_minor(minor)?, time, true)
}
pub(super) fn nonreceipt_tx(
    conn: &Connection,
    o: &Order,
    seller: &str,
    version: i64,
    note: &str,
    time: &str,
) -> Result<Order, AppError> {
    if o.seller_id != seller {
        return Err(AppError::Forbidden(
            "only the seller can report non-receipt".into(),
        ));
    }
    let prior: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM market_c2c_payment_reviews WHERE order_id=?1)",
            params![o.id],
            |r| r.get(0),
        )
        .map_err(db_error("read C2C non-receipt replay"))?;
    if prior {
        return order_tx(conn, &o.id);
    }
    if o.version != version || !matches!(o.status.as_str(), "paid" | "disputed") {
        return Err(problem(
            "C2C_STALE",
            "reload the payment-declared order before reporting non-receipt",
        ));
    }
    let note = text(note, 1500)?;
    let o = if o.status == "paid" {
        dispute_tx(conn, o, seller, &note, time)?
    } else {
        o.clone()
    };
    conn.execute("INSERT INTO market_c2c_payment_reviews(order_id,opened_by,reason,evidence_due_at,created_at) VALUES(?1,?2,?3,?4,?5)",params![o.id,seller,note,(at(time)?+Duration::hours(24)).to_rfc3339(),time]).map_err(db_error("open C2C non-receipt review"))?;
    event_tx(conn, &o, seller, "nonreceipt_reported", &note, time)?;
    order_tx(conn, &o.id)
}
pub(super) fn evidence_tx(
    conn: &Connection,
    o: &Order,
    buyer: &str,
    note: &str,
    time: &str,
) -> Result<Order, AppError> {
    if o.buyer_id != buyer || o.status != "disputed" {
        return Err(AppError::Forbidden(
            "payment evidence is only accepted from this disputed order's buyer".into(),
        ));
    }
    let note = text(note, 1500)?;
    // Evidence after the deadline remains admissible; a missed deadline never
    // proves non-payment and cannot destroy an actually paid buyer's claim.
    if conn.execute("UPDATE market_c2c_payment_reviews SET buyer_evidence=?2,evidence_at=?3 WHERE order_id=?1 AND resolved_at IS NULL",params![o.id,note,time]).map_err(db_error("record C2C payment evidence"))?!=1{return Err(problem("C2C_STALE","no unresolved non-receipt review exists"))}
    event_tx(conn, o, buyer, "payment_evidence_submitted", &note, time)?;
    order_tx(conn, &o.id)
}
pub(super) fn tick_reviews_tx(conn: &Connection, time: &str) -> Result<(), AppError> {
    let mut s=conn.prepare("SELECT order_id FROM market_c2c_payment_reviews WHERE resolved_at IS NULL AND escalated_at IS NULL AND julianday(evidence_due_at)<=julianday(?1) ORDER BY evidence_due_at,order_id LIMIT 100").map_err(db_error("prepare C2C evidence deadlines"))?;
    let ids = s
        .query_map(params![time], |r| r.get::<_, String>(0))
        .map_err(db_error("read C2C evidence deadlines"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read C2C evidence deadlines"))?;
    for id in ids {
        let o = order_tx(conn, &id)?;
        if matches!(o.status.as_str(), "released" | "cancelled") {
            conn.execute(
                "UPDATE market_c2c_payment_reviews SET resolved_at=?2 WHERE order_id=?1",
                params![id, time],
            )
            .map_err(db_error("close C2C payment review"))?;
            continue;
        }
        event_tx(
            conn,
            &o,
            "system",
            "evidence_deadline_escalated",
            "independent adjudication required; funds stay reserved",
            time,
        )?;
        crate::market_notifications::owner_signal_tx(
            conn,
            "c2c_dispute",
            "c2c_order",
            &id,
            serde_json::json!({"reason":"nonreceipt_evidence_deadline","orderId":id}),
            time,
            false,
        )?;
        conn.execute(
            "UPDATE market_c2c_payment_reviews SET escalated_at=?2 WHERE order_id=?1",
            params![id, time],
        )
        .map_err(db_error("escalate C2C payment review"))?;
    }
    Ok(())
}

/// A correction appends evidence; it never removes the original adjudication or
/// rewrites the terminal order. The decision id is the concurrency/replay key.
pub(super) fn restriction_tx(
    conn: &Connection,
    order: &str,
) -> Result<Option<serde_json::Value>, AppError> {
    conn.query_row("SELECT e.id,e.subject_id,e.created_at,(SELECT r.created_at FROM market_financial_risk_events r WHERE r.kind='c2c_false_payment_cleared' AND r.subject_id=e.subject_id AND r.object_id=e.id) FROM market_financial_risk_events e WHERE e.kind='c2c_false_payment' AND e.object_id=?1", params![order], |r| Ok(serde_json::json!({"decisionId":r.get::<_,String>(0)?,"buyerId":r.get::<_,String>(1)?,"createdAt":r.get::<_,String>(2)?,"clearedAt":r.get::<_,Option<String>>(3)?}))).optional().map_err(db_error("read C2C purchase restriction"))
}
pub(super) fn clear_restriction_tx(
    conn: &Connection,
    order: &Order,
    actor: &crate::models::AuthSession,
    decision: &str,
    evidence: &str,
    attest: bool,
    time: &str,
) -> Result<serde_json::Value, AppError> {
    if actor.user_id == order.buyer_id || actor.user_id == order.seller_id {
        return Err(AppError::Forbidden(
            "a participant cannot clear this order's buyer restriction".into(),
        ));
    }
    crate::market_finance_auth::require_live_session_tx(conn, actor, time)?;
    let evidence = text(evidence, 1500)?;
    if !attest || evidence.chars().count() < 8 {
        return Err(AppError::BadRequest(
            "independent correction evidence and attestation are required".into(),
        ));
    }
    let current = restriction_tx(conn, &order.id)?
        .ok_or_else(|| AppError::NotFound("no false-payment restriction for this order".into()))?;
    if current["decisionId"].as_str() != Some(decision) {
        return Err(problem("C2C_STALE", "buyer restriction changed; reload"));
    }
    if !current["clearedAt"].is_null() {
        return Ok(current);
    }
    conn.execute("INSERT INTO market_financial_risk_events(id,kind,subject_id,object_id,details_json,created_at) VALUES(?1,'c2c_false_payment_cleared',?2,?3,?4,?5)",params![Uuid::new_v4().to_string(),order.buyer_id,decision,serde_json::json!({"actor":actor.user_id,"orderId":order.id,"evidence":evidence,"proofId":actor.financial_proof_id}).to_string(),time]).map_err(db_error("correct C2C buyer restriction"))?;
    event_tx(
        conn,
        order,
        &actor.user_id,
        "buyer_restriction_cleared",
        &evidence,
        time,
    )?;
    crate::market_notifications::enqueue_user_notification_tx(
        conn,
        "buyer",
        &order.buyer_id,
        "c2c_buyer_restriction_changed",
        "c2c_order",
        &order.id,
        &format!("c2c_restriction_cleared:{decision}"),
        serde_json::json!({"orderId":order.id,"cleared":true}),
        time,
    )?;
    restriction_tx(conn, &order.id)?
        .ok_or_else(|| AppError::Internal("missing corrected restriction".into()))
}
