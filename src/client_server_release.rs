use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

pub const CLIENT_SERVER_RELEASE_ENV: &str = "CC_SWITCH_ROUTER_CLIENT_SERVER_RELEASE";
pub const DEFAULT_CLIENT_SERVER_RELEASE: &str = "latest";
const RELEASE_API_BASE: &str =
    "https://api.github.com/repos/Xiechengqi/cc-switch-server/releases/tags";
const POSITIVE_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(30);
const UNAVAILABLE_CACHE_TTL: Duration = Duration::from_secs(10);

pub const REQUIRED_RELEASE_ASSETS: [&str; 4] = [
    "cc-switch-server-linux-amd64",
    "cc-switch-server-linux-amd64.sha256",
    "cc-switch-server-linux-arm64",
    "cc-switch-server-linux-arm64.sha256",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientServerReleaseValidationStatus {
    Valid,
    NotFound,
    IncompleteAssets,
    CommitMismatch,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientServerReleaseValidation {
    pub release: String,
    pub valid: bool,
    pub status: ClientServerReleaseValidationStatus,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_commitish: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing_assets: Vec<String>,
    pub checked_at: String,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    target_commitish: String,
    assets: Vec<GithubReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubReleaseAsset {
    name: String,
    state: String,
    size: u64,
}

#[derive(Debug, Clone)]
struct CachedValidation {
    expires_at: Instant,
    value: ClientServerReleaseValidation,
}

/// Validates Router-selected cc-switch-server releases against GitHub.
///
/// A per-selector lock makes concurrent UI validation and Settings PATCH
/// requests share one upstream fetch. Results are cached briefly to avoid
/// exhausting GitHub's anonymous API allowance while still detecting a
/// newly published manual release promptly.
pub struct ClientServerReleaseValidator {
    client: reqwest::Client,
    api_base: String,
    cache: Mutex<HashMap<String, CachedValidation>>,
    selector_locks: Mutex<HashMap<String, Weak<Mutex<()>>>>,
}

impl ClientServerReleaseValidator {
    pub fn new() -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .user_agent("cc-switch-router/0.1 client-server-release-validator")
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            client,
            api_base: RELEASE_API_BASE.to_string(),
            cache: Mutex::new(HashMap::new()),
            selector_locks: Mutex::new(HashMap::new()),
        })
    }

    pub async fn validate(
        &self,
        raw_release: &str,
    ) -> Result<ClientServerReleaseValidation, String> {
        let release = normalize_client_server_release(raw_release)?;
        if let Some(cached) = self.cached(&release).await {
            return Ok(cached);
        }

        let selector_lock = {
            let mut locks = self.selector_locks.lock().await;
            locks.retain(|_, existing| existing.strong_count() > 0);
            if let Some(existing) = locks.get(&release).and_then(Weak::upgrade) {
                existing
            } else {
                let created = Arc::new(Mutex::new(()));
                locks.insert(release.clone(), Arc::downgrade(&created));
                created
            }
        };
        let _flight = selector_lock.lock().await;
        if let Some(cached) = self.cached(&release).await {
            return Ok(cached);
        }

        let value = self.fetch(&release).await;
        let ttl = match value.status {
            ClientServerReleaseValidationStatus::Valid => POSITIVE_CACHE_TTL,
            ClientServerReleaseValidationStatus::Unavailable => UNAVAILABLE_CACHE_TTL,
            _ => NEGATIVE_CACHE_TTL,
        };
        self.cache.lock().await.insert(
            release,
            CachedValidation {
                expires_at: Instant::now() + ttl,
                value: value.clone(),
            },
        );
        Ok(value)
    }

    async fn cached(&self, release: &str) -> Option<ClientServerReleaseValidation> {
        let mut cache = self.cache.lock().await;
        let now = Instant::now();
        cache.retain(|_, entry| entry.expires_at > now);
        cache.get(release).map(|entry| entry.value.clone())
    }

    async fn fetch(&self, release: &str) -> ClientServerReleaseValidation {
        let url = format!("{}/{release}", self.api_base.trim_end_matches('/'));
        let response = match self
            .client
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(release, error = %error, "cc-switch-server release validation request failed");
                return validation_result(
                    release,
                    ClientServerReleaseValidationStatus::Unavailable,
                    "GitHub release validation is temporarily unavailable",
                    None,
                    Vec::new(),
                );
            }
        };

        if let Some(validation) = validation_for_http_status(release, response.status()) {
            if validation.status == ClientServerReleaseValidationStatus::Unavailable {
                tracing::warn!(release, status = %response.status(), "GitHub release validation was rate limited");
            }
            return validation;
        }
        if !response.status().is_success() {
            tracing::warn!(release, status = %response.status(), "GitHub release validation returned an unexpected status");
            return validation_result(
                release,
                ClientServerReleaseValidationStatus::Unavailable,
                "GitHub release validation is temporarily unavailable",
                None,
                Vec::new(),
            );
        }

        let payload = match response.json::<GithubRelease>().await {
            Ok(payload) => payload,
            Err(error) => {
                tracing::warn!(release, error = %error, "GitHub release validation returned an invalid payload");
                return validation_result(
                    release,
                    ClientServerReleaseValidationStatus::Unavailable,
                    "GitHub release validation is temporarily unavailable",
                    None,
                    Vec::new(),
                );
            }
        };
        validate_release_payload(release, payload)
    }
}

