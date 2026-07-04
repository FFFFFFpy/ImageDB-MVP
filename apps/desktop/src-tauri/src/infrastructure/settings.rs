use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const SETTINGS_FILE: &str = "settings.toml";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppSettings {
    #[serde(default)]
    pub database_mode: Option<String>,
    #[serde(default)]
    pub library_root: Option<String>,
    #[serde(default)]
    pub external_host: Option<String>,
    #[serde(default)]
    pub external_port: Option<u16>,
    #[serde(default)]
    pub external_database: Option<String>,
    #[serde(default)]
    pub external_username: Option<String>,
    #[serde(default)]
    pub external_tls_mode: Option<String>,
    #[serde(default)]
    pub external_ca_cert_path: Option<String>,
    #[serde(default)]
    pub external_client_cert_path: Option<String>,
    #[serde(default)]
    pub external_client_key_path: Option<String>,
    #[serde(default)]
    pub external_connect_timeout_secs: Option<u64>,
    #[serde(default)]
    pub external_query_timeout_secs: Option<u64>,
    #[serde(default)]
    pub external_profile_name: Option<String>,
    #[serde(default)]
    pub first_run_completed: bool,
}

pub struct SettingsStore {
    file_path: PathBuf,
    settings: AppSettings,
}

impl SettingsStore {
    pub fn new(app_data_dir: &Path) -> Result<Self, AppError> {
        let file_path = app_data_dir.join(SETTINGS_FILE);

        let settings = if file_path.exists() {
            let content = std::fs::read_to_string(&file_path)
                .map_err(|e| AppError::Internal(format!("failed to read settings: {e}")))?;
            toml::from_str(&content).unwrap_or_else(|e| {
                tracing::warn!("Failed to parse settings file, using defaults: {e}");
                AppSettings::default()
            })
        } else {
            AppSettings::default()
        };

        Ok(Self {
            file_path,
            settings,
        })
    }

    pub fn get(&self) -> &AppSettings {
        &self.settings
    }

    pub fn save(&mut self) -> Result<(), AppError> {
        let content = toml::to_string_pretty(&self.settings)
            .map_err(|e| AppError::Internal(format!("failed to serialize settings: {e}")))?;

        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&self.file_path, content)
            .map_err(|e| AppError::Internal(format!("failed to write settings: {e}")))?;

        Ok(())
    }

    pub fn update(&mut self, settings: AppSettings) -> Result<(), AppError> {
        self.settings = settings;
        self.save()
    }

    pub fn set_database_mode(&mut self, mode: &str) -> Result<(), AppError> {
        self.settings.database_mode = Some(mode.to_string());
        self.save()
    }

    pub fn set_library_root(&mut self, path: &str) -> Result<(), AppError> {
        self.settings.library_root = Some(path.to_string());
        self.save()
    }

    pub fn set_first_run_completed(&mut self, completed: bool) -> Result<(), AppError> {
        self.settings.first_run_completed = completed;
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_settings() {
        let tmp = TempDir::new().unwrap();
        let store = SettingsStore::new(tmp.path()).unwrap();
        let settings = store.get();
        assert!(!settings.first_run_completed);
        assert!(settings.database_mode.is_none());
        assert!(settings.library_root.is_none());
    }

    #[test]
    fn test_save_and_reload() {
        let tmp = TempDir::new().unwrap();
        let mut store = SettingsStore::new(tmp.path()).unwrap();
        store.set_database_mode("managed_local").unwrap();
        store.set_library_root("/tmp/library").unwrap();
        store.set_first_run_completed(true).unwrap();

        let store2 = SettingsStore::new(tmp.path()).unwrap();
        let s = store2.get();
        assert_eq!(s.database_mode.as_deref(), Some("managed_local"));
        assert_eq!(s.library_root.as_deref(), Some("/tmp/library"));
        assert!(s.first_run_completed);
    }

    #[test]
    fn test_corrupt_settings_uses_defaults() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(SETTINGS_FILE), "not valid toml {{{").unwrap();
        let store = SettingsStore::new(tmp.path()).unwrap();
        assert!(!store.get().first_run_completed);
    }
}
