use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{Mutex, OwnedMutexGuard};
use tokio_util::sync::CancellationToken;
use tracing::warn;
use uuid::Uuid;

use crate::admin::restart::{RestartStrategy, schedule_restart};
use crate::admin::upgrade_task::{
    UPGRADE_PROTOCOL_VERSION, UpgradeBuild, UpgradePhase, cancelled_error, load_snapshot,
};
pub use crate::admin::upgrade_task::{
    UpgradeHandle, UpgradeLogEntry, UpgradeLogLevel, UpgradeStatus, UpgradeTaskSnapshot,
};
use crate::admin::version::{
    BINARY_INSTALL_PATH, BINARY_ROLLBACK_PATH, RELEASE_BINARY_URL, detect_service_status,
};
use crate::error::AppError;

const SANITY_TIMEOUT: Duration = Duration::from_secs(5);
const DATABASE_SANITY_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);
const RESTART_TIMEOUT: Duration = Duration::from_secs(300);

/// Probe a wildcard listen address through loopback.
fn loopback(mut address: std::net::SocketAddr) -> std::net::SocketAddr {
    if address.ip().is_unspecified() {
        address.set_ip(if address.is_ipv4() {
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        } else {
            std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        });
    }
    address
}

/// A bound listener accepts TCP connections into its backlog before the
/// server runs; only the server's `SSH-` identification proves it serves.
async fn ssh_banner_ready(address: std::net::SocketAddr) -> bool {
    use tokio::io::AsyncReadExt;
    let probe = async {
        let mut stream = tokio::net::TcpStream::connect(address).await.ok()?;
        let mut banner = [0u8; 4];
        stream.read_exact(&mut banner).await.ok()?;
        Some(&banner == b"SSH-")
    };
    matches!(
        tokio::time::timeout(Duration::from_secs(2), probe).await,
        Ok(Some(true))
    )
}

/// A restarted process starts its revisions above any the previous process
/// served, persisted or not (see `UpgradeHandle::update`).
fn restarted_revision(saved: u64) -> u64 {
    let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0);
    saved.saturating_add(1).max(now)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackResponse {
    pub ok: bool,
    pub strategy: String,
    pub backup_path: String,
}

pub struct UpgradeRegistry {
    inner: Mutex<Option<UpgradeHandle>>,
    journal: Option<PathBuf>,
    instance_id: String,
    operation: Arc<Mutex<()>>,
    maintenance_pending: Arc<AtomicBool>,
    storage_fault: Arc<AtomicBool>,
}

pub struct ServiceActionGuard {
    _guard: OwnedMutexGuard<()>,
    maintenance_pending: Arc<AtomicBool>,
}

impl ServiceActionGuard {
    pub fn restart_scheduled(&self) {
        self.maintenance_pending.store(true, Ordering::SeqCst);
        let pending = self.maintenance_pending.clone();
        tokio::spawn(async move {
            tokio::time::sleep(RESTART_TIMEOUT).await;
            if pending.swap(false, Ordering::SeqCst) {
                tracing::error!(
                    "service restart deadline passed while the original process is still running"
                );
            }
        });
    }
}

impl UpgradeRegistry {
    fn empty(journal: Option<PathBuf>) -> Self {
        Self {
            inner: Mutex::new(None),
            journal,
            instance_id: Uuid::new_v4().to_string(),
            operation: Arc::new(Mutex::new(())),
            maintenance_pending: Arc::default(),
            storage_fault: Arc::default(),
        }
    }

    #[cfg(test)]
    pub fn new() -> Self {
        Self::empty(None)
    }

