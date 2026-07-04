use crate::domain::{
    ConnectionConfig, DatabaseState, ExternalCheckResult, ExternalMigrationProgress,
    ExternalMigrationResult, TlsMode,
};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
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
pub async fn switch_to_managed_database(
    state: State<'_, AppState>,
) -> Result<DatabaseState, String> {
    let service = &state.database_service;
    service
        .switch_to_managed()
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
pub async fn migrate_managed_to_external_database(
    state: State<'_, AppState>,
    config: ExternalConnectionDto,
) -> Result<ExternalMigrationResult, String> {
    let service = &state.database_service;
    let conn_config: ConnectionConfig = config.into();
    service
        .migrate_managed_to_external(&conn_config)
        .await
        .map_err(|e| format!("{e}"))
}

#[tauri::command]
pub async fn start_managed_to_external_migration(
    state: State<'_, AppState>,
    config: ExternalConnectionDto,
) -> Result<String, String> {
    let conn_config: ConnectionConfig = config.into();
    let mut migration_state = state.external_migration_state.lock().await;

    if migration_state
        .active
        .as_ref()
        .map(|handle| handle.task.is_finished())
        .unwrap_or(false)
    {
        if let Some(handle) = migration_state.active.take() {
            let progress = resolve_external_migration_handle(handle).await;
            let mut tracker = migration_state.progress_tracker.lock().await;
            *tracker = progress;
        }
    }

    if migration_state.active.is_some() {
        return Err("An external migration is already running".to_string());
    }

    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let progress_tracker = std::sync::Arc::new(tokio::sync::Mutex::new(
        ExternalMigrationProgress::running("queued"),
    ));
    let service = state.database_service.clone();

    let cancelled_clone = cancelled.clone();
    let tracker_clone = progress_tracker.clone();
    let task = tokio::spawn(async move {
        let result = service
            .migrate_managed_to_external_with_control(
                &conn_config,
                cancelled_clone.clone(),
                tracker_clone.clone(),
            )
            .await;

        match result {
            Ok(_) => {
                let progress = tracker_clone.lock().await;
                progress.clone()
            }
            Err(_e) if cancelled_clone.load(Ordering::Relaxed) => {
                let mut progress = tracker_clone.lock().await;
                if progress.state != "cancelled" {
                    let stage = progress.current_stage.clone();
                    let diagnostics = progress.diagnostics.clone();
                    *progress = ExternalMigrationProgress::cancelled(&stage, diagnostics);
                }
                progress.clone()
            }
            Err(e) => {
                let mut progress = tracker_clone.lock().await;
                let stage = progress.current_stage.clone();
                let diagnostics = progress.diagnostics.clone();
                *progress = ExternalMigrationProgress::failed(&stage, e.to_string(), diagnostics);
                progress.clone()
            }
        }
    });

    migration_state.active = Some(crate::state::ExternalMigrationHandle { cancelled, task });
    migration_state.progress_tracker = progress_tracker;

    Ok("external migration started".to_string())
}

#[tauri::command]
pub async fn cancel_external_migration(state: State<'_, AppState>) -> Result<String, String> {
    let migration_state = state.external_migration_state.lock().await;
    if let Some(ref handle) = migration_state.active {
        handle.cancelled.store(true, Ordering::Relaxed);
        let mut progress = migration_state.progress_tracker.lock().await;
        progress.cancel_requested = true;
        Ok("external migration cancellation requested".to_string())
    } else {
        Err("No active external migration".to_string())
    }
}

async fn resolve_external_migration_handle(
    handle: crate::state::ExternalMigrationHandle,
) -> ExternalMigrationProgress {
    match handle.task.await {
        Ok(progress) => progress,
        Err(join_err) => {
            let msg = if join_err.is_panic() {
                let panic_msg = join_err
                    .into_panic()
                    .downcast::<String>()
                    .map(|s| *s)
                    .unwrap_or_else(|_| "external migration task panicked".to_string());
                format!("panic: {panic_msg}")
            } else {
                "external migration task cancelled".to_string()
            };
            ExternalMigrationProgress::failed("failed", msg, Vec::new())
        }
    }
}

#[tauri::command]
pub async fn get_external_migration_progress(
    state: State<'_, AppState>,
) -> Result<ExternalMigrationProgress, String> {
    let mut migration_state = state.external_migration_state.lock().await;
    if migration_state
        .active
        .as_ref()
        .map(|handle| handle.task.is_finished())
        .unwrap_or(false)
    {
        if let Some(handle) = migration_state.active.take() {
            let progress = resolve_external_migration_handle(handle).await;
            let mut tracker = migration_state.progress_tracker.lock().await;
            *tracker = progress;
        }
    }
    let tracker = migration_state.progress_tracker.lock().await;
    Ok(tracker.clone())
}

#[tauri::command]
pub async fn shutdown_database(state: State<'_, AppState>) -> Result<(), String> {
    let mut mgr = state.postgres_manager.lock().await;
    mgr.shutdown().await.map_err(|e| format!("{e}"))
}
