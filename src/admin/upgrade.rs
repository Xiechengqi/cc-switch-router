use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::{Mutex, broadcast};
use tracing::warn;
use uuid::Uuid;

use crate::admin::restart::{RestartStrategy, schedule_restart};
use crate::admin::version::{BINARY_INSTALL_PATH, RELEASE_BINARY_URL, detect_service_status};
use crate::error::AppError;

const LOG_CHANNEL_CAPACITY: usize = 256;
const TOTAL_STEPS: usize = 7;
const DOWNLOAD_BUFFER_TICK_BYTES: u64 = 256 * 1024;
const SANITY_TIMEOUT: Duration = Duration::from_secs(5);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeLogLevel {
    Info,
    Progress,
    Success,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeLogEntry {
    pub task_id: String,
    pub step: usize,
    pub total_steps: usize,
    pub level: UpgradeLogLevel,
    pub message: String,
    pub progress: Option<u8>,
    pub at: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeStatus {
    Running,
    Success,
    Failed,
}

#[derive(Clone)]
pub struct UpgradeHandle {
    pub task_id: String,
    pub status: Arc<Mutex<UpgradeStatus>>,
    pub sender: broadcast::Sender<UpgradeLogEntry>,
    pub history: Arc<Mutex<Vec<UpgradeLogEntry>>>,
}

#[derive(Default)]
pub struct UpgradeRegistry {
    inner: Mutex<Option<UpgradeHandle>>,
}

impl UpgradeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn start(
        &self,
        client: reqwest::Client,
        actor: Option<String>,
    ) -> Result<UpgradeHandle, AppError> {
        let mut guard = self.inner.lock().await;
        if let Some(handle) = guard.as_ref() {
            let status = *handle.status.lock().await;
            if matches!(status, UpgradeStatus::Running) {
                return Err(AppError::Conflict("an upgrade is already in progress".into()));
            }
        }
        let task_id = Uuid::new_v4().to_string();
        let (tx, _rx) = broadcast::channel(LOG_CHANNEL_CAPACITY);
        let handle = UpgradeHandle {
            task_id: task_id.clone(),
            status: Arc::new(Mutex::new(UpgradeStatus::Running)),
            sender: tx,
            history: Arc::new(Mutex::new(Vec::new())),
        };
        *guard = Some(handle.clone());
        drop(guard);

        let handle_for_task = handle.clone();
        tokio::spawn(async move {
            let outcome = run_upgrade(client, &handle_for_task, actor).await;
            let mut status_guard = handle_for_task.status.lock().await;
            *status_guard = match outcome {
                Ok(()) => UpgradeStatus::Success,
                Err(_) => UpgradeStatus::Failed,
            };
        });
        Ok(handle)
    }

    pub async fn current(&self) -> Option<UpgradeHandle> {
        self.inner.lock().await.clone()
    }
}

async fn emit(
    handle: &UpgradeHandle,
    step: usize,
    level: UpgradeLogLevel,
    message: impl Into<String>,
    progress: Option<u8>,
) {
    let entry = UpgradeLogEntry {
        task_id: handle.task_id.clone(),
        step,
        total_steps: TOTAL_STEPS,
        level,
        message: message.into(),
        progress,
        at: Utc::now().to_rfc3339(),
    };
    handle.history.lock().await.push(entry.clone());
    // Ignore receiver-gone errors; history covers late subscribers.
    let _ = handle.sender.send(entry);
}

async fn run_upgrade(
    client: reqwest::Client,
    handle: &UpgradeHandle,
    actor: Option<String>,
) -> Result<(), AppError> {
    let actor = actor.unwrap_or_else(|| "unknown".to_string());
    emit(
        handle,
        1,
        UpgradeLogLevel::Info,
        format!("upgrade requested by {actor}"),
        Some(progress_pct(1, 0)),
    )
    .await;

    // 1) tmp file beside the install target. Staging in the same directory
    // as BINARY_INSTALL_PATH guarantees the final rename(2) is intra-fs and
    // cannot fail with EXDEV — common on hosts where /tmp is tmpfs while
    // /usr/local/bin lives on the root volume.
    let target = Path::new(BINARY_INSTALL_PATH);
    let target_parent = match target.parent() {
        Some(p) => p.to_path_buf(),
        None => {
            emit(
                handle,
                1,
                UpgradeLogLevel::Error,
                format!("install target has no parent: {}", target.display()),
                None,
            )
            .await;
            return Err(AppError::Internal(format!(
                "install target has no parent: {}",
                target.display()
            )));
        }
    };
    if let Err(err) = std::fs::create_dir_all(&target_parent) {
        emit(
            handle,
            1,
            UpgradeLogLevel::Error,
            format!("ensure install dir failed: {err}"),
            None,
        )
        .await;
        return Err(AppError::Internal(format!(
            "ensure install dir failed: {err}"
        )));
    }
    let tmp_path =
        target_parent.join(format!("cc-switch-router.upgrade-{}", handle.task_id));
    emit(
        handle,
        1,
        UpgradeLogLevel::Success,
        format!("staging in {}", target_parent.display()),
        Some(progress_pct(1, 100)),
    )
    .await;

    // 2) download
    emit(
        handle,
        2,
        UpgradeLogLevel::Info,
        format!("downloading {RELEASE_BINARY_URL}"),
        Some(progress_pct(2, 0)),
    )
    .await;
    let bytes_written = match download_with_progress(&client, RELEASE_BINARY_URL, &tmp_path, handle).await {
        Ok(n) => n,
        Err(err) => {
            cleanup_tmp(&tmp_path);
            emit(
                handle,
                2,
                UpgradeLogLevel::Error,
                format!("download failed: {err}"),
                None,
            )
            .await;
            return Err(err);
        }
    };
    emit(
        handle,
        2,
        UpgradeLogLevel::Success,
        format!("downloaded {bytes_written} bytes"),
        Some(progress_pct(2, 100)),
    )
    .await;

    // 3) chmod +x + sanity
    if let Err(err) = chmod_exec(&tmp_path) {
        cleanup_tmp(&tmp_path);
        emit(
            handle,
            3,
            UpgradeLogLevel::Error,
            format!("chmod failed: {err}"),
            None,
        )
        .await;
        return Err(err);
    }
    if let Err(err) = sanity_exec(&tmp_path).await {
        cleanup_tmp(&tmp_path);
        emit(
            handle,
            3,
            UpgradeLogLevel::Error,
            format!("sanity check failed: {err}"),
            None,
        )
        .await;
        return Err(err);
    }
    emit(
        handle,
        3,
        UpgradeLogLevel::Success,
        "binary is executable and responds to --help",
        Some(progress_pct(3, 100)),
    )
    .await;

    // 4) sha256
    let new_sha = match sha256_of_file(&tmp_path) {
        Ok(v) => v,
        Err(err) => {
            cleanup_tmp(&tmp_path);
            emit(
                handle,
                4,
                UpgradeLogLevel::Error,
                format!("sha256 failed: {err}"),
                None,
            )
            .await;
            return Err(err);
        }
    };
    let current_sha = sha256_of_file(Path::new(BINARY_INSTALL_PATH)).ok();
    emit(
        handle,
        4,
        UpgradeLogLevel::Info,
        format!(
            "new sha256: {new_sha}; current: {}",
            current_sha.as_deref().unwrap_or("(missing)")
        ),
        Some(progress_pct(4, 100)),
    )
    .await;
    if current_sha.as_deref() == Some(new_sha.as_str()) {
        emit(
            handle,
            4,
            UpgradeLogLevel::Warn,
            "downloaded binary matches the running one; restart will still pick up env changes",
            None,
        )
        .await;
    }

    // 5) backup + atomic swap
    let bak_path = format!("{BINARY_INSTALL_PATH}.bak");
    let swap_result = swap_binary(&tmp_path, target, Path::new(&bak_path));
    cleanup_tmp(&tmp_path);
    if let Err(err) = swap_result {
        emit(
            handle,
            5,
            UpgradeLogLevel::Error,
            format!("swap failed: {err}"),
            None,
        )
        .await;
        return Err(err);
    }
    emit(
        handle,
        5,
        UpgradeLogLevel::Success,
        format!(
            "installed new binary at {BINARY_INSTALL_PATH} (backup at {bak_path})"
        ),
        Some(progress_pct(5, 100)),
    )
    .await;

    // 6) restart
    let manager = detect_service_status().manager;
    let strategy = RestartStrategy::from_manager(manager);
    emit(
        handle,
        6,
        UpgradeLogLevel::Info,
        format!("triggering restart via {} mode", strategy.label()),
        Some(progress_pct(6, 30)),
    )
    .await;
    let restart_script = match schedule_restart(strategy) {
        Ok(script) => script,
        Err(err) => {
            emit(
                handle,
                6,
                UpgradeLogLevel::Error,
                format!("restart spawn failed: {err}"),
                None,
            )
            .await;
            return Err(err);
        }
    };
    emit(
        handle,
        6,
        UpgradeLogLevel::Success,
        format!("restart scheduled: {restart_script}"),
        Some(progress_pct(6, 100)),
    )
    .await;

    // 7) hand off
    emit(
        handle,
        7,
        UpgradeLogLevel::Success,
        "process will exit shortly; dashboard should reload once health probe succeeds",
        Some(progress_pct(7, 100)),
    )
    .await;
    Ok(())
}

async fn download_with_progress(
    client: &reqwest::Client,
    url: &str,
    target: &Path,
    handle: &UpgradeHandle,
) -> Result<u64, AppError> {
    let response = client
        .get(url)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .map_err(|err| AppError::Internal(format!("download request failed: {err}")))?;
    if !response.status().is_success() {
        return Err(AppError::Internal(format!(
            "download HTTP {}",
            response.status()
        )));
    }
    let total = response.content_length();
    let mut file = tokio::fs::File::create(target)
        .await
        .map_err(|err| AppError::Internal(format!("open tmp file failed: {err}")))?;
    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut next_tick: u64 = DOWNLOAD_BUFFER_TICK_BYTES;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| AppError::Internal(format!("download chunk failed: {err}")))?;
        file.write_all(&chunk)
            .await
            .map_err(|err| AppError::Internal(format!("write tmp failed: {err}")))?;
        downloaded += chunk.len() as u64;
        if downloaded >= next_tick {
            next_tick = downloaded + DOWNLOAD_BUFFER_TICK_BYTES;
            let pct = match total {
                Some(t) if t > 0 => Some(((downloaded as f64 / t as f64) * 100.0).clamp(0.0, 100.0) as u8),
                _ => None,
            };
            let msg = match total {
                Some(t) => format!(
                    "downloaded {:.1} MiB / {:.1} MiB",
                    downloaded as f64 / 1024.0 / 1024.0,
                    t as f64 / 1024.0 / 1024.0
                ),
                None => format!("downloaded {:.1} MiB", downloaded as f64 / 1024.0 / 1024.0),
            };
            emit(handle, 2, UpgradeLogLevel::Progress, msg, pct).await;
        }
    }
    file.flush()
        .await
        .map_err(|err| AppError::Internal(format!("flush tmp failed: {err}")))?;
    Ok(downloaded)
}

