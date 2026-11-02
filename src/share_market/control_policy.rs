//! Managed policies contain Server-materialized timestamps, not just the patch.
//! Only those deterministic defaults may differ; every contractual field stays
//! exact, including USD tariffs and application scope.
use super::*;

pub(super) fn materialize_policy(
    policy: &ShareUserPolicy,
    duration_seconds: Option<u64>,
    applied_at_ms: i64,
) -> Result<ShareUserPolicy, AppError> {
    let applied_at = DateTime::<Utc>::from_timestamp_millis(applied_at_ms)
        .filter(|_| applied_at_ms >= 0)
        .ok_or_else(|| {
            AppError::BadRequest("managed grant appliedAtMs is outside the supported range".into())
        })?;
    let mut effective = policy.clone();
    if effective.token_period_anchor_at_ms.is_none() {
        effective.token_period_anchor_at_ms =
            token_period_anchor_at_ms(policy.token_period, applied_at);
    }
    if let Some(seconds) = duration_seconds {
        effective.expires_at = Some(
            i64::try_from(seconds)
                .ok()
                .and_then(|seconds| seconds.checked_mul(1_000))
                .and_then(|duration| applied_at_ms.checked_add(duration))
                .filter(|expires| DateTime::<Utc>::from_timestamp_millis(*expires).is_some())
                .ok_or_else(|| AppError::BadRequest("managed grant expiry overflowed".into()))?,
        );
    }
    Ok(effective)
}

pub(crate) fn managed_grant_matches(
    operation: &ShareManagedGrantOperation,
    grant: &ShareUserGrant,
) -> bool {
    if !grant.active
        || grant.manager != ShareGrantManager::RouterShareMarket
        || !grant.email.eq_ignore_ascii_case(operation.email.trim())
        || grant.entitlement_id.as_deref() != Some(operation.entitlement_id.as_str())
        || grant.pending_value_quota.is_some()
    {
        return false;
    }
    let Ok(applied_at_ms) = i64::try_from(grant.updated_at_ms) else {
        return false;
    };
    effective_policy_matches(operation, applied_at_ms, &grant.policy)
}

