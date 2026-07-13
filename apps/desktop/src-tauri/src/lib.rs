mod commands;
mod domain;
mod error;
pub mod infrastructure;
mod repositories;
mod services;
mod state;

#[cfg(any(feature = "fail-injection", feature = "real-db-tests"))]
pub mod tests;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tauri::Manager;

const INSTALL_GATE_MANAGED_PROBE_ENV: &str = "IMAGEDB_INSTALL_GATE_MANAGED_BOOTSTRAP";
const INSTALL_GATE_LAUNCH_SMOKE_ENV: &str = "IMAGEDB_INSTALL_GATE_LAUNCH_SMOKE";
const MANAGED_POSTGRES_SHUTDOWN_TIMEOUT_SECS: u64 = 20;
const SHUTDOWN_IDLE: u8 = 0;
const SHUTDOWN_IN_PROGRESS: u8 = 1;
const SHUTDOWN_COMPLETE: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownStart {
    Start,
    InProgress,
    Complete,
}

#[derive(Default)]
struct GracefulShutdownCoordinator {
    state: AtomicU8,
}

impl GracefulShutdownCoordinator {
    fn begin(&self) -> ShutdownStart {
        match self.state.compare_exchange(
            SHUTDOWN_IDLE,
            SHUTDOWN_IN_PROGRESS,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => ShutdownStart::Start,
            Err(SHUTDOWN_IN_PROGRESS) => ShutdownStart::InProgress,
            Err(SHUTDOWN_COMPLETE) => ShutdownStart::Complete,
            Err(_) => ShutdownStart::InProgress,
        }
    }

    fn finish(&self, succeeded: bool) {
        self.state.store(
            if succeeded {
                SHUTDOWN_COMPLETE
            } else {
                SHUTDOWN_IDLE
            },
            Ordering::Release,
        );
    }

    fn is_complete(&self) -> bool {
        self.state.load(Ordering::Acquire) == SHUTDOWN_COMPLETE
    }
}

pub fn enable_install_gate_managed_probe() {
    std::env::set_var(INSTALL_GATE_MANAGED_PROBE_ENV, "1");
}

pub fn enable_install_gate_launch_smoke() {
    std::env::set_var(INSTALL_GATE_LAUNCH_SMOKE_ENV, "1");
}

fn install_gate_managed_probe_enabled() -> bool {
    std::env::var(INSTALL_GATE_MANAGED_PROBE_ENV).as_deref() == Ok("1")
}

fn install_gate_launch_smoke_enabled() -> bool {
    std::env::var(INSTALL_GATE_LAUNCH_SMOKE_ENV).as_deref() == Ok("1")
}

fn configure_postgres_runtime(runtime_dir: PathBuf) {
    #[cfg(feature = "bundled-runtime-required")]
    {
        std::env::remove_var("IMAGEDB_POSTGRES_RUNTIME_DIR");
        std::env::remove_var("IMAGEDB_POSTGRES_BIN");
        std::env::set_var("IMAGEDB_POSTGRES_RUNTIME_REQUIRED", "1");
        std::env::set_var("IMAGEDB_POSTGRES_RUNTIME_DIR", runtime_dir);
    }

    #[cfg(not(feature = "bundled-runtime-required"))]
    if runtime_dir.join("bin").is_dir() {
        std::env::set_var("IMAGEDB_POSTGRES_RUNTIME_DIR", runtime_dir);
    }
}

async fn shutdown_postgres_manager_with_timeout(
    postgres_manager: Arc<tokio::sync::Mutex<infrastructure::postgres::PostgresManager>>,
    max_wait: std::time::Duration,
) -> Result<(), String> {
    match tokio::time::timeout(max_wait, async move {
        postgres_manager
            .lock()
            .await
            .shutdown()
            .await
            .map_err(|error| error.to_string())
    })
    .await
    {
        Ok(result) => result,
        Err(_) => Err(format!(
            "managed PostgreSQL shutdown timed out after {} seconds",
            max_wait.as_secs_f64()
        )),
    }
}

#[cfg(any(feature = "bundled-runtime-required", test))]
fn combine_probe_and_shutdown_results<T>(
    probe_result: Result<T, String>,
    shutdown_result: Result<(), String>,
) -> Result<T, String> {
    match (probe_result, shutdown_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(probe_error), Ok(())) => Err(probe_error),
        (Ok(_), Err(shutdown_error)) => Err(format!(
            "managed PostgreSQL probe completed, but shutdown failed: {shutdown_error}"
        )),
        (Err(probe_error), Err(shutdown_error)) => Err(format!(
            "managed PostgreSQL shutdown failed: {shutdown_error}; original probe error: {probe_error}"
        )),
    }
}

