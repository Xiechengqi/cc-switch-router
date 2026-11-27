//! Host-local upgrade journal. The task mutex is also the cancellation/install
//! boundary: neither cancellation nor binary replacement may bypass it.
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, broadcast};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::AppError;

pub const UPGRADE_PROTOCOL_VERSION: u32 = 1;
pub const TOTAL_STEPS: usize = 7;
const MAX_LOG_LINES: usize = 1000;
const MAX_MESSAGE_CHARS: usize = 1024;
const MAX_JOURNAL_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
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
    pub seq: u64,
    pub task_id: String,
    pub step: usize,
    pub total_steps: usize,
    pub level: UpgradeLogLevel,
    pub message: String,
    pub progress: Option<u8>,
    pub at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeStatus {
    Running,
    Cancelling,
    Restarting,
    Success,
    Failed,
    Cancelled,
    Interrupted,
}

impl UpgradeStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Success | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpgradePhase {
    Preparing,
    Downloading,
    Validating,
    Hashing,
    Installing,
    Restarting,
    Verifying,
    Complete,
}

impl UpgradePhase {
    pub fn cancellable(self) -> bool {
        matches!(
            self,
            Self::Preparing | Self::Downloading | Self::Validating | Self::Hashing
        )
    }

    pub fn step(self) -> usize {
        match self {
            Self::Preparing => 1,
            Self::Downloading => 2,
            Self::Validating => 3,
            Self::Hashing => 4,
            Self::Installing => 5,
            Self::Restarting => 6,
            Self::Verifying | Self::Complete => 7,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeBuild {
    pub version: String,
    pub commit: String,
    pub build_time: String,
    #[serde(default)]
    pub upgrade_protocol_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeTaskSnapshot {
    pub schema_version: u32,
    pub task_id: String,
    pub revision: u64,
    pub status: UpgradeStatus,
    pub phase: UpgradePhase,
    pub can_cancel: bool,
    pub actor: String,
    pub source_instance_id: String,
    pub target: Option<UpgradeBuild>,
    pub target_sha256: Option<String>,
    pub binary_replaced: bool,
    pub restart_confirmed: bool,
    pub started_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub progress: Option<u8>,
    pub error: Option<String>,
    pub persistence_error: bool,
    pub last_seq: u64,
    pub logs_truncated: bool,
    pub logs: Vec<UpgradeLogEntry>,
}

impl UpgradeTaskSnapshot {
    pub fn new(instance_id: &str, actor: Option<String>) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            schema_version: UPGRADE_PROTOCOL_VERSION,
            task_id: Uuid::new_v4().to_string(),
            revision: 0,
            status: UpgradeStatus::Running,
            phase: UpgradePhase::Preparing,
            can_cancel: true,
            actor: actor.unwrap_or_else(|| "unknown".into()),
            source_instance_id: instance_id.into(),
            target: None,
            target_sha256: None,
            binary_replaced: false,
            restart_confirmed: false,
            started_at: now.clone(),
            updated_at: now,
            finished_at: None,
            downloaded_bytes: 0,
            total_bytes: None,
            progress: None,
            error: None,
            persistence_error: false,
            last_seq: 0,
            logs_truncated: false,
            logs: Vec::new(),
        }
    }

    pub fn append_log(&mut self, level: UpgradeLogLevel, message: impl Into<String>) {
        self.last_seq += 1;
        self.logs.push(UpgradeLogEntry {
            seq: self.last_seq,
            task_id: self.task_id.clone(),
            step: self.phase.step(),
            total_steps: TOTAL_STEPS,
            level,
            message: message.into().chars().take(MAX_MESSAGE_CHARS).collect(),
            progress: self.progress,
            at: Utc::now().to_rfc3339(),
        });
        if self.logs.len() > MAX_LOG_LINES {
            self.logs.drain(..self.logs.len() - MAX_LOG_LINES);
            self.logs_truncated = true;
        }
    }

    fn validate(&self) -> Result<(), AppError> {
        if self.schema_version != UPGRADE_PROTOCOL_VERSION
            || Uuid::parse_str(&self.task_id).is_err()
            || self.logs.len() > MAX_LOG_LINES
            || self
                .logs
                .iter()
                .any(|line| line.task_id != self.task_id || line.seq > self.last_seq)
            || self.logs.windows(2).any(|pair| pair[0].seq >= pair[1].seq)
            || self.can_cancel
                != (self.status == UpgradeStatus::Running && self.phase.cancellable())
            || (self.binary_replaced
                && (self.can_cancel || self.status == UpgradeStatus::Cancelled))
            || (self.status == UpgradeStatus::Cancelling && !self.phase.cancellable())
            || self.target_sha256.as_deref().is_some_and(|sha| {
                sha.len() != 64 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            || (self.status == UpgradeStatus::Success
                && (!self.restart_confirmed
                    || !self.binary_replaced
                    || self.phase != UpgradePhase::Complete))
        {
            return Err(AppError::Internal(
                "invalid or unsupported upgrade journal".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct UpgradeHandle {
    pub task_id: String,
    pub sender: broadcast::Sender<()>,
    pub cancel: CancellationToken,
    state: Arc<Mutex<UpgradeTaskSnapshot>>,
    journal: Option<PathBuf>,
    pub storage_fault: Arc<AtomicBool>,
}

impl UpgradeHandle {
    pub fn new(
        snapshot: UpgradeTaskSnapshot,
        journal: Option<PathBuf>,
        storage_fault: Arc<AtomicBool>,
    ) -> Self {
        Self {
            task_id: snapshot.task_id.clone(),
            sender: broadcast::channel(32).0,
            cancel: CancellationToken::new(),
            state: Arc::new(Mutex::new(snapshot)),
            journal,
            storage_fault,
        }
    }

    pub async fn snapshot(&self) -> UpgradeTaskSnapshot {
        self.state.lock().await.clone()
    }

    /// Persist before publishing. Holding this mutex through the atomic write
    /// serializes cancel acceptance with the transition into installation.
    pub async fn update(
        &self,
        change: impl FnOnce(&mut UpgradeTaskSnapshot) -> Result<(), AppError>,
    ) -> Result<UpgradeTaskSnapshot, AppError> {
        let mut guard = self.state.lock().await;
        let mut next = guard.clone();
        change(&mut next)?;
        next.revision += 1;
        next.updated_at = Utc::now().to_rfc3339();
        next.can_cancel = next.status == UpgradeStatus::Running && next.phase.cancellable();
        if let Some(path) = &self.journal {
            if let Err(error) = persist_snapshot(path.clone(), next.clone()).await {
                self.storage_fault.store(true, Ordering::SeqCst);
                guard.persistence_error = true;
                // The filesystem rename cannot be undone by a failed journal
                // write. Preserve this fact in the live diagnostic snapshot.
                guard.binary_replaced |= next.binary_replaced;
                guard.error = Some(format!("upgrade state could not be persisted: {error}"));
                guard.can_cancel = false;
                guard.revision += 1;
                guard.updated_at = Utc::now().to_rfc3339();
                if guard.phase.cancellable() {
                    self.cancel.cancel();
                }
                let _ = self.sender.send(());
                return Err(error);
            }
        }
        *guard = next.clone();
        let _ = self.sender.send(());
        Ok(next)
    }

    pub async fn phase(&self, phase: UpgradePhase, message: &str) -> Result<(), AppError> {
        self.update(|next| {
            if next.status == UpgradeStatus::Cancelling || self.cancel.is_cancelled() {
                return Err(cancelled_error());
            }
            next.phase = phase;
            next.progress = None;
            next.append_log(UpgradeLogLevel::Info, message);
            Ok(())
        })
        .await?;
        Ok(())
    }

    pub async fn cancel(&self) -> Result<UpgradeTaskSnapshot, AppError> {
        let current = self.snapshot().await;
        if matches!(
            current.status,
            UpgradeStatus::Cancelled | UpgradeStatus::Cancelling
        ) {
            self.cancel.cancel();
            return Ok(current);
        }
        let next = self.update(|next| {
            if matches!(next.status, UpgradeStatus::Cancelled | UpgradeStatus::Cancelling) {
                return Ok(());
            }
            if !next.can_cancel {
                return Err(AppError::coded_conflict(
                    "UPGRADE_NOT_CANCELLABLE",
                    "upgrade can no longer be interrupted",
                    serde_json::json!({"taskId": next.task_id, "status": next.status, "phase": next.phase}),
                ));
            }
            next.status = UpgradeStatus::Cancelling;
            next.append_log(UpgradeLogLevel::Warn, "interruption requested; stopping and cleaning up");
            Ok(())
        }).await?;
        // The persisted Cancelling state already blocks installation even if
        // the worker wins this scheduling race before the token is signalled.
        self.cancel.cancel();
        Ok(next)
    }

    pub async fn finish(&self, status: UpgradeStatus, error: Option<String>, message: &str) {
        let change = |next: &mut UpgradeTaskSnapshot| {
            next.status = status;
            next.error = error;
            next.finished_at = Some(Utc::now().to_rfc3339());
            if status == UpgradeStatus::Success {
                next.phase = UpgradePhase::Complete;
                next.restart_confirmed = true;
            }
            next.append_log(
                match status {
                    UpgradeStatus::Success => UpgradeLogLevel::Success,
                    UpgradeStatus::Failed | UpgradeStatus::Interrupted => UpgradeLogLevel::Error,
                    _ => UpgradeLogLevel::Warn,
                },
                message,
            );
            Ok(())
        };
        if let Err(error) = self.update(change).await {
            // Keep the UI truthful even if the disk has failed. New service
            // actions remain fenced by storage_fault until process recovery.
            tracing::error!(%error, task_id = %self.task_id, "persist upgrade result failed");
            let mut guard = self.state.lock().await;
            guard.status = UpgradeStatus::Failed;
            guard.can_cancel = false;
            guard.persistence_error = true;
            guard.finished_at = Some(Utc::now().to_rfc3339());
            guard.error = Some("upgrade state could not be persisted; inspect the Router service log before another operation".into());
            guard.append_log(UpgradeLogLevel::Error, message);
            guard.revision += 1;
            let _ = self.sender.send(());
        }
    }

    pub fn check_cancelled(&self) -> Result<(), AppError> {
        if self.cancel.is_cancelled() {
            Err(cancelled_error())
        } else {
            Ok(())
        }
    }
}

pub fn cancelled_error() -> AppError {
    AppError::Conflict("upgrade interruption requested".into())
}

pub async fn load_snapshot(path: PathBuf) -> Result<Option<UpgradeTaskSnapshot>, AppError> {
    tokio::task::spawn_blocking(move || {
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(journal_error(error)),
        };
        if metadata.len() > MAX_JOURNAL_BYTES {
            return Err(AppError::Internal(
                "upgrade journal exceeds its size limit".into(),
            ));
        }
        let snapshot: UpgradeTaskSnapshot =
            serde_json::from_slice(&fs::read(path).map_err(journal_error)?)
                .map_err(journal_error)?;
        snapshot.validate()?;
        Ok(Some(snapshot))
    })
    .await
    .map_err(journal_error)?
}

pub async fn persist_snapshot(
    path: PathBuf,
    snapshot: UpgradeTaskSnapshot,
) -> Result<(), AppError> {
    tokio::task::spawn_blocking(move || write_snapshot(&path, &snapshot))
        .await
        .map_err(journal_error)?
}

fn write_snapshot(path: &Path, snapshot: &UpgradeTaskSnapshot) -> Result<(), AppError> {
    snapshot.validate()?;
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Internal("upgrade journal has no parent".into()))?;
    let parent_existed = parent.is_dir();
    fs::create_dir_all(parent).map_err(journal_error)?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(journal_error)?;
    if !parent_existed && let Some(grandparent) = parent.parent() {
        File::open(grandparent)
            .and_then(|directory| directory.sync_all())
            .map_err(journal_error)?;
    }
    let bytes = serde_json::to_vec(snapshot).map_err(journal_error)?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(AppError::Internal(
            "upgrade journal exceeds its size limit".into(),
        ));
    }
    let temporary = path.with_extension("json.new");
    let result = (|| {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(journal_error)?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(journal_error)?;
        fs::rename(&temporary, path).map_err(journal_error)?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(journal_error)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn journal_error(error: impl std::fmt::Display) -> AppError {
    AppError::Internal(format!("upgrade journal: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn accepted_cancellation_prevents_the_install_boundary() {
        let handle =
            UpgradeHandle::new(UpgradeTaskSnapshot::new("boot", None), None, Arc::default());
        handle
            .phase(UpgradePhase::Hashing, "hashing")
            .await
            .unwrap();
        let cancelled = handle.cancel().await.unwrap();
        assert_eq!(cancelled.status, UpgradeStatus::Cancelling);
        assert!(
            handle
                .phase(UpgradePhase::Installing, "installing")
                .await
                .is_err()
        );
        assert_eq!(handle.snapshot().await.phase, UpgradePhase::Hashing);
    }

    #[tokio::test]
    async fn installation_rejects_a_late_cancellation() {
        let handle =
            UpgradeHandle::new(UpgradeTaskSnapshot::new("boot", None), None, Arc::default());
        handle
            .phase(UpgradePhase::Installing, "installing")
            .await
            .unwrap();
        assert_eq!(
            handle.cancel().await.unwrap_err().code(),
            Some("UPGRADE_NOT_CANCELLABLE")
        );
        assert!(!handle.cancel.is_cancelled());
    }

    #[tokio::test]
    async fn journal_replays_a_completed_result_and_bounds_logs() {
        let directory =
            std::env::temp_dir().join(format!("router-upgrade-journal-{}", Uuid::new_v4()));
        let path = directory.join("current.json");
        let mut snapshot = UpgradeTaskSnapshot::new("old-boot", None);
        for index in 0..MAX_LOG_LINES + 4 {
            snapshot.append_log(UpgradeLogLevel::Info, format!("entry {index}"));
        }
        let handle = UpgradeHandle::new(snapshot, Some(path.clone()), Arc::default());
        handle
            .finish(UpgradeStatus::Cancelled, None, "cancelled")
            .await;
        let recovered = load_snapshot(path).await.unwrap().unwrap();
        assert_eq!(recovered.status, UpgradeStatus::Cancelled);
        assert!(!recovered.can_cancel);
        assert!(recovered.logs_truncated);
        assert_eq!(recovered.logs.len(), MAX_LOG_LINES);
        assert_eq!(recovered.last_seq, (MAX_LOG_LINES + 5) as u64);
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn a_failed_journal_write_fences_installation() {
        let root =
            std::env::temp_dir().join(format!("router-upgrade-write-failure-{}", Uuid::new_v4()));
        fs::write(&root, b"not a directory").unwrap();
        let fault = Arc::new(AtomicBool::new(false));
        let handle = UpgradeHandle::new(
            UpgradeTaskSnapshot::new("boot", None),
            Some(root.join("current.json")),
            fault.clone(),
        );
        assert!(
            handle
                .phase(UpgradePhase::Installing, "installing")
                .await
                .is_err()
        );
        assert!(fault.load(Ordering::SeqCst));
        assert_eq!(handle.snapshot().await.phase, UpgradePhase::Preparing);
        fs::remove_file(root).unwrap();
    }
}
