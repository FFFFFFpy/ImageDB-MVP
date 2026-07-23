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
    match run_state {
        "abandoned" => return ImportWorkflowStage::Abandoned,
        "completed" => return ImportWorkflowStage::Completed,
        "recovery_required" => return ImportWorkflowStage::Recovery,
        "failed" => return ImportWorkflowStage::Failed,
        "committing" => return ImportWorkflowStage::Committing,
        _ => {}
    }

    if !transaction_states.is_empty() {
        return if run_state == "cancelled"
            || transaction_states.iter().any(|state| {
                matches!(
                    state.as_str(),
                    "cleanup_required" | "conflict" | "failed" | "cancelled"
                )
            }) {
            ImportWorkflowStage::Recovery
        } else {
            ImportWorkflowStage::Committing
        };
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
}
