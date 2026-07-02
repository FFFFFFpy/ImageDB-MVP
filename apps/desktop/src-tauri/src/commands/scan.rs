use crate::domain::import_state::{ScanProgress, ScanSourceInfo};
use crate::services::scan_service;
use crate::state::AppState;
use std::sync::atomic::Ordering;
use tauri::State;

#[tauri::command]
pub async fn validate_source_directory(source_root: String) -> Result<ScanSourceInfo, String> {
    scan_service::scan_source_info(&source_root)
        .await
        .map_err(|e| format!("{e}"))
}

#[tauri::command]
pub async fn start_scan(
    state: State<'_, AppState>,
    source_root: String,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    let mut scan_state = state.scan_state.lock().await;

    if scan_state
        .active
        .as_ref()
        .map(|handle| handle.task.is_finished())
        .unwrap_or(false)
    {
        scan_state.active = None;
    }

    if scan_state.active.is_some() {
        return Err("A scan is already running".to_string());
    }

    scan_service::validate_source_directory(&source_root).map_err(|e| format!("{e}"))?;

    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let postgres_manager = state.postgres_manager.clone();
    let settings = state.settings.clone();
    let progress_tracker = std::sync::Arc::new(tokio::sync::Mutex::new(ScanProgress {
        state: "running".to_string(),
        current_stage: "scanning".to_string(),
        ..ScanProgress::idle()
    }));

    let cancelled_clone = cancelled.clone();
    let app_handle_clone = app_handle.clone();
    let source_root_clone = source_root.clone();
    let tracker_clone = progress_tracker.clone();

    let task = tokio::spawn(async move {
        let result = scan_service::run_scan(
            Some(app_handle_clone),
            postgres_manager,
            settings,
            source_root_clone,
            cancelled_clone,
            tracker_clone,
        )
        .await;

        match result {
            Ok(progress) => progress,
            Err(e) => ScanProgress {
                state: "failed".to_string(),
                current_stage: "failed".to_string(),
                errors: vec![e.to_string()],
                ..ScanProgress::idle()
            },
        }
    });

    scan_state.active = Some(crate::state::ScanHandle { cancelled, task });
    scan_state.progress_tracker = progress_tracker;

    Ok("scan started".to_string())
}

#[tauri::command]
pub async fn cancel_scan(state: State<'_, AppState>) -> Result<String, String> {
    let scan_state = state.scan_state.lock().await;
    if let Some(ref handle) = scan_state.active {
        handle.cancelled.store(true, Ordering::Relaxed);
        Ok("scan cancellation requested".to_string())
    } else {
        Err("No active scan".to_string())
    }
}

#[tauri::command]
pub async fn get_scan_progress(state: State<'_, AppState>) -> Result<ScanProgress, String> {
    let mut scan_state = state.scan_state.lock().await;
    if scan_state
        .active
        .as_ref()
        .map(|handle| handle.task.is_finished())
        .unwrap_or(false)
    {
        scan_state.active = None;
    }
    let tracker = scan_state.progress_tracker.lock().await;
    Ok(tracker.clone())
}
