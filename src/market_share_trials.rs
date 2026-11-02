//! One-time Share trial eligibility and acknowledgement-fenced conversion.
use crate::market_wallet_policy::db_error;
use crate::share_value_pricing::PICOS_PER_MICRO;
use crate::{
    db::{Connection, OptionalExtension, params},
    error::AppError,
    models::{ShareMarketEntitlement, ShareMarketPhase},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Frozen one-time trial terms of a quote. A Token seat freezes a Token
/// allowance/budget; a USD-equivalent seat freezes a USD allowance/budget
/// (canonical decimal micros, "0" = time-only) and keeps both Token fields 0.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrialQuote {
    pub version: u16,
    pub policy_version: i64,
    pub hours_inherited: bool,
    pub tokens_inherited: bool,
    pub token_allowance: u64,
    pub token_budget: u64,
    /// Present exactly for USD-equivalent seats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_allowance_usd_micros: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_budget_usd_micros: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub value_inherited: bool,
    /// The buyer's trial on this Share already started in the other unit:
    /// no budget applies, only the remaining trial time.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unit_locked: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrialView {
    pub phase: String,
    pub token_allowance: u64,
    pub tokens_used: u64,
    pub tokens_remaining: Option<u64>,
    pub end_reason: Option<String>,
    /// USD-equivalent trials only, decimal micros. Used rounds up and
    /// remaining rounds down so a fraction never favours the buyer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_allowance_usd_micros: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_used_usd_micros: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_remaining_usd_micros: Option<String>,
}

/// The unit a one-time trial budget is denominated in; it follows the seat's
/// quota and the first trial contract fixes it for the whole scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrialUnit {
    Tokens,
    Usd,
}

impl TrialUnit {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Tokens => "tokens",
            Self::Usd => "usd",
        }
    }
}

/// The trial budget a seat offers before scope consumption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrialAllowance {
    Tokens(u64),
    UsdMicros(i128),
}

impl TrialAllowance {
    fn unit(self) -> TrialUnit {
        match self {
            Self::Tokens(_) => TrialUnit::Tokens,
            Self::UsdMicros(_) => TrialUnit::Usd,
        }
    }
}

/// What remains of a buyer's one-time trial on one Share, in the seat's unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TrialRemaining {
    pub seconds: i64,
    pub token_budget: u64,
    pub value_budget_micros: i128,
    /// The scope's trial started in the other unit: time-only.
    pub unit_locked: bool,
}

impl TrialRemaining {
    const NONE: Self = Self {
        seconds: 0,
        token_budget: 0,
        value_budget_micros: 0,
        unit_locked: false,
    };
}

fn stored_decimal(raw: &str, what: &str) -> Result<i128, AppError> {
    crate::share_value_pricing::unsigned_decimal(raw)
        .map_err(|_| AppError::Internal(format!("stored Share trial {what} is invalid")))
}

/// Used picos rounded up to whole micros: the buyer never gains a fraction.
pub(crate) fn picos_to_micros_ceil(picos: i128) -> i128 {
    picos.div_euclid(PICOS_PER_MICRO) + i128::from(picos.rem_euclid(PICOS_PER_MICRO) != 0)
}

/// Remaining USD budget in whole micros after `used_picos`, never negative.
pub(crate) fn remaining_value_micros(allowance_micros: i128, used_picos: i128) -> i128 {
    allowance_micros
        .saturating_sub(picos_to_micros_ceil(used_picos))
        .max(0)
}

fn micros_to_picos(micros: i128) -> Result<i128, AppError> {
    micros
        .checked_mul(PICOS_PER_MICRO)
        .ok_or_else(|| AppError::Internal("Share trial USD budget overflow".into()))
}

