use crate::domain::import_state::CommitProgress;
use crate::services::commit_service;
use crate::state::{AppState, CriticalTaskKind};
use std::sync::atomic::Ordering;
use tauri::State;

#[tauri::command]
pub async fn start_import_commit(
    state: State<'_, AppState>,
    import_run_id: String,
    expected_plan_hash: String,
) -> Result<String, String> {
    start_import_commit_for_state_with_expected_hash(
        &state,
        import_run_id,
        Some(expected_plan_hash),
    )
    .await
}

#[allow(dead_code)]
pub(crate) async fn start_import_commit_for_state(
    state: &AppState,
    import_run_id: String,
) -> Result<String, String> {
    start_import_commit_for_state_with_expected_hash(state, import_run_id, None).await
}

pub(crate) async fn start_import_commit_for_state_with_expected_hash(
    state: &AppState,
    import_run_id: String,
    expected_plan_hash: Option<String>,
) -> Result<String, String> {
    tracing::info!(import_run_id = %import_run_id, "start_import_commit command received");
    let run_id = uuid::Uuid::parse_str(&import_run_id).map_err(|e| format!("invalid UUID: {e}"))?;
    let commit_lease = state
        .critical_operation_guard
        .begin_task(CriticalTaskKind::Commit)?;

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
        tracing::warn!("start_import_commit rejected because another commit is active");
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
        let _commit_lease = commit_lease;
        let result = commit_service::run_import_commit_with_expected_plan_hash(
            postgres_manager,
            library_root,
            run_id,
            cancelled_clone,
            tracker_clone.clone(),
            expected_plan_hash,
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

    tracing::info!(%run_id, "start_import_commit command accepted");
    Ok("commit started".to_string())
}

#[tauri::command]
pub async fn cancel_import_commit(state: State<'_, AppState>) -> Result<String, String> {
    cancel_import_commit_for_state(&state).await
}

pub(crate) async fn cancel_import_commit_for_state(state: &AppState) -> Result<String, String> {
    let commit_state = state.commit_state.lock().await;
    if let Some(ref handle) = commit_state.active {
        handle.cancelled.store(true, Ordering::Relaxed);
        tracing::warn!("cancel_import_commit command accepted");
        Ok("commit cancellation requested".to_string())
    } else {
        tracing::warn!("cancel_import_commit rejected because no commit is active");
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
    get_commit_progress_for_state(&state).await
}

pub(crate) async fn get_commit_progress_for_state(
    state: &AppState,
) -> Result<CommitProgress, String> {
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
