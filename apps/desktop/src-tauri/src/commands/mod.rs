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

#[derive(Debug, Clone, serde::Serialize)]
pub struct BuildInfo {
    pub app_version: String,
    pub git_commit: String,
    pub git_dirty: Option<bool>,
}

#[tauri::command]
pub fn get_build_info() -> BuildInfo {
    BuildInfo {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        git_commit: option_env!("IMAGEDB_GIT_COMMIT")
            .unwrap_or("unknown")
            .to_string(),
        git_dirty: option_env!("IMAGEDB_GIT_DIRTY").and_then(|value| value.parse().ok()),
    }
}

#[tauri::command]
pub async fn get_app_status() -> Result<String, String> {
    Ok("Rust Core 已连接".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_info_identifies_the_compiled_backend() {
        let info = get_build_info();
        assert_eq!(info.app_version, env!("CARGO_PKG_VERSION"));
        assert!(!info.git_commit.trim().is_empty());
    }
}