/// Application-level sum of a scope's USD trial consumption (never SQL SUM
/// over TEXT).
fn scope_value_used_picos_tx(conn: &Connection, scope: &str) -> Result<i128, AppError> {
    let used = conn
        .prepare("SELECT value_used_picos FROM market_share_trial_contracts WHERE scope_id=?1")
        .and_then(|mut statement| {
            statement
                .query_map(params![scope], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(db_error("read consumed Share trial USD"))?;
    used.iter().try_fold(0_i128, |total, raw| {
        total
            .checked_add(stored_decimal(raw, "USD usage")?)
            .ok_or_else(|| AppError::Internal("Share trial USD usage overflow".into()))
    })
}

fn scope_tokens_used_tx(conn: &Connection, scope: &str) -> Result<u64, AppError> {
    conn.query_row(
        "SELECT COALESCE(SUM(tokens_used),0) FROM market_share_trial_contracts WHERE scope_id=?1",
        params![scope],
        |r| r.get(0),
    )
    .map_err(db_error("read consumed Share trial Tokens"))
}

pub(crate) fn view_tx(
    conn: &Connection,
    subscription: &str,
) -> Result<Option<TrialView>, AppError> {
    let row: Option<(String, u64, u64, u64, Option<String>, u64, String, Option<String>, Option<String>, String)> = conn.query_row("SELECT trial.phase, trial.token_allowance, trial.tokens_used, trial.token_budget, trial.end_reason,
        (SELECT COALESCE(SUM(previous.tokens_used),0) FROM market_share_trial_contracts previous WHERE previous.scope_id=trial.scope_id),
        trial.scope_id, trial.value_allowance_usd_micros, trial.value_budget_usd_micros, trial.value_used_picos
        FROM market_share_trial_contracts trial JOIN market_platform_contracts c ON c.id = trial.contract_id
        WHERE c.product_kind = 'share' AND c.product_ref = ?1", params![subscription], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?))
    }).optional().map_err(db_error("read Share trial view"))?;
    let Some((
        phase,
        allowance,
        used,
        budget,
        end_reason,
        scope_used,
        scope,
        value_allowance,
        value_budget,
        value_used,
    )) = row
    else {
        return Ok(None);
    };
    let mut view = TrialView {
        phase,
        token_allowance: allowance,
        tokens_used: scope_used,
        tokens_remaining: (allowance > 0).then_some(
            budget
                .saturating_sub(used)
                .min(allowance.saturating_sub(scope_used)),
        ),
        end_reason,
        value_allowance_usd_micros: None,
        value_used_usd_micros: None,
        value_remaining_usd_micros: None,
    };
    if let Some(raw_allowance) = value_allowance {
        let allowance = stored_decimal(&raw_allowance, "USD allowance")?;
        let scope_used = scope_value_used_picos_tx(conn, &scope)?;
        view.value_used_usd_micros = Some(picos_to_micros_ceil(scope_used).to_string());
        if allowance > 0 {
            let budget = stored_decimal(value_budget.as_deref().unwrap_or("0"), "USD budget")?;
            let used = stored_decimal(&value_used, "USD usage")?;
            let remaining = micros_to_picos(budget)?
                .saturating_sub(used)
                .min(micros_to_picos(allowance)?.saturating_sub(scope_used))
                .max(0);
            view.value_remaining_usd_micros =
                Some(remaining.div_euclid(PICOS_PER_MICRO).to_string());
        }
        view.value_allowance_usd_micros = Some(raw_allowance);
    }
    Ok(Some(view))
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
    let remaining = remaining_in_unit_tx(
        conn,
        buyer,
        provider,
        share,
        seconds,
        TrialAllowance::Tokens(tokens),
    )?;
    Ok((remaining.seconds, remaining.token_budget))
}

/// Remaining trial in the seat's unit. The scope's first trial contract fixes
/// its unit: a seat in the other unit gets only the remaining time (no second
/// budget), and the locked unit's exhaustion still ends the trial.
pub(crate) fn remaining_in_unit_tx(
    conn: &Connection,
    buyer: &str,
    provider: &str,
    share: &str,
    seconds: i64,
    allowance: TrialAllowance,
) -> Result<TrialRemaining, AppError> {
    let scope: Option<(String, Option<String>, bool)> = conn.query_row("SELECT id, unit, ended_reason IS NOT NULL FROM market_share_trial_scopes WHERE buyer_user_id=?1 AND provider_user_id=?2 AND share_id=?3", params![buyer, provider, share], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional().map_err(db_error("read ended Share trial"))?;
    if scope.as_ref().is_some_and(|(_, _, ended)| *ended) || seconds == 0 {
        return Ok(TrialRemaining::NONE);
    }
    // A pre-v9 trial that reached paid service has already spent its one-time
    // eligibility. A larger future default cannot reopen it.
    let already_paid: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM market_platform_contracts WHERE buyer_user_id=?1 AND provider_user_id=?2 AND product_kind='share' AND trial_scope_ref=?3 AND paid_started_at IS NOT NULL)",params![buyer,provider,share],|r|r.get(0)).map_err(db_error("read historical Share trial eligibility"))?;
    if already_paid {
        return Ok(TrialRemaining::NONE);
    }
    let remaining = crate::market_platform_contracts::trial_seconds_remaining_tx(
        conn, buyer, provider, "share", share, seconds,
    )?;
    let Some((scope, unit, _)) = scope else {
        let (token_budget, value_budget_micros) = match allowance {
            TrialAllowance::Tokens(tokens) => (tokens, 0),
            TrialAllowance::UsdMicros(micros) => (0, micros.max(0)),
        };
        return Ok(TrialRemaining {
            seconds: remaining,
            token_budget,
            value_budget_micros,
            unit_locked: false,
        });
    };
    if let Some(unit) = unit.filter(|unit| unit != allowance.unit().as_str()) {
        let exhausted = locked_budget_exhausted_tx(conn, &scope, &unit)?;
        return Ok(TrialRemaining {
            seconds: if exhausted { 0 } else { remaining },
            unit_locked: true,
            ..TrialRemaining::NONE
        });
    }
    Ok(match allowance {
        TrialAllowance::Tokens(tokens) => {
            let budget = tokens.saturating_sub(scope_tokens_used_tx(conn, &scope)?);
            TrialRemaining {
                seconds: if tokens > 0 && budget == 0 {
                    0
                } else {
                    remaining
                },
                token_budget: budget,
                ..TrialRemaining::NONE
            }
        }
        TrialAllowance::UsdMicros(micros) => {
            let budget = remaining_value_micros(micros, scope_value_used_picos_tx(conn, &scope)?);
            TrialRemaining {
                seconds: if micros > 0 && budget == 0 {
                    0
                } else {
                    remaining
                },
                value_budget_micros: budget,
                ..TrialRemaining::NONE
            }
        }
    })
}

/// Whether a scope locked to `unit` already spent the budget its latest
/// budgeted contract froze. Such a scope offers no further trial time either.
fn locked_budget_exhausted_tx(
    conn: &Connection,
    scope: &str,
    unit: &str,
) -> Result<bool, AppError> {
    if unit == TrialUnit::Usd.as_str() {
        let allowance: Option<String> = conn.query_row("SELECT value_allowance_usd_micros FROM market_share_trial_contracts WHERE scope_id=?1 AND value_allowance_usd_micros IS NOT NULL AND value_allowance_usd_micros<>'0' ORDER BY created_at DESC, contract_id DESC LIMIT 1", params![scope], |r| r.get(0)).optional().map_err(db_error("read locked Share trial USD allowance"))?;
        let Some(allowance) = allowance else {
            return Ok(false);
        };
        return Ok(scope_value_used_picos_tx(conn, scope)?
            >= micros_to_picos(stored_decimal(&allowance, "USD allowance")?)?);
    }
    let allowance: Option<u64> = conn.query_row("SELECT token_allowance FROM market_share_trial_contracts WHERE scope_id=?1 AND token_allowance>0 ORDER BY created_at DESC, contract_id DESC LIMIT 1", params![scope], |r| r.get(0)).optional().map_err(db_error("read locked Share trial Token allowance"))?;
    let Some(allowance) = allowance else {
        return Ok(false);
    };
    Ok(scope_tokens_used_tx(conn, scope)? >= allowance)
}

/// A switched-unit quote is time-only, but an old in-flight request may
/// finish after it was committed. Exhausting the original unit must still
/// end the scope instead of leaving the new time-only trial running.
fn switched_budget_exhausted_tx(
    conn: &Connection,
    contract: &str,
) -> Result<Option<&'static str>, AppError> {
    let (scope, unit, value): (String, Option<String>, Option<String>) = conn.query_row(
        "SELECT t.scope_id,s.unit,t.value_allowance_usd_micros FROM market_share_trial_contracts t
         JOIN market_share_trial_scopes s ON s.id=t.scope_id WHERE t.contract_id=?1",
        params![contract], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    ).map_err(db_error("read switched Share trial budget"))?;
    let current = if value.is_some() {
        TrialUnit::Usd
    } else {
        TrialUnit::Tokens
    };
    match unit.as_deref() {
        Some(unit)
            if unit != current.as_str() && locked_budget_exhausted_tx(conn, &scope, unit)? =>
        {
            Ok(Some(if unit == TrialUnit::Usd.as_str() {
                "value"
            } else {
                "tokens"
            }))
        }
        _ => Ok(None),
    }
}

pub(crate) fn prepare_tx(
    conn: &Connection,
    contract: &str,
    entitlement: &str,
    quote: &TrialQuote,
    now: &str,
) -> Result<(), AppError> {
    let invalid =
        || AppError::Conflict("invalid frozen Share trial terms; request a new quote".into());
    let frozen_micros = |raw: Option<&str>| {
        raw.map(crate::market_trial_policy::parse_value_micros)
            .transpose()
            .map_err(|_| invalid())
    };
    let value_allowance = frozen_micros(quote.value_allowance_usd_micros.as_deref())?;
    let value_budget = frozen_micros(quote.value_budget_usd_micros.as_deref())?;
    if quote.version != 1
        || quote.policy_version < 1
        || quote.token_allowance > crate::market_trial_policy::MAX_TOKENS
        || quote.token_budget > quote.token_allowance
        || value_allowance.is_some() != value_budget.is_some()
        || value_budget > value_allowance
        // A USD trial never carries a Token budget, and only a USD quote
        // may claim USD inheritance.
        || (value_allowance.is_some() && (quote.token_allowance != 0 || quote.tokens_inherited))
        || (value_allowance.is_none() && quote.value_inherited)
        || (quote.unit_locked && (quote.token_allowance != 0 || value_allowance.unwrap_or(0) != 0))
    {
        return Err(invalid());
    }
    let allowance = match value_allowance {
        Some(micros) => TrialAllowance::UsdMicros(micros),
        None => TrialAllowance::Tokens(quote.token_allowance),
    };
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
    let current = remaining_in_unit_tx(
        conn,
        &buyer,
        &provider,
        &share,
        crate::market_trial_policy::MAX_HOURS * 3600,
        allowance,
    )?;
    // The time allowance was already fenced against actual historical usage by
    // prepare_contract_tx. Here only scope termination, unit lock and budget
    // drift matter.
    if remaining > 0
        && (current.seconds == 0
            || current.unit_locked != quote.unit_locked
            || current.token_budget < quote.token_budget
            || current.value_budget_micros < value_budget.unwrap_or(0))
    {
        return Err(AppError::Conflict(
            "Share trial usage changed; request a new quote".into(),
        ));
    }
    let scope = uuid::Uuid::new_v4().to_string();
    conn.execute("INSERT OR IGNORE INTO market_share_trial_scopes(id,buyer_user_id,provider_user_id,share_id) VALUES(?1,?2,?3,?4)",params![scope,buyer,provider,share]).map_err(db_error("create Share trial scope"))?;
    if remaining > 0 {
        // The first trial contract fixes the scope's unit, even a time-only one.
        conn.execute("UPDATE market_share_trial_scopes SET unit=COALESCE(unit,?4) WHERE buyer_user_id=?1 AND provider_user_id=?2 AND share_id=?3",params![buyer,provider,share,allowance.unit().as_str()]).map_err(db_error("lock Share trial unit"))?;
    }
    let frozen_value_budget =
        value_budget.map(|budget| if remaining > 0 { budget } else { 0 }.to_string());
    conn.execute("INSERT INTO market_share_trial_contracts(contract_id,scope_id,entitlement_id,token_allowance,token_budget,created_at,updated_at,value_allowance_usd_micros,value_budget_usd_micros) SELECT ?1,id,?5,?6,?7,?8,?8,?9,?10 FROM market_share_trial_scopes WHERE buyer_user_id=?2 AND provider_user_id=?3 AND share_id=?4", params![contract,buyer,provider,share,entitlement,quote.token_allowance,if remaining > 0 { quote.token_budget } else { 0 },now,value_allowance.map(|micros| micros.to_string()),frozen_value_budget]).map_err(db_error("freeze Share trial budget"))?;
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

/// The USD trial terms sent with a managed upsert: (allowance, this
/// contract's remaining cap), both or neither. Only a trialing USD contract
/// with a non-zero budget carries them; otherwise the trial is time-only.
pub(crate) fn control_value_terms_tx(
    conn: &Connection,
    subscription: &str,
) -> Result<(Option<String>, Option<String>), AppError> {
    let row: Option<(String, Option<String>, Option<String>)> = conn.query_row("SELECT t.phase,t.value_allowance_usd_micros,t.value_budget_usd_micros FROM market_share_trial_contracts t JOIN market_platform_contracts c ON c.id=t.contract_id WHERE c.product_kind='share' AND c.product_ref=?1",params![subscription],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(db_error("read Share USD trial control terms"))?;
    Ok(match row {
        Some((phase, Some(allowance), Some(budget)))
            if phase == "trial" && allowance != "0" && budget != "0" =>
        {
            (Some(allowance), Some(budget))
        }
        _ => (None, None),
    })
}

/// Descriptor-level shape of a market entitlement's USD trial fields:
/// both-or-neither canonical micros with 0 < limit <= allowance <= max,
/// never together with Token trial fields, and canonical used picos.
pub(crate) fn valid_entitlement_value_terms(state: &ShareMarketEntitlement) -> bool {
    let micros = |raw: &str| {
        crate::share_value_pricing::unsigned_decimal(raw)
            .ok()
            .filter(|value| *value <= crate::share_value_pricing::MAX_LIMIT_MICROS)
    };
    if crate::share_value_pricing::unsigned_decimal(&state.trial_value_used_picos).is_err() {
        return false;
    }
    match (
        state.trial_value_allowance_usd_micros.as_deref(),
        state.trial_value_limit_usd_micros.as_deref(),
    ) {
        (None, None) => true,
        (Some(allowance), Some(limit)) => {
            state.trial_token_allowance.is_none()
                && state.trial_token_limit.is_none()
                && micros(allowance)
                    .zip(micros(limit))
                    .is_some_and(|(allowance, limit)| limit > 0 && limit <= allowance)
        }
        _ => false,
    }
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
        let (scope,allowance,budget,value_allowance,value_budget,value_used):(String,u64,u64,Option<String>,Option<String>,String)=conn.query_row("SELECT scope_id,token_allowance,token_budget,value_allowance_usd_micros,value_budget_usd_micros,value_used_picos FROM market_share_trial_contracts WHERE contract_id=?1",params![contract],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).map_err(db_error("read frozen Share trial identity"))?;
        let capped = allowance > 0 && budget > 0;
        // The frozen USD cap is sent with the trial phase only, but the
        // Server keeps reporting it; compare the contract's frozen pair.
        let value_capped = value_allowance.as_deref().is_some_and(|v| v != "0")
            && value_budget.as_deref().is_some_and(|v| v != "0");
        if state.trial_scope_id != scope
            || state.trial_token_limit != capped.then_some(budget)
            || state.trial_token_allowance != capped.then_some(allowance)
            || state.trial_value_limit_usd_micros.as_deref()
                != value_budget.as_deref().filter(|_| value_capped)
            || state.trial_value_allowance_usd_micros.as_deref()
                != value_allowance.as_deref().filter(|_| value_capped)
        {
            return Err(AppError::Conflict(
                "Server trial scope or budget differs from frozen terms".into(),
            ));
        }
        if state.usage_revision < revision {
            continue;
        }
        let value_used = stored_decimal(&value_used, "USD usage")?;
        let reported_value =
            crate::share_value_pricing::unsigned_decimal(&state.trial_value_used_picos).map_err(
                |_| AppError::Conflict("Share trial USD usage is not a canonical decimal".into()),
            )?;
        if state.trial_tokens_used < used
            || reported_value < value_used
            || (state.usage_revision == revision
                && (state.trial_tokens_used != used || reported_value != value_used))
        {
            return Err(AppError::Conflict(
                "Share trial usage watermark changed payload".into(),
            ));
        }
        conn.execute("UPDATE market_share_trial_contracts SET tokens_used=?2,usage_revision=?3,updated_at=?4,value_used_picos=?5 WHERE contract_id=?1",params![contract,state.trial_tokens_used,state.usage_revision,now,reported_value.to_string()]).map_err(db_error("persist Share trial usage"))?;
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

/// A budgeted USD trial ends once this contract's frozen budget or the
/// scope's lifetime allowance is spent, exactly like the Token budget.
fn value_trial_exhausted_tx(conn: &Connection, contract: &str) -> Result<bool, AppError> {
    let (scope, allowance, budget, used): (String, Option<String>, Option<String>, String) = conn.query_row("SELECT scope_id,value_allowance_usd_micros,value_budget_usd_micros,value_used_picos FROM market_share_trial_contracts WHERE contract_id=?1",params![contract],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(db_error("read Share trial USD thresholds"))?;
    let Some(allowance) = allowance else {
        return Ok(false);
    };
    let allowance = stored_decimal(&allowance, "USD allowance")?;
    if allowance == 0 {
        return Ok(false);
    }
    let budget = stored_decimal(budget.as_deref().unwrap_or("0"), "USD budget")?;
    Ok(
        stored_decimal(&used, "USD usage")? >= micros_to_picos(budget)?
            || scope_value_used_picos_tx(conn, &scope)? >= micros_to_picos(allowance)?,
    )
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
    let switched_exhaustion = if phase == "trial" {
        switched_budget_exhausted_tx(conn, contract)?
    } else {
        None
    };
    let tokens_exhausted = (allowance > 0 && (used >= budget || scope_used >= allowance))
        || switched_exhaustion == Some("tokens");
    let value_exhausted =
        value_trial_exhausted_tx(conn, contract)? || switched_exhaustion == Some("value");
    if phase == "trial" && (remaining == 0 || tokens_exhausted || value_exhausted) {
        request_conversion_tx(
            conn,
            contract,
            if tokens_exhausted {
                "tokens"
            } else if value_exhausted {
                "value"
            } else {
                "time"
            },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usd_trial_arithmetic_rounds_against_the_buyer() {
        assert_eq!(picos_to_micros_ceil(0), 0);
        assert_eq!(picos_to_micros_ceil(1), 1);
        assert_eq!(picos_to_micros_ceil(PICOS_PER_MICRO), 1);
        assert_eq!(picos_to_micros_ceil(PICOS_PER_MICRO + 1), 2);
        assert_eq!(remaining_value_micros(1_000_000, 0), 1_000_000);
        // Any started micro is spent.
        assert_eq!(remaining_value_micros(1_000_000, 1), 999_999);
        assert_eq!(
            remaining_value_micros(1_000_000, 1_000_000 * PICOS_PER_MICRO),
            0
        );
        assert_eq!(remaining_value_micros(1_000_000, i128::MAX), 0);
        assert_eq!(remaining_value_micros(0, 0), 0);
    }

    fn entitlement() -> ShareMarketEntitlement {
        ShareMarketEntitlement {
            trial_scope_id: "scope".into(),
            trial_token_allowance: None,
            email: "buyer@example.com".into(),
            phase: ShareMarketPhase::Trial,
            active: true,
            trial_token_limit: None,
            trial_tokens_used: 0,
            trial_value_allowance_usd_micros: Some("1000000".into()),
            trial_value_limit_usd_micros: Some("750000".into()),
            trial_value_used_picos: "0".into(),
            usage_revision: 0,
            usage_uncertain: false,
            applied_at_ms: 1,
            paid_started_at_ms: None,
        }
    }

    #[test]
    fn entitlement_usd_trial_terms_are_paired_canonical_and_exclusive() {
        assert!(valid_entitlement_value_terms(&entitlement()));
        let none = ShareMarketEntitlement {
            trial_value_allowance_usd_micros: None,
            trial_value_limit_usd_micros: None,
            trial_token_allowance: Some(10),
            trial_token_limit: Some(5),
            ..entitlement()
        };
        assert!(valid_entitlement_value_terms(&none));
        let cases: Vec<Box<dyn Fn(&mut ShareMarketEntitlement)>> = vec![
            Box::new(|s| s.trial_value_limit_usd_micros = None),
            Box::new(|s| s.trial_value_allowance_usd_micros = None),
            Box::new(|s| s.trial_value_limit_usd_micros = Some("0".into())),
            Box::new(|s| s.trial_value_limit_usd_micros = Some("1000001".into())),
            Box::new(|s| s.trial_value_limit_usd_micros = Some("0750000".into())),
            Box::new(|s| s.trial_value_allowance_usd_micros = Some("1000000000000001".into())),
            Box::new(|s| s.trial_token_limit = Some(5)),
            Box::new(|s| s.trial_token_allowance = Some(5)),
            Box::new(|s| s.trial_value_used_picos = "01".into()),
            Box::new(|s| s.trial_value_used_picos = String::new()),
        ];
        for (index, mutate) in cases.iter().enumerate() {
            let mut state = entitlement();
            mutate(&mut state);
            assert!(!valid_entitlement_value_terms(&state), "case {index}");
        }
    }

    #[test]
    fn entitlement_wire_keeps_token_only_payloads_and_omits_zero_usd_usage() {
        let json = serde_json::json!({
            "trialScopeId": "scope", "email": "buyer@example.com", "phase": "trial",
            "active": true, "trialTokensUsed": 0, "usageRevision": 0, "appliedAtMs": 1
        });
        let state: ShareMarketEntitlement = serde_json::from_value(json).unwrap();
        assert_eq!(state.trial_value_used_picos, "0");
        let encoded = serde_json::to_value(&state).unwrap();
        assert!(encoded.get("trialValueUsedPicos").is_none());
        let usd = serde_json::to_value(ShareMarketEntitlement {
            trial_value_used_picos: "12".into(),
            ..entitlement()
        })
        .unwrap();
        assert_eq!(usd["trialValueAllowanceUsdMicros"], "1000000");
        assert_eq!(usd["trialValueLimitUsdMicros"], "750000");
        assert_eq!(usd["trialValueUsedPicos"], "12");
    }
}
