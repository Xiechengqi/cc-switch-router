//! Client-only, recoverable immediate termination. SSH cleanup is never run
//! inside a financial transaction. Compensation is an award, not delivery.
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, header},
    routing::{get, post},
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    ServerState,
    db::{Connection, OptionalExtension, TransactionBehavior, params},
    error::AppError,
    market_wallet::{Posting, credit_wallet_from_liability_tx, post_journal_tx, serialize_units},
    market_wallet_policy::db_error,
    models::AuthSession,
};

#[cfg(test)]
mod tests;

fn time(at: &str) -> Result<DateTime<Utc>, AppError> {
    DateTime::parse_from_rfc3339(at)
        .map(|v| v.with_timezone(&Utc))
        .map_err(|_| AppError::Internal("invalid Client termination time".into()))
}
fn no_store() -> [(header::HeaderName, &'static str); 1] {
    [(header::CACHE_CONTROL, "private, no-store")]
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    pub version: i64,
    pub rate_bps: i64,
}
pub(crate) fn policy_tx(conn: &Connection) -> Result<Policy, AppError> {
    conn.query_row(
        "SELECT version,rate_bps FROM client_termination_policies ORDER BY version DESC LIMIT 1",
        [],
        |r| {
            Ok(Policy {
                version: r.get(0)?,
                rate_bps: r.get(1)?,
            })
        },
    )
    .map_err(db_error("read Client termination policy"))
}
pub(crate) fn freeze_quote_tx(conn: &Connection, quote: &str) -> Result<(), AppError> {
    let p = policy_tx(conn)?;
    conn.execute(
        "INSERT OR IGNORE INTO client_termination_quote_terms VALUES (?1,?2,?3)",
        params![quote, p.version, p.rate_bps],
    )
    .map_err(db_error("freeze Client termination quote"))?;
    Ok(())
}
pub(crate) fn freeze_contract_tx(
    conn: &Connection,
    contract: &str,
    quote: &str,
) -> Result<(), AppError> {
    conn.execute("INSERT INTO client_termination_contract_terms
        SELECT ?1,q.policy_version,q.rate_bps FROM client_termination_quote_terms q
        JOIN market_platform_contracts c ON c.id=?1 AND c.product_kind='client_host' WHERE q.quote_id=?2",
        params![contract,quote]).map_err(db_error("freeze Client termination contract"))?;
    Ok(())
}
pub(crate) fn freeze_period_tx(
    conn: &Connection,
    contract: &str,
    period: &str,
    first: bool,
    anchor: &str,
) -> Result<(), AppError> {
    if first {
        conn.execute(
            "INSERT INTO client_termination_period_terms SELECT ?2,t.policy_version,t.rate_bps
            FROM client_termination_contract_terms t WHERE t.contract_id=?1",
            params![contract, period],
        )
        .map_err(db_error("freeze first Client termination period"))?;
    } else {
        // Delayed pre-upgrade renewals must not gain fees retroactively.
        conn.execute("INSERT INTO client_termination_period_terms SELECT ?2,p.version,p.rate_bps
            FROM client_termination_policies p JOIN market_platform_contracts c ON c.id=?1 AND c.product_kind='client_host'
            WHERE julianday(p.created_at)<=julianday(?3) ORDER BY p.version DESC LIMIT 1",params![contract,period,anchor])
            .map_err(db_error("freeze renewed Client termination period"))?;
    }
    Ok(())
}

/// Integer round-half-up in ledger units (not cents), capped by the remaining
/// consideration. Never debits another balance or creates a tenant debt.
pub(crate) fn compensation(period: i64, remaining: i64, bps: i64) -> Result<i64, AppError> {
    if period < 0 || remaining < 0 || remaining > period || !(0..=10000).contains(&bps) {
        return Err(AppError::Internal(
            "invalid Client compensation base".into(),
        ));
    }
    Ok(remaining.min(((i128::from(period) * i128::from(bps) + 5000) / 10000) as i64))
}

struct Basis {
    contract: Option<String>,
    period: Option<String>,
    provider: String,
    buyer: String,
    host: String,
    policy: i64,
    rate: i64,
    amount: i64,
    remaining: i64,
    eligible: bool,
}
fn basis_tx(conn: &Connection, installation: &str) -> Result<Basis, AppError> {
    let b=conn.query_row("SELECT s.platform_contract_id,p.id,s.provider_id,s.client_user_id,s.host_id,
        COALESCE(t.policy_version,0),COALESCE(t.rate_bps,0),COALESCE(p.amount_units,0),
        COALESCE(p.amount_units-p.fulfilled_units-p.refunded_units-p.termination_compensation_units,0),
        COALESCE(c.paid_started_at IS NOT NULL AND p.status='open',0)
        FROM client_market_subscriptions s LEFT JOIN market_platform_contracts c ON c.id=s.platform_contract_id
        LEFT JOIN market_platform_periods p ON p.contract_id=c.id AND p.sequence=c.current_period_sequence
        LEFT JOIN client_termination_period_terms t ON t.period_id=p.id WHERE s.installation_id=?1",
        params![installation], |r| Ok(Basis { contract:r.get(0)?,period:r.get(1)?,provider:r.get(2)?,buyer:r.get(3)?,
            host:r.get(4)?,policy:r.get(5)?,rate:r.get(6)?,amount:r.get(7)?,remaining:r.get(8)?,eligible:r.get(9)? }))
        .optional().map_err(db_error("read Client termination basis"))?
        .ok_or_else(|| AppError::NotFound("Client rental not found".into()))?;
    if let Some(contract) = b.contract.as_deref() {
        let valid:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM market_platform_contracts WHERE id=?1
            AND product_kind='client_host' AND service_ref=?2 AND buyer_user_id=?3 AND provider_user_id=?4)",
            params![contract,installation,b.buyer,b.provider],|r|r.get(0)).map_err(db_error("verify Client termination identity"))?;
        if !valid {
            return Err(AppError::Conflict(
                "Client contract identity requires Owner review".into(),
            ));
        }
    }
    Ok(b)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Preview {
    preview_id: String,
    expires_at: String,
    policy_version: i64,
    rate_bps: i64,
    #[serde(serialize_with = "serialize_units")]
    period_amount_units: i64,
    #[serde(serialize_with = "serialize_units")]
    remaining_units: i64,
    #[serde(serialize_with = "serialize_units")]
    compensation_units: i64,
    #[serde(serialize_with = "serialize_units")]
    return_units: i64,
}
fn preview_tx(
    conn: &Connection,
    installation: &str,
    buyer: &str,
    at: &str,
) -> Result<Preview, AppError> {
    let b = basis_tx(conn, installation)?;
    if b.buyer != buyer {
        return Err(AppError::Forbidden(
            "Client rental belongs to another buyer".into(),
        ));
    }
    if let Some(c) = b.contract.as_deref() {
        crate::market_platform_contracts::settle_client_termination_tail_tx(conn, c, at)?;
    }
    let b = basis_tx(conn, installation)?;
    let cap = if b.eligible {
        compensation(b.amount, b.remaining, b.rate)?
    } else {
        0
    };
    let id = Uuid::new_v4().to_string();
    let expires = (time(at)? + Duration::seconds(120)).to_rfc3339();
    // At most one outstanding preview per rental; retrying replaces the token.
    conn.execute(
        "DELETE FROM client_termination_previews WHERE installation_id=?1 OR id IN
        (SELECT id FROM client_termination_previews WHERE expires_at<=?2 LIMIT 200)",
        params![installation, at],
    )
    .map_err(db_error("expire Client termination previews"))?;
    conn.execute(
        "INSERT INTO client_termination_previews VALUES (?1,?2,?3,?4,?5,?6)",
        params![id, installation, buyer, b.period, b.policy, expires],
    )
    .map_err(db_error("create Client termination preview"))?;
    Ok(Preview {
        preview_id: id,
        expires_at: expires,
        policy_version: b.policy,
        rate_bps: b.rate,
        period_amount_units: b.amount,
        remaining_units: b.remaining,
        compensation_units: cap,
        return_units: b.remaining - cap,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TerminationRequest {
    pub preview_id: String,
    pub reason: String,
    #[serde(default)]
    pub claim: String,
}

/// Must execute in the SAME transaction as durable cleanup job creation and
/// tunnel disable. A replay keeps its original cutoff and monetary snapshot.
pub(crate) fn accept_tx(
    conn: &Connection,
    installation: &str,
    actor: Option<&str>,
    reason: &str,
    request: Option<&TerminationRequest>,
    at: &str,
) -> Result<(), AppError> {
    if read_tx(conn, installation)?.is_some() {
        return Ok(());
    }
    let b = match basis_tx(conn, installation) {
        Ok(b) => b,
        Err(AppError::NotFound(_)) if reason != "client_release" => return Ok(()),
        Err(error) => return Err(error),
    };
    // A Provider cleaning their own self-rented Host is still a Provider
    // operation. An unrelated tenant cannot use that path to bypass preview.
    let voluntary = reason == "client_release"
        || (actor == Some(b.buyer.as_str()) && actor != Some(b.provider.as_str()));
    if actor.is_some() && actor != Some(b.buyer.as_str()) && actor != Some(b.provider.as_str()) {
        return Err(AppError::Forbidden(
            "cleanup actor is not a party to this rental".into(),
        ));
    }
    let (reason, claim, responsibility) = if voluntary {
        if actor != Some(b.buyer.as_str()) {
            return Err(AppError::Forbidden(
                "only the Client buyer may terminate".into(),
            ));
        }
        let req = request.ok_or_else(|| {
            AppError::Conflict("preview immediate termination before confirming".into())
        })?;
        let valid: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM client_termination_previews WHERE id=?1
            AND installation_id=?2 AND buyer_user_id=?3 AND period_id IS ?4 AND policy_version=?5
            AND julianday(expires_at)>julianday(?6))",
                params![
                    req.preview_id,
                    installation,
                    b.buyer,
                    b.period,
                    b.policy,
                    at
                ],
                |r| r.get(0),
            )
            .map_err(db_error("validate Client termination preview"))?;
        if !valid {
            return Err(AppError::Conflict(
                "termination preview expired or period changed; reload".into(),
            ));
        }
        if !matches!(
            req.reason.as_str(),
            "voluntary" | "provider_issue" | "platform_issue"
        ) || req.claim.len() > 2000
        {
            return Err(AppError::BadRequest(
                "invalid reason or explanation exceeds 2000 bytes".into(),
            ));
        }
        (
            req.reason.as_str(),
            req.claim.as_str(),
            if req.reason == "voluntary" {
                "buyer"
            } else {
                "undetermined"
            },
        )
    } else {
        (
            reason,
            "",
            if actor == Some(b.provider.as_str()) {
                "provider"
            } else {
                "none"
            },
        )
    };
    if let Some(c) = b.contract.as_deref() {
        crate::market_platform_contracts::settle_client_termination_tail_tx(conn, c, at)?;
    }
    let b = basis_tx(conn, installation)?;
    let cap = if voluntary && b.eligible {
        compensation(b.amount, b.remaining, b.rate)?
    } else {
        0
    };
    // Free/trial/zero-rate exits have no compensation to pursue. Do not show
    // an unresolved financial responsibility or start a meaningless deadline.
    let responsibility = if responsibility == "undetermined" && cap == 0 {
        "none"
    } else {
        responsibility
    };
    let id = Uuid::new_v4().to_string();
    conn.execute("INSERT INTO client_market_terminations (id,installation_id,host_id,contract_id,period_id,
        buyer_user_id,provider_user_id,policy_version,rate_bps,period_amount_units,remaining_units,
        compensation_cap_units,reason,claim,responsibility,status,cutoff_at,updated_at,dispute_used,dispute_opened_at)
        VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,'cleanup_pending',?16,?16,?17,?18)",
        params![id,installation,b.host,b.contract,b.period,b.buyer,b.provider,b.policy,b.rate,b.amount,b.remaining,
            cap,reason,claim,responsibility,at, i64::from(responsibility=="undetermined" && cap>0),
            (responsibility=="undetermined" && cap>0).then_some(at)])
        .map_err(db_error("record Client termination"))?;
    if let Some(c) = b.contract.as_deref() {
        conn.execute("UPDATE market_platform_contracts SET renewal_policy='manual',status=CASE
            WHEN status IN ('ended','activation_failed') THEN status ELSE 'ending' END,updated_at=?2 WHERE id=?1",params![c,at])
            .map_err(db_error("stop Client billing at accepted termination"))?;
    }
    event_tx(
        conn,
        &id,
        actor,
        "accepted",
        json!({"reason":reason,"cutoffAt":at}),
        None,
        at,
    )?;
    if responsibility == "undetermined" && cap > 0 {
        crate::store::client_chat::ensure_termination_chat_tx(conn, installation, at)?;
        notify_claim_tx(conn, &by_id_tx(conn, &id)?, "opened", at)?;
        crate::market_notifications::owner_signal_tx(
            conn,
            "client_compensation_dispute",
            "client_termination",
            &id,
            json!({"terminationId":id,"action":"review_financial_responsibility"}),
            at,
            false,
        )?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Termination {
    pub id: String,
    pub installation_id: String,
    pub host_id: String,
    pub contract_id: Option<String>,
    pub period_id: Option<String>,
    pub buyer_user_id: String,
    pub provider_user_id: String,
    pub policy_version: i64,
    pub rate_bps: i64,
    #[serde(serialize_with = "serialize_units")]
    pub period_amount_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pub remaining_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pub compensation_cap_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pub returned_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pub compensation_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pub held_units: i64,
    pub reason: String,
    pub claim: String,
    pub responsibility: String,
    pub status: String,
    pub cutoff_at: String,
    pub cleanup_confirmed_at: Option<String>,
    pub cleanup_job_id: Option<String>,
    pub dispute_used: bool,
    pub dispute_opened_at: Option<String>,
    pub dispute_deadline_at: Option<String>,
    pub provider_responded_at: Option<String>,
    pub version: i64,
    pub updated_at: String,
    pub unlock_at: Option<String>,
    #[serde(serialize_with = "serialize_units")]
    pub credited_units: i64,
    pub client_subdomain: Option<String>,
    pub buyer_email: Option<String>,
    pub provider_email: Option<String>,
    pub decision_note: Option<String>,
    pub resolution: Option<String>,
    pub chat_available: bool,
}
fn read_tx(conn: &Connection, installation: &str) -> Result<Option<Termination>, AppError> {
    conn.query_row("SELECT t.id,t.installation_id,t.host_id,t.contract_id,t.period_id,t.buyer_user_id,t.provider_user_id,
        t.policy_version,t.rate_bps,t.period_amount_units,t.remaining_units,t.compensation_cap_units,t.returned_units,
        t.compensation_units,t.held_units,t.reason,t.claim,t.responsibility,t.status,t.cutoff_at,t.cleanup_confirmed_at,
        t.cleanup_job_id,t.dispute_used,t.version,t.updated_at,c.unlock_at,COALESCE(c.credited_units,0),
        (SELECT client_subdomain FROM client_market_subscriptions WHERE installation_id=t.installation_id),
        (SELECT email_normalized FROM users WHERE id=t.buyer_user_id),
        (SELECT email_normalized FROM users WHERE id=t.provider_user_id),
        (SELECT json_extract(detail_json,'$.note') FROM client_termination_events WHERE termination_id=t.id
            AND action IN ('provider_waiver','owner_decision','provider_response','buyer_withdrawal','timeout_waiver') ORDER BY version DESC,created_at DESC LIMIT 1),
        t.dispute_opened_at,NULL,t.provider_responded_at,
        (SELECT action FROM client_termination_events WHERE termination_id=t.id
            AND action IN ('provider_waiver','owner_decision','buyer_withdrawal','timeout_waiver') ORDER BY version DESC,created_at DESC LIMIT 1),
        EXISTS(SELECT 1 FROM chat_rooms WHERE installation_id=t.installation_id)
        FROM client_market_terminations t LEFT JOIN client_termination_compensations c ON c.id=t.id WHERE t.installation_id=?1",
        params![installation],|r|Ok(Termination{id:r.get(0)?,installation_id:r.get(1)?,host_id:r.get(2)?,contract_id:r.get(3)?,
            period_id:r.get(4)?,buyer_user_id:r.get(5)?,provider_user_id:r.get(6)?,policy_version:r.get(7)?,rate_bps:r.get(8)?,
            period_amount_units:r.get(9)?,remaining_units:r.get(10)?,compensation_cap_units:r.get(11)?,returned_units:r.get(12)?,
            compensation_units:r.get(13)?,held_units:r.get(14)?,reason:r.get(15)?,claim:r.get(16)?,responsibility:r.get(17)?,
            status:r.get(18)?,cutoff_at:r.get(19)?,cleanup_confirmed_at:r.get(20)?,cleanup_job_id:r.get(21)?,dispute_used:r.get(22)?,
            version:r.get(23)?,updated_at:r.get(24)?,unlock_at:r.get(25)?,credited_units:r.get(26)?,
            client_subdomain:r.get(27)?,buyer_email:r.get(28)?,provider_email:r.get(29)?,decision_note:r.get(30)?,
            dispute_opened_at:r.get(31)?,dispute_deadline_at:r.get(32)?,provider_responded_at:r.get(33)?,
            resolution:r.get(34)?,chat_available:r.get(35)?}))
        .optional().map_err(db_error("read Client termination"))?
        .map(|mut t| {
            // Preserve sub-millisecond precision: SQLite strftime would round
            // a deadline earlier than the authoritative chrono comparison.
            t.dispute_deadline_at = t.dispute_opened_at.as_deref().map(time).transpose()?
                .map(|opened| (opened + Duration::days(3)).to_rfc3339());
            Ok(t)
        }).transpose()
}
fn by_id_tx(conn: &Connection, id: &str) -> Result<Termination, AppError> {
    let installation: String = conn
        .query_row(
            "SELECT installation_id FROM client_market_terminations WHERE id=?1",
            params![id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error("find Client termination"))?
        .ok_or_else(|| AppError::NotFound("termination not found".into()))?;
    read_tx(conn, &installation)?.ok_or_else(|| AppError::NotFound("termination not found".into()))
}
pub(crate) fn replay_job_tx(
    conn: &Connection,
    installation: &str,
    buyer: &str,
) -> Result<Option<Option<String>>, AppError> {
    let Some(t) = read_tx(conn, installation)? else {
        return Ok(None);
    };
    if t.buyer_user_id != buyer {
        return Err(AppError::Forbidden(
            "Client rental belongs to another buyer".into(),
        ));
    }
    if t.cleanup_confirmed_at.is_some() {
        return Ok(Some(t.cleanup_job_id));
    }
    if let Some(job) = t.cleanup_job_id {
        let running:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM provisioning_jobs WHERE id=?1 AND status IN ('pending','running','succeeded'))",params![job],|r|r.get(0))
            .map_err(db_error("read replayed Client cleanup job"))?;
        if running {
            return Ok(Some(Some(job)));
        }
    }
    Ok(None)
}
/// Prevent the shared finalizer from returning compensated Client rent in
/// full. Share contracts have no Client termination record and are untouched.
pub(crate) fn finalize_recorded_month_tx(
    conn: &Connection,
    contract: &str,
    at: &str,
) -> Result<Option<crate::market_platform_contracts::ContractTermination>, AppError> {
    let installation: Option<String> = conn
        .query_row(
            "SELECT installation_id FROM client_market_terminations
        WHERE contract_id=?1 AND period_id IS NOT NULL",
            params![contract],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error("find recorded Client monthly termination"))?;
    let Some(installation) = installation else {
        return Ok(None);
    };
    let before = read_tx(conn, &installation)?.unwrap();
    cleanup_confirmed_tx(conn, &installation, &before.host_id, at)?;
    let after = read_tx(conn, &installation)?.unwrap();
    Ok(Some(
        crate::market_platform_contracts::ContractTermination {
            refunded_units: after.returned_units - before.returned_units,
            unfunded_observations: 0,
            cleanup_confirmed: true,
            finalized: matches!(after.status.as_str(), "completed" | "disputed"),
        },
    ))
}

fn event_tx(
    conn: &Connection,
    id: &str,
    actor: Option<&str>,
    action: &str,
    detail: serde_json::Value,
    journal: Option<&str>,
    at: &str,
) -> Result<(), AppError> {
    conn.execute("INSERT INTO client_termination_events VALUES (?1,?2,(SELECT version FROM client_market_terminations WHERE id=?2),?3,?4,?5,?6,?7)",
        params![Uuid::new_v4().to_string(),id,actor,action,detail.to_string(),journal,at]).map_err(db_error("audit Client termination"))?;
    Ok(())
}

fn return_tx(
    conn: &Connection,
    t: &Termination,
    units: i64,
    liability: &str,
    subject: &str,
    action: &str,
    at: &str,
) -> Result<(), AppError> {
    if units == 0 {
        return Ok(());
    }
    let journal = credit_wallet_from_liability_tx(
        conn,
        &t.buyer_user_id,
        units,
        liability,
        subject,
        "client_termination",
        &t.id,
        &format!("{action}:{}", t.version),
        at,
    )?;
    if !journal.inserted {
        return Err(AppError::Internal(
            "Client termination return projection drift".into(),
        ));
    }
    conn.execute(
        "UPDATE client_market_terminations SET returned_units=returned_units+?2 WHERE id=?1",
        params![t.id, units],
    )
    .map_err(db_error("record Client wallet return"))?;
    if let Some(period) = t.period_id.as_deref() {
        conn.execute("UPDATE market_platform_periods SET refunded_units=refunded_units+?2,updated_at=?3 WHERE id=?1",params![period,units,at])
            .map_err(db_error("record Client period return"))?;
    }
    event_tx(
        conn,
        &t.id,
        None,
        action,
        json!({"amountUnits":units.to_string()}),
        Some(&journal.id),
        at,
    )
}
fn award_tx(conn: &Connection, t: &Termination, units: i64, at: &str) -> Result<(), AppError> {
    if units == 0 {
        return Ok(());
    }
    if units < 0
        || units > t.compensation_cap_units
        || units > compensation(t.period_amount_units, t.remaining_units, t.rate_bps)?
    {
        return Err(AppError::Internal(
            "Client compensation exceeds frozen agreement".into(),
        ));
    }
    let period = t
        .period_id
        .as_deref()
        .ok_or_else(|| AppError::Internal("compensation period missing".into()))?;
    let journal = post_journal_tx(
        conn,
        "client_termination",
        &t.id,
        "award",
        &format!("{period}:{}:{units}", t.provider_user_id),
        &[
            Posting {
                kind: "unfulfilled",
                subject_id: period,
                signed_units: units,
            },
            Posting {
                kind: "provider_pending",
                subject_id: &t.provider_user_id,
                signed_units: -units,
            },
        ],
        at,
    )?;
    if !journal.inserted {
        return Err(AppError::Internal("Client award projection drift".into()));
    }
    conn.execute("INSERT INTO client_termination_compensations (id,provider_user_id,amount_units,awarded_at,unlock_at,journal_id)
        VALUES (?1,?2,?3,?4,?5,?6)",params![t.id,t.provider_user_id,units,at,(time(at)?+Duration::days(7)).to_rfc3339(),journal.id])
        .map_err(db_error("record Client compensation award"))?;
    conn.execute(
        "UPDATE client_market_terminations SET compensation_units=?2 WHERE id=?1",
        params![t.id, units],
    )
    .map_err(db_error("record termination compensation"))?;
    conn.execute("UPDATE market_platform_periods SET termination_compensation_units=termination_compensation_units+?2,updated_at=?3 WHERE id=?1",
        params![period,units,at]).map_err(db_error("allocate Client period compensation"))?;
    event_tx(
        conn,
        &t.id,
        None,
        "awarded",
        json!({"amountUnits":units.to_string()}),
        Some(&journal.id),
        at,
    )
}

fn ensure_settlement_safe_tx(conn: &Connection, t: &Termination) -> Result<(), AppError> {
    if !evidence_valid_tx(conn, &t.id)? {
        return Err(AppError::Conflict(
            "Client termination evidence requires Owner review".into(),
        ));
    }
    crate::market_reconcile::ensure_finance_scopes_safe_tx(
        conn,
        &[
            crate::market_reconcile::FinanceScope::User(&t.buyer_user_id),
            crate::market_reconcile::FinanceScope::Provider(&t.provider_user_id),
            crate::market_reconcile::FinanceScope::User(&t.provider_user_id),
            crate::market_reconcile::FinanceScope::Contract(t.contract_id.as_deref().unwrap_or("")),
        ],
    )?;
    if t.remaining_units > 0 {
        crate::market_wallet::reconcile_wallet_tx(conn, &t.buyer_user_id)?;
    }
    Ok(())
}

/// A financial exception must not roll back a physically verified cleanup or
/// keep a clean Host unavailable. Retain the receipt and retry money in its
/// own savepoint; force-ending a rental never writes this receipt.
pub(crate) fn record_cleanup_confirmed_tx(
    conn: &Connection,
    installation: &str,
    host: &str,
    at: &str,
) -> Result<(), AppError> {
    let rental_host: Option<String> = conn
        .query_row(
            "SELECT host_id FROM client_market_subscriptions WHERE installation_id=?1",
            params![installation],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error("read verified Client cleanup binding"))?;
    let Some(rental_host) = rental_host else {
        return Ok(());
    };
    if rental_host != host {
        return Err(AppError::Conflict(
            "cleanup receipt Host does not match rental".into(),
        ));
    }
    conn.execute(
        "INSERT OR IGNORE INTO client_cleanup_receipts VALUES (?1,?2,?3)",
        params![installation, host, at],
    )
    .map_err(db_error("record verified Client cleanup receipt"))?;
    conn.execute_batch("SAVEPOINT client_cleanup_finance")
        .map_err(db_error("begin Client cleanup finance savepoint"))?;
    match cleanup_confirmed_tx(conn, installation, host, at) {
        Ok(()) => conn
            .execute_batch("RELEASE SAVEPOINT client_cleanup_finance")
            .map_err(db_error("commit Client cleanup finance savepoint"))?,
        Err(_) => {
            conn.execute_batch("ROLLBACK TO SAVEPOINT client_cleanup_finance; RELEASE SAVEPOINT client_cleanup_finance")
                .map_err(db_error("rollback Client cleanup finance savepoint"))?;
            conn.execute("UPDATE client_market_terminations SET cleanup_confirmed_at=COALESCE(cleanup_confirmed_at,?2),
                status='finance_pending',version=version+1,updated_at=?2 WHERE installation_id=?1",params![installation,at])
                .map_err(db_error("retain verified Client cleanup awaiting finance"))?;
            crate::market_reconcile::case_tx(
                conn,
                "client_cleanup_finalization_failed",
                "installation",
                installation,
                at,
            )?;
            crate::market_notifications::owner_signal_tx(
                conn,
                "client_cleanup_finalization_failed",
                "installation",
                installation,
                json!({"installationId":installation,"action":"review_financial_finalization"}),
                at,
                false,
            )?;
        }
    }
    Ok(())
}

/// Called only with verified cleanup evidence bound to this installation and
/// Host. Clearing the renter's creation gate is NOT cleanup evidence.
pub(crate) fn cleanup_confirmed_tx(
    conn: &Connection,
    installation: &str,
    host: &str,
    at: &str,
) -> Result<(), AppError> {
    if read_tx(conn, installation)?.is_none() {
        // Historical successful cleanup: no new compensation can be imposed.
        let cutoff: String = conn
            .query_row(
                "SELECT COALESCE((SELECT MIN(created_at) FROM provisioning_jobs
            WHERE installation_id=?1 AND host_id=?2 AND type='cleanup'),updated_at)
            FROM client_market_subscriptions WHERE installation_id=?1 AND host_id=?2",
                params![installation, host],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error("read historical Client cleanup"))?
            .ok_or_else(|| {
                AppError::Conflict("cleanup evidence does not match rental Host".into())
            })?;
        accept_tx(
            conn,
            installation,
            None,
            "historical_cleanup",
            None,
            &cutoff,
        )?;
    }
    let t = read_tx(conn, installation)?.unwrap();
    if t.host_id != host {
        return Err(AppError::Conflict(
            "cleanup Host does not match termination".into(),
        ));
    }
    if t.cleanup_confirmed_at.is_some() && t.status != "finance_pending" {
        return Ok(());
    }
    ensure_settlement_safe_tx(conn, &t)?;
    if let Some(c) = t.contract_id.as_deref() {
        let monthly = t.period_id.is_some();
        if !monthly {
            let result =
                crate::market_platform_contracts::terminate_after_cleanup_tx(conn, c, true, at)?;
            if !result.finalized {
                return Ok(());
            }
        } else {
            let current = basis_tx(conn, installation)?;
            if current.period != t.period_id || current.remaining != t.remaining_units {
                return Err(AppError::Conflict(
                    "Client termination liability changed; Owner review required".into(),
                ));
            }
            let hold = if t.responsibility == "undetermined" {
                t.compensation_cap_units
            } else {
                0
            };
            let award = if t.responsibility == "buyer" {
                t.compensation_cap_units
            } else {
                0
            };
            return_tx(
                conn,
                &t,
                t.remaining_units - hold - award,
                "unfulfilled",
                t.period_id.as_deref().unwrap(),
                "return_unused",
                at,
            )?;
            award_tx(conn, &t, award, at)?;
            conn.execute(
                "UPDATE client_market_terminations SET held_units=?2 WHERE id=?1",
                params![t.id, hold],
            )
            .map_err(db_error("hold only disputed Client compensation"))?;
            conn.execute("UPDATE market_platform_periods SET status='closed',settled_through_at=period_end,updated_at=?2 WHERE id=?1",
                params![t.period_id,at]).map_err(db_error("close terminated Client period"))?;
            conn.execute("UPDATE market_platform_contracts SET status='ended',ended_at=?2,updated_at=?3 WHERE id=?1",
                params![c,t.cutoff_at,at]).map_err(db_error("end terminated Client contract"))?;
        }
        conn.execute("UPDATE market_finance_reconciliation_cases SET status='resolved',resolved_at=?2
            WHERE object_kind='platform_contract' AND object_id=?1 AND code='wallet_cleanup_uncertain' AND status='open'",params![c,at])
            .map_err(db_error("close verified Client cleanup case"))?;
        crate::market_finance::release_earning_holds_tx(
            conn,
            "cleanup_uncertain",
            c,
            "system",
            "verified_client_cleanup",
            at,
        )?;
    }
    conn.execute("UPDATE client_market_terminations SET cleanup_confirmed_at=?2,status=CASE WHEN held_units>0
        THEN 'disputed' ELSE 'completed' END,version=version+1,updated_at=?2 WHERE id=?1",params![t.id,at])
        .map_err(db_error("confirm Client cleanup finance"))?;
    event_tx(
        conn,
        &t.id,
        None,
        "cleanup_confirmed",
        json!({"hostId":host}),
        None,
        at,
    )?;
    clear_cleanup_case_tx(conn, installation, at)?;
    notify_tx(conn, &by_id_tx(conn, &t.id)?, "settled", at)
}

fn clear_cleanup_case_tx(conn: &Connection, installation: &str, at: &str) -> Result<(), AppError> {
    let changed=conn.execute("UPDATE market_finance_reconciliation_cases SET status='resolved',resolved_at=?2
        WHERE code='client_cleanup_finalization_failed' AND object_kind='installation' AND object_id=?1 AND status='open'",params![installation,at])
        .map_err(db_error("close recovered Client cleanup case"))?;
    if changed > 0 {
        crate::market_notifications::owner_signal_tx(
            conn,
            "client_cleanup_finalization_failed",
            "installation",
            installation,
            json!({"installationId":installation}),
            at,
            true,
        )?;
    }
    Ok(())
}

pub(crate) fn cleanup_failed_tx(
    conn: &Connection,
    installation: &str,
    at: &str,
) -> Result<(), AppError> {
    conn.execute("UPDATE client_market_terminations SET status='cleanup_failed',version=version+1,updated_at=?2
        WHERE installation_id=?1 AND cleanup_confirmed_at IS NULL AND status!='cleanup_failed'",params![installation,at])
        .map_err(db_error("record Client cleanup failure"))?;
    Ok(())
}
fn notify_tx(conn: &Connection, t: &Termination, stage: &str, at: &str) -> Result<(), AppError> {
    if t.status != "completed" {
        return Ok(());
    }
    // Recovery must not replay a mailbox full of old free/trial terminations.
    if t.reason == "historical_cleanup" && t.returned_units == 0 && t.compensation_units == 0 {
        return Ok(());
    }
    for (role, user) in [
        ("buyer", &t.buyer_user_id),
        ("provider", &t.provider_user_id),
    ] {
        if role == "provider" && user == &t.buyer_user_id {
            continue;
        }
        crate::market_notifications::enqueue_user_notification_tx(
            conn,
            role,
            user,
            "client_termination_settled",
            "client_termination",
            &t.id,
            &format!("client_termination:{}:{stage}:{role}", t.id),
            json!({"terminationId":t.id,"role":role,"status":t.status,
                "amountUnits":if role=="buyer" {t.returned_units} else {t.compensation_units}.to_string(),
                "heldUnits":t.held_units.to_string(),"recordedAt":at,"clientReference":t.client_subdomain.as_deref().unwrap_or(&t.installation_id),
                "walletReturnUnits":t.returned_units.to_string(),"compensationUnits":t.compensation_units.to_string()}),
            at,
        )?;
    }
    Ok(())
}

/// Only financial milestones notify the parties. Reading, chatting and worker
/// retries never produce another financial email, nor publish private claims.
fn notify_claim_tx(
    conn: &Connection,
    t: &Termination,
    stage: &str,
    at: &str,
) -> Result<(), AppError> {
    for (role, user) in [
        ("buyer", &t.buyer_user_id),
        ("provider", &t.provider_user_id),
    ] {
        if role == "provider" && user == &t.buyer_user_id {
            continue;
        }
        crate::market_notifications::enqueue_user_notification_tx(
            conn,
            role,
            user,
            "client_termination_claim",
            "client_termination",
            &t.id,
            &format!("client_termination:{}:claim:{stage}:{role}", t.id),
            json!({"terminationId":t.id,"role":role,"action":stage,
                "clientReference":t.client_subdomain.as_deref().unwrap_or(&t.installation_id),
                "responseDeadline":t.dispute_deadline_at,"recordedAt":at}),
            at,
        )?;
    }
    Ok(())
}

fn remaining_compensation(t: &Termination) -> i64 {
    if t.cleanup_confirmed_at.is_none() || t.status == "finance_pending" {
        t.compensation_cap_units
    } else if t.compensation_units > 0 {
        t.compensation_units
    } else {
        t.held_units
    }
}
fn timeout_due(t: &Termination, at: &str) -> Result<bool, AppError> {
    let now = time(at)?;
    Ok(t.responsibility == "undetermined"
        && t.dispute_used
        && t.provider_responded_at.is_none()
        && t.dispute_opened_at
            .as_deref()
            .map(time)
            .transpose()?
            .is_some_and(|opened| now >= opened + Duration::days(3)))
}

enum DecisionActor<'a> {
    Owner(&'a str),
    Provider(&'a str),
    BuyerWithdrawal(&'a str),
    Timeout,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Decision {
    expected_version: i64,
    award_units: String,
    responsibility: String,
    note: String,
}
fn decide_tx(
    conn: &Connection,
    id: &str,
    actor: &str,
    owner: bool,
    input: &Decision,
    at: &str,
) -> Result<Termination, AppError> {
    apply_decision_tx(
        conn,
        id,
        if owner {
            DecisionActor::Owner(actor)
        } else {
            DecisionActor::Provider(actor)
        },
        input,
        at,
    )
}
fn apply_decision_tx(
    conn: &Connection,
    id: &str,
    authority: DecisionActor<'_>,
    input: &Decision,
    at: &str,
) -> Result<Termination, AppError> {
    let t = by_id_tx(conn, id)?;
    let (actor, action) = match authority {
        DecisionActor::Owner(user) => (Some(user), "owner_decision"),
        DecisionActor::Provider(user) if t.provider_user_id == user => {
            (Some(user), "provider_waiver")
        }
        DecisionActor::BuyerWithdrawal(user) if t.buyer_user_id == user => {
            (Some(user), "buyer_withdrawal")
        }
        DecisionActor::Timeout => (None, "timeout_waiver"),
        _ => {
            return Err(AppError::Forbidden(
                "not a party allowed to decide this compensation".into(),
            ));
        }
    };
    let provider = action == "provider_waiver";
    let withdrawal = action == "buyer_withdrawal";
    let automatic = action == "timeout_waiver";
    let due = timeout_due(&t, at)?;
    if automatic != due {
        return Err(AppError::Conflict(
            "the Provider response deadline requires automatic waiver; reload".into(),
        ));
    }
    if (withdrawal || automatic) && (t.responsibility != "undetermined" || !t.dispute_used) {
        return Err(AppError::Conflict(
            "this compensation claim is already closed".into(),
        ));
    }
    if t.status == "finance_pending" {
        return Err(AppError::Conflict(
            "repair financial evidence before deciding compensation".into(),
        ));
    }
    if t.version != input.expected_version {
        return Err(AppError::Conflict(
            "termination changed; reload before deciding".into(),
        ));
    }
    if input.note.trim().chars().count() < 3 || input.note.len() > 2000 {
        return Err(AppError::BadRequest(
            "an audit explanation is required".into(),
        ));
    }
    if !matches!(
        input.responsibility.as_str(),
        "buyer" | "provider" | "platform" | "none"
    ) {
        return Err(AppError::BadRequest("invalid responsibility".into()));
    }
    let award = input
        .award_units
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest("invalid compensation units".into()))?;
    let current = remaining_compensation(&t);
    if current == 0 && t.cleanup_confirmed_at.is_some() {
        return Err(AppError::Conflict(
            "no unsettled compensation remains".into(),
        ));
    }
    if award < 0
        || award > current
        || (matches!(
            input.responsibility.as_str(),
            "provider" | "platform" | "none"
        ) && award > 0)
        || (provider && award == current && current > 0)
        || (withdrawal && (award != current || input.responsibility != "buyer"))
        || (automatic && award != 0)
    {
        return Err(AppError::BadRequest(
            "a decision may only reduce compensation within the agreed cap".into(),
        ));
    }
    if t.credited_units > 0 {
        return Err(AppError::Conflict(
            "compensation already credited; the settlement window has ended".into(),
        ));
    }
    ensure_settlement_safe_tx(conn, &t)?;
    // Provider waivers can reduce a claim, but cannot settle the buyer's
    // responsibility dispute for a nonzero amount without Owner adjudication.
    let still_disputed = provider && t.responsibility == "undetermined" && award > 0;
    let responsibility = if provider {
        if still_disputed {
            "undetermined"
        } else if award == 0 {
            "provider"
        } else {
            t.responsibility.as_str()
        }
    } else {
        input.responsibility.as_str()
    };
    if t.cleanup_confirmed_at.is_none() {
        conn.execute(
            "UPDATE client_market_terminations SET compensation_cap_units=?2,responsibility=?3,
            version=version+1,updated_at=?4 WHERE id=?1",
            params![id, award, responsibility, at],
        )
        .map_err(db_error("record pre-cleanup Client decision"))?;
    } else {
        let difference = current - award;
        // Release projection before the matching wallet credit, retaining the
        // original immutable award and appending a reversal journal.
        if t.compensation_units > 0 {
            let maturity = t
                .unlock_at
                .as_deref()
                .ok_or_else(|| AppError::Internal("award maturity missing".into()))?;
            if time(at)? >= time(maturity)? && t.status != "disputed" {
                return Err(AppError::Conflict(
                    "compensation settlement window has ended".into(),
                ));
            }
            conn.execute("UPDATE client_termination_compensations SET reversed_units=reversed_units+?2,disputed=?3 WHERE id=?1",
                params![id,difference,i64::from(still_disputed)]).map_err(db_error("reduce Client award"))?;
            conn.execute(
                "UPDATE client_market_terminations SET compensation_units=?2 WHERE id=?1",
                params![id, award],
            )
            .map_err(db_error("reduce termination award projection"))?;
            if let Some(period) = t.period_id.as_deref() {
                conn.execute("UPDATE market_platform_periods SET termination_compensation_units=termination_compensation_units-?2 WHERE id=?1",params![period,difference])
                    .map_err(db_error("reverse Client period compensation"))?;
            }
            return_tx(
                conn,
                &t,
                difference,
                "provider_pending",
                &t.provider_user_id,
                "return_compensation",
                at,
            )?;
        } else {
            conn.execute(
                "UPDATE client_market_terminations SET held_units=0 WHERE id=?1",
                params![id],
            )
            .map_err(db_error("release Client compensation hold"))?;
            return_tx(
                conn,
                &t,
                difference,
                "unfulfilled",
                t.period_id.as_deref().unwrap_or(""),
                "return_disputed",
                at,
            )?;
            if still_disputed {
                conn.execute(
                    "UPDATE client_market_terminations SET held_units=?2 WHERE id=?1",
                    params![id, award],
                )
                .map_err(db_error("retain reduced Client dispute"))?;
            } else {
                award_tx(conn, &t, award, at)?;
            }
        }
        conn.execute("UPDATE client_market_terminations SET responsibility=?2,status=?3,version=version+1,updated_at=?4 WHERE id=?1",
            params![id,responsibility,if still_disputed {"disputed"} else {"completed"},at]).map_err(db_error("finalize Client responsibility decision"))?;
    }
    if provider && t.responsibility == "undetermined" {
        conn.execute("UPDATE client_market_terminations SET provider_responded_at=COALESCE(provider_responded_at,?2) WHERE id=?1",
            params![id,at]).map_err(db_error("record Provider compensation response"))?;
    }
    event_tx(
        conn,
        id,
        actor,
        action,
        json!({"awardUnits":input.award_units,"responsibility":responsibility,"note":input.note}),
        None,
        at,
    )?;
    let result = by_id_tx(conn, id)?;
    if result.responsibility != "undetermined" {
        if t.responsibility == "undetermined" {
            // Keep the completed discussion available for the normal retention
            // window, even if Owner adjudication took longer than that window.
            conn.execute("UPDATE chat_rooms SET delete_after=?2 WHERE installation_id=?1 AND status='archived'",
                params![t.installation_id,(time(at)?+Duration::seconds(crate::client_chat::CHAT_ARCHIVE_RETENTION_SECS)).to_rfc3339()])
                .map_err(db_error("retain concluded termination chat history"))?;
        }
        crate::market_notifications::owner_signal_tx(
            conn,
            "client_compensation_dispute",
            "client_termination",
            id,
            json!({"terminationId":id}),
            at,
            true,
        )?;
        let closed=conn.execute("UPDATE market_finance_reconciliation_cases SET status='resolved',resolved_at=?2
            WHERE code='client_compensation_dispute_overdue' AND object_kind='client_termination' AND object_id=?1 AND status='open'",params![id,at])
            .map_err(db_error("close decided Client compensation reminder"))?;
        if closed > 0 {
            crate::market_notifications::owner_signal_tx(
                conn,
                "client_compensation_dispute_overdue",
                "client_termination",
                id,
                json!({"terminationId":id}),
                at,
                true,
            )?;
        }
    }
    if result.cleanup_confirmed_at.is_some() && !still_disputed {
        notify_tx(conn, &result, &format!("decision:{}", result.version), at)?;
    }
    if still_disputed && t.provider_responded_at.is_none() {
        notify_claim_tx(conn, &result, "responded", at)?;
    }
    Ok(result)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VersionedAction {
    expected_version: i64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderResponse {
    expected_version: i64,
    note: String,
}

fn respond_tx(
    conn: &Connection,
    id: &str,
    provider: &str,
    input: &ProviderResponse,
    at: &str,
) -> Result<Termination, AppError> {
    let t = by_id_tx(conn, id)?;
    if t.provider_user_id != provider {
        return Err(AppError::Forbidden("only the Provider may respond".into()));
    }
    if t.version != input.expected_version
        || t.responsibility != "undetermined"
        || !t.dispute_used
        || t.provider_responded_at.is_some()
        || remaining_compensation(&t) == 0
        || timeout_due(&t, at)?
    {
        return Err(AppError::Conflict(
            "claim changed, closed or the response deadline has passed; reload".into(),
        ));
    }
    if input.note.trim().chars().count() < 3 || input.note.len() > 2000 {
        return Err(AppError::BadRequest(
            "an audit explanation is required".into(),
        ));
    }
    // A response is not an award or a finding against the buyer. Funds remain
    // held until the buyer withdraws or the Owner makes an explicit decision.
    conn.execute("UPDATE client_market_terminations SET provider_responded_at=?2,version=version+1,updated_at=?2 WHERE id=?1",
        params![id,at]).map_err(db_error("record formal Provider response"))?;
    event_tx(
        conn,
        id,
        Some(provider),
        "provider_response",
        json!({"note":input.note}),
        None,
        at,
    )?;
    let result = by_id_tx(conn, id)?;
    notify_claim_tx(conn, &result, "responded", at)?;
    Ok(result)
}
fn withdraw_tx(
    conn: &Connection,
    id: &str,
    buyer: &str,
    input: &VersionedAction,
    at: &str,
) -> Result<Termination, AppError> {
    let t = by_id_tx(conn, id)?;
    apply_decision_tx(
        conn,
        id,
        DecisionActor::BuyerWithdrawal(buyer),
        &Decision {
            expected_version: input.expected_version,
            award_units: remaining_compensation(&t).to_string(),
            responsibility: "buyer".into(),
            note:
                "Buyer voluntarily ended the claim and accepted the remaining Provider compensation"
                    .into(),
        },
        at,
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Dispute {
    expected_version: i64,
    #[serde(default)]
    claim: String,
}
fn dispute_tx(
    conn: &Connection,
    id: &str,
    buyer: &str,
    input: &Dispute,
    at: &str,
) -> Result<Termination, AppError> {
    let t = by_id_tx(conn, id)?;
    if t.buyer_user_id != buyer {
        return Err(AppError::Forbidden(
            "termination belongs to another buyer".into(),
        ));
    }
    if t.status == "finance_pending" {
        return Err(AppError::Conflict(
            "financial settlement requires Owner review".into(),
        ));
    }
    if t.version != input.expected_version
        || t.dispute_used
        || t.compensation_cap_units == 0
        || t.credited_units > 0
    {
        return Err(AppError::Conflict(
            "compensation cannot be disputed in its current state".into(),
        ));
    }
    if input.claim.len() > 2000 {
        return Err(AppError::BadRequest(
            "explanation exceeds 2000 bytes".into(),
        ));
    }
    if let Some(unlock) = t.unlock_at.as_deref() {
        if time(at)? >= time(unlock)? {
            return Err(AppError::Conflict(
                "compensation settlement window has ended".into(),
            ));
        }
    }
    if t.cleanup_confirmed_at.is_some() && t.compensation_units == 0 {
        return Err(AppError::Conflict(
            "no compensation remains to dispute".into(),
        ));
    }
    conn.execute("UPDATE client_market_terminations SET responsibility='undetermined',claim=?2,dispute_used=1,
        dispute_opened_at=?3,provider_responded_at=NULL,status=CASE WHEN cleanup_confirmed_at IS NULL THEN status ELSE 'disputed' END,
        version=version+1,updated_at=?3 WHERE id=?1",params![id,input.claim,at]).map_err(db_error("open Client compensation dispute"))?;
    conn.execute(
        "UPDATE client_termination_compensations SET disputed=1 WHERE id=?1",
        params![id],
    )
    .map_err(db_error("hold disputed Client award"))?;
    event_tx(
        conn,
        id,
        Some(buyer),
        "disputed",
        json!({"claim":input.claim}),
        None,
        at,
    )?;
    crate::market_notifications::owner_signal_tx(
        conn,
        "client_compensation_dispute",
        "client_termination",
        id,
        json!({"terminationId":id,"action":"review_financial_responsibility"}),
        at,
        false,
    )?;
    let result = by_id_tx(conn, id)?;
    crate::store::client_chat::ensure_termination_chat_tx(conn, &t.installation_id, at)?;
    notify_claim_tx(conn, &result, "opened", at)?;
    by_id_tx(conn, id)
}

async fn session(state: &ServerState, headers: &HeaderMap) -> Result<AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))
}
pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/client-market/termination-policy", get(get_policy))
        .route(
            "/v1/admin/market/client-termination-policy",
            get(get_admin_policy).put(set_policy),
        )
        .route(
            "/v1/client-market/clients/:id/termination-preview",
            post(preview),
        )
        .route("/v1/client-market/clients/:id/termination", get(detail))
        .route("/v1/client-market/terminations", get(list))
        .route("/v1/client-market/terminations/:id/waive", post(waive))
        .route("/v1/client-market/terminations/:id/dispute", post(dispute))
        .route("/v1/client-market/terminations/:id/respond", post(respond))
        .route(
            "/v1/client-market/terminations/:id/withdraw",
            post(withdraw),
        )
        .route("/v1/admin/market/client-terminations", get(admin_list))
        .route(
            "/v1/admin/market/client-terminations/:id/resolve",
            post(resolve),
        )
}
async fn get_policy(
    State(state): State<ServerState>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let conn = state.store.conn.lock().await;
    Ok((no_store(), Json(policy_tx(&conn)?)))
}
async fn get_admin_policy(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = crate::api::require_admin_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let p = policy_tx(&conn)?;
    Ok((
        no_store(),
        Json(
            json!({"version":p.version,"rateBps":p.rate_bps,"canEdit":crate::api::is_finance_owner(&state.config,&actor.email)}),
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
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<SetPolicy>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = crate::api::require_owner_session(&state, &headers).await?;
    if !(0..=10000).contains(&input.rate_bps) {
        return Err(AppError::BadRequest(
            "rate must be between 0 and 100 percent".into(),
        ));
    }
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin Client termination policy"))?;
    if policy_tx(&tx)?.version != input.expected_version {
        return Err(AppError::Conflict(
            "policy changed; reload before saving".into(),
        ));
    }
    let version = input
        .expected_version
        .checked_add(1)
        .ok_or_else(|| AppError::Internal("policy version overflow".into()))?;
    tx.execute(
        "INSERT INTO client_termination_policies VALUES (?1,?2,?3,?4)",
        params![
            version,
            input.rate_bps,
            actor.user_id,
            Utc::now().to_rfc3339()
        ],
    )
    .map_err(db_error("write Client termination policy"))?;
    tx.commit()
        .map_err(db_error("commit Client termination policy"))?;
    Ok((
        no_store(),
        Json(Policy {
            version,
            rate_bps: input.rate_bps,
        }),
    ))
}
async fn preview(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin Client termination preview"))?;
    let view = preview_tx(&tx, &id, &actor.user_id, &Utc::now().to_rfc3339())?;
    tx.commit()
        .map_err(db_error("commit Client termination preview"))?;
    Ok((no_store(), Json(view)))
}
async fn detail(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let b = basis_tx(&conn, &id)?;
    if b.buyer != actor.user_id
        && b.provider != actor.user_id
        && !crate::api::is_finance_owner(&state.config, &actor.email)
    {
        return Err(AppError::Forbidden(
            "not allowed to view this termination".into(),
        ));
    }
    Ok((no_store(), Json(read_tx(&conn, &id)?)))
}
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    cursor: Option<String>,
    #[serde(default)]
    provider: bool,
    #[serde(default)]
    open_only: bool,
}
fn list_tx(
    conn: &Connection,
    user: Option<&str>,
    q: &ListQuery,
) -> Result<serde_json::Value, AppError> {
    let cursor = q.cursor.as_deref();
    let cursor_at = cursor
        .map(|id| {
            conn.query_row(
                "SELECT cutoff_at FROM client_market_terminations WHERE id=?1
        AND (?2 IS NULL OR (?3=0 AND buyer_user_id=?2) OR (?3=1 AND provider_user_id=?2))",
                params![id, user, q.provider],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error("read Client termination cursor"))?
            .ok_or_else(|| AppError::BadRequest("invalid termination cursor".into()))
        })
        .transpose()?;
    let ids = {
        let mut s = conn
            .prepare(
                "SELECT installation_id FROM client_market_terminations
        WHERE (?1 IS NULL OR (?2=0 AND buyer_user_id=?1) OR (?2=1 AND provider_user_id=?1))
        AND (?4 IS NULL OR cutoff_at<?4 OR (cutoff_at=?4 AND id<?3))
        AND (?5=0 OR status!='completed') ORDER BY cutoff_at DESC,id DESC LIMIT 51",
            )
            .map_err(db_error("prepare Client terminations"))?;
        s.query_map(
            params![user, q.provider, cursor, cursor_at, q.open_only],
            |r| r.get::<_, String>(0),
        )
        .map_err(db_error("query Client terminations"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read Client terminations"))?
    };
    let mut items = Vec::new();
    for id in ids.iter().take(50) {
        if let Some(t) = read_tx(conn, id)? {
            items.push(t);
        }
    }
    let cursor = if ids.len() > 50 {
        items.last().map(|t| t.id.clone())
    } else {
        None
    };
    Ok(json!({"items":items,"nextCursor":cursor}))
}
async fn list(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((no_store(), Json(list_tx(&conn, Some(&actor.user_id), &q)?)))
}
async fn admin_list(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::api::require_owner_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((no_store(), Json(list_tx(&conn, None, &q)?)))
}
async fn waive(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<Decision>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin Client waiver"))?;
    let view = decide_tx(
        &tx,
        &id,
        &actor.user_id,
        false,
        &input,
        &Utc::now().to_rfc3339(),
    )?;
    tx.commit().map_err(db_error("commit Client waiver"))?;
    Ok((no_store(), Json(view)))
}
async fn resolve(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<Decision>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = crate::api::require_owner_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin Client decision"))?;
    let view = decide_tx(
        &tx,
        &id,
        &actor.user_id,
        true,
        &input,
        &Utc::now().to_rfc3339(),
    )?;
    tx.commit().map_err(db_error("commit Client decision"))?;
    Ok((no_store(), Json(view)))
}
async fn dispute(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<Dispute>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin Client compensation dispute"))?;
    let view = dispute_tx(&tx, &id, &actor.user_id, &input, &Utc::now().to_rfc3339())?;
    tx.commit()
        .map_err(db_error("commit Client compensation dispute"))?;
    Ok((no_store(), Json(view)))
}
async fn respond(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ProviderResponse>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin Client Provider response"))?;
    let view = respond_tx(&tx, &id, &actor.user_id, &input, &Utc::now().to_rfc3339())?;
    tx.commit()
        .map_err(db_error("commit Client Provider response"))?;
    Ok((no_store(), Json(view)))
}
async fn withdraw(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<VersionedAction>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin Client claim withdrawal"))?;
    let view = withdraw_tx(&tx, &id, &actor.user_id, &input, &Utc::now().to_rfc3339())?;
    tx.commit()
        .map_err(db_error("commit Client claim withdrawal"))?;
    Ok((no_store(), Json(view)))
}

/// A contractual waiver for a missed response deadline, not technical proof of
/// Provider fault. Fair, bounded retries never let a corrupt record starve the
/// rest. Cleanup and ledger validation remain mandatory for actual credits.
pub(crate) fn expire_disputes_isolated(
    conn: &Connection,
    at: &str,
    limit: usize,
) -> Result<usize, AppError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin Client dispute timeout"))?;
    let cursor: String = tx
        .query_row(
            "SELECT client_dispute_timeout_cursor FROM market_finance_control WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(db_error("read Client timeout cursor"))?;
    let ids = {
        let mut s = tx
            .prepare(
                "SELECT id FROM client_market_terminations WHERE responsibility='undetermined'
            AND dispute_used=1 AND provider_responded_at IS NULL AND dispute_opened_at IS NOT NULL
            AND julianday(dispute_opened_at)<=julianday(?1)-3 AND id>?2 ORDER BY id LIMIT ?3",
            )
            .map_err(db_error("prepare expired Client claims"))?;
        s.query_map(params![at, cursor, limit.clamp(1, 200) as i64], |r| {
            r.get::<_, String>(0)
        })
        .map_err(db_error("query expired Client claims"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read expired Client claims"))?
    };
    let mut count = 0;
    for id in &ids {
        tx.execute_batch("SAVEPOINT client_dispute_timeout")
            .map_err(db_error("begin Client timeout savepoint"))?;
        let result = (|| {
            let t = by_id_tx(&tx, id)?;
            apply_decision_tx(&tx,id,DecisionActor::Timeout,&Decision {
                expected_version:t.version,award_units:"0".into(),responsibility:"none".into(),
                note:"Compensation waived automatically: no formal Provider response within 72 hours".into(),
            },at)
        })();
        match result {
            Ok(_) => {
                tx.execute_batch("RELEASE SAVEPOINT client_dispute_timeout")
                    .map_err(db_error("commit Client timeout savepoint"))?;
                count += 1;
            }
            Err(_) => {
                tx.execute_batch("ROLLBACK TO SAVEPOINT client_dispute_timeout; RELEASE SAVEPOINT client_dispute_timeout")
                    .map_err(db_error("rollback Client timeout savepoint"))?;
                tracing::warn!(termination_id=%id,"Client automatic compensation waiver deferred; funds unchanged");
            }
        }
    }
    tx.execute(
        "UPDATE market_finance_control SET client_dispute_timeout_cursor=?1 WHERE id=1",
        params![ids.last().map(String::as_str).unwrap_or("")],
    )
    .map_err(db_error("advance Client timeout cursor"))?;
    tx.commit()
        .map_err(db_error("commit Client dispute timeouts"))?;
    Ok(count)
}

/// Independent evidence check used before credit and by the finance audit.
/// The unfulfilled period audit also checks compensation separately from rent.
pub(crate) fn evidence_valid_tx(conn: &Connection, id: &str) -> Result<bool, AppError> {
    conn.query_row("SELECT t.returned_units=COALESCE((SELECT -SUM(p.signed_units)
            FROM market_journal_transactions j JOIN market_journal_postings p ON p.journal_id=j.id
            WHERE j.source_kind='client_termination' AND j.source_id=t.id
              AND p.account_kind='wallet_available' AND p.subject_id=t.buyer_user_id),0)
        AND t.compensation_units=COALESCE(c.amount_units-c.reversed_units,0)
        AND (t.period_id IS NULL OR EXISTS(SELECT 1 FROM market_platform_periods p
            WHERE p.id=t.period_id AND p.contract_id=t.contract_id AND p.amount_units=t.period_amount_units
              AND p.amount_units-p.fulfilled_units-p.refunded_units+t.returned_units=t.remaining_units
              AND p.termination_compensation_units=t.compensation_units))
        AND (t.policy_version=0 AND t.rate_bps=0 OR EXISTS(SELECT 1 FROM client_termination_period_terms terms
            WHERE terms.period_id=t.period_id AND terms.policy_version=t.policy_version AND terms.rate_bps=t.rate_bps))
        AND (t.cleanup_confirmed_at IS NULL OR t.status='finance_pending' OR t.returned_units+t.compensation_units+t.held_units=t.remaining_units)
        AND (c.id IS NULL OR (c.amount_units<=t.compensation_cap_units
            AND c.disputed=CASE WHEN t.status='disputed' THEN 1 ELSE 0 END
            AND COALESCE(julianday(c.unlock_at)=julianday(c.awarded_at)+7,0)
            AND c.reversed_units=COALESCE((SELECT SUM(p.signed_units) FROM market_journal_transactions j
                JOIN market_journal_postings p ON p.journal_id=j.id
                WHERE j.source_kind='client_termination' AND j.source_id=t.id AND j.action LIKE 'return_compensation:%'
                AND p.account_kind='provider_pending' AND p.subject_id=t.provider_user_id),0)
            AND c.provider_user_id=t.provider_user_id
            AND EXISTS(SELECT 1 FROM market_journal_transactions j JOIN market_journal_seals s ON s.journal_id=j.id
                WHERE j.id=c.journal_id AND j.source_kind='client_termination' AND j.source_id=t.id AND j.action='award')
            AND c.amount_units=COALESCE((SELECT -SUM(signed_units) FROM market_journal_postings
                WHERE journal_id=c.journal_id AND account_kind='provider_pending' AND subject_id=t.provider_user_id),0)
            AND c.amount_units=COALESCE((SELECT SUM(signed_units) FROM market_journal_postings
                WHERE journal_id=c.journal_id AND account_kind='unfulfilled' AND subject_id=t.period_id),0)
            AND ((c.credited_units=0 AND c.credit_journal_id IS NULL)
                OR (c.credited_units>0 AND EXISTS(SELECT 1 FROM market_journal_transactions j
                    JOIN market_journal_seals s ON s.journal_id=j.id
                    JOIN client_termination_events e ON e.journal_id=j.id AND e.termination_id=t.id
                    WHERE j.id=c.credit_journal_id AND j.source_kind='client_compensation'
                        AND j.source_id=t.id AND j.action='automatic_credit')))
            AND c.credited_units=COALESCE((SELECT -SUM(signed_units) FROM market_journal_postings
                WHERE journal_id=c.credit_journal_id AND account_kind='wallet_available' AND subject_id=t.provider_user_id),0)
            AND c.credited_units=COALESCE((SELECT SUM(signed_units) FROM market_journal_postings
                WHERE journal_id=c.credit_journal_id AND account_kind='provider_pending' AND subject_id=t.provider_user_id),0)))
        FROM client_market_terminations t LEFT JOIN client_termination_compensations c ON c.id=t.id WHERE t.id=?1",
        params![id],|r|r.get(0)).optional().map_err(db_error("verify Client termination evidence")).map(|v|v.unwrap_or(false))
}

/// Fair bounded credit, separate from delivery lots. No unverified claim can
/// mature by timeout. Each item has its own savepoint and persisted scan cursor.
pub(crate) fn credit_due_isolated(
    conn: &Connection,
    at: &str,
    limit: i64,
) -> Result<usize, AppError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin Client compensation credit"))?;
    let cursor: String = tx
        .query_row(
            "SELECT client_compensation_cursor FROM market_finance_control WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(db_error("read Client compensation cursor"))?;
    let ids = {
        let mut s=tx.prepare("SELECT id FROM client_termination_compensations WHERE id>?1 AND disputed=0
        AND amount_units-reversed_units-credited_units>0 AND julianday(unlock_at)<=julianday(?2) ORDER BY id LIMIT ?3")
        .map_err(db_error("prepare Client compensation credits"))?;
        s.query_map(params![cursor, at, limit.clamp(1, 200)], |r| {
            r.get::<_, String>(0)
        })
        .map_err(db_error("query Client compensation credits"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read Client compensation credits"))?
    };
    let mut count = 0;
    for id in &ids {
        tx.execute_batch("SAVEPOINT client_compensation_credit")
            .map_err(db_error("begin Client credit savepoint"))?;
        let result = (|| {
            let t = by_id_tx(&tx, id)?;
            if t.status != "completed"
                || !crate::market_provider_finance::can_manage_earnings_tx(
                    &tx,
                    &t.provider_user_id,
                )?
            {
                return Ok(false);
            }
            if !evidence_valid_tx(&tx, id)? {
                return Err(AppError::Internal(
                    "Client compensation evidence requires review".into(),
                ));
            }
            crate::market_reconcile::ensure_finance_scopes_safe_tx(
                &tx,
                &[
                    crate::market_reconcile::FinanceScope::Provider(&t.provider_user_id),
                    crate::market_reconcile::FinanceScope::User(&t.provider_user_id),
                    crate::market_reconcile::FinanceScope::Contract(
                        t.contract_id.as_deref().unwrap_or(""),
                    ),
                ],
            )?;
            crate::market_wallet::reconcile_wallet_tx(&tx, &t.provider_user_id)?;
            let journal = credit_wallet_from_liability_tx(
                &tx,
                &t.provider_user_id,
                t.compensation_units,
                "provider_pending",
                &t.provider_user_id,
                "client_compensation",
                id,
                "automatic_credit",
                at,
            )?;
            if !journal.inserted {
                return Err(AppError::Internal(
                    "Client compensation credit projection drift".into(),
                ));
            }
            tx.execute("UPDATE client_termination_compensations SET credited_units=?2,credit_journal_id=?3 WHERE id=?1 AND credited_units=0",
                params![id,t.compensation_units,journal.id]).map_err(db_error("record Client compensation credit"))?;
            event_tx(
                &tx,
                id,
                None,
                "automatic_credit",
                json!({"amountUnits":t.compensation_units.to_string()}),
                Some(&journal.id),
                at,
            )?;
            Ok::<_, AppError>(true)
        })();
        match result {
            Ok(done) => {
                tx.execute_batch("RELEASE SAVEPOINT client_compensation_credit")
                    .map_err(db_error("commit Client credit savepoint"))?;
                count += usize::from(done);
            }
            Err(_) => {
                tx.execute_batch("ROLLBACK TO SAVEPOINT client_compensation_credit; RELEASE SAVEPOINT client_compensation_credit")
                .map_err(db_error("rollback Client credit savepoint"))?;
                tracing::warn!(termination_id=%id,"Client compensation credit deferred; funds unchanged");
            }
        }
    }
    tx.execute(
        "UPDATE market_finance_control SET client_compensation_cursor=?1 WHERE id=1",
        params![ids.last().map(String::as_str).unwrap_or("")],
    )
    .map_err(db_error("advance Client compensation cursor"))?;
    tx.commit()
        .map_err(db_error("commit Client compensation credits"))?;
    Ok(count)
}

/// Recover financial finalization from durable successful cleanup jobs, never
/// from a Host's present state (it may already belong to a different renter).
pub(crate) fn recover_cleanup_isolated(conn: &Connection, at: &str) -> Result<usize, AppError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin Client cleanup recovery"))?;
    let cursor: String = tx
        .query_row(
            "SELECT client_cleanup_finance_cursor FROM market_finance_control WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(db_error("read Client cleanup recovery cursor"))?;
    let rows = {
        let mut s=tx.prepare("SELECT s.installation_id,s.host_id FROM client_market_subscriptions s
        LEFT JOIN provisioning_jobs j ON j.installation_id=s.installation_id AND j.host_id=s.host_id
          AND j.type='cleanup' AND j.status='succeeded'
        LEFT JOIN client_cleanup_receipts r ON r.installation_id=s.installation_id AND r.host_id=s.host_id
        LEFT JOIN client_market_terminations t ON t.installation_id=s.installation_id
        WHERE s.status='released' AND (r.installation_id IS NOT NULL OR j.id IS NOT NULL)
          AND (t.cleanup_confirmed_at IS NULL OR t.status='finance_pending') AND s.installation_id>?1
        GROUP BY s.installation_id,s.host_id ORDER BY s.installation_id LIMIT 50")
        .map_err(db_error("prepare Client cleanup recovery"))?;
        s.query_map(params![cursor], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(db_error("query Client cleanup recovery"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read Client cleanup recovery"))?
    };
    let mut count = 0;
    let next_cursor = rows.last().map(|r| r.0.clone()).unwrap_or_default();
    for (installation, host) in rows {
        tx.execute_batch("SAVEPOINT client_cleanup_recovery")
            .map_err(db_error("begin Client recovery savepoint"))?;
        match cleanup_confirmed_tx(&tx, &installation, &host, at) {
            Ok(()) => {
                tx.execute_batch("RELEASE SAVEPOINT client_cleanup_recovery")
                    .map_err(db_error("commit Client recovery savepoint"))?;
                count += 1;
                clear_cleanup_case_tx(&tx, &installation, at)?;
            }
            Err(_) => {
                tx.execute_batch("ROLLBACK TO SAVEPOINT client_cleanup_recovery; RELEASE SAVEPOINT client_cleanup_recovery")
                    .map_err(db_error("rollback Client recovery savepoint"))?;
                crate::market_reconcile::case_tx(
                    &tx,
                    "client_cleanup_finalization_failed",
                    "installation",
                    &installation,
                    at,
                )?;
                // Rotate an existing bad record so it cannot starve good ones.
                tx.execute(
                    "UPDATE client_market_terminations SET updated_at=?2 WHERE installation_id=?1",
                    params![installation, at],
                )
                .map_err(db_error("rotate deferred Client cleanup"))?;
            }
        }
    }
    tx.execute(
        "UPDATE market_finance_control SET client_cleanup_finance_cursor=?1 WHERE id=1",
        params![next_cursor],
    )
    .map_err(db_error("advance Client cleanup recovery cursor"))?;
    // A timely formal response leaves the remaining dispute to the Owner.
    // Unanswered claims are waived by the separate timeout worker instead.
    let overdue: Vec<String> = {
        let mut s=tx.prepare("SELECT id FROM client_market_terminations
        WHERE responsibility='undetermined' AND dispute_opened_at IS NOT NULL AND provider_responded_at IS NOT NULL
        AND NOT EXISTS(SELECT 1 FROM market_finance_reconciliation_cases c WHERE c.code='client_compensation_dispute_overdue'
            AND c.object_id=client_market_terminations.id AND c.status='open')
        AND julianday(dispute_opened_at)<=julianday(?1)-3 ORDER BY dispute_opened_at LIMIT 50")
        .map_err(db_error("prepare overdue Client disputes"))?;
        s.query_map(params![at], |r| r.get(0))
            .map_err(db_error("query overdue Client disputes"))?
            .collect::<crate::db::Result<Vec<_>>>()
            .map_err(db_error("read overdue Client disputes"))?
    };
    for id in overdue {
        crate::market_reconcile::case_tx(
            &tx,
            "client_compensation_dispute_overdue",
            "client_termination",
            &id,
            at,
        )?;
        crate::market_notifications::owner_signal_tx(
            &tx,
            "client_compensation_dispute_overdue",
            "client_termination",
            &id,
            json!({"terminationId":id,"action":"review_financial_responsibility"}),
            at,
            false,
        )?;
    }
    tx.commit()
        .map_err(db_error("commit Client cleanup recovery"))?;
    Ok(count)
}
