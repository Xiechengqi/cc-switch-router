use super::*;
use futures_util::{StreamExt, stream};
use std::time::Duration as Delay;

fn iso(sec: i64) -> Result<String, AppError> {
    chrono::DateTime::from_timestamp(sec, 0)
        .map(|d| d.to_rfc3339())
        .ok_or_else(|| conflict("invalid lease timestamp"))
}
pub(super) fn fence(
    c: &Connection,
    installation: &str,
    id: &str,
    now: i64,
) -> Result<(), AppError> {
    let time = iso(now)?;
    if current(c, installation)?.is_none_or(|r| r.id != id || r.cutoff.is_some()) {
        return Ok(());
    }
    let seq = advance(c, installation)?;
    c.execute("UPDATE online_client_leases SET state='ending',sequence=?2,cutoff_at=?3 WHERE id=?1 AND cutoff_at IS NULL",params![id,seq,now]).market()?;
    crate::public_hosts::tombstone_subject(c, crate::namespace::PublicHostKind::Client, id)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    c.execute("UPDATE share_market_listings SET status='closed',updated_at=?2 WHERE status='active' AND share_id IN(SELECT share_id FROM online_client_share_bindings WHERE lease_id=?1)",params![id,time]).market()?;
    queue(
        c,
        installation,
        seq,
        json!({"operation":"fence","leaseId":id}),
    )?;
    // Persist the cutoff and revoke public access even if a wallet is frozen.
    // The ending lease is the durable retry record for the financial snapshot.
    c.execute_batch("SAVEPOINT online_automatic_exit")
        .market()?;
    match automatic_exit_tx(c, id, now) {
        Ok(()) => c.execute_batch("RELEASE online_automatic_exit").market()?,
        Err(error) => {
            c.execute_batch("ROLLBACK TO online_automatic_exit; RELEASE online_automatic_exit")
                .market()?;
            tracing::warn!(lease_id=id, %error, "Client access revoked; termination accounting deferred");
        }
    }
    Ok(())
}
/// Record why the system closes a rental. A due seller reclaim is the seller's
/// responsibility; a trial-only rental whose trial ran out ended with its
/// trial; anything else is an ordinary expiry. All three return unused rent
/// in full. A termination recorded earlier (e.g. by the buyer) wins.
pub(super) fn automatic_exit_tx(c: &Connection, id: &str, cutoff: i64) -> Result<(), AppError> {
    let (seller, reclaimed, trial_done): (String, bool, bool) = c
        .query_row(
            "SELECT r.seller_id,
                    coalesce(r.reclaim_effective_at<=?2 AND r.reclaim_cancelled_at IS NULL,0),
                    coalesce((SELECT k.after_trial='end' AND k.paid_started_at IS NULL AND k.trial_seconds_remaining<=0
                        FROM market_platform_contracts k WHERE k.product_kind='client_host' AND k.product_ref=r.id),0)
               FROM online_client_leases r WHERE r.id=?1",
            params![id, cutoff],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .market()?;
    let at = iso(cutoff)?;
    if reclaimed {
        crate::client_market_termination::accept_tx(
            c,
            id,
            Some(&seller),
            "provider_reclaim",
            None,
            &at,
        )
    } else if trial_done {
        crate::client_market_termination::accept_tx(c, id, None, "trial_completed", None, &at)
    } else {
        crate::client_market_termination::accept_tx(c, id, None, "workspace_expired", None, &at)
    }
}

fn children_finished(c: &Connection, id: &str, at: &str) -> Result<bool, AppError> {
    let mut stmt=c.prepare("SELECT s.id,s.service_started_at FROM share_market_subscriptions s JOIN online_client_share_bindings b ON b.share_id=s.share_id
        WHERE b.lease_id=?1 AND s.status NOT IN ('released','grant_failed')").market()?;
    let subs = stmt
        .query_map(params![id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })
        .market()?
        .collect::<crate::db::Result<Vec<_>>>()
        .market()?;
    for (sub, started) in &subs {
        if started.is_some() {
            crate::share_market_termination::parent_release_tx(c, sub, at)?;
        } else {
            crate::share_market::request_termination_revoke_tx(c, sub, at)?;
        }
    }
    let pending:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM share_market_terminations t JOIN online_client_share_bindings b ON b.share_id=t.share_id
        WHERE b.lease_id=?1 AND (json_extract(t.payload_json,'$.revokedAt') IS NULL OR t.status='finance_pending'))",params![id],|r|r.get(0)).market()?;
    Ok(subs.is_empty() && !pending)
}

/// An automatically renewing month is granted past its period end. The next
/// period only exists after the wallet renews at the boundary, and the Client
/// must acknowledge the extension; without this margin every renewal would cut
/// the tenant off and a briefly offline Client would lose a paid lease. A failed
/// or cancelled renewal changes the contract status and fences immediately.
pub(super) const RENEWAL_GRACE_SECS: i64 = 7_200;

/// A seller's Client that never returns must not hold the buyer's money. Access
/// and billing stopped at the cutoff; after this long the termination settles
/// without the cleanup receipt, which still gates relisting.
pub(super) const OFFLINE_SETTLEMENT_SECS: i64 = 86_400;

fn settle_offline(c: &Connection, lease: &str, now: i64) -> Result<(), AppError> {
    let installation: String = c
        .query_row(
            "SELECT installation_id FROM online_client_leases WHERE id=?1",
            params![lease],
            |r| r.get(0),
        )
        .market()?;
    c.execute_batch("SAVEPOINT online_offline_settlement")
        .market()?;
    match crate::client_market_termination::settle_unconfirmed_cleanup_tx(
        c,
        lease,
        &installation,
        &iso(now)?,
    ) {
        Ok(settled) => {
            c.execute_batch("RELEASE online_offline_settlement")
                .market()?;
            if settled {
                tracing::info!(
                    lease_id = lease,
                    "settled ending lease without cleanup receipt"
                );
            }
        }
        Err(error) => {
            c.execute_batch(
                "ROLLBACK TO online_offline_settlement; RELEASE online_offline_settlement",
            )
            .market()?;
            tracing::debug!(lease_id = lease, %error, "offline settlement deferred");
        }
    }
    Ok(())
}

// Offline parents can stay in 'ending' indefinitely. Walk the index across
// ticks so their pending receipts cannot starve later third-party refunds.
fn next_ending_batch(c: &Connection, after: &mut String) -> Result<Vec<(String, i64)>, AppError> {
    const BATCH_SIZE: usize = 100;
    let mut stmt = c
        .prepare(
            "SELECT id,cutoff_at FROM online_client_leases
         WHERE state='ending' AND id>?1 ORDER BY id LIMIT ?2",
        )
        .market()?;
    let rows = stmt
        .query_map(params![after.as_str(), BATCH_SIZE as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get(1)?))
        })
        .market()?
        .collect::<crate::db::Result<Vec<_>>>()
        .market()?;
    if rows.len() == BATCH_SIZE {
        after.clone_from(&rows[BATCH_SIZE - 1].0);
    } else {
        after.clear();
    }
    Ok(rows)
}
struct Pending {
    seq: i64,
    command: Value,
}
struct Rental {
    id: String,
    state: String,
    buyer: String,
    seller: String,
    email: String,
    alias: String,
    until: i64,
    cutoff: Option<i64>,
    management_version: i64,
}
fn current(c: &Connection, id: &str) -> Result<Option<Rental>, AppError> {
    c.query_row("SELECT id,state,buyer_id,seller_id,tenant_email,alias,grant_until,cutoff_at,management_version FROM online_client_leases WHERE installation_id=?1 AND released_at IS NULL",
        params![id],|r|Ok(Rental{id:r.get(0)?,state:r.get(1)?,buyer:r.get(2)?,seller:r.get(3)?,email:r.get(4)?,alias:r.get(5)?,until:r.get(6)?,cutoff:r.get(7)?,management_version:r.get(8)?})).optional().market()
}
fn take_work(c: &Connection, id: &str) -> Result<(i64, Option<Pending>, Value), AppError> {
    let seq = c
        .query_row(
            "SELECT sequence FROM online_client_listings WHERE installation_id=?1",
            params![id],
            |r| r.get(0),
        )
        .market()?;
    let p:Option<(i64,String)>=c.query_row("SELECT sequence,request_json FROM online_client_operations WHERE installation_id=?1 AND next_attempt<=unixepoch()",params![id],|r|Ok((r.get(0)?,r.get(1)?))).optional().market()?;
    let p = p
        .map(|(seq, v)| serde_json::from_str(&v).map(|command| Pending { seq, command }))
        .transpose()
        .map_err(|e| AppError::Internal(format!("corrupt Client operation: {e}")))?;
    c.execute(
        "UPDATE online_client_listings SET next_probe=(unixepoch()/300+1)*300 + abs(random()%25) WHERE installation_id=?1",
        params![id],
    )
    .market()?;
    if p.is_some() {
        c.execute("UPDATE online_client_operations SET next_attempt=unixepoch()+min(120,5*(attempts+1)),attempts=attempts+1,last_attempt_at=unixepoch() WHERE installation_id=?1",params![id]).market()?;
    }
    let mut ack=c.query_row("SELECT r.id,r.receipt_ack_json FROM online_client_leases r WHERE installation_id=?1 AND released_at IS NULL",params![id],|r|Ok(json!({"leaseId":r.get::<_,String>(0)?,"received":serde_json::from_str::<Value>(&r.get::<_,String>(1)?).unwrap_or_default()}))).optional().market()?.unwrap_or_else(||json!({}));
    let refresh_hardware:bool=c.query_row("SELECT coalesce(hardware_at,0)<=unixepoch()-3600 FROM online_client_listings WHERE installation_id=?1",params![id],|r|r.get(0)).market()?;
    if refresh_hardware {
        ack["hardware"] = json!(true);
    }
    Ok((seq, p, ack))
}
fn record(c: &Connection, lease: &Rental, events: &[AccessEvent]) -> Result<(), AppError> {
    for e in events.iter().take(1024) {
        if e.id.len() != 32
            || !e.id.bytes().all(|b| b.is_ascii_hexdigit())
            || e.occurrences == Some(0)
            || !matches!(
                e.kind.as_str(),
                "assistance"
                    | "terminal"
                    | "restart"
                    | "upgrade"
                    | "rollback"
                    | "host_settings"
                    | "tenant_upgrade"
                    | "tenant_auto_upgrade"
                    | "tenant_restart"
                    | "tenant_rollback"
            )
        {
            return Err(conflict("invalid workspace access receipt"));
        }
        c.execute("INSERT OR IGNORE INTO online_client_events(lease_id,event_key,kind,recorded_at,occurrences) VALUES(?1,?2,?3,?4,?5)",params![lease.id,e.id,e.kind,e.at,e.occurrences.unwrap_or(1)]).market()?;
        c.execute("INSERT OR IGNORE INTO user_notification_events(id,scope,recipient_email,category,event_kind,object_kind,object_id,payload_json,created_at_ms)
            VALUES(?1,'user',?2,'client',?6,'client',?3,?4,?5)",
            params![format!("lease:{}:{}",lease.id,e.id),lease.email,lease.id,json!({"clientLabel":lease.alias,"action":e.kind,"count":e.occurrences.unwrap_or(1)}).to_string(),e.at.saturating_mul(1000), if e.kind.starts_with("tenant_") { "client_workspace_management" } else { "client_workspace_access" }]).market()?;
    }
    let received: Vec<_> = events.iter().take(1024).map(|event| &event.id).collect();
    c.execute(
        "UPDATE online_client_leases SET receipt_ack_json=?2 WHERE id=?1",
        params![lease.id, json!(received).to_string()],
    )
    .market()?;
    Ok(())
}
fn activate(c: &Connection, id: &str, l: &Rental, now: i64) -> Result<(), AppError> {
    let at = iso(now)?;
    if let Some(contract) = wallet::contract_for_product_tx(c, "client_host", &l.id)? {
        wallet::activate_contract_tx(c, &contract, &at, &at)?;
    }
    let days: Option<u32> = c
        .query_row(
            "SELECT q.free_days FROM online_client_leases r JOIN online_client_quotes q ON q.id=r.quote_id WHERE r.id=?1",
            params![l.id],
            |r| r.get(0),
        )
        .market()?;
    if let Some(days) = days {
        if crate::market_common::free_usage_remaining_tx(
            c,
            &l.buyer,
            &l.seller,
            "client_host",
            id,
            days,
            &at,
        )? < l.until - now
        {
            return fence(c, id, &l.id, now);
        }
        let seconds = crate::market_common::claim_free_usage_tx(
            c,
            &l.buyer,
            &l.seller,
            "client_host",
            id,
            days,
            &at,
        )?;
        if seconds <= 0 {
            return Err(conflict("free allowance unavailable at activation"));
        }
        c.execute(
            "UPDATE online_client_leases SET grant_until=min(grant_until,?2) WHERE id=?1",
            params![l.id, now + seconds],
        )
        .market()?;
    }
    c.execute("UPDATE online_client_leases SET state='active',activated_at=?2 WHERE id=?1 AND state='starting'",params![l.id,now]).market()?;
    c.execute("UPDATE public_hosts SET lifecycle='active',revision=revision+1,updated_at=?2 WHERE subject_id=?1 AND kind='client' AND lifecycle='disabled'",params![l.id,at]).market()?;
    c.execute("INSERT OR IGNORE INTO user_notification_events(id,scope,recipient_email,category,event_kind,object_kind,object_id,payload_json,created_at_ms)
        VALUES(?1,'user',?2,'client','client_workspace_ready','client_lease',?3,?4,?5)",params![format!("lease:{}:ready",l.id),l.email,l.id,json!({"clientLabel":l.alias}).to_string(),now*1000]).market()?;
    Ok(())
}
fn release(c: &Connection, l: &Rental, now: i64) -> Result<(), AppError> {
    let at = iso(now)?;
    let installation: String = c
        .query_row(
            "SELECT installation_id FROM online_client_leases WHERE id=?1",
            params![l.id],
            |r| r.get(0),
        )
        .market()?;
    crate::client_market_termination::record_cleanup_confirmed_tx(c, &l.id, &installation, &at)?;
    let mut stmt = c
        .prepare("SELECT share_id FROM online_client_share_bindings WHERE lease_id=?1")
        .market()?;
    let ids = stmt
        .query_map(params![l.id], |r| r.get::<_, String>(0))
        .market()?
        .collect::<crate::db::Result<Vec<_>>>()
        .market()?;
    for share in ids {
        crate::store::retire_share_tx(c, &share, &installation)?;
    }
    c.execute(
        "UPDATE online_client_leases SET state='released',released_at=?2 WHERE id=?1",
        params![l.id, now],
    )
    .market()?;
    // A seller who reclaimed the Client usually wants it back: unlist it once
    // the workspace is verified clean, instead of offering it again.
    let unlist: bool = c
        .query_row(
            "SELECT reclaim_unlist_after=1 AND reclaim_effective_at IS NOT NULL AND reclaim_cancelled_at IS NULL
               FROM online_client_leases WHERE id=?1",
            params![l.id],
            |r| r.get(0),
        )
        .market()?;
    if unlist {
        lifecycle::request_unlist(c, &installation, now)?;
    }
    lifecycle::schedule_unlist(c, &installation)?;
    Ok(())
}

fn queue_handback(
    c: &Connection,
    installation: &str,
    rental: &Rental,
    status: &ClientStatus,
) -> Result<bool, AppError> {
    if let Some(pending) = status.awaiting_return.as_deref() {
        if pending != rental.id {
            return Err(conflict("unexpected lease waiting for handback"));
        }
        let returned:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM online_client_operations WHERE installation_id=?1 AND sequence=?2 AND json_extract(request_json,'$.action.operation')='return' AND json_extract(request_json,'$.action.leaseId')=?3)",params![installation,status.sequence,pending],|row|row.get(0)).market()?;
        if returned {
            // The Client confirmed clean handback and still holds its local lock.
            // Probe immediately after commit to acknowledge release on the Client.
            c.execute(
                "UPDATE online_client_listings SET next_probe=0 WHERE installation_id=?1",
                params![installation],
            )
            .market()?;
            return Ok(false);
        }
        let seq = advance(c, installation)?;
        c.execute(
            "UPDATE online_client_leases SET state='cleaning',sequence=?2 WHERE id=?1",
            params![rental.id, seq],
        )
        .market()?;
        queue(
            c,
            installation,
            seq,
            json!({"operation":"return","leaseId":pending}),
        )?;
        return Ok(true);
    }
    if rental.management_version >= 2 && !status.exclusive_management {
        return Err(conflict("exclusive Client required for handback"));
    }
    Ok(false)
}

