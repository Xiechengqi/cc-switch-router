//! Display-only provider identity. Never used by routing, health or rental snapshots.
use std::{collections::BTreeMap, sync::OnceLock};

use serde::{Deserialize, Serialize};

use crate::models::{ShareAppProvider, ShareAppProviders, ShareDescriptor};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderIdentity {
    pub schema_version: u32,
    pub family_id: String,
    pub category: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_color: Option<String>,
    pub catalog_version: String,
    pub revision: u64,
}

/// Safe public subset: no Bundle id, provider id, account or credential source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicProviderIdentity {
    pub schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family_id: Option<String>,
    pub category: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_color: Option<String>,
    pub catalog_version: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Catalog {
    catalog_version: String,
    families: BTreeMap<String, Family>,
}
#[derive(Deserialize)]
struct Family {
    category: String,
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../assets/provider-presentation.json"))
            .expect("validated provider presentation catalog")
    })
}

fn neutral(category: &str) -> PublicProviderIdentity {
    PublicProviderIdentity {
        schema_version: 1,
        family_id: None,
        category: category.into(),
        display_name: if category == "legacy_mixed" {
            "Legacy mixed providers"
        } else {
            "Unknown provider"
        }
        .into(),
        icon: None,
        icon_color: None,
        catalog_version: catalog().catalog_version.clone(),
    }
}

fn valid(identity: &ProviderIdentity) -> bool {
    identity.schema_version == 1
        && !identity.display_name.trim().is_empty()
        && identity.display_name.len() <= 1024
        && identity.family_id.len() <= 128
        && identity.category.len() <= 64
        && identity.catalog_version.len() <= 128
        && identity.icon.as_ref().is_none_or(|v| v.len() <= 128)
        && identity.icon_color.as_ref().is_none_or(|v| v.len() <= 64)
        && catalog()
            .families
            .get(&identity.family_id)
            .is_some_and(|family| family.category == identity.category)
}

fn providers(apps: &ShareAppProviders) -> impl Iterator<Item = &ShareAppProvider> {
    apps.claude.iter().chain(&apps.codex).chain(&apps.gemini)
}

pub fn identity_for_share(share: &ShareDescriptor) -> PublicProviderIdentity {
    let mut bindings = share.bindings.clone();
    if bindings.is_empty()
        && let Some(id) = share.provider_id.as_ref()
    {
        bindings.insert(share.app_type.clone(), id.clone());
    }
    aggregate(&bindings, &share.app_providers)
}

pub fn identity_from_json(bindings: &str, apps: Option<&str>) -> PublicProviderIdentity {
    let bindings = serde_json::from_str(bindings).unwrap_or_default();
    let apps = apps
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    aggregate(&bindings, &apps)
}

pub fn aggregate(
    bindings: &BTreeMap<String, String>,
    apps: &ShareAppProviders,
) -> PublicProviderIdentity {
    let bound: Vec<_> = bindings
        .iter()
        .filter_map(|(app, id)| providers(apps).find(|p| &p.app == app && &p.id == id))
        .collect();
    if bindings.is_empty() || bound.len() != bindings.len() {
        return neutral("unknown");
    }
    let first = bound[0];
    if bound.len() > 1
        && (first.bundle_id.as_deref().is_none_or(str::is_empty)
            || bound.iter().any(|p| p.bundle_id != first.bundle_id))
    {
        return neutral("legacy_mixed");
    }
    let Some(identity) = first.provider_identity.as_ref().filter(|i| valid(i)) else {
        return neutral("unknown");
    };
    if bound.iter().any(|p| {
        p.provider_identity.as_ref().is_none_or(|i| {
            !valid(i)
                || i.family_id != identity.family_id
                || i.category != identity.category
                || i.display_name != identity.display_name
                || i.icon != identity.icon
                || i.icon_color != identity.icon_color
                || i.catalog_version != identity.catalog_version
        })
    }) {
        return neutral("unknown");
    }
    PublicProviderIdentity {
        schema_version: 1,
        family_id: Some(identity.family_id.clone()),
        category: identity.category.clone(),
        display_name: identity.display_name.clone(),
        icon: identity.icon.clone(),
        icon_color: identity.icon_color.clone(),
        catalog_version: identity.catalog_version.clone(),
    }
}

/// Runtime refreshes are observations, never an authority for static display identity.
/// Matching the full binding prevents a delayed refresh from attaching an old brand to a rebind.
pub fn preserve_runtime_identities(incoming: &mut ShareAppProviders, stored: &ShareAppProviders) {
    for p in incoming
        .claude
        .iter_mut()
        .chain(&mut incoming.codex)
        .chain(&mut incoming.gemini)
    {
        p.provider_identity = providers(stored)
            .find(|old| old.app == p.app && old.id == p.id && old.bundle_id == p.bundle_id)
            .and_then(|old| old.provider_identity.clone());
    }
    // An incomplete/old runtime response must not delete static identity entries.
    // Real removal/rebinds are authored by descriptors, which replace the stored list.
    for (next, previous) in [
        (&mut incoming.claude, &stored.claude),
        (&mut incoming.codex, &stored.codex),
        (&mut incoming.gemini, &stored.gemini),
    ] {
        for old in previous.iter().filter(|p| p.provider_identity.is_some()) {
            if !next.iter().any(|p| p.app == old.app && p.id == old.id) {
                next.push(old.clone());
            }
        }
    }
}

