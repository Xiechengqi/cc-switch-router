use super::*;
use crate::models::ShareTokenPeriod;
use chrono::{Datelike, TimeZone};

/// Uses the enforcement calendar, not wall-clock days rounded to 30.
fn window(policy: &ShareUserPolicy, at: i64) -> Result<(i64, i64), AppError> {
    let d = Utc
        .timestamp_millis_opt(at)
        .single()
        .ok_or_else(|| AppError::Internal("quota window outside UTC range".into()))?;
    let day = 86_400_000;
    Ok(match policy.token_period {
        ShareTokenPeriod::Lifetime => (0, i64::MAX),
        ShareTokenPeriod::Day => {
            let s = d
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp_millis();
            (s, s + day)
        }
        ShareTokenPeriod::Week => {
            let s = (d.date_naive()
                - Duration::days(i64::from(d.weekday().num_days_from_monday())))
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
            (s, s + 7 * day)
        }
        ShareTokenPeriod::SevenDays | ShareTokenPeriod::ThirtyDays => {
            let length = if policy.token_period == ShareTokenPeriod::SevenDays {
                7 * day
            } else {
                30 * day
            };
            let anchor = policy
                .token_period_anchor_at_ms
                .ok_or_else(|| AppError::Internal("missing quota anchor".into()))?;
            let s = anchor + (at - anchor).div_euclid(length) * length;
            (s, s + length)
        }
        ShareTokenPeriod::CalendarMonth => {
            let s = Utc
                .with_ymd_and_hms(d.year(), d.month(), 1, 0, 0, 0)
                .single()
                .unwrap();
            let e = s
                .checked_add_months(chrono::Months::new(1))
                .ok_or_else(|| AppError::Internal("quota month overflow".into()))?;
            (s.timestamp_millis(), e.timestamp_millis())
        }
        ShareTokenPeriod::AccountReset => {
            return Err(AppError::Conflict(
                "account-reset quota requires a proven settlement window".into(),
            ));
        }
    })
}
fn limit(t: &Termination) -> Result<Option<i128>, AppError> {
    if let Some(q) = &t.policy.value_quota {
        return q
            .limit_picos()
            .map(Some)
            .map_err(|e| AppError::Internal(e.to_string()));
    }
    Ok(t.policy.token_limit.map(i128::from))
}
/// floor(money * numerator / denominator) without multiplying the large USD
/// picos by wallet units. numerator is clamped, all values remain integers.
fn ratio(money: i64, n: i128, d: i128) -> Result<i64, AppError> {
    if money < 0 || n < 0 || d <= 0 || n > d {
        return Err(AppError::Internal(
            "invalid Share settlement fraction".into(),
        ));
    }
    let (mut q, mut r) = (0i128, 0i128);
    for bit in (0..63).rev() {
        let v = r
            .checked_mul(2)
            .and_then(|v| v.checked_add(if (money >> bit) & 1 == 1 { n } else { 0 }))
            .ok_or_else(|| AppError::Internal("Share fraction overflow".into()))?;
        q = q * 2 + v / d;
        r = v % d;
    }
    i64::try_from(q).map_err(|_| AppError::Internal("Share allocation overflow".into()))
}
pub(super) fn validate_usage(t: &Termination, r: &TerminationReceipt) -> Result<i64, AppError> {
    if r.windows.len() > 512 {
        return Err(AppError::BadRequest("too many settlement windows".into()));
    }
    let Some(limit) = limit(t)? else {
        if !r.windows.is_empty() && r.windows.iter().any(|w| w.used != "0") {
            return Ok(0);
        }
        return Ok(0);
    };
    if t.period_amount_units == 0 {
        return Ok(0);
    }
    let scale = 1_000_000i128;
    let (mut cursor, mut budget) = (t.period_start_ms, 0i128);
    if t.policy.token_period == ShareTokenPeriod::Lifetime {
        let before = crate::share_value_pricing::unsigned_decimal(&r.quota_used_before_period)
            .map_err(|e| AppError::BadRequest(e.to_string()))?;
        let remaining = (limit - before).max(0);
        budget = remaining
            .checked_mul(scale)
            .ok_or_else(|| AppError::Internal("quota budget overflow".into()))?;
    } else {
        for _ in 0..512 {
            if cursor >= t.period_end_ms {
                break;
            }
            let (s, e) = window(&t.policy, cursor)?;
            let end = e.min(t.period_end_ms);
            if e <= s || end <= cursor {
                return Err(AppError::Internal("invalid quota window".into()));
            }
            let part = limit
                .checked_mul(i128::from(end - cursor))
                .and_then(|n| n.checked_mul(scale))
                .map(|n| n / i128::from(e - s))
                .ok_or_else(|| AppError::Internal("quota budget overflow".into()))?;
            budget = budget
                .checked_add(part)
                .ok_or_else(|| AppError::Internal("quota budget overflow".into()))?;
            cursor = end;
        }
        if cursor < t.period_end_ms {
            return Err(AppError::Internal(
                "quota period exceeds settlement bound".into(),
            ));
        }
    }
    let (mut used, mut previous) = (0i128, None);
    for w in &r.windows {
        if w.starts_at_ms >= w.ends_at_ms
            || w.starts_at_ms >= t.period_end_ms
            || w.ends_at_ms <= t.period_start_ms
            || previous.is_some_and(|end| w.starts_at_ms < end)
        {
            return Err(AppError::BadRequest(
                "overlapping or unrelated Share usage windows".into(),
            ));
        }
        let expected = if t.policy.token_period == ShareTokenPeriod::Lifetime {
            (t.period_start_ms, t.period_end_ms)
        } else {
            window(&t.policy, w.starts_at_ms.max(t.period_start_ms))?
        };
        if expected != (w.starts_at_ms, w.ends_at_ms) {
            return Err(AppError::BadRequest(
                "Share usage window does not match frozen policy".into(),
            ));
        }
        let amount = crate::share_value_pricing::unsigned_decimal(&w.used)
            .map_err(|e| AppError::BadRequest(e.to_string()))?;
        used = used
            .checked_add(amount)
            .ok_or_else(|| AppError::Internal("Share usage overflow".into()))?;
        previous = Some(w.ends_at_ms);
    }
    if budget <= 0 {
        return Ok(if used > 0 { t.remaining_units } else { 0 });
    }
    let scaled = used
        .checked_mul(scale)
        .ok_or_else(|| AppError::Internal("Share usage overflow".into()))?
        .min(budget);
    let quota_value = ratio(t.period_amount_units, scaled, budget)?;
    // Previously returned credit is never debited again. Additional resource
    // delivery can only consume this period's remaining, funded liability.
    Ok((quota_value - t.time_earned_units)
        .max(0)
        .min(t.remaining_units))
}

