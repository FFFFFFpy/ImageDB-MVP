use crate::error::AppError;
use keyring::Entry;
use std::path::{Path, PathBuf};

const CREDENTIALS_DIR: &str = "credentials";
const SERVICE_NAME: &str = "ImageDB External PostgreSQL";

pub struct CredentialStore {
    backend: CredentialBackend,
}

enum CredentialBackend {
    System {
        service_name: String,
        fallback_dir: PathBuf,
    },
    #[cfg(test)]
    File { dir: PathBuf },
}

impl CredentialStore {
    pub fn new(app_data_dir: &Path) -> Result<Self, AppError> {
        let fallback_dir = app_data_dir.join(CREDENTIALS_DIR);
        std::fs::create_dir_all(&fallback_dir)?;
        Ok(Self {
            backend: CredentialBackend::System {
                service_name: SERVICE_NAME.to_string(),
                fallback_dir,
            },
        })
    }

    #[cfg(test)]
    pub(crate) fn new_file_for_tests(app_data_dir: &Path) -> Result<Self, AppError> {
        let dir = app_data_dir.join(CREDENTIALS_DIR);
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            backend: CredentialBackend::File { dir },
        })
    }

    pub fn store(&self, key: &str, value: &str) -> Result<(), AppError> {
        match &self.backend {
            CredentialBackend::System {
                service_name,
                fallback_dir,
            } => match Entry::new(service_name, key) {
                Ok(entry) => {
                    if entry.set_password(value).is_err() {
                        write_file_credential(fallback_dir, key, value)?;
                    }
                }
                Err(_) => {
                    write_file_credential(fallback_dir, key, value)?;
                }
            },
            #[cfg(test)]
            CredentialBackend::File { dir } => {
                write_file_credential(dir, key, value)?;
            }
        }

        Ok(())
    }

    pub fn load(&self, key: &str) -> Result<Option<String>, AppError> {
        match &self.backend {
            CredentialBackend::System {
                service_name,
                fallback_dir,
            } => match Entry::new(service_name, key) {
                Ok(entry) => match entry.get_password() {
                    Ok(value) => Ok(Some(value)),
                    Err(keyring::Error::NoEntry) => read_file_credential(fallback_dir, key),
                    Err(_) => read_file_credential(fallback_dir, key),
                },
                Err(_) => read_file_credential(fallback_dir, key),
            },
            #[cfg(test)]
            CredentialBackend::File { dir } => read_file_credential(dir, key),
        }
    }

    pub fn delete(&self, key: &str) -> Result<(), AppError> {
        match &self.backend {
            CredentialBackend::System {
                service_name,
                fallback_dir,
            } => {
                if let Ok(entry) = Entry::new(service_name, key) {
                    let _ = entry.delete_credential();
                }
                delete_file_credential(fallback_dir, key)
            }
            #[cfg(test)]
            CredentialBackend::File { dir } => delete_file_credential(dir, key),
        }
    }
}

pub fn external_profile_key(host: &str, port: u16, database: &str, username: &str) -> String {
    format!("{username}@{host}:{port}/{database}")
}

fn file_key(key: &str) -> String {
    blake3::hash(key.as_bytes()).to_hex().to_string()
}

fn write_file_credential(dir: &Path, key: &str, value: &str) -> Result<(), AppError> {
    std::fs::create_dir_all(dir)?;
    let file_path = dir.join(file_key(key));
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

fn read_file_credential(dir: &Path, key: &str) -> Result<Option<String>, AppError> {
    let file_path = dir.join(file_key(key));
    if !file_path.exists() {
        return Ok(None);
    }

    let value = std::fs::read_to_string(&file_path)
        .map_err(|e| AppError::Internal(format!("failed to read credential '{key}': {e}")))?;
    Ok(Some(value))
}

fn delete_file_credential(dir: &Path, key: &str) -> Result<(), AppError> {
    let file_path = dir.join(file_key(key));
    if file_path.exists() {
        std::fs::remove_file(&file_path)
            .map_err(|e| AppError::Internal(format!("failed to delete credential '{key}': {e}")))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_store_and_load() {
        let tmp = TempDir::new().unwrap();
        let store = CredentialStore::new_file_for_tests(tmp.path()).unwrap();

        store.store("db_password", "secret123").unwrap();
        let loaded = store.load("db_password").unwrap();
        assert_eq!(loaded, Some("secret123".to_string()));
    }

    #[test]
    fn test_load_missing_returns_none() {
        let tmp = TempDir::new().unwrap();
        let store = CredentialStore::new_file_for_tests(tmp.path()).unwrap();
        let loaded = store.load("nonexistent").unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn test_delete() {
        let tmp = TempDir::new().unwrap();
        let store = CredentialStore::new_file_for_tests(tmp.path()).unwrap();

        store.store("temp_key", "value").unwrap();
        assert!(store.load("temp_key").unwrap().is_some());

        store.delete("temp_key").unwrap();
        assert!(store.load("temp_key").unwrap().is_none());
    }

    #[test]
    fn production_store_falls_back_to_app_data_files() {
        let tmp = TempDir::new().unwrap();
        let store = CredentialStore::new(tmp.path()).unwrap();
        let key = "external@example.test:5432/imagedb";

        store.store(key, "fallback-secret").unwrap();
        assert_eq!(store.load(key).unwrap().as_deref(), Some("fallback-secret"));

        store.delete(key).unwrap();
        assert!(store.load(key).unwrap().is_none());
    }
}