    pub async fn open(data_dir: &Path) -> Self {
        let registry = Self::empty(Some(data_dir.join("upgrade/current.json")));
        match load_snapshot(registry.journal.clone().expect("journal configured")).await {
            Ok(Some(mut snapshot)) => {
                // Writes that failed to persist still raised the revision a
                // browser saw. Restart above any of them, so the states this
                // process publishes are never discarded as stale.
                snapshot.revision = restarted_revision(snapshot.revision);
                let handle = UpgradeHandle::new(
                    snapshot,
                    registry.journal.clone(),
                    registry.storage_fault.clone(),
                );
                *registry.inner.lock().await = Some(handle.clone());
                if handle.snapshot().await.persistence_error {
                    let _ = handle
                        .update(|next| {
                            next.persistence_error = false;
                            next.append_log(
                                UpgradeLogLevel::Warn,
                                "upgrade journal is writable again after startup",
                            );
                            Ok(())
                        })
                        .await;
                }
                if !registry.storage_fault.load(Ordering::SeqCst) {
                    registry.recover(&handle).await;
                }
            }
            Ok(None) => {}
            Err(error) => {
                // A corrupt journal must fence maintenance, not prevent the
                // Router from serving existing users and diagnostics.
                tracing::error!(%error, "upgrade journal recovery failed; service operations are fenced");
                registry.storage_fault.store(true, Ordering::SeqCst);
            }
        }
        registry
    }

    async fn recover(&self, handle: &UpgradeHandle) {
        let snapshot = handle.snapshot().await;
        if snapshot.status.is_terminal() {
            return;
        }
        let cleanup = remove_task_staging(&handle.task_id).await;
        if snapshot.phase.cancellable() {
            if let Err(error) = cleanup {
                handle
                    .finish(
                        UpgradeStatus::Failed,
                        Some(error.to_string()),
                        &error.to_string(),
                    )
                    .await;
                return;
            }
            let status = if snapshot.status == UpgradeStatus::Cancelling {
                UpgradeStatus::Cancelled
            } else {
                UpgradeStatus::Interrupted
            };
            handle
                .finish(
                    status,
                    None,
                    "previous process stopped before installation; staged download removed",
                )
                .await;
            return;
        }
        let running_sha = sha256_of_file(&running_executable(), &CancellationToken::new()).await;
        let target_running = snapshot.target_sha256.is_some()
            && running_sha.as_ref().ok() == snapshot.target_sha256.as_ref();
        let replaced = if snapshot.binary_replaced || target_running {
            true
        } else {
            let installed_sha =
                sha256_of_file(Path::new(BINARY_INSTALL_PATH), &CancellationToken::new()).await;
            snapshot.target_sha256.is_some()
                && installed_sha.as_ref().ok() == snapshot.target_sha256.as_ref()
        };
        // Recover the irreversible rename fact even if a leftover file cannot
        // be removed. Cleanup failures must not claim the old binary was kept.
        if replaced && !snapshot.binary_replaced {
            if let Err(error) = handle
                .update(|next| {
                    next.binary_replaced = true;
                    Ok(())
                })
                .await
            {
                tracing::error!(%error, "persist recovered installation fact failed");
                return;
            }
        }
        if let Err(error) = cleanup {
            handle
                .finish(
                    UpgradeStatus::Failed,
                    Some(error.to_string()),
                    &error.to_string(),
                )
                .await;
            return;
        }
        if target_running && snapshot.source_instance_id != self.instance_id {
            if let Err(error) =
                handle
                    .update(|next| {
                        next.status = UpgradeStatus::Restarting;
                        next.phase = UpgradePhase::Verifying;
                        next.progress = None;
                        next.append_log(UpgradeLogLevel::Info,
                    "target binary is running; waiting for HTTP, SSH and database readiness");
                        Ok(())
                    })
                    .await
            {
                tracing::error!(%error, "persist recovered upgrade failed");
            }
        } else {
            handle
                .finish(
                    UpgradeStatus::Interrupted,
                    Some("the process restarted without the expected target binary".into()),
                    "upgrade recovery could not confirm the expected target binary",
                )
                .await;
        }
    }

    /// Called only after both listeners have been bound and the HTTP/SSH tasks
    /// started. The loopback probes must reach this new process: HTTP with a
    /// healthy DB, and SSH answering with its identification banner.
    ///
    /// Gives up after `RESTART_TIMEOUT`: a task left verifying would block
    /// restart and rollback, the very actions that repair a bad binary.
    pub async fn confirm_after_startup(
        &self,
        address: std::net::SocketAddr,
        ssh_address: std::net::SocketAddr,
    ) -> Result<(), AppError> {
        self.confirm_after_startup_within(address, ssh_address, RESTART_TIMEOUT)
            .await
    }

