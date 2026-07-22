use crate::domain::import_state::{
    ImportPlan, ReviewCandidateDetail, ReviewCandidateSummary, ReviewDecisionAction,
    ReviewGroupDetail, ReviewGroupMemberDecision, ReviewGroupSummary, ReviewProgress,
    SourceFileMode, REVIEW_DECISION_VALUES,
};
use crate::services::review_service;
use crate::state::AppState;
use tauri::State;
use uuid::Uuid;

fn parse_uuid(s: &str) -> Result<Uuid, String> {
    Uuid::parse_str(s).map_err(|e| format!("invalid UUID: {e}"))
}

#[tauri::command]
pub async fn get_review_groups(
    state: State<'_, AppState>,
    import_run_id: String,
) -> Result<Vec<ReviewGroupSummary>, String> {
    let run_id = parse_uuid(&import_run_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = review_service::get_review_groups(&client, run_id)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

#[tauri::command]
pub async fn get_review_group_detail(
    state: State<'_, AppState>,
    group_id: String,
) -> Result<ReviewGroupDetail, String> {
    let group_id = parse_uuid(&group_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = review_service::get_review_group_detail(&client, group_id)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

#[tauri::command]
pub async fn submit_review_group_decision(
    state: State<'_, AppState>,
    group_id: String,
    decisions: Vec<ReviewGroupMemberDecision>,
) -> Result<(), String> {
    let group_id = parse_uuid(&group_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = review_service::submit_review_group_decision(&client, group_id, &decisions)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

#[tauri::command]
pub async fn get_review_queue(
    state: State<'_, AppState>,
    import_run_id: String,
) -> Result<Vec<ReviewCandidateSummary>, String> {
    let run_id = parse_uuid(&import_run_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = review_service::get_review_queue(&client, run_id)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

#[tauri::command]
pub async fn get_review_candidate_detail(
    state: State<'_, AppState>,
    candidate_id: String,
) -> Result<ReviewCandidateDetail, String> {
    let cid = parse_uuid(&candidate_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = review_service::get_review_detail(&client, cid)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

#[tauri::command]
pub async fn submit_review_decision(
    state: State<'_, AppState>,
    candidate_id: String,
    decision: String,
) -> Result<(), String> {
    let cid = parse_uuid(&candidate_id)?;
    let action = ReviewDecisionAction::from_str_opt(&decision).ok_or_else(|| {
        format!("invalid decision: {decision}; expected one of {REVIEW_DECISION_VALUES}")
    })?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = review_service::submit_decision(&client, cid, action)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

#[tauri::command]
pub async fn skip_review_album(
    state: State<'_, AppState>,
    import_run_id: String,
    album_id: String,
) -> Result<u32, String> {
    let run_id = parse_uuid(&import_run_id)?;
    let aid = parse_uuid(&album_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = review_service::skip_album_candidates(&client, run_id, aid)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

#[tauri::command]
pub async fn get_review_progress(
    state: State<'_, AppState>,
    import_run_id: String,
) -> Result<ReviewProgress, String> {
    get_review_progress_for_state(&state, import_run_id).await
}

pub(crate) async fn get_review_progress_for_state(
    state: &AppState,
    import_run_id: String,
) -> Result<ReviewProgress, String> {
    let run_id = parse_uuid(&import_run_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = review_service::get_review_progress(&client, run_id)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

#[tauri::command]
pub async fn generate_import_plan(
    state: State<'_, AppState>,
    import_run_id: String,
) -> Result<ImportPlan, String> {
    generate_import_plan_for_state(&state, import_run_id).await
}

pub(crate) async fn generate_import_plan_for_state(
    state: &AppState,
    import_run_id: String,
) -> Result<ImportPlan, String> {
    let run_id = parse_uuid(&import_run_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = review_service::generate_import_plan(&client, run_id)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

/// Freeze the import plan for a run as a single atomic database
/// transaction. Writes the three plan tables + plan_hash, transitions the
/// plan to `frozen`, and advances the run to `ready_to_commit`. Idempotent —
/// re-freezing returns the existing frozen plan summary. This is the public
/// main-chain freeze entry point called from the review flow.
#[tauri::command]
pub async fn freeze_import_plan(
    state: State<'_, AppState>,
    import_run_id: String,
) -> Result<ImportPlan, String> {
    freeze_import_plan_for_state(&state, import_run_id).await
}

pub(crate) async fn freeze_import_plan_for_state(
    state: &AppState,
    import_run_id: String,
) -> Result<ImportPlan, String> {
    let run_id = parse_uuid(&import_run_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = review_service::freeze_import_plan(&client, run_id)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

/// Read the frozen plan summary for the commit-confirm page. Returns the
/// persisted frozen view (kept images, counts, skipped albums) — never
/// re-derives from candidates/reviews, so post-freeze edits cannot change
/// what the commit page shows. Returns `null` when no frozen plan exists.
#[tauri::command]
pub async fn get_frozen_import_plan_summary(
    state: State<'_, AppState>,
    import_run_id: String,
) -> Result<Option<ImportPlan>, String> {
    get_frozen_import_plan_summary_for_state(&state, import_run_id).await
}

pub(crate) async fn get_frozen_import_plan_summary_for_state(
    state: &AppState,
    import_run_id: String,
) -> Result<Option<ImportPlan>, String> {
    let run_id = parse_uuid(&import_run_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = review_service::get_frozen_plan_summary(&client, run_id)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

/// Read the persisted editable draft shown by the manual plan-review page.
/// Drafts carry no plan hash and cannot be committed.
#[tauri::command]
pub async fn get_import_plan_draft_summary(
    state: State<'_, AppState>,
    import_run_id: String,
) -> Result<Option<ImportPlan>, String> {
    get_import_plan_draft_summary_for_state(&state, import_run_id).await
}

pub(crate) async fn get_import_plan_draft_summary_for_state(
    state: &AppState,
    import_run_id: String,
) -> Result<Option<ImportPlan>, String> {
    let run_id = parse_uuid(&import_run_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = review_service::get_draft_plan_summary(&client, run_id)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

/// Abandon the entire frozen import workflow only while it is still before
/// transaction prewrite. The service owns the row lock and safety checks.
#[tauri::command]
pub async fn abandon_frozen_import_workflow(
    state: State<'_, AppState>,
    import_run_id: String,
) -> Result<(), String> {
    abandon_frozen_import_workflow_for_state(&state, import_run_id).await
}

pub(crate) async fn abandon_frozen_import_workflow_for_state(
    state: &AppState,
    import_run_id: String,
) -> Result<(), String> {
    let run_id = parse_uuid(&import_run_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = review_service::abandon_frozen_import_workflow(&client, run_id)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

#[tauri::command]
pub async fn set_import_plan_album_included(
    state: State<'_, AppState>,
    import_run_id: String,
    album_id: String,
    included: bool,
) -> Result<ImportPlan, String> {
    let run_id = parse_uuid(&import_run_id)?;
    let aid = parse_uuid(&album_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = review_service::set_plan_album_included(&client, run_id, aid, included)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

#[tauri::command]
pub async fn set_import_plan_image_included(
    state: State<'_, AppState>,
    import_run_id: String,
    image_id: String,
    target_album_id: String,
    included: bool,
) -> Result<ImportPlan, String> {
    set_import_plan_image_included_for_state(&state, import_run_id, image_id, target_album_id, included).await
}

pub(crate) async fn set_import_plan_image_included_for_state(
    state: &AppState,
    import_run_id: String,
    image_id: String,
    target_album_id: String,
    included: bool,
) -> Result<ImportPlan, String> {
    let run_id = parse_uuid(&import_run_id)?;
    let iid = parse_uuid(&image_id)?;
    let aid = parse_uuid(&target_album_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = review_service::set_plan_image_included(&client, run_id, iid, aid, included)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

#[tauri::command]
pub async fn set_import_plan_source_file_mode(
    state: State<'_, AppState>,
    import_run_id: String,
    source_file_mode: String,
) -> Result<ImportPlan, String> {
    set_import_plan_source_file_mode_for_state(&state, import_run_id, source_file_mode).await
}

pub(crate) async fn set_import_plan_source_file_mode_for_state(
    state: &AppState,
    import_run_id: String,
    source_file_mode: String,
) -> Result<ImportPlan, String> {
    let run_id = parse_uuid(&import_run_id)?;
    let mode = SourceFileMode::from_str_opt(&source_file_mode)
        .ok_or_else(|| format!("invalid source_file_mode: {source_file_mode}"))?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = review_service::set_plan_source_file_mode(&client, run_id, mode)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

#[tauri::command]
pub async fn get_latest_completed_import_run(
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    get_latest_completed_import_run_for_state(&state).await
}

pub(crate) async fn get_latest_completed_import_run_for_state(
    state: &AppState,
) -> Result<Option<String>, String> {
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result =
        crate::repositories::import_repository::ImportRepository::get_latest_completed_run(&client)
            .await
            .map(|opt| opt.map(|id| id.to_string()))
            .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

/// Find the most recent run that the review page should load — a run in
/// `review_required` or `ready_to_commit` (a freshly-finished scan leaves
/// the run here, never `completed`). Returns `null` when no reviewable run
/// exists, which the review page surfaces as "complete a scan first".
#[tauri::command]
pub async fn get_latest_reviewable_import_run(
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    get_latest_reviewable_import_run_for_state(&state).await
}

pub(crate) async fn get_latest_reviewable_import_run_for_state(
    state: &AppState,
) -> Result<Option<String>, String> {
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result =
        crate::repositories::import_repository::ImportRepository::get_latest_reviewable_run(
            &client,
        )
        .await
        .map(|opt| opt.map(|id| id.to_string()))
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

/// Find the most recent run that can be (re-)entered from the commit page:
/// `completed`, `ready_to_commit`, or `cancelled`. Used by the CommitPage so
/// a run cancelled before any transaction was prewritten (P0 fix) is
/// re-committable rather than stuck at `recovery_required` with no
/// transaction to recover.
#[tauri::command]
pub async fn get_latest_committable_import_run(
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    get_latest_committable_import_run_for_state(&state).await
}

pub(crate) async fn get_latest_committable_import_run_for_state(
    state: &AppState,
) -> Result<Option<String>, String> {
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result =
        crate::repositories::import_repository::ImportRepository::get_latest_committable_run(
            &client,
        )
        .await
        .map(|opt| opt.map(|id| id.to_string()))
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

#[derive(serde::Serialize)]
pub struct ImagePreview {
    pub data_url: String,
}

#[tauri::command]
pub async fn get_image_preview(
    state: State<'_, AppState>,
    candidate_id: String,
    image_side: String,
) -> Result<ImagePreview, String> {
    let cid = parse_uuid(&candidate_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let data_url = review_service::load_image_preview_by_candidate(&client, cid, &image_side)
        .await
        .map_err(|e| format!("{e}"))?;
    handle.abort();
    Ok(ImagePreview { data_url })
}

#[tauri::command]
pub async fn get_review_group_member_preview(
    state: State<'_, AppState>,
    group_id: String,
    image_id: String,
    image_source: String,
) -> Result<ImagePreview, String> {
    let group_id = parse_uuid(&group_id)?;
    let image_id = parse_uuid(&image_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = review_service::load_review_group_member_preview(
        &client,
        group_id,
        image_id,
        &image_source,
    )
    .await
    .map(|data_url| ImagePreview { data_url })
    .map_err(|e| format!("{e}"));
    handle.abort();
    result
}

#[tauri::command]
pub async fn get_import_plan_image_preview(
    state: State<'_, AppState>,
    import_run_id: String,
    image_id: String,
) -> Result<ImagePreview, String> {
    let run_id = parse_uuid(&import_run_id)?;
    let iid = parse_uuid(&image_id)?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let data_url = review_service::load_image_preview_by_import_image(&client, run_id, iid)
        .await
        .map_err(|e| format!("{e}"))?;
    handle.abort();
    Ok(ImagePreview { data_url })
}