/// Internal installer probe body. The normal Tauri setup must resolve the
/// resource directory and configure strict runtime policy before calling it.
/// The body refuses to touch the user's normal app-data location: the
/// installation gate must provide an isolated `IMAGEDB_APP_DATA_DIR`.
fn run_install_gate_managed_probe_after_setup(resource_dir: PathBuf) -> Result<(), String> {
    #[cfg(not(feature = "bundled-runtime-required"))]
    {
        let _ = resource_dir;
        Err("install probe requires the bundled-runtime-required build feature".to_string())
    }

    #[cfg(feature = "bundled-runtime-required")]
    {
        let expected_runtime_dir = resource_dir.join("postgres-runtime");
        let configured_runtime_dir = std::env::var_os("IMAGEDB_POSTGRES_RUNTIME_DIR")
            .map(PathBuf::from)
            .ok_or_else(|| {
                "Tauri setup did not configure IMAGEDB_POSTGRES_RUNTIME_DIR".to_string()
            })?;
        if configured_runtime_dir != expected_runtime_dir {
            return Err(format!(
                "Tauri setup runtime mismatch: expected '{}', got '{}'",
                expected_runtime_dir.display(),
                configured_runtime_dir.display()
            ));
        }
        let runtime_required =
            std::env::var("IMAGEDB_POSTGRES_RUNTIME_REQUIRED").unwrap_or_default();
        if runtime_required != "1" {
            return Err(format!(
                "Tauri setup did not enable strict bundled runtime policy: got '{runtime_required}'"
            ));
        }
        if std::env::var_os("IMAGEDB_POSTGRES_BIN").is_some() {
            return Err(
                "Tauri setup did not clear the inherited IMAGEDB_POSTGRES_BIN override".to_string(),
            );
        }

        let app_data_dir = std::env::var("IMAGEDB_APP_DATA_DIR")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| "install probe requires an isolated IMAGEDB_APP_DATA_DIR".to_string())?;
        std::fs::create_dir_all(&app_data_dir)
            .map_err(|error| format!("cannot create probe app-data directory: {error}"))?;

        let runtime = tokio::runtime::Runtime::new()
            .map_err(|error| format!("cannot start install probe runtime: {error}"))?;
        runtime.block_on(async move {
            let fixture_dir = app_data_dir.join("fixtures");
            let app_state = state::AppState::new(&app_data_dir, fixture_dir)
                .map_err(|error| error.to_string())?;

            let probe_result: Result<(String, String, String), String> = async {
                let database = app_state
                    .database_service
                    .initialize_managed()
                    .await
                    .map_err(|error| error.to_string())?;
                if database.status != domain::DatabaseStatus::Connected {
                    return Err(format!(
                        "managed bootstrap did not connect: {} ({:?})",
                        database.status, database.diagnostics
                    ));
                }
                if !database.pgvector_available {
                    return Err(format!(
                        "managed bootstrap did not load pgvector: {:?}",
                        database.diagnostics
                    ));
                }
                let expected_migration =
                    infrastructure::postgres::MigrationRunner::latest_version().to_string();
                if database.migration_version.as_deref() != Some(expected_migration.as_str()) {
                    return Err(format!(
                        "managed bootstrap migration mismatch: expected {expected_migration}, got {:?}",
                        database.migration_version
                    ));
                }

                let manager = app_state.postgres_manager.lock().await;
                let (client, handle) = manager.connect().await.map_err(|error| error.to_string())?;
                let server_version: String = client
                    .query_one("SHOW server_version", &[])
                    .await
                    .map_err(|error| format!("cannot query server_version: {error}"))?
                    .get(0);
                let vector_version: String = client
                    .query_one(
                        "SELECT extversion FROM pg_extension WHERE extname = 'vector'",
                        &[],
                    )
                    .await
                    .map_err(|error| format!("cannot query pgvector version: {error}"))?
                    .get(0);
                drop(client);
                handle.abort();
                drop(manager);

                if server_version != "18.4" {
                    return Err(format!(
                        "installed PostgreSQL version mismatch: expected 18.4, got {server_version}"
                    ));
                }
                if vector_version != "0.8.3" {
                    return Err(format!(
                        "installed pgvector version mismatch: expected 0.8.3, got {vector_version}"
                    ));
                }
                Ok((server_version, vector_version, expected_migration))
            }
            .await;

            let shutdown_result = shutdown_postgres_manager_with_timeout(
                app_state.postgres_manager.clone(),
                std::time::Duration::from_secs(MANAGED_POSTGRES_SHUTDOWN_TIMEOUT_SECS),
            )
            .await;
            let (server_version, vector_version, migration_version) =
                combine_probe_and_shutdown_results(probe_result, shutdown_result)?;
            println!(
                "IMAGEDB_INSTALL_PROBE_JSON={}",
                serde_json::json!({
                    "postgres": server_version,
                    "pgvector": vector_version,
                    "migration": migration_version,
                    "resource_dir": resource_dir.display().to_string(),
                    "runtime_dir": configured_runtime_dir.display().to_string(),
                    "runtime_required": runtime_required,
                    "postgres_bin_cleared": true,
                    "status": "passed"
                })
            );
            Ok(())
        })
    }
}

