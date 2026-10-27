//! One-time Share trial eligibility and acknowledgement-fenced conversion.
use crate::market_wallet_policy::db_error;
use crate::{
    db::{Connection, OptionalExtension, params},
    error::AppError,
    models::{ShareMarketEntitlement, ShareMarketPhase},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrialQuote {
    pub version: u16,
    pub policy_version: i64,
    pub hours_inherited: bool,
    pub tokens_inherited: bool,
    pub token_allowance: u64,
    pub token_budget: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrialView {
    pub phase: String,
    pub token_allowance: u64,
    pub tokens_used: u64,
    pub tokens_remaining: Option<u64>,
    pub end_reason: Option<String>,
}

pub(crate) fn view_tx(
    conn: &Connection,
    subscription: &str,
) -> Result<Option<TrialView>, AppError> {
    conn.query_row("SELECT trial.phase, trial.token_allowance, trial.tokens_used, trial.token_budget, trial.end_reason,
        (SELECT COALESCE(SUM(previous.tokens_used),0) FROM market_share_trial_contracts previous WHERE previous.scope_id=trial.scope_id)
        FROM market_share_trial_contracts trial JOIN market_platform_contracts c ON c.id = trial.contract_id
        WHERE c.product_kind = 'share' AND c.product_ref = ?1", params![subscription], |r| {
        let allowance: u64 = r.get(1)?; let used: u64 = r.get(2)?; let budget: u64 = r.get(3)?; let scope_used: u64 = r.get(5)?;
        Ok(TrialView { phase: r.get(0)?, token_allowance: allowance, tokens_used: scope_used, tokens_remaining: (allowance > 0).then_some(budget.saturating_sub(used).min(allowance.saturating_sub(scope_used))), end_reason: r.get(4)? })
    }).optional().map_err(db_error("read Share trial view"))
}

/// Consumption survives releases, seat changes and policy updates. Once either
/// threshold ends a scope, a larger future offer cannot revive its free trial.
pub(crate) fn remaining_tx(
    conn: &Connection,
    buyer: &str,
    provider: &str,
    share: &str,
    seconds: i64,
    tokens: u64,
) -> Result<(i64, u64), AppError> {
    let ended: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM market_share_trial_scopes WHERE buyer_user_id=?1 AND provider_user_id=?2 AND share_id=?3 AND ended_reason IS NOT NULL)", params![buyer, provider, share], |r| r.get(0)).map_err(db_error("read ended Share trial"))?;
    if ended || seconds == 0 {
        return Ok((0, 0));
    }
    // A pre-v9 trial that reached paid service has already spent its one-time
    // eligibility. A larger future default cannot reopen it.
    let already_paid: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM market_platform_contracts WHERE buyer_user_id=?1 AND provider_user_id=?2 AND product_kind='share' AND trial_scope_ref=?3 AND paid_started_at IS NOT NULL)",params![buyer,provider,share],|r|r.get(0)).map_err(db_error("read historical Share trial eligibility"))?;
    if already_paid {
        return Ok((0, 0));
    }
    let remaining = crate::market_platform_contracts::trial_seconds_remaining_tx(
        conn, buyer, provider, "share", share, seconds,
    )?;
    let used: u64 = conn.query_row("SELECT COALESCE(SUM(t.tokens_used),0) FROM market_share_trial_contracts t JOIN market_share_trial_scopes s ON s.id=t.scope_id WHERE s.buyer_user_id=?1 AND s.provider_user_id=?2 AND s.share_id=?3", params![buyer, provider, share], |r| r.get(0)).map_err(db_error("read consumed Share trial Tokens"))?;
    let budget = tokens.saturating_sub(used);
    Ok((
        if tokens > 0 && budget == 0 {
            0
        } else {
            remaining
        },
        budget,
    ))
}

