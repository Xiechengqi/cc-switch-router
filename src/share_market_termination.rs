//! Share-only immediate termination. A signed, drained entitlement receipt
//! and financial settlement are independent, recoverable commit points.
use crate::{
    ServerState,
    db::{Connection, OptionalExtension, TransactionBehavior, params},
    error::AppError,
    market_wallet::{Posting, credit_wallet_from_liability_tx, post_journal_tx},
    market_wallet_policy::db_error,
    models::{AuthSession, ShareUserPolicy},
    share_market_termination_contract::{TerminationFence, TerminationReceipt},
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, header},
    routing::{get, post},
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

mod maintenance;
mod settlement;
pub(crate) use maintenance::evidence_valid as evidence_valid_tx;
pub(crate) use maintenance::maintain_isolated;
#[cfg(test)]
mod tests;

fn time(s: &str) -> Result<DateTime<Utc>, AppError> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| AppError::Internal("invalid Share termination timestamp".into()))
}
fn no_store() -> [(header::HeaderName, &'static str); 1] {
    [(header::CACHE_CONTROL, "private, no-store")]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    pub version: i64,
    pub rate_bps: i64,
}
pub(crate) fn policy_tx(c: &Connection) -> Result<Policy, AppError> {
    c.query_row(
        "SELECT version,rate_bps FROM share_termination_policies ORDER BY version DESC LIMIT 1",
        [],
        |r| {
            Ok(Policy {
                version: r.get(0)?,
                rate_bps: r.get(1)?,
            })
        },
    )
    .map_err(db_error("read Share termination policy"))
}
pub(crate) fn quote_policy_tx(c: &Connection, share: &str) -> Result<Option<Policy>, AppError> {
    let since: Option<i64> = c
        .query_row(
            "SELECT termination_archive_since_ms FROM shares WHERE share_id=?1",
            params![share],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error("read Share settlement capability"))?
        .flatten();
    since.map(|_| policy_tx(c)).transpose()
}
pub(crate) fn freeze_quote_tx(
    c: &Connection,
    id: &str,
    p: Option<&Policy>,
) -> Result<(), AppError> {
    if let Some(p) = p {
        c.execute(
            "INSERT INTO share_termination_quote_terms VALUES(?1,?2,?3)",
            params![id, p.version, p.rate_bps],
        )
        .map_err(db_error("freeze Share quote terms"))?;
    }
    Ok(())
}
pub(crate) fn freeze_contract_tx(c: &Connection, id: &str, quote: &str) -> Result<(), AppError> {
    c.execute("INSERT INTO share_termination_contract_terms SELECT ?1,t.policy_version,t.rate_bps
        FROM share_termination_quote_terms t JOIN market_platform_contracts c ON c.id=?1 AND c.product_kind='share' WHERE t.quote_id=?2",params![id,quote]).map_err(db_error("freeze Share contract terms"))?;
    Ok(())
}
pub(crate) fn freeze_period_tx(c: &Connection, id: &str, period: &str) -> Result<(), AppError> {
    // Keep the accepted contract's terms on automatic renewal. A changed
    // global policy requires a new explicit quote, never silent consent.
    c.execute("INSERT INTO share_termination_period_terms SELECT ?2,policy_version,rate_bps FROM share_termination_contract_terms WHERE contract_id=?1",params![id,period]).map_err(db_error("freeze Share period terms"))?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Termination {
    pub id: String,
    pub subscription_id: String,
    pub share_id: String,
    pub seat_id: String,
    pub listing_id: String,
    pub entitlement_id: String,
    pub installation_id: String,
    pub share_name: String,
    pub subdomain: String,
    pub buyer_user_id: String,
    pub provider_user_id: String,
    pub buyer_email: String,
    pub provider_email: String,
    pub contract_id: Option<String>,
    pub period_id: Option<String>,
    pub policy_version: i64,
    pub rate_bps: i64,
    pub period_amount_units: i64,
    pub time_earned_units: i64,
    pub prior_refunded_units: i64,
    pub quota_earned_units: i64,
    pub remaining_units: i64,
    pub compensation_cap_units: i64,
    pub returned_units: i64,
    pub compensation_units: i64,
    pub held_units: i64,
    pub policy: ShareUserPolicy,
    pub period_start_ms: i64,
    pub period_end_ms: i64,
    pub cutoff_at: String,
    pub revoked_at: Option<String>,
    pub status: String,
    pub reason: String,
    pub claim: String,
    pub responsibility: String,
    pub dispute_opened_at: Option<String>,
    pub provider_responded_at: Option<String>,
    pub decision_note: Option<String>,
    pub decided_award_units: Option<i64>,
    pub version: i64,
    pub updated_at: String,
}
fn load(c: &Connection, id: &str) -> Result<Termination, AppError> {
    let s: String = c
        .query_row(
            "SELECT payload_json FROM share_market_terminations WHERE id=?1",
            params![id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error("read Share termination"))?
        .ok_or_else(|| AppError::NotFound("Share termination not found".into()))?;
    serde_json::from_str(&s)
        .map_err(|_| AppError::Internal("invalid Share termination payload".into()))
}
pub(crate) fn for_subscription_tx(
    c: &Connection,
    id: &str,
) -> Result<Option<Termination>, AppError> {
    c.query_row(
        "SELECT id FROM share_market_terminations WHERE subscription_id=?1",
        params![id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(db_error("read Share termination identity"))?
    .map(|id| load(c, &id))
    .transpose()
}
fn save(c: &Connection, t: &Termination) -> Result<(), AppError> {
    c.execute("UPDATE share_market_terminations SET status=?2,version=?3,updated_at=?4,payload_json=?5 WHERE id=?1",
        params![t.id,t.status,t.version,t.updated_at,serde_json::to_string(t).map_err(|e|AppError::Internal(e.to_string()))?]).map_err(db_error("save Share termination"))?;
    Ok(())
}
fn event(
    c: &Connection,
    t: &Termination,
    actor: Option<&str>,
    action: &str,
    detail: Value,
    journal: Option<&str>,
    at: &str,
) -> Result<(), AppError> {
    c.execute(
        "INSERT INTO share_termination_events VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            Uuid::new_v4().to_string(),
            t.id,
            t.version,
            actor,
            action,
            detail.to_string(),
            journal,
            at
        ],
    )
    .map_err(db_error("audit Share termination"))?;
    Ok(())
}
fn wire(t: &Termination) -> Value {
    let mut v = serde_json::to_value(t).expect("serializable Share termination");
    if let Some(o) = v.as_object_mut() {
        for (k, v) in o {
            if k.ends_with("Units") && v.is_i64() {
                *v = Value::String(v.to_string());
            }
        }
    }
    v
}

fn basis(c: &Connection, sub: &str, at: &str) -> Result<Termination, AppError> {
    let mut t=c.query_row("SELECT s.share_id,s.seat_id,s.listing_id,s.entitlement_id,s.installation_id,
        COALESCE(sh.share_name,''),COALESCE(sh.subdomain,''),s.renter_user_id,s.owner_user_id,s.renter_email,s.owner_email,
        co.id,p.id,COALESCE(terms.policy_version,0),COALESCE(terms.rate_bps,0),COALESCE(p.amount_units,0),
        COALESCE(p.fulfilled_units,0),COALESCE(p.refunded_units,0),p.period_start,p.period_end
        FROM share_market_subscriptions s LEFT JOIN shares sh ON sh.share_id=s.share_id
        LEFT JOIN market_platform_contracts co ON co.product_kind='share' AND co.product_ref=s.id
        LEFT JOIN market_platform_periods p ON p.contract_id=co.id AND p.sequence=co.current_period_sequence AND p.status='open'
        LEFT JOIN share_termination_period_terms terms ON terms.period_id=p.id
        WHERE s.id=?1 AND s.service_started_at IS NOT NULL AND s.status NOT IN('released','grant_failed')",params![sub],|r|Ok(Termination{
            id:Uuid::new_v4().to_string(),subscription_id:sub.into(),share_id:r.get(0)?,seat_id:r.get(1)?,listing_id:r.get(2)?,
            entitlement_id:r.get(3)?,installation_id:r.get(4)?,share_name:r.get(5)?,subdomain:r.get(6)?,buyer_user_id:r.get(7)?,provider_user_id:r.get(8)?,
            buyer_email:r.get(9)?,provider_email:r.get(10)?,contract_id:r.get(11)?,period_id:r.get(12)?,policy_version:r.get(13)?,rate_bps:r.get(14)?,
            period_amount_units:r.get(15)?,time_earned_units:r.get(16)?,prior_refunded_units:r.get(17)?,quota_earned_units:0,
            remaining_units:0,compensation_cap_units:0,returned_units:0,compensation_units:0,held_units:0,policy:Default::default(),
            period_start_ms:0,period_end_ms:0,cutoff_at:at.into(),revoked_at:None,status:"revoke_pending".into(),reason:"voluntary".into(),claim:String::new(),
            responsibility:"buyer".into(),dispute_opened_at:None,provider_responded_at:None,decision_note:None,decided_award_units:None,version:1,updated_at:at.into(),
        })).optional().map_err(db_error("read Share termination basis"))?.ok_or_else(||AppError::NotFound("active Share rental not found".into()))?;
    if let Some(p) = t.period_id.as_deref() {
        let (start, end): (String, String) = c
            .query_row(
                "SELECT period_start,period_end FROM market_platform_periods WHERE id=?1",
                params![p],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(db_error("read Share paid period"))?;
        t.period_start_ms = time(&start)?.timestamp_millis();
        t.period_end_ms = time(&end)?.timestamp_millis();
    } else {
        t.period_start_ms = time(at)?.timestamp_millis();
        t.period_end_ms = t.period_start_ms + 1;
    }
    t.policy = crate::share_market::termination_policy_tx(c, sub)?;
    t.remaining_units = t
        .period_amount_units
        .checked_sub(t.time_earned_units)
        .and_then(|v| v.checked_sub(t.prior_refunded_units))
        .filter(|v| *v >= 0)
        .ok_or_else(|| AppError::Internal("Share liability drift".into()))?;
    t.compensation_cap_units = crate::client_market_termination::compensation(
        t.period_amount_units,
        t.remaining_units,
        t.rate_bps,
    )?;
    Ok(t)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationBlockedReason {
    ServiceNotStarted,
    LegacyPaidPeriod,
    ArchiveUnavailable,
    ArchiveIncomplete,
    LifetimeArchiveIncomplete,
    PaidPeriodExpired,
    PaidPeriodRollingOver,
}

impl TerminationBlockedReason {
    fn message(self) -> &'static str {
        match self {
            Self::ServiceNotStarted => {
                "Share access is still being activated; the unstarted rental can be returned without a termination charge"
            }
            Self::LegacyPaidPeriod => {
                "this historical paid period retains its original end-of-period cancellation terms"
            }
            Self::ArchiveUnavailable => {
                "Share immediate termination requires an authoritative Server settlement archive"
            }
            Self::ArchiveIncomplete => {
                "Share immediate termination requires an authoritative Server settlement archive covering this period"
            }
            Self::LifetimeArchiveIncomplete => {
                "lifetime quota requires archive coverage from the first paid admission"
            }
            Self::PaidPeriodExpired => {
                "paid interval expired; refresh the rental before terminating"
            }
            Self::PaidPeriodRollingOver => {
                "paid period is rolling over; refresh before terminating"
            }
        }
    }
}

fn blocked_reason(
    c: &Connection,
    t: &Termination,
) -> Result<Option<TerminationBlockedReason>, AppError> {
    use TerminationBlockedReason::*;
    if t.period_id.is_some() && t.policy_version == 0 {
        return Ok(Some(LegacyPaidPeriod));
    }
    let since: Option<i64> = c
        .query_row(
            "SELECT termination_archive_since_ms FROM shares WHERE share_id=?1",
            params![&t.share_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error("verify Share settlement capability"))?
        .flatten();
    let Some(since) = since else {
        return Ok(Some(ArchiveUnavailable));
    };
    if since > t.period_start_ms {
        return Ok(Some(ArchiveIncomplete));
    }
    if t.period_id.is_some() && time(&t.cutoff_at)?.timestamp_millis() > t.period_end_ms {
        return Ok(Some(PaidPeriodExpired));
    }
    if let Some(contract) = t.contract_id.as_deref() {
        let (model, paid): (String, Option<String>) = c
            .query_row(
                "SELECT billing_model,paid_started_at FROM market_platform_contracts WHERE id=?1",
                params![contract],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(db_error("verify Share billing phase"))?;
        if model == "calendar_month" && paid.is_some() && t.period_id.is_none() {
            return Ok(Some(PaidPeriodRollingOver));
        }
        if t.policy.token_period == crate::models::ShareTokenPeriod::Lifetime
            && paid
                .as_deref()
                .map(time)
                .transpose()?
                .is_some_and(|start| since > start.timestamp_millis())
        {
            return Ok(Some(LifetimeArchiveIncomplete));
        }
    }
    Ok(None)
}

fn eligible(c: &Connection, t: &Termination) -> Result<(), AppError> {
    if let Some(reason) = blocked_reason(c, t)? {
        return Err(AppError::Conflict(reason.message().into()));
    }
    Ok(())
}

pub(crate) fn availability_tx(
    c: &Connection,
    sub: &str,
) -> Result<(bool, Option<TerminationBlockedReason>), AppError> {
    if for_subscription_tx(c, sub)?.is_some() {
        return Ok((false, None));
    }
    match basis(c, sub, &Utc::now().to_rfc3339()) {
        Ok(t) => {
            let reason = blocked_reason(c, &t)?;
            Ok((reason.is_none(), reason))
        }
        Err(AppError::NotFound(_)) => {
            let pending: bool = c
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM share_market_subscriptions WHERE id=?1
                    AND service_started_at IS NULL AND status NOT IN ('released','grant_failed'))",
                    params![sub],
                    |r| r.get(0),
                )
                .map_err(db_error("read Share activation state"))?;
            Ok((
                false,
                pending.then_some(TerminationBlockedReason::ServiceNotStarted),
            ))
        }
        Err(e) => Err(e),
    }
}

fn preview_tx(c: &Connection, sub: &str, buyer: &str, at: &str) -> Result<Value, AppError> {
    let t = basis(c, sub, at)?;
    if t.buyer_user_id != buyer {
        return Err(AppError::Forbidden(
            "Share rental belongs to another buyer".into(),
        ));
    }
    eligible(c, &t)?;
    let id = Uuid::new_v4().to_string();
    let expires = (time(at)? + Duration::seconds(120)).to_rfc3339();
    c.execute("DELETE FROM share_termination_previews WHERE subscription_id=?1 OR id IN(SELECT id FROM share_termination_previews WHERE expires_at<=?2 LIMIT 200)",params![sub,at]).map_err(db_error("expire Share previews"))?;
    c.execute(
        "INSERT INTO share_termination_previews VALUES(?1,?2,?3,?4,?5,?6)",
        params![id, sub, buyer, t.period_id, t.policy_version, expires],
    )
    .map_err(db_error("create Share preview"))?;
    let mut view = wire(&t);
    view["previewId"] = json!(id);
    view["expiresAt"] = json!(expires);
    view["serverNow"] = json!(at);
    view["returnUnits"] = json!((t.remaining_units - t.compensation_cap_units).to_string());
    view["usagePending"] = json!(t.policy.token_limit.is_some() || t.policy.value_quota.is_some());
    Ok(view)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Accept {
    preview_id: String,
    reason: String,
    #[serde(default)]
    claim: String,
}
fn accept_tx(
    c: &Connection,
    sub: &str,
    buyer: &str,
    input: &Accept,
    at: &str,
) -> Result<Termination, AppError> {
    if let Some(t) = for_subscription_tx(c, sub)? {
        if t.buyer_user_id != buyer {
            return Err(AppError::Forbidden("not this buyer".into()));
        }
        return Ok(t);
    }
    if !matches!(
        input.reason.as_str(),
        "voluntary" | "provider_issue" | "platform_issue"
    ) || input.claim.len() > 2000
    {
        return Err(AppError::BadRequest(
            "invalid termination reason or explanation".into(),
        ));
    }
    let mut t = basis(c, sub, at)?;
    if t.buyer_user_id != buyer {
        return Err(AppError::Forbidden("not this buyer".into()));
    }
    eligible(c, &t)?;
    let valid:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM share_termination_previews WHERE id=?1 AND subscription_id=?2
        AND buyer_user_id=?3 AND period_id IS ?4 AND policy_version=?5 AND julianday(expires_at)>julianday(?6))",
        params![input.preview_id,sub,buyer,t.period_id,t.policy_version,at],|r|r.get(0)).map_err(db_error("verify Share preview"))?;
    if !valid {
        return Err(AppError::Conflict(
            "termination preview expired or period changed; reload".into(),
        ));
    }
    if let Some(contract) = t.contract_id.as_deref() {
        crate::market_platform_contracts::settle_share_termination_tail_tx(c, contract, at)?;
    }
    t = basis(c, sub, at)?;
    eligible(c, &t)?;
    t.reason = input.reason.clone();
    t.claim = input.claim.trim().into();
    if t.reason != "voluntary" && t.compensation_cap_units > 0 {
        t.responsibility = "undetermined".into();
        t.dispute_opened_at = Some(at.into());
    }
    c.execute(
        "INSERT INTO share_market_terminations VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?9,1,?10,?11)",
        params![
            t.id,
            sub,
            t.share_id,
            t.contract_id,
            t.period_id,
            t.buyer_user_id,
            t.provider_user_id,
            t.status,
            at,
            serde_json::to_string(&t).map_err(|e| AppError::Internal(e.to_string()))?,
            t.entitlement_id
        ],
    )
    .map_err(db_error("accept Share termination"))?;
    if let Some(contract) = t.contract_id.as_deref() {
        c.execute("UPDATE market_platform_contracts SET renewal_policy='manual',status='ending',last_evaluated_at=?2,updated_at=?2 WHERE id=?1 AND status NOT IN('ended','activation_failed')",params![contract,at]).map_err(db_error("fence Share renewal and billing"))?;
    }
    crate::share_market::request_termination_revoke_tx(c, sub, at)?;
    event(
        c,
        &t,
        Some(buyer),
        "accepted",
        json!({"reason":t.reason}),
        None,
        at,
    )?;
    notify(c, &t, "accepted", at)?;
    Ok(t)
}

pub(crate) fn fence_tx(c: &Connection, sub: &str) -> Result<Option<TerminationFence>, AppError> {
    for_subscription_tx(c, sub)?
        .map(|t| {
            Ok(TerminationFence {
                termination_id: t.id,
                cutoff_at_ms: time(&t.cutoff_at)?.timestamp_millis(),
                period_start_ms: t.period_start_ms,
                period_end_ms: t.period_end_ms,
                policy: t.policy,
            })
        })
        .transpose()
}

pub(crate) fn finalize_contract_tx(
    c: &Connection,
    contract: &str,
    confirmed: bool,
    at: &str,
) -> Result<Option<crate::market_platform_contracts::ContractTermination>, AppError> {
    let sub:Option<String>=c.query_row("SELECT subscription_id FROM share_market_terminations WHERE contract_id=?1 AND period_id IS NOT NULL",params![contract],|r|r.get(0)).optional().map_err(db_error("fence Share financial finalizer"))?;
    let Some(sub) = sub else {
        return Ok(None);
    };
    let finalized = if confirmed {
        finish_tx(c, &sub, at)?;
        for_subscription_tx(c, &sub)?.is_some_and(|t| t.status != "finance_pending")
    } else {
        false
    };
    Ok(Some(
        crate::market_platform_contracts::ContractTermination {
            cleanup_confirmed: confirmed,
            finalized,
            refunded_units: 0,
            unfunded_observations: 0,
        },
    ))
}
pub(crate) fn record_receipt_tx(
    c: &Connection,
    installation: &str,
    receipt: &TerminationReceipt,
    at: &str,
) -> Result<(), AppError> {
    let t = load(c, &receipt.termination_id)?;
    if t.installation_id != installation
        || t.entitlement_id != receipt.entitlement_id
        || t.share_id != receipt.share_id
        || !t.buyer_email.eq_ignore_ascii_case(&receipt.email)
        || receipt.cutoff_at_ms != time(&t.cutoff_at)?.timestamp_millis()
        || receipt.period_start_ms != t.period_start_ms
        || receipt.period_end_ms != t.period_end_ms
        || receipt.archive_since_ms > t.period_start_ms
        || receipt.revoked_at_ms < receipt.cutoff_at_ms
        || receipt.revoked_at_ms > time(at)?.timestamp_millis() + 300_000
    {
        return Err(AppError::Conflict(
            "Share revoke receipt does not match the original rental".into(),
        ));
    }
    let valid:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM share_control_operations WHERE id=?1 AND subscription_id=?2 AND entitlement_id=?3 AND action='revoke' AND status IN('pending','dispatched','applied'))",params![receipt.operation_id,t.subscription_id,t.entitlement_id],|r|r.get(0)).map_err(db_error("bind Share revoke receipt"))?;
    if !valid {
        return Err(AppError::Conflict("unknown Share revoke operation".into()));
    }
    let encoded = serde_json::to_string(receipt).map_err(|e| AppError::Internal(e.to_string()))?;
    let old: Option<String> = c
        .query_row(
            "SELECT payload_json FROM share_termination_receipts WHERE termination_id=?1",
            params![&t.id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error("read Share revoke receipt"))?;
    if let Some(old) = old {
        if old != encoded {
            return Err(AppError::Conflict("Share revoke receipt changed".into()));
        }
        return Ok(());
    }
    settlement::validate_usage(&t, receipt)?;
    c.execute(
        "INSERT INTO share_termination_receipts VALUES(?1,?2,?3,?4)",
        params![t.id, receipt.operation_id, encoded, at],
    )
    .map_err(db_error("persist Share revoke receipt"))?;
    Ok(())
}

/// A successful revoke is durable even if the following financial savepoint
/// fails. The caller may recycle the seat; old access can never be restored.
pub(crate) fn finish_tx(c: &Connection, sub: &str, at: &str) -> Result<bool, AppError> {
    let Some(mut t) = for_subscription_tx(c, sub)? else {
        return Ok(false);
    };
    if t.revoked_at.is_some() && t.status != "finance_pending" {
        return Ok(true);
    }
    let receipt: Option<String> = c
        .query_row(
            "SELECT payload_json FROM share_termination_receipts WHERE termination_id=?1",
            params![&t.id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error("read confirmed Share cleanup"))?;
    let Some(receipt) = receipt else {
        return Err(AppError::Conflict(
            "Share final consumption receipt is still required".into(),
        ));
    };
    let receipt: TerminationReceipt = serde_json::from_str(&receipt)
        .map_err(|_| AppError::Internal("invalid saved Share receipt".into()))?;
    t.revoked_at.get_or_insert(at.into());
    t.status = "finance_pending".into();
    t.updated_at = at.into();
    save(c, &t)?;
    c.execute_batch("SAVEPOINT share_termination_finance")
        .map_err(db_error("begin Share settlement savepoint"))?;
    match settlement::settle(c, t.clone(), &receipt, at) {
        Ok(()) => {
            c.execute_batch("RELEASE SAVEPOINT share_termination_finance")
                .map_err(db_error("commit Share settlement"))?;
        }
        Err(error) => {
            c.execute_batch("ROLLBACK TO SAVEPOINT share_termination_finance; RELEASE SAVEPOINT share_termination_finance").map_err(db_error("retain Share cleanup evidence"))?;
            crate::market_notifications::owner_signal_tx(
                c,
                "share_termination_finance_pending",
                "share_termination",
                &t.id,
                json!({"terminationId":t.id}),
                at,
                false,
            )?;
            tracing::warn!(termination_id=%t.id,%error,"Share revoked; financial settlement deferred");
        }
    }
    Ok(true)
}

fn notify(c: &Connection, t: &Termination, stage: &str, at: &str) -> Result<(), AppError> {
    for (role, user) in [
        ("buyer", &t.buyer_user_id),
        ("provider", &t.provider_user_id),
    ] {
        crate::market_notifications::enqueue_user_notification_tx(
            c,
            role,
            user,
            "share_termination_updated",
            "share_termination",
            &t.id,
            &format!("share-termination:{}:{stage}:{role}", t.id),
            json!({"terminationId":t.id,"shareId":t.share_id,"shareName":t.share_name,"stage":stage,"recordedAt":at}),
            at,
        )?;
    }
    Ok(())
}

fn return_money(
    c: &Connection,
    t: &mut Termination,
    amount: i64,
    action: &str,
    at: &str,
) -> Result<(), AppError> {
    if amount == 0 {
        return Ok(());
    }
    if amount < 0 {
        return Err(AppError::Internal("negative Share return".into()));
    }
    crate::market_wallet::reconcile_wallet_tx(c, &t.buyer_user_id)?;
    let period = t
        .period_id
        .as_deref()
        .ok_or_else(|| AppError::Internal("missing Share rent period".into()))?;
    let journal = credit_wallet_from_liability_tx(
        c,
        &t.buyer_user_id,
        amount,
        "unfulfilled",
        period,
        "share_termination",
        &t.id,
        &format!("{action}:{}", t.version),
        at,
    )?;
    if !journal.inserted {
        return Err(AppError::Internal("Share refund projection drift".into()));
    }
    c.execute("UPDATE market_platform_periods SET refunded_units=refunded_units+?2,updated_at=?3 WHERE id=?1",params![period,amount,at]).map_err(db_error("record Share returned rent"))?;
    t.returned_units += amount;
    event(
        c,
        t,
        None,
        action,
        json!({"amountUnits":amount.to_string()}),
        Some(&journal.id),
        at,
    )
}

fn deadline(t: &Termination) -> Result<Option<DateTime<Utc>>, AppError> {
    if t.responsibility != "undetermined" {
        return Ok(None);
    }
    t.provider_responded_at
        .as_deref()
        .or(t.dispute_opened_at.as_deref())
        .map(time)
        .transpose()
        .map(|v| {
            v.map(|v| {
                v + Duration::days(if t.provider_responded_at.is_some() {
                    7
                } else {
                    3
                })
            })
        })
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Action {
    expected_version: i64,
    #[serde(default)]
    award_units: Option<String>,
    #[serde(default)]
    note: String,
}
fn act_tx(
    c: &Connection,
    id: &str,
    actor: &str,
    owner: bool,
    action: &str,
    input: &Action,
    at: &str,
) -> Result<Termination, AppError> {
    let mut t = load(c, id)?;
    if input.expected_version != t.version {
        return Err(AppError::Conflict(
            "termination changed; reload before processing".into(),
        ));
    }
    if t.status == "finance_pending" {
        return Err(AppError::Conflict(
            "financial evidence must be repaired first".into(),
        ));
    }
    if t.responsibility != "undetermined" || t.compensation_cap_units == 0 {
        return Err(AppError::Conflict("no open compensation claim".into()));
    }
    let automatic = action == "timeout";
    let due = deadline(&t)?.is_some_and(|d| time(at).is_ok_and(|now| now >= d));
    if due != automatic {
        return Err(AppError::Conflict("claim deadline changed; reload".into()));
    }
    if input.note.len() > 2000 {
        return Err(AppError::BadRequest("note exceeds 2000 bytes".into()));
    }
    let cap = if t.revoked_at.is_some() {
        t.held_units
    } else {
        t.compensation_cap_units
    };
    let mut award = cap;
    match action {
        "respond" if t.provider_user_id == actor && t.provider_responded_at.is_none() => {
            if input.note.trim().chars().count() < 3 {
                return Err(AppError::BadRequest(
                    "a formal response requires an audit explanation".into(),
                ));
            }
            t.provider_responded_at = Some(at.into());
        }
        "waive" if t.provider_user_id == actor => {
            award = input
                .award_units
                .as_deref()
                .ok_or_else(|| AppError::BadRequest("awardUnits required".into()))?
                .parse()
                .map_err(|_| AppError::BadRequest("invalid award amount".into()))?;
            if award < 0 || award >= cap {
                return Err(AppError::BadRequest(
                    "Provider may only reduce the remaining compensation".into(),
                ));
            }
            if award == 0 {
                t.responsibility = "none".into();
                t.decided_award_units = Some(0);
            } else {
                if input.note.trim().chars().count() < 3 {
                    return Err(AppError::BadRequest(
                        "a partial waiver requires a formal explanation for Owner review".into(),
                    ));
                }
                t.provider_responded_at.get_or_insert(at.into());
            }
            t.compensation_cap_units = award;
        }
        "resolve" if owner && t.provider_responded_at.is_some() => {
            if actor == t.provider_user_id || actor == t.buyer_user_id {
                return Err(AppError::Forbidden(
                    "a transaction party cannot adjudicate its own compensation".into(),
                ));
            }
            if input.note.trim().chars().count() < 3 {
                return Err(AppError::BadRequest(
                    "Owner decision requires an audit explanation".into(),
                ));
            }
            award = input
                .award_units
                .as_deref()
                .ok_or_else(|| AppError::BadRequest("awardUnits required".into()))?
                .parse()
                .map_err(|_| AppError::BadRequest("invalid award".into()))?;
            if award < 0 || award > cap {
                return Err(AppError::BadRequest(
                    "award exceeds remaining compensation".into(),
                ));
            }
            t.responsibility = if award > 0 { "buyer" } else { "none" }.into();
            t.decided_award_units = Some(award);
            t.compensation_cap_units = award;
        }
        "withdraw" if t.buyer_user_id == actor => {
            t.responsibility = "buyer".into();
            t.decided_award_units = Some(cap);
        }
        "timeout" if automatic => {
            award = 0;
            t.responsibility = "none".into();
            t.decided_award_units = Some(0);
            t.compensation_cap_units = 0;
        }
        _ => {
            return Err(AppError::Forbidden(
                "not allowed to perform this compensation action".into(),
            ));
        }
    }
    if let Some(contract) = t.contract_id.as_deref() {
        crate::market_reconcile::ensure_contract_finance_safe_tx(c, contract)?;
    }
    // The original allocation must still be supported before a new decision
    // moves any held liability. A responsibility choice cannot repair money.
    if t.revoked_at.is_some() && !maintenance::evidence_valid(c, id)? {
        return Err(AppError::Conflict(
            "Share financial evidence requires reconciliation before a decision".into(),
        ));
    }
    t.version += 1;
    t.updated_at = at.into();
    t.decision_note = Some(if automatic {
        "Compensation waived automatically after the response deadline".into()
    } else {
        input.note.trim().into()
    });
    if t.revoked_at.is_some() {
        let refund = cap - award;
        return_money(c, &mut t, refund, "return_compensation", at)?;
        t.held_units = award;
        if t.responsibility == "buyer" {
            settlement::income(c, &t, "share_termination", award, at)?;
            t.compensation_units = award;
            t.held_units = 0;
            c.execute("UPDATE market_platform_periods SET termination_compensation_units=termination_compensation_units+?2 WHERE id=?1",params![t.period_id,award]).map_err(db_error("record Share compensation decision"))?;
        }
        t.status = if t.held_units > 0 {
            "disputed"
        } else {
            "completed"
        }
        .into();
    }
    save(c, &t)?;
    if t.provider_responded_at.is_some()
        && t.responsibility == "undetermined"
        && matches!(action, "respond" | "waive")
    {
        notify_owner_review(c, &t, at)?;
    }
    event(
        c,
        &t,
        if automatic { None } else { Some(actor) },
        action,
        json!({"awardUnits":award.to_string(),"note":t.decision_note}),
        None,
        at,
    )?;
    notify(c, &t, &format!("{action}:{}", t.version), at)?;
    Ok(t)
}

fn authorized(t: &Termination, actor: &AuthSession, owner: bool) -> bool {
    t.buyer_user_id == actor.user_id || t.provider_user_id == actor.user_id || owner
}
fn notify_owner_review(c: &Connection, t: &Termination, at: &str) -> Result<(), AppError> {
    // The event store accepts client/system categories. The notification feed
    // projects this event as a service action, including for non-admin Owners.
    // Ignore only duplicate event IDs, never invalid notification rows.
    c.execute("INSERT INTO user_notification_events(id,scope,category,event_kind,object_kind,object_id,payload_json,created_at_ms)
        VALUES(?1,'owner','system','share_termination_updated','share_termination',?2,?3,?4)
        ON CONFLICT(id) DO NOTHING",params![format!("share-termination-review:{}",t.id),t.id,json!({"terminationId":t.id,"shareId":t.share_id,"shareName":t.share_name,"stage":"owner_review"}).to_string(),time(at)?.timestamp_millis()]).map_err(db_error("notify Share compensation adjudicator"))?;
    Ok(())
}
fn view(
    c: &Connection,
    t: &Termination,
    actor: &AuthSession,
    owner: bool,
    at: &str,
    include_events: bool,
) -> Result<Value, AppError> {
    if !authorized(t, actor, owner) {
        return Err(AppError::NotFound("Share termination not found".into()));
    }
    let buyer = t.buyer_user_id == actor.user_id;
    let provider = t.provider_user_id == actor.user_id;
    let open = t.responsibility == "undetermined"
        && t.compensation_cap_units > 0
        && t.status != "finance_pending";
    let before = deadline(t)?.is_none_or(|d| time(at).is_ok_and(|n| n < d));
    let mut v = wire(t);
    v["permissions"] = json!({"buyer":buyer,"provider":provider,"owner":owner,
        "withdraw":buyer&&open&&before,"waive":provider&&open&&before,
        "respond":provider&&open&&before&&t.provider_responded_at.is_none(),
        "resolve":owner&&!buyer&&!provider&&open&&before&&t.provider_responded_at.is_some(),
        "retryRevoke":(provider||owner)&&t.revoked_at.is_none()});
    v["actionRequired"] = json!(
        open && before
            && ((provider && t.provider_responded_at.is_none())
                || (owner && !buyer && !provider && t.provider_responded_at.is_some()))
    );
    v["responseDeadlineAt"] = json!(deadline(t)?.map(|d| d.to_rfc3339()));
    v["serverNow"] = json!(at);
    v["events"] = json!([]);
    v["eventsNextCursor"] = Value::Null;
    // Row polling only needs action state. Load the immutable history once
    // the user opens a detail dialog, not once per case on every card poll.
    if !include_events {
        return Ok(v);
    }
    let mut s=c.prepare("SELECT id,action,detail_json,created_at FROM share_termination_events WHERE termination_id=?1 ORDER BY created_at DESC,id DESC LIMIT 51").map_err(db_error("read Share termination history"))?;
    let mut events=s.query_map(params![&t.id],|r|Ok(json!({"id":r.get::<_,String>(0)?,"action":r.get::<_,String>(1)?,"detail":serde_json::from_str::<Value>(&r.get::<_,String>(2)?).unwrap_or(Value::Null),"createdAt":r.get::<_,String>(3)?}))).map_err(db_error("query Share termination history"))?.collect::<crate::db::Result<Vec<_>>>().map_err(db_error("load Share termination history"))?;
    v["eventsNextCursor"] = if events.len() > 50 {
        events[49]["id"].clone()
    } else {
        Value::Null
    };
    events.truncate(50);
    v["events"] = json!(events);
    Ok(v)
}

async fn session(state: &ServerState, h: &HeaderMap) -> Result<AuthSession, AppError> {
    crate::api::resolve_router_session(state, h)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))
}
pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/share-market/termination-policy", get(public_policy))
        .route(
            "/v1/admin/market/share-termination-policy",
            get(admin_policy).put(set_policy),
        )
        .route(
            "/v1/share-market/subscriptions/:id/termination-preview",
            post(preview),
        )
        .route("/v1/share-market/subscriptions/:id/terminate", post(accept))
        .route("/v1/share-market/terminations", get(list))
        .route("/v1/share-market/terminations/:id", get(detail))
        .route("/v1/share-market/terminations/:id/events", get(history))
        .route("/v1/share-market/terminations/:id/:action", post(action))
}
async fn public_policy(
    State(s): State<ServerState>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let c = s.store.conn.lock().await;
    Ok((no_store(), Json(policy_tx(&c)?)))
}
async fn admin_policy(
    State(s): State<ServerState>,
    h: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let a = crate::api::require_admin_session(&s, &h).await?;
    let c = s.store.conn.lock().await;
    let p = policy_tx(&c)?;
    Ok((
        no_store(),
        Json(
            json!({"version":p.version,"rateBps":p.rate_bps,"canEdit":crate::api::is_finance_owner(&s.config,&a.email)}),
        ),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetPolicy {
    expected_version: i64,
    rate_bps: i64,
}
async fn set_policy(
    State(s): State<ServerState>,
    h: HeaderMap,
    Json(i): Json<SetPolicy>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let a = crate::api::require_owner_session(&s, &h).await?;
    if !(0..=10_000).contains(&i.rate_bps) {
        return Err(AppError::BadRequest(
            "rate must be between 0 and 100 percent".into(),
        ));
    }
    let c = s.store.conn.lock().await;
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin Share policy"))?;
    if policy_tx(&tx)?.version != i.expected_version {
        return Err(AppError::Conflict("policy changed; reload".into()));
    }
    let version = i
        .expected_version
        .checked_add(1)
        .ok_or_else(|| AppError::Internal("policy version overflow".into()))?;
    tx.execute(
        "INSERT INTO share_termination_policies VALUES(?1,?2,?3,?4)",
        params![version, i.rate_bps, a.user_id, Utc::now().to_rfc3339()],
    )
    .map_err(db_error("save Share policy"))?;
    tx.commit().map_err(db_error("commit Share policy"))?;
    Ok((
        no_store(),
        Json(Policy {
            version,
            rate_bps: i.rate_bps,
        }),
    ))
}
async fn preview(
    State(s): State<ServerState>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let a = session(&s, &h).await?;
    let c = s.store.conn.lock().await;
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin Share preview"))?;
    let v = preview_tx(&tx, &id, &a.user_id, &Utc::now().to_rfc3339())?;
    tx.commit().map_err(db_error("commit Share preview"))?;
    Ok((no_store(), Json(v)))
}
async fn accept(
    State(s): State<ServerState>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(i): Json<Accept>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let a = session(&s, &h).await?;
    let c = s.store.conn.lock().await;
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin Share termination"))?;
    let at = Utc::now().to_rfc3339();
    let t = accept_tx(&tx, &id, &a.user_id, &i, &at)?;
    let v = view(
        &tx,
        &t,
        &a,
        crate::api::is_finance_owner(&s.config, &a.email),
        &at,
        true,
    )?;
    tx.commit().map_err(db_error("commit Share termination"))?;
    Ok((no_store(), Json(v)))
}
async fn detail(
    State(s): State<ServerState>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let a = session(&s, &h).await?;
    let c = s.store.conn.lock().await;
    let t = load(&c, &id)?;
    Ok((
        no_store(),
        Json(view(
            &c,
            &t,
            &a,
            crate::api::is_finance_owner(&s.config, &a.email),
            &Utc::now().to_rfc3339(),
            true,
        )?),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EventQuery {
    cursor: Option<String>,
}
async fn history(
    State(s): State<ServerState>,
    h: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<EventQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let a = session(&s, &h).await?;
    let c = s.store.conn.lock().await;
    let t = load(&c, &id)?;
    if !authorized(&t, &a, crate::api::is_finance_owner(&s.config, &a.email)) {
        return Err(AppError::NotFound("Share termination not found".into()));
    }
    let mut st=c.prepare("SELECT id,action,created_at,detail_json FROM share_termination_events WHERE termination_id=?1
        AND (?2 IS NULL OR (created_at,id)<(SELECT created_at,id FROM share_termination_events WHERE id=?2 AND termination_id=?1)) ORDER BY created_at DESC,id DESC LIMIT 51").map_err(db_error("prepare Share event page"))?;
    let mut items=st.query_map(params![id,q.cursor],|r|Ok(json!({"id":r.get::<_,String>(0)?,"action":r.get::<_,String>(1)?,"createdAt":r.get::<_,String>(2)?,"detail":serde_json::from_str::<Value>(&r.get::<_,String>(3)?).unwrap_or(Value::Null)}))).map_err(db_error("query Share event page"))?.collect::<crate::db::Result<Vec<_>>>().map_err(db_error("read Share event page"))?;
    let next = if items.len() > 50 {
        items.get(49).and_then(|i| i.get("id")).cloned()
    } else {
        None
    };
    items.truncate(50);
    Ok((no_store(), Json(json!({"items":items,"nextCursor":next}))))
}
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    cursor: Option<String>,
    share_id: Option<String>,
    seat_id: Option<String>,
    #[serde(default)]
    pending_only: bool,
    #[serde(default)]
    action_only: bool,
}
async fn list(
    State(s): State<ServerState>,
    h: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let a = session(&s, &h).await?;
    let owner = crate::api::is_finance_owner(&s.config, &a.email);
    let c = s.store.conn.lock().await;
    let at = Utc::now().to_rfc3339();
    let mut st=c.prepare("SELECT id FROM share_market_terminations WHERE (?1 OR buyer_user_id=?2 OR provider_user_id=?2)
        AND (?3 IS NULL OR share_id=?3) AND (?4 IS NULL OR (cutoff_at,id)<(SELECT cutoff_at,id FROM share_market_terminations WHERE id=?4)) AND (?5=0 OR status!='completed')
        AND (?6=0 OR (json_extract(payload_json,'$.responsibility')='undetermined' AND json_extract(payload_json,'$.compensationCapUnits')>0 AND status!='finance_pending'
            AND ((provider_user_id=?2 AND json_extract(payload_json,'$.providerRespondedAt') IS NULL)
                OR (?1 AND provider_user_id!=?2 AND buyer_user_id!=?2 AND json_extract(payload_json,'$.providerRespondedAt') IS NOT NULL))))
        AND (?7 IS NULL OR json_extract(payload_json,'$.seatId')=?7)
        AND (?6=0 OR julianday(?8)<julianday(COALESCE(json_extract(payload_json,'$.providerRespondedAt'),json_extract(payload_json,'$.disputeOpenedAt')),
            CASE WHEN json_extract(payload_json,'$.providerRespondedAt') IS NULL THEN '+3 days' ELSE '+7 days' END))
        ORDER BY cutoff_at DESC,id DESC LIMIT 51").map_err(db_error("prepare Share termination list"))?;
    let ids = st
        .query_map(
            params![
                owner,
                a.user_id,
                q.share_id,
                q.cursor,
                q.pending_only,
                q.action_only,
                q.seat_id,
                at
            ],
            |r| r.get::<_, String>(0),
        )
        .map_err(db_error("query Share terminations"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("load Share termination list"))?;
    let mut items = Vec::new();
    for id in ids.iter().take(50) {
        items.push(view(&c, &load(&c, id)?, &a, owner, &at, false)?);
    }
    Ok((
        no_store(),
        Json(
            json!({"items":items,"nextCursor":if ids.len()>50{ids.get(49)}else{None},"serverNow":at}),
        ),
    ))
}
async fn action(
    State(s): State<ServerState>,
    h: HeaderMap,
    Path((id, kind)): Path<(String, String)>,
    Json(i): Json<Action>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    if !matches!(
        kind.as_str(),
        "respond" | "waive" | "withdraw" | "resolve" | "retry-revoke"
    ) {
        return Err(AppError::NotFound("unknown termination action".into()));
    }
    let a = session(&s, &h).await?;
    let owner = crate::api::is_finance_owner(&s.config, &a.email);
    let c = s.store.conn.lock().await;
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin Share termination action"))?;
    let at = Utc::now().to_rfc3339();
    let t = if kind == "retry-revoke" {
        let t = load(&tx, &id)?;
        if (!owner && t.provider_user_id != a.user_id)
            || i.expected_version != t.version
            || t.revoked_at.is_some()
        {
            return Err(AppError::Forbidden(
                "not allowed to retry this original rental".into(),
            ));
        }
        crate::share_market::retry_termination_revoke_tx(&tx, &t.subscription_id, &at)?;
        t
    } else {
        act_tx(&tx, &id, &a.user_id, owner, &kind, &i, &at)?
    };
    let v = view(&tx, &t, &a, owner, &at, true)?;
    tx.commit()
        .map_err(db_error("commit Share termination action"))?;
    Ok((no_store(), Json(v)))
}
