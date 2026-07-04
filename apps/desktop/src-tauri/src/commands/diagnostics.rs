use crate::services::diagnostics_service::{self, DiagnosticsExportResult};
use crate::state::AppState;
use tauri::State;

#[tauri::command]
pub async fn export_diagnostics(
    state: State<'_, AppState>,
) -> Result<DiagnosticsExportResult, String> {
    export_diagnostics_for_state(&state).await
}

pub(crate) async fn export_diagnostics_for_state(
    state: &AppState,
) -> Result<DiagnosticsExportResult, String> {
    diagnostics_service::export_diagnostics(state)
        .await
        .map_err(|e| format!("{e}"))
}