pub(crate) fn prepare_tx(
    conn: &Connection,
    contract: &str,
    entitlement: &str,
    quote: &TrialQuote,
    now: &str,
) -> Result<(), AppError> {
    if quote.version != 1
        || quote.policy_version < 1
        || quote.token_allowance > crate::market_trial_policy::MAX_TOKENS
        || quote.token_budget > quote.token_allowance
    {
        return Err(AppError::Conflict(
            "invalid frozen Share trial terms; request a new quote".into(),
        ));
    }
    let (buyer, provider, share, remaining): (String,String,String,i64) = conn.query_row("SELECT buyer_user_id,provider_user_id,trial_scope_ref,trial_seconds_remaining FROM market_platform_contracts WHERE id=?1", params![contract], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(db_error("read Share trial contract"))?;
    if remaining > 0 {
        require_capability_tx(conn, &share)?;
    } else {
        let version: i64 = conn
            .query_row(
                "SELECT share_access_policy_version FROM shares WHERE share_id=?1",
                params![share],
                |r| r.get(0),
            )
            .map_err(db_error("read paid phase capability"))?;
        if version < i64::from(crate::models::SHARE_TRIAL_CONTRACT_VERSION) {
            return Ok(());
        }
    }
    let (current, budget) = remaining_tx(
        conn,
        &buyer,
        &provider,
        &share,
        crate::market_trial_policy::MAX_HOURS * 3600,
        quote.token_allowance,
    )?;
    // The time allowance was already fenced against actual historical usage by
    // prepare_contract_tx. Here only scope termination and Token drift matter.
    if remaining > 0 && (current == 0 || budget < quote.token_budget) {
        return Err(AppError::Conflict(
            "Share trial usage changed; request a new quote".into(),
        ));
    }
    let scope = uuid::Uuid::new_v4().to_string();
    conn.execute("INSERT OR IGNORE INTO market_share_trial_scopes(id,buyer_user_id,provider_user_id,share_id) VALUES(?1,?2,?3,?4)",params![scope,buyer,provider,share]).map_err(db_error("create Share trial scope"))?;
    conn.execute("INSERT INTO market_share_trial_contracts(contract_id,scope_id,entitlement_id,token_allowance,token_budget,created_at,updated_at) SELECT ?1,id,?5,?6,?7,?8,?8 FROM market_share_trial_scopes WHERE buyer_user_id=?2 AND provider_user_id=?3 AND share_id=?4", params![contract,buyer,provider,share,entitlement,quote.token_allowance,if remaining > 0 { quote.token_budget } else { 0 },now]).map_err(db_error("freeze Share trial budget"))?;
    if remaining == 0 {
        conn.execute(
            "UPDATE market_share_trial_contracts SET phase='paid' WHERE contract_id=?1",
            params![contract],
        )
        .map_err(db_error("freeze paid Share phase"))?;
    }
    Ok(())
}

pub(crate) fn require_capability_tx(conn: &Connection, share: &str) -> Result<(), AppError> {
    let version: i64 = conn
        .query_row(
            "SELECT share_access_policy_version FROM shares WHERE share_id=?1",
            params![share],
            |r| r.get(0),
        )
        .map_err(db_error("read Share trial capability"))?;
    if version < i64::from(crate::models::SHARE_TRIAL_CONTRACT_VERSION) {
        return Err(AppError::coded_conflict(
            "share_market_trial_upgrade_required",
            "upgrade cc-switch-server to support one-time market trials",
            serde_json::json!({"requiredVersion": crate::models::SHARE_TRIAL_CONTRACT_VERSION,"currentVersion":version}),
        ));
    }
    Ok(())
}

pub(crate) fn control_terms_tx(
    conn: &Connection,
    subscription: &str,
) -> Result<(Option<ShareMarketPhase>, Option<u64>), AppError> {
    let row: Option<(String,u64,u64)> = conn.query_row("SELECT t.phase,t.token_allowance,t.token_budget FROM market_share_trial_contracts t JOIN market_platform_contracts c ON c.id=t.contract_id WHERE c.product_kind='share' AND c.product_ref=?1",params![subscription],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(db_error("read Share phase control terms"))?;
    Ok(match row {
        Some((phase, allowance, budget)) if phase == "trial" => (
            Some(ShareMarketPhase::Trial),
            (allowance > 0).then_some(budget),
        ),
        Some(_) => (Some(ShareMarketPhase::Paid), None),
        None => (None, None),
    })
}

pub(crate) fn control_scope_tx(
    conn: &Connection,
    subscription: &str,
) -> Result<(Option<String>, Option<u64>), AppError> {
    conn.query_row("SELECT t.scope_id, CASE WHEN t.token_allowance>0 AND t.token_budget>0 AND t.phase='trial' THEN t.token_allowance ELSE NULL END FROM market_share_trial_contracts t JOIN market_platform_contracts c ON c.id=t.contract_id WHERE c.product_kind='share' AND c.product_ref=?1",params![subscription],|r|Ok((Some(r.get(0)?),r.get(1)?))).optional().map(|r|r.unwrap_or((None,None))).map_err(db_error("read Share trial scope terms"))
}

pub(crate) fn is_managed_tx(conn: &Connection, contract: &str) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM market_share_trial_contracts WHERE contract_id=?1)",
        params![contract],
        |r| r.get(0),
    )
    .map_err(db_error("read managed Share trial"))
}

