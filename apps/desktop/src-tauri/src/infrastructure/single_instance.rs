use crate::error::AppError;
use fs2::FileExt;
use std::fs::File;
use std::path::{Path, PathBuf};

const LOCK_FILE: &str = "imagedb.lock";

#[derive(Debug)]
pub struct SingleInstanceLock {
    _file: File,
    lock_path: PathBuf,
}

impl SingleInstanceLock {
    pub fn acquire(app_data_dir: &Path) -> Result<Self, AppError> {
        std::fs::create_dir_all(app_data_dir)?;
        let lock_path = app_data_dir.join(LOCK_FILE);
        let file = File::create(&lock_path)?;

        match file.try_lock_exclusive() {
            Ok(()) => Ok(Self {
                _file: file,
                lock_path,
            }),
            Err(_) => Err(AppError::Internal(
                "Another instance of ImageDB is already running".to_string(),
            )),
        }
    }
}

impl Drop for SingleInstanceLock {
    fn drop(&mut self) {
        let _ = self._file.unlock();
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_acquire_lock() {
        let tmp = TempDir::new().unwrap();
        let lock = SingleInstanceLock::acquire(tmp.path());
        assert!(lock.is_ok());
    }

    #[test]
    fn test_double_lock_fails() {
        let tmp = TempDir::new().unwrap();
        let _lock1 = SingleInstanceLock::acquire(tmp.path()).unwrap();
        let lock2 = SingleInstanceLock::acquire(tmp.path());
        assert!(lock2.is_err());
        let err = lock2.unwrap_err().to_string();
        assert!(err.contains("already running"));
    }

    #[test]
    fn test_lock_released_on_drop() {
        let tmp = TempDir::new().unwrap();
        {
            let _lock = SingleInstanceLock::acquire(tmp.path()).unwrap();
        }
        let lock2 = SingleInstanceLock::acquire(tmp.path());
        assert!(lock2.is_ok());
    }
}
