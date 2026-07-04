use crate::domain::{ConnectionConfig, DatabaseState, ExternalCheckResult, TlsMode};
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
    pub tls_mode: Option<String>,
    pub ca_cert_path: Option<String>,
    pub client_cert_path: Option<String>,
    pub client_key_path: Option<String>,
    pub connect_timeout_secs: Option<u64>,
    pub query_timeout_secs: Option<u64>,
    pub profile_name: Option<String>,
}

impl From<ExternalConnectionDto> for ConnectionConfig {
    fn from(dto: ExternalConnectionDto) -> Self {
        ConnectionConfig {
            host: dto.host,
            port: dto.port,
            database: dto.database,
            username: dto.username,
            password: dto.password,
            tls_mode: dto
                .tls_mode
                .as_deref()
                .and_then(TlsMode::from_str_opt)
                .unwrap_or_default(),
            ca_cert_path: dto.ca_cert_path,
            client_cert_path: dto.client_cert_path,
            client_key_path: dto.client_key_path,
            connect_timeout_secs: dto.connect_timeout_secs.unwrap_or(10),
            query_timeout_secs: dto.query_timeout_secs.unwrap_or(15),
            profile_name: dto.profile_name,
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
