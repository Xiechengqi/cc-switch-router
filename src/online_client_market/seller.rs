//! Seller controls on a listed Client: reclaiming a rental, changing the offer
//! for future orders, and pausing new orders.
use super::*;

/// A reclaim with notice gives the tenant a day to export the workspace.
pub(super) const RECLAIM_NOTICE_SECS: i64 = 86_400;

/// Public listing titles: short, single-line, and never a link or contact.
pub(super) fn normalize_title(raw: Option<&str>) -> Result<Option<String>, AppError> {
    let Some(title) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if title.chars().count() > 30 || title.chars().any(char::is_control) || looks_like_contact(title)
    {
        return Err(AppError::coded_bad_request(
            "CLIENT_LISTING_TITLE_INVALID",
            "use at most 30 characters, without line breaks, links or email addresses",
            json!({ "maxLength": 30 }),
        ));
    }
    Ok(Some(title.to_owned()))
}

fn looks_like_contact(title: &str) -> bool {
    let lower = title.to_ascii_lowercase();
    if lower.contains("://") || lower.contains("www.") || lower.contains('@') {
        return true;
    }
    // A bare domain such as "example.com": letters after the last dot.
    title.split_whitespace().any(|token| {
        token.rsplit_once('.').is_some_and(|(head, tld)| {
            !head.is_empty()
                && (2..=10).contains(&tld.len())
                && tld.bytes().all(|b| b.is_ascii_alphabetic())
        })
    })
}

/// Shared rental terms validation for listing and later offer changes.
pub(super) fn validate_terms(price: i64, free_days: Option<u32>, trial_days: u32) -> Result<(), AppError> {
    if !(0..=100_000_000).contains(&price)
        || free_days.is_some_and(|n| !(1..=365).contains(&n))
        || trial_days > 30
        || price > 0 && free_days.is_some()
        || price == 0 && trial_days != 0
    {
        return Err(AppError::BadRequest("invalid Client rental terms".into()));
    }
    Ok(())
}

/// In-app notice for the current tenant of a lease.
pub(super) fn notify_tenant_tx(
    c: &Connection,
    lease: &str,
    key: &str,
    kind: &str,
    payload: Value,
    at_ms: i64,
) -> Result<(), AppError> {
    c.execute(
        "INSERT OR IGNORE INTO user_notification_events(id,scope,recipient_email,category,event_kind,object_kind,object_id,payload_json,created_at_ms)
         SELECT 'lease:'||r.id||':'||?2,'user',r.tenant_email,'client',?3,'client_lease',r.id,
                json_patch(json_object('clientLabel',r.alias),?4),?5
           FROM online_client_leases r WHERE r.id=?1",
        params![lease, key, kind, payload.to_string(), at_ms],
    )
    .market()?;
    Ok(())
}

