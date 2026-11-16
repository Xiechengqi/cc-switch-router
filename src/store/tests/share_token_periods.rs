use super::*;

fn grant_on(email: &str, period: ShareTokenPeriod) -> ShareUserGrant {
    serde_json::from_value(serde_json::json!({
        "email": email,
        "role": "shareto",
        "policy": { "tokenPeriod": period },
    }))
    .unwrap()
}

fn revoked_on(email: &str, period: ShareTokenPeriod) -> ShareUserGrant {
    ShareUserGrant {
        active: false,
        ..grant_on(email, period)
    }
}

#[test]
fn account_reset_grant_keeps_its_period_while_the_client_withdraws_the_offer() {
    let supported = vec![ShareTokenPeriod::Lifetime, ShareTokenPeriod::Day];
    let existing = BTreeMap::from([
        (
            "reset@example.com".to_string(),
            grant_on("reset@example.com", ShareTokenPeriod::AccountReset),
        ),
        (
            "plain@example.com".to_string(),
            grant_on("plain@example.com", ShareTokenPeriod::Lifetime),
        ),
        (
            "revoked@example.com".to_string(),
            revoked_on("revoked@example.com", ShareTokenPeriod::AccountReset),
        ),
    ]);

    // An offered period is always allowed.
    assert!(user_token_period_allowed(
        &supported,
        &existing,
        "plain@example.com",
        ShareTokenPeriod::Day,
    ));
    // The grant already on accountReset stays editable, whatever the case of
    // the email in the patch.
    assert!(user_token_period_allowed(
        &supported,
        &existing,
        " Reset@Example.com ",
        ShareTokenPeriod::AccountReset,
    ));
    // Switching another grant, or adding a new one, onto accountReset still
    // needs the Client's offer.
    assert!(!user_token_period_allowed(
        &supported,
        &existing,
        "plain@example.com",
        ShareTokenPeriod::AccountReset,
    ));
    assert!(!user_token_period_allowed(
        &supported,
        &existing,
        "new@example.com",
        ShareTokenPeriod::AccountReset,
    ));
    // A revoked grant on accountReset does not carry the period back.
    assert!(!user_token_period_allowed(
        &supported,
        &existing,
        "revoked@example.com",
        ShareTokenPeriod::AccountReset,
    ));
    // Other unoffered periods are not unlocked by an accountReset grant.
    assert!(!user_token_period_allowed(
        &supported,
        &existing,
        "reset@example.com",
        ShareTokenPeriod::ThirtyDays,
    ));
}
