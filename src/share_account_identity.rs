//! Browser-facing account identity only. Never apply this projection to stored
//! descriptors, signed payloads, routing decisions or immutable service snapshots.

use std::collections::BTreeMap;

use crate::models::{
    ShareAppProvider, ShareAppProviders, ShareAppRuntimes, ShareUpstreamProvider,
    ShareUpstreamQuota, ShareUserGrant, ShareView, UserModelRoutingShareCapability,
};

pub(crate) fn is_share_owner(owner: Option<&str>, viewer: Option<&str>) -> bool {
    owner.zip(viewer).is_some_and(|(owner, viewer)| {
        !owner.trim().is_empty() && owner.trim().eq_ignore_ascii_case(viewer.trim())
    })
}

pub(crate) fn active_share_grant<'a>(
    grants: &'a BTreeMap<String, ShareUserGrant>,
    email: &str,
    now_ms: i64,
) -> Option<&'a ShareUserGrant> {
    grants.iter().find_map(|(key, grant)| {
        (grant.active
            && grant.role == "shareto"
            && grant.policy.expires_at.is_none_or(|expiry| expiry > now_ms)
            && (key.eq_ignore_ascii_case(email) || grant.email.eq_ignore_ascii_case(email)))
        .then_some(grant)
    })
}

/// Authentication is required even for a free Share. Evaluate against the
/// current descriptor, before its grants are trimmed for the browser.
pub(crate) fn can_view_account_identity(
    owner: Option<&str>,
    free_access: bool,
    grants: &BTreeMap<String, ShareUserGrant>,
    viewer: Option<&str>,
    now_ms: i64,
) -> bool {
    let Some(viewer) = viewer.map(str::trim).filter(|email| !email.is_empty()) else {
        return false;
    };
    is_share_owner(owner, Some(viewer))
        || free_access
        || active_share_grant(grants, viewer, now_ms).is_some_and(|grant| {
            grant
                .pending_value_quota
                .as_ref()
                .is_none_or(|pending| pending.retain_access)
        })
}

/// Fixed-size masks hide the original length. Non-email identities retain one
/// Unicode character at each end; a one-character identity retains its prefix.
pub(crate) fn mask_account_identity(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() || value == "-" {
        return value.to_string();
    }
    if let Some((local, domain)) = value.rsplit_once('@')
        && !local.is_empty()
        && !domain.is_empty()
        && !value.chars().any(char::is_whitespace)
    {
        let local: Vec<_> = local.chars().collect();
        let masked = if local.len() <= 2 || local.iter().all(|c| *c == '*') {
            "***".to_string()
        } else {
            format!("{}***{}", local[0], local[local.len() - 1])
        };
        return format!("{masked}@{domain}");
    }
    let chars: Vec<_> = value.chars().collect();
    // Preserve the common already-masked mobile number emitted by Server.
    if chars.len() == 11
        && chars[..3].iter().all(char::is_ascii_digit)
        && chars[3..7].iter().all(|c| *c == '*')
        && chars[7..].iter().all(char::is_ascii_digit)
    {
        return value.to_string();
    }
    if chars.iter().all(|c| *c == '*') {
        return "****".to_string();
    }
    if chars.len() == 1 || (chars.len() == 5 && chars[1..].iter().all(|c| *c == '*')) {
        return format!("{}****", chars[0]);
    }
    format!("{}****{}", chars[0], chars[chars.len() - 1])
}

/// Account aliases can also be copied into a Provider's display name or a
/// credential/plan message. Scrub these display-only fallbacks together.
#[derive(Default)]
pub(crate) struct AccountAliases(Vec<(String, String)>);

