mod commands;
mod domain;
mod error;
pub mod infrastructure;
mod repositories;
mod services;
mod state;

use std::path::PathBuf;

pub fn run() {
    let app_data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ImageDB");

    std::fs::create_dir_all(&app_data_dir).ok();

    infrastructure::logging::init_logging(&app_data_dir);

    match infrastructure::single_instance::SingleInstanceLock::acquire(&app_data_dir) {
        Ok(_lock) => {
            std::mem::forget(_lock);
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

    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::get_app_status,
            commands::probe_postgres,
            commands::probe_image_fingerprint,
            commands::probe_file_transaction,
            commands::run_all_probes,
            commands::get_database_status,
            commands::initialize_managed_database,
            commands::test_external_connection,
            commands::initialize_external_database,
            commands::shutdown_database,
            commands::get_settings,
            commands::update_settings,
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
            commands::get_latest_completed_import_run,
            commands::get_image_preview,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run ImageDB");
}