fn validation_for_http_status(
    release: &str,
    status: StatusCode,
) -> Option<ClientServerReleaseValidation> {
    match status {
        StatusCode::NOT_FOUND => Some(validation_result(
            release,
            ClientServerReleaseValidationStatus::NotFound,
            "The selected cc-switch-server release does not exist",
            None,
            Vec::new(),
        )),
        StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS => Some(validation_result(
            release,
            ClientServerReleaseValidationStatus::Unavailable,
            "GitHub release validation is temporarily unavailable",
            None,
            Vec::new(),
        )),
        _ => None,
    }
}

pub fn normalize_client_server_release(raw: &str) -> Result<String, String> {
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized == DEFAULT_CLIENT_SERVER_RELEASE {
        return Ok(normalized);
    }
    if normalized.len() == 7 && normalized.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(normalized);
    }
    Err(format!(
        "{CLIENT_SERVER_RELEASE_ENV} must be 'latest' or exactly 7 hexadecimal commit characters"
    ))
}

pub fn client_server_release_from_env() -> String {
    let configured = std::env::var(CLIENT_SERVER_RELEASE_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CLIENT_SERVER_RELEASE.to_string());
    normalize_client_server_release(&configured).unwrap_or_else(|message| panic!("{message}"))
}

fn validate_release_payload(
    release: &str,
    payload: GithubRelease,
) -> ClientServerReleaseValidation {
    let metadata = Some((payload.tag_name.clone(), payload.target_commitish.clone()));
    let tag_matches = payload.tag_name.trim().eq_ignore_ascii_case(release);
    let target = payload.target_commitish.trim().to_ascii_lowercase();
    let target_is_commit =
        (7..=40).contains(&target.len()) && target.bytes().all(|byte| byte.is_ascii_hexdigit());
    let commit_matches = target_is_commit
        && (release == DEFAULT_CLIENT_SERVER_RELEASE || target.starts_with(release));
    if !tag_matches || !commit_matches {
        return validation_result(
            release,
            ClientServerReleaseValidationStatus::CommitMismatch,
            "The release tag does not resolve to the selected commit",
            metadata,
            Vec::new(),
        );
    }

    let missing_assets = REQUIRED_RELEASE_ASSETS
        .iter()
        .filter(|required| {
            !payload.assets.iter().any(|asset| {
                asset.name == **required && asset.state == "uploaded" && asset.size > 0
            })
        })
        .map(|asset| (*asset).to_string())
        .collect::<Vec<_>>();
    if !missing_assets.is_empty() {
        return validation_result(
            release,
            ClientServerReleaseValidationStatus::IncompleteAssets,
            "The selected release is missing required Linux binaries or checksums",
            metadata,
            missing_assets,
        );
    }

    validation_result(
        release,
        ClientServerReleaseValidationStatus::Valid,
        "The selected cc-switch-server release is ready for installation",
        metadata,
        Vec::new(),
    )
}

