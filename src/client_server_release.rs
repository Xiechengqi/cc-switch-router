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
pub(crate) struct InstallationArtifact {
    pub name: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub(crate) struct InstallationRelease {
    pub selector: String,
    pub commit: String,
    pub artifacts: Vec<InstallationArtifact>,
}

#[derive(Debug, Clone)]
struct CachedValidation {
    expires_at: Instant,
    value: ClientServerReleaseValidation,
}

#[derive(Clone)]
struct CachedInstallation {
    release: InstallationRelease,
    etag: Option<String>,
    checked_at: Instant,
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
    installations: Mutex<HashMap<String, CachedInstallation>>,
}

impl ClientServerReleaseValidator {
    /// Installation checks always consult the published Release. In particular,
    /// the Settings validator's five-minute cache is not a freshness guarantee.
    pub(crate) async fn resolve_installation(
        &self,
        selector: &str,
    ) -> Result<InstallationRelease, String> {
        let selector = normalize_client_server_release(selector)?;
        let requested_at = Instant::now();
        let lock = {
            let mut locks = self.selector_locks.lock().await;
            locks.retain(|_, lock| lock.strong_count() > 0);
            match locks.get(&selector).and_then(Weak::upgrade) {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(Mutex::new(()));
                    locks.insert(selector.clone(), Arc::downgrade(&lock));
                    lock
                }
            }
        };
        let _flight = lock.lock().await;
        if let Some(cached) = self
            .installations
            .lock()
            .await
            .get(&selector)
            .filter(|cached| cached.checked_at >= requested_at)
        {
            return Ok(cached.release.clone());
        }
        let mut last_error = String::new();
        for attempt in 0..3 {
            match self.fetch_installation(&selector).await {
                Ok(release) => return Ok(release),
                Err(error) => last_error = error,
            }
            if attempt < 2 {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        Err(last_error)
    }

    async fn fetch_installation(&self, selector: &str) -> Result<InstallationRelease, String> {
        let cached = self.installations.lock().await.get(selector).cloned();
        let mut request = self
            .client
            .get(format!("{}/{selector}", self.api_base))
            .header("Accept", "application/vnd.github+json")
            .header("Cache-Control", "no-cache");
        if let Some(etag) = cached.as_ref().and_then(|cached| cached.etag.as_deref()) {
            request = request.header("If-None-Match", etag);
        }
        let response = request
            .send()
            .await
            .map_err(|_| "Release lookup unavailable".to_string())?
            .error_for_status()
            .map_err(|_| "Published Release unavailable".to_string())?;
        if response.status() == StatusCode::NOT_MODIFIED {
            let mut cached = cached.ok_or("Release returned 304 without a cached identity")?;
            cached.checked_at = Instant::now();
            let release = cached.release.clone();
            self.installations
                .lock()
                .await
                .insert(selector.into(), cached);
            return Ok(release);
        }
        let etag = response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = read_installation_response(response, 1024 * 1024).await?;
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| "Invalid Release metadata".to_string())?;
        let payload: GithubRelease = serde_json::from_value(value.clone())
            .map_err(|_| "Invalid Release metadata".to_string())?;
        let validation = validate_release_payload(selector, payload);
        if !validation.valid {
            return Err(validation.message);
        }
        let commit = validation
            .target_commitish
            .ok_or("Release commit is missing")?
            .to_ascii_lowercase();
        if commit.len() != 40 {
            return Err("Release requires a full commit ID".into());
        }
        let assets = value["assets"].as_array().ok_or("Release assets missing")?;
        let mut artifacts = Vec::new();
        for name in [
            "cc-switch-server-linux-amd64",
            "cc-switch-server-linux-arm64",
        ] {
            let asset = assets
                .iter()
                .find(|asset| asset["name"].as_str() == Some(name))
                .ok_or("Binary asset missing")?;
            let checksum_name = format!("{name}.sha256");
            let checksum = assets
                .iter()
                .find(|asset| asset["name"].as_str() == Some(checksum_name.as_str()))
                .ok_or("Checksum asset missing")?;
            let url = installation_asset_url(asset, selector, name)?;
            let checksum_url = installation_asset_url(checksum, selector, &checksum_name)?;
            let checksum_response = self
                .client
                .get(checksum_url)
                .header("Cache-Control", "no-cache")
                .send()
                .await
                .map_err(|_| "Checksum download unavailable".to_string())?
                .error_for_status()
                .map_err(|_| "Checksum download unavailable".to_string())?;
            let bytes = read_installation_response(checksum_response, 512).await?;
            let text =
                std::str::from_utf8(&bytes).map_err(|_| "Invalid checksum file".to_string())?;
            let sha256 = parse_installation_checksum(text)?;
            if let Some(digest) = asset["digest"].as_str()
                && digest != format!("sha256:{sha256}")
            {
                return Err("Release asset digest changed".into());
            }
            artifacts.push(InstallationArtifact {
                name: name.into(),
                url,
                sha256,
            });
        }
        let release = InstallationRelease {
            selector: selector.into(),
            commit,
            artifacts,
        };
        let mut installations = self.installations.lock().await;
        if installations.len() >= 32 {
            installations.clear();
        }
        installations.insert(
            selector.into(),
            CachedInstallation {
                release: release.clone(),
                etag,
                checked_at: Instant::now(),
            },
        );
        Ok(release)
    }

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
            installations: Mutex::new(HashMap::new()),
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

async fn read_installation_response(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Release download interrupted".to_string())?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err("Release response exceeds size limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn installation_asset_url(
    asset: &serde_json::Value,
    selector: &str,
    name: &str,
) -> Result<String, String> {
    let expected = format!(
        "https://github.com/Xiechengqi/cc-switch-server/releases/download/{selector}/{name}"
    );
    if asset["browser_download_url"].as_str() != Some(expected.as_str()) {
        return Err("Unexpected Release asset URL".into());
    }
    Ok(expected)
}

fn parse_installation_checksum(text: &str) -> Result<String, String> {
    let lines = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if lines.len() != 1 {
        return Err("Checksum must contain one digest".into());
    }
    let digest = lines[0]
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid checksum digest".into());
    }
    Ok(digest)
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

    #[tokio::test]
    async fn installation_checks_revalidate_etags_coalesce_and_reject_unavailable_release() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let requests = Arc::new(AtomicUsize::new(0));
        let available = Arc::new(AtomicBool::new(true));
        let hits = requests.clone();
        let live = available.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                axum::Router::new().route(
                    "/latest",
                    axum::routing::get(move |headers: axum::http::HeaderMap| {
                        let hits = hits.clone();
                        let live = live.clone();
                        async move {
                            hits.fetch_add(1, Ordering::SeqCst);
                            assert_eq!(headers["if-none-match"], "\"published\"");
                            tokio::time::sleep(Duration::from_millis(25)).await;
                            if live.load(Ordering::SeqCst) {
                                StatusCode::NOT_MODIFIED
                            } else {
                                StatusCode::NOT_FOUND
                            }
                        }
                    }),
                ),
            )
            .await
            .unwrap();
        });
        let mut validator = ClientServerReleaseValidator::new().unwrap();
        validator.api_base = format!("http://{addr}");
        validator.installations.lock().await.insert(
            "latest".into(),
            CachedInstallation {
                release: InstallationRelease {
                    selector: "latest".into(),
                    commit: "a".repeat(40),
                    artifacts: Vec::new(),
                },
                etag: Some("\"published\"".into()),
                checked_at: Instant::now() - Duration::from_secs(1),
            },
        );
        let (a, b) = tokio::join!(
            validator.resolve_installation("latest"),
            validator.resolve_installation("latest")
        );
        assert_eq!(a.unwrap().commit, b.unwrap().commit);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        validator.resolve_installation("latest").await.unwrap();
        assert_eq!(
            requests.load(Ordering::SeqCst),
            2,
            "sequential launch skipped its freshness check"
        );
        available.store(false, Ordering::SeqCst);
        assert!(
            validator.resolve_installation("latest").await.is_err(),
            "unavailable release reused a stale identity"
        );
        server.abort();
    }
    #[test]
    fn installation_checksum_rejects_ambiguous_and_invalid_files() {
        let digest = "a".repeat(64);
        assert_eq!(
            parse_installation_checksum(&format!("{digest}  binary\n")).unwrap(),
            digest
        );
        assert!(parse_installation_checksum(&format!("{digest}\n{digest}\n")).is_err());
        assert!(parse_installation_checksum("unknown binary").is_err());
        assert!(parse_installation_checksum("").is_err());
        assert!(
            installation_asset_url(
                &serde_json::json!({"browser_download_url":"https://attacker.test/binary"}),
                "latest",
                "cc-switch-server-linux-amd64"
            )
            .is_err()
        );
    }

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
