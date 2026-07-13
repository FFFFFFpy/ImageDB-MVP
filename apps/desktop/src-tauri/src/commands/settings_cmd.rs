use crate::infrastructure::settings::AppSettings;
use crate::infrastructure::storage_capabilities::{
    probe_storage_capabilities as run_storage_capability_probe, StorageCapabilities,
};
use crate::state::{AppState, CriticalOperationKind};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::State;

#[derive(Debug, Serialize, Deserialize)]
pub struct SettingsDto {
    pub database_mode: Option<String>,
    pub library_root: Option<String>,
    pub external_host: Option<String>,
    pub external_port: Option<u16>,
    pub external_database: Option<String>,
    pub external_username: Option<String>,
    pub external_tls_mode: Option<String>,
    pub external_ca_cert_path: Option<String>,
    pub external_client_cert_path: Option<String>,
    pub external_client_key_path: Option<String>,
    pub external_connect_timeout_secs: Option<u64>,
    pub external_query_timeout_secs: Option<u64>,
    pub external_profile_name: Option<String>,
    pub first_run_completed: bool,
}

impl From<&AppSettings> for SettingsDto {
    fn from(s: &AppSettings) -> Self {
        Self {
            database_mode: s.database_mode.clone(),
            library_root: s.library_root.clone(),
            external_host: s.external_host.clone(),
            external_port: s.external_port,
            external_database: s.external_database.clone(),
            external_username: s.external_username.clone(),
            external_tls_mode: s.external_tls_mode.clone(),
            external_ca_cert_path: s.external_ca_cert_path.clone(),
            external_client_cert_path: s.external_client_cert_path.clone(),
            external_client_key_path: s.external_client_key_path.clone(),
            external_connect_timeout_secs: s.external_connect_timeout_secs,
            external_query_timeout_secs: s.external_query_timeout_secs,
            external_profile_name: s.external_profile_name.clone(),
            first_run_completed: s.first_run_completed,
        }
    }
}

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<SettingsDto, String> {
    let settings = state.settings.lock().await;
    Ok(SettingsDto::from(settings.get()))
}

#[tauri::command]
pub async fn update_settings(
    state: State<'_, AppState>,
    settings: SettingsDto,
) -> Result<SettingsDto, String> {
    update_settings_for_state(&state, settings).await
}

pub(crate) async fn update_settings_for_state(
    state: &AppState,
    settings: SettingsDto,
) -> Result<SettingsDto, String> {
    // SettingsDto currently consists entirely of database profile, library
    // root, and onboarding fields. Serialize the whole update with database
    // lifecycle changes so a full-form save cannot overwrite a critical field
    // after a stale pre-check.
    let _operation = state
        .critical_operation_guard
        .begin_operation(CriticalOperationKind::UpdateCriticalSettings)?;
    let mut store = state.settings.lock().await;
    let app_settings = AppSettings {
        database_mode: settings.database_mode,
        library_root: settings.library_root,
        external_host: settings.external_host,
        external_port: settings.external_port,
        external_database: settings.external_database,
        external_username: settings.external_username,
        external_tls_mode: settings.external_tls_mode,
        external_ca_cert_path: settings.external_ca_cert_path,
        external_client_cert_path: settings.external_client_cert_path,
        external_client_key_path: settings.external_client_key_path,
        external_connect_timeout_secs: settings.external_connect_timeout_secs,
        external_query_timeout_secs: settings.external_query_timeout_secs,
        external_profile_name: settings.external_profile_name,
        first_run_completed: settings.first_run_completed,
    };
    store.update(app_settings).map_err(|e| format!("{e}"))?;
    Ok(SettingsDto::from(store.get()))
}

#[tauri::command]
pub async fn probe_storage_capabilities(path: String) -> Result<StorageCapabilities, String> {
    if path.trim().is_empty() {
        return Err("storage path is required".to_string());
    }

    Ok(run_storage_capability_probe(PathBuf::from(path)))
}

#[cfg(test)]
mod critical_settings_tests {
    use super::*;
    use crate::state::CriticalTaskKind;
    use tempfile::TempDir;

    fn settings(library_root: &str) -> SettingsDto {
        SettingsDto {
            database_mode: Some("managed_local".to_string()),
            library_root: Some(library_root.to_string()),
            external_host: None,
            external_port: None,
            external_database: None,
            external_username: None,
            external_tls_mode: None,
            external_ca_cert_path: None,
            external_client_cert_path: None,
            external_client_key_path: None,
            external_connect_timeout_secs: None,
            external_query_timeout_secs: None,
            external_profile_name: None,
            first_run_completed: true,
        }
    }

    #[tokio::test]
    async fn active_commit_rejects_library_root_update_without_losing_existing_setting() {
        let temp = TempDir::new().unwrap();
        let state = AppState::new(temp.path(), temp.path().join("fixtures")).unwrap();
        state
            .settings
            .lock()
            .await
            .update(AppSettings {
                database_mode: Some("managed_local".to_string()),
                library_root: Some("C:/library/original".to_string()),
                first_run_completed: true,
                ..AppSettings::default()
            })
            .unwrap();
        let commit = state
            .critical_operation_guard
            .begin_task(CriticalTaskKind::Commit)
            .unwrap();

        let error = update_settings_for_state(&state, settings("C:/library/replacement"))
            .await
            .unwrap_err();
        assert_eq!(
            error,
            "cannot change database or library settings while import commit is running"
        );
        assert_eq!(
            state.settings.lock().await.get().library_root.as_deref(),
            Some("C:/library/original")
        );
        assert_eq!(
            state.critical_operation_guard.snapshot().active_task_kinds,
            vec!["commit"]
        );

        drop(commit);
        let updated = update_settings_for_state(&state, settings("C:/library/replacement"))
            .await
            .expect("settings update must work after commit completion");
        assert_eq!(
            updated.library_root.as_deref(),
            Some("C:/library/replacement")
        );
    }
}
