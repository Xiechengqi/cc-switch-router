use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtomicCreateOutcome {
    Created,
    AlreadyExists,
}

pub(crate) fn atomic_replace_file_mode(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let (parent, temporary) = prepare_temporary_file(path, bytes, mode)?;
    let result = (|| -> Result<()> {
        fs::rename(&temporary, path)
            .with_context(|| format!("replace file failed: {}", path.display()))?;
        sync_directory(&parent)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn atomic_create_file_mode(
    path: &Path,
    bytes: &[u8],
    mode: u32,
) -> Result<AtomicCreateOutcome> {
    atomic_create_file_mode_with(path, bytes, mode, |_| Ok(()))
}

fn atomic_create_file_mode_with<F>(
    path: &Path,
    bytes: &[u8],
    mode: u32,
    before_activate: F,
) -> Result<AtomicCreateOutcome>
where
    F: FnOnce(&Path) -> Result<()>,
{
    let (parent, temporary) = prepare_temporary_file(path, bytes, mode)?;
    let result = (|| -> Result<AtomicCreateOutcome> {
        before_activate(&temporary)?;
        match fs::hard_link(&temporary, path) {
            Ok(()) => {
                fs::remove_file(&temporary).with_context(|| {
                    format!(
                        "remove activated temporary file failed: {}",
                        temporary.display()
                    )
                })?;
                sync_directory(&parent)?;
                Ok(AtomicCreateOutcome::Created)
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                fs::remove_file(&temporary).with_context(|| {
                    format!(
                        "remove losing temporary file failed: {}",
                        temporary.display()
                    )
                })?;
                Ok(AtomicCreateOutcome::AlreadyExists)
            }
            Err(error) => {
                Err(error).with_context(|| format!("activate file failed: {}", path.display()))
            }
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn enforce_file_mode(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        let metadata = fs::metadata(path)
            .with_context(|| format!("read file metadata failed: {}", path.display()))?;
        let current = metadata.permissions().mode() & 0o777;
        if current != mode {
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).with_context(|| {
                format!(
                    "set file permissions to {mode:o} failed: {}",
                    path.display()
                )
            })?;
            fs::File::open(path)
                .and_then(|file| file.sync_all())
                .with_context(|| format!("sync file permissions failed: {}", path.display()))?;
        }
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

fn prepare_temporary_file(path: &Path, bytes: &[u8], mode: u32) -> Result<(PathBuf, PathBuf)> {
    let parent = normalized_parent(path);
    fs::create_dir_all(&parent)
        .with_context(|| format!("create directory failed: {}", parent.display()))?;
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy())
        .unwrap_or_else(|| "secure-file".into());
    let temporary = parent.join(format!(".{name}.{}.tmp", Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(mode);
        let mut file = options
            .open(&temporary)
            .with_context(|| format!("create temporary file failed: {}", temporary.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("write temporary file failed: {}", temporary.display()))?;
        #[cfg(unix)]
        file.set_permissions(fs::Permissions::from_mode(mode))
            .with_context(|| {
                format!(
                    "set temporary file permissions failed: {}",
                    temporary.display()
                )
            })?;
        file.sync_all()
            .with_context(|| format!("sync temporary file failed: {}", temporary.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
        result?;
    }
    Ok((parent, temporary))
}

fn normalized_parent(path: &Path) -> PathBuf {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("sync directory failed: {}", path.display()))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cc-switch-router-{label}-{}", Uuid::new_v4()))
    }

    #[test]
    fn interrupted_create_never_exposes_partial_final_file() {
        let root = test_root("atomic-create-interrupted");
        let path = root.join("secret");
        let error = atomic_create_file_mode_with(&path, b"complete-secret", 0o600, |temporary| {
            assert!(!path.exists());
            assert_eq!(fs::read(temporary)?, b"complete-secret");
            anyhow::bail!("injected activation failure")
        })
        .expect_err("activation should fail");
        assert!(error.to_string().contains("injected activation failure"));
        assert!(!path.exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn atomic_create_never_replaces_an_existing_file() {
        let root = test_root("atomic-create-no-clobber");
        let path = root.join("secret");
        fs::create_dir_all(&root).unwrap();
        fs::write(&path, b"winner").unwrap();

        let outcome = atomic_create_file_mode(&path, b"loser", 0o600).unwrap();
        assert_eq!(outcome, AtomicCreateOutcome::AlreadyExists);
        assert_eq!(fs::read(&path).unwrap(), b"winner");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn atomic_replace_publishes_complete_content() {
        let root = test_root("atomic-replace");
        let path = root.join("state");
        fs::create_dir_all(&root).unwrap();
        fs::write(&path, b"old").unwrap();

        atomic_replace_file_mode(&path, b"new-state", 0o600).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new-state");
        fs::remove_dir_all(root).unwrap();
    }
}