pub(super) fn income(
    c: &Connection,
    t: &Termination,
    kind: &str,
    gross: i64,
    at: &str,
) -> Result<(), AppError> {
    if kind == "share_quota" {
        return crate::market_finance::record_share_quota_delivery_tx(c, t, gross, at);
    }
    if kind != "share_termination" || gross < 0 {
        return Err(AppError::Internal(
            "invalid Share compensation award".into(),
        ));
    }
    if gross == 0 {
        return Ok(());
    }
    let period = t
        .period_id
        .as_deref()
        .ok_or_else(|| AppError::Internal("income period missing".into()))?;
    let id = format!("{}:{kind}", t.id);
    let postings = [
        Posting {
            kind: "unfulfilled",
            subject_id: period,
            signed_units: gross,
        },
        Posting {
            kind: "provider_pending",
            subject_id: &t.provider_user_id,
            signed_units: -gross,
        },
    ];
    let j = post_journal_tx(
        c,
        "share_termination",
        &t.id,
        kind,
        &format!("{period}:{kind}:{gross}:0"),
        &postings,
        at,
    )?;
    if !j.inserted {
        return Err(AppError::Internal("Share income projection drift".into()));
    }
    c.execute("INSERT INTO share_termination_income(id,termination_id,provider_user_id,kind,amount_units,commission_units,awarded_at,unlock_at,journal_id)
        VALUES(?1,?2,?3,?4,?5,0,?6,?7,?8)",params![id,t.id,t.provider_user_id,kind,gross,at,(time(at)?+Duration::days(7)).to_rfc3339(),j.id]).map_err(db_error("record Share income"))?;
    event(
        c,
        t,
        None,
        kind,
        json!({"amountUnits":gross.to_string(),"commissionUnits":"0"}),
        Some(&j.id),
        at,
    )
}

pub(super) fn settle(
    c: &Connection,
    mut t: Termination,
    r: &TerminationReceipt,
    at: &str,
) -> Result<(), AppError> {
    if t.returned_units > 0 || t.compensation_units > 0 || t.held_units > 0 {
        return Err(AppError::Conflict(
            "Share termination already financially allocated".into(),
        ));
    }
    if let Some(contract) = t.contract_id.as_deref() {
        crate::market_reconcile::ensure_contract_finance_safe_tx(c, contract)?;
    }
    if let Some(period) = t.period_id.clone() {
        let valid:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM market_platform_periods WHERE id=?1 AND contract_id=?2 AND status='open'
            AND fulfilled_units=?3 AND refunded_units=?4 AND termination_compensation_units=0 AND share_quota_units=0)",
            params![period,t.contract_id,t.time_earned_units,t.prior_refunded_units],|r|r.get(0)).map_err(db_error("verify Share remaining liability"))?;
        if !valid {
            return Err(AppError::Conflict(
                "Share period changed; financial reconciliation required".into(),
            ));
        }
        t.quota_earned_units = validate_usage(&t, r)?;
        income(c, &t, "share_quota", t.quota_earned_units, at)?;
        c.execute(
            "UPDATE market_platform_periods SET share_quota_units=?2 WHERE id=?1",
            params![period, t.quota_earned_units],
        )
        .map_err(db_error("record Share resource delivery"))?;
        t.remaining_units -= t.quota_earned_units;
        t.compensation_cap_units =
            t.compensation_cap_units
                .min(crate::client_market_termination::compensation(
                    t.period_amount_units,
                    t.remaining_units,
                    t.rate_bps,
                )?);
        if let Some(award) = t.decided_award_units {
            t.compensation_cap_units = t.compensation_cap_units.min(award);
        }
        if t.compensation_cap_units == 0 {
            t.responsibility = "none".into();
        }
        let cap = t.compensation_cap_units;
        let return_units = t.remaining_units
            - if matches!(t.responsibility.as_str(), "buyer" | "undetermined") {
                cap
            } else {
                0
            };
        return_money(c, &mut t, return_units, "return_unused", at)?;
        if t.responsibility == "buyer" {
            income(c, &t, "share_termination", cap, at)?;
            t.compensation_units = cap;
            c.execute(
                "UPDATE market_platform_periods SET termination_compensation_units=?2 WHERE id=?1",
                params![period, cap],
            )
            .map_err(db_error("record Share compensation"))?;
        } else if t.responsibility == "undetermined" {
            t.held_units = cap;
        }
        c.execute("UPDATE market_platform_periods SET status='closed',settled_through_at=period_end,updated_at=?2 WHERE id=?1",params![period,at]).map_err(db_error("close Share rent period"))?;
        c.execute("UPDATE market_platform_contracts SET status='ended',ended_at=?2,updated_at=?3 WHERE id=?1",params![t.contract_id,t.cutoff_at,at]).map_err(db_error("end Share rent contract"))?;
    } else if let Some(contract) = t.contract_id.as_deref() {
        let result =
            crate::market_platform_contracts::terminate_after_cleanup_tx(c, contract, true, at)?;
        if !result.finalized {
            return Err(AppError::Conflict(
                "metered Share settlement is pending".into(),
            ));
        }
    }
    t.status = if t.held_units > 0 {
        "disputed"
    } else {
        "completed"
    }
    .into();
    t.updated_at = at.into();
    t.version += 1;
    save(c, &t)?;
    event(
        c,
        &t,
        None,
        "settled",
        json!({"returnedUnits":t.returned_units.to_string()}),
        None,
        at,
    )?;
    notify(c, &t, "settled", at)?;
    Ok(())
}

#[cfg(test)]
mod arithmetic_tests {
    use super::*;
    #[test]
    fn exact_ratio_never_multiplies_large_units() {
        assert_eq!(ratio(100, 60, 100).unwrap(), 60);
        assert_eq!(
            ratio(
                i64::MAX,
                1_000_000_000_000_000_000_000_000_000,
                2_000_000_000_000_000_000_000_000_000
            )
            .unwrap(),
            i64::MAX / 2
        );
    }
}
