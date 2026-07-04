use crate::domain::import_state::{ScanProgress, ScanSourceInfo};
use crate::services::scan_service;
use crate::state::AppState;
use std::sync::atomic::Ordering;
use tauri::{Emitter, Runtime, State};

#[tauri::command]
pub async fn validate_source_directory(source_root: String) -> Result<ScanSourceInfo, String> {
    scan_service::scan_source_info(&source_root)
        .await
        .map_err(|e| format!("{e}"))
}

#[tauri::command]
pub async fn start_scan<R: Runtime>(
    state: State<'_, AppState>,
    source_root: String,
    app_handle: tauri::AppHandle<R>,
) -> Result<String, String> {
    let mut scan_state = state.scan_state.lock().await;

    if scan_state
        .active
        .as_ref()
        .map(|handle| handle.task.is_finished())
        .unwrap_or(false)
    {
        // Await and resolve the finished task to ensure JoinHandle is joined.
        if let Some(handle) = scan_state.active.take() {
            let progress = resolve_scan_handle(handle).await;
            let mut tracker = scan_state.progress_tracker.lock().await;
            *tracker = progress;
        }
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

    let event_tracker = progress_tracker.clone();
    let event_handle = app_handle_clone;
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let snapshot = {
                let progress = event_tracker.lock().await;
                progress.clone()
            };
            let _ = event_handle.emit(scan_service::SCAN_PROGRESS_EVENT, snapshot.clone());
            if matches!(
                snapshot.state.as_str(),
                "ready_to_commit" | "review_required" | "completed" | "cancelled" | "failed"
            ) {
                break;
            }
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

/// Await a finished JoinHandle and handle JoinError (panic).
/// Returns the inner value on success, or a failed ScanProgress on panic.
async fn resolve_scan_handle(handle: crate::state::ScanHandle) -> ScanProgress {
    match handle.task.await {
        Ok(progress) => progress,
        Err(join_err) => {
            let msg = if join_err.is_panic() {
                let panic_msg = join_err
                    .into_panic()
                    .downcast::<String>()
                    .map(|s| *s)
                    .unwrap_or_else(|_| "scan task panicked".to_string());
                format!("panic: {panic_msg}")
            } else {
                "scan task cancelled".to_string()
            };
            ScanProgress {
                state: "failed".to_string(),
                current_stage: "failed".to_string(),
                errors: vec![msg],
                ..ScanProgress::idle()
            }
        }
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
        // Await and resolve the finished task to ensure JoinHandle is joined.
        if let Some(handle) = scan_state.active.take() {
            let progress = resolve_scan_handle(handle).await;
            let mut tracker = scan_state.progress_tracker.lock().await;
            *tracker = progress;
        }
    }
    let tracker = scan_state.progress_tracker.lock().await;
    Ok(tracker.clone())
}
