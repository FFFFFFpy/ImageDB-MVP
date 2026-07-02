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
        first_run_completed: settings.first_run_completed,
    };
    store.update(app_settings).map_err(|e| format!("{e}"))?;
    Ok(SettingsDto::from(store.get()))
}
