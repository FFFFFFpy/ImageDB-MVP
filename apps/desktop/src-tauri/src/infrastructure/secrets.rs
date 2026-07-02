use crate::error::AppError;
use std::path::{Path, PathBuf};

const CREDENTIALS_DIR: &str = "credentials";

pub struct CredentialStore {
    dir: PathBuf,
}

impl CredentialStore {
    pub fn new(app_data_dir: &Path) -> Result<Self, AppError> {
        let dir = app_data_dir.join(CREDENTIALS_DIR);
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    pub fn store(&self, key: &str, value: &str) -> Result<(), AppError> {
        let file_path = self.dir.join(key);
        std::fs::write(&file_path, value)
            .map_err(|e| AppError::Internal(format!("failed to write credential '{key}': {e}")))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o600)).map_err(
                |e| AppError::Internal(format!("failed to set credential permissions: {e}")),
            )?;
        }

        Ok(())
    }

    pub fn load(&self, key: &str) -> Result<Option<String>, AppError> {
        let file_path = self.dir.join(key);
        if file_path.exists() {
            let value = std::fs::read_to_string(&file_path).map_err(|e| {
                AppError::Internal(format!("failed to read credential '{key}': {e}"))
            })?;
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    pub fn delete(&self, key: &str) -> Result<(), AppError> {
        let file_path = self.dir.join(key);
        if file_path.exists() {
            std::fs::remove_file(&file_path).map_err(|e| {
                AppError::Internal(format!("failed to delete credential '{key}': {e}"))
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_store_and_load() {
        let tmp = TempDir::new().unwrap();
        let store = CredentialStore::new(tmp.path()).unwrap();

        store.store("db_password", "secret123").unwrap();
        let loaded = store.load("db_password").unwrap();
        assert_eq!(loaded, Some("secret123".to_string()));
    }

    #[test]
    fn test_load_missing_returns_none() {
        let tmp = TempDir::new().unwrap();
        let store = CredentialStore::new(tmp.path()).unwrap();
        let loaded = store.load("nonexistent").unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn test_delete() {
        let tmp = TempDir::new().unwrap();
        let store = CredentialStore::new(tmp.path()).unwrap();

        store.store("temp_key", "value").unwrap();
        assert!(store.load("temp_key").unwrap().is_some());

        store.delete("temp_key").unwrap();
        assert!(store.load("temp_key").unwrap().is_none());
    }
}