pub(crate) fn confirmed_hold_tx(
    conn: &Connection,
    contract: &str,
) -> Result<Option<String>, AppError> {
    conn.query_row("SELECT conversion_hold_id FROM market_share_trial_contracts WHERE contract_id=?1 AND paid_applied_at IS NOT NULL AND phase='activating'",params![contract],|r| r.get(0)).optional().map(|v| v.flatten()).map_err(db_error("read confirmed Share conversion"))
}

pub(crate) fn request_conversion_tx(
    conn: &Connection,
    contract: &str,
    reason: &str,
    now: &str,
) -> Result<(), AppError> {
    let (phase,scope,product,buyer,price): (String,String,String,String,i64) = conn.query_row("SELECT t.phase,t.scope_id,c.product_ref,c.buyer_user_id,COALESCE(c.cycle_price_minor,c.daily_rate_minor) FROM market_share_trial_contracts t JOIN market_platform_contracts c ON c.id=t.contract_id WHERE c.id=?1",params![contract],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).map_err(db_error("read Share conversion"))?;
    if phase != "trial" {
        return Ok(());
    }
    conn.execute("UPDATE market_share_trial_scopes SET ended_reason=COALESCE(ended_reason,?2),ended_at=COALESCE(ended_at,?3) WHERE id=?1",params![scope,reason,now]).map_err(db_error("end Share trial eligibility"))?;
    conn.execute("UPDATE market_platform_contracts SET trial_seconds_remaining=0,updated_at=?2 WHERE id=?1 AND status='trialing'",params![contract,now]).map_err(db_error("end Share trial"))?;
    crate::market_reconcile::ensure_contract_finance_safe_tx(conn, contract)?;
    let units = crate::market_wallet::units_from_minor(price)?;
    let prior = crate::market_platform_contracts::prior_due_funding_units_tx(
        conn,
        &buyer,
        Some(contract),
        now,
    )?;
    if crate::market_wallet::wallet_snapshot_tx(conn, &buyer)?.available_units
        < prior
            .checked_add(units)
            .ok_or_else(|| AppError::Internal("Share trial funding overflow".into()))?
    {
        conn.execute("UPDATE market_share_trial_contracts SET phase='revoking',end_reason=?2,updated_at=?3 WHERE contract_id=?1",params![contract,reason,now]).map_err(db_error("mark unfunded Share trial"))?;
        crate::market_platform_contracts::mark_for_confirmed_cleanup_tx(conn, contract, now)?;
        return Ok(());
    }
    let hold = crate::market_wallet::reserve_wallet_tx(
        conn,
        &buyer,
        "platform_contract",
        contract,
        "activation",
        units,
        None,
        now,
    )?;
    conn.execute("UPDATE market_share_trial_contracts SET phase='activating',end_reason=?2,conversion_hold_id=?3,conversion_requested_at=?4,updated_at=?4 WHERE contract_id=?1 AND phase='trial'",params![contract,reason,hold,now]).map_err(db_error("prepare Share paid activation"))?;
    conn.execute(
        "UPDATE market_platform_contracts SET activation_hold_id=?2 WHERE id=?1",
        params![contract, hold],
    )
    .map_err(db_error("bind Share conversion hold"))?;
    crate::share_market::enqueue_trial_paid_activation_tx(conn, &product, now)?;
    Ok(())
}

