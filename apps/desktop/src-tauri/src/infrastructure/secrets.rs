use crate::error::AppError;
use keyring::Entry;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

#[cfg(test)]
const CREDENTIALS_DIR: &str = "credentials";
const SERVICE_NAME: &str = "ImageDB External PostgreSQL";

pub struct CredentialStore {
    backend: CredentialBackend,
}

enum CredentialBackend {
    System {
        service_name: String,
    },
    #[cfg(test)]
    File {
        dir: PathBuf,
    },
}

impl CredentialStore {
    pub fn new(_app_data_dir: &Path) -> Result<Self, AppError> {
        Ok(Self {
            backend: CredentialBackend::System {
                service_name: SERVICE_NAME.to_string(),
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
            CredentialBackend::System { service_name } => {
                let entry = Entry::new(service_name, key).map_err(|e| {
                    AppError::Internal(format!("failed to open system credential store: {e}"))
                })?;
                entry.set_password(value).map_err(|e| {
                    AppError::Internal(format!(
                        "failed to save credential '{key}' in system credential store: {e}"
                    ))
                })?;
            }
            #[cfg(test)]
            CredentialBackend::File { dir } => {
                let file_path = dir.join(test_file_key(key));
                std::fs::write(&file_path, value).map_err(|e| {
                    AppError::Internal(format!("failed to write credential '{key}': {e}"))
                })?;

                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o600))
                        .map_err(|e| {
                            AppError::Internal(format!("failed to set credential permissions: {e}"))
                        })?;
                }
            }
        }

        Ok(())
    }

    pub fn load(&self, key: &str) -> Result<Option<String>, AppError> {
        match &self.backend {
            CredentialBackend::System { service_name } => {
                let entry = Entry::new(service_name, key).map_err(|e| {
                    AppError::Internal(format!("failed to open system credential store: {e}"))
                })?;
                match entry.get_password() {
                    Ok(value) => Ok(Some(value)),
                    Err(keyring::Error::NoEntry) => Ok(None),
                    Err(e) => Err(AppError::Internal(format!(
                        "failed to read credential '{key}' from system credential store: {e}"
                    ))),
                }
            }
            #[cfg(test)]
            CredentialBackend::File { dir } => {
                let file_path = dir.join(test_file_key(key));
                if file_path.exists() {
                    let value = std::fs::read_to_string(&file_path).map_err(|e| {
                        AppError::Internal(format!("failed to read credential '{key}': {e}"))
                    })?;
                    Ok(Some(value))
                } else {
                    Ok(None)
                }
            }
        }
    }

    pub fn delete(&self, key: &str) -> Result<(), AppError> {
        match &self.backend {
            CredentialBackend::System { service_name } => {
                let entry = Entry::new(service_name, key).map_err(|e| {
                    AppError::Internal(format!("failed to open system credential store: {e}"))
                })?;
                match entry.delete_credential() {
                    Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                    Err(e) => Err(AppError::Internal(format!(
                        "failed to delete credential '{key}' from system credential store: {e}"
                    ))),
                }
            }
            #[cfg(test)]
            CredentialBackend::File { dir } => {
                let file_path = dir.join(test_file_key(key));
                if file_path.exists() {
                    std::fs::remove_file(&file_path).map_err(|e| {
                        AppError::Internal(format!("failed to delete credential '{key}': {e}"))
                    })?;
                }
                Ok(())
            }
        }
    }
}

pub fn external_profile_key(host: &str, port: u16, database: &str, username: &str) -> String {
    format!("{username}@{host}:{port}/{database}")
}

#[cfg(test)]
fn test_file_key(key: &str) -> String {
    blake3::hash(key.as_bytes()).to_hex().to_string()
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
}
