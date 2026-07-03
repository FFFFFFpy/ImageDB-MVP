//! IPC commands for the recovery workflow.
//!
//! These surface the Recovery Service to the desktop GUI: scan non-terminal
//! transactions, execute recovery for one, and re-verify a transaction's
//! on-disk evidence.
use crate::repositories::import_repository::ImportRepository;
use crate::services::commit_service::{
    validate_and_hash_frozen_plan, verify_complete_evidence, IdempotencyVerdict,
};
use crate::services::recovery_service;
use crate::state::AppState;
use serde::Serialize;
use std::path::PathBuf;
use tauri::State;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct RecoveryDiagnosticDto {
    pub transaction_id: String,
    pub import_run_id: String,
    pub import_album_id: String,
    pub current_state: String,
    pub staging_path: Option<String>,
    pub target_path: Option<String>,
    pub manifest_path: Option<String>,
    pub staging_exists: bool,
    pub target_exists: bool,
    pub manifest_exists: bool,
    pub plan_hash: Option<String>,
    pub last_error: Option<String>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecoveryOutcomeDto {
    pub transaction_id: String,
    pub final_state: String,
    pub recovered: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReverifyResultDto {
    pub transaction_id: String,
    pub verdict: String,
    pub message: String,
}

/// List every non-terminal file transaction with its recovery diagnostics.
#[tauri::command]
pub async fn scan_recoverable_transactions(
    state: State<'_, AppState>,
) -> Result<Vec<RecoveryDiagnosticDto>, String> {
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = recovery_service::scan_recoverable_transactions(&client)
        .await
        .map_err(|e| format!("{e}"));
    handle.abort();
    result.map(|diags| {
        diags
            .into_iter()
            .map(|d| RecoveryDiagnosticDto {
                transaction_id: d.transaction_id.to_string(),
                import_run_id: d.import_run_id.to_string(),
                import_album_id: d.import_album_id.to_string(),
                current_state: d.current_state,
                staging_path: d.staging_path,
                target_path: d.target_path,
                manifest_path: d.manifest_path,
                staging_exists: d.staging_exists,
                target_exists: d.target_exists,
                manifest_exists: d.manifest_exists,
                plan_hash: d.plan_hash,
                last_error: d.last_error,
                diagnostics: d.diagnostics,
            })
            .collect()
    })
}

/// Execute recovery for a single transaction (idempotent).
#[tauri::command]
pub async fn recover_transaction(
    state: State<'_, AppState>,
    transaction_id: String,
) -> Result<RecoveryOutcomeDto, String> {
    let tx_id = Uuid::parse_str(&transaction_id).map_err(|e| format!("invalid UUID: {e}"))?;
    let pg = state.postgres_manager.clone();
    let outcome = recovery_service::recover_transaction(pg, tx_id)
        .await
        .map_err(|e| format!("{e}"))?;
    Ok(RecoveryOutcomeDto {
        transaction_id: outcome.transaction_id.to_string(),
        final_state: outcome.final_state,
        recovered: outcome.recovered,
        message: outcome.message,
    })
}

/// Re-verify a transaction's complete on-disk + DB evidence without mutating
/// anything. Returns the verdict (already_committed / resume / conflict).
#[tauri::command]
pub async fn reverify_transaction(
    state: State<'_, AppState>,
    transaction_id: String,
) -> Result<ReverifyResultDto, String> {
    let tx_id = Uuid::parse_str(&transaction_id).map_err(|e| format!("invalid UUID: {e}"))?;
    let (client, handle) = {
        let mgr = state.postgres_manager.lock().await;
        mgr.connect().await.map_err(|e| format!("{e}"))?
    };
    let result = reverify_with_client(&client, tx_id).await;
    handle.abort();
    let (verdict, message) = result.map_err(|e| format!("{e}"))?;
    Ok(ReverifyResultDto {
        transaction_id: transaction_id.clone(),
        verdict,
        message,
    })
}

async fn reverify_with_client(
    client: &tokio_postgres::Client,
    transaction_id: Uuid,
) -> Result<(String, String), crate::error::AppError> {
    let tx = ImportRepository::get_file_transaction(client, transaction_id)
        .await?
        .ok_or_else(|| {
            crate::error::AppError::Internal(format!("transaction {transaction_id} not found"))
        })?;
    let frozen = ImportRepository::load_frozen_plan(client, tx.import_run_id)
        .await?
        .ok_or_else(|| crate::error::AppError::Internal("no frozen plan".to_string()))?;
    let (plan_album, plan_images) = frozen
        .albums
        .iter()
        .find(|(a, _)| a.import_album_id == tx.import_album_id)
        .ok_or_else(|| crate::error::AppError::Internal("album not in plan".to_string()))?
        .clone();
    let library_root_path =
        ImportRepository::get_library_root_path(client, frozen.library_root_id).await?;
    let library_root = PathBuf::from(&library_root_path);
    let validated_plan_hash = validate_and_hash_frozen_plan(&frozen, frozen.library_root_id)?;
    let album_rel =
        crate::services::commit_service::normalize_relative_path(&plan_album.target_relative_path)?;
    let verdict = verify_complete_evidence(
        client,
        &library_root,
        frozen.library_root_id,
        &tx,
        frozen.plan_id,
        &validated_plan_hash,
        &album_rel,
        &plan_images,
    )
    .await?;
    let (verdict_str, msg) = match verdict {
        IdempotencyVerdict::AlreadyCommitted => (
            "already_committed".to_string(),
            "all evidence matches".to_string(),
        ),
        IdempotencyVerdict::Conflict(m) => ("conflict".to_string(), m),
        IdempotencyVerdict::Resume { transaction_id } => (
            "resume".to_string(),
            format!("mid-flight transaction {transaction_id} detected; route to recovery"),
        ),
    };
    Ok((verdict_str, msg))
}