pub(super) fn effective_policy_matches(
    operation: &ShareManagedGrantOperation,
    applied_at_ms: i64,
    policy: &ShareUserPolicy,
) -> bool {
    operation.policy.as_ref().is_some_and(|requested| {
        materialize_policy(requested, operation.duration_seconds, applied_at_ms)
            .is_ok_and(|expected| &expected == policy)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(
        period: ShareTokenPeriod,
        usd: bool,
    ) -> (ShareManagedGrantOperation, ShareUserGrant) {
        let operation: ShareManagedGrantOperation = serde_json::from_value(serde_json::json!({
            "operationId":"operation", "entitlementId":"entitlement", "shareSequence":1,
            "expectedConfigRevision":5, "action":"upsert", "email":"buyer@example.com",
            "policy": {"parallelLimit":1,"tokenPeriod":period,
                "allowedApps":["claude","codex","gemini"]}
        }))
        .unwrap();
        let mut operation = operation;
        if usd {
            operation.policy.as_mut().unwrap().value_quota = Some(ValueQuota {
                limit_usd_micros: "100000000".into(),
                tariff_policy_id: "api-equivalent-v1".into(),
            });
        } else {
            operation.policy.as_mut().unwrap().token_limit = Some(1_000);
        }
        let applied = 1_793_630_462_123;
        let grant = ShareUserGrant {
            email: operation.email.clone(),
            active: true,
            manager: ShareGrantManager::RouterShareMarket,
            entitlement_id: Some(operation.entitlement_id.clone()),
            updated_at_ms: applied as u128,
            policy: materialize_policy(operation.policy.as_ref().unwrap(), None, applied).unwrap(),
            ..ShareUserGrant::default()
        };
        (operation, grant)
    }

    #[test]
    fn initial_token_and_usd_rolling_anchors_match_server_materialization() {
        for period in [ShareTokenPeriod::SevenDays, ShareTokenPeriod::ThirtyDays] {
            for usd in [false, true] {
                let (operation, mut grant) = fixture(period, usd);
                assert_eq!(
                    grant.policy.token_period_anchor_at_ms,
                    Some(1_793_630_460_000)
                );
                assert!(managed_grant_matches(&operation, &grant));
                grant.policy.token_period_anchor_at_ms = Some(1_793_630_520_000);
                assert!(!managed_grant_matches(&operation, &grant));
            }
        }
    }

    #[test]
    fn duration_grants_still_compare_all_terms_and_exact_expiry() {
        let (mut operation, mut grant) = fixture(ShareTokenPeriod::ThirtyDays, true);
        operation.duration_seconds = Some(86_400);
        grant.policy = materialize_policy(
            operation.policy.as_ref().unwrap(),
            operation.duration_seconds,
            grant.updated_at_ms as i64,
        )
        .unwrap();
        assert!(managed_grant_matches(&operation, &grant));
        for field in [
            "parallelLimit",
            "valueQuota",
            "allowedApps",
            "expiresAt",
            "tokenPeriod",
        ] {
            let mut tampered = serde_json::to_value(&grant).unwrap();
            tampered["policy"][field] = match field {
                "parallelLimit" => serde_json::json!(2),
                "valueQuota" => {
                    serde_json::json!({"limitUsdMicros":"200000000","tariffPolicyId":"api-equivalent-v1"})
                }
                "allowedApps" => serde_json::json!(["codex"]),
                "expiresAt" => serde_json::json!(grant.policy.expires_at.unwrap() + 1),
                _ => serde_json::json!("sevenDays"),
            };
            assert!(
                !managed_grant_matches(&operation, &serde_json::from_value(tampered).unwrap()),
                "{field}"
            );
        }
        grant.policy.value_quota.as_mut().unwrap().tariff_policy_id = "other-tariff".into();
        assert!(!managed_grant_matches(&operation, &grant));
    }

    #[test]
    fn explicit_anchors_and_expiry_are_preserved_on_replay_and_resume() {
        let (mut operation, mut grant) = fixture(ShareTokenPeriod::SevenDays, false);
        operation.policy = Some(grant.policy.clone());
        operation.policy.as_mut().unwrap().expires_at = Some(1_893_630_462_123);
        grant.policy = operation.policy.clone().unwrap();
        grant.updated_at_ms += 120_000;
        assert!(managed_grant_matches(&operation, &grant));
        grant.policy.token_period_anchor_at_ms = Some(grant.updated_at_ms as i64 / 60_000 * 60_000);
        assert!(!managed_grant_matches(&operation, &grant));
    }

    #[test]
    fn nonrolling_policies_do_not_gain_an_anchor_and_schedules_cannot_activate() {
        for period in [
            ShareTokenPeriod::Lifetime,
            ShareTokenPeriod::Day,
            ShareTokenPeriod::Week,
            ShareTokenPeriod::CalendarMonth,
        ] {
            let (operation, mut grant) = fixture(period, true);
            assert!(managed_grant_matches(&operation, &grant));
            grant.pending_value_quota = Some(crate::models::PendingValueQuota {
                policy: grant.policy.clone(),
                effective_from_ms: 1_893_630_462_123,
                retain_access: false,
            });
            assert!(!managed_grant_matches(&operation, &grant));
            grant.pending_value_quota = None;
            grant.policy.token_period_anchor_at_ms = Some(1_793_630_460_000);
            assert!(!managed_grant_matches(&operation, &grant));
        }
    }

    #[test]
    fn identity_and_clock_overflows_fail_closed() {
        let (mut operation, mut grant) = fixture(ShareTokenPeriod::ThirtyDays, false);
        grant.email = "other@example.com".into();
        assert!(!managed_grant_matches(&operation, &grant));
        grant.email = operation.email.clone();
        grant.updated_at_ms = u128::MAX;
        assert!(!managed_grant_matches(&operation, &grant));
        operation.duration_seconds = Some(u64::MAX);
        assert!(
            materialize_policy(
                operation.policy.as_ref().unwrap(),
                operation.duration_seconds,
                1_000
            )
            .is_err()
        );
        assert!(materialize_policy(operation.policy.as_ref().unwrap(), None, -1).is_err());
    }
}
