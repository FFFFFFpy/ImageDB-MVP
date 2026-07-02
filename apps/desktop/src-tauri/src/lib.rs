mod commands;
mod domain;
mod error;
pub mod infrastructure;
mod state;

use std::path::PathBuf;

pub fn run() {
    let app_data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ImageDB");

    let fixture_dir = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("fixtures");

    let app_state = state::AppState::new(&app_data_dir, fixture_dir);

    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::get_app_status,
            commands::probe_postgres,
            commands::probe_image_fingerprint,
            commands::probe_file_transaction,
            commands::run_all_probes,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run ImageDB");
}