fn apply_status(
    c: &Connection,
    id: &str,
    seq: i64,
    st: &ClientStatus,
    now: i64,
) -> Result<bool, AppError> {
    let (seller, mode): (String, String) = c
        .query_row(
            "SELECT owner_email,state FROM online_client_listings WHERE installation_id=?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .market()?;
    if !st.owner_email.eq_ignore_ascii_case(&seller) {
        return Err(conflict("installation ownership changed"));
    }
    if let Some(hardware) = &st.hardware {
        hardware.validate()?;
        c.execute("UPDATE online_client_listings SET check_json=?2,hardware_at=?3 WHERE installation_id=?1",
            params![id,json!({"exclusiveManagement":st.exclusive_management,"ownerRental":st.owner_rental,"hardware":hardware,"version":st.version.chars().take(80).collect::<String>()}).to_string(),now]).market()?;
    } else {
        c.execute(
            "UPDATE online_client_listings SET check_json=json_patch(
                 CASE WHEN json_valid(check_json) THEN check_json ELSE '{}' END,
                 json_object('ownerRental',json(?2),'exclusiveManagement',json(?3)))
             WHERE installation_id=?1 AND CASE WHEN json_valid(check_json) THEN
                 json_type(check_json,'$.ownerRental') IS NOT ?2 OR
                 json_type(check_json,'$.exclusiveManagement') IS NOT ?3 ELSE 1 END",
            params![
                id,
                st.owner_rental.to_string(),
                st.exclusive_management.to_string()
            ],
        )
        .market()?;
    }
    let rental = current(c, id)?;
    let running = st.running_lease.as_deref();
    let grant = st.grant.as_ref();
    if let Some(l) = &rental {
        if grant.is_some_and(|g| g.lease_id == l.id) {
            record(c, l, &st.events)?;
        }
    }
    if st.sequence != seq || st.switching {
        return Ok(false);
    }
    if st.awaiting_return.is_some()
        && rental
            .as_ref()
            .is_none_or(|lease| matches!(lease.state.as_str(), "starting" | "active"))
    {
        return Ok(false);
    }
    if mode == "locking"
        && rental.is_none()
        && st.listed_owner.as_deref() == Some(&seller)
        && running.is_none()
        && grant.is_none()
    {
        c.execute(
            "UPDATE online_client_listings SET state='listed' WHERE installation_id=?1",
            params![id],
        )
        .market()?;
    } else if mode == "unlisted"
        && rental.is_none()
        && st.listed_owner.is_none()
        && grant.is_none()
        && running.is_none()
    {
        c.execute(
            "UPDATE online_client_listings SET unlist_pending=0 WHERE installation_id=?1 AND state='unlisted'",
            params![id],
        )
        .market()?;
    } else if let Some(l) = rental {
        if st.listed_owner.as_deref() != Some(&seller) {
            return Ok(false);
        }
        let same = grant.is_some_and(|g| {
            g.lease_id == l.id
                && g.alias == l.alias
                && g.tenant_email == l.email
                && g.buyer_id.as_ref().is_none_or(|id| id == &l.buyer)
        });
        let management_ready = st.exclusive_management || l.management_version == 1;
        match l.state.as_str() {
            "starting"
                if management_ready
                    && same
                    && running == Some(&l.id)
                    && !st.fenced
                    && grant.unwrap().expires_at == l.until
                    && now < l.until =>
            {
                activate(c, id, &l, now)?
            }
            "ending"
                if (same && st.fenced && running == Some(&l.id))
                    || grant.is_none() && running.is_none() =>
            {
                let cutoff = iso(l.cutoff.unwrap_or(now))?;
                // Receipts can only be acknowledged while the Client still holds
                // this lease's grant. Once it is gone they cannot block release.
                if !children_finished(c, &l.id, &cutoff)?
                    || !st.cleanup_safe
                    || (grant.is_some() && !st.events.is_empty())
                {
                    return Ok(false);
                }
                automatic_exit_tx(c, &l.id, l.cutoff.unwrap_or(now))?;
                if grant.is_none() {
                    if queue_handback(c, id, &l, st)? {
                        return Ok(false);
                    }
                    release(c, &l, now)?;
                } else {
                    let next = advance(c, id)?;
                    c.execute(
                        "UPDATE online_client_leases SET state='cleaning',sequence=?2 WHERE id=?1",
                        params![l.id, next],
                    )
                    .market()?;
                    queue(c, id, next, json!({"operation":"clean","leaseId":l.id}))?;
                }
            }
            "cleaning" if running.is_none() && grant.is_none() => {
                if queue_handback(c, id, &l, st)? {
                    return Ok(false);
                }
                release(c, &l, now)?;
            }
            "active" if management_ready && same && running == Some(&l.id) && !st.fenced => {
                let expected:Option<i64>=c.query_row("SELECT json_extract(request_json,'$.action.expiresAt') FROM online_client_operations WHERE installation_id=?1 AND sequence=?2",params![id,seq],|r|r.get(0)).optional().market()?.flatten();
                let expires = grant.unwrap().expires_at;
                if expires != expected.unwrap_or(l.until) {
                    return Ok(false);
                }
                if expires != l.until {
                    c.execute(
                        "UPDATE online_client_leases SET grant_until=?2 WHERE id=?1",
                        params![l.id, expires],
                    )
                    .market()?;
                }
            }
            "active" if grant.is_none() && running.is_none() && l.until <= now => {
                fence(c, id, &l.id, l.until)?;
                return Ok(false);
            }
            _ => return Ok(false),
        }
        // An invalid or fenced receipt must not promote a legacy buyer's permissions.
        if l.management_version == 1 && st.exclusive_management {
            c.execute(
                "UPDATE online_client_leases SET management_version=2
                 WHERE id=?1 AND state='active' AND grant_until>?2",
                params![l.id, now],
            )
            .market()?;
        }
    } else if mode != "listed"
        || st.listed_owner.as_deref() != Some(&seller)
        || running.is_some()
        || grant.is_some()
    {
        return Ok(false);
    }
    c.execute(
        "DELETE FROM online_client_operations WHERE installation_id=?1 AND sequence=?2",
        params![id, seq],
    )
    .market()?;
    Ok(true)
}
fn sample(c: &Connection, id: &str, healthy: bool, now: i64) -> Result<(), AppError> {
    let rental = current(c, id)?;
    let scope = rental.as_ref().map(|r| r.id.as_str()).unwrap_or("");
    let available = healthy
        && rental
            .as_ref()
            .is_none_or(|r| r.state == "active" && r.until > now);
    c.execute(
        "UPDATE online_client_listings SET healthy=?2,observed_at=?3 WHERE installation_id=?1",
        params![id, available, now],
    )
    .market()?;
    c.execute("INSERT INTO online_client_probes(installation_id,lease_id,bucket,checked_at,healthy,seller_id) SELECT ?1,?2,?3,?4,?5,seller_id FROM online_client_listings WHERE installation_id=?1
        ON CONFLICT(installation_id,lease_id,bucket) DO UPDATE SET healthy=excluded.healthy,checked_at=excluded.checked_at,seller_id=excluded.seller_id
        WHERE online_client_probes.healthy=0",params![id,scope,now/300,now,available]).market()?;
    Ok(())
}
async fn probe(s: &ServerState, id: String) -> Result<(), AppError> {
    if s.proxy
        .active_client_route_for_installation(&id)
        .await
        .is_none()
    {
        let c = s.store.conn.lock().await;
        let tx = c.transaction().market()?;
        tx.execute("UPDATE online_client_operations SET waiting_reconnect=1,next_attempt=unixepoch()+300,last_error='tunnel_offline' WHERE installation_id=?1",params![id]).market()?;
        tx.execute("UPDATE online_client_listings SET next_probe=unixepoch()+300,probe_error='tunnel_offline' WHERE installation_id=?1",params![id]).market()?;
        let monitored:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM online_client_listings WHERE installation_id=?1 AND state!='unlisted') OR EXISTS(SELECT 1 FROM online_client_leases WHERE installation_id=?1 AND released_at IS NULL)",params![id],|r|r.get(0)).market()?;
        if monitored {
            sample(&tx, &id, false, stamp())?;
        }
        tx.commit().market()?;
        return Ok(());
    }
    let (seq, p, ack) = {
        let c = s.store.conn.lock().await;
        let tx = c.transaction().market()?;
        lifecycle::schedule_unlist(&tx, &id)?;
        let w = take_work(&tx, &id)?;
        tx.commit().market()?;
        w
    };
    // Always reconcile the recorded intent against local durable state first. A
    // lost reply or reboot never turns into another charge or a new operation.
    let mut st = rpc(s, &id, "/_ctl/market/status", ack).await;
    // Unlock only after durable settlement. Also recover a previous Router’s released
    // journal without consuming the sequence of a later pending Begin.
    if let Ok(status) = &st {
        if let Some(lease) = status.awaiting_return.as_deref().filter(|_| {
            status.exclusive_management
                && !status.switching
                && status.grant.is_none()
                && status.running_lease.is_none()
        }) {
            let released:bool = s.store.conn.lock().await.query_row(
                "SELECT EXISTS(SELECT 1 FROM online_client_leases WHERE installation_id=?1 AND id=?2 AND released_at IS NOT NULL)",
                params![id,lease], |row| row.get(0)).market()?;
            if released {
                st = rpc(
                    s,
                    &id,
                    "/_ctl/market/status",
                    json!({"releasedLeaseId":lease}),
                )
                .await;
            }
        }
    }
    if let (Ok(current), Some(p)) = (&st, &p) {
        if current.sequence < p.seq && !current.switching {
            let mut command = p.command.clone();
            if command["action"]["operation"] == "fence"
                && current.grant.is_none()
                && current.running_lease.is_none()
            {
                // Reserve this sequence on the Client even when a delayed Begin
                // never arrived. Its replay is subsequently stale.
                command["action"] =
                    json!({"operation":"cancel","leaseId":command["action"]["leaseId"]});
            }
            {
                let c = s.store.conn.lock().await;
                c.execute("UPDATE online_client_operations SET delivery_attempts=delivery_attempts+1,last_attempt_at=unixepoch(),waiting_reconnect=0 WHERE installation_id=?1 AND sequence=?2",params![id,p.seq]).market()?;
            }
            st = rpc(s, &id, "/_ctl/market/command", command).await;
        }
    }
    let tunnel_offline = st.is_err()
        && s.proxy
            .active_client_route_for_installation(&id)
            .await
            .is_none();
    let c = s.store.conn.lock().await;
    let tx = c.transaction().market()?;
    let latest: i64 = tx
        .query_row(
            "SELECT sequence FROM online_client_listings WHERE installation_id=?1",
            params![id],
            |r| r.get(0),
        )
        .market()?;
    if latest != seq {
        return Ok(());
    }
    let now = stamp();
    let dashboard_permissions = |conn: &Connection| {
        conn.query_row(
        "SELECT r.state,r.management_version,l.exclusive_management FROM online_client_leases r
         JOIN online_client_listings l USING(installation_id) WHERE r.installation_id=?1 AND r.released_at IS NULL",
        params![id],|row| Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,Option<bool>>(2)?))).optional().market()
    };
    let before = dashboard_permissions(&tx)?;
    tx.execute_batch("SAVEPOINT online_probe_result").market()?;
    let (applied, reason) = match st {
        Ok(ref status) => match apply_status(&tx, &id, seq, status, now) {
            Ok(true) if status.grant.is_none() && status.upgrade_busy => (true, Some("upgrading")),
            Ok(true) => (true, None),
            Ok(false) if status.switching => (false, Some("switching")),
            Ok(false) => (false, Some("awaiting_receipt")),
            Err(AppError::Conflict(_)) => {
                tx.execute_batch("ROLLBACK TO online_probe_result")
                    .market()?;
                (false, Some("status_mismatch"))
            }
            Err(e) => return Err(e),
        },
        Err(_) if tunnel_offline => (false, Some("tunnel_offline")),
        Err(_) => (false, Some("control_unavailable")),
    };
    tx.execute_batch("RELEASE online_probe_result").market()?;
    tx.execute(
        "UPDATE online_client_listings SET probe_error=?2 WHERE installation_id=?1 AND sequence=?3",
        params![id, reason, seq],
    )
    .market()?;
    tx.execute("UPDATE online_client_operations SET last_error=?2 WHERE installation_id=?1 AND sequence=?3",params![id,reason,seq]).market()?;
    let available = applied
        && st.as_ref().is_ok_and(|status| {
            status.grant.is_some() || !status.upgrade_busy && status.awaiting_return.is_none()
        });
    sample(&tx, &id, available, now)?;
    let changed = before != dashboard_permissions(&tx)?;
    tx.commit().market()?;
    if changed {
        s.dashboard_cache.invalidate();
    }
    Ok(())
}
pub(crate) async fn run(s: ServerState) -> anyhow::Result<()> {
    let mut tick = tokio::time::interval(Delay::from_secs(3));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut next_prune = tokio::time::Instant::now();
    let mut ending_cursor = String::new();
    loop {
        let now = tick.tick().await;
        // Observe route activation without coupling SSH registration to SQL.
        // Only dormant outboxes are woken; offline checks never count as sends.
        let connected = s.proxy.active_client_installation_ids().await;
        {
            let c = s.store.conn.lock().await;
            let wake_result = (|| -> Result<(), AppError> {
                for id in connected {
                    let woke=c.execute("UPDATE online_client_operations SET waiting_reconnect=0,next_attempt=0 WHERE installation_id=?1 AND waiting_reconnect=1",params![id]).market()?;
                    if woke > 0 {
                        c.execute("UPDATE online_client_listings SET next_probe=0 WHERE installation_id=?1",params![id]).market()?;
                    }
                }
                Ok(())
            })();
            if let Err(error) = wake_result {
                tracing::warn!(%error,"Client outbox reconnect wakeup deferred");
            }
        }
        let prune = now >= next_prune;
        match cycle(&s, prune, &mut ending_cursor).await {
            Ok(()) if prune => next_prune = now + Delay::from_secs(900),
            Ok(()) => {}
            Err(e) => tracing::warn!(error=%e,"online Client market reconciliation delayed"),
        }
    }
}
async fn cycle(s: &ServerState, prune: bool, ending_cursor: &mut String) -> Result<(), AppError> {
    let ids = {
        let c = s.store.conn.lock().await;
        let tx = c.transaction().market()?;
        let now = stamp();
        tx.execute("UPDATE online_client_listings SET state='unlisted',challenge=NULL WHERE state='locking' AND check_json IS NULL AND challenge_until<unixepoch()-30",[]).market()?;
        if prune {
            tx.execute("DELETE FROM online_client_quotes WHERE expires_at<unixepoch() AND NOT EXISTS(SELECT 1 FROM online_client_leases r WHERE r.quote_id=online_client_quotes.id)",[]).market()?;
            tx.execute(
                "DELETE FROM online_client_probes WHERE bucket<(unixepoch()/300)-18144",
                [],
            )
            .market()?;
        }
        let extensions = {
            let mut stmt=tx.prepare("SELECT r.installation_id,r.id,
                CASE WHEN q.cycle_price_minor=0 THEN unixepoch()+2592000 WHEN c.status='trialing' THEN unixepoch()+max(86400,c.trial_seconds_remaining)
                    ELSE unixepoch(p.period_end)+CASE WHEN c.renewal_policy='automatic' THEN ?1 ELSE 0 END END
                FROM online_client_leases r JOIN online_client_quotes q ON q.id=r.quote_id
                LEFT JOIN market_platform_contracts c ON c.product_ref=r.id AND c.product_kind='client_host'
                LEFT JOIN market_platform_periods p ON p.contract_id=c.id AND p.sequence=c.current_period_sequence AND p.status='open'
                WHERE r.state='active' AND NOT EXISTS(SELECT 1 FROM online_client_operations o WHERE o.installation_id=r.installation_id)
                AND ((q.cycle_price_minor=0 AND q.free_days IS NULL AND r.grant_until<unixepoch()+604800)
                    OR (c.status='trialing' AND r.grant_until<unixepoch()+43200)
                    OR (c.status='active' AND unixepoch(p.period_end)+CASE WHEN c.renewal_policy='automatic' THEN ?1 ELSE 0 END>r.grant_until)) LIMIT 100").market()?;
            stmt.query_map(params![RENEWAL_GRACE_SECS], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })
            .market()?
            .collect::<crate::db::Result<Vec<_>>>()
            .market()?
        };
        for (inst, id, until) in extensions {
            let seq = advance(&tx, &inst)?;
            tx.execute(
                "UPDATE online_client_leases SET sequence=?2 WHERE id=?1",
                params![id, seq],
            )
            .market()?;
            queue(
                &tx,
                &inst,
                seq,
                json!({"operation":"extend","leaseId":id,"expiresAt":until}),
            )?;
        }
        let reclaims = {
            let mut stmt = tx
                .prepare(
                    "SELECT installation_id,id,reclaim_reason FROM online_client_leases
                     WHERE reclaim_effective_at<=?1 AND reclaim_cancelled_at IS NULL AND released_at IS NULL
                       AND state IN ('starting','active') AND cutoff_at IS NULL LIMIT 100",
                )
                .market()?;
            stmt.query_map(params![now], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })
            .market()?
            .collect::<crate::db::Result<Vec<_>>>()
            .market()?
        };
        let reclaimed = !reclaims.is_empty();
        for (installation, id, reason) in reclaims {
            super::seller::deny_buyer_if_requested_tx(&tx, &id)?;
            super::seller::lease_event_tx(
                &tx,
                &id,
                &format!("reclaim:{now}:executed"),
                "reclaim_executed",
                now,
            )?;
            super::seller::notify_tenant_tx(
                &tx,
                &id,
                &format!("reclaim:{now}:executed"),
                "client_lease_reclaimed",
                json!({ "reason": reason }),
                now * 1000,
            )?;
            fence(&tx, &installation, &id, now)?;
        }
        let expired = {
            let mut stmt=tx.prepare("SELECT r.installation_id,r.id,min(r.grant_until,?1) FROM online_client_leases r LEFT JOIN market_platform_contracts c ON c.product_ref=r.id AND c.product_kind='client_host'
            WHERE r.state IN ('starting','active') AND ((r.grant_until<unixepoch()-300) OR (r.grant_until<=unixepoch() AND (c.id IS NULL AND EXISTS(SELECT 1 FROM online_client_quotes q WHERE q.id=r.quote_id AND q.free_days IS NOT NULL) OR c.id IS NOT NULL AND c.status!='trialing' AND NOT EXISTS(SELECT 1 FROM market_platform_periods p WHERE p.contract_id=c.id AND p.sequence=c.current_period_sequence AND unixepoch(p.period_end)>unixepoch())
                AND (c.renewal_policy!='automatic' OR r.grant_until<unixepoch()-180))) OR c.status IN ('ending','suspended','review_required') OR r.state='starting' AND r.created_at<unixepoch()-600) LIMIT 100").market()?;
            stmt.query_map(params![now], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })
            .market()?
            .collect::<crate::db::Result<Vec<_>>>()
            .market()?
        };
        let fenced = reclaimed || !expired.is_empty();
        for (installation, id, cutoff) in expired {
            fence(&tx, &installation, &id, cutoff)?;
        }
        let endings = next_ending_batch(&tx, ending_cursor)?;
        // A disconnected Client must not keep a third-party listing on sale.
        // A disputed child cannot roll back an already-committed parent fence.
        tx.commit().market()?;
        if fenced {
            s.dashboard_cache.invalidate();
        }
        for (id, cutoff) in endings {
            let child = c.transaction().market()?;
            match children_finished(&child, &id, &iso(cutoff)?) {
                Ok(done) => {
                    if done && cutoff <= now - OFFLINE_SETTLEMENT_SECS {
                        settle_offline(&child, &id, now)?;
                    }
                    child.commit().market()?
                }
                Err(e) => tracing::debug!(lease_id=id,error=%e,"waiting for child settlement"),
            }
        }
        let tx = c.transaction().market()?;
        let ids = {
            let mut stmt=tx.prepare("SELECT l.installation_id FROM online_client_listings l LEFT JOIN online_client_operations o USING(installation_id)
            WHERE (l.state!='unlisted' OR l.unlist_pending=1 OR EXISTS(SELECT 1 FROM online_client_leases r WHERE r.installation_id=l.installation_id AND r.released_at IS NULL)) AND (l.state!='locking' OR l.check_json IS NOT NULL) AND (l.next_probe<=unixepoch() OR o.next_attempt<=unixepoch())
            ORDER BY min(l.next_probe,coalesce(o.next_attempt,l.next_probe)),l.installation_id LIMIT 96").market()?;
            stmt.query_map([], |r| r.get::<_, String>(0))
                .market()?
                .collect::<crate::db::Result<Vec<_>>>()
                .market()?
        };
        tx.commit().market()?;
        ids
    };
    stream::iter(ids)
        .for_each_concurrent(12, |id| async move {
            if let Err(e) = probe(s, id.clone()).await {
                tracing::warn!(installation_id=id,error=%e,"online Client probe deferred");
            }
        })
        .await;
    let due = {
        let c = s.store.conn.lock().await;
        let mut stmt = c
            .prepare(
                "SELECT installation_id FROM online_client_listings
                 WHERE state='listed' AND coalesce(egress_checked_at,0)<=unixepoch()-?1
                 ORDER BY coalesce(egress_checked_at,0),installation_id LIMIT 8",
            )
            .market()?;
        stmt.query_map(params![EGRESS_RECHECK_SECS], |r| r.get::<_, String>(0))
            .market()?
            .collect::<crate::db::Result<Vec<_>>>()
            .market()?
    };
    stream::iter(due)
        .for_each_concurrent(4, |id| async move {
            if let Err(e) = recheck_egress(s, &id).await {
                tracing::debug!(installation_id=id,error=%e,"Client egress re-check deferred");
            }
        })
        .await;
    Ok(())
}

