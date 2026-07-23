use crate::services::workflow_service::{self, ImportWorkflowResolution};
use crate::state::AppState;
use tauri::State;
use uuid::Uuid;

#[tauri::command]
pub async fn resolve_import_workflow(
    state: State<'_, AppState>,
    import_run_id: Option<String>,
) -> Result<ImportWorkflowResolution, String> {
    let requested_run_id = import_run_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|error| format!("invalid UUID: {error}"))?;
    let (client, handle) = {
        let manager = state.postgres_manager.lock().await;
        manager
            .connect()
            .await
            .map_err(|error| format!("{error}"))?
    };
    let result = workflow_service::resolve_import_workflow(&client, requested_run_id)
        .await
        .map_err(|error| format!("{error}"));
    handle.abort();
    result
}
