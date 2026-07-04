use crate::infrastructure::settings::AppSettings;
use crate::state::AppState;
use serde::{Deserialize, Serialize};
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