/// Tenants evaluate the egress IP themselves, so the real outbound path is
/// re-measured about hourly and a change is reported to the current tenant.
const EGRESS_RECHECK_SECS: i64 = 3_600;
/// A failed re-check is retried after about ten minutes.
const EGRESS_RETRY_SECS: i64 = 600;

async fn recheck_egress(s: &ServerState, id: &str) -> Result<(), AppError> {
    let challenge = Uuid::new_v4().simple().to_string();
    {
        let c = s.store.conn.lock().await;
        c.execute(
            "UPDATE online_client_listings SET egress_checked_at=unixepoch()-?2 WHERE installation_id=?1",
            params![id, EGRESS_RECHECK_SECS - EGRESS_RETRY_SECS],
        )
        .market()?;
        c.execute(
            "INSERT INTO online_client_preflight(challenge,installation_id,expires_at) VALUES(?1,?2,unixepoch()+30)",
            params![challenge, id],
        )
        .market()?;
    }
    let checked = super::probe_listing(s, id, &challenge).await;
    let c = s.store.conn.lock().await;
    let tx = c.transaction().market()?;
    let measured: Option<(Option<String>, Option<String>)> = tx
        .query_row(
            "DELETE FROM online_client_preflight WHERE challenge=?1 RETURNING egress_ip,country_code",
            params![challenge],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .market()?;
    let (Ok(_), Some((Some(ip), country))) = (checked, measured) else {
        tx.commit().market()?;
        return Ok(());
    };
    let (old_ip, old_country): (Option<String>, Option<String>) = tx
        .query_row(
            "SELECT egress_ip,country_code FROM online_client_listings WHERE installation_id=?1 AND state='listed'",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .market()?
        .unwrap_or_default();
    let country = country.or(old_country.clone());
    tx.execute(
        "UPDATE online_client_listings SET egress_ip=?2,country_code=?3,egress_checked_at=unixepoch()
         WHERE installation_id=?1 AND state='listed'",
        params![id, ip, country],
    )
    .market()?;
    if old_ip.as_deref().is_some_and(|old| old != ip) {
        let lease: Option<String> = tx
            .query_row(
                "SELECT id FROM online_client_leases WHERE installation_id=?1 AND released_at IS NULL
                   AND state IN ('starting','active')",
                params![id],
                |r| r.get(0),
            )
            .optional()
            .market()?;
        if let Some(lease) = lease {
            let now = stamp();
            super::seller::lease_event_tx(
                &tx,
                &lease,
                &format!("egress:{now}"),
                "egress_changed",
                now,
            )?;
            super::seller::notify_tenant_tx(
                &tx,
                &lease,
                &format!("egress:{now}"),
                "client_egress_changed",
                // Only the tenant receives this, and the tenant already sees
                // the full IP; a change within the same /24 must stay visible.
                json!({
                    "fromIp": old_ip, "toIp": ip,
                    "fromCountry": old_country, "toCountry": country,
                    "countryChanged": old_country.is_some() && old_country != country,
                }),
                now * 1000,
            )?;
        }
    }
    tx.commit().market()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_ending_leases_cannot_starve_later_child_settlements() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE online_client_leases(id TEXT PRIMARY KEY, state TEXT, cutoff_at INTEGER);
             WITH RECURSIVE leases(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM leases WHERE n<101)
             INSERT INTO online_client_leases SELECT printf('lease-%03d',n),'ending',n FROM leases;
             INSERT INTO online_client_leases VALUES('already-released','released',0);",
        )
        .unwrap();
        let mut cursor = String::new();
        assert_eq!(next_ending_batch(&c, &mut cursor).unwrap().len(), 100);
        // All 100 earlier parents are still waiting on disconnected Clients.
        assert_eq!(
            next_ending_batch(&c, &mut cursor).unwrap(),
            [("lease-101".into(), 101)]
        );
        assert!(cursor.is_empty());
        assert_eq!(
            next_ending_batch(&c, &mut cursor).unwrap()[0].0,
            "lease-001"
        );
    }

    fn actor(id: &str) -> AuthSession {
        let now = Utc::now();
        AuthSession {
            financial_proof_id: None,
            session_id: format!("session-{id}"),
            user_id: id.into(),
            email: format!("{id}@example.org"),
            auth_source_kind: "browser".into(),
            auth_source_id: id.into(),
            access_token_hash: String::new(),
            refresh_token_hash: String::new(),
            access_expires_at: now + Duration::days(1),
            refresh_expires_at: now + Duration::days(10),
            created_at: now,
            last_used_at: now,
        }
    }
    fn listed(c: &Connection) {
        c.execute("INSERT INTO installations(id,public_key,platform,app_version,created_at,last_seen_at,control_secret_b64,owner_email,owner_verified_at,client_activated_at,market_lease_v1)
            VALUES('physical','key','linux','1','now','now','local','owner@example.org','now','now',1)",[]).unwrap();
        c.execute("INSERT INTO online_client_listings(installation_id,seller_id,owner_email,state,revision,sequence,cycle_price_minor,updated_at,healthy,observed_at,check_json)
            VALUES('physical','owner','owner@example.org','listed',1,1,0,unixepoch(),1,unixepoch(),json_object('exclusiveManagement',json('true')))",[]).unwrap();
    }
    fn offer(tx: &Transaction<'_>, buyer: &AuthSession) -> String {
        let quote = create_quote(
            tx,
            "physical",
            buyer,
            &Quote {
                revision: 1,
                auto_renew: false,
            },
        )
        .unwrap();
        quote["id"].as_str().unwrap().to_owned()
    }
    fn status(seq: i64, rental: Option<&Rental>) -> ClientStatus {
        ClientStatus {
            exclusive_management: true,
            upgrade_busy: false,
            awaiting_return: None,
            capability: "marketLeaseV1".into(),
            owner_rental: true,
            owner_email: "owner@example.org".into(),
            listed_owner: Some("owner@example.org".into()),
            sequence: seq,
            grant: rental.map(|r| Grant {
                buyer_id: Some(r.buyer.clone()),
                lease_id: r.id.clone(),
                tenant_email: r.email.clone(),
                alias: r.alias.clone(),
                expires_at: r.until,
            }),
            running_lease: rental.map(|r| r.id.clone()),
            switching: false,
            fenced: false,
            cleanup_safe: true,
            hardware: None,
            events: vec![],
            version: "test".into(),
        }
    }

    #[test]
    fn offline_unlisting_is_immediate_idempotent_and_invalidates_quotes() {
        let c = crate::test_support::database().unwrap();
        listed(&c);
        let tx = c.transaction().unwrap();
        let buyer = actor("buyer");
        let quote = offer(&tx, &buyer);
        lifecycle::request_unlist(&tx, "physical", stamp()).unwrap();
        let state:(String,bool,i64)=tx.query_row("SELECT state,unlist_pending,sequence FROM online_client_listings WHERE installation_id='physical'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(state, ("unlisted".into(), true, 2));
        assert!(commit_quote(&tx, &quote, &buyer, false).is_err());
        lifecycle::request_unlist(&tx, "physical", stamp()).unwrap();
        assert_eq!(
            tx.query_row(
                "SELECT sequence FROM online_client_listings WHERE installation_id='physical'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
        let mut stale = status(1, None);
        stale.listed_owner = None;
        assert!(!apply_status(&tx, "physical", 2, &stale, stamp()).unwrap());
        assert!(tx.query_row("SELECT unlist_pending FROM online_client_listings WHERE installation_id='physical'",[],|r|r.get::<_,bool>(0)).unwrap());
        let mut confirmed = status(2, None);
        confirmed.listed_owner = None;
        assert!(apply_status(&tx, "physical", 2, &confirmed, stamp()).unwrap());
        assert!(!tx.query_row("SELECT unlist_pending FROM online_client_listings WHERE installation_id='physical'",[],|r|r.get::<_,bool>(0)).unwrap());
    }

    #[test]
    fn unlisting_an_active_rental_preserves_its_command_and_identity_protection() {
        let c = crate::test_support::database().unwrap();
        listed(&c);
        crate::test_support::online_lease(
            &c,
            "active-lease",
            "physical",
            "owner",
            "buyer",
            &iso(stamp()).unwrap(),
        );
        c.execute(
            "UPDATE online_client_listings SET sequence=2 WHERE installation_id='physical'",
            [],
        )
        .unwrap();
        queue(
            &c,
            "physical",
            2,
            json!({"operation":"extend","leaseId":"active-lease","expiresAt":stamp()+3600}),
        )
        .unwrap();
        let before:String=c.query_row("SELECT request_json FROM online_client_operations WHERE installation_id='physical'",[],|r|r.get(0)).unwrap();
        lifecycle::request_unlist(&c, "physical", stamp()).unwrap();
        let after:String=c.query_row("SELECT request_json FROM online_client_operations WHERE installation_id='physical'",[],|r|r.get(0)).unwrap();
        assert_eq!(before, after);
        assert!(current(&c, "physical").unwrap().is_some());
        assert!(
            c.execute(
                "UPDATE installations SET owner_email='intruder@example.org' WHERE id='physical'",
                []
            )
            .is_err()
        );
        assert!(
            c.execute("DELETE FROM installations WHERE id='physical'", [])
                .is_err()
        );
    }
    #[test]
    fn legacy_management_promotes_only_after_the_current_grant_is_reconciled() {
        let conn = crate::test_support::database().unwrap();
        listed(&conn);
        let now = stamp();
        crate::test_support::online_lease(
            &conn,
            "old-lease",
            "physical",
            "owner",
            "buyer",
            &iso(now).unwrap(),
        );
        conn.execute("UPDATE online_client_listings SET sequence=2,check_json='invalid' WHERE installation_id='physical'", []).unwrap();
        let lease = current(&conn, "physical").unwrap().unwrap();
        let mut receipt = status(2, Some(&lease));
        receipt.grant.as_mut().unwrap().expires_at -= 1;
        assert!(!apply_status(&conn, "physical", 2, &receipt, now).unwrap());
        assert_eq!(
            current(&conn, "physical")
                .unwrap()
                .unwrap()
                .management_version,
            1
        );
        receipt.grant.as_mut().unwrap().expires_at = lease.until;
        receipt.fenced = true;
        assert!(!apply_status(&conn, "physical", 2, &receipt, now).unwrap());
        receipt.fenced = false;
        receipt.awaiting_return = Some(lease.id.clone());
        assert!(!apply_status(&conn, "physical", 2, &receipt, now).unwrap());
        receipt.awaiting_return = None;
        apply_status(&conn, "physical", 2, &receipt, lease.until).unwrap();
        assert_eq!(
            current(&conn, "physical")
                .unwrap()
                .unwrap()
                .management_version,
            1
        );
        assert!(apply_status(&conn, "physical", 2, &receipt, now).unwrap());
        let migrated = current(&conn, "physical").unwrap().unwrap();
        assert_eq!(migrated.id, lease.id);
        assert_eq!(migrated.management_version, 2);
    }

    #[test]
    fn checkout_replay_and_fencing_preserve_workspace_identity_until_cleanup() {
        let c = crate::test_support::database().unwrap();
        let tx = c.transaction().unwrap();
        listed(&tx);
        let a = actor("alpha");
        let b = actor("beta");
        let alpha_quote = offer(&tx, &a);
        let beta_quote = offer(&tx, &b);
        let now = stamp();
        let started = commit_quote(&tx, &alpha_quote, &a, false).unwrap();
        assert_eq!(commit_quote(&tx, &alpha_quote, &a, false).unwrap(), started);
        assert!(commit_quote(&tx, &alpha_quote, &b, false).is_err());
        assert!(commit_quote(&tx, &beta_quote, &b, false).is_err());
        let rental = current(&tx, "physical").unwrap().unwrap();
        let nonce = format!("lease_{}_{}", rental.id, Uuid::new_v4().simple());
        check_nonce(&tx, "physical", "client_tunnel_lease", &nonce).unwrap();
        check_nonce(&tx, "physical", "share_edit_ack", &nonce).unwrap();
        let mut first = status(2, Some(&rental));
        first.switching = true;
        assert!(!apply_status(&tx, "physical", 2, &first, now).unwrap());
        first.switching = false;
        assert!(apply_status(&tx, "physical", 2, &first, now).unwrap());
        let lifecycle = || {
            crate::public_hosts::get_by_label(&tx, &rental.alias)
                .unwrap()
                .unwrap()
                .lifecycle
        };
        assert_eq!(
            lifecycle(),
            crate::public_hosts::PublicHostLifecycle::Active
        );
        tx.execute(
            "INSERT INTO online_client_share_bindings VALUES('alpha-share',?1)",
            params![rental.id],
        )
        .unwrap();
        assert!(share_owner(&tx, "physical", "owner@example.org").unwrap() == "alpha@example.org");
        assert!(!allow_traffic(&tx, "physical", "owner-share").unwrap());
        assert!(allow_traffic(&tx, "physical", "alpha-share").unwrap());
        assert!(check_nonce(&tx, "physical", "share_sync", "stale-owner").is_err());
        first.events.push(AccessEvent {
            occurrences: Some(3),
            id: "1".repeat(32),
            kind: "assistance".into(),
            at: now,
        });
        for _ in 0..2 {
            assert!(apply_status(&tx, "physical", 2, &first, now).unwrap());
        }
        let (_, _, ack) = take_work(&tx, "physical").unwrap();
        assert_eq!(ack["received"], json!(["1".repeat(32)]));
        assert_eq!(ack["leaseId"], rental.id);
        assert_eq!(
            tx.query_row(
                "SELECT sum(occurrences) FROM online_client_events WHERE lease_id=?1",
                params![rental.id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            3
        );
        assert_eq!(
            tx.query_row("SELECT count(*) FROM user_notification_events WHERE event_kind='client_workspace_access'", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            1
        );
        fence(&tx, "physical", &rental.id, now + 1).unwrap();
        assert_eq!(
            lifecycle(),
            crate::public_hosts::PublicHostLifecycle::Tombstoned
        );
        assert!(!allow_traffic(&tx, "physical", "alpha-share").unwrap());
        first.sequence = 3;
        first.fenced = true;
        assert_eq!(tx.query_row("SELECT count(*) FROM user_notification_events WHERE event_kind='client_workspace_ready'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
        first.events.clear();
        first.cleanup_safe = false;
        assert!(!apply_status(&tx, "physical", 3, &first, now + 2).unwrap());
        assert!(commit_quote(&tx, &beta_quote, &b, false).is_err());
        first.cleanup_safe = true;
        assert!(apply_status(&tx, "physical", 3, &first, now + 2).unwrap());
        assert_eq!(current(&tx, "physical").unwrap().unwrap().state, "cleaning");
        assert!(!apply_status(&tx, "physical", 4, &first, now + 2).unwrap());
        assert!(apply_status(&tx, "physical", 4, &status(4, None), now + 3).unwrap());
        let next = commit_quote(&tx, &beta_quote, &b, false).unwrap();
        assert_ne!(next["leaseId"], started["leaseId"]);
        assert!(check_share_id(&tx, "physical", "alpha-share").is_err());
        assert!(check_nonce(&tx, "physical", "share_edit_ack", &nonce).is_err());
        assert!(!apply_status(&tx, "physical", 5, &status(4, None), now + 4).unwrap());
        assert_eq!(
            lifecycle(),
            crate::public_hosts::PublicHostLifecycle::Tombstoned
        );
        tx.commit().unwrap();
    }

    #[test]
    fn cancel_before_begin_requires_a_durable_client_sequence() {
        let c = crate::test_support::database().unwrap();
        let tx = c.transaction().unwrap();
        listed(&tx);
        let buyer = actor("alpha");
        let quote = offer(&tx, &buyer);
        let receipt = commit_quote(&tx, &quote, &buyer, false).unwrap();
        let now = stamp();
        fence(&tx, "physical", receipt["leaseId"].as_str().unwrap(), now).unwrap();
        assert!(!apply_status(&tx, "physical", 3, &status(1, None), now).unwrap());
        assert!(current(&tx, "physical").unwrap().is_some());
        assert!(apply_status(&tx, "physical", 3, &status(3, None), now).unwrap());
        assert!(current(&tx, "physical").unwrap().is_none());
        assert_eq!(
            commit_quote(&tx, &quote, &buyer, false).unwrap()["state"],
            "released"
        );
    }
    fn paid_listing(tx: &Transaction<'_>, trial_days: u32) {
        listed(tx);
        let at = Utc::now().to_rfc3339();
        tx.execute(
            "UPDATE market_finance_control SET mode='enabled',cutover_approved_at=?1",
            params![at],
        )
        .unwrap();
        for id in ["owner", "alpha"] {
            tx.execute("INSERT INTO users(id,email_normalized,email_verified_at,created_at,last_login_at) VALUES(?1,?2,?3,?3,?3)",
                params![id,format!("{id}@example.org"),at]).unwrap();
        }
        crate::market_access::configure_open_test_policy(tx, &actor("owner"), "USD", 1000, &at);
        tx.execute(
            "UPDATE online_client_listings SET cycle_price_minor=1000,trial_days=?1 WHERE installation_id='physical'",
            params![trial_days],
        )
        .unwrap();
        crate::market_wallet::credit_verified_topup_tx(
            tx,
            "alpha",
            "topup",
            1000,
            "local-fixture",
            &at,
        )
        .unwrap();
    }

    #[test]
    fn self_rental_freezes_zero_terms_and_remains_exclusive_through_cleanup() {
        for paid in [true, false] {
            let c = crate::test_support::database().unwrap();
            let tx = c.transaction().unwrap();
            if paid {
                paid_listing(&tx, 7);
            } else {
                listed(&tx);
                tx.execute("UPDATE online_client_listings SET free_days=2", [])
                    .unwrap();
            }
            let owner = actor("owner");
            let other = actor("alpha");
            let request = Quote {
                revision: 1,
                auto_renew: true,
            };
            let now = stamp();
            // Independent self-rental capability is also checked at commit.
            let external = offer(&tx, &other);
            assert!(create_quote(&tx, "physical", &owner, &request).is_err());
            let mut st = status(1, None);
            apply_status(&tx, "physical", 1, &st, now).unwrap();
            let quote = create_quote(&tx, "physical", &owner, &request).unwrap();
            let q = quote["id"].as_str().unwrap();
            let terms = &quote["terms"];
            assert_eq!(terms["listingPriceMinor"], if paid { 1000 } else { 0 });
            assert_eq!(terms["cyclePriceMinor"], 0);
            assert_eq!(terms["selfRental"], true);
            assert_eq!(terms["trialSeconds"], 0);
            assert_eq!(terms["autoRenew"], false);
            assert!(terms["freeDays"].is_null() && quote["walletFunding"].is_null());
            st.owner_rental = false;
            apply_status(&tx, "physical", 1, &st, now).unwrap();
            assert!(commit_quote(&tx, q, &owner, false).is_err());
            st.owner_rental = true;
            apply_status(&tx, "physical", 1, &st, now).unwrap();
            let receipt = commit_quote(&tx, q, &owner, false).unwrap();
            assert_eq!(commit_quote(&tx, q, &owner, false).unwrap(), receipt);
            assert!(commit_quote(&tx, &external, &other, false).is_err());
            let rental = current(&tx, "physical").unwrap().unwrap();
            assert!(rental.until >= now + 30 * 86400);
            apply_status(&tx, "physical", 2, &status(2, Some(&rental)), now).unwrap();
            assert_eq!(current(&tx, "physical").unwrap().unwrap().state, "active");
            assert!(
                wallet::contract_for_product_tx(&tx, "client_host", &rental.id)
                    .unwrap()
                    .is_none()
            );
            let traces: bool = tx
                .query_row(
                    "SELECT
                EXISTS(SELECT 1 FROM market_commission_quote_terms WHERE quote_id=?1) OR
                EXISTS(SELECT 1 FROM market_free_usage_ledgers WHERE buyer_user_id='owner') OR
                EXISTS(SELECT 1 FROM client_market_trial_scopes WHERE buyer_user_id='owner') OR
                EXISTS(SELECT 1 FROM market_wallet_holds WHERE user_id='owner')",
                    params![q],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(!traces);
            fence(&tx, "physical", &rental.id, now + 1).unwrap();
            assert!(commit_quote(&tx, &external, &other, false).is_err());
            let mut stopped = status(3, Some(&rental));
            stopped.fenced = true;
            apply_status(&tx, "physical", 3, &stopped, now + 2).unwrap();
            assert!(commit_quote(&tx, &external, &other, false).is_err());
            let mut clean = status(4, None);
            clean.awaiting_return = Some(rental.id.clone());
            assert!(!apply_status(&tx, "physical", 4, &clean, now + 3).unwrap());
            assert!(current(&tx, "physical").unwrap().is_some());
            clean.sequence = 5;
            assert!(apply_status(&tx, "physical", 5, &clean, now + 4).unwrap());
            assert_ne!(
                commit_quote(&tx, &external, &other, false).unwrap()["leaseId"],
                receipt["leaseId"]
            );
        }
    }

    #[test]
    fn observed_installation_minutes_count_each_sampled_minute_once() {
        let conn = crate::test_support::database().unwrap();
        assert!(
            crate::store::installation_health_minutes_24h(&conn)
                .unwrap()
                .is_empty()
        );
        let minute = stamp() / 60 * 60 - 2 * 60;
        for (offset, status) in [
            (0, "healthy"),
            (5, "unhealthy"),
            (60, "unhealthy"),
            (-60, "unknown"),
            (-86400, "healthy"),
        ] {
            conn.execute("INSERT INTO installation_health_checks(installation_id,checked_at,is_healthy,status)
                VALUES('observed',?1,?2,?3)", params![minute+offset,status=="healthy",status]).unwrap();
        }
        assert_eq!(
            crate::store::installation_health_minutes_24h(&conn).unwrap()["observed"],
            (1, 2)
        );
    }

    #[test]
    fn checkout_after_trial_reset_requires_a_fresh_quote_even_with_the_same_remaining_time() {
        let c = crate::test_support::database().unwrap();
        let tx = c.transaction().unwrap();
        paid_listing(&tx, 1);
        let buyer = actor("alpha");
        let quote = offer(&tx, &buyer);
        let balance = crate::market_wallet::wallet_snapshot_tx(&tx, "alpha").unwrap();
        tx.execute(
            "INSERT INTO client_market_trial_scopes(buyer_user_id,provider_user_id,host_id,current_epoch)
             VALUES ('alpha','owner','physical',1)", [],
        ).unwrap();
        let fresh_quote = offer(&tx, &buyer);
        let frozen = |id: &str| {
            tx.query_row(
            "SELECT trial_epoch, json_extract(projection_json,'$.trialSeconds') FROM online_client_quotes WHERE id=?1",
            params![id], |r| Ok((r.get::<_, i64>(0)?,r.get::<_, i64>(1)?)),
        ).unwrap()
        };
        assert_eq!(frozen(&quote), (0, 86_400));
        assert_eq!(frozen(&fresh_quote), (1, 86_400));
        assert!(matches!(
            commit_quote(&tx, &quote, &buyer, false),
            Err(AppError::Conflict(_))
        ));
        let after = crate::market_wallet::wallet_snapshot_tx(&tx, "alpha").unwrap();
        assert_eq!(after.available_units, balance.available_units);
        assert_eq!(after.held_units, balance.held_units);
        assert!(!tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM market_platform_contracts) OR EXISTS(SELECT 1 FROM online_client_leases)
                 OR EXISTS(SELECT 1 FROM online_client_operations)", [], |r| r.get::<_, bool>(0),
        ).unwrap());
        assert_eq!(
            commit_quote(&tx, &fresh_quote, &buyer, false).unwrap()["state"],
            "starting"
        );
    }

    #[test]
    fn online_month_charges_only_confirmed_ten_of_twelve_hours() {
        let c = crate::test_support::database().unwrap();
        let tx = c.transaction().unwrap();
        paid_listing(&tx, 0);
        let buyer = actor("alpha");
        let quote = offer(&tx, &buyer);
        // A new policy takes effect while the buyer is reviewing the quote.
        let at = Utc::now().to_rfc3339();
        tx.execute(
            "INSERT INTO market_commission_policies VALUES (2,500,?1,'owner',?1)",
            params![at],
        )
        .unwrap();
        let start = (stamp() / 3600 + 1) * 3600;
        commit_quote(&tx, &quote, &buyer, false).unwrap();
        assert!(
            crate::market_wallet::wallet_snapshot_tx(&tx, "alpha")
                .unwrap()
                .held_units
                > 0
        );
        let l = current(&tx, "physical").unwrap().unwrap();
        let contract = wallet::contract_for_product_tx(&tx, "client_host", &l.id)
            .unwrap()
            .unwrap();
        assert!(apply_status(&tx, "physical", 2, &status(2, Some(&l)), start).unwrap());
        let commission = tx.query_row(
            "SELECT q.policy_version,q.rate_bps,c.policy_version,c.rate_bps,p.policy_version,p.rate_bps
             FROM market_commission_quote_terms q JOIN market_commission_contract_terms c ON c.quote_id=q.quote_id
             JOIN market_commission_periods p ON p.contract_id=c.contract_id
             WHERE q.quote_id=?1 AND q.provider_user_id='owner' AND c.contract_id=?2 AND p.source_kind='monthly_period'",
            params![quote, contract],
            |r| Ok([r.get::<_,i64>(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?]),
        ).unwrap();
        assert_eq!(commission, [1, 1000, 1, 1000, 1, 1000]);
        assert_eq!(
            commit_quote(&tx, &quote, &buyer, false).unwrap()["leaseId"],
            l.id
        );
        for (hour, success) in [(0, 10), (1, 9)] {
            for slot in 0..12 {
                sample(
                    &tx,
                    "physical",
                    slot < success,
                    start + hour * 3600 + slot * 300 + 1,
                )
                .unwrap();
            }
        }
        // A retry in an already successful slot cannot erase the observation
        // used for billing. The listing still reflects the most recent probe.
        sample(&tx, "physical", false, start + 9 * 300 + 2).unwrap();
        let cutoff = iso(start + 7200).unwrap();
        wallet::settle_client_termination_tail_tx(&tx, &contract, &cutoff).unwrap();
        wallet::settle_client_termination_tail_tx(&tx, &contract, &cutoff).unwrap();
        let verdicts=tx.prepare("SELECT service_available FROM market_platform_service_evidence WHERE contract_id=?1 ORDER BY started_at").unwrap()
            .query_map(params![contract],|r|r.get::<_,bool>(0)).unwrap().collect::<crate::db::Result<Vec<_>>>().unwrap();
        assert_eq!(verdicts, vec![true, false]);
        let (earned,refunded):(i64,i64)=tx.query_row("SELECT fulfilled_units,refunded_units FROM market_platform_periods WHERE contract_id=?1",params![contract],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert!(earned > 0 && refunded > 0 && (earned - refunded).abs() <= 1);
        tx.execute_batch(
            "CREATE TEMP TRIGGER hold_exit_snapshot BEFORE INSERT ON client_market_terminations
            BEGIN SELECT RAISE(ABORT,'temporarily unavailable'); END;",
        )
        .unwrap();
        fence(&tx, "physical", &l.id, start + 7200).unwrap();
        assert_eq!(
            current(&tx, "physical").unwrap().unwrap().cutoff,
            Some(start + 7200)
        );
        assert_eq!(
            crate::public_hosts::get_by_label(&tx, &l.alias)
                .unwrap()
                .unwrap()
                .lifecycle,
            crate::public_hosts::PublicHostLifecycle::Tombstoned
        );
        tx.execute_batch("DROP TRIGGER hold_exit_snapshot").unwrap();
        let mut sealed = status(3, Some(&l));
        sealed.fenced = true;
        assert!(apply_status(&tx, "physical", 3, &sealed, start + 7201).unwrap());
        tx.execute_batch("CREATE TEMP TRIGGER hold_exit_finance BEFORE UPDATE OF status ON client_market_terminations
            WHEN NEW.status='completed' BEGIN SELECT RAISE(ABORT,'temporarily unavailable'); END;").unwrap();
        assert!(apply_status(&tx, "physical", 4, &status(4, None), start + 7210).unwrap());
        assert!(
            tx.query_row(
                "SELECT occupied FROM online_client_supply WHERE installation_id='physical'",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
        );
        tx.execute_batch("DROP TRIGGER hold_exit_finance").unwrap();
        tx.commit().unwrap();
        crate::client_market_termination::recover_cleanup_isolated(&c, &iso(start + 7220).unwrap())
            .unwrap();
        let tx = c.transaction().unwrap();
        let (result,receipt):(String,bool)=tx.query_row("SELECT t.status,EXISTS(SELECT 1 FROM client_cleanup_receipts e WHERE e.installation_id=t.installation_id AND e.host_id=t.host_id)
            FROM client_market_terminations t WHERE t.installation_id=?1",params![l.id],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(result, "completed");
        assert!(receipt);
        assert!(
            !tx.query_row(
                "SELECT occupied FROM online_client_supply WHERE installation_id='physical'",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
        );
        crate::market_wallet::reconcile_wallet_tx(&tx, "alpha").unwrap();
    }
}