fn validation_result(
    release: &str,
    status: ClientServerReleaseValidationStatus,
    message: impl Into<String>,
    metadata: Option<(String, String)>,
    missing_assets: Vec<String>,
) -> ClientServerReleaseValidation {
    let (tag_name, target_commitish) = metadata
        .map(|(tag, target)| (Some(tag), Some(target)))
        .unwrap_or((None, None));
    ClientServerReleaseValidation {
        release: release.to_string(),
        valid: status == ClientServerReleaseValidationStatus::Valid,
        status,
        message: message.into(),
        tag_name,
        target_commitish,
        missing_assets,
        checked_at: chrono::Utc::now().to_rfc3339(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(selector: &str, target: &str, assets: &[&str]) -> GithubRelease {
        GithubRelease {
            tag_name: selector.to_string(),
            target_commitish: target.to_string(),
            assets: assets
                .iter()
                .map(|name| GithubReleaseAsset {
                    name: (*name).to_string(),
                    state: "uploaded".to_string(),
                    size: 1,
                })
                .collect(),
        }
    }

    #[test]
    fn selector_accepts_latest_and_canonicalizes_seven_hex_characters() {
        assert_eq!(
            normalize_client_server_release(" latest ").unwrap(),
            "latest"
        );
        assert_eq!(
            normalize_client_server_release("AbC1234").unwrap(),
            "abc1234"
        );
        assert!(normalize_client_server_release("abc123").is_err());
        assert!(normalize_client_server_release("abc12345").is_err());
        assert!(normalize_client_server_release("abc12-z").is_err());
    }

    #[test]
    fn release_requires_both_architectures_and_checksums() {
        let result = validate_release_payload(
            "abc1234",
            release(
                "abc1234",
                "abc1234567890abcdef1234567890abcdef12345",
                &["cc-switch-server-linux-amd64"],
            ),
        );
        assert_eq!(
            result.status,
            ClientServerReleaseValidationStatus::IncompleteAssets
        );
        assert_eq!(result.missing_assets.len(), 3);
    }

    #[test]
    fn pinned_release_target_must_match_selector() {
        let result = validate_release_payload(
            "abc1234",
            release(
                "abc1234",
                "def5678901234567890123456789012345678901",
                &REQUIRED_RELEASE_ASSETS,
            ),
        );
        assert_eq!(
            result.status,
            ClientServerReleaseValidationStatus::CommitMismatch
        );
    }

    #[test]
    fn complete_release_is_valid() {
        let result = validate_release_payload(
            "abc1234",
            release(
                "abc1234",
                "abc1234567890abcdef1234567890abcdef12345",
                &REQUIRED_RELEASE_ASSETS,
            ),
        );
        assert_eq!(result.status, ClientServerReleaseValidationStatus::Valid);
        assert!(result.valid);
    }

    #[test]
    fn latest_release_requires_a_resolved_target_commit_for_rollout_safety() {
        let unresolved = validate_release_payload(
            "latest",
            release("latest", "main", &REQUIRED_RELEASE_ASSETS),
        );
        assert_eq!(
            unresolved.status,
            ClientServerReleaseValidationStatus::CommitMismatch
        );

        let resolved = validate_release_payload(
            "latest",
            release(
                "latest",
                "abc1234567890abcdef1234567890abcdef12345",
                &REQUIRED_RELEASE_ASSETS,
            ),
        );
        assert_eq!(resolved.status, ClientServerReleaseValidationStatus::Valid);
    }

    #[test]
    fn not_found_and_rate_limit_are_distinct_outcomes() {
        let not_found = validation_for_http_status("abc1234", StatusCode::NOT_FOUND).unwrap();
        let rate_limited =
            validation_for_http_status("abc1234", StatusCode::TOO_MANY_REQUESTS).unwrap();
        assert_eq!(
            not_found.status,
            ClientServerReleaseValidationStatus::NotFound
        );
        assert_eq!(
            rate_limited.status,
            ClientServerReleaseValidationStatus::Unavailable
        );
    }
}
