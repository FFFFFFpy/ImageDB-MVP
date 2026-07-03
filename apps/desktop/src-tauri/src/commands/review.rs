use crate::domain::import_state::{
    ImportPlan, ReviewCandidateDetail, ReviewCandidateSummary, ReviewDecisionAction,
    ReviewProgress, REVIEW_DECISION_VALUES,
};
use crate::services::review_service;
use crate::state::AppState;
use std::path::PathBuf;
use tauri::State;
use uuid::Uuid;

fn parse_uuid(s: &str) -> Result<Uuid, String> {
    Uuid::parse_str(s).map_err(|e| format!("invalid UUID: {e}"))
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

#[tauri::command]
pub async fn get_latest_completed_import_run(
    state: State<'_, AppState>,
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

#[derive(serde::Serialize)]
pub struct ImagePreview {
    pub data_url: String,
}

#[tauri::command]
pub async fn get_image_preview(path: String) -> Result<ImagePreview, String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("file not found: {path}"));
    }
    let data_url = review_service::load_image_preview(&p).map_err(|e| format!("{e}"))?;
    Ok(ImagePreview { data_url })
}