fn run_install_gate_managed_probe_and_exit(resource_dir: PathBuf) -> ! {
    // Tauri setup may execute within its async runtime. Run the synchronous
    // probe wrapper (which owns a Tokio runtime) on a fresh OS thread to avoid
    // nesting runtimes while still using the exact setup-resolved path.
    let result = match std::thread::Builder::new()
        .name("imagedb-install-gate-probe".to_string())
        .spawn(move || run_install_gate_managed_probe_after_setup(resource_dir))
    {
        Ok(handle) => match handle.join() {
            Ok(result) => result,
            Err(_) => Err("install probe thread panicked".to_string()),
        },
        Err(error) => Err(format!("cannot start install probe thread: {error}")),
    };

    match result {
        Ok(()) => {
            use std::io::Write;
            let _ = std::io::stdout().flush();
            std::process::exit(0);
        }
        Err(error) => {
            eprintln!("ImageDB install probe failed: {error}");
            use std::io::Write;
            let _ = std::io::stderr().flush();
            std::process::exit(2);
        }
    }
}

fn shutdown_managed_postgres_before_exit(app_handle: &tauri::AppHandle) -> Result<(), String> {
    let postgres_manager = app_handle
        .state::<state::AppState>()
        .postgres_manager
        .clone();
    let shutdown_thread = std::thread::Builder::new()
        .name("imagedb-graceful-postgres-shutdown".to_string())
        .spawn(move || {
            let runtime = tokio::runtime::Runtime::new()
                .map_err(|error| format!("cannot start PostgreSQL shutdown runtime: {error}"))?;
            runtime.block_on(shutdown_postgres_manager_with_timeout(
                postgres_manager,
                std::time::Duration::from_secs(MANAGED_POSTGRES_SHUTDOWN_TIMEOUT_SECS),
            ))
        })
        .map_err(|error| format!("cannot start PostgreSQL shutdown thread: {error}"))?;

    shutdown_thread
        .join()
        .map_err(|_| "PostgreSQL shutdown thread panicked".to_string())?
}

enum GracefulShutdownOutcome {
    Succeeded,
    InProgress,
    Failed(String),
}

enum BackgroundShutdownStart {
    Started,
    InProgress,
    Complete,
    Failed,
}

fn attempt_graceful_shutdown(
    app_handle: &tauri::AppHandle,
    coordinator: &GracefulShutdownCoordinator,
) -> GracefulShutdownOutcome {
    match coordinator.begin() {
        ShutdownStart::Complete => GracefulShutdownOutcome::Succeeded,
        ShutdownStart::InProgress => GracefulShutdownOutcome::InProgress,
        ShutdownStart::Start => {
            let result = shutdown_managed_postgres_before_exit(app_handle);
            coordinator.finish(result.is_ok());
            match result {
                Ok(()) => GracefulShutdownOutcome::Succeeded,
                Err(error) => GracefulShutdownOutcome::Failed(error),
            }
        }
    }
}

fn report_shutdown_failure(app_handle: &tauri::AppHandle, window_label: Option<&str>, error: &str) {
    tracing::error!(%error, "graceful application shutdown failed; keeping the UI open for retry");
    eprintln!("ImageDB graceful shutdown failed; close the window to retry: {error}");
    let user_message = format!(
        "ImageDB 无法安全关闭，因为本地 PostgreSQL 未能停止。窗口已保留，请稍后再次关闭。\n\n{error}"
    );
    let encoded_message = serde_json::to_string(&user_message)
        .unwrap_or_else(|_| "\"ImageDB 无法安全关闭，请稍后重试。\"".to_string());
    let alert_script = format!("window.alert({encoded_message});");

    let restore_and_notify = |window: &tauri::WebviewWindow| {
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.set_title("ImageDB - 无法安全关闭，请重试");
        let _ = window.eval(alert_script.clone());
    };

    if let Some(label) = window_label {
        if let Some(window) = app_handle.get_webview_window(label) {
            restore_and_notify(&window);
            return;
        }
    }
    for window in app_handle.webview_windows().values() {
        restore_and_notify(window);
    }
}