/// Apply a reclaim's "don't accept this tenant again" choice, once the reclaim
/// takes effect (executed, or the tenant left during the notice).
pub(super) fn deny_buyer_if_requested_tx(c: &Connection, lease: &str) -> Result<(), AppError> {
    let row: Option<(String, String, String, String)> = c
        .query_row(
            "SELECT r.seller_id,l.owner_email,r.buyer_id,r.tenant_email
               FROM online_client_leases r JOIN online_client_listings l ON l.installation_id=r.installation_id
              WHERE r.id=?1 AND r.reclaim_deny_buyer=1 AND r.reclaim_cancelled_at IS NULL
                AND r.reclaim_effective_at IS NOT NULL AND r.buyer_id!=r.seller_id",
            params![lease],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .market()?;
    let Some((seller, seller_email, buyer, buyer_email)) = row else {
        return Ok(());
    };
    let at = Utc::now().to_rfc3339();
    // The access decision is a courtesy to the seller: if it cannot be written
    // (e.g. a legacy email), the reclaim and the tenant's exit still proceed.
    c.execute_batch("SAVEPOINT client_reclaim_deny").market()?;
    let result = [crate::market_access::PRICING_FREE, crate::market_access::PRICING_PAID]
        .into_iter()
        .try_for_each(|pricing| {
            crate::market_access::set_product_access_decision_tx(
                c,
                &seller,
                &seller_email,
                &buyer,
                &buyer_email,
                crate::market_access::PRODUCT_CLIENT_HOST,
                pricing,
                crate::market_access::DECISION_DENY,
                &seller,
                &at,
            )
            .map(drop)
        });
    match result {
        Ok(()) => c.execute_batch("RELEASE client_reclaim_deny").market()?,
        Err(error) => {
            c.execute_batch("ROLLBACK TO client_reclaim_deny; RELEASE client_reclaim_deny")
                .market()?;
            tracing::warn!(lease_id = lease, error = %error, "Client reclaim could not deny the tenant");
        }
    }
    Ok(())
}

/// Rental history visible to both parties in the lease's event list.
pub(super) fn lease_event_tx(c: &Connection, lease: &str, key: &str, kind: &str, at: i64) -> Result<(), AppError> {
    c.execute(
        "INSERT OR IGNORE INTO online_client_events(lease_id,event_key,kind,recorded_at) VALUES(?1,?2,?3,?4)",
        params![lease, key, kind, at],
    )
    .market()?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Reclaim {
    mode: String,
    reason: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    deny_future_access: bool,
    #[serde(default = "default_true")]
    unlist_after: bool,
}
fn default_true() -> bool {
    true
}

pub(super) async fn reclaim(
    State(s): State<ServerState>,
    Path(lease): Path<String>,
    h: HeaderMap,
    Json(v): Json<Reclaim>,
) -> Reply {
    let a = actor(&s, &h).await?;
    let note = v.note.trim();
    if !matches!(v.mode.as_str(), "notice" | "immediate")
        || !matches!(v.reason.as_str(), "need_machine" | "tenant_violation" | "other")
        || note.chars().count() > 2000
    {
        return Err(AppError::BadRequest("invalid reclaim request".into()));
    }
    if v.mode == "immediate" && (v.reason != "tenant_violation" || note.is_empty()) {
        return Err(AppError::coded_bad_request(
            "CLIENT_RECLAIM_EXPLANATION_REQUIRED",
            "an immediate reclaim needs a documented tenant violation",
            json!({}),
        ));
    }
    let c = s.store.conn.lock().await;
    let tx = c.transaction().market()?;
    // `paid_end`: when the rental ends on its own once renewal is off and a
    // running trial is made trial-only (below) — the current paid month, the
    // trial's remaining time, or a fixed free term. Open-ended free rentals
    // have none (grant_until is only a rolling window for them).
    let (installation, state, buyer, cutoff, pending, paid_end): (String, String, String, Option<i64>, bool, Option<i64>) = tx
        .query_row(
            "SELECT r.installation_id,r.state,r.buyer_id,r.cutoff_at,
                    r.reclaim_effective_at IS NOT NULL AND r.reclaim_cancelled_at IS NULL,
                    CASE WHEN c.id IS NULL AND q.free_days IS NOT NULL THEN r.grant_until
                         WHEN c.status IN ('pending_activation','trialing') AND c.paid_started_at IS NULL
                              AND c.trial_seconds_remaining>0 THEN unixepoch()+c.trial_seconds_remaining
                         WHEN c.status='active' THEN unixepoch(p.period_end) END
               FROM online_client_leases r JOIN online_client_quotes q ON q.id=r.quote_id
               LEFT JOIN market_platform_contracts c ON c.product_kind='client_host' AND c.product_ref=r.id
               LEFT JOIN market_platform_periods p ON p.contract_id=c.id AND p.sequence=c.current_period_sequence
              WHERE r.id=?1 AND r.seller_id=?2 AND r.released_at IS NULL",
            params![lease, a.user_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()
        .market()?
        .ok_or_else(missing)?;
    if buyer == a.user_id {
        return Err(conflict("end your own rental from My rentals"));
    }
    let terminated: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM client_market_terminations WHERE installation_id=?1)",
            params![lease],
            |r| r.get(0),
        )
        .market()?;
    if !matches!(state.as_str(), "starting" | "active") || cutoff.is_some() || terminated {
        return Err(AppError::coded_conflict(
            "CLIENT_RENTAL_ENDING",
            "this rental is already ending",
            json!({}),
        ));
    }
    // Escalating a pending notice to an immediate reclaim is allowed; a second
    // notice would only move the date.
    if pending && v.mode == "notice" {
        return Err(AppError::coded_conflict(
            "CLIENT_RECLAIM_PENDING",
            "a reclaim is already scheduled",
            json!({}),
        ));
    }
    let now = stamp();
    // A rental that never started has nothing to hand over. With notice, the
    // reclaim happens after 24 hours or when the paid time runs out, whichever
    // comes first: renewal is switched off below, so the rental cannot run on.
    let effective = if v.mode == "immediate" || state == "starting" {
        now
    } else {
        match paid_end {
            Some(end) if end > now => (now + RECLAIM_NOTICE_SECS).min(end),
            _ => now + RECLAIM_NOTICE_SECS,
        }
    };
    let auto_renew: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM market_platform_contracts
              WHERE product_kind='client_host' AND product_ref=?1 AND renewal_policy='automatic')",
            params![lease],
            |r| r.get(0),
        )
        .market()?;
    let at = Utc::now().to_rfc3339();
    // A trial set to continue would start a paid month during the notice;
    // make it trial-only so the trial ending ends the rental, free of charge.
    let trial_continue = tx
        .execute(
            "UPDATE market_platform_contracts SET after_trial='end',updated_at=?2
             WHERE product_kind='client_host' AND product_ref=?1 AND after_trial='continue'
               AND status IN ('pending_activation','trialing') AND paid_started_at IS NULL",
            params![lease, at],
        )
        .market()?
        == 1;
    tx.execute(
        "UPDATE online_client_leases SET reclaim_requested_at=?2,reclaim_effective_at=?3,reclaim_mode=?4,
             reclaim_reason=?5,reclaim_note=?6,reclaim_deny_buyer=?7,reclaim_unlist_after=?8,reclaim_cancelled_at=NULL,
             reclaim_restore_auto_renew=CASE WHEN ?9 THEN reclaim_restore_auto_renew ELSE ?10 END,
             reclaim_restore_continue=CASE WHEN ?9 THEN reclaim_restore_continue ELSE ?11 END
         WHERE id=?1",
        params![lease, now, effective, v.mode, v.reason, note, v.deny_future_access, v.unlist_after, pending, auto_renew, trial_continue],
    )
    .market()?;
    tx.execute(
        "UPDATE market_platform_contracts SET renewal_policy='manual',updated_at=?2
         WHERE product_kind='client_host' AND product_ref=?1 AND renewal_policy='automatic'",
        params![lease, at],
    )
    .market()?;
    lease_event_tx(&tx, &lease, &format!("reclaim:{now}:scheduled"), "reclaim_scheduled", now)?;
    if effective <= now {
        // "Don't accept this tenant again" takes effect when the reclaim
        // happens, so a cancelled notice leaves the buyer's access untouched.
        deny_buyer_if_requested_tx(&tx, &lease)?;
        lease_event_tx(&tx, &lease, &format!("reclaim:{now}:executed"), "reclaim_executed", now)?;
        notify_tenant_tx(
            &tx,
            &lease,
            &format!("reclaim:{now}:executed"),
            "client_lease_reclaimed",
            json!({"reason": v.reason}),
            now * 1000,
        )?;
        reconcile::fence(&tx, &installation, &lease, now)?;
    } else {
        notify_tenant_tx(
            &tx,
            &lease,
            &format!("reclaim:{now}:scheduled"),
            "client_lease_reclaim_scheduled",
            json!({"effectiveAt": effective, "reason": v.reason}),
            now * 1000,
        )?;
    }
    tx.commit().market()?;
    s.dashboard_cache.invalidate();
    Ok(Json(json!({"leaseId": lease, "reclaimEffectiveAt": effective, "state": if effective <= now { "ending" } else { state.as_str() }})))
}