    async fn confirm_after_startup_within(
        &self,
        address: std::net::SocketAddr,
        ssh_address: std::net::SocketAddr,
        deadline: Duration,
    ) -> Result<(), AppError> {
        let Some(handle) = self.current().await else {
            return Ok(());
        };
        let snapshot = handle.snapshot().await;
        if snapshot.phase != UpgradePhase::Verifying
            || snapshot.status != UpgradeStatus::Restarting
            || snapshot.source_instance_id == self.instance_id
            || self.storage_fault.load(Ordering::SeqCst)
        {
            return Ok(());
        }
        let address = loopback(address);
        let ssh_address = loopback(ssh_address);
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .map_err(|error| {
                AppError::Internal(format!("upgrade readiness client failed: {error}"))
            })?;
        let url = format!("http://{address}/v1/healthz");
        let give_up = tokio::time::Instant::now() + deadline;
        let mut http_ready = false;
        let mut ssh_ready = false;
        loop {
            if !http_ready {
                http_ready = client
                    .get(&url)
                    .send()
                    .await
                    .is_ok_and(|response| response.status().is_success());
            }
            if !ssh_ready {
                ssh_ready = ssh_banner_ready(ssh_address).await;
            }
            if http_ready && ssh_ready {
                handle
                    .finish(
                        UpgradeStatus::Success,
                        None,
                        "target process restarted successfully; HTTP, SSH and database are ready",
                    )
                    .await;
                return Ok(());
            }
            if tokio::time::Instant::now() >= give_up {
                let pending = match (http_ready, ssh_ready) {
                    (false, false) => "HTTP/database and SSH",
                    (false, true) => "HTTP/database",
                    _ => "SSH",
                };
                let reason = format!(
                    "the new process did not report {pending} readiness within {}s; the new binary is installed, restart or roll back after checking the service log",
                    deadline.as_secs()
                );
                handle
                    .finish(UpgradeStatus::Failed, Some(reason.clone()), &reason)
                    .await;
                return Ok(());
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    pub async fn reserve_service_action(&self) -> Result<ServiceActionGuard, AppError> {
        let guard = self
            .operation
            .clone()
            .try_lock_owned()
            .map_err(|_| busy_error())?;
        if self.storage_fault.load(Ordering::SeqCst) {
            return Err(journal_unavailable_error());
        }
        if self.maintenance_pending.load(Ordering::SeqCst) {
            return Err(busy_error());
        }
        if let Some(handle) = self.current().await {
            if !handle.snapshot().await.status.is_terminal() {
                return Err(busy_error());
            }
        }
        Ok(ServiceActionGuard {
            _guard: guard,
            maintenance_pending: self.maintenance_pending.clone(),
        })
    }

    pub async fn start(
        &self,
        client: reqwest::Client,
        actor: Option<String>,
    ) -> Result<UpgradeHandle, AppError> {
        let operation = self.reserve_service_action().await?;
        let mut current = self.inner.lock().await;
        let handle = UpgradeHandle::new(
            UpgradeTaskSnapshot::new(&self.instance_id, actor),
            self.journal.clone(),
            self.storage_fault.clone(),
        );
        handle
            .update(|next| {
                next.append_log(
                    UpgradeLogLevel::Info,
                    format!("upgrade requested by {}", next.actor),
                );
                Ok(())
            })
            .await?;
        *current = Some(handle.clone());
        drop(current);
        let worker = handle.clone();
        tokio::spawn(async move {
            let _operation = operation;
            let outcome = run_upgrade(client, &worker).await;
            let cleanup = remove_task_staging(&worker.task_id).await;
            let outcome = cleanup.and(outcome);
            if let Err(error) = outcome {
                let cancelled = worker.cancel.is_cancelled()
                    && !worker.storage_fault.load(Ordering::SeqCst)
                    && error.code() != Some("UPGRADE_CLEANUP_FAILED");
                if cancelled {
                    worker.finish(UpgradeStatus::Cancelled, None, "upgrade interrupted; temporary files removed; current binary was not replaced").await;
                } else {
                    worker
                        .finish(
                            UpgradeStatus::Failed,
                            Some(error.to_string()),
                            &format!("upgrade failed: {error}"),
                        )
                        .await;
                }
            } else {
                // If this task is still alive after the restart deadline, the
                // original process demonstrably never handed over to a new one.
                tokio::spawn(async move {
                    tokio::time::sleep(RESTART_TIMEOUT).await;
                    if worker.snapshot().await.status == UpgradeStatus::Restarting {
                        worker.finish(UpgradeStatus::Failed,
                            Some("the original process is still running after the restart deadline".into()),
                            "new binary was installed but restart was not completed").await;
                    }
                });
            }
        });
        Ok(handle)
    }

    pub async fn current(&self) -> Option<UpgradeHandle> {
        self.inner.lock().await.clone()
    }

    pub async fn cancel(&self, task_id: &str) -> Result<UpgradeTaskSnapshot, AppError> {
        // Keep cancellation atomic with publication of a new task; an older
        // cancellation must never overwrite the new task's journal.
        let current = self.inner.lock().await;
        let handle = current
            .as_ref()
            .ok_or_else(|| AppError::NotFound("no upgrade task exists".into()))?;
        if handle.task_id != task_id {
            return Err(AppError::coded_conflict(
                "UPGRADE_TASK_CHANGED",
                "the upgrade task has changed",
                serde_json::json!({"taskId": handle.task_id}),
            ));
        }
        handle.cancel().await
    }

    pub async fn snapshot(
        &self,
        task_id: Option<&str>,
    ) -> Result<Option<UpgradeTaskSnapshot>, AppError> {
        if self.storage_fault.load(Ordering::SeqCst) && self.current().await.is_none() {
            return Err(journal_unavailable_error());
        }
        let Some(handle) = self.current().await else {
            return Ok(None);
        };
        if task_id.is_some_and(|expected| expected != handle.task_id) {
            return Err(AppError::NotFound(
                "upgrade task id does not match the current task".into(),
            ));
        }
        let mut snapshot = handle.snapshot().await;
        if self.storage_fault.load(Ordering::SeqCst) {
            // An initial write for a new task may fail before publication,
            // leaving an older result as current. Still expose the global fence.
            snapshot.persistence_error = true;
            snapshot.can_cancel = false;
        }
        Ok(Some(snapshot))
    }
}

fn busy_error() -> AppError {
    AppError::coded_conflict(
        "SERVICE_OPERATION_IN_PROGRESS",
        "a service operation is already in progress",
        serde_json::json!({}),
    )
}

fn journal_unavailable_error() -> AppError {
    AppError::Coded { status: axum::http::StatusCode::SERVICE_UNAVAILABLE, code: "UPGRADE_JOURNAL_UNAVAILABLE",
        message: "upgrade state could not be recovered; inspect the Router service log before another operation".into(), details: serde_json::json!({}) }
}

fn staging_path(task_id: &str) -> PathBuf {
    Path::new(BINARY_INSTALL_PATH).with_file_name(format!("cc-switch-router.upgrade-{task_id}"))
}

fn backup_staging_path(task_id: &str) -> PathBuf {
    Path::new(BINARY_INSTALL_PATH).with_file_name(format!("cc-switch-router.backup-{task_id}"))
}

struct StagingFile(PathBuf);
impl Drop for StagingFile {
    fn drop(&mut self) {
        cleanup_tmp(&self.0);
    }
}

async fn run_upgrade(client: reqwest::Client, handle: &UpgradeHandle) -> Result<(), AppError> {
    let target = Path::new(BINARY_INSTALL_PATH);
    let parent = target
        .parent()
        .ok_or_else(|| AppError::Internal("binary install path has no parent".into()))?;
    tokio::fs::create_dir_all(parent).await.map_err(io_error)?;
    let staging = StagingFile(staging_path(&handle.task_id));
    handle.check_cancelled()?;
    handle
        .phase(
            UpgradePhase::Downloading,
            "downloading the latest Router binary",
        )
        .await?;
    download_with_progress(&client, &staging.0, handle).await?;

    handle
        .phase(
            UpgradePhase::Validating,
            "checking executable, upgrade protocol and database compatibility",
        )
        .await?;
    chmod_exec(&staging.0)?;
    run_check(&staging.0, "--help", SANITY_TIMEOUT, &handle.cancel).await?;
    let build_output = run_check(&staging.0, "build-info", SANITY_TIMEOUT, &handle.cancel).await?;
    let build: UpgradeBuild = serde_json::from_slice(&build_output)
        .map_err(|error| AppError::Internal(format!("invalid target build info: {error}")))?;
    if build.upgrade_protocol_version != UPGRADE_PROTOCOL_VERSION {
        return Err(AppError::Conflict("the release binary does not support resumable Router upgrades; the current binary was not replaced".into()));
    }
    run_check(
        &staging.0,
        "check-db",
        DATABASE_SANITY_TIMEOUT,
        &handle.cancel,
    )
    .await?;
    handle
        .update(|next| {
            next.target = Some(build);
            next.append_log(
                UpgradeLogLevel::Success,
                "binary and database compatibility checks passed",
            );
            Ok(())
        })
        .await?;

    handle
        .phase(UpgradePhase::Hashing, "computing target binary SHA-256")
        .await?;
    let sha = sha256_of_file(&staging.0, &handle.cancel).await?;
    handle
        .update(|next| {
            next.target_sha256 = Some(sha);
            Ok(())
        })
        .await?;

    // This durable transition and cancellation acceptance share one mutex.
    // Once it succeeds, no later cancel request can stop installation/restart.
    handle
        .phase(
            UpgradePhase::Installing,
            "installing the new binary and preserving the rollback backup",
        )
        .await?;
    let new_path = staging.0.clone();
    let install_path = target.to_path_buf();
    let task_id = handle.task_id.clone();
    tokio::task::spawn_blocking(move || {
        swap_binary(
            &new_path,
            &install_path,
            Path::new(BINARY_ROLLBACK_PATH),
            &task_id,
        )
    })
    .await
    .map_err(io_error)??;
    handle
        .update(|next| {
            next.binary_replaced = true;
            next.append_log(
                UpgradeLogLevel::Success,
                "new binary installed; rollback backup preserved",
            );
            Ok(())
        })
        .await?;
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(io_error)?;

    let strategy = RestartStrategy::from_manager(detect_service_status().manager);
    // Persist handoff *before* spawning a detached restart child. Never label
    // a successful spawn as a successful upgrade.
    handle
        .update(|next| {
            next.status = UpgradeStatus::Restarting;
            next.phase = UpgradePhase::Restarting;
            next.progress = None;
            next.append_log(
                UpgradeLogLevel::Info,
                format!(
                    "scheduling restart via {}; waiting for the new process",
                    strategy.label()
                ),
            );
            Ok(())
        })
        .await?;
    schedule_restart(strategy)?;
    Ok(())
}

async fn download_with_progress(
    client: &reqwest::Client,
    target: &Path,
    handle: &UpgradeHandle,
) -> Result<(), AppError> {
    let request = client
        .get(RELEASE_BINARY_URL)
        .timeout(DOWNLOAD_TIMEOUT)
        .send();
    let response = tokio::select! {
        biased;
        _ = handle.cancel.cancelled() => return Err(cancelled_error()),
        response = request => response.map_err(|error| AppError::Internal(format!("download request failed: {error}")))?,
    };
    if !response.status().is_success() {
        return Err(AppError::Internal(format!(
            "download HTTP {}",
            response.status()
        )));
    }
    let total = response.content_length().filter(|total| *total > 0);
    handle
        .update(|next| {
            next.total_bytes = total;
            Ok(())
        })
        .await?;
    let mut file = tokio::fs::File::create(target).await.map_err(io_error)?;
    let mut stream = response.bytes_stream();
    let mut downloaded = 0;
    let mut last_tick = Instant::now();
    loop {
        let chunk = tokio::select! {
            biased;
            _ = handle.cancel.cancelled() => return Err(cancelled_error()),
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = chunk else {
            break;
        };
        let chunk =
            chunk.map_err(|error| AppError::Internal(format!("download chunk failed: {error}")))?;
        tokio::select! {
            biased;
            _ = handle.cancel.cancelled() => return Err(cancelled_error()),
            result = file.write_all(&chunk) => result.map_err(io_error)?,
        }
        downloaded += chunk.len() as u64;
        if last_tick.elapsed() >= Duration::from_secs(1) {
            record_download(handle, downloaded, total).await?;
            last_tick = Instant::now();
        }
    }
    handle.check_cancelled()?;
    file.flush().await.map_err(io_error)?;
    file.sync_all().await.map_err(io_error)?;
    if downloaded == 0 {
        return Err(AppError::Internal("downloaded binary is empty".into()));
    }
    record_download(handle, downloaded, total).await
}

async fn record_download(
    handle: &UpgradeHandle,
    downloaded: u64,
    total: Option<u64>,
) -> Result<(), AppError> {
    handle
        .update(|next| {
            next.downloaded_bytes = downloaded;
            next.total_bytes = total;
            next.progress = total
                .map(|total| ((downloaded as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as u8);
            next.append_log(
                UpgradeLogLevel::Progress,
                match total {
                    Some(total) => format!(
                        "downloaded {:.1} MiB / {:.1} MiB",
                        downloaded as f64 / 1048576.0,
                        total as f64 / 1048576.0
                    ),
                    None => format!("downloaded {:.1} MiB", downloaded as f64 / 1048576.0),
                },
            );
            Ok(())
        })
        .await?;
    Ok(())
}

fn chmod_exec(path: &Path) -> Result<(), AppError> {
    let mut permissions = std::fs::metadata(path).map_err(io_error)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).map_err(io_error)
}

async fn read_output(pipe: Option<impl AsyncRead + Unpin>) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if let Some(pipe) = pipe {
        pipe.take(64 * 1024).read_to_end(&mut bytes).await?;
    }
    Ok(bytes)
}

async fn run_check(
    path: &Path,
    argument: &str,
    deadline: Duration,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, AppError> {
    if cancel.is_cancelled() {
        return Err(cancelled_error());
    }
    let mut child = Command::new(path)
        .arg(argument)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(io_error)?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => None,
        result = tokio::time::timeout(deadline, async {
            tokio::try_join!(child.wait(), read_output(stdout), read_output(stderr))
        }) => Some(result),
    };
    match result {
        Some(Ok(Ok((status, output, _)))) if status.success() => Ok(output),
        Some(Ok(Ok((status, _, error)))) => Err(AppError::Internal(format!(
            "{argument} exited with {status}: {}",
            String::from_utf8_lossy(&error).trim()
        ))),
        other => {
            // timeout/drop alone does not reliably reap a Tokio child. Both
            // cancellation and deadline paths explicitly kill and wait.
            let _ = child.start_kill();
            let _ = child.wait().await;
            match other {
                None => Err(cancelled_error()),
                Some(Err(_)) => Err(AppError::Internal(format!("{argument} timed out"))),
                Some(Ok(Err(error))) => Err(io_error(error)),
                _ => unreachable!(),
            }
        }
    }
}

async fn sanity_exec(path: &Path, check_database: bool) -> Result<(), AppError> {
    let token = CancellationToken::new();
    run_check(path, "--help", SANITY_TIMEOUT, &token).await?;
    if check_database {
        run_check(path, "check-db", DATABASE_SANITY_TIMEOUT, &token).await?;
    }
    Ok(())
}

async fn sha256_of_file(path: &Path, cancel: &CancellationToken) -> Result<String, AppError> {
    let mut file = tokio::fs::File::open(path).await.map_err(io_error)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let count = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(cancelled_error()),
            result = file.read(&mut buffer) => result.map_err(io_error)?,
        };
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn running_executable() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        PathBuf::from("/proc/self/exe")
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::env::current_exe().unwrap_or_else(|_| PathBuf::from(BINARY_INSTALL_PATH))
    }
}

fn swap_binary(
    new_path: &Path,
    target: &Path,
    backup: &Path,
    task_id: &str,
) -> Result<(), AppError> {
    // Copy the old executable to a staged backup, then atomically replace each
    // path. There is never a gap in which the install target is missing.
    let staged_backup = StagingFile(backup_staging_path(task_id));
    if target.exists() {
        std::fs::copy(target, &staged_backup.0).map_err(io_error)?;
        std::fs::File::open(&staged_backup.0)
            .and_then(|file| file.sync_all())
            .map_err(io_error)?;
        std::fs::rename(&staged_backup.0, backup).map_err(io_error)?;
        if let Some(parent) = backup.parent() {
            std::fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(io_error)?;
        }
    }
    std::fs::rename(new_path, target).map_err(io_error)
}

fn cleanup_tmp(file: &Path) {
    if let Err(error) = std::fs::remove_file(file) {
        if error.kind() != std::io::ErrorKind::NotFound {
            warn!(path = %file.display(), %error, "cleanup upgrade file failed");
        }
    }
}

async fn remove_staging(path: &Path) -> Result<(), AppError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppError::coded_conflict(
            "UPGRADE_CLEANUP_FAILED",
            format!("upgrade stopped but the staged file could not be removed: {error}"),
            serde_json::json!({}),
        )),
    }
}

