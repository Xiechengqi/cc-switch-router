use std::process::Command;
use std::time::Duration;

use serde::Serialize;
use std::time::Instant;

use crate::error::AppError;

pub const SERVICE_UNIT: &str = "cc-switch-router.service";
pub const BINARY_INSTALL_PATH: &str = "/usr/local/bin/cc-switch-router";
pub const SERVICE_LOG_PATH: &str = "/var/log/cc-switch-router.log";
pub const RELEASE_BINARY_URL: &str = "https://github.com/Xiechengqi/cc-switch-router/releases/download/latest/cc-switch-router-linux-amd64";

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceManager {
    Systemd,
    Nohup,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildInfo {
    pub version: &'static str,
    pub commit: &'static str,
    pub build_time: &'static str,
}

pub fn build_info() -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION"),
        commit: option_env!("GIT_COMMIT").unwrap_or("dev"),
        build_time: option_env!("BUILD_TIME").unwrap_or("unknown"),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    pub manager: ServiceManager,
    pub active: bool,
    pub unit_name: Option<&'static str>,
    pub active_state: Option<String>,
    pub unit_file_state: Option<String>,
}

/// Inspect systemd for the configured unit. Returns `ServiceManager::Systemd`
/// only when `systemctl show` reports the unit exists and is currently
/// active; everything else falls back to nohup mode.
pub fn detect_service_status() -> ServiceStatus {
    let output = Command::new("systemctl")
        .args([
            "--no-pager",
            "show",
            "--property=ActiveState",
            "--property=UnitFileState",
            "--property=LoadState",
            SERVICE_UNIT,
        ])
        .output();

    let Ok(output) = output else {
        return nohup_status();
    };
    if !output.status.success() {
        return nohup_status();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut active_state = None;
    let mut unit_file_state = None;
    let mut load_state = None;
    for line in stdout.lines() {
        if let Some(v) = line.strip_prefix("ActiveState=") {
            active_state = Some(v.to_string());
        } else if let Some(v) = line.strip_prefix("UnitFileState=") {
            unit_file_state = Some(v.to_string());
        } else if let Some(v) = line.strip_prefix("LoadState=") {
            load_state = Some(v.to_string());
        }
    }
    let loaded_known = load_state
        .as_deref()
        .map(|s| s != "not-found" && s != "masked")
        .unwrap_or(false);
    if !loaded_known {
        return nohup_status();
    }
    let active = active_state.as_deref() == Some("active");
    ServiceStatus {
        manager: ServiceManager::Systemd,
        active,
        unit_name: Some(SERVICE_UNIT),
        active_state,
        unit_file_state,
    }
}

fn nohup_status() -> ServiceStatus {
    ServiceStatus {
        manager: ServiceManager::Nohup,
        active: true,
        unit_name: None,
        active_state: Some("running".into()),
        unit_file_state: None,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestReleaseMeta {
    pub binary_url: String,
    pub available: bool,
    pub etag: Option<String>,
    pub content_length: Option<u64>,
    pub error: Option<String>,
}

pub async fn fetch_latest_release_meta(client: &reqwest::Client) -> LatestReleaseMeta {
    let mut meta = LatestReleaseMeta {
        binary_url: RELEASE_BINARY_URL.to_string(),
        available: false,
        etag: None,
        content_length: None,
        error: None,
    };
    match client
        .head(RELEASE_BINARY_URL)
        .timeout(Duration::from_secs(8))
        .send()
        .await
    {
        Ok(resp) => {
            if resp.status().is_success() || resp.status().is_redirection() {
                meta.available = true;
                if let Some(value) = resp.headers().get("etag") {
                    meta.etag = value.to_str().ok().map(str::to_string);
                }
                if let Some(value) = resp.headers().get("content-length") {
                    meta.content_length = value.to_str().ok().and_then(|v| v.trim().parse().ok());
                }
            } else {
                meta.error = Some(format!("HTTP {}", resp.status()));
            }
        }
        Err(err) => {
            meta.error = Some(err.to_string());
        }
    }
    meta
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionResponse {
    pub version: &'static str,
    pub commit: &'static str,
    pub build_time: &'static str,
    pub binary_path: &'static str,
    pub uptime_secs: u64,
    pub service: ServiceStatus,
    pub latest: LatestReleaseMeta,
}

pub fn uptime_secs_from(start: Instant) -> u64 {
    start.elapsed().as_secs()
}

pub fn ensure_binary_writable() -> Result<(), AppError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = match std::fs::metadata(BINARY_INSTALL_PATH) {
        Ok(m) => m,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(AppError::Internal(format!(
                "stat {BINARY_INSTALL_PATH} failed: {err}"
            )));
        }
    };
    let mode = metadata.permissions().mode();
    if mode & 0o200 == 0 {
        return Err(AppError::Forbidden(format!(
            "binary at {BINARY_INSTALL_PATH} is not writable by this process"
        )));
    }
    Ok(())
}
