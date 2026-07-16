use crate::domain::{
    ConnectionConfig, DatabaseState, ExternalCheckResult, ExternalMigrationProgress,
    ExternalMigrationResult, TlsMode,
};
use crate::infrastructure::postgres::{migration::DatabaseResetSummary, MigrationRunner};
use crate::repositories::import_repository::{
    DatabaseInfoDashboard, DatabaseInfoDatabaseSection, ImportRepository,
};
use crate::state::{
    AppState, CriticalOperationGuardSnapshot, CriticalOperationKind, CriticalTaskKind,
};
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CriticalOperationGuardStatusDto {
    pub is_blocked: bool,
    pub blocking_reason: Option<String>,
    pub active_task_kinds: Vec<String>,
    pub active_operation: Option<String>,
}

impl From<CriticalOperationGuardSnapshot> for CriticalOperationGuardStatusDto {
    fn from(snapshot: CriticalOperationGuardSnapshot) -> Self {
        Self {
            is_blocked: snapshot.is_blocked,
            blocking_reason: snapshot.blocking_reason,
            active_task_kinds: snapshot.active_task_kinds,
            active_operation: snapshot.active_operation,
        }
    }
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
pub async fn get_critical_operation_guard_status(
    state: State<'_, AppState>,
) -> Result<CriticalOperationGuardStatusDto, String> {
    Ok(state.critical_operation_guard.snapshot().into())
}

#[tauri::command]
pub async fn get_database_info_dashboard(
    state: State<'_, AppState>,
) -> Result<DatabaseInfoDashboard, String> {
    let database_state = state.database_service.get_state().await;
    let database = DatabaseInfoDatabaseSection {
        mode: database_state.mode.as_ref().map(ToString::to_string),
        status: database_state.status.to_string(),
        pgvector_available: database_state.pgvector_available,
        migration_version: database_state.migration_version.clone(),
    };

    let connect_result = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await
    };

    let (client, handle) = match connect_result {
        Ok(pair) => pair,
        Err(_) => return Ok(ImportRepository::empty_database_info_dashboard(database)),
    };

