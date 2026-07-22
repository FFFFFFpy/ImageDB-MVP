use crate::error::AppError;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkflowStage {
    pub stage: String,
    pub import_run_id: Option<String>,
}

/// Resolve the current import workflow stage by selecting the highest-priority
/// actionable run. Priority: committing > recovery_required > ready_to_commit >
/// cancelled > review_required > analysis states. Completed/failed runs are not
/// actionable and never shadow an in-progress workflow.
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
                      AND ia.state IN ('pending', 'analyzing', 'failed')
                ) AS analysis_complete
             FROM import_runs r
             WHERE r.state IN (
                 'committing', 'recovery_required', 'ready_to_commit',
                 'review_required',
                 'created', 'scanning', 'fingerprinting', 'detecting_duplicates', 'analyzing'
             )
             OR (
                 r.state = 'cancelled'
                 AND EXISTS (
                     SELECT 1 FROM import_plans p2
                     WHERE p2.import_run_id = r.id
                       AND p2.state IN ('frozen', 'consumed')
                       AND p2.plan_hash IS NOT NULL
                 )
                 AND NOT EXISTS (
                     SELECT 1 FROM file_transactions ft2
                     WHERE ft2.import_run_id = r.id
                 )
             )
             ORDER BY CASE r.state
                 WHEN 'committing' THEN 0
                 WHEN 'recovery_required' THEN 1
                 WHEN 'ready_to_commit' THEN 2
                 WHEN 'cancelled' THEN 3
                 WHEN 'review_required' THEN 4
                 ELSE 5
             END, r.started_at DESC
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
        "cancelled" => {
            if has_frozen_plan && !has_transactions {
                "commit_confirm"
            } else {
                "idle"
            }
        }
        _ => "idle",
    };

    Ok(WorkflowStage {
        stage: stage.to_string(),
        import_run_id: Some(run_id.to_string()),
    })
}

/// Resolve the workflow stage for a specific import run by ID. Used by pages
/// that already carry a runId in the URL and need to restore the correct phase
/// after a page refresh (e.g. CommitPage must show "committing" not "confirm").
pub async fn resolve_workflow_stage_for_run(
    client: &Client,
    run_id: Uuid,
) -> Result<WorkflowStage, AppError> {
    let row = client
        .query_opt(
            "SELECT r.state,
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
                      AND ia.state IN ('pending', 'analyzing', 'failed')
                ) AS analysis_complete
             FROM import_runs r
             WHERE r.id = $1",
            &[&run_id],
        )
        .await
        .map_err(|e| AppError::Internal(format!("failed to resolve workflow stage for run: {e}")))?;

    let Some(row) = row else {
        return Ok(WorkflowStage {
            stage: "idle".to_string(),
            import_run_id: None,
        });
    };

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
                "idle"
            }
        }
        _ => "idle",
    };

    Ok(WorkflowStage {
        stage: stage.to_string(),
        import_run_id: Some(run_id.to_string()),
    })
}