fn start_window_close_shutdown(
    app_handle: &tauri::AppHandle,
    coordinator: &Arc<GracefulShutdownCoordinator>,
    window_label: String,
) -> BackgroundShutdownStart {
    match coordinator.begin() {
        ShutdownStart::Complete => BackgroundShutdownStart::Complete,
        ShutdownStart::InProgress => BackgroundShutdownStart::InProgress,
        ShutdownStart::Start => {
            let worker_app_handle = app_handle.clone();
            let worker_coordinator = coordinator.clone();
            let worker_label = window_label.clone();
            match std::thread::Builder::new()
                .name("imagedb-window-close-shutdown".to_string())
                .spawn(move || {
                    let result = shutdown_managed_postgres_before_exit(&worker_app_handle);
                    worker_coordinator.finish(result.is_ok());
                    match result {
                        Ok(()) => worker_app_handle.exit(0),
                        Err(error) => {
                            report_shutdown_failure(&worker_app_handle, Some(&worker_label), &error)
                        }
                    }
                }) {
                Ok(_) => BackgroundShutdownStart::Started,
                Err(error) => {
                    coordinator.finish(false);
                    report_shutdown_failure(
                        app_handle,
                        Some(&window_label),
                        &format!("cannot start graceful shutdown worker: {error}"),
                    );
                    BackgroundShutdownStart::Failed
                }
            }
        }
    }
}

