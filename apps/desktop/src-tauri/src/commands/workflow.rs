use crate::services::workflow_service;
use crate::state::AppState;
use tauri::State;

#[tauri::command]
pub async fn get_import_workflow_stage(
    state: State<'_, AppState>,
) -> Result<workflow_service::WorkflowStage, String> {
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = workflow_service::resolve_workflow_stage(&client)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result
}
