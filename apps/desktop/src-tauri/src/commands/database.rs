use crate::domain::{ConnectionConfig, DatabaseState, ExternalCheckResult};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use tauri::State;

#[derive(Debug, Serialize, Deserialize)]
pub struct ExternalConnectionDto {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: Option<String>,
}

impl From<ExternalConnectionDto> for ConnectionConfig {
    fn from(dto: ExternalConnectionDto) -> Self {
        ConnectionConfig {
            host: dto.host,
            port: dto.port,
            database: dto.database,
            username: dto.username,
            password: dto.password,
        }
    }
}

#[tauri::command]
pub async fn get_database_status(state: State<'_, AppState>) -> Result<DatabaseState, String> {
    let service = &state.database_service;
    Ok(service.get_state().await)
}

#[tauri::command]
pub async fn initialize_managed_database(
    state: State<'_, AppState>,
) -> Result<DatabaseState, String> {
    let service = &state.database_service;
    service
        .initialize_managed()
        .await
        .map_err(|e| format!("{e}"))
}

#[tauri::command]
pub async fn test_external_connection(
    state: State<'_, AppState>,
    config: ExternalConnectionDto,
) -> Result<ExternalCheckResult, String> {
    let service = &state.database_service;
    let conn_config: ConnectionConfig = config.into();
    service
        .test_external_connection(&conn_config)
        .await
        .map_err(|e| format!("{e}"))
}

#[tauri::command]
pub async fn initialize_external_database(
    state: State<'_, AppState>,
    config: ExternalConnectionDto,
) -> Result<DatabaseState, String> {
    let service = &state.database_service;
    let conn_config: ConnectionConfig = config.into();
    service
        .initialize_external(&conn_config)
        .await
        .map_err(|e| format!("{e}"))
}

#[tauri::command]
pub async fn shutdown_database(state: State<'_, AppState>) -> Result<(), String> {
    let mut mgr = state.postgres_manager.lock().await;
    mgr.shutdown().await.map_err(|e| format!("{e}"))
}
