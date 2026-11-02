use super::*;

/// Verify the new projections against sealed money and original-identity
/// evidence. Used by credit and finance audit; absence is never proof of zero.
pub(crate) fn evidence_valid(c: &Connection, id: &str) -> Result<bool, AppError> {
    let t = load(c, id)?;
    if t.revoked_at.is_none() || t.status == "finance_pending" {
        return Ok(true);
    }
    let receipt: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM share_termination_receipts WHERE termination_id=?1)",
            params![id],
            |r| r.get(0),
        )
        .map_err(db_error("audit Share cleanup evidence"))?;
    if !receipt {
        return Ok(false);
    }
    if t.period_id.is_none() {
        return Ok(t.compensation_units == 0 && t.returned_units == 0);
    }
    let allocated = t
        .quota_earned_units
        .checked_add(t.returned_units)
        .and_then(|v| v.checked_add(t.compensation_units))
        .and_then(|v| v.checked_add(t.held_units));
    // remaining_units is the opening liability minus quantity-based delivery.
    if allocated != Some(t.remaining_units + t.quota_earned_units) {
        return Ok(false);
    }
    let valid:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM market_platform_periods p WHERE p.id=?1 AND p.contract_id=?2
        AND p.fulfilled_units=?3 AND p.refunded_units=?4 AND p.termination_compensation_units=?5 AND p.share_quota_units=?6
        AND COALESCE((SELECT SUM(signed_units) FROM market_journal_postings WHERE account_kind='unfulfilled' AND subject_id=p.id),0)=-?7)",
        params![t.period_id,t.contract_id,t.time_earned_units,t.prior_refunded_units+t.returned_units,t.compensation_units,t.quota_earned_units,t.held_units],|r|r.get(0)).map_err(db_error("audit Share final allocation"))?;
    if !valid {
        return Ok(false);
    }
    let raw: String = c
        .query_row(
            "SELECT payload_json FROM share_termination_receipts WHERE termination_id=?1",
            params![id],
            |r| r.get(0),
        )
        .map_err(db_error("read audited Share receipt"))?;
    let r: TerminationReceipt =
        serde_json::from_str(&raw).map_err(|e| AppError::Internal(e.to_string()))?;
    let mut opening = t.clone();
    opening.remaining_units += t.quota_earned_units;
    if settlement::validate_usage(&opening, &r)? != t.quota_earned_units {
        return Ok(false);
    }
    let allocations:bool=c.query_row("SELECT
        COALESCE((SELECT SUM(p.signed_units) FROM market_journal_transactions j JOIN market_journal_postings p ON p.journal_id=j.id
            WHERE j.source_kind='share_termination' AND j.source_id=?1 AND p.account_kind='wallet_available' AND p.subject_id=?2),0)=-?3
        AND COALESCE((SELECT SUM(amount_units) FROM market_delivery_slices WHERE source_kind='share_quota' AND source_id=?1 AND contract_id=?4),0)=?5",params![id,t.buyer_user_id,t.returned_units,t.contract_id,t.quota_earned_units],|r|r.get(0)).map_err(db_error("verify Share return and quantity evidence"))?;
    if !allocations {
        return Ok(false);
    }
    let award:Option<(i64,i64,String,Option<String>)>=c.query_row("SELECT amount_units,credited_units,journal_id,credit_journal_id FROM share_termination_income WHERE termination_id=?1 AND provider_user_id=?2 AND kind='share_termination'",params![id,t.provider_user_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(db_error("audit Share compensation"))?;
    let Some((gross, credited, journal, credit)) = award else {
        return Ok(t.compensation_units == 0);
    };
    if gross != t.compensation_units || credited < 0 || credited > gross {
        return Ok(false);
    }
    let valid:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM market_journal_transactions j JOIN market_journal_seals s ON s.journal_id=j.id
        WHERE j.id=?1 AND j.source_kind='share_termination' AND j.source_id=?2 AND j.action='share_termination' AND s.posting_count=2
        AND COALESCE((SELECT SUM(signed_units) FROM market_journal_postings WHERE journal_id=j.id AND account_kind='unfulfilled' AND subject_id=?3),0)=?4
        AND COALESCE((SELECT SUM(signed_units) FROM market_journal_postings WHERE journal_id=j.id AND account_kind='provider_pending' AND subject_id=?5),0)=-?4)",params![journal,id,t.period_id,gross,t.provider_user_id],|r|r.get(0)).map_err(db_error("verify sealed Share compensation"))?;
    if !valid {
        return Ok(false);
    }
    if credited == 0 {
        return Ok(credit.is_none());
    }
    c.query_row("SELECT EXISTS(SELECT 1 FROM market_journal_transactions j JOIN market_journal_seals s ON s.journal_id=j.id
        WHERE j.id=?1 AND j.source_kind='share_compensation' AND j.source_id=?2 AND j.action='automatic_credit' AND s.posting_count=2
        AND COALESCE((SELECT SUM(signed_units) FROM market_journal_postings WHERE journal_id=j.id AND account_kind='provider_pending' AND subject_id=?3),0)=?4
        AND COALESCE((SELECT SUM(signed_units) FROM market_journal_postings WHERE journal_id=j.id AND account_kind='wallet_available' AND subject_id=?3),0)=-?4)",params![credit,format!("{id}:share_termination"),t.provider_user_id,credited],|r|r.get(0)).map_err(db_error("verify Share automatic credit"))
}

/// Fair, bounded scan; one broken case never prevents another deadline or
/// earning credit. The scan cursor commits even when an item is deferred.
pub(crate) fn maintain_isolated(c: &Connection, at: &str, limit: i64) -> Result<(), AppError> {
    time(at)?;
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin Share termination maintenance"))?;
    let cursor: String = tx
        .query_row(
            "SELECT share_termination_cursor FROM market_finance_control WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(db_error("read Share maintenance cursor"))?;
    let ids = {
        let mut q=tx.prepare("SELECT id FROM share_market_terminations WHERE id>?1 AND status!='completed' ORDER BY id LIMIT ?2").map_err(db_error("scan Share terminations"))?;
        q.query_map(params![cursor, limit.clamp(1, 200)], |r| {
            r.get::<_, String>(0)
        })
        .map_err(db_error("query Share maintenance"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read Share maintenance"))?
    };
    for id in &ids {
        tx.execute_batch("SAVEPOINT share_case_maintenance")
            .map_err(db_error("begin Share maintenance item"))?;
        let result = (|| {
            let mut t = load(&tx, id)?;
            if t.status == "finance_pending" {
                finish_tx(&tx, &t.subscription_id, at)?;
                t = load(&tx, id)?;
            }
            if deadline(&t)?.is_some_and(|d| time(at).is_ok_and(|n| n >= d))
                && t.status != "finance_pending"
            {
                act_tx(
                    &tx,
                    id,
                    "",
                    false,
                    "timeout",
                    &Action {
                        expected_version: t.version,
                        award_units: None,
                        note: String::new(),
                    },
                    at,
                )?;
            }
            Ok::<_, AppError>(())
        })();
        if let Err(error) = result {
            tx.execute_batch("ROLLBACK TO SAVEPOINT share_case_maintenance")
                .map_err(db_error("defer Share case"))?;
            tracing::warn!(termination_id=%id,%error,"Share maintenance deferred");
        }
        tx.execute_batch("RELEASE SAVEPOINT share_case_maintenance")
            .map_err(db_error("commit Share maintenance item"))?;
    }
    tx.execute(
        "UPDATE market_finance_control SET share_termination_cursor=?1 WHERE id=1",
        params![ids.last().map(String::as_str).unwrap_or("")],
    )
    .map_err(db_error("advance Share maintenance cursor"))?;
    let cursor: String = tx
        .query_row(
            "SELECT share_income_cursor FROM market_finance_control WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(db_error("read Share income cursor"))?;
    let ids = {
        let mut q=tx.prepare("SELECT id FROM share_termination_income WHERE id>?1 AND credited_units=0 AND julianday(unlock_at)<=julianday(?2) ORDER BY id LIMIT ?3").map_err(db_error("scan Share income"))?;
        q.query_map(params![cursor, at, limit.clamp(1, 200)], |r| {
            r.get::<_, String>(0)
        })
        .map_err(db_error("query Share income"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read Share income"))?
    };
    for id in &ids {
        tx.execute_batch("SAVEPOINT share_income_credit")
            .map_err(db_error("begin Share credit"))?;
        let result = (|| {
            let (termination,provider,amount):(String,String,i64)=tx.query_row("SELECT termination_id,provider_user_id,amount_units-commission_units FROM share_termination_income WHERE id=?1",params![id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(db_error("read Share income award"))?;
            let t = load(&tx, &termination)?;
            if t.status != "completed" || t.revoked_at.is_none() || provider != t.provider_user_id {
                return Err(AppError::Conflict(
                    "Share compensation is not ready for credit".into(),
                ));
            }
            if !crate::market_provider_finance::can_manage_earnings_tx(&tx, &provider)? {
                return Ok(());
            }
            if !evidence_valid(&tx, &termination)? {
                return Err(AppError::Internal(
                    "Share termination evidence requires reconciliation".into(),
                ));
            }
            crate::market_reconcile::ensure_finance_scopes_safe_tx(
                &tx,
                &[
                    crate::market_reconcile::FinanceScope::Provider(&provider),
                    crate::market_reconcile::FinanceScope::User(&provider),
                    crate::market_reconcile::FinanceScope::Contract(
                        t.contract_id.as_deref().unwrap_or(""),
                    ),
                ],
            )?;
            crate::market_wallet::reconcile_wallet_tx(&tx, &provider)?;
            let j = credit_wallet_from_liability_tx(
                &tx,
                &provider,
                amount,
                "provider_pending",
                &provider,
                "share_compensation",
                id,
                "automatic_credit",
                at,
            )?;
            if !j.inserted {
                return Err(AppError::Internal(
                    "Share compensation credit projection drift".into(),
                ));
            }
            tx.execute("UPDATE share_termination_income SET credited_units=?2,credit_journal_id=?3 WHERE id=?1 AND credited_units=0",params![id,amount,j.id]).map_err(db_error("record Share credit"))?;
            event(
                &tx,
                &t,
                None,
                "automatic_credit",
                json!({"amountUnits":amount.to_string()}),
                Some(&j.id),
                at,
            )
        })();
        if let Err(error) = result {
            tx.execute_batch("ROLLBACK TO SAVEPOINT share_income_credit")
                .map_err(db_error("defer Share credit"))?;
            tracing::warn!(income_id=%id,%error,"Share earning credit deferred");
        }
        tx.execute_batch("RELEASE SAVEPOINT share_income_credit")
            .map_err(db_error("commit Share credit"))?;
    }
    tx.execute(
        "UPDATE market_finance_control SET share_income_cursor=?1 WHERE id=1",
        params![ids.last().map(String::as_str).unwrap_or("")],
    )
    .map_err(db_error("advance Share income cursor"))?;
    tx.commit().map_err(db_error("commit Share maintenance"))
}