/// Old writers may omit the extension; do not erase an acknowledged identity on downgrade.
pub fn merge_descriptor_identities(incoming: &mut ShareAppProviders, stored: &ShareAppProviders) {
    for p in incoming
        .claude
        .iter_mut()
        .chain(&mut incoming.codex)
        .chain(&mut incoming.gemini)
    {
        if p.provider_identity.is_none()
            && let Some(old) = providers(stored)
                .find(|old| old.app == p.app && old.id == p.id && old.bundle_id == p.bundle_id)
                .and_then(|old| old.provider_identity.as_ref())
        {
            p.provider_identity = Some(old.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn fixtures() -> Vec<Value> {
        serde_json::from_str(include_str!("../assets/provider-identity-fixtures.json")).unwrap()
    }

    #[test]
    fn all_server_fixtures_produce_one_public_identity_without_private_ids() {
        let fixtures = fixtures();
        assert_eq!(fixtures.len(), catalog().families.len());
        for f in fixtures {
            let bindings = serde_json::from_value(f["bindings"].clone()).unwrap();
            let apps = serde_json::from_value(f["appProviders"].clone()).unwrap();
            let identity = aggregate(&bindings, &apps);
            assert_eq!(
                identity.category,
                f["expectedCategory"].as_str().unwrap(),
                "{}",
                f["name"]
            );
            let public = serde_json::to_string(&identity).unwrap();
            for forbidden in [
                "fixture-provider",
                "fixture-private-bundle",
                "\"revision\":",
                "\"accountEmail\":",
                "\"accountId\":",
                "\"bundleId\":",
                "\"providerId\":",
                "\"credentialSource\":",
                "\"apiKey\":",
            ] {
                assert!(!public.contains(forbidden), "{forbidden}");
            }
        }
    }

    fn minimax() -> (BTreeMap<String, String>, ShareAppProviders) {
        let f = fixtures()
            .into_iter()
            .find(|f| f["name"] == "family.minimax_cn")
            .unwrap();
        (
            serde_json::from_value(f["bindings"].clone()).unwrap(),
            serde_json::from_value(f["appProviders"].clone()).unwrap(),
        )
    }

    #[test]
    fn protocol_name_logo_and_enabled_apps_never_select_a_category() {
        let (bindings, mut apps) = minimax();
        for p in apps.claude.iter_mut().chain(&mut apps.codex) {
            p.kind = Some(p.app.clone());
            p.name = "OpenAI Claude Gemini Custom".into();
            p.enabled = false;
            p.provider_identity.as_mut().unwrap().icon = Some("openai".into());
        }
        assert_eq!(aggregate(&bindings, &apps).category, "minimax");
        apps.codex[0].bundle_id = Some("different-bundle".into());
        assert_eq!(aggregate(&bindings, &apps).category, "legacy_mixed");
        for p in &mut apps.claude {
            p.bundle_id = None;
        }
        assert_eq!(aggregate(&bindings, &apps).category, "legacy_mixed");
    }

    #[test]
    fn missing_inconsistent_or_forged_metadata_remains_neutral() {
        let (bindings, apps) = minimax();
        let mut missing = apps.clone();
        missing.codex[0].provider_identity = None;
        assert_eq!(aggregate(&bindings, &missing).category, "unknown");
        let mut forged = apps.clone();
        forged.codex[0].provider_identity.as_mut().unwrap().category = "openai".into();
        assert_eq!(aggregate(&bindings, &forged).category, "unknown");
        let mut inconsistent = apps.clone();
        inconsistent.codex[0]
            .provider_identity
            .as_mut()
            .unwrap()
            .icon_color = Some("#FFFFFF".into());
        assert_eq!(aggregate(&bindings, &inconsistent).category, "unknown");
        assert_eq!(aggregate(&BTreeMap::new(), &apps).category, "unknown");
        assert_eq!(
            identity_from_json("invalid", Some("invalid")).category,
            "unknown"
        );
    }

    #[test]
    fn delayed_runtime_and_old_writers_cannot_erase_or_revert_static_identity() {
        let (_, stored) = minimax();
        let mut incomplete = ShareAppProviders::default();
        preserve_runtime_identities(&mut incomplete, &stored);
        assert_eq!(
            serde_json::to_value(incomplete).unwrap(),
            serde_json::to_value(&stored).unwrap()
        );
        let mut stale = stored.clone();
        stale.codex[0]
            .provider_identity
            .as_mut()
            .unwrap()
            .display_name = "Stale".into();
        stale.claude[0].provider_identity = None;
        preserve_runtime_identities(&mut stale, &stored);
        assert_eq!(
            serde_json::to_value(&stale).unwrap(),
            serde_json::to_value(&stored).unwrap()
        );
        stale.codex[0].provider_identity = None;
        merge_descriptor_identities(&mut stale, &stored);
        assert!(stale.codex[0].provider_identity.is_some());
        stale.codex[0].bundle_id = Some("new-binding".into());
        preserve_runtime_identities(&mut stale, &stored);
        assert!(stale.codex[0].provider_identity.is_none());
        let restored: ShareAppProviders =
            serde_json::from_str(&serde_json::to_string(&stored).unwrap()).unwrap();
        assert_eq!(
            serde_json::to_value(restored).unwrap(),
            serde_json::to_value(stored).unwrap()
        );
    }
}