impl AccountAliases {
    pub(crate) fn from_metadata(
        upstream: Option<&ShareUpstreamProvider>,
        runtimes: &ShareAppRuntimes,
        providers: &ShareAppProviders,
    ) -> Self {
        let mut aliases = Self::default();
        for runtime in [
            upstream,
            runtimes.claude.as_ref(),
            runtimes.codex.as_ref(),
            runtimes.gemini.as_ref(),
            runtimes.kiro.as_ref(),
            runtimes.cursor.as_ref(),
            runtimes.antigravity.as_ref(),
            runtimes.copilot.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            aliases.add(runtime.account_label.as_deref());
            aliases.add(runtime.account_email.as_deref());
        }
        for provider in providers
            .claude
            .iter()
            .chain(&providers.codex)
            .chain(&providers.gemini)
        {
            aliases.add(provider.account_label.as_deref());
            aliases.add(provider.account_email.as_deref());
        }
        aliases
    }

    pub(crate) fn add(&mut self, value: Option<&str>) {
        let Some(value) = value.map(str::trim).filter(|v| !v.is_empty() && *v != "-") else {
            return;
        };
        if !self.0.iter().any(|(raw, _)| raw == value) {
            self.0.push((value.into(), mask_account_identity(value)));
            self.0.sort_by_key(|(raw, _)| std::cmp::Reverse(raw.len()));
        }
    }

    pub(crate) fn display_text(&self, value: &str) -> String {
        // Scan the original text once: short aliases must also be scrubbed,
        // without replacing characters inside a mask we have just emitted.
        let mut result = String::with_capacity(value.len());
        let mut remaining = value;
        while !remaining.is_empty() {
            if let Some((_, masked)) = self
                .0
                .iter()
                .find(|(_, masked)| remaining.starts_with(masked))
            {
                result.push_str(masked);
                remaining = &remaining[masked.len()..];
            } else if let Some((raw, masked)) = self.0.iter().find(|(raw, _)| {
                if !remaining.starts_with(raw) {
                    return false;
                }
                // A one-letter ASCII ID must not corrupt ordinary words such
                // as Claude or plan. Its isolated account token is still masked.
                if raw.len() == 1 && raw.as_bytes()[0].is_ascii_alphanumeric() {
                    let previous = value[..value.len() - remaining.len()].chars().next_back();
                    let next = remaining[raw.len()..].chars().next();
                    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
                    return !previous.is_some_and(word) && !next.is_some_and(word);
                }
                true
            }) {
                result.push_str(masked);
                remaining = &remaining[raw.len()..];
            } else {
                let character = remaining.chars().next().unwrap();
                result.push(character);
                remaining = &remaining[character.len_utf8()..];
            }
        }
        // A legacy Provider name can itself be an email even with no explicit
        // account metadata. Such a fallback must not publish the email either.
        crate::notifications::mask_email_like_tokens(&result)
    }

    fn display_option(&self, value: &mut Option<String>) {
        if let Some(text) = value {
            *text = self.display_text(text);
        }
    }

    fn quota(&self, quota: &mut Option<ShareUpstreamQuota>) {
        if let Some(quota) = quota {
            self.display_option(&mut quota.plan);
        }
    }

    fn runtime(&self, runtime: &mut ShareUpstreamProvider) {
        mask_option(&mut runtime.account_label);
        mask_option(&mut runtime.account_email);
        self.display_option(&mut runtime.provider_name);
        self.display_option(&mut runtime.subscription_level);
        self.quota(&mut runtime.quota);
    }

    fn provider(&self, provider: &mut ShareAppProvider) {
        mask_option(&mut provider.account_label);
        mask_option(&mut provider.account_email);
        provider.name = self.display_text(&provider.name);
        if let Some(identity) = &mut provider.provider_identity {
            identity.display_name = self.display_text(&identity.display_name);
        }
        self.display_option(&mut provider.subscription_level);
        self.quota(&mut provider.quota);
    }
}

fn mask_option(value: &mut Option<String>) {
    *value = value
        .as_deref()
        .map(mask_account_identity)
        .filter(|s| !s.is_empty());
}