pub(super) async fn cancel_reclaim(
    State(s): State<ServerState>,
    Path(lease): Path<String>,
    h: HeaderMap,
) -> Reply {
    let a = actor(&s, &h).await?;
    let c = s.store.conn.lock().await;
    let tx = c.transaction().market()?;
    let now = stamp();
    let changed = tx
        .execute(
            "UPDATE online_client_leases SET reclaim_cancelled_at=?3
             WHERE id=?1 AND seller_id=?2 AND state IN ('starting','active') AND cutoff_at IS NULL
               AND reclaim_effective_at>?3 AND reclaim_cancelled_at IS NULL",
            params![lease, a.user_id, now],
        )
        .market()?;
    if changed != 1 {
        return Err(conflict("no reclaim can be cancelled"));
    }
    // The reclaim made a continuing trial trial-only; put the choice back.
    let continue_restored = tx
        .execute(
            "UPDATE market_platform_contracts SET after_trial='continue',updated_at=?2
             WHERE product_kind='client_host' AND product_ref=?1 AND after_trial='end'
               AND status IN ('pending_activation','trialing') AND paid_started_at IS NULL
               AND EXISTS(SELECT 1 FROM online_client_leases r WHERE r.id=?1 AND r.reclaim_restore_continue=1)",
            params![lease, Utc::now().to_rfc3339()],
        )
        .market()?
        == 1;
    // The reclaim turned the tenant's automatic renewal off; put it back.
    let restored = tx
        .execute(
            "UPDATE market_platform_contracts SET renewal_policy='automatic',updated_at=?2
             WHERE product_kind='client_host' AND product_ref=?1 AND renewal_policy='manual'
               AND status IN ('pending_activation','trialing','active')
               AND EXISTS(SELECT 1 FROM online_client_leases r WHERE r.id=?1 AND r.reclaim_restore_auto_renew=1)",
            params![lease, Utc::now().to_rfc3339()],
        )
        .market()?
        == 1;
    tx.execute(
        "UPDATE online_client_leases SET reclaim_restore_auto_renew=0,reclaim_restore_continue=0 WHERE id=?1",
        params![lease],
    )
    .market()?;
    lease_event_tx(&tx, &lease, &format!("reclaim:{now}:cancelled"), "reclaim_cancelled", now)?;
    notify_tenant_tx(
        &tx,
        &lease,
        &format!("reclaim:{now}:cancelled"),
        "client_lease_reclaim_cancelled",
        json!({ "renewalRestored": restored, "continueRestored": continue_restored }),
        now * 1000,
    )?;
    tx.commit().market()?;
    s.dashboard_cache.invalidate();
    Ok(Json(json!({"leaseId": lease, "cancelled": true, "renewalRestored": restored || continue_restored})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ListingUpdate {
    cycle_price_minor: i64,
    free_days: Option<u32>,
    #[serde(default)]
    trial_days: u32,
    #[serde(default)]
    title: Option<String>,
}

/// New terms apply to the next order only; signed contracts keep theirs.
pub(super) async fn update_listing(
    State(s): State<ServerState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(v): Json<ListingUpdate>,
) -> Reply {
    let a = actor(&s, &h).await?;
    validate_terms(v.cycle_price_minor, v.free_days, v.trial_days)?;
    let title = normalize_title(v.title.as_deref())?;
    let c = s.store.conn.lock().await;
    let tx = c.transaction().market()?;
    // Going paid claims the seller's provider identity, as a paid listing does.
    let tx = if v.cycle_price_minor > 0 { super::ensure_paid_provider(&c, tx, &a)? } else { tx };
    let current: (i64, Option<u32>, u32) = tx
        .query_row(
            "SELECT cycle_price_minor,free_days,trial_days FROM online_client_listings
             WHERE installation_id=?1 AND seller_id=?2 AND state='listed'",
            params![id, a.user_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .market()?
        .ok_or_else(|| conflict("only a listed Client can change its offer"))?;
    let terms_changed = current != (v.cycle_price_minor, v.free_days, v.trial_days);
    tx.execute(
        "UPDATE online_client_listings SET cycle_price_minor=?3,free_days=?4,trial_days=?5,title=?6,
             revision=revision+?7,updated_at=unixepoch()
         WHERE installation_id=?1 AND seller_id=?2 AND state='listed'",
        params![id, a.user_id, v.cycle_price_minor, v.free_days, v.trial_days, title, i64::from(terms_changed)],
    )
    .market()?;
    tx.commit().market()?;
    s.dashboard_cache.invalidate();
    Ok(Json(json!({"installationId": id, "termsChanged": terms_changed})))
}

/// Pausing stops new orders and invalidates outstanding quotes. The current
/// rental, if any, is not affected.
pub(super) async fn pause(State(s): State<ServerState>, Path(id): Path<String>, h: HeaderMap) -> Reply {
    let a = actor(&s, &h).await?;
    let c = s.store.conn.lock().await;
    let changed = c
        .execute(
            "UPDATE online_client_listings SET paused_at=unixepoch(),revision=revision+1,updated_at=unixepoch()
             WHERE installation_id=?1 AND seller_id=?2 AND state='listed' AND paused_at IS NULL",
            params![id, a.user_id],
        )
        .market()?;
    if changed != 1 {
        return Err(conflict("only an active listing can pause"));
    }
    s.dashboard_cache.invalidate();
    Ok(Json(json!({"installationId": id, "paused": true})))
}

pub(super) async fn resume(State(s): State<ServerState>, Path(id): Path<String>, h: HeaderMap) -> Reply {
    let a = actor(&s, &h).await?;
    let c = s.store.conn.lock().await;
    let changed = c
        .execute(
            "UPDATE online_client_listings SET paused_at=NULL,updated_at=unixepoch()
             WHERE installation_id=?1 AND seller_id=?2 AND state='listed' AND paused_at IS NOT NULL",
            params![id, a.user_id],
        )
        .market()?;
    if changed != 1 {
        return Err(conflict("this listing is not paused"));
    }
    s.dashboard_cache.invalidate();
    Ok(Json(json!({"installationId": id, "paused": false})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_short_single_line_and_not_contacts() {
        assert_eq!(normalize_title(Some("  东京 · 稳定在线 ")).unwrap().as_deref(), Some("东京 · 稳定在线"));
        assert_eq!(normalize_title(Some("   ")).unwrap(), None);
        assert_eq!(normalize_title(None).unwrap(), None);
        assert!(normalize_title(Some(&"长".repeat(31))).is_err());
        assert!(normalize_title(Some(&"长".repeat(30))).is_ok());
        for bad in ["a\nb", "see https://x.y", "mail me@x.io", "www.shop", "visit example.com", "tab\there"] {
            assert!(normalize_title(Some(bad)).is_err(), "{bad}");
        }
        assert!(normalize_title(Some("Tokyo v1.2 x86_64")).is_ok());
    }

    #[test]
    fn offer_terms_follow_the_listing_rules() {
        assert!(validate_terms(0, None, 0).is_ok());
        assert!(validate_terms(0, Some(30), 0).is_ok());
        assert!(validate_terms(500, None, 7).is_ok());
        assert!(validate_terms(500, Some(3), 0).is_err());
        assert!(validate_terms(0, None, 3).is_err());
        assert!(validate_terms(500, None, 31).is_err());
        assert!(validate_terms(-1, None, 0).is_err());
    }

    #[test]
    fn paused_listings_stop_new_orders_and_titles_are_checked_by_the_database() {
        let conn = crate::test_support::database().unwrap();
        crate::test_support::online_listing(&conn, "physical", "seller", "seller@example.test");
        let read = || -> (String, String) {
            conn.query_row(
                "SELECT availability, supply_status FROM online_client_supply WHERE installation_id='physical'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
        };
        assert_eq!(read().0, "available");
        conn.execute("UPDATE online_client_listings SET paused_at=unixepoch() WHERE installation_id='physical'", [])
            .unwrap();
        assert_eq!(read(), ("paused".to_owned(), "maintaining".to_owned()));
        conn.execute("UPDATE online_client_listings SET paused_at=NULL WHERE installation_id='physical'", [])
            .unwrap();
        assert_eq!(read().0, "available");
        let title = |value: &str| {
            conn.execute(
                "UPDATE online_client_listings SET title=?1 WHERE installation_id='physical'",
                params![value],
            )
        };
        assert!(title("东京 · 稳定在线").is_ok());
        let long = "长".repeat(31);
        for bad in [" padded", "two\nlines", "", long.as_str()] {
            assert!(title(bad).is_err(), "{bad:?}");
        }
    }
}
