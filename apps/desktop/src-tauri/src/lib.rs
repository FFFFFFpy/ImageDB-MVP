mod commands;
mod state;

pub fn run() {
    tauri::Builder::default()
        .manage(state::AppState)
        .invoke_handler(tauri::generate_handler![commands::get_app_status])
        .run(tauri::generate_context!())
        .expect("failed to run ImageDB");
}