/// Runs even for released entitlements: delayed trial completions must reduce
/// the shared lifetime budget before a new quotation is granted.
pub(crate) fn observe_share_tx(
    conn: &Connection,
    share: &str,
    states: &BTreeMap<String, ShareMarketEntitlement>,
    now: &str,
) -> Result<(), AppError> {
    for (id, state) in states {
        let row:Option<(String,u64,u64,String)>=conn.query_row("SELECT t.contract_id,t.usage_revision,t.tokens_used,c.buyer_user_id FROM market_share_trial_contracts t JOIN market_platform_contracts c ON c.id=t.contract_id JOIN share_market_subscriptions s ON s.id=c.product_ref WHERE t.entitlement_id=?1 AND s.share_id=?2 AND lower(s.renter_email)=lower(?3)",params![id,share,state.email],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(db_error("read Share trial watermark"))?;
        let Some((contract, revision, used, _)) = row else {
            continue;
        };
        let (scope,allowance,budget):(String,u64,u64)=conn.query_row("SELECT scope_id,token_allowance,token_budget FROM market_share_trial_contracts WHERE contract_id=?1",params![contract],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(db_error("read frozen Share trial identity"))?;
        let capped = allowance > 0 && budget > 0;
        if state.trial_scope_id != scope
            || state.trial_token_limit != capped.then_some(budget)
            || state.trial_token_allowance != capped.then_some(allowance)
        {
            return Err(AppError::Conflict(
                "Server trial scope or budget differs from frozen terms".into(),
            ));
        }
        if state.usage_revision < revision {
            continue;
        }
        if state.trial_tokens_used < used
            || (state.usage_revision == revision && state.trial_tokens_used != used)
        {
            return Err(AppError::Conflict(
                "Share trial usage watermark changed payload".into(),
            ));
        }
        conn.execute("UPDATE market_share_trial_contracts SET tokens_used=?2,usage_revision=?3,updated_at=?4 WHERE contract_id=?1",params![contract,state.trial_tokens_used,state.usage_revision,now]).map_err(db_error("persist Share trial usage"))?;
        if state.usage_uncertain {
            isolate_tx(conn, &contract, "usage_unavailable", now)?;
        }
    }
    Ok(())
}

fn isolate_tx(conn: &Connection, contract: &str, reason: &str, now: &str) -> Result<(), AppError> {
    let scope: String = conn
        .query_row(
            "SELECT scope_id FROM market_share_trial_contracts WHERE contract_id=?1",
            params![contract],
            |row| row.get(0),
        )
        .map_err(db_error("read unsafe Share trial scope"))?;
    let active = conn
        .prepare(
            "SELECT trial.contract_id FROM market_share_trial_contracts trial
         JOIN market_platform_contracts contract ON contract.id=trial.contract_id
         WHERE trial.scope_id=?1 AND contract.status IN ('trialing','active','suspended')",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![scope], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(db_error("read unsafe Share scope contracts"))?;
    conn.execute("UPDATE market_share_trial_scopes SET ended_reason=COALESCE(ended_reason,?2),ended_at=COALESCE(ended_at,?3) WHERE id=?1",params![scope,reason,now]).map_err(db_error("end unsafe Share trial"))?;
    conn.execute("UPDATE market_share_trial_contracts SET phase='revoking',end_reason=?3,updated_at=?4 WHERE scope_id=?2 AND (contract_id=?1 OR contract_id IN (SELECT id FROM market_platform_contracts WHERE status IN ('trialing','active','suspended')))",params![contract,scope,reason,now]).map_err(db_error("isolate unsafe Share activation"))?;
    // An old released entitlement may report unknown usage after a new rental
    // starts. Isolate the whole scope, not only the already-ended old contract.
    for id in active {
        crate::market_platform_contracts::mark_for_confirmed_cleanup_tx(conn, &id, now)?;
    }
    Ok(())
}

pub(crate) fn reconcile_tx(conn: &Connection, contract: &str, now: &str) -> Result<bool, AppError> {
    let row:Option<(String,u64,u64,i64,String,Option<String>)>=conn.query_row("SELECT t.phase,t.token_allowance,t.token_budget,c.trial_seconds_remaining,s.share_id,t.conversion_requested_at FROM market_share_trial_contracts t JOIN market_platform_contracts c ON c.id=t.contract_id JOIN share_market_subscriptions s ON s.id=c.product_ref WHERE c.id=?1",params![contract],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional().map_err(db_error("read Share trial reconciliation"))?;
    let Some((_, allowance, budget, _, share, requested)) = row else {
        return Ok(false);
    };
    let json: String = conn
        .query_row(
            "SELECT market_entitlements_json FROM shares WHERE share_id=?1",
            params![share],
            |r| r.get(0),
        )
        .map_err(db_error("read Share trial Server evidence"))?;
    let states: BTreeMap<String, ShareMarketEntitlement> = serde_json::from_str(&json)
        .map_err(|e| AppError::Internal(format!("invalid Share trial evidence: {e}")))?;
    observe_share_tx(conn, &share, &states, now)?;
    let mut phase: String = conn
        .query_row(
            "SELECT phase FROM market_share_trial_contracts WHERE contract_id=?1",
            params![contract],
            |r| r.get(0),
        )
        .map_err(db_error("read observed Share trial phase"))?;
    let (used,remaining):(u64,i64)=conn.query_row("SELECT t.tokens_used,c.trial_seconds_remaining FROM market_share_trial_contracts t JOIN market_platform_contracts c ON c.id=t.contract_id WHERE c.id=?1",params![contract],|r|Ok((r.get(0)?,r.get(1)?))).map_err(db_error("read Share trial thresholds"))?;
    let scope_used:u64=conn.query_row("SELECT COALESCE(SUM(tokens_used),0) FROM market_share_trial_contracts WHERE scope_id=(SELECT scope_id FROM market_share_trial_contracts WHERE contract_id=?1)",params![contract],|r|r.get(0)).map_err(db_error("read lifetime Share trial Tokens"))?;
    let tokens_exhausted = allowance > 0 && (used >= budget || scope_used >= allowance);
    if phase == "trial" && (remaining == 0 || tokens_exhausted) {
        request_conversion_tx(
            conn,
            contract,
            if tokens_exhausted { "tokens" } else { "time" },
            now,
        )?;
        phase = conn
            .query_row(
                "SELECT phase FROM market_share_trial_contracts WHERE contract_id=?1",
                params![contract],
                |r| r.get(0),
            )
            .map_err(db_error("read Share conversion phase"))?;
    }
    if phase == "activating" {
        let id: String = conn
            .query_row(
                "SELECT entitlement_id FROM market_share_trial_contracts WHERE contract_id=?1",
                params![contract],
                |r| r.get(0),
            )
            .map_err(db_error("read Share conversion entitlement"))?;
        if let Some(state) = states
            .get(&id)
            .filter(|s| s.active && s.phase == ShareMarketPhase::Paid)
        {
            if let Some(applied) = state.paid_started_at_ms {
                let Some(at) = chrono::DateTime::from_timestamp_millis(applied) else {
                    isolate_tx(conn, contract, "invalid_paid_receipt", now)?;
                    return Ok(true);
                };
                let requested:String=conn.query_row("SELECT conversion_requested_at FROM market_share_trial_contracts WHERE contract_id=?1",params![contract],|r|r.get(0)).map_err(db_error("read conversion fence"))?;
                // Server receipts use integer milliseconds; Router timestamps
                // can include nanoseconds. Compare the same precision so a
                // valid application in the request's own millisecond survives.
                if applied
                    < chrono::DateTime::parse_from_rfc3339(&requested)
                        .map_err(|e| AppError::Internal(e.to_string()))?
                        .timestamp_millis()
                    || applied
                        > chrono::DateTime::parse_from_rfc3339(now)
                            .map_err(|e| AppError::Internal(e.to_string()))?
                            .timestamp_millis()
                {
                    isolate_tx(conn, contract, "invalid_paid_receipt", now)?;
                    return Ok(true);
                }
                if let Err(error) =
                    crate::share_market::validate_trial_paid_activation_tx(conn, contract, at)
                {
                    if matches!(error, AppError::Conflict(_)) {
                        isolate_tx(conn, contract, "paid_policy_mismatch", now)?;
                        return Ok(true);
                    }
                    return Err(error);
                }
                conn.execute("UPDATE market_share_trial_contracts SET paid_applied_at=?2 WHERE contract_id=?1",params![contract,at.to_rfc3339()]).map_err(db_error("confirm Share paid activation"))?;
                crate::market_platform_contracts::start_paid_trial_tx(
                    conn,
                    contract,
                    &at.to_rfc3339(),
                    now,
                )?;
                conn.execute("UPDATE market_share_trial_contracts SET phase='paid',updated_at=?2 WHERE contract_id=?1",params![contract,now]).map_err(db_error("complete Share conversion"))?;
                return Ok(true);
            }
        }
        // Unknown acknowledgement never releases the reservation. Revoke first;
        // normal confirmed cleanup settles/releases the held obligation.
        if requested.as_deref().is_some_and(|at| {
            chrono::DateTime::parse_from_rfc3339(at)
                .ok()
                .zip(chrono::DateTime::parse_from_rfc3339(now).ok())
                .is_some_and(|(a, b)| (b - a).num_seconds() > 300)
        }) {
            isolate_tx(conn, contract, "activation_unconfirmed", now)?;
        }
    }
    Ok(phase == "activating" || phase == "revoking")
}
