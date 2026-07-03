//! Recovery service: resumes interrupted import transactions from their
//! persisted state. This module is rewritten in Phase 7; the placeholder
//! functions below are replaced with real recovery actions then.
#![allow(dead_code)]
use crate::error::AppError;
use tokio_postgres::Client;
use uuid::Uuid;

/// Diagnostic information for a recoverable transaction.
#[derive(Debug, Clone)]
pub struct RecoveryDiagnostic {
    pub transaction_id: Uuid,
    pub import_run_id: Uuid,
    pub import_album_id: Uuid,
    pub current_state: String,
    pub staging_path: Option<String>,
    pub target_path: Option<String>,
    pub manifest_path: Option<String>,
    pub staging_exists: bool,
    pub target_exists: bool,
    pub manifest_exists: bool,
    pub diagnostics: Vec<String>,
}

/// Scan all non-terminal transactions and generate recovery diagnostics.
pub async fn scan_recoverable_transactions(
    client: &Client,
) -> Result<Vec<RecoveryDiagnostic>, AppError> {
    let rows = client
        .query(
            "SELECT ft.id, ft.import_run_id, ft.import_album_id, ft.state,
                    ft.staging_path, ft.target_path, ft.manifest_path,
                    ft.last_error
             FROM file_transactions ft
             WHERE ft.state NOT IN ('source_archived', 'failed', 'cancelled')
             ORDER BY ft.started_at",
            &[],
        )
        .await
        .map_err(|e| {
            AppError::Internal(format!("failed to query recoverable transactions: {e}"))
        })?;

    let mut diagnostics = Vec::new();
    for row in &rows {
        let staging_path: Option<String> = row.get("staging_path");
        let target_path: Option<String> = row.get("target_path");
        let manifest_path: Option<String> = row.get("manifest_path");

        let staging_exists = staging_path
            .as_ref()
            .map(|p| std::path::Path::new(p).exists())
            .unwrap_or(false);
        let target_exists = target_path
            .as_ref()
            .map(|p| std::path::Path::new(p).exists())
            .unwrap_or(false);
        let manifest_exists = manifest_path
            .as_ref()
            .map(|p| std::path::Path::new(p).exists())
            .unwrap_or(false);

        let mut diags = Vec::new();
        let state: String = row.get("state");
        match state.as_str() {
            "planned" | "staging" => {
                diags.push("staging incomplete; can retry copy".to_string());
            }
            "verifying" | "verified" => {
                diags.push("staging complete; needs verification and publish".to_string());
            }
            "publishing" => {
                if staging_exists && !target_exists {
                    diags.push("staging ready; target missing; retry rename".to_string());
                } else if target_exists && manifest_exists {
                    diags.push("target exists with manifest; check consistency".to_string());
                } else if staging_exists && target_exists {
                    diags.push(
                        "both staging and target exist; needs conflict resolution".to_string(),
                    );
                } else {
                    diags.push("unknown publish state".to_string());
                }
            }
            "published" | "db_committing" => {
                if target_exists && manifest_exists {
                    diags.push("published; retry database commit".to_string());
                } else {
                    diags.push("published but target or manifest missing".to_string());
                }
            }
            "library_committed" | "source_archiving" => {
                if target_exists {
                    diags.push("library committed; retry source archive".to_string());
                } else {
                    diags.push("library committed but target missing".to_string());
                }
            }
            "cleanup_required" => {
                diags.push("cleanup needed for staging directory".to_string());
            }
            "conflict" => {
                diags.push("target directory conflict; manual resolution required".to_string());
            }
            _ => {
                diags.push(format!("unhandled state: {state}"));
            }
        }

        diagnostics.push(RecoveryDiagnostic {
            transaction_id: row.get("id"),
            import_run_id: row.get("import_run_id"),
            import_album_id: row.get("import_album_id"),
            current_state: state,
            staging_path,
            target_path,
            manifest_path,
            staging_exists,
            target_exists,
            manifest_exists,
            diagnostics: diags,
        });
    }

    Ok(diagnostics)
}

/// Attempt to recover a single transaction based on its current state.
pub async fn recover_transaction(
    client: &Client,
    transaction_id: Uuid,
) -> Result<String, AppError> {
    let row = client
        .query_one(
            "SELECT state FROM file_transactions WHERE id = $1",
            &[&transaction_id],
        )
        .await
        .map_err(|e| AppError::Internal(format!("failed to query transaction: {e}")))?;

    let state: String = row.get("state");
    match state.as_str() {
        "planned" | "staging" | "verifying" | "verified" => Ok("retry_staging".to_string()),
        "publishing" | "published" | "db_committing" => Ok("retry_publish_and_commit".to_string()),
        "library_committed" | "source_archiving" => Ok("retry_archive".to_string()),
        "cleanup_required" => Ok("cleanup".to_string()),
        "conflict" => Err(AppError::Internal(
            "conflict requires manual resolution".to_string(),
        )),
        _ => Err(AppError::Internal(format!(
            "cannot recover from state: {state}"
        ))),
    }
}
