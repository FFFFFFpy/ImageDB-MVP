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
use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            if let Ok(resource_dir) = app.path().resource_dir() {
                let runtime_dir = resource_dir.join("postgres-runtime");
                if runtime_dir.join("bin").is_dir() {
                    std::env::set_var("IMAGEDB_POSTGRES_RUNTIME_DIR", runtime_dir);
                }
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_status,
            commands::probe_postgres,
            commands::probe_image_fingerprint,
            commands::probe_file_transaction,
            commands::run_all_probes,
            commands::get_database_status,
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
            commands::validate_source_directory,
            commands::start_scan,
            commands::cancel_scan,
            commands::get_scan_progress,
            commands::get_review_queue,
            commands::get_review_candidate_detail,
            commands::submit_review_decision,
            commands::skip_review_album,
            commands::get_review_progress,
            commands::generate_import_plan,
            commands::freeze_import_plan,
            commands::get_frozen_import_plan_summary,
            commands::set_import_plan_album_included,
            commands::set_import_plan_image_included,
            commands::move_import_plan_image,
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
        .run(tauri::generate_context!())
        .expect("failed to run ImageDB");
}
