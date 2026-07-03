use crate::domain::import_state::CommitProgress;
use crate::services::commit_service;
use crate::state::AppState;
use std::sync::atomic::Ordering;
use tauri::State;

#[tauri::command]
pub async fn start_import_commit(
    state: State<'_, AppState>,
    import_run_id: String,
) -> Result<String, String> {
    let run_id = uuid::Uuid::parse_str(&import_run_id).map_err(|e| format!("invalid UUID: {e}"))?;

    let mut commit_state = state.commit_state.lock().await;

    if commit_state
        .active
        .as_ref()
        .map(|h| h.task.is_finished())
        .unwrap_or(false)
    {
        // Await and resolve the finished task to ensure JoinHandle is joined.
        if let Some(handle) = commit_state.active.take() {
            let progress = resolve_commit_handle(handle).await;
            let mut tracker = commit_state.progress_tracker.lock().await;
            *tracker = progress;
        }
    }

    if commit_state.active.is_some() {
        return Err("A commit is already running".to_string());
    }

    let library_root = {
        let settings = state.settings.lock().await;
        let s = settings.get();
        s.library_root
            .clone()
            .ok_or_else(|| "library root not configured".to_string())?
    };

    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let postgres_manager = state.postgres_manager.clone();
    let progress_tracker = std::sync::Arc::new(tokio::sync::Mutex::new(CommitProgress::idle(
        &import_run_id,
    )));

    let cancelled_clone = cancelled.clone();
    let tracker_clone = progress_tracker.clone();

    let task = tokio::spawn(async move {
        let result = commit_service::run_import_commit(
            postgres_manager,
            library_root,
            run_id,
            cancelled_clone,
            tracker_clone.clone(),
        )
        .await;

        match result {
            Ok(_) => {
                let tracker = tracker_clone.lock().await;
                tracker.clone()
            }
            Err(e) => CommitProgress {
                state: "failed".to_string(),
                import_run_id: import_run_id.clone(),
                current_stage: "failed".to_string(),
                errors: vec![e.to_string()],
                ..CommitProgress::idle(&import_run_id)
            },
        }
    });

    commit_state.active = Some(crate::state::CommitHandle { cancelled, task });
    commit_state.progress_tracker = progress_tracker;

    Ok("commit started".to_string())
}

#[tauri::command]
pub async fn cancel_import_commit(state: State<'_, AppState>) -> Result<String, String> {
    let commit_state = state.commit_state.lock().await;
    if let Some(ref handle) = commit_state.active {
        handle.cancelled.store(true, Ordering::Relaxed);
        Ok("commit cancellation requested".to_string())
    } else {
        Err("No active commit".to_string())
    }
}

/// Await a finished JoinHandle and handle JoinError (panic).
/// Returns the inner value on success, or a failed CommitProgress on panic.
async fn resolve_commit_handle(handle: crate::state::CommitHandle) -> CommitProgress {
    match handle.task.await {
        Ok(progress) => progress,
        Err(join_err) => {
            let msg = if join_err.is_panic() {
                let panic_msg = join_err
                    .into_panic()
                    .downcast::<String>()
                    .map(|s| *s)
                    .unwrap_or_else(|_| "commit task panicked".to_string());
                format!("panic: {panic_msg}")
            } else {
                "commit task cancelled".to_string()
            };
            CommitProgress {
                state: "failed".to_string(),
                import_run_id: String::new(),
                current_stage: "failed".to_string(),
                errors: vec![msg],
                ..CommitProgress::idle("")
            }
        }
    }
}

#[tauri::command]
pub async fn get_commit_progress(state: State<'_, AppState>) -> Result<CommitProgress, String> {
    let mut commit_state = state.commit_state.lock().await;
    if commit_state
        .active
        .as_ref()
        .map(|h| h.task.is_finished())
        .unwrap_or(false)
    {
        // Await and resolve the finished task to ensure JoinHandle is joined.
        if let Some(handle) = commit_state.active.take() {
            let progress = resolve_commit_handle(handle).await;
            let mut tracker = commit_state.progress_tracker.lock().await;
            *tracker = progress;
        }
    }
    let tracker = commit_state.progress_tracker.lock().await;
    Ok(tracker.clone())
}
