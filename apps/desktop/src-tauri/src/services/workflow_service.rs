use crate::error::AppError;
use crate::repositories::import_repository::ImportRepository;
use serde::{Deserialize, Serialize};
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportWorkflowStage {
    Analysis,
    Review,
    GeneratePlan,
    PlanDraft,
    CommitConfirm,
    Committing,
    Recovery,
    Completed,
    Failed,
    Abandoned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportWorkflowResolution {
    pub import_run_id: Option<String>,
    pub stage: ImportWorkflowStage,
    pub run_state: Option<String>,
    pub plan_state: Option<String>,
    pub file_transaction_count: u32,
}

fn resolve_stage(
    run_state: &str,
    plan_state: Option<&str>,
    transaction_states: &[String],
    unresolved_reviews: u32,
) -> ImportWorkflowStage {
    if run_state == "abandoned" {
        return ImportWorkflowStage::Abandoned;
    }

    if transaction_states.iter().any(|state| {
        matches!(
            state.as_str(),
            "cleanup_required" | "conflict" | "failed" | "cancelled"
        )
    }) {
        return ImportWorkflowStage::Recovery;
    }

    if transaction_states
        .iter()
        .any(|state| !matches!(state.as_str(), "source_archived" | "source_files_removed"))
    {
        return ImportWorkflowStage::Committing;
    }

    match run_state {
        "recovery_required" => return ImportWorkflowStage::Recovery,
        "completed" => return ImportWorkflowStage::Completed,
        "failed" => return ImportWorkflowStage::Failed,
        "committing" => return ImportWorkflowStage::Committing,
        _ => {}
    }

    match plan_state {
        Some("draft") => return ImportWorkflowStage::PlanDraft,
        Some("frozen" | "consumed") => return ImportWorkflowStage::CommitConfirm,
        _ => {}
    }

    match run_state {
        "created" | "scanning" | "fingerprinting" | "detecting_duplicates" | "analyzing" => {
            ImportWorkflowStage::Analysis
        }
        "review_required" | "ready_to_commit" if unresolved_reviews > 0 => {
            ImportWorkflowStage::Review
        }
        "review_required" | "ready_to_commit" => ImportWorkflowStage::GeneratePlan,
        "cancelled" => ImportWorkflowStage::Failed,
        _ => ImportWorkflowStage::Failed,
    }
}

pub async fn resolve_import_workflow(
    client: &Client,
    requested_run_id: Option<Uuid>,
) -> Result<ImportWorkflowResolution, AppError> {
    let import_run_id = match requested_run_id {
        Some(run_id) => Some(run_id),
        None => ImportRepository::get_latest_actionable_run_summary(client)
            .await?
            .and_then(|run| Uuid::parse_str(&run.run.import_run_id).ok()),
    };

    let Some(import_run_id) = import_run_id else {
        return Ok(ImportWorkflowResolution {
            import_run_id: None,
            stage: ImportWorkflowStage::Completed,
            run_state: None,
            plan_state: None,
            file_transaction_count: 0,
        });
    };

    let run = ImportRepository::get_import_run_by_id(client, import_run_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("import run {import_run_id} not found")))?;
    let plan_state = ImportRepository::get_latest_plan_state(client, import_run_id).await?;
    let transactions =
        ImportRepository::get_all_transactions_for_run(client, import_run_id).await?;
    let unresolved_reviews = if matches!(run.state.as_str(), "review_required" | "ready_to_commit")
    {
        let progress = ImportRepository::get_review_progress(client, import_run_id).await?;
        progress.total.saturating_sub(progress.decided)
    } else {
        0
    };
    let transaction_states: Vec<String> = transactions
        .iter()
        .map(|transaction| transaction.state.clone())
        .collect();

    Ok(ImportWorkflowResolution {
        import_run_id: Some(import_run_id.to_string()),
        stage: resolve_stage(
            &run.state,
            plan_state.as_deref(),
            &transaction_states,
            unresolved_reviews,
        ),
        run_state: Some(run.state),
        plan_state,
        file_transaction_count: transactions.len() as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_every_workflow_boundary_from_persisted_facts() {
        assert_eq!(
            resolve_stage("analyzing", None, &[], 0),
            ImportWorkflowStage::Analysis
        );
        assert_eq!(
            resolve_stage("review_required", None, &[], 2),
            ImportWorkflowStage::Review
        );
        assert_eq!(
            resolve_stage("ready_to_commit", None, &[], 0),
            ImportWorkflowStage::GeneratePlan
        );
        assert_eq!(
            resolve_stage("ready_to_commit", Some("draft"), &[], 0),
            ImportWorkflowStage::PlanDraft
        );
        assert_eq!(
            resolve_stage("ready_to_commit", Some("frozen"), &[], 0),
            ImportWorkflowStage::CommitConfirm
        );
        assert_eq!(
            resolve_stage("committing", Some("frozen"), &["planned".to_string()], 0),
            ImportWorkflowStage::Committing
        );
        assert_eq!(
            resolve_stage("cancelled", Some("frozen"), &["cancelled".to_string()], 0),
            ImportWorkflowStage::Recovery
        );
        assert_eq!(
            resolve_stage("completed", Some("consumed"), &[], 0),
            ImportWorkflowStage::Completed
        );
        assert_eq!(
            resolve_stage("failed", None, &[], 0),
            ImportWorkflowStage::Failed
        );
        assert_eq!(
            resolve_stage("failed", None, &["conflict".to_string()], 0),
            ImportWorkflowStage::Recovery
        );
        assert_eq!(
            resolve_stage("abandoned", None, &[], 0),
            ImportWorkflowStage::Abandoned
        );
    }

    #[test]
    fn cancelled_prewrite_with_frozen_plan_returns_to_confirmation() {
        assert_eq!(
            resolve_stage("cancelled", Some("frozen"), &[], 0),
            ImportWorkflowStage::CommitConfirm
        );
    }

    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_failed_run_with_conflict_transaction_routes_to_recovery() {
        use crate::domain::import_state::ImportRunState;
        use crate::domain::state_machine::TransactionState;
        use crate::infrastructure::postgres::{MigrationRunner, PostgresManager};
        use tempfile::TempDir;

        if std::env::var("IMAGEDB_POSTGRES_BIN")
            .unwrap_or_default()
            .is_empty()
        {
            panic!("IMAGEDB_POSTGRES_BIN is not set; cannot run the real workflow resolver test");
        }

        let tmp = TempDir::new().unwrap();
        let mut manager = PostgresManager::new(tmp.path());
        let probe = manager.initialize().await.unwrap();
        assert!(probe.connection_ok, "diagnostics: {:?}", probe.diagnostics);
        let (mut client, handle) = manager.connect_raw().await.unwrap();
        MigrationRunner::run_pending(&mut client).await.unwrap();

        let library_root_id = ImportRepository::upsert_default_library_root(&client)
            .await
            .unwrap();
        let import_run_id = ImportRepository::create_import_run(
            &client,
            "C:/workflow-resolver/source",
            library_root_id,
        )
        .await
        .unwrap();
        let import_album_id = ImportRepository::insert_import_album(
            &client,
            import_run_id,
            "C:/workflow-resolver/source/album",
            "album",
        )
        .await
        .unwrap();
        ImportRepository::update_import_run_state(&client, import_run_id, &ImportRunState::Failed)
            .await
            .unwrap();
        ImportRepository::insert_file_transaction(
            &client,
            Uuid::new_v4(),
            import_run_id,
            import_album_id,
            &TransactionState::Conflict,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let resolution = resolve_import_workflow(&client, Some(import_run_id))
            .await
            .unwrap();
        assert_eq!(resolution.stage, ImportWorkflowStage::Recovery);
        assert_eq!(resolution.file_transaction_count, 1);

        drop(client);
        handle.abort();
        manager.shutdown().await.unwrap();
    }
}