fn chmod_exec(path: &Path) -> Result<(), AppError> {
    let mut perms = std::fs::metadata(path)
        .map_err(|err| AppError::Internal(format!("stat tmp failed: {err}")))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)
        .map_err(|err| AppError::Internal(format!("chmod failed: {err}")))
}

async fn sanity_exec(path: &Path) -> Result<(), AppError> {
    let output = tokio::time::timeout(
        SANITY_TIMEOUT,
        Command::new(path).arg("--help").output(),
    )
    .await
    .map_err(|_| AppError::Internal("sanity --help timed out".into()))?
    .map_err(|err| AppError::Internal(format!("sanity exec failed: {err}")))?;
    if !output.status.success() {
        return Err(AppError::Internal(format!(
            "sanity --help exited with status {}",
            output.status
        )));
    }
    Ok(())
}

fn sha256_of_file(path: &Path) -> Result<String, AppError> {
    let mut hasher = Sha256::new();
    let mut file = std::fs::File::open(path)
        .map_err(|err| AppError::Internal(format!("open for sha256 failed: {err}")))?;
    std::io::copy(&mut file, &mut hasher)
        .map_err(|err| AppError::Internal(format!("read for sha256 failed: {err}")))?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn swap_binary(new_path: &Path, target: &Path, bak: &Path) -> Result<(), AppError> {
    if target.exists() {
        if bak.exists() {
            let _ = std::fs::remove_file(bak);
        }
        std::fs::rename(target, bak)
            .map_err(|err| AppError::Internal(format!("backup current binary failed: {err}")))?;
    }
    if let Err(err) = std::fs::rename(new_path, target) {
        // Try to restore the backup so the service stays alive.
        if bak.exists() {
            let _ = std::fs::rename(bak, target);
        }
        return Err(AppError::Internal(format!("install new binary failed: {err}")));
    }
    Ok(())
}

fn cleanup_tmp(file: &Path) {
    if let Err(err) = std::fs::remove_file(file) {
        if !matches!(err.kind(), std::io::ErrorKind::NotFound) {
            warn!(path = %file.display(), error = %err, "cleanup tmp upgrade file failed");
        }
    }
}

fn progress_pct(step: usize, within_step: u8) -> u8 {
    let base = (step.saturating_sub(1) * 100 / TOTAL_STEPS) as u32;
    let inc = (within_step as u32) * 100 / (TOTAL_STEPS as u32) / 100;
    base.saturating_add(inc).min(100) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_pct_monotonic() {
        let mut last = 0u8;
        for step in 1..=TOTAL_STEPS {
            for pct in [0u8, 50, 100] {
                let p = progress_pct(step, pct);
                assert!(p >= last, "step {step} pct {pct}: {p} < {last}");
                last = p;
            }
        }
    }
}

/// Backports of arc-by-ref helpers so the registry stays Send + Sync.
pub fn registry() -> Arc<UpgradeRegistry> {
    Arc::new(UpgradeRegistry::new())
}

pub type SharedUpgradeRegistry = Arc<UpgradeRegistry>;

/// Path to the latest backup, if any.
pub fn current_backup_path() -> PathBuf {
    PathBuf::from(format!("{BINARY_INSTALL_PATH}.bak"))
}
