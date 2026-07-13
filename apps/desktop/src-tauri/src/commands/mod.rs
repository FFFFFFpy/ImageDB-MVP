mod commit;
mod database;
mod diagnostics;
mod library;
mod probe;
mod recovery;
pub mod review;
pub mod scan;
mod settings_cmd;

pub use commit::*;
pub use database::*;
pub use diagnostics::*;
pub use library::*;
pub use probe::*;
pub use recovery::*;
pub use review::*;
pub use scan::*;
pub use settings_cmd::*;

#[tauri::command]
pub async fn get_app_status() -> Result<String, String> {
    Ok("Rust Core 已连接".to_string())
}
