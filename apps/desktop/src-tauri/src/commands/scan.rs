use crate::domain::import_state::{ScanProgress, ScanSourceInfo};
use crate::repositories::import_repository::{
    ImportAlbumStatus, ImportRepository, ImportRunDashboard,
};
use crate::services::scan_service;
use crate::state::AppState;
use std::sync::atomic::Ordering;
use tauri::{Emitter, Runtime, State};
use uuid::Uuid;

#[tauri::command]
pub async fn select_source_directory() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("选择包含图集的源目录")
            .pick_folder()
            .map(|path| path.to_string_lossy().into_owned())
    })
    .await
    .map_err(|error| format!("source directory dialog failed: {error}"))
}

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
    let result = start_scan_for_state(&state, source_root).await?;
    let event_tracker = {
        let scan_state = state.scan_state.lock().await;
        scan_state.progress_tracker.clone()
    };
    spawn_scan_progress_events(event_tracker, app_handle);
    Ok(result)
}

pub(crate) async fn start_scan_for_state(
    state: &AppState,
    source_root: String,
) -> Result<String, String> {
    start_scan_for_state_inner(state, source_root, None).await
}

async fn start_scan_for_state_inner(
    state: &AppState,
    source_root: String,
    import_run_id: Option<Uuid>,
) -> Result<String, String> {
    tracing::info!(source_root = %source_root, "start_scan command received");
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
        tracing::warn!("start_scan rejected because another scan is active");
        return Err("A scan is already running".to_string());
    }

    scan_service::validate_source_directory(&source_root).map_err(|e| {
        tracing::warn!(source_root = %source_root, error = %e, "start_scan validation failed");
        format!("{e}")
    })?;

    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let postgres_manager = state.postgres_manager.clone();
    let settings = state.settings.clone();
    let progress_tracker = std::sync::Arc::new(tokio::sync::Mutex::new(ScanProgress {
        state: "running".to_string(),
        current_stage: "scanning".to_string(),
        ..ScanProgress::idle()
    }));

    let cancelled_clone = cancelled.clone();
    let source_root_clone = source_root.clone();
    let tracker_clone = progress_tracker.clone();

    let task = tokio::spawn(async move {
        let result = if let Some(import_run_id) = import_run_id {
            scan_service::run_scan_for_import_run(
                postgres_manager,
                settings,
                source_root_clone,
                import_run_id,
                cancelled_clone,
                tracker_clone,
            )
            .await
        } else {
            scan_service::run_scan(
                postgres_manager,
                settings,
                source_root_clone,
                cancelled_clone,
                tracker_clone,
            )
            .await
        };

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

    tracing::info!(source_root = %source_root, "start_scan command accepted");
    Ok("scan started".to_string())
}

fn spawn_scan_progress_events<R: Runtime>(
    event_tracker: std::sync::Arc<tokio::sync::Mutex<ScanProgress>>,
    app_handle: tauri::AppHandle<R>,
) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let snapshot = {
                let progress = event_tracker.lock().await;
                progress.clone()
            };
            let _ = app_handle.emit(scan_service::SCAN_PROGRESS_EVENT, snapshot.clone());
            if matches!(
                snapshot.state.as_str(),
                "ready_to_commit" | "review_required" | "completed" | "cancelled" | "failed"
            ) {
                break;
            }
        }
    });
}

#[tauri::command]
pub async fn cancel_scan(state: State<'_, AppState>) -> Result<String, String> {
    let scan_state = state.scan_state.lock().await;
    if let Some(ref handle) = scan_state.active {
        handle.cancelled.store(true, Ordering::Relaxed);
        tracing::warn!("cancel_scan command accepted");
        Ok("scan cancellation requested".to_string())
    } else {
        tracing::warn!("cancel_scan rejected because no scan is active");
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
    get_scan_progress_for_state(&state).await
}

pub(crate) async fn get_scan_progress_for_state(state: &AppState) -> Result<ScanProgress, String> {
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

fn parse_uuid(s: &str) -> Result<Uuid, String> {
    Uuid::parse_str(s).map_err(|e| format!("invalid uuid '{s}': {e}"))
}

#[tauri::command]
pub async fn get_import_runs_dashboard(
    state: State<'_, AppState>,
) -> Result<Vec<ImportRunDashboard>, String> {
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = ImportRepository::list_import_runs_summary(&client)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

#[tauri::command]
pub async fn get_import_run_albums(
    state: State<'_, AppState>,
    import_run_id: String,
) -> Result<Vec<ImportAlbumStatus>, String> {
    let run_id = parse_uuid(&import_run_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = ImportRepository::get_import_run_album_status(&client, run_id)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

#[tauri::command]
pub async fn retry_import_album(
    state: State<'_, AppState>,
    album_id: String,
) -> Result<ImportAlbumStatus, String> {
    // Keep the scan-state guard until the retry transaction commits. This
    // closes the check/start race: start_scan must acquire the same guard and
    // therefore cannot begin while retry is deleting partial album rows.
    let scan_state = state.scan_state.lock().await;
    if scan_state
        .active
        .as_ref()
        .is_some_and(|handle| !handle.task.is_finished())
    {
        return Err("Cannot retry an album while a scan is running".to_string());
    }

    let album_id = parse_uuid(&album_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = async {
        ImportRepository::reset_failed_album_for_retry(&client, album_id).await?;
        ImportRepository::get_import_album_status_by_id(&client, album_id)
            .await?
            .ok_or_else(|| {
                crate::error::AppError::Internal(format!(
                    "album {album_id} was not found after retry reset"
                ))
            })
    }
    .await
    .map_err(|e| format!("{e}"));
    handle.abort();
    drop(scan_state);
    result
}

#[tauri::command]
pub async fn abandon_import_run(
    state: State<'_, AppState>,
    import_run_id: String,
) -> Result<(), String> {
    // Serialize against start/resume/retry so an in-flight analysis cannot be
    // abandoned underneath its worker.
    let scan_state = state.scan_state.lock().await;
    if scan_state
        .active
        .as_ref()
        .is_some_and(|handle| !handle.task.is_finished())
    {
        return Err("Cannot abandon an import run while a scan is running".to_string());
    }
    let run_id = parse_uuid(&import_run_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = ImportRepository::abandon_import_run(&client, run_id)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    drop(scan_state);
    result
}

#[tauri::command]
pub async fn resume_import_run<R: Runtime>(
    state: State<'_, AppState>,
    import_run_id: String,
    app_handle: tauri::AppHandle<R>,
) -> Result<String, String> {
    let run_id = parse_uuid(&import_run_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = async {
        let run = ImportRepository::get_import_run_by_id(&client, run_id)
            .await?
            .ok_or_else(|| {
                crate::error::AppError::Internal(format!("import run {run_id} was not found"))
            })?;
        if !matches!(
            run.state.as_str(),
            "analyzing" | "scanning" | "fingerprinting" | "cancelled" | "failed"
        ) {
            return Err(crate::error::AppError::Internal(format!(
                "import run {run_id} is not resumable from state '{}'",
                run.state
            )));
        }
        Ok::<String, crate::error::AppError>(run.source_root)
    }
    .await
    .map_err(|e| format!("{e}"));
    handle.abort();
    let source_root = result?;
    let result = start_scan_for_state_inner(&state, source_root, Some(run_id)).await?;
    let event_tracker = {
        let scan_state = state.scan_state.lock().await;
        scan_state.progress_tracker.clone()
    };
    spawn_scan_progress_events(event_tracker, app_handle);
    Ok(result)
}
