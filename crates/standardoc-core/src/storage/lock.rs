use std::fs::{File, OpenOptions};
use std::path::Path;

use fs4::{FileExt, TryLockError};

use crate::storage::error::StorageError;

#[derive(Debug)]
pub(crate) struct WorkspaceLock {
    file: File,
}

impl WorkspaceLock {
    pub(crate) fn acquire(lock_path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        match FileExt::try_lock(&file) {
            Ok(()) => Ok(Self { file }),
            Err(TryLockError::WouldBlock) => Err(StorageError::LockHeld {
                path: lock_path.to_path_buf(),
            }),
            Err(TryLockError::Error(e)) => Err(StorageError::Io(e)),
        }
    }
}

impl Drop for WorkspaceLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn acquire_succeeds_on_fresh_path() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("db.lock");
        let _lock = WorkspaceLock::acquire(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn acquire_creates_parent_dir() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".standardoc/db.lock");
        let _lock = WorkspaceLock::acquire(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn second_acquire_in_same_process_fails() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("db.lock");
        let _lock1 = WorkspaceLock::acquire(&path).unwrap();
        let err = WorkspaceLock::acquire(&path).unwrap_err();
        assert!(matches!(err, StorageError::LockHeld { .. }));
    }

    #[test]
    fn release_on_drop_allows_reacquire() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("db.lock");
        {
            let _lock = WorkspaceLock::acquire(&path).unwrap();
        }
        let _lock2 = WorkspaceLock::acquire(&path).unwrap();
    }
}
