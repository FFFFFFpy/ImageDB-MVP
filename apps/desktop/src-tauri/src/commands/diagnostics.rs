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
    tracing::info!("export_diagnostics command received");
    let result = diagnostics_service::export_diagnostics(state)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "export_diagnostics failed");
            format!("{e}")
        })?;
    tracing::info!(
        path = %result.path,
        byte_size = result.byte_size,
        "export_diagnostics finished"
    );
    Ok(result)
}