pub(crate) fn project_share(view: &mut ShareView) {
    if view.can_manage {
        return;
    }
    let aliases = AccountAliases::from_metadata(
        view.upstream_provider.as_ref(),
        &view.app_runtimes,
        &view.app_providers,
    );
    // A Share grant is not access to unrelated legacy Provider inventory.
    // Current Server descriptors only carry bound Providers, but older stored
    // inventories may contain other accounts.
    let reveal = view.can_view_account_identity;
    let mut bound_accounts = Vec::new();
    let mut bound_apps = Vec::new();
    for (app, runtime, providers) in [
        (
            "claude",
            view.app_runtimes.claude.as_ref(),
            &view.app_providers.claude,
        ),
        (
            "codex",
            view.app_runtimes.codex.as_ref(),
            &view.app_providers.codex,
        ),
        (
            "gemini",
            view.app_runtimes.gemini.as_ref(),
            &view.app_providers.gemini,
        ),
    ] {
        let provider = providers
            .iter()
            .find(|provider| view.bindings.get(app) == Some(&provider.id));
        if reveal
            && view
                .bindings
                .get(app)
                .is_some_and(|id| !id.trim().is_empty())
            && (providers.is_empty() || provider.is_some())
        {
            bound_apps.push(app);
            for account in [
                runtime.and_then(|runtime| runtime.account_label.as_deref()),
                runtime.and_then(|runtime| runtime.account_email.as_deref()),
                provider.and_then(|provider| provider.account_label.as_deref()),
                provider.and_then(|provider| provider.account_email.as_deref()),
            ]
            .into_iter()
            .flatten()
            .filter(|value| !value.trim().is_empty() && *value != "-")
            {
                bound_accounts.push(account.to_string());
            }
        }
    }
    for (app, providers) in [
        ("claude", &mut view.app_providers.claude),
        ("codex", &mut view.app_providers.codex),
        ("gemini", &mut view.app_providers.gemini),
    ] {
        for provider in providers {
            if !reveal || view.bindings.get(app) != Some(&provider.id) {
                aliases.provider(provider);
            }
        }
    }
    let runtimes = &mut view.app_runtimes;
    for (app, runtime) in [
        ("claude", &mut runtimes.claude),
        ("codex", &mut runtimes.codex),
        ("gemini", &mut runtimes.gemini),
    ] {
        if !bound_apps.contains(&app)
            && let Some(runtime) = runtime
        {
            aliases.runtime(runtime);
        }
    }
    for runtime in [
        &mut view.upstream_provider,
        &mut runtimes.kiro,
        &mut runtimes.cursor,
        &mut runtimes.antigravity,
        &mut runtimes.copilot,
    ]
    .into_iter()
    .filter_map(Option::as_mut)
    {
        // Legacy standalone runtimes have no binding ID. Only expose their
        // account when the bound surfaces independently identify that account.
        let accounts: Vec<_> = [
            runtime.account_label.as_ref(),
            runtime.account_email.as_ref(),
        ]
        .into_iter()
        .flatten()
        .filter(|account| !account.trim().is_empty() && account.as_str() != "-")
        .collect();
        let bound = !accounts.is_empty()
            && accounts
                .iter()
                .all(|account| bound_accounts.contains(account));
        if !reveal || !bound {
            aliases.runtime(runtime);
        }
    }
    if !reveal {
        view.provider_identity.display_name =
            aliases.display_text(&view.provider_identity.display_name);
    }
}

