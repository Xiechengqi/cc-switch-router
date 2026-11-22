//! A mistaken transfer is corrected by a new, recipient-consented journal.
//! Administrators may assist a request but cannot confiscate another balance.
//!
//! Retired for new requests: a transfer to a new recipient now waits before it
//! credits and can be withdrawn by its sender, and a recipient can send money
//! back as an ordinary transfer. Requests created earlier can still be decided
//! by their recipient.
use super::*;
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Return {
    id: String,
    transfer_id: String,
    requested_by: String,
    reason: String,
    status: String,
    version: i64,
    journal_id: Option<String>,
    created_at: String,
    resolved_at: Option<String>,
}
fn read_tx(c: &Connection, transfer: &str) -> Result<Option<Return>, AppError> {
    c.query_row("SELECT id,transfer_id,requested_by,reason,status,version,journal_id,created_at,resolved_at FROM market_transfer_returns WHERE transfer_id=?1",params![transfer],|r|Ok(Return{id:r.get(0)?,transfer_id:r.get(1)?,requested_by:r.get(2)?,reason:r.get(3)?,status:r.get(4)?,version:r.get(5)?,journal_id:r.get(6)?,created_at:r.get(7)?,resolved_at:r.get(8)?})).optional().map_err(db_error("read transfer return"))
}
fn parties_tx(c: &Connection, id: &str) -> Result<(String, String, i64), AppError> {
    c.query_row("SELECT sender_user_id,recipient_user_id,amount_units FROM market_wallet_transfers WHERE id=?1",params![id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(db_error("read return parties"))?.ok_or_else(||AppError::NotFound("transfer not found".into()))
}
#[cfg_attr(not(test), allow(dead_code))]
fn request_tx(
    c: &Connection,
    actor: &crate::models::AuthSession,
    id: &str,
    reason: &str,
    owner: bool,
    time: &str,
) -> Result<Return, AppError> {
    let (sender, recipient, _) = parties_tx(c, id)?;
    if actor.user_id != sender && !owner {
        return Err(AppError::NotFound("transfer not found".into()));
    }
    crate::market_finance_auth::require_live_session_tx(c, actor, time)?;
    if let Some(prior) = read_tx(c, id)? {
        return Ok(prior);
    }
    let reason = reason.trim();
    if reason.chars().count() < 8
        || reason.chars().count() > 1000
        || reason.chars().any(|c| c.is_control())
    {
        return Err(AppError::BadRequest(
            "a reason of 8–1000 characters is required".into(),
        ));
    }
    let request = Uuid::new_v4().to_string();
    c.execute("INSERT INTO market_transfer_returns(id,transfer_id,requested_by,reason,created_at) VALUES(?1,?2,?3,?4,?5)",params![request,id,actor.user_id,reason,time]).map_err(db_error("request transfer return"))?;
    c.execute("INSERT INTO market_financial_risk_events(id,kind,subject_id,object_id,details_json,created_at) VALUES(?1,'transfer_return_requested',?2,?3,?4,?5)",params![Uuid::new_v4().to_string(),actor.user_id,request,serde_json::json!({"ownerAssisted":owner,"proofId":actor.financial_proof_id}).to_string(),time]).map_err(db_error("audit return request"))?;
    crate::market_notifications::enqueue_user_notification_tx(
        c,
        "buyer",
        &recipient,
        "wallet_transfer_return_requested",
        "wallet_transfer",
        id,
        &format!("transfer_return_requested:{request}"),
        serde_json::json!({"transferId":id,"returnId":request,"reason":reason}),
        time,
    )?;
    read_tx(c, id)?.ok_or_else(|| AppError::Internal("missing return request".into()))
}
fn decide_tx(
    c: &Connection,
    actor: &crate::models::AuthSession,
    id: &str,
    version: i64,
    approve: bool,
    time: &str,
) -> Result<Return, AppError> {
    let (sender, recipient, amount) = parties_tx(c, id)?;
    if actor.user_id != recipient {
        return Err(AppError::NotFound("return not found".into()));
    }
    let r = read_tx(c, id)?.ok_or_else(|| AppError::NotFound("return not found".into()))?;
    // Committed replay precedes pauses, challenge expiry and balance checks.
    if r.status == if approve { "approved" } else { "declined" } {
        return Ok(r);
    }
    if r.version != version || r.status != "requested" {
        return Err(AppError::Conflict("return request changed; reload".into()));
    }
    crate::market_finance_auth::require_live_session_tx(c, actor, time)?;
    let mut journal = None;
    let mut consent = None;
    if approve {
        crate::market_reconcile::ensure_finance_scopes_safe_tx(
            c,
            &[
                crate::market_reconcile::FinanceScope::User(&sender),
                crate::market_reconcile::FinanceScope::User(&recipient),
            ],
        )?;
        crate::market_wallet::reconcile_wallet_tx(c, &sender)?;
        crate::market_wallet::reconcile_wallet_tx(c, &recipient)?;
        let available = wallet_snapshot_tx(c, &recipient)?
            .available_units
            .saturating_sub(
                crate::market_platform_contracts::prior_due_funding_units_tx(
                    c, &recipient, None, time,
                )?,
            );
        if available < amount {
            return Err(problem(
                "WALLET_RETURN_INSUFFICIENT",
                "return needs the full uncommitted original amount; add funds or decline the request",
            ));
        }
        crate::market_wallet::ensure_wallet_tx(c, &sender, time)?;
        wallet_snapshot_tx(c, &sender)?
            .posted_units
            .checked_add(amount)
            .ok_or_else(|| AppError::Conflict("sender balance overflow".into()))?;
        let fingerprint = format!("{id}:{recipient}:{sender}:{amount}");
        let evidence = Uuid::new_v4().to_string();
        c.execute("INSERT INTO market_transfer_return_consents(id,return_id,actor_user_id,counterparty_user_id,amount_units,session_reference,request_proof_id,fingerprint,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![evidence,r.id,recipient,sender,amount,actor.session_id,actor.financial_proof_id,fingerprint,time]).map_err(db_error("record recipient return consent"))?;
        let j = post_journal_tx(
            c,
            "wallet_transfer_return",
            &r.id,
            "return",
            &fingerprint,
            &[
                Posting {
                    kind: "wallet_available",
                    subject_id: &recipient,
                    signed_units: amount,
                },
                Posting {
                    kind: "wallet_available",
                    subject_id: &sender,
                    signed_units: -amount,
                },
            ],
            time,
        )?;
        if c.execute("UPDATE market_wallet_accounts SET posted_units=posted_units-?2,version=version+1,updated_at=?3 WHERE user_id=?1 AND asset_code='USD' AND posted_units-held_units>=?2",params![recipient,amount,time]).map_err(db_error("debit consented return"))?!=1{return Err(problem("WALLET_RETURN_INSUFFICIENT","available balance changed"))}
        c.execute("UPDATE market_wallet_accounts SET posted_units=posted_units+?2,version=version+1,updated_at=?3 WHERE user_id=?1 AND asset_code='USD'",params![sender,amount,time]).map_err(db_error("credit transfer return"))?;
        journal = Some(j.id);
        consent = Some(evidence);
    }
    if c.execute("UPDATE market_transfer_returns SET status=?3,version=version+1,authorized_by=?4,authorization_id=?5,journal_id=?6,resolved_at=?7 WHERE id=?1 AND version=?2 AND status='requested'",params![r.id,version,if approve{"approved"}else{"declined"},actor.user_id,consent,journal,time]).map_err(db_error("finish transfer return"))?!=1{return Err(AppError::Conflict("return request changed".into()))}
    for user in [&sender, &recipient] {
        crate::market_notifications::enqueue_user_notification_tx(
            c,
            "buyer",
            user,
            "wallet_transfer_return_resolved",
            "wallet_transfer",
            id,
            &format!("transfer_return_resolved:{}:{user}", r.id),
            serde_json::json!({"transferId":id,"approved":approve,"amountUnits":amount.to_string()}),
            time,
        )?;
    }
    if approve {
        crate::market_wallet::reconcile_wallet_tx(c, &sender)?;
        crate::market_wallet::reconcile_wallet_tx(c, &recipient)?;
    }
    read_tx(c, id)?.ok_or_else(|| AppError::Internal("missing return result".into()))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DecideReturn {
    expected_version: i64,
}
pub(super) fn router() -> Router<ServerState> {
    Router::new()
        .route(
            "/v1/market-wallet/transfers/:id/return",
            get(view).post(request_retired),
        )
        .route("/v1/market-wallet/transfer-returns/awaiting", get(awaiting))
        .route(
            "/v1/market-wallet/transfers/:id/return/approve",
            post(approve),
        )
        .route(
            "/v1/market-wallet/transfers/:id/return/decline",
            post(decline),
        )
        .route(
            "/v1/admin/market-finance/transfers/:id/return-request",
            post(request_retired),
        )
}
async fn request_retired() -> AppError {
    AppError::Gone(
        "Transfer return requests were retired. Withdraw a transfer before it credits, or ask the recipient to send it back.".into(),
    )
}
/// Earlier return requests that still wait for this user, as recipient, to decide.
async fn awaiting(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let a = session(&state, &headers).await?;
    let c = state.store.conn.lock().await;
    let mut stmt = c
        .prepare(
            "SELECT r.id,r.transfer_id,r.reason,r.version,r.created_at,t.sender_email,t.amount_units
             FROM market_transfer_returns r JOIN market_wallet_transfers t ON t.id=r.transfer_id
             WHERE t.recipient_user_id=?1 AND r.status='requested' ORDER BY r.created_at LIMIT 50",
        )
        .map_err(db_error("prepare awaiting returns"))?;
    let items = stmt
        .query_map(params![a.user_id], |r| {
            Ok(serde_json::json!({
                "id": r.get::<_, String>(0)?,
                "transferId": r.get::<_, String>(1)?,
                "reason": r.get::<_, String>(2)?,
                "version": r.get::<_, i64>(3)?,
                "createdAt": r.get::<_, String>(4)?,
                "senderEmail": r.get::<_, String>(5)?,
                "amountUnits": r.get::<_, i64>(6)?.to_string(),
            }))
        })
        .map_err(db_error("query awaiting returns"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read awaiting returns"))?;
    Ok((no_store(), Json(serde_json::json!({ "items": items }))))
}
async fn view(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let a = session(&state, &headers).await?;
    let c = state.store.conn.lock().await;
    let (s, r, _) = parties_tx(&c, &id)?;
    if a.user_id != s && a.user_id != r {
        return Err(AppError::NotFound("transfer not found".into()));
    }
    Ok((no_store(), Json(read_tx(&c, &id)?)))
}
async fn do_decide(
    state: ServerState,
    headers: HeaderMap,
    id: String,
    input: DecideReturn,
    approve: bool,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let a = crate::market_finance_auth::write_session(&state, &headers).await?;
    let c = state.store.conn.lock().await;
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin recipient return decision"))?;
    let r = decide_tx(&tx, &a, &id, input.expected_version, approve, &now())?;
    tx.commit().map_err(db_error("commit transfer return"))?;
    Ok((no_store(), Json(r)))
}
async fn approve(
    State(s): State<ServerState>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(i): Json<DecideReturn>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    do_decide(s, h, id, i, true).await
}
async fn decline(
    State(s): State<ServerState>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(i): Json<DecideReturn>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    do_decide(s, h, id, i, false).await
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn return_requires_recipient_consent_and_creates_one_compensating_journal() {
        let (c, a) = super::super::tests::fixture();
        let q = super::super::tests::quoted(&c, 8_640_000, "return-quote");
        let t = super::super::tests::confirm_released(
            &c,
            &a,
            &a.user_id,
            "return-test",
            &q,
            "2026-10-01T00:01:00Z",
        )
        .unwrap();
        let r = request_tx(
            &c,
            &a,
            &t.id,
            "Wrong recipient selected",
            false,
            "2026-10-01T00:01:01Z",
        )
        .unwrap();
        assert!(decide_tx(&c, &a, &t.id, r.version, true, "2026-10-01T00:01:02Z").is_err());
        let recipient = parties_tx(&c, &t.id).unwrap().1;
        let b = crate::market_finance_auth::test_session(&c, &recipient);
        let done = decide_tx(&c, &b, &t.id, r.version, true, "2026-10-01T00:01:02Z").unwrap();
        assert_eq!(done.status, "approved");
        assert_eq!(
            decide_tx(&c, &b, &t.id, r.version, true, "2026-10-01T00:01:03Z")
                .unwrap()
                .journal_id,
            done.journal_id
        );
        let valid: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM market_verified_transfer_return_evidence WHERE id=?1)",
                params![done.id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(valid);
    }
    #[test]
    fn return_outbox_failure_rolls_back_consent_balances_and_journal() {
        let (c, a) = super::super::tests::fixture();
        let q = super::super::tests::quoted(&c, 8_640_000, "rollback-return-quote");
        let t = super::super::tests::confirm_released(
            &c,
            &a,
            &a.user_id,
            "rollback-return",
            &q,
            "2026-10-01T00:01:00Z",
        )
        .unwrap();
        let r = request_tx(
            &c,
            &a,
            &t.id,
            "Wrong recipient selected",
            false,
            "2026-10-01T00:01:01Z",
        )
        .unwrap();
        let b = crate::market_finance_auth::test_session(&c, "recipient");
        let before = wallet_snapshot_tx(&c, "recipient").unwrap().posted_units;
        c.execute_batch("CREATE TRIGGER reject_return_notice BEFORE INSERT ON market_notification_outbox WHEN NEW.event_kind='wallet_transfer_return_resolved' BEGIN SELECT RAISE(ABORT,'outbox unavailable'); END;").unwrap();
        {
            let tx = c
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            assert!(decide_tx(&tx, &b, &t.id, r.version, true, "2026-10-01T00:01:02Z").is_err());
        }
        assert_eq!(
            wallet_snapshot_tx(&c, "recipient").unwrap().posted_units,
            before
        );
        assert_eq!(read_tx(&c, &t.id).unwrap().unwrap().status, "requested");
        for table in [
            "market_transfer_return_consents",
            "market_verified_transfer_return_evidence",
        ] {
            assert_eq!(
                c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        assert_eq!(c.query_row("SELECT COUNT(*) FROM market_journal_transactions WHERE source_kind='wallet_transfer_return'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        c.execute_batch("DROP TRIGGER reject_return_notice;")
            .unwrap();
        {
            let tx = c
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            decide_tx(&tx, &b, &t.id, r.version, true, "2026-10-01T00:01:03Z").unwrap();
            tx.commit().unwrap();
        }
    }
    #[test]
    fn a_spent_recipient_balance_cannot_be_forced_negative_by_a_return() {
        let (c, a) = super::super::tests::fixture();
        let q = super::super::tests::quoted(&c, 8_640_000, "spent-return-quote");
        let t = super::super::tests::confirm_released(
            &c,
            &a,
            &a.user_id,
            "spent-return",
            &q,
            "2026-10-01T00:01:00Z",
        )
        .unwrap();
        let r = request_tx(
            &c,
            &a,
            &t.id,
            "Wrong recipient selected",
            false,
            "2026-10-01T00:01:01Z",
        )
        .unwrap();
        let b = crate::market_finance_auth::test_session(&c, "recipient");
        super::super::verified_transfer_for_test(
            &c,
            "recipient",
            "sender",
            8_640_000,
            "2026-10-01T00:01:02Z",
        )
        .unwrap();
        {
            let tx = c
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            assert_eq!(
                decide_tx(&tx, &b, &t.id, r.version, true, "2026-10-01T00:01:03Z")
                    .unwrap_err()
                    .code(),
                Some("WALLET_RETURN_INSUFFICIENT")
            );
        }
        assert_eq!(wallet_snapshot_tx(&c, "recipient").unwrap().posted_units, 0);
        assert_eq!(read_tx(&c, &t.id).unwrap().unwrap().status, "requested");
    }
    #[test]
    fn independent_connections_approve_one_compensating_return() {
        let dir = std::env::temp_dir().join(format!("router-return-race-{}", Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("money.sqlite");
        let c = crate::test_support::database_at(&path).unwrap();
        let a = crate::market_finance_auth::test_session(&c, "race-sender");
        let b = crate::market_finance_auth::test_session(&c, "race-recipient");
        crate::market_wallet::credit_verified_topup_tx(
            &c,
            &a.user_id,
            "return-race-seed",
            100,
            "race-receipt",
            "2026-10-01T00:00:00Z",
        )
        .unwrap();
        super::super::verified_transfer_for_test(
            &c,
            &a.user_id,
            &b.user_id,
            8_640_000,
            "2026-10-01T00:01:00Z",
        )
        .unwrap();
        let id: String = c
            .query_row("SELECT id FROM market_wallet_transfers", [], |r| r.get(0))
            .unwrap();
        let r = request_tx(
            &c,
            &a,
            &id,
            "Wrong recipient selected",
            false,
            "2026-10-01T00:01:01Z",
        )
        .unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let tasks = (0..2)
            .map(|_| {
                let db = Connection::open(&path).unwrap();
                let gate = barrier.clone();
                let b = b.clone();
                let id = id.clone();
                let version = r.version;
                std::thread::spawn(move || {
                    gate.wait();
                    let Ok(tx) = db.transaction_with_behavior(TransactionBehavior::Immediate)
                    else {
                        return false;
                    };
                    if decide_tx(&tx, &b, &id, version, true, "2026-10-01T00:01:02Z").is_err() {
                        return false;
                    }
                    tx.commit().unwrap();
                    true
                })
            })
            .collect::<Vec<_>>();
        let successes = tasks
            .into_iter()
            .map(|t| usize::from(t.join().unwrap()))
            .sum::<usize>();
        assert!(successes >= 1);
        assert_eq!(
            c.query_row(
                "SELECT COUNT(*) FROM market_verified_transfer_return_evidence",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(wallet_snapshot_tx(&c, &b.user_id).unwrap().posted_units, 0);
        assert_eq!(
            wallet_snapshot_tx(&c, &a.user_id).unwrap().posted_units,
            8_640_000
        );
        drop(c);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