pub fn run() {
    let app = tauri::Builder::default()
        .setup(|app| {
            // A bundled build must use the PostgreSQL runtime shipped with that
            // exact ImageDB build. Falling back to a system PostgreSQL can
            // initialize app-owned data with an incompatible major version
            // and can silently omit pgvector. Development builds keep their
            // explicit/runtime search overrides for local and real-DB tests.
            let resource_dir = app.path().resource_dir().map_err(|error| {
                std::io::Error::other(format!(
                    "failed to resolve Tauri resource directory: {error}"
                ))
            })?;
            configure_postgres_runtime(resource_dir.join("postgres-runtime"));

            if install_gate_managed_probe_enabled() {
                run_install_gate_managed_probe_and_exit(resource_dir);
            }

            let app_data_dir = std::env::var("IMAGEDB_APP_DATA_DIR")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    dirs::data_local_dir()
                        .unwrap_or_else(|| PathBuf::from("."))
                        .join("ImageDB")
                });

            std::fs::create_dir_all(&app_data_dir).ok();
            infrastructure::logging::init_logging(&app_data_dir);

            match infrastructure::single_instance::SingleInstanceLock::acquire(&app_data_dir) {
                Ok(lock) => {
                    std::mem::forget(lock);
                }
                Err(e) => {
                    eprintln!("ImageDB: {e}");
                    std::process::exit(1);
                }
            }

            let fixture_dir = std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("fixtures");

            let app_state = state::AppState::new(&app_data_dir, fixture_dir)
                .expect("failed to initialize application state");
            app.manage(app_state);

            if install_gate_launch_smoke_enabled() {
                let app_handle = app.handle().clone();
                std::thread::Builder::new()
                    .name("imagedb-install-gate-launch-smoke".to_string())
                    .spawn(move || {
                        std::thread::sleep(std::time::Duration::from_secs(5));
                        match app_handle.get_webview_window("main") {
                            Some(window) => {
                                if let Err(error) = window.close() {
                                    eprintln!(
                                        "ImageDB install-gate could not request main-window close: {error}"
                                    );
                                }
                            }
                            None => {
                                eprintln!(
                                    "ImageDB install-gate could not find the main window to close"
                                );
                            }
                        }
                    })
                    .map_err(|error| {
                        std::io::Error::other(format!(
                            "cannot schedule install-gate launch smoke exit: {error}"
                        ))
                    })?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_status,
            commands::probe_postgres,
            commands::probe_image_fingerprint,
            commands::probe_file_transaction,
            commands::run_all_probes,
            commands::get_database_status,
            commands::get_database_info_dashboard,
            commands::get_library_albums,
            commands::get_library_images,
            commands::initialize_managed_database,
            commands::switch_to_managed_database,
            commands::test_external_connection,
            commands::initialize_external_database,
            commands::migrate_managed_to_external_database,
            commands::start_managed_to_external_migration,
            commands::cancel_external_migration,
            commands::get_external_migration_progress,
            commands::shutdown_database,
            commands::export_diagnostics,
            commands::get_settings,
            commands::update_settings,
            commands::probe_storage_capabilities,
            commands::select_source_directory,
            commands::validate_source_directory,
            commands::start_scan,
            commands::cancel_scan,
            commands::get_scan_progress,
            commands::get_import_runs_dashboard,
            commands::get_import_run_albums,
            commands::resume_import_run,
            commands::retry_import_album,
            commands::abandon_import_run,
            commands::get_review_queue,
            commands::get_review_candidate_detail,
            commands::submit_review_decision,
            commands::skip_review_album,
            commands::get_review_progress,
            commands::generate_import_plan,
            commands::freeze_import_plan,
            commands::get_frozen_import_plan_summary,
            commands::withdraw_frozen_import_plan,
            commands::set_import_plan_album_included,
            commands::set_import_plan_image_included,
            commands::get_latest_completed_import_run,
            commands::get_latest_reviewable_import_run,
            commands::get_latest_committable_import_run,
            commands::get_image_preview,
            commands::get_import_plan_image_preview,
            commands::start_import_commit,
            commands::cancel_import_commit,
            commands::get_commit_progress,
            commands::scan_recoverable_transactions,
            commands::recover_transaction,
            commands::reverify_transaction,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build ImageDB");

    let shutdown_coordinator = Arc::new(GracefulShutdownCoordinator::default());
    app.run(move |app_handle, event| match event {
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::CloseRequested { api, .. },
            ..
        } if app_handle.webview_windows().len() <= 1 => {
            // Keep the last real window alive until PostgreSQL has stopped.
            // This avoids the Destroyed -> failed ExitRequested path leaving
            // an invisible background process with no way to retry shutdown.
            api.prevent_close();
            match start_window_close_shutdown(app_handle, &shutdown_coordinator, label) {
                BackgroundShutdownStart::Complete => app_handle.exit(0),
                BackgroundShutdownStart::InProgress => {
                    tracing::warn!("graceful shutdown is already in progress");
                }
                BackgroundShutdownStart::Started | BackgroundShutdownStart::Failed => {}
            }
        }
        tauri::RunEvent::ExitRequested { code, api, .. } if !shutdown_coordinator.is_complete() => {
            api.prevent_exit();
            match attempt_graceful_shutdown(app_handle, &shutdown_coordinator) {
                GracefulShutdownOutcome::Succeeded => {
                    // Restart requests cannot be prevented by Tauri and
                    // will continue after this bounded callback returns.
                    if code != Some(tauri::RESTART_EXIT_CODE) {
                        app_handle.exit(code.unwrap_or(0));
                    }
                }
                GracefulShutdownOutcome::InProgress => {
                    tracing::warn!("graceful shutdown is already in progress");
                }
                GracefulShutdownOutcome::Failed(error) => {
                    report_shutdown_failure(app_handle, None, &error);
                }
            }
        }
        _ => {}
    });
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    #[test]
    fn shutdown_coordinator_blocks_duplicates_and_allows_retry_after_failure() {
        let coordinator = GracefulShutdownCoordinator::default();
        assert_eq!(coordinator.begin(), ShutdownStart::Start);
        assert_eq!(coordinator.begin(), ShutdownStart::InProgress);

        coordinator.finish(false);
        assert_eq!(coordinator.begin(), ShutdownStart::Start);

        coordinator.finish(true);
        assert_eq!(coordinator.begin(), ShutdownStart::Complete);
        assert!(coordinator.is_complete());
    }

    #[tokio::test]
    async fn managed_postgres_shutdown_timeout_includes_manager_lock_wait() {
        let temp = tempfile::TempDir::new().unwrap();
        let manager = Arc::new(tokio::sync::Mutex::new(
            infrastructure::postgres::PostgresManager::new(temp.path()),
        ));
        let guard = manager.lock().await;

        let error = shutdown_postgres_manager_with_timeout(
            manager.clone(),
            std::time::Duration::from_millis(20),
        )
        .await
        .expect_err("held manager mutex must be covered by the total shutdown timeout");
        assert!(error.contains("timed out"));
        drop(guard);
    }

    #[test]
    fn probe_error_preserves_shutdown_failure_context() {
        let combined = combine_probe_and_shutdown_results::<()>(
            Err("primary probe failure".to_string()),
            Err("shutdown failure".to_string()),
        )
        .expect_err("both failures must remain visible");
        assert!(combined.contains("primary probe failure"));
        assert!(combined.contains("shutdown failure"));

        let shutdown_only = combine_probe_and_shutdown_results(Ok(()), Err("locked".to_string()))
            .expect_err("shutdown failure must override a successful probe");
        assert!(shutdown_only.contains("shutdown failed"));
        assert!(shutdown_only.contains("locked"));
    }
}
