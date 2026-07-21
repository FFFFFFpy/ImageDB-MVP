use crate::error::AppError;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkflowStage {
    pub stage: String,
    pub import_run_id: Option<String>,
}

/// Resolve the current import workflow stage by inspecting the latest
/// actionable run. This is the single entry point used by the frontend
/// router when no explicit runId is carried by the navigation.
pub async fn resolve_workflow_stage(client: &Client) -> Result<WorkflowStage, AppError> {
    let row = client
        .query_opt(
            "SELECT r.id, r.state,
                EXISTS (
                    SELECT 1 FROM review_groups rg
                    WHERE rg.import_run_id = r.id
                      AND rg.requires_manual_review
                      AND rg.state = 'pending'
                ) AS has_pending_reviews,
                EXISTS (
                    SELECT 1 FROM import_plans p
                    WHERE p.import_run_id = r.id AND p.state = 'draft'
                ) AS has_draft_plan,
                EXISTS (
                    SELECT 1 FROM import_plans p
                    WHERE p.import_run_id = r.id
                      AND p.state IN ('frozen', 'consumed')
                      AND p.plan_hash IS NOT NULL
                ) AS has_frozen_plan,
                EXISTS (
                    SELECT 1 FROM file_transactions ft
                    WHERE ft.import_run_id = r.id
                ) AS has_transactions,
                NOT EXISTS (
                    SELECT 1 FROM import_albums ia
                    WHERE ia.import_run_id = r.id
                      AND ia.state IN ('pending', 'analyzing')
                ) AS analysis_complete
             FROM import_runs r
             WHERE r.state <> 'abandoned'
             ORDER BY r.started_at DESC
             LIMIT 1",
            &[],
        )
        .await
        .map_err(|e| AppError::Internal(format!("failed to resolve workflow stage: {e}")))?;

    let Some(row) = row else {
        return Ok(WorkflowStage {
            stage: "idle".to_string(),
            import_run_id: None,
        });
    };

    let run_id: Uuid = row.get("id");
    let state: String = row.get("state");
    let has_pending_reviews: bool = row.get("has_pending_reviews");
    let has_draft_plan: bool = row.get("has_draft_plan");
    let has_frozen_plan: bool = row.get("has_frozen_plan");
    let has_transactions: bool = row.get("has_transactions");
    let analysis_complete: bool = row.get("analysis_complete");

    let stage = match state.as_str() {
        "created" | "scanning" | "fingerprinting" | "detecting_duplicates" | "analyzing" => {
            "analysis"
        }
        "review_required" => {
            if has_pending_reviews || !analysis_complete {
                "review"
            } else if has_frozen_plan {
                "commit_confirm"
            } else if has_draft_plan {
                "plan_draft"
            } else {
                "generate_plan"
            }
        }
        "ready_to_commit" => {
            if has_frozen_plan {
                "commit_confirm"
            } else if has_draft_plan {
                "plan_draft"
            } else {
                "generate_plan"
            }
        }
        "committing" => "committing",
        "recovery_required" => "recovery",
        "completed" => "completed",
        "failed" => "failed",
        "cancelled" => {
            if has_frozen_plan && !has_transactions {
                "commit_confirm"
            } else {
                "abandoned"
            }
        }
        "abandoned" => "abandoned",
        _ => "idle",
    };

    Ok(WorkflowStage {
        stage: stage.to_string(),
        import_run_id: Some(run_id.to_string()),
    })
}
