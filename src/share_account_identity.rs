//! Browser-facing account identity only. Never apply this projection to stored
//! descriptors, signed payloads, routing decisions or immutable service snapshots.

use crate::models::{
    ShareAppProvider, ShareAppProviders, ShareAppRuntimes, ShareUpstreamProvider,
    ShareUpstreamQuota, ShareView, UserModelRoutingShareCapability,
};

pub(crate) fn is_share_owner(owner: Option<&str>, viewer: Option<&str>) -> bool {
    owner.zip(viewer).is_some_and(|(owner, viewer)| {
        !owner.trim().is_empty() && owner.trim().eq_ignore_ascii_case(viewer.trim())
    })
}

/// Fixed-size masks hide the original length. Work in Unicode scalar values,
/// not byte offsets; short identities must not disclose all their characters.
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
    if (10..=15).contains(&chars.len()) && chars.iter().all(char::is_ascii_digit) {
        return format!(
            "{}****{}",
            collect(&chars[..3]),
            collect(&chars[chars.len() - 4..])
        );
    }
    if chars.len() <= 4 || chars.iter().all(|c| *c == '*') {
        return "****".to_string();
    }
    let visible = if chars.len() >= 12 { 4 } else { 1 };
    format!(
        "{}****{}",
        collect(&chars[..visible]),
        collect(&chars[chars.len() - visible..])
    )
}

fn collect(chars: &[char]) -> String {
    chars.iter().collect()
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
        let mut result = value.to_string();
        for (raw, masked) in &self.0 {
            if result.trim() == raw {
                result = masked.clone();
            } else if raw.chars().count() >= 4 {
                result = result.replace(raw, masked);
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

pub(crate) fn project_share(view: &mut ShareView, viewer: Option<&str>) {
    if is_share_owner(view.owner_email.as_deref(), viewer) {
        return;
    }
    let aliases = AccountAliases::from_metadata(
        view.upstream_provider.as_ref(),
        &view.app_runtimes,
        &view.app_providers,
    );
    let runtimes = &mut view.app_runtimes;
    let runtimes = [
        &mut view.upstream_provider,
        &mut runtimes.claude,
        &mut runtimes.codex,
        &mut runtimes.gemini,
        &mut runtimes.kiro,
        &mut runtimes.cursor,
        &mut runtimes.antigravity,
        &mut runtimes.copilot,
    ];
    let providers = &mut view.app_providers;
    let providers = [
        &mut providers.claude,
        &mut providers.codex,
        &mut providers.gemini,
    ];
    for runtime in runtimes.into_iter().filter_map(Option::as_mut) {
        aliases.runtime(runtime);
    }
    for provider in providers.into_iter().flat_map(|p| p.iter_mut()) {
        aliases.provider(provider);
    }
    view.provider_identity.display_name =
        aliases.display_text(&view.provider_identity.display_name);
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
    fn account_masks_are_unicode_safe_short_value_safe_and_idempotent() {
        for (raw, expected) in [
            ("alice@example.com", "a***e@example.com"),
            ("a@example.com", "***@example.com"),
            ("ab@example.com", "***@example.com"),
            ("13812345678", "138****5678"),
            ("138****0000", "138****0000"),
            ("327d03ae-885d-4b06-b073-df7493b0b194", "327d****b194"),
            ("中文昵称", "****"),
            ("中文昵称测试", "中****试"),
            ("😀a界b🦀", "😀****🦀"),
            ("", ""),
            ("  ", ""),
            ("-", "-"),
            ("x", "****"),
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