async fn remove_task_staging(task_id: &str) -> Result<(), AppError> {
    let download = remove_staging(&staging_path(task_id)).await;
    let backup = remove_staging(&backup_staging_path(task_id)).await;
    download.and(backup)
}

fn io_error(error: impl std::fmt::Display) -> AppError {
    AppError::Internal(format!("upgrade I/O: {error}"))
}

pub type SharedUpgradeRegistry = Arc<UpgradeRegistry>;

#[cfg(test)]
mod tests {
    use super::*;

    /// A loopback port nothing listens on: probes are refused at once.
    async fn closed_port() -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        address
    }

    #[tokio::test]
    async fn a_process_that_never_becomes_ready_fails_and_unblocks_rollback() {
        let registry = UpgradeRegistry::new();
        let mut snapshot = UpgradeTaskSnapshot::new("old-boot", None);
        snapshot.status = UpgradeStatus::Restarting;
        snapshot.phase = UpgradePhase::Verifying;
        snapshot.binary_replaced = true;
        *registry.inner.lock().await = Some(UpgradeHandle::new(snapshot, None, Arc::default()));
        assert!(registry.reserve_service_action().await.is_err());

        let (http, ssh) = (closed_port().await, closed_port().await);
        registry
            .confirm_after_startup_within(http, ssh, Duration::ZERO)
            .await
            .unwrap();
        let task = registry.current().await.unwrap().snapshot().await;
        assert_eq!(task.status, UpgradeStatus::Failed);
        assert!(task.binary_replaced, "rollback needs the installed fact");
        assert!(task.error.unwrap().contains("HTTP/database and SSH"));
        assert!(registry.reserve_service_action().await.is_ok());
    }

    #[tokio::test]
    async fn ssh_readiness_needs_the_server_banner_not_just_a_bound_port() {
        // Bound but never accepting: the kernel backlog completes the TCP
        // handshake, yet no SSH server speaks.
        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        assert!(!ssh_banner_ready(silent.local_addr().unwrap()).await);
        assert!(!ssh_banner_ready(closed_port().await).await);

        let serving = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = serving.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let (mut socket, _) = serving.accept().await.unwrap();
            socket.write_all(b"SSH-2.0-test\r\n").await.unwrap();
        });
        assert!(ssh_banner_ready(address).await);
    }

    #[test]
    fn a_restarted_process_never_reuses_a_served_revision() {
        assert!(restarted_revision(7) > 7);
        assert_eq!(restarted_revision(u64::MAX - 1), u64::MAX);
    }

    #[tokio::test]
    async fn service_actions_share_the_upgrade_exclusion_lock() {
        let registry = UpgradeRegistry::new();
        let operation = registry.reserve_service_action().await.unwrap();
        assert_eq!(
            registry
                .reserve_service_action()
                .await
                .err()
                .unwrap()
                .code(),
            Some("SERVICE_OPERATION_IN_PROGRESS")
        );
        drop(operation);
        assert!(registry.reserve_service_action().await.is_ok());
    }

    #[tokio::test]
    async fn stale_cancellation_cannot_replace_a_new_task_journal() {
        let root =
            std::env::temp_dir().join(format!("router-upgrade-stale-cancel-{}", Uuid::new_v4()));
        let path = root.join("current.json");
        let registry = UpgradeRegistry::empty(Some(path.clone()));
        let old = UpgradeTaskSnapshot::new("old-boot", None);
        let old_id = old.task_id;
        let snapshot = UpgradeTaskSnapshot::new("new-boot", None);
        let task_id = snapshot.task_id.clone();
        let handle = UpgradeHandle::new(snapshot, Some(path.clone()), Arc::default());
        handle.update(|_| Ok(())).await.unwrap();
        *registry.inner.lock().await = Some(handle);
        assert_eq!(
            registry.cancel(&old_id).await.unwrap_err().code(),
            Some("UPGRADE_TASK_CHANGED")
        );
        assert_eq!(load_snapshot(path).await.unwrap().unwrap().task_id, task_id);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_corrupt_journal_fences_actions_without_blocking_router_startup() {
        let root = std::env::temp_dir().join(format!("router-upgrade-corrupt-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("upgrade")).unwrap();
        std::fs::write(root.join("upgrade/current.json"), b"invalid journal").unwrap();
        let registry = UpgradeRegistry::open(&root).await;
        assert_eq!(
            registry.snapshot(None).await.unwrap_err().code(),
            Some("UPGRADE_JOURNAL_UNAVAILABLE")
        );
        assert_eq!(
            registry
                .reserve_service_action()
                .await
                .err()
                .unwrap()
                .code(),
            Some("UPGRADE_JOURNAL_UNAVAILABLE")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_new_task_write_failure_is_visible_with_an_older_result() {
        let registry = UpgradeRegistry::new();
        let mut previous = UpgradeTaskSnapshot::new("old-boot", None);
        previous.status = UpgradeStatus::Cancelled;
        previous.can_cancel = false;
        let handle = UpgradeHandle::new(previous, None, registry.storage_fault.clone());
        *registry.inner.lock().await = Some(handle);
        registry.storage_fault.store(true, Ordering::SeqCst);
        let snapshot = registry.snapshot(None).await.unwrap().unwrap();
        assert!(snapshot.persistence_error);
        assert!(!snapshot.can_cancel);
        assert_eq!(
            registry
                .reserve_service_action()
                .await
                .err()
                .unwrap()
                .code(),
            Some("UPGRADE_JOURNAL_UNAVAILABLE")
        );
    }
}

pub async fn rollback_to_previous_binary() -> Result<RollbackResponse, AppError> {
    let target = Path::new(BINARY_INSTALL_PATH);
    let bak_path = BINARY_ROLLBACK_PATH.to_string();
    let bak = Path::new(&bak_path);
    if !bak.exists() {
        return Err(AppError::NotFound(format!(
            "rollback backup not found: {bak_path}"
        )));
    }
    chmod_exec(bak)?;
    sanity_exec(bak, false).await?;

    let rollback_tmp = target.with_file_name(format!(
        "cc-switch-router.rollback-current-{}",
        Uuid::new_v4()
    ));
    if target.exists() {
        std::fs::rename(target, &rollback_tmp)
            .map_err(|err| AppError::Internal(format!("stage current binary failed: {err}")))?;
    }
    if let Err(err) = std::fs::rename(bak, target) {
        if rollback_tmp.exists() {
            let _ = std::fs::rename(&rollback_tmp, target);
        }
        return Err(AppError::Internal(format!(
            "restore rollback backup failed: {err}"
        )));
    }
    if rollback_tmp.exists() {
        if let Err(err) = std::fs::rename(&rollback_tmp, bak) {
            warn!(
                current = %rollback_tmp.display(),
                backup = %bak.display(),
                error = %err,
                "failed to preserve replaced binary as rollback backup"
            );
            cleanup_tmp(&rollback_tmp);
        }
    }
    let strategy = RestartStrategy::from_manager(detect_service_status().manager);
    schedule_restart(strategy)?;
    Ok(RollbackResponse {
        ok: true,
        strategy: strategy.label().to_string(),
        backup_path: bak_path,
    })
}