    let result = ImportRepository::get_database_info_dashboard(&client, database)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

#[tauri::command]
pub async fn initialize_managed_database(
    state: State<'_, AppState>,
) -> Result<DatabaseState, String> {
    initialize_managed_database_for_state(&state).await
}

pub(crate) async fn initialize_managed_database_for_state(
    state: &AppState,
) -> Result<DatabaseState, String> {
    let _operation = state
        .critical_operation_guard
        .begin_operation(CriticalOperationKind::InitializeManagedDatabase)?;
    let service = &state.database_service;
    let result = service
        .initialize_managed()
        .await
        .map_err(|e| format!("{e}"))?;
    *state.library_fingerprint_index.write().await = None;
    Ok(result)
}

#[tauri::command]
pub async fn switch_to_managed_database(
    state: State<'_, AppState>,
) -> Result<DatabaseState, String> {
    switch_to_managed_database_for_state(&state).await
}

pub(crate) async fn switch_to_managed_database_for_state(
    state: &AppState,
) -> Result<DatabaseState, String> {
    let _operation = state
        .critical_operation_guard
        .begin_operation(CriticalOperationKind::SwitchToManagedDatabase)?;
    let service = &state.database_service;
    let result = service
        .switch_to_managed()
        .await
        .map_err(|e| format!("{e}"))?;
    *state.library_fingerprint_index.write().await = None;
    Ok(result)
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
    initialize_external_database_for_state(&state, config).await
}

pub(crate) async fn initialize_external_database_for_state(
    state: &AppState,
    config: ExternalConnectionDto,
) -> Result<DatabaseState, String> {
    let _operation = state
        .critical_operation_guard
        .begin_operation(CriticalOperationKind::InitializeExternalDatabase)?;
    let service = &state.database_service;
    let conn_config: ConnectionConfig = config.into();
    let result = service
        .initialize_external(&conn_config)
        .await
        .map_err(|e| format!("{e}"))?;
    *state.library_fingerprint_index.write().await = None;
    Ok(result)
}

#[tauri::command]
pub async fn migrate_managed_to_external_database(
    state: State<'_, AppState>,
    config: ExternalConnectionDto,
) -> Result<ExternalMigrationResult, String> {
    migrate_managed_to_external_database_for_state(&state, config).await
}

pub(crate) async fn migrate_managed_to_external_database_for_state(
    state: &AppState,
    config: ExternalConnectionDto,
) -> Result<ExternalMigrationResult, String> {
    let _migration = state
        .critical_operation_guard
        .begin_task(CriticalTaskKind::ExternalMigration)?;
    let service = &state.database_service;
    let conn_config: ConnectionConfig = config.into();
    let result = service
        .migrate_managed_to_external(&conn_config)
        .await
        .map_err(|e| format!("{e}"))?;
    *state.library_fingerprint_index.write().await = None;
    Ok(result)
}

#[tauri::command]
pub async fn start_managed_to_external_migration(
    state: State<'_, AppState>,
    config: ExternalConnectionDto,
) -> Result<String, String> {
    let migration_lease = state
        .critical_operation_guard
        .begin_task(CriticalTaskKind::ExternalMigration)?;
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
    let library_fingerprint_index = state.library_fingerprint_index.clone();

    let cancelled_clone = cancelled.clone();
    let tracker_clone = progress_tracker.clone();
    let task = tokio::spawn(async move {
        let _migration_lease = migration_lease;
        let result = service
            .migrate_managed_to_external_with_control(
                &conn_config,
                cancelled_clone.clone(),
                tracker_clone.clone(),
            )
            .await;

        match result {
            Ok(_) => {
                *library_fingerprint_index.write().await = None;
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
    shutdown_database_for_state(&state).await
}

const DATABASE_RESET_CONFIRMATION: &str = "从零开始";

#[tauri::command]
pub async fn reset_database_history(
    state: State<'_, AppState>,
    confirmation: String,
) -> Result<DatabaseResetSummary, String> {
    reset_database_history_for_state(&state, &confirmation).await
}

pub(crate) async fn reset_database_history_for_state(
    state: &AppState,
    confirmation: &str,
) -> Result<DatabaseResetSummary, String> {
    if confirmation != DATABASE_RESET_CONFIRMATION {
        return Err(format!(
            "confirmation text must exactly match '{DATABASE_RESET_CONFIRMATION}'"
        ));
    }

    let _operation = state
        .critical_operation_guard
        .begin_operation(CriticalOperationKind::ResetDatabaseHistory)?;
    let (mut client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|error| format!("{error}"))?
    };
    let result = MigrationRunner::reset_to_empty(&mut client)
        .await
        .map_err(|error| format!("{error}"));
    handle.abort();
    let summary = result?;

    *state.library_fingerprint_index.write().await = None;
    *state.scan_state.lock().await = crate::state::ScanState::default();
    *state.commit_state.lock().await = crate::state::CommitState::default();
    *state.external_migration_state.lock().await = crate::state::ExternalMigrationState::default();

    Ok(summary)
}

pub(crate) async fn shutdown_database_for_state(state: &AppState) -> Result<(), String> {
    let _operation = state
        .critical_operation_guard
        .begin_operation(CriticalOperationKind::ShutdownDatabase)?;
    let mut mgr = state.postgres_manager.lock().await;
    mgr.shutdown().await.map_err(|e| format!("{e}"))?;
    drop(mgr);
    *state.library_fingerprint_index.write().await = None;
    Ok(())
}

#[cfg(test)]
mod critical_lifecycle_tests {
    use super::*;
    use tempfile::TempDir;

    fn app_state(temp: &TempDir) -> AppState {
        AppState::new(temp.path(), temp.path().join("fixtures")).unwrap()
    }

    fn external_config() -> ExternalConnectionDto {
        ExternalConnectionDto {
            host: "127.0.0.1".to_string(),
            port: 5432,
            database: "imagedb".to_string(),
            username: "imagedb".to_string(),
            password: None,
            tls_mode: Some("disable".to_string()),
            ca_cert_path: None,
            client_cert_path: None,
            client_key_path: None,
            connect_timeout_secs: Some(1),
            query_timeout_secs: Some(1),
            profile_name: Some("blocked-test".to_string()),
        }
    }

    async fn database_mode(state: &AppState) -> Option<String> {
        state.settings.lock().await.get().database_mode.clone()
    }

    #[tokio::test]
    async fn active_commit_rejects_lifecycle_commands_without_changing_database_mode() {
        let temp = TempDir::new().unwrap();
        let state = app_state(&temp);
        state
            .settings
            .lock()
            .await
            .set_database_mode("external")
            .unwrap();
        let commit = state
            .critical_operation_guard
            .begin_task(CriticalTaskKind::Commit)
            .unwrap();

        let shutdown_error = shutdown_database_for_state(&state).await.unwrap_err();
        assert_eq!(
            shutdown_error,
            "cannot stop database while import commit is running"
        );
        let switch_error = switch_to_managed_database_for_state(&state)
            .await
            .unwrap_err();
        assert_eq!(
            switch_error,
            "cannot switch database while import commit is running"
        );
        let initialize_managed_error = initialize_managed_database_for_state(&state)
            .await
            .unwrap_err();
        assert_eq!(
            initialize_managed_error,
            "cannot initialize managed database while import commit is running"
        );
        let initialize_external_error =
            initialize_external_database_for_state(&state, external_config())
                .await
                .unwrap_err();
        assert_eq!(
            initialize_external_error,
            "cannot initialize external database while import commit is running"
        );
        let reset_error = reset_database_history_for_state(&state, DATABASE_RESET_CONFIRMATION)
            .await
            .unwrap_err();
        assert_eq!(
            reset_error,
            "cannot reset database history while import commit is running"
        );

        assert_eq!(database_mode(&state).await.as_deref(), Some("external"));
        assert_eq!(
            state.critical_operation_guard.snapshot().active_task_kinds,
            vec!["commit"]
        );

        drop(commit);
        shutdown_database_for_state(&state)
            .await
            .expect("shutdown must work after commit completion");
    }

    #[tokio::test]
    async fn database_reset_requires_exact_confirmation_before_connecting() {
        let temp = TempDir::new().unwrap();
        let state = app_state(&temp);

        let error = reset_database_history_for_state(&state, "重新开始")
            .await
            .unwrap_err();

        assert_eq!(error, "confirmation text must exactly match '从零开始'");
        assert_eq!(
            state.critical_operation_guard.snapshot().active_operation,
            None
        );
    }

    #[tokio::test]
    async fn active_scan_rejects_database_switch_and_preserves_task() {
        let temp = TempDir::new().unwrap();
        let state = app_state(&temp);
        let scan = state
            .critical_operation_guard
            .begin_task(CriticalTaskKind::Scan)
            .unwrap();

        let error = switch_to_managed_database_for_state(&state)
            .await
            .unwrap_err();
        assert_eq!(error, "cannot switch database while import scan is running");
        assert_eq!(
            state.critical_operation_guard.snapshot().active_task_kinds,
            vec!["scan"]
        );
        drop(scan);
    }

    #[tokio::test]
    async fn active_migration_rejects_switch_and_shutdown() {
        let temp = TempDir::new().unwrap();
        let state = app_state(&temp);
        let migration = state
            .critical_operation_guard
            .begin_task(CriticalTaskKind::ExternalMigration)
            .unwrap();

        let switch_error = switch_to_managed_database_for_state(&state)
            .await
            .unwrap_err();
        assert_eq!(
            switch_error,
            "cannot switch database while external database migration is running"
        );
        let shutdown_error = shutdown_database_for_state(&state).await.unwrap_err();
        assert_eq!(
            shutdown_error,
            "cannot stop database while external database migration is running"
        );
        assert_eq!(
            state.critical_operation_guard.snapshot().active_task_kinds,
            vec!["external_migration"]
        );
        drop(migration);
    }

    #[tokio::test]
    async fn active_recovery_rejects_database_shutdown() {
        let temp = TempDir::new().unwrap();
        let state = app_state(&temp);
        let recovery = state
            .critical_operation_guard
            .begin_task(CriticalTaskKind::Recovery)
            .unwrap();

        let error = shutdown_database_for_state(&state).await.unwrap_err();
        assert_eq!(
            error,
            "cannot stop database while transaction recovery is running"
        );
        assert_eq!(
            state.critical_operation_guard.snapshot().active_task_kinds,
            vec!["recovery"]
        );
        drop(recovery);
    }

    #[test]
    fn guard_status_dto_exposes_backend_blocking_reason_and_task_kinds() {
        let guard = crate::state::CriticalOperationGuard::default();
        let recovery = guard.begin_task(CriticalTaskKind::Recovery).unwrap();

        let status = CriticalOperationGuardStatusDto::from(guard.snapshot());
        assert!(status.is_blocked);
        assert_eq!(status.active_task_kinds, vec!["recovery"]);
        assert_eq!(status.active_operation, None);
        assert!(status
            .blocking_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("transaction recovery")));

        drop(recovery);
        assert!(!CriticalOperationGuardStatusDto::from(guard.snapshot()).is_blocked);
    }
}
