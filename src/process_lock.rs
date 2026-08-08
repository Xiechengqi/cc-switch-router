use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::{Context, Result};
use fs2::FileExt;

pub struct ProcessLock {
    file: File,
}

impl ProcessLock {
    pub fn acquire(data_dir: &Path) -> Result<Self> {
        fs::create_dir_all(data_dir).with_context(|| {
            format!(
                "create Router data directory failed: {}",
                data_dir.display()
            )
        })?;
        let path = data_dir.join("cc-switch-router.lock");
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open Router process lock failed: {}", path.display()))?;
        file.try_lock_exclusive().with_context(|| {
            format!(
                "another Router process is using {}; stop it before running this command",
                data_dir.display()
            )
        })?;
        file.set_len(0)
            .context("truncate Router process lock metadata failed")?;
        file.seek(SeekFrom::Start(0))
            .context("seek Router process lock failed")?;
        writeln!(file, "{}", std::process::id())
            .context("write Router process lock metadata failed")?;
        file.sync_all()
            .context("sync Router process lock metadata failed")?;
        Ok(Self { file })
    }
}

impl Drop for ProcessLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_second_process_lock() {
        let root = std::env::temp_dir().join(format!(
            "cc-switch-router-process-lock-{}",
            uuid::Uuid::new_v4()
        ));
        let first = ProcessLock::acquire(&root).expect("acquire first lock");
        assert!(ProcessLock::acquire(&root).is_err());
        drop(first);
        ProcessLock::acquire(&root).expect("reacquire released lock");
        let _ = fs::remove_dir_all(root);
    }
}