pub(crate) fn project_routing_capability(
    capability: &mut UserModelRoutingShareCapability,
    aliases: &AccountAliases,
) {
    mask_option(&mut capability.account_label);
    mask_option(&mut capability.account_email);
    aliases.display_option(&mut capability.provider_name);
    aliases.display_option(&mut capability.subscription_level);
    aliases.quota(&mut capability.quota);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_masks_preserve_non_email_edges_and_are_idempotent() {
        for (raw, expected) in [
            ("alice@example.com", "a***e@example.com"),
            ("a@example.com", "***@example.com"),
            ("ab@example.com", "***@example.com"),
            ("13812345678", "1****8"),
            ("138****0000", "138****0000"),
            ("327d03ae-885d-4b06-b073-df7493b0b194", "3****4"),
            ("中文昵称", "中****称"),
            ("邵义强", "邵****强"),
            ("李雷", "李****雷"),
            ("邵", "邵****"),
            ("中文昵称测试", "中****试"),
            ("😀a界b🦀", "😀****🦀"),
            ("", ""),
            ("  ", ""),
            ("-", "-"),
            ("x", "x****"),
            ("xy", "x****y"),
            ("****", "****"),
            ("********", "****"),
        ] {
            assert_eq!(mask_account_identity(raw), expected, "{raw}");
            assert_eq!(
                mask_account_identity(expected),
                expected,
                "idempotency: {raw}"
            );
        }
    }

    #[test]
    fn short_account_aliases_are_scrubbed_without_rewriting_emitted_masks() {
        let mut aliases = AccountAliases::default();
        for raw in ["邵义强", "李雷", "邵", "abc", "a"] {
            aliases.add(Some(raw));
        }
        for (raw, expected) in [
            ("账号：邵义强 / 李雷", "账号：邵****强 / 李****雷"),
            ("User abc / a", "User a****c / a****"),
            ("邵****强", "邵****强"),
            ("a****c", "a****c"),
            ("Claude plan / a", "Claude plan / a****"),
        ] {
            assert_eq!(aliases.display_text(raw), expected);
            assert_eq!(aliases.display_text(expected), expected);
        }
    }

    #[test]
    fn account_visibility_requires_authentication_and_current_authorization() {
        let now = 1_000;
        let mut grants = BTreeMap::from([(
            "shared@example.com".into(),
            ShareUserGrant {
                email: "shared@example.com".into(),
                role: "shareto".into(),
                active: true,
                ..Default::default()
            },
        )]);
        let owner = Some("owner@example.com");
        for (viewer, free, expected) in [
            (None, false, false),
            (None, true, false),
            (Some(" "), true, false),
            (Some("OWNER@example.com"), false, true),
            (Some("SHARED@example.com"), false, true),
            (Some("other@example.com"), false, false),
            (Some("other@example.com"), true, true),
        ] {
            assert_eq!(
                can_view_account_identity(owner, free, &grants, viewer, now),
                expected
            );
        }
        grants
            .get_mut("shared@example.com")
            .unwrap()
            .policy
            .expires_at = Some(now);
        assert!(!can_view_account_identity(
            owner,
            false,
            &grants,
            Some("shared@example.com"),
            now
        ));
        let grant = grants.get_mut("shared@example.com").unwrap();
        grant.policy.expires_at = None;
        grant.pending_value_quota = Some(crate::models::PendingValueQuota {
            policy: Default::default(),
            effective_from_ms: now + 100,
            retain_access: false,
        });
        assert!(!can_view_account_identity(
            owner,
            false,
            &grants,
            Some("shared@example.com"),
            now
        ));
        grants
            .get_mut("shared@example.com")
            .unwrap()
            .pending_value_quota
            .as_mut()
            .unwrap()
            .retain_access = true;
        assert!(can_view_account_identity(
            owner,
            false,
            &grants,
            Some("shared@example.com"),
            now
        ));
        grants.get_mut("shared@example.com").unwrap().active = false;
        assert!(!can_view_account_identity(
            owner,
            false,
            &grants,
            Some("shared@example.com"),
            now
        ));
    }

    #[test]
    fn account_permission_uses_current_share_owner_only() {
        assert!(is_share_owner(
            Some("Owner@Example.com"),
            Some("owner@example.com")
        ));
        assert!(!is_share_owner(
            Some("owner@example.com"),
            Some("admin@example.com")
        ));
        assert!(!is_share_owner(Some("owner@example.com"), None));
        assert!(!is_share_owner(None, Some("owner@example.com")));
        assert!(!is_share_owner(Some(""), Some("")));
    }
}
