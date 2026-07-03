mod database;
mod probe;
pub mod review;
pub mod scan;
mod settings_cmd;

pub use database::*;
pub use probe::*;
pub use review::*;
pub use scan::*;
pub use settings_cmd::*;

#[tauri::command]
pub async fn get_app_status() -> Result<String, String> {
    Ok("Rust Core 已连接".to_string())
}
