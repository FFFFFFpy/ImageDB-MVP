#[tauri::command]
pub async fn get_app_status() -> Result<String, String> {
    Ok("Rust Core 已连接".to_string())
}
