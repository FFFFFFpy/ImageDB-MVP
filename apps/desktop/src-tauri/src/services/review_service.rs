use crate::domain::duplicate_group::{build_duplicate_groups, compute_excluded_ids, DuplicateEdge};
use crate::domain::import_state::{
    ImportAlbumState, ImportPlan, ImportPlanAlbum, ImportPlanImage, ImportRunState,
    ReviewCandidateDetail, ReviewCandidateSummary, ReviewDecisionAction, ReviewProgress,
};
use crate::domain::state_machine::PlanState;
use crate::error::AppError;
use crate::repositories::import_repository::{
    AlbumRow, ImportImageFullRow, ImportPlanCandidateRow, ImportPlanImageRow, ImportRepository,
    ImportRunRecord,
};
use base64::Engine;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tokio_postgres::Client;
use uuid::Uuid;

pub async fn get_review_queue(
    client: &Client,
    import_run_id: Uuid,
) -> Result<Vec<ReviewCandidateSummary>, AppError> {
    let rows = ImportRepository::get_review_candidates(client, import_run_id).await?;

    Ok(rows
        .into_iter()
        .map(|r| ReviewCandidateSummary {
            candidate_id: r.candidate_id.to_string(),
            source_image_id: r.source_image_id.to_string(),
            candidate_source_image_id: r.candidate_source_image_id.map(|id| id.to_string()),
            candidate_library_image_id: r.candidate_library_image_id.map(|id| id.to_string()),
            scope: r.scope,
            match_type: r.match_type,
            transform_type: r.transform_type,
            confidence: r.confidence,
            album_name: r.album_name,
            has_decision: r.has_decision,
        })
        .collect())
}

pub async fn get_review_detail(
    client: &Client,
    candidate_id: Uuid,
) -> Result<ReviewCandidateDetail, AppError> {
    let row = ImportRepository::get_review_candidate_detail(client, candidate_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("candidate {candidate_id} not found")))?;

    Ok(ReviewCandidateDetail {
        candidate_id: row.candidate_id.to_string(),
        source_image_id: row.source_image_id.to_string(),
        source_image_path: row.source_image_path,
        source_image_file_size: row.source_image_file_size,
        source_image_width: row.source_image_width,
        source_image_height: row.source_image_height,
        candidate_source_image_id: row.candidate_source_image_id.map(|id| id.to_string()),
        candidate_source_image_path: row.candidate_source_image_path,
        candidate_source_image_file_size: row.candidate_source_image_file_size,
        candidate_source_image_width: row.candidate_source_image_width,
        candidate_source_image_height: row.candidate_source_image_height,
        candidate_library_image_id: row.candidate_library_image_id.map(|id| id.to_string()),
        candidate_library_image_path: row.candidate_library_image_path,
        candidate_library_image_file_size: row.candidate_library_image_file_size,
        candidate_library_image_width: row.candidate_library_image_width,
        candidate_library_image_height: row.candidate_library_image_height,
        scope: row.scope,
        match_type: row.match_type,
        blake3_equal: row.blake3_equal,
        pixel_hash_equal: row.pixel_hash_equal,
        gradient_distance: row.gradient_distance,
        block_distance: row.block_distance,
        median_distance: row.median_distance,
        transform_type: row.transform_type,
        confidence: row.confidence,
        album_name: row.album_name,
        album_id: row.album_id.to_string(),
        existing_decision: row.existing_decision,
    })
}

pub async fn submit_decision(
    client: &Client,
    candidate_id: Uuid,
    action: ReviewDecisionAction,
) -> Result<(), AppError> {
    let row = ImportRepository::get_review_candidate_detail(client, candidate_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("candidate {candidate_id} not found")))?;

    let selected_image_id = match action {
        ReviewDecisionAction::KeepSource => Some(row.source_image_id),
        ReviewDecisionAction::KeepCandidate => row
            .candidate_source_image_id
            .or(row.candidate_library_image_id),
        ReviewDecisionAction::KeepAll | ReviewDecisionAction::SkipAlbum => None,
    };

    let decision_str = action.to_string();
    ImportRepository::insert_review_decision_once(
        client,
        candidate_id,
        &decision_str,
        selected_image_id,
        None,
    )
    .await?;
    ImportRepository::refresh_review_album_and_run(client, row.album_id).await?;
    Ok(())
}

pub async fn skip_album_candidates(
    client: &Client,
    import_run_id: Uuid,
    album_id: Uuid,
) -> Result<u32, AppError> {
    client
        .batch_execute("BEGIN")
        .await
        .map_err(|e| AppError::Internal(format!("failed to begin skip album transaction: {e}")))?;
    let result = async {
        let inserted = client
            .execute(
                "INSERT INTO review_decisions
                    (id, candidate_id, decision, selected_image_id, notes)
                 SELECT gen_random_uuid(), dc.id, 'skip_album', NULL, 'album skipped'
                 FROM duplicate_candidates dc
                 JOIN import_images source ON source.id = dc.source_image_id
                 LEFT JOIN review_decisions existing ON existing.candidate_id = dc.id
                 WHERE dc.import_run_id = $1
                   AND source.import_album_id = $2
                   AND dc.decision IS NULL
                   AND existing.id IS NULL
                 ON CONFLICT (candidate_id) DO NOTHING",
                &[&import_run_id, &album_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to skip album candidates: {e}")))?;

        ImportRepository::refresh_review_album_and_run(client, album_id).await?;
        Ok::<u32, AppError>(inserted as u32)
    }
    .await;

    match result {
        Ok(count) => {
            client.batch_execute("COMMIT").await.map_err(|e| {
                AppError::Internal(format!("failed to commit skip album transaction: {e}"))
            })?;
            Ok(count)
        }
        Err(e) => {
            let _ = client.batch_execute("ROLLBACK").await;
            Err(e)
        }
    }
}

pub async fn get_review_progress(
    client: &Client,
    import_run_id: Uuid,
) -> Result<ReviewProgress, AppError> {
    let row = ImportRepository::get_review_progress(client, import_run_id).await?;
    let remaining = row.total.saturating_sub(row.decided);

    Ok(ReviewProgress {
        import_run_id: import_run_id.to_string(),
        total_review_candidates: row.total,
        decided_count: row.decided,
        remaining_count: remaining,
        all_decided: remaining == 0,
    })
}

pub async fn generate_import_plan(
    client: &Client,
    import_run_id: Uuid,
) -> Result<ImportPlan, AppError> {
    // `freeze_import_plan` owns both the undecided-review guard and the
    // idempotent frozen-plan fast path. Keeping a single gate matters after
    // Commit advances the run: a second Generate request must return the
    // persisted projection instead of re-evaluating mutable review rows.
    freeze_import_plan(client, import_run_id).await
}

/// Freeze the import plan for a run in a single database transaction. Reads
/// the current review/candidate state, builds the plan, writes the three
/// plan tables + plan_hash + plan state `frozen`, and advances the run to
/// `ready_to_commit` — all atomically. Idempotent: a second call with an
/// existing frozen plan reuses it without rewriting rows. This is the
/// `freeze_import_plan` IPC entry point and the public main-chain freeze.
pub async fn freeze_import_plan(
    client: &Client,
    import_run_id: Uuid,
) -> Result<ImportPlan, AppError> {
    client
        .batch_execute("BEGIN")
        .await
        .map_err(|e| AppError::Internal(format!("failed to begin plan freeze: {e}")))?;

    let result = async {
        // The run row is the serialization point shared by Freeze, plan
        // editors, summary readers, and Commit. Re-check the idempotent path
        // only after taking it so a concurrent freeze/edit cannot interleave
        // the summary's multiple reads.
        lock_import_run_for_plan_access(client, import_run_id).await?;
        if let Some(existing) =
            ImportRepository::load_frozen_plan_summary(client, import_run_id).await?
        {
            return Ok(existing);
        }

        // Async review may expose candidates while the rest of the run is
        // still being analyzed. Never let that transiently-empty review queue
        // create a partial commit set.
        let import_run = require_freezable_import_run(client, import_run_id).await?;
        let progress = ImportRepository::get_review_progress(client, import_run_id).await?;
        let remaining = progress.total.saturating_sub(progress.decided);
        if remaining > 0 {
            return Err(AppError::Internal(format!(
                "cannot freeze import plan while {remaining} review candidates remain undecided"
            )));
        }

        let all_images =
            ImportRepository::get_all_import_images_with_album(client, import_run_id).await?;
        let all_candidates =
            ImportRepository::get_all_candidates_for_import_plan(client, import_run_id).await?;
        let albums = ImportRepository::get_albums_for_run(client, import_run_id).await?;
        let plan = build_import_plan(
            import_run_id.to_string(),
            &all_images,
            &all_candidates,
            &albums,
        );

        let kept_ids = parse_kept_image_ids(&plan)?;
        let full_images = ImportRepository::get_import_images_by_ids(client, &kept_ids).await?;
        let image_by_id: HashMap<Uuid, ImportImageFullRow> =
            full_images.into_iter().map(|img| (img.id, img)).collect();
        let mut kept_images_by_album: HashMap<Uuid, Vec<ImportImageFullRow>> = HashMap::new();
        for album in &albums {
            let album_images: Vec<ImportImageFullRow> = kept_ids
                .iter()
                .filter_map(|id| image_by_id.get(id).cloned())
                .filter(|img| img.import_album_id == album.id)
                .collect();
            if !album_images.is_empty() {
                kept_images_by_album.insert(album.id, album_images);
            }
        }

        // Compute the hash from the same draft shape Commit validates, then
        // write and re-read the frozen projection before releasing the lock.
        let draft =
            synthesize_draft_for_hash(import_run_id, &import_run, &albums, &kept_images_by_album);
        let plan_hash = crate::services::commit_service::compute_plan_hash(&draft)?;
        ImportRepository::write_frozen_import_plan_in_transaction(
            client,
            import_run_id,
            &albums,
            &kept_images_by_album,
            &import_run.policy_version,
            import_run.library_root_id,
            &plan_hash,
        )
        .await?;

        ImportRepository::load_frozen_plan_summary(client, import_run_id)
            .await?
            .ok_or_else(|| {
                AppError::Internal(format!(
                    "freeze did not produce a frozen plan for run {import_run_id}"
                ))
            })
    }
    .await;

    finish_plan_transaction(client, result, "plan freeze").await
}

async fn lock_import_run_for_plan_access(
    client: &Client,
    import_run_id: Uuid,
) -> Result<(), AppError> {
    client
        .query_opt(
            "SELECT id FROM import_runs WHERE id = $1 FOR UPDATE",
            &[&import_run_id],
        )
        .await
        .map_err(|e| AppError::Internal(format!("failed to lock import run: {e}")))?
        .ok_or_else(|| AppError::Internal(format!("import run {import_run_id} not found")))?;
    Ok(())
}

async fn require_freezable_import_run(
    client: &Client,
    import_run_id: Uuid,
) -> Result<ImportRunRecord, AppError> {
    let import_run = ImportRepository::get_import_run_by_id(client, import_run_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("import run {import_run_id} not found")))?;

    let review_required = ImportRunState::ReviewRequired.to_string();
    let ready_to_commit = ImportRunState::ReadyToCommit.to_string();
    if import_run.state != review_required && import_run.state != ready_to_commit {
        return Err(AppError::Internal(format!(
            "cannot freeze import plan for run {import_run_id} in state '{}'; expected {review_required} or {ready_to_commit}",
            import_run.state,
        )));
    }

    let allowed_album_states = vec![
        ImportAlbumState::Analyzed.to_string(),
        ImportAlbumState::ReviewRequired.to_string(),
    ];
    let invalid_album = client
        .query_opt(
            "SELECT source_name, state
             FROM import_albums
             WHERE import_run_id = $1
               AND NOT (state = ANY($2))
             ORDER BY source_name
             LIMIT 1",
            &[&import_run_id, &allowed_album_states],
        )
        .await
        .map_err(|e| {
            AppError::Internal(format!(
                "failed to validate album states before freezing import plan: {e}"
            ))
        })?;

    if let Some(row) = invalid_album {
        let source_name: String = row.get("source_name");
        let state: String = row.get("state");
        return Err(AppError::Internal(format!(
            "cannot freeze import plan while album '{source_name}' is in state '{state}'; expected {} or {}",
            allowed_album_states[0], allowed_album_states[1],
        )));
    }

    Ok(import_run)
}

/// Read the frozen plan summary for the commit-confirm page. Returns None
/// when no frozen plan exists yet. This is the `get_frozen_import_plan_summary`
/// IPC entry point — the commit page reads this instead of re-generating.
pub async fn get_frozen_plan_summary(
    client: &Client,
    import_run_id: Uuid,
) -> Result<Option<ImportPlan>, AppError> {
    client
        .batch_execute("BEGIN")
        .await
        .map_err(|e| AppError::Internal(format!("failed to begin frozen plan read: {e}")))?;
    let result = async {
        lock_import_run_for_plan_access(client, import_run_id).await?;
        ImportRepository::load_frozen_plan_summary(client, import_run_id).await
    }
    .await;
    finish_plan_transaction(client, result, "frozen plan read").await
}

pub async fn set_plan_album_included(
    client: &Client,
    import_run_id: Uuid,
    album_id: Uuid,
    included: bool,
) -> Result<ImportPlan, AppError> {
    client
        .batch_execute("BEGIN")
        .await
        .map_err(|e| AppError::Internal(format!("failed to begin plan edit: {e}")))?;

    let result = async {
        ensure_plan_editable(client, import_run_id).await?;
        let frozen = require_frozen_plan(client, import_run_id).await?;
        if included {
            include_album_images(client, frozen.plan_id, album_id).await?;
        } else {
            client
                .execute(
                    "DELETE FROM import_plan_albums WHERE plan_id = $1 AND import_album_id = $2",
                    &[&frozen.plan_id, &album_id],
                )
                .await
                .map_err(|e| AppError::Internal(format!("failed to skip plan album: {e}")))?;
        }
        refresh_plan_hash(client, import_run_id).await
    }
    .await;

    finish_plan_edit_transaction(client, result).await?;
    get_frozen_plan_summary(client, import_run_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("frozen plan {import_run_id} not found")))
}

pub async fn set_plan_image_included(
    client: &Client,
    import_run_id: Uuid,
    image_id: Uuid,
    target_album_id: Uuid,
    included: bool,
) -> Result<ImportPlan, AppError> {
    client
        .batch_execute("BEGIN")
        .await
        .map_err(|e| AppError::Internal(format!("failed to begin plan edit: {e}")))?;

    let result = async {
        ensure_plan_editable(client, import_run_id).await?;
        let frozen = require_frozen_plan(client, import_run_id).await?;
        if included {
            include_image_in_album(client, frozen.plan_id, image_id, target_album_id).await?;
        } else {
            client
                .execute(
                    "DELETE FROM import_plan_images WHERE import_image_id = $1 AND plan_album_id IN (
                        SELECT id FROM import_plan_albums WHERE plan_id = $2
                    )",
                    &[&image_id, &frozen.plan_id],
                )
                .await
                .map_err(|e| AppError::Internal(format!("failed to skip plan image: {e}")))?;
            delete_empty_plan_albums(client, frozen.plan_id).await?;
        }
        refresh_plan_hash(client, import_run_id).await
    }
    .await;

    finish_plan_edit_transaction(client, result).await?;
    get_frozen_plan_summary(client, import_run_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("frozen plan {import_run_id} not found")))
}

async fn ensure_plan_editable(client: &Client, import_run_id: Uuid) -> Result<(), AppError> {
    // Every plan editor calls this after BEGIN. The row lock is the per-run
    // serialization point shared with Commit's short plan-capture
    // transaction, closing the load/hash/prewrite TOCTOU window.
    let row = client
        .query_opt(
            "SELECT state FROM import_runs WHERE id = $1 FOR UPDATE",
            &[&import_run_id],
        )
        .await
        .map_err(|e| AppError::Internal(format!("failed to lock import run for plan edit: {e}")))?
        .ok_or_else(|| AppError::Internal(format!("import run {import_run_id} not found")))?;
    let run_state: String = row.get("state");
    if !matches!(run_state.as_str(), "ready_to_commit" | "cancelled") {
        return Err(AppError::Internal(format!(
            "cannot edit import plan while run is in state '{run_state}'"
        )));
    }

    let active_transactions: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM file_transactions WHERE import_run_id = $1",
            &[&import_run_id],
        )
        .await
        .map_err(|e| AppError::Internal(format!("failed to check plan transactions: {e}")))?
        .get(0);
    if active_transactions > 0 {
        return Err(AppError::Internal(
            "cannot edit import plan after commit transactions have been created".to_string(),
        ));
    }
    require_frozen_plan(client, import_run_id).await?;
    Ok(())
}

async fn require_frozen_plan(
    client: &Client,
    import_run_id: Uuid,
) -> Result<crate::repositories::import_repository::FrozenPlanRow, AppError> {
    let frozen = ImportRepository::load_frozen_plan(client, import_run_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("frozen plan {import_run_id} not found")))?;
    if frozen.plan_state != PlanState::Frozen.to_string() {
        return Err(AppError::Internal(format!(
            "plan for run {import_run_id} is not frozen"
        )));
    }
    Ok(frozen)
}

async fn include_album_images(
    client: &Client,
    plan_id: Uuid,
    album_id: Uuid,
) -> Result<(), AppError> {
    let album = ImportRepository::get_import_album_by_id(client, album_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("import album {album_id} not found")))?;
    let plan_album_id = ensure_plan_album(client, plan_id, album_id, &album.source_name).await?;
    let images = ImportRepository::get_import_images_by_album(client, album_id).await?;
    for img in images {
        let already_planned: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM import_plan_images ipi
                 JOIN import_plan_albums ipa ON ipa.id = ipi.plan_album_id
                 WHERE ipa.plan_id = $1 AND ipi.import_image_id = $2",
                &[&plan_id, &img.id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to check plan image: {e}")))?
            .get(0);
        if already_planned > 0 {
            continue;
        }
        let expected_blake3 = img.blake3.as_deref().ok_or_else(|| {
            AppError::Internal(format!(
                "cannot include image {} without BLAKE3 fingerprint",
                img.id
            ))
        })?;
        let target_relative_path =
            target_relative_path_for_album(&album.source_name, &img.relative_path)?;
        ImportRepository::insert_plan_image(
            client,
            plan_album_id,
            img.id,
            &img.source_path,
            &img.relative_path,
            &target_relative_path,
            img.file_size,
            expected_blake3,
            img.width,
            img.height,
            img.format.as_deref(),
        )
        .await?;
    }
    refresh_plan_album_count(client, plan_album_id).await
}

async fn include_image_in_album(
    client: &Client,
    plan_id: Uuid,
    image_id: Uuid,
    target_album_id: Uuid,
) -> Result<(), AppError> {
    let image = ImportRepository::get_import_images_by_ids(client, &[image_id])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Internal(format!("import image {image_id} not found")))?;
    if image.import_album_id != target_album_id {
        return Err(AppError::Internal(
            "cross-source album move is blocked by the current file transaction model".to_string(),
        ));
    }
    let album = ImportRepository::get_import_album_by_id(client, target_album_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("import album {target_album_id} not found")))?;
    let plan_album_id =
        ensure_plan_album(client, plan_id, target_album_id, &album.source_name).await?;
    client
        .execute(
            "DELETE FROM import_plan_images WHERE import_image_id = $1 AND plan_album_id IN (
                SELECT id FROM import_plan_albums WHERE plan_id = $2
            )",
            &[&image_id, &plan_id],
        )
        .await
        .map_err(|e| AppError::Internal(format!("failed to replace plan image: {e}")))?;
    let expected_blake3 = image.blake3.as_deref().ok_or_else(|| {
        AppError::Internal(format!(
            "cannot include image {} without BLAKE3 fingerprint",
            image.id
        ))
    })?;
    let target_relative_path =
        target_relative_path_for_album(&album.source_name, &image.relative_path)?;
    ImportRepository::insert_plan_image(
        client,
        plan_album_id,
        image.id,
        &image.source_path,
        &image.relative_path,
        &target_relative_path,
        image.file_size,
        expected_blake3,
        image.width,
        image.height,
        image.format.as_deref(),
    )
    .await?;
    refresh_plan_album_count(client, plan_album_id).await?;
    delete_empty_plan_albums(client, plan_id).await
}

async fn ensure_plan_album(
    client: &Client,
    plan_id: Uuid,
    album_id: Uuid,
    album_name: &str,
) -> Result<Uuid, AppError> {
    if let Some(row) = client
        .query_opt(
            "SELECT id FROM import_plan_albums WHERE plan_id = $1 AND import_album_id = $2",
            &[&plan_id, &album_id],
        )
        .await
        .map_err(|e| AppError::Internal(format!("failed to query plan album: {e}")))?
    {
        return Ok(row.get("id"));
    }
    ImportRepository::insert_plan_album(client, plan_id, album_id, album_name, 0).await
}

async fn refresh_plan_album_count(client: &Client, plan_album_id: Uuid) -> Result<(), AppError> {
    client
        .execute(
            "UPDATE import_plan_albums
             SET expected_image_count = (
                SELECT COUNT(*)::INTEGER FROM import_plan_images WHERE plan_album_id = $1
             )
             WHERE id = $1",
            &[&plan_album_id],
        )
        .await
        .map_err(|e| AppError::Internal(format!("failed to refresh plan album count: {e}")))?;
    Ok(())
}

async fn delete_empty_plan_albums(client: &Client, plan_id: Uuid) -> Result<(), AppError> {
    client
        .execute(
            "DELETE FROM import_plan_albums ipa
             WHERE ipa.plan_id = $1
             AND NOT EXISTS (
                SELECT 1 FROM import_plan_images ipi WHERE ipi.plan_album_id = ipa.id
             )",
            &[&plan_id],
        )
        .await
        .map_err(|e| AppError::Internal(format!("failed to delete empty plan albums: {e}")))?;
    Ok(())
}

async fn refresh_plan_hash(client: &Client, import_run_id: Uuid) -> Result<(), AppError> {
    let frozen = require_frozen_plan(client, import_run_id).await?;
    for (album, _) in &frozen.albums {
        refresh_plan_album_count(client, album.plan_album_id).await?;
    }
    let refreshed = require_frozen_plan(client, import_run_id).await?;
    let plan_hash = crate::services::commit_service::compute_plan_hash(&refreshed)?;
    ImportRepository::set_plan_hash(client, refreshed.plan_id, &plan_hash).await
}

async fn finish_plan_edit_transaction(
    client: &Client,
    result: Result<(), AppError>,
) -> Result<(), AppError> {
    finish_plan_transaction(client, result, "plan edit").await
}

async fn finish_plan_transaction<T>(
    client: &Client,
    result: Result<T, AppError>,
    operation: &str,
) -> Result<T, AppError> {
    match result {
        Ok(value) => {
            client
                .batch_execute("COMMIT")
                .await
                .map_err(|e| AppError::Internal(format!("failed to commit {operation}: {e}")))?;
            Ok(value)
        }
        Err(e) => {
            let _ = client.batch_execute("ROLLBACK").await;
            Err(e)
        }
    }
}

/// Build a transient `FrozenPlanRow` purely to compute the plan hash with
/// the same `compute_plan_hash` the commit pipeline uses for validation.
/// This keeps the freeze-time hash and commit-time validation identical
/// without persisting a draft first.
fn synthesize_draft_for_hash(
    import_run_id: Uuid,
    import_run: &crate::repositories::import_repository::ImportRunRecord,
    albums: &[AlbumRow],
    kept_images_by_album: &HashMap<Uuid, Vec<ImportImageFullRow>>,
) -> crate::repositories::import_repository::FrozenPlanRow {
    use crate::repositories::import_repository::{PlanAlbumRow, PlanImageRow};

    let plan_id = Uuid::nil();
    let mut album_rows: Vec<(PlanAlbumRow, Vec<PlanImageRow>)> = Vec::new();
    for album in albums {
        let Some(images) = kept_images_by_album.get(&album.id) else {
            continue;
        };
        if images.is_empty() {
            continue;
        }
        let plan_album_id = Uuid::nil();
        let plan_album = PlanAlbumRow {
            plan_album_id,
            import_album_id: album.id,
            target_relative_path: album.source_name.clone(),
            expected_image_count: images.len() as i32,
            album_plan_hash: None,
        };
        let mut plan_images: Vec<PlanImageRow> = Vec::new();
        for img in images {
            let target_relative_path =
                target_relative_path_for_album(&album.source_name, &img.relative_path)
                    .unwrap_or_else(|_| img.relative_path.clone());
            let expected_blake3 = img.blake3.clone().unwrap_or_default();
            plan_images.push(PlanImageRow {
                id: Uuid::nil(),
                plan_album_id,
                import_image_id: img.id,
                source_path: img.source_path.clone(),
                source_relative_path: img.relative_path.clone(),
                target_relative_path,
                expected_file_size: img.file_size,
                expected_blake3,
                width: img.width,
                height: img.height,
                format: img.format.clone(),
            });
        }
        album_rows.push((plan_album, plan_images));
    }

    crate::repositories::import_repository::FrozenPlanRow {
        plan_id,
        import_run_id,
        library_root_id: import_run.library_root_id,
        plan_state: PlanState::Draft.to_string(),
        plan_hash: None,
        policy_version: import_run.policy_version.clone(),
        albums: album_rows,
    }
}

fn parse_kept_image_ids(plan: &ImportPlan) -> Result<Vec<Uuid>, AppError> {
    plan.kept_images
        .iter()
        .map(|img| {
            Uuid::parse_str(&img.image_id).map_err(|e| {
                AppError::Internal(format!(
                    "invalid import plan image id {}: {e}",
                    img.image_id
                ))
            })
        })
        .collect()
}

fn target_relative_path_for_album(
    album_name: &str,
    source_relative_path: &str,
) -> Result<String, AppError> {
    let slash_prefix = format!("{album_name}/");
    let backslash_prefix = format!("{album_name}\\");
    let rel = source_relative_path
        .strip_prefix(&slash_prefix)
        .or_else(|| source_relative_path.strip_prefix(&backslash_prefix))
        .unwrap_or(source_relative_path);
    crate::services::commit_service::normalize_relative_path(rel)
}

pub fn build_import_plan(
    import_run_id: String,
    all_images: &[ImportPlanImageRow],
    all_candidates: &[ImportPlanCandidateRow],
    albums: &[AlbumRow],
) -> ImportPlan {
    let mut excluded_image_ids: HashSet<Uuid> = HashSet::new();
    let duplicate_album_resolution =
        detect_covered_import_albums(all_images, all_candidates, albums);
    let mut skipped_album_ids: HashSet<Uuid> = duplicate_album_resolution.skipped_album_ids.clone();

    let album_name_map: HashMap<Uuid, String> = albums
        .iter()
        .map(|a: &AlbumRow| (a.id, a.source_name.clone()))
        .collect();

    // Phase 1: Build duplicate groups from auto-duplicate candidates.
    let auto_edges: Vec<DuplicateEdge> = all_candidates
        .iter()
        .filter(|c| c.candidate_decision.as_deref() == Some("auto_duplicate"))
        .filter(|c| !duplicate_album_resolution.should_ignore_auto_edge(c))
        .filter_map(|c| {
            let candidate_id = c
                .candidate_source_image_id
                .or(c.candidate_library_image_id)?;
            Some(DuplicateEdge {
                image_a: c.source_image_id,
                image_b: candidate_id,
                a_is_import: true,
                b_is_import: c.candidate_library_image_id.is_none(),
                confidence: c.confidence.unwrap_or(0.5),
                blake3_equal: c.blake3_equal,
                pixel_hash_equal: c.pixel_hash_equal,
            })
        })
        .collect();

    let groups = build_duplicate_groups(&auto_edges);
    let auto_excluded = compute_excluded_ids(&groups);
    excluded_image_ids.extend(auto_excluded);

    // Phase 2: Apply review decisions.
    for c in all_candidates {
        if c.candidate_decision.is_some() {
            // Already handled by auto-grouping above.
            continue;
        }
        match c.review_decision.as_deref() {
            Some("keep_source") => {
                if c.scope == "intra_album" {
                    if let Some(cid) = c.candidate_source_image_id {
                        excluded_image_ids.insert(cid);
                    }
                }
            }
            Some("keep_candidate") => {
                if c.scope == "intra_album" || c.scope == "library" {
                    excluded_image_ids.insert(c.source_image_id);
                }
            }
            Some("keep_all") => {}
            Some("skip_album") => {
                skipped_album_ids.insert(c.source_album_id);
            }
            _ => {}
        }
    }

    let kept_images: Vec<ImportPlanImage> = all_images
        .iter()
        .filter(|img| {
            !excluded_image_ids.contains(&img.id) && !skipped_album_ids.contains(&img.album_id)
        })
        .map(|img: &ImportPlanImageRow| ImportPlanImage {
            image_id: img.id.to_string(),
            source_path: img.source_path.clone(),
            relative_path: img.relative_path.clone(),
            file_size: img.file_size,
            album_name: img.album_name.clone(),
            album_id: img.album_id.to_string(),
            source_album_id: img.album_id.to_string(),
            included: true,
        })
        .collect();

    let total_images = all_images.len() as u32;
    let mut skipped_album_names: Vec<String> = skipped_album_ids
        .iter()
        .filter_map(|id| album_name_map.get(id).cloned())
        .collect();
    skipped_album_names.sort();

    let kept_ids: HashSet<Uuid> = kept_images
        .iter()
        .filter_map(|img| Uuid::parse_str(&img.image_id).ok())
        .collect();
    let mut albums_out: Vec<ImportPlanAlbum> = albums
        .iter()
        .map(|album| {
            let mut images: Vec<ImportPlanImage> = all_images
                .iter()
                .filter(|img| img.album_id == album.id)
                .map(|img| {
                    let included = kept_ids.contains(&img.id);
                    ImportPlanImage {
                        image_id: img.id.to_string(),
                        source_path: img.source_path.clone(),
                        relative_path: img.relative_path.clone(),
                        file_size: img.file_size,
                        album_name: album.source_name.clone(),
                        album_id: album.id.to_string(),
                        source_album_id: img.album_id.to_string(),
                        included,
                    }
                })
                .collect();
            images.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
            let included = images.iter().any(|img| img.included);
            let image_count = images.iter().filter(|img| img.included).count() as u32;
            let total_size = images
                .iter()
                .filter(|img| img.included)
                .map(|img| img.file_size)
                .sum();
            ImportPlanAlbum {
                album_id: album.id.to_string(),
                album_name: album.source_name.clone(),
                included,
                image_count,
                total_size,
                images,
            }
        })
        .collect();
    albums_out.sort_by(|a, b| a.album_name.cmp(&b.album_name));

    ImportPlan {
        import_run_id,
        plan_hash: None,
        total_albums: albums.len() as u32,
        total_images,
        excluded_count: total_images.saturating_sub(kept_images.len() as u32),
        kept_images,
        skipped_albums: skipped_album_names,
        albums: albums_out,
    }
}

struct DuplicateAlbumResolution {
    skipped_album_ids: HashSet<Uuid>,
    skipped_image_ids: HashSet<Uuid>,
}

impl DuplicateAlbumResolution {
    fn empty() -> Self {
        Self {
            skipped_album_ids: HashSet::new(),
            skipped_image_ids: HashSet::new(),
        }
    }

    fn should_ignore_auto_edge(&self, candidate: &ImportPlanCandidateRow) -> bool {
        if self.skipped_image_ids.contains(&candidate.source_image_id) {
            return true;
        }
        let Some(candidate_source_image_id) = candidate.candidate_source_image_id else {
            return false;
        };
        self.skipped_image_ids.contains(&candidate_source_image_id)
    }
}

fn detect_covered_import_albums(
    all_images: &[ImportPlanImageRow],
    all_candidates: &[ImportPlanCandidateRow],
    albums: &[AlbumRow],
) -> DuplicateAlbumResolution {
    if albums.len() < 2 || all_images.is_empty() {
        return DuplicateAlbumResolution::empty();
    }

    let album_name_by_id: HashMap<Uuid, &str> = albums
        .iter()
        .map(|album| (album.id, album.source_name.as_str()))
        .collect();
    let mut image_album_by_id: HashMap<Uuid, Uuid> = HashMap::new();
    let mut image_ids_by_album: HashMap<Uuid, Vec<Uuid>> = HashMap::new();

    for img in all_images {
        image_album_by_id.insert(img.id, img.album_id);
        image_ids_by_album
            .entry(img.album_id)
            .or_default()
            .push(img.id);
    }

    let mut exact_cross_pairs: HashSet<(Uuid, Uuid)> = HashSet::new();
    for candidate in all_candidates {
        if candidate.candidate_decision.as_deref() != Some("auto_duplicate")
            || candidate.scope != "cross_album"
            || !candidate.blake3_equal
        {
            continue;
        }
        let Some(candidate_source_image_id) = candidate.candidate_source_image_id else {
            continue;
        };
        let Some(source_album_id) = image_album_by_id.get(&candidate.source_image_id) else {
            continue;
        };
        let Some(candidate_album_id) = image_album_by_id.get(&candidate_source_image_id) else {
            continue;
        };
        if source_album_id == candidate_album_id {
            continue;
        }
        exact_cross_pairs.insert(ordered_uuid_pair(
            candidate.source_image_id,
            candidate_source_image_id,
        ));
    }

    if exact_cross_pairs.is_empty() {
        return DuplicateAlbumResolution::empty();
    }

    for ids in image_ids_by_album.values_mut() {
        ids.sort();
    }

    let mut skipped_album_ids = HashSet::new();
    for i in 0..albums.len() {
        for j in (i + 1)..albums.len() {
            let album_a = albums[i].id;
            let album_b = albums[j].id;
            let Some(images_a) = image_ids_by_album.get(&album_a) else {
                continue;
            };
            let Some(images_b) = image_ids_by_album.get(&album_b) else {
                continue;
            };
            if images_a.is_empty() || images_b.is_empty() {
                continue;
            }

            let exact_match_count =
                exact_match_count_between_albums(images_a, images_b, &exact_cross_pairs);
            let a_covers_b =
                exact_match_count == images_b.len() && images_a.len() >= images_b.len();
            let b_covers_a =
                exact_match_count == images_a.len() && images_b.len() >= images_a.len();

            if a_covers_b && b_covers_a {
                let winner = preferred_album_for_equal_content(album_a, album_b, &album_name_by_id);
                let loser = if winner == album_a { album_b } else { album_a };
                skipped_album_ids.insert(loser);
            } else if a_covers_b {
                skipped_album_ids.insert(album_b);
            } else if b_covers_a {
                skipped_album_ids.insert(album_a);
            }
        }
    }

    if skipped_album_ids.is_empty() {
        return DuplicateAlbumResolution::empty();
    }

    let skipped_image_ids: HashSet<Uuid> = all_images
        .iter()
        .filter(|img| skipped_album_ids.contains(&img.album_id))
        .map(|img| img.id)
        .collect();

    DuplicateAlbumResolution {
        skipped_album_ids,
        skipped_image_ids,
    }
}

fn exact_match_count_between_albums(
    images_a: &[Uuid],
    images_b: &[Uuid],
    exact_cross_pairs: &HashSet<(Uuid, Uuid)>,
) -> usize {
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); images_a.len()];
    for (left_idx, image_a) in images_a.iter().enumerate() {
        for (right_idx, image_b) in images_b.iter().enumerate() {
            if exact_cross_pairs.contains(&ordered_uuid_pair(*image_a, *image_b)) {
                adjacency[left_idx].push(right_idx);
            }
        }
    }
    maximum_bipartite_matches(&adjacency, images_b.len())
}

fn maximum_bipartite_matches(adjacency: &[Vec<usize>], right_len: usize) -> usize {
    fn try_match(
        left_idx: usize,
        adjacency: &[Vec<usize>],
        seen_right: &mut [bool],
        matched_left_by_right: &mut [Option<usize>],
    ) -> bool {
        for &right_idx in &adjacency[left_idx] {
            if seen_right[right_idx] {
                continue;
            }
            seen_right[right_idx] = true;
            if matched_left_by_right[right_idx]
                .map(|other_left| {
                    try_match(other_left, adjacency, seen_right, matched_left_by_right)
                })
                .unwrap_or(true)
            {
                matched_left_by_right[right_idx] = Some(left_idx);
                return true;
            }
        }
        false
    }

    let mut matched_left_by_right = vec![None; right_len];
    let mut count = 0;
    for left_idx in 0..adjacency.len() {
        let mut seen_right = vec![false; right_len];
        if try_match(
            left_idx,
            adjacency,
            &mut seen_right,
            &mut matched_left_by_right,
        ) {
            count += 1;
        }
    }
    count
}

fn preferred_album_for_equal_content(
    album_a: Uuid,
    album_b: Uuid,
    album_name_by_id: &HashMap<Uuid, &str>,
) -> Uuid {
    let name_a = album_name_by_id.get(&album_a).copied().unwrap_or("");
    let name_b = album_name_by_id.get(&album_b).copied().unwrap_or("");
    if (name_a, album_a) <= (name_b, album_b) {
        album_a
    } else {
        album_b
    }
}

fn ordered_uuid_pair(a: Uuid, b: Uuid) -> (Uuid, Uuid) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// Decode an image from disk, cap its decoded pixel count, downscale to a
/// size-limited thumbnail, and re-encode as JPEG. Returns a data URL. Never
/// returns the full-resolution original.
fn render_thumbnail(
    path: &Path,
    max_dim: u32,
    max_pixels: u64,
    max_source_bytes: u64,
) -> Result<String, AppError> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > max_source_bytes {
        return Err(AppError::IoError(format!(
            "image too large to preview: {} ({} bytes > {})",
            path.display(),
            metadata.len(),
            max_source_bytes
        )));
    }

    // Validate the format by extension before reading, so a non-image file
    // (e.g. a renamed executable) is rejected cheaply.
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let _mime = match ext.as_str() {
        "jpg" | "jpeg" | "png" | "webp" => ext.clone(),
        _ => {
            return Err(AppError::Internal(format!(
                "unsupported image format for preview: {}",
                path.display()
            )));
        }
    };

    let bytes = std::fs::read(path)
        .map_err(|e| AppError::IoError(format!("failed to read image {}: {e}", path.display())))?;
    let reader = std::io::Cursor::new(bytes);
    let img = image::ImageReader::new(reader)
        .with_guessed_format()
        .map_err(|e| AppError::ImageError(format!("cannot inspect image: {e}")))?
        .decode()
        .map_err(|e| AppError::ImageError(format!("corrupt or undecodable image: {e}")))?;

    // Cap decoded pixels so a maliciously huge-but-valid image cannot exhaust
    // memory during the resize.
    let (w, h) = (img.width() as u64, img.height() as u64);
    if w.saturating_mul(h) > max_pixels {
        return Err(AppError::ImageError(format!(
            "decoded image too large for preview: {w}x{h} (>{max_pixels} pixels)"
        )));
    }

    // Downscale so neither dimension exceeds max_dim.
    let thumb = if img.width() > max_dim || img.height() > max_dim {
        img.resize(max_dim, max_dim, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };

    let mut buf = std::io::Cursor::new(Vec::new());
    thumb
        .write_to(&mut buf, image::ImageFormat::Jpeg)
        .map_err(|e| AppError::ImageError(format!("thumbnail encode failed: {e}")))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(buf.into_inner());
    Ok(format!("data:image/jpeg;base64,{b64}"))
}

/// Allowed-roots check: a preview path must canonicalize to a location inside
/// the candidate's source root or library root. This blocks path-escape
/// attacks (e.g. a DB row pointing at /etc/passwd or ..\\..\\secrets).
fn path_within_allowed_roots(resolved: &Path, allowed: &[PathBuf]) -> Result<(), AppError> {
    let canon = resolved.canonicalize().map_err(|e| {
        AppError::IoError(format!("cannot canonicalize {}: {e}", resolved.display()))
    })?;
    for root in allowed {
        let root_canon = match root.canonicalize() {
            Ok(c) => c,
            Err(_) => continue, // a root that doesn't exist can't be matched
        };
        if canon.starts_with(&root_canon) {
            return Ok(());
        }
    }
    Err(AppError::Internal(format!(
        "preview path {} is outside the candidate's allowed source/library roots",
        resolved.display()
    )))
}

/// Load an image preview for a review candidate, restricted to persisted records.
///
/// The `image_side` parameter determines which image to preview:
/// - "source": the source image (import_image referenced by candidate)
/// - "candidate": the candidate image (import_image or library_image)
///
/// This function validates that:
/// 1. The candidate exists in the database.
/// 2. The image_side is valid.
/// 3. The resolved path canonicalizes inside the candidate's source root or
///    library root (no path escape).
/// 4. The file is a supported image format.
/// 5. The source file size is within limits.
/// 6. The decoded pixel count is within limits.
/// 7. A size-limited JPEG thumbnail is returned, never the full-resolution
///    original.
pub async fn load_image_preview_by_candidate(
    client: &Client,
    candidate_id: Uuid,
    image_side: &str,
) -> Result<String, AppError> {
    use crate::repositories::import_repository::ImportRepository;
    let detail = ImportRepository::get_review_candidate_detail(client, candidate_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("candidate {candidate_id} not found")))?;

    let (path, is_library_candidate) = match image_side {
        "source" => (PathBuf::from(&detail.source_image_path), false),
        "candidate" => {
            if let Some(ref p) = detail.candidate_source_image_path {
                (PathBuf::from(p), false)
            } else if let Some(ref p) = detail.candidate_library_image_path {
                (PathBuf::from(p), true)
            } else {
                return Err(AppError::Internal(format!(
                    "candidate {candidate_id} has no candidate image path"
                )));
            }
        }
        _ => {
            return Err(AppError::Internal(format!(
                "invalid image_side: {image_side}; expected 'source' or 'candidate'"
            )));
        }
    };

    // Build the allowed roots: the import run's source_root (for import images)
    // and the library root of the candidate library image (for library images).
    let mut allowed: Vec<PathBuf> = Vec::new();
    // Import run source root.
    let run_row = client
        .query_opt(
            "SELECT ir.source_root FROM import_runs ir
             JOIN import_albums ia ON ia.import_run_id = ir.id
             JOIN import_images ii ON ii.import_album_id = ia.id
             WHERE ii.id = $1",
            &[&detail.source_image_id],
        )
        .await
        .map_err(|e| AppError::Internal(format!("failed to query import run root: {e}")))?;
    if let Some(row) = run_row {
        let source_root: String = row.get("source_root");
        allowed.push(PathBuf::from(source_root));
    }
    if is_library_candidate {
        // Library image path is already resolved as root/album_rel/img_rel in
        // get_review_candidate_detail; its allowed root is the library root,
        // which is the parent of the album-relative path. We add the path's
        // own album directory and the broader library root by querying it.
        if let Some(lib_img_id) = detail.candidate_library_image_id {
            let lib_row = client
                .query_opt(
                    "SELECT lr.path AS root_path
                     FROM library_images li
                     JOIN library_albums la ON la.id = li.album_id
                     JOIN library_roots lr ON lr.id = la.library_root_id
                     WHERE li.id = $1",
                    &[&lib_img_id],
                )
                .await
                .map_err(|e| AppError::Internal(format!("failed to query library root: {e}")))?;
            if let Some(row) = lib_row {
                let root_path: String = row.get("root_path");
                allowed.push(PathBuf::from(root_path));
            }
        }
    }

    // Path escape check.
    path_within_allowed_roots(&path, &allowed)?;

    // Validate the path exists.
    if !path.exists() {
        return Err(AppError::IoError(format!(
            "image file not found: {}",
            path.display()
        )));
    }

    render_thumbnail(
        &path,
        PREVIEW_MAX_DIMENSION,
        PREVIEW_MAX_PIXELS,
        PREVIEW_MAX_SOURCE_BYTES,
    )
}

pub async fn load_image_preview_by_import_image(
    client: &Client,
    import_run_id: Uuid,
    image_id: Uuid,
) -> Result<String, AppError> {
    let row = client
        .query_opt(
            "SELECT ii.source_path, ir.source_root
             FROM import_images ii
             JOIN import_albums ia ON ia.id = ii.import_album_id
             JOIN import_runs ir ON ir.id = ia.import_run_id
             WHERE ir.id = $1 AND ii.id = $2",
            &[&import_run_id, &image_id],
        )
        .await
        .map_err(|e| AppError::Internal(format!("failed to query import image preview: {e}")))?
        .ok_or_else(|| {
            AppError::Internal(format!(
                "import image {image_id} does not belong to run {import_run_id}"
            ))
        })?;
    let path = PathBuf::from(row.get::<_, String>("source_path"));
    let source_root = PathBuf::from(row.get::<_, String>("source_root"));
    path_within_allowed_roots(&path, &[source_root])?;
    if !path.exists() {
        return Err(AppError::IoError(format!(
            "image file not found: {}",
            path.display()
        )));
    }
    render_thumbnail(
        &path,
        PREVIEW_MAX_DIMENSION,
        PREVIEW_MAX_PIXELS,
        PREVIEW_MAX_SOURCE_BYTES,
    )
}

/// Maximum dimension (px) of a generated preview thumbnail.
const PREVIEW_MAX_DIMENSION: u32 = 800;
/// Maximum decoded pixel count for a preview source.
const PREVIEW_MAX_PIXELS: u64 = 50_000_000;
/// Maximum source file size (bytes) for a preview.
const PREVIEW_MAX_SOURCE_BYTES: u64 = 100 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn path_within_allowed_roots_rejects_escape() {
        let tmp = TempDir::new().unwrap();
        let allowed_root = tmp.path().join("src");
        std::fs::create_dir_all(&allowed_root).unwrap();
        let inside = allowed_root.join("a.jpg");
        std::fs::write(&inside, b"x").unwrap();
        let outside = tmp.path().join("secret.txt");
        std::fs::write(&outside, b"x").unwrap();

        assert!(path_within_allowed_roots(&inside, std::slice::from_ref(&allowed_root)).is_ok());
        assert!(
            path_within_allowed_roots(&outside, &[allowed_root]).is_err(),
            "path outside allowed root must be rejected"
        );
    }

    #[test]
    fn path_within_allowed_roots_rejects_traversal() {
        let tmp = TempDir::new().unwrap();
        let allowed_root = tmp.path().join("src");
        std::fs::create_dir_all(&allowed_root).unwrap();
        // A symlink-free traversal: ../secret relative to src.
        let secret = tmp.path().join("secret.txt");
        std::fs::write(&secret, b"x").unwrap();
        let escaped = allowed_root.join("..").join("secret.txt");
        assert!(
            path_within_allowed_roots(&escaped, &[allowed_root]).is_err(),
            "traversal escape must be rejected"
        );
    }

    #[test]
    fn render_thumbnail_rejects_non_image() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("not_image.jpg");
        std::fs::write(&p, b"this is not a real jpeg").unwrap();
        let result = render_thumbnail(&p, 800, 50_000_000, 100 * 1024 * 1024);
        assert!(result.is_err(), "non-image file must be rejected");
    }

    #[test]
    fn render_thumbnail_rejects_corrupt_image() {
        let tmp = TempDir::new().unwrap();
        // Valid extension, garbage content.
        let p = tmp.path().join("corrupt.png");
        std::fs::write(&p, b"\x89PNG\r\n\x1a\nGARBAGE").unwrap();
        let result = render_thumbnail(&p, 800, 50_000_000, 100 * 1024 * 1024);
        assert!(result.is_err(), "corrupt image must be rejected");
    }

    #[test]
    fn render_thumbnail_returns_small_jpeg_data_url() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("ok.png");
        image::RgbImage::new(2000, 2000).save(&p).unwrap();
        let url = render_thumbnail(&p, 800, 50_000_000, 100 * 1024 * 1024).unwrap();
        assert!(url.starts_with("data:image/jpeg;base64,"));
        // Decoded thumbnail bytes must be much smaller than the original.
        let b64 = &url["data:image/jpeg;base64,".len()..];
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert!(
            bytes.len() < 2000 * 2000 * 3,
            "thumbnail must be downscaled"
        );
    }

    #[test]
    fn render_thumbnail_rejects_unsupported_format() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("doc.txt");
        std::fs::write(&p, b"hello").unwrap();
        let result = render_thumbnail(&p, 800, 50_000_000, 100 * 1024 * 1024);
        assert!(result.is_err());
    }

    #[test]
    fn render_thumbnail_rejects_oversized_source() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("big.png");
        std::fs::write(&p, b"x").unwrap();
        // max_source_bytes = 0 → any non-empty file is too large.
        let result = render_thumbnail(&p, 800, 50_000_000, 0);
        assert!(result.is_err(), "oversized source must be rejected");
    }

    #[test]
    fn review_decision_action_display_parse() {
        let actions = [
            ReviewDecisionAction::KeepSource,
            ReviewDecisionAction::KeepCandidate,
            ReviewDecisionAction::KeepAll,
            ReviewDecisionAction::SkipAlbum,
        ];
        for a in actions {
            assert_eq!(ReviewDecisionAction::from_str_opt(&a.to_string()), Some(a));
        }
    }

    #[test]
    fn review_decision_rejects_unknown() {
        assert_eq!(ReviewDecisionAction::from_str_opt("unknown"), None);
        assert_eq!(ReviewDecisionAction::from_str_opt(""), None);
    }

    fn make_image(id: Uuid, album_id: Uuid, name: &str) -> ImportPlanImageRow {
        make_image_in_album(id, album_id, "album_a", name)
    }

    fn make_image_in_album(
        id: Uuid,
        album_id: Uuid,
        album_name: &str,
        relative_name: &str,
    ) -> ImportPlanImageRow {
        ImportPlanImageRow {
            id,
            source_path: format!("/src/{album_name}/{relative_name}"),
            relative_path: format!("{album_name}/{relative_name}"),
            file_size: 1000,
            album_id,
            album_name: album_name.to_string(),
        }
    }

    fn make_album(id: Uuid, name: &str) -> AlbumRow {
        AlbumRow {
            id,
            source_name: name.to_string(),
        }
    }

    #[test]
    fn plan_excludes_auto_duplicates() {
        let album_id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let img_a = Uuid::parse_str("00000000-0000-0000-0000-000000000010").unwrap();
        let img_b = Uuid::parse_str("00000000-0000-0000-0000-000000000020").unwrap();
        let cand_id = Uuid::parse_str("00000000-0000-0000-0000-000000000100").unwrap();

        let images = vec![
            make_image(img_a, album_id, "a.jpg"),
            make_image(img_b, album_id, "b.jpg"),
        ];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_b,
            candidate_source_image_id: Some(img_a),
            candidate_library_image_id: None,
            scope: "intra_album".to_string(),
            candidate_decision: Some("auto_duplicate".to_string()),
            review_decision: None,
            source_album_id: album_id,
            blake3_equal: true,
            pixel_hash_equal: true,
            confidence: Some(1.0),
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 1);
        assert_eq!(plan.kept_images[0].image_id, img_a.to_string());
        assert_eq!(plan.excluded_count, 1);
    }

    #[test]
    fn plan_keeps_one_complete_album_for_exact_duplicate_album_copies() {
        let album_a = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let album_b = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        let a_001 = Uuid::parse_str("00000000-0000-0000-0000-000000000020").unwrap();
        let b_001 = Uuid::parse_str("00000000-0000-0000-0000-000000000010").unwrap();
        let a_002 = Uuid::parse_str("00000000-0000-0000-0000-000000000030").unwrap();
        let b_002 = Uuid::parse_str("00000000-0000-0000-0000-000000000040").unwrap();

        let images = vec![
            make_image_in_album(a_001, album_a, "album_a", "001.jpg"),
            make_image_in_album(a_002, album_a, "album_a", "002.jpg"),
            make_image_in_album(b_001, album_b, "album_b_copy", "001.jpg"),
            make_image_in_album(b_002, album_b, "album_b_copy", "002.jpg"),
        ];
        let candidates = vec![
            ImportPlanCandidateRow {
                candidate_id: Uuid::parse_str("00000000-0000-0000-0000-000000000100").unwrap(),
                source_image_id: a_001,
                candidate_source_image_id: Some(b_001),
                candidate_library_image_id: None,
                scope: "cross_album".to_string(),
                candidate_decision: Some("auto_duplicate".to_string()),
                review_decision: None,
                source_album_id: album_a,
                blake3_equal: true,
                pixel_hash_equal: true,
                confidence: Some(1.0),
            },
            ImportPlanCandidateRow {
                candidate_id: Uuid::parse_str("00000000-0000-0000-0000-000000000101").unwrap(),
                source_image_id: a_002,
                candidate_source_image_id: Some(b_002),
                candidate_library_image_id: None,
                scope: "cross_album".to_string(),
                candidate_decision: Some("auto_duplicate".to_string()),
                review_decision: None,
                source_album_id: album_a,
                blake3_equal: true,
                pixel_hash_equal: true,
                confidence: Some(1.0),
            },
        ];
        let albums = vec![
            make_album(album_a, "album_a"),
            make_album(album_b, "album_b_copy"),
        ];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        let kept_ids: HashSet<&str> = plan
            .kept_images
            .iter()
            .map(|img| img.image_id.as_str())
            .collect();
        assert_eq!(kept_ids.len(), 2);
        assert!(kept_ids.contains(a_001.to_string().as_str()));
        assert!(kept_ids.contains(a_002.to_string().as_str()));
        assert_eq!(plan.skipped_albums, vec!["album_b_copy".to_string()]);
        assert_eq!(plan.excluded_count, 2);
    }

    #[test]
    fn plan_keeps_superset_album_when_one_album_contains_another() {
        let album_a = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let album_b = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        let a_extra = Uuid::parse_str("00000000-0000-0000-0000-000000000010").unwrap();
        let a_copy_001 = Uuid::parse_str("00000000-0000-0000-0000-000000000030").unwrap();
        let a_copy_002 = Uuid::parse_str("00000000-0000-0000-0000-000000000040").unwrap();
        let b_001 = Uuid::parse_str("00000000-0000-0000-0000-000000000020").unwrap();
        let b_002 = Uuid::parse_str("00000000-0000-0000-0000-000000000050").unwrap();

        let images = vec![
            make_image_in_album(a_extra, album_a, "album_a", "cover.jpg"),
            make_image_in_album(a_copy_001, album_a, "album_a", "copied_b/001.jpg"),
            make_image_in_album(a_copy_002, album_a, "album_a", "copied_b/002.jpg"),
            make_image_in_album(b_001, album_b, "album_b", "001.jpg"),
            make_image_in_album(b_002, album_b, "album_b", "002.jpg"),
        ];
        let candidates = vec![
            ImportPlanCandidateRow {
                candidate_id: Uuid::parse_str("00000000-0000-0000-0000-000000000110").unwrap(),
                source_image_id: a_copy_001,
                candidate_source_image_id: Some(b_001),
                candidate_library_image_id: None,
                scope: "cross_album".to_string(),
                candidate_decision: Some("auto_duplicate".to_string()),
                review_decision: None,
                source_album_id: album_a,
                blake3_equal: true,
                pixel_hash_equal: true,
                confidence: Some(1.0),
            },
            ImportPlanCandidateRow {
                candidate_id: Uuid::parse_str("00000000-0000-0000-0000-000000000111").unwrap(),
                source_image_id: a_copy_002,
                candidate_source_image_id: Some(b_002),
                candidate_library_image_id: None,
                scope: "cross_album".to_string(),
                candidate_decision: Some("auto_duplicate".to_string()),
                review_decision: None,
                source_album_id: album_a,
                blake3_equal: true,
                pixel_hash_equal: true,
                confidence: Some(1.0),
            },
        ];
        let albums = vec![
            make_album(album_a, "album_a"),
            make_album(album_b, "album_b"),
        ];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        let kept_ids: HashSet<&str> = plan
            .kept_images
            .iter()
            .map(|img| img.image_id.as_str())
            .collect();
        assert_eq!(kept_ids.len(), 3);
        assert!(kept_ids.contains(a_extra.to_string().as_str()));
        assert!(kept_ids.contains(a_copy_001.to_string().as_str()));
        assert!(kept_ids.contains(a_copy_002.to_string().as_str()));
        assert_eq!(plan.skipped_albums, vec!["album_b".to_string()]);
        assert_eq!(plan.excluded_count, 2);
    }

    #[test]
    fn plan_keep_source_excludes_candidate_intra_album() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let img_b = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![
            make_image(img_a, album_id, "a.jpg"),
            make_image(img_b, album_id, "b.jpg"),
        ];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: Some(img_b),
            candidate_library_image_id: None,
            scope: "intra_album".to_string(),
            candidate_decision: None,
            review_decision: Some("keep_source".to_string()),
            source_album_id: album_id,
            blake3_equal: false,
            pixel_hash_equal: false,
            confidence: None,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 1);
        assert_eq!(plan.kept_images[0].image_id, img_a.to_string());
    }

    #[test]
    fn plan_keep_candidate_excludes_source_intra_album() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let img_b = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![
            make_image(img_a, album_id, "a.jpg"),
            make_image(img_b, album_id, "b.jpg"),
        ];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: Some(img_b),
            candidate_library_image_id: None,
            scope: "intra_album".to_string(),
            candidate_decision: None,
            review_decision: Some("keep_candidate".to_string()),
            source_album_id: album_id,
            blake3_equal: false,
            pixel_hash_equal: false,
            confidence: None,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 1);
        assert_eq!(plan.kept_images[0].image_id, img_b.to_string());
    }

    #[test]
    fn plan_keep_all_keeps_both() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let img_b = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![
            make_image(img_a, album_id, "a.jpg"),
            make_image(img_b, album_id, "b.jpg"),
        ];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: Some(img_b),
            candidate_library_image_id: None,
            scope: "intra_album".to_string(),
            candidate_decision: None,
            review_decision: Some("keep_all".to_string()),
            source_album_id: album_id,
            blake3_equal: false,
            pixel_hash_equal: false,
            confidence: None,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 2);
        assert_eq!(plan.excluded_count, 0);
    }

    #[test]
    fn plan_skip_album_excludes_all_images_in_album() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let img_b = Uuid::new_v4();
        let img_c = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![
            make_image(img_a, album_id, "a.jpg"),
            make_image(img_b, album_id, "b.jpg"),
            make_image(img_c, album_id, "c.jpg"),
        ];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: Some(img_b),
            candidate_library_image_id: None,
            scope: "intra_album".to_string(),
            candidate_decision: None,
            review_decision: Some("skip_album".to_string()),
            source_album_id: album_id,
            blake3_equal: false,
            pixel_hash_equal: false,
            confidence: None,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 0);
        assert_eq!(plan.excluded_count, 3);
        assert_eq!(plan.skipped_albums, vec!["album_a".to_string()]);
    }

    #[test]
    fn plan_library_scope_keep_source_does_not_exclude_library() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![make_image(img_a, album_id, "a.jpg")];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: None,
            candidate_library_image_id: None,
            scope: "library".to_string(),
            candidate_decision: None,
            review_decision: Some("keep_source".to_string()),
            source_album_id: album_id,
            blake3_equal: false,
            pixel_hash_equal: false,
            confidence: None,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 1);
        assert_eq!(plan.excluded_count, 0);
    }

    #[test]
    fn plan_library_scope_keep_candidate_excludes_source() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![make_image(img_a, album_id, "a.jpg")];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: None,
            candidate_library_image_id: None,
            scope: "library".to_string(),
            candidate_decision: None,
            review_decision: Some("keep_candidate".to_string()),
            source_album_id: album_id,
            blake3_equal: false,
            pixel_hash_equal: false,
            confidence: None,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 0);
        assert_eq!(plan.excluded_count, 1);
    }

    #[test]
    fn plan_undecided_review_candidate_not_excluded() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let img_b = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![
            make_image(img_a, album_id, "a.jpg"),
            make_image(img_b, album_id, "b.jpg"),
        ];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: Some(img_b),
            candidate_library_image_id: None,
            scope: "intra_album".to_string(),
            candidate_decision: None,
            review_decision: None,
            source_album_id: album_id,
            blake3_equal: false,
            pixel_hash_equal: false,
            confidence: None,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 2);
    }

    #[test]
    fn plan_empty_run() {
        let plan = build_import_plan("run-1".to_string(), &[], &[], &[]);
        assert_eq!(plan.kept_images.len(), 0);
        assert_eq!(plan.total_albums, 0);
        assert_eq!(plan.total_images, 0);
        assert_eq!(plan.excluded_count, 0);
    }

    /// Real PostgreSQL review integration test.
    ///
    /// Invocation:
    ///   IMAGEDB_POSTGRES_BIN=/path/to/pgsql/bin cargo test \
    ///       --manifest-path apps/desktop/src-tauri/Cargo.toml \
    ///       --features real-db-tests real_review_decision_persists_and_filters_plan \
    ///       -- --ignored --test-threads=1
    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_review_decision_persists_and_filters_plan() {
        use crate::domain::import_state::{
            DecodeState, DuplicateScope, ImportImageState, MatchType,
        };
        use crate::infrastructure::postgres::{MigrationRunner, PostgresManager};
        use crate::repositories::import_repository::{NewDuplicateCandidate, NewImportImage};
        use tempfile::TempDir;

        if std::env::var("IMAGEDB_POSTGRES_BIN")
            .unwrap_or_default()
            .is_empty()
        {
            panic!(
                "IMAGEDB_POSTGRES_BIN is not set; cannot run the real review integration test. \
                 Set IMAGEDB_POSTGRES_BIN to a PostgreSQL 18.x bin directory, or run \
                 `node scripts/package-postgres-runtime.mjs` to populate the packaged runtime \
                 at .local/db-tools/postgresql-18.4/pgsql/bin."
            );
        }

        let tmp = TempDir::new().unwrap();
        let app_data = tmp.path().join("app_data");
        let source_root = tmp.path().join("source");
        let album_path = source_root.join("album_a");
        let album_b_path = source_root.join("album_b");
        std::fs::create_dir_all(&album_path).unwrap();
        std::fs::create_dir_all(&album_b_path).unwrap();
        std::fs::write(album_path.join("source.png"), b"source").unwrap();
        std::fs::write(album_path.join("candidate.png"), b"candidate").unwrap();
        std::fs::write(album_b_path.join("source.png"), b"source-b").unwrap();
        std::fs::write(album_b_path.join("candidate.png"), b"candidate-b").unwrap();

        let mut manager = PostgresManager::new(&app_data);
        assert!(manager.binaries_available());
        let probe = manager.initialize().await.unwrap();
        assert!(probe.connection_ok, "diagnostics: {:?}", probe.diagnostics);
        assert!(
            probe.pgvector_available,
            "diagnostics: {:?}",
            probe.diagnostics
        );

        let (mut client, db_handle) = manager.connect().await.unwrap();
        MigrationRunner::run_pending(&mut client).await.unwrap();

        let library_root_id = ImportRepository::upsert_default_library_root(&client)
            .await
            .unwrap();
        let import_run_id = ImportRepository::create_import_run(
            &client,
            &source_root.display().to_string(),
            library_root_id,
        )
        .await
        .unwrap();
        let album_id = ImportRepository::insert_import_album(
            &client,
            import_run_id,
            &album_path.display().to_string(),
            "album_a",
        )
        .await
        .unwrap();
        let album_b_id = ImportRepository::insert_import_album(
            &client,
            import_run_id,
            &album_b_path.display().to_string(),
            "album_b",
        )
        .await
        .unwrap();

        let source_id = ImportRepository::insert_import_image(
            &client,
            NewImportImage {
                album_id,
                source_path: album_path.join("source.png").display().to_string(),
                relative_path: "album_a/source.png".to_string(),
                file_size: 6,
                modified_at: None,
                width: Some(10),
                height: Some(10),
                format: Some("png".to_string()),
                decode_state: DecodeState::Decoded,
                blake3: Some(vec![1; 32]),
                pixel_hash: Some(vec![1; 8]),
                gradient_hash: Some(vec![1; 8]),
                block_hash: Some(vec![1; 8]),
                median_hash: Some(vec![1; 8]),
                fingerprint_version: Some("test".to_string()),
                state: ImportImageState::Fingerprinted,
            },
        )
        .await
        .unwrap();
        let candidate_id = ImportRepository::insert_import_image(
            &client,
            NewImportImage {
                album_id,
                source_path: album_path.join("candidate.png").display().to_string(),
                relative_path: "album_a/candidate.png".to_string(),
                file_size: 9,
                modified_at: None,
                width: Some(10),
                height: Some(10),
                format: Some("png".to_string()),
                decode_state: DecodeState::Decoded,
                blake3: Some(vec![2; 32]),
                pixel_hash: Some(vec![2; 8]),
                gradient_hash: Some(vec![2; 8]),
                block_hash: Some(vec![2; 8]),
                median_hash: Some(vec![2; 8]),
                fingerprint_version: Some("test".to_string()),
                state: ImportImageState::Fingerprinted,
            },
        )
        .await
        .unwrap();

        let review_candidate_id = ImportRepository::insert_duplicate_candidate(
            &client,
            NewDuplicateCandidate {
                import_run_id,
                source_image_id: source_id,
                candidate_source_image_id: Some(candidate_id),
                candidate_library_image_id: None,
                scope: DuplicateScope::IntraAlbum,
                match_type: MatchType::PerceptualSimilar,
                blake3_equal: false,
                pixel_hash_equal: false,
                gradient_distance: Some(10),
                block_distance: Some(11),
                median_distance: Some(12),
                transform_type: Some("identity".to_string()),
                confidence: Some(0.75),
                decision: None,
                decision_source: None,
            },
        )
        .await
        .unwrap();
        let source_b_id = ImportRepository::insert_import_image(
            &client,
            NewImportImage {
                album_id: album_b_id,
                source_path: album_b_path.join("source.png").display().to_string(),
                relative_path: "album_b/source.png".to_string(),
                file_size: 8,
                modified_at: None,
                width: Some(10),
                height: Some(10),
                format: Some("png".to_string()),
                decode_state: DecodeState::Decoded,
                blake3: Some(vec![3; 32]),
                pixel_hash: Some(vec![3; 8]),
                gradient_hash: Some(vec![3; 8]),
                block_hash: Some(vec![3; 8]),
                median_hash: Some(vec![3; 8]),
                fingerprint_version: Some("test".to_string()),
                state: ImportImageState::Fingerprinted,
            },
        )
        .await
        .unwrap();
        let candidate_b_id = ImportRepository::insert_import_image(
            &client,
            NewImportImage {
                album_id: album_b_id,
                source_path: album_b_path.join("candidate.png").display().to_string(),
                relative_path: "album_b/candidate.png".to_string(),
                file_size: 11,
                modified_at: None,
                width: Some(10),
                height: Some(10),
                format: Some("png".to_string()),
                decode_state: DecodeState::Decoded,
                blake3: Some(vec![4; 32]),
                pixel_hash: Some(vec![4; 8]),
                gradient_hash: Some(vec![4; 8]),
                block_hash: Some(vec![4; 8]),
                median_hash: Some(vec![4; 8]),
                fingerprint_version: Some("test".to_string()),
                state: ImportImageState::Fingerprinted,
            },
        )
        .await
        .unwrap();
        let failed_image_id = ImportRepository::insert_import_image(
            &client,
            NewImportImage {
                album_id: album_b_id,
                source_path: album_b_path.join("broken.png").display().to_string(),
                relative_path: "album_b/broken.png".to_string(),
                file_size: 7,
                modified_at: None,
                width: None,
                height: None,
                format: None,
                decode_state: DecodeState::Failed,
                blake3: None,
                pixel_hash: None,
                gradient_hash: None,
                block_hash: None,
                median_hash: None,
                fingerprint_version: None,
                state: ImportImageState::Failed,
            },
        )
        .await
        .unwrap();
        ImportRepository::insert_duplicate_candidate(
            &client,
            NewDuplicateCandidate {
                import_run_id,
                source_image_id: source_b_id,
                candidate_source_image_id: Some(candidate_b_id),
                candidate_library_image_id: None,
                scope: DuplicateScope::IntraAlbum,
                match_type: MatchType::PerceptualSimilar,
                blake3_equal: false,
                pixel_hash_equal: false,
                gradient_distance: Some(9),
                block_distance: Some(9),
                median_distance: Some(9),
                transform_type: Some("identity".to_string()),
                confidence: Some(0.7),
                decision: None,
                decision_source: None,
            },
        )
        .await
        .unwrap();
        ImportRepository::mark_import_album_analyzing(&client, album_id)
            .await
            .unwrap();
        ImportRepository::finalize_import_album_analysis(&client, album_id)
            .await
            .unwrap();
        ImportRepository::mark_import_album_analyzing(&client, album_b_id)
            .await
            .unwrap();
        ImportRepository::finalize_import_album_analysis(&client, album_b_id)
            .await
            .unwrap();

        ImportRepository::update_import_run_state(
            &client,
            import_run_id,
            &ImportRunState::Analyzing,
        )
        .await
        .unwrap();
        let error = freeze_import_plan(&client, import_run_id)
            .await
            .expect_err("an in-flight run must block plan freezing");
        assert!(
            error.to_string().contains("state 'analyzing'"),
            "expected in-flight run state in freeze error, got: {error}"
        );

        ImportRepository::update_import_run_state(
            &client,
            import_run_id,
            &ImportRunState::ReviewRequired,
        )
        .await
        .unwrap();

        for invalid_state in [
            ImportAlbumState::Analyzing,
            ImportAlbumState::Pending,
            ImportAlbumState::Failed,
        ] {
            ImportRepository::update_import_album_state(&client, album_b_id, &invalid_state)
                .await
                .unwrap();

            let error = freeze_import_plan(&client, import_run_id)
                .await
                .expect_err("an incomplete album must block plan freezing");
            let message = error.to_string();
            assert!(
                message.contains(&invalid_state.to_string()),
                "expected invalid album state in freeze error, got: {message}"
            );

            let plan_count: i64 = client
                .query_one(
                    "SELECT COUNT(*) FROM import_plans WHERE import_run_id = $1",
                    &[&import_run_id],
                )
                .await
                .unwrap()
                .get(0);
            assert_eq!(plan_count, 0, "rejected freeze must not persist a plan");

            ImportRepository::mark_import_album_analyzing(&client, album_b_id)
                .await
                .unwrap();
            ImportRepository::finalize_import_album_analysis(&client, album_b_id)
                .await
                .unwrap();
        }

        let queue = get_review_queue(&client, import_run_id).await.unwrap();
        assert_eq!(queue.len(), 2);
        assert!(!queue[0].has_decision);

        let blocked_plan = generate_import_plan(&client, import_run_id).await;
        assert!(blocked_plan.is_err());

        submit_decision(
            &client,
            review_candidate_id,
            ReviewDecisionAction::KeepSource,
        )
        .await
        .unwrap();
        let album_a_status = ImportRepository::get_import_album_status_by_id(&client, album_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(album_a_status.review_candidate_count, 0);
        assert_eq!(album_a_status.state, "analyzed");
        submit_decision(
            &client,
            review_candidate_id,
            ReviewDecisionAction::KeepSource,
        )
        .await
        .unwrap();
        let conflicting = submit_decision(
            &client,
            review_candidate_id,
            ReviewDecisionAction::KeepCandidate,
        )
        .await;
        assert!(conflicting.is_err());

        client
            .batch_execute(
                "CREATE FUNCTION imagedb_test_reject_skip() RETURNS trigger LANGUAGE plpgsql AS $$
                 BEGIN
                     RAISE EXCEPTION 'injected skip failure';
                 END $$;
                 CREATE TRIGGER imagedb_test_reject_skip_trigger
                 BEFORE INSERT ON review_decisions
                 FOR EACH ROW WHEN (NEW.decision = 'skip_album')
                 EXECUTE FUNCTION imagedb_test_reject_skip();",
            )
            .await
            .unwrap();
        let injected = skip_album_candidates(&client, import_run_id, album_b_id).await;
        assert!(injected.is_err(), "injected skip failure must surface");
        let decisions_after_rollback: i64 = client
            .query_one(
                "SELECT COUNT(*)
                 FROM review_decisions rd
                 JOIN duplicate_candidates dc ON dc.id = rd.candidate_id
                 JOIN import_images ii ON ii.id = dc.source_image_id
                 WHERE ii.import_album_id = $1",
                &[&album_b_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(decisions_after_rollback, 0);
        let album_b_after_rollback =
            ImportRepository::get_import_album_status_by_id(&client, album_b_id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(album_b_after_rollback.review_candidate_count, 1);
        client
            .batch_execute(
                "DROP TRIGGER imagedb_test_reject_skip_trigger ON review_decisions;
                 DROP FUNCTION imagedb_test_reject_skip();",
            )
            .await
            .unwrap();

        let skipped = skip_album_candidates(&client, import_run_id, album_b_id)
            .await
            .unwrap();
        assert_eq!(skipped, 1);
        let album_b_status = ImportRepository::get_import_album_status_by_id(&client, album_b_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(album_b_status.review_candidate_count, 0);
        assert_eq!(album_b_status.state, "analyzed");

        let progress = get_review_progress(&client, import_run_id).await.unwrap();
        assert_eq!(progress.total_review_candidates, 2);
        assert_eq!(progress.decided_count, 2);
        assert!(progress.all_decided);

        let queue = get_review_queue(&client, import_run_id).await.unwrap();
        assert_eq!(queue.len(), 2);
        assert!(queue[0].has_decision);

        let (freeze_client_a, freeze_handle_a) = manager.connect().await.unwrap();
        let (freeze_client_b, freeze_handle_b) = manager.connect().await.unwrap();
        let (plan_a, plan_b) = tokio::join!(
            generate_import_plan(&freeze_client_a, import_run_id),
            generate_import_plan(&freeze_client_b, import_run_id),
        );
        drop(freeze_client_a);
        drop(freeze_client_b);
        freeze_handle_a.abort();
        freeze_handle_b.abort();
        let plan = plan_a.expect("first concurrent freeze must succeed");
        let concurrent_plan = plan_b.expect("second concurrent freeze must reuse the frozen plan");
        assert_eq!(concurrent_plan.total_images, plan.total_images);
        assert_eq!(concurrent_plan.excluded_count, plan.excluded_count);
        assert_eq!(concurrent_plan.kept_images, plan.kept_images);
        let frozen_plan_count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM import_plans WHERE import_run_id = $1",
                &[&import_run_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(
            frozen_plan_count, 1,
            "concurrent freeze calls must persist exactly one plan"
        );
        assert_eq!(plan.total_images, 4);
        assert!(
            plan.albums
                .iter()
                .flat_map(|album| &album.images)
                .all(|image| image.image_id != failed_image_id.to_string()),
            "failed images without a fingerprint must not enter the frozen plan"
        );
        assert_eq!(plan.kept_images.len(), 1);
        assert_eq!(plan.kept_images[0].image_id, source_id.to_string());
        assert_eq!(plan.excluded_count, 3);

        let reloaded_plan = get_frozen_plan_summary(&client, import_run_id)
            .await
            .unwrap()
            .expect("generated plan must be reloadable");
        assert_eq!(reloaded_plan.total_images, plan.total_images);
        assert_eq!(reloaded_plan.excluded_count, plan.excluded_count);
        assert_eq!(reloaded_plan.kept_images, plan.kept_images);

        // Commit and plan edits serialize on the import_run row. Simulate the
        // short Commit capture transaction holding that lock and publishing
        // the guarded state transition; a concurrent editor must wait, then
        // reject without changing persisted plan rows.
        let plan_image_count_before: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM import_plan_images ipi
                 JOIN import_plan_albums ipa ON ipa.id = ipi.plan_album_id
                 JOIN import_plans ip ON ip.id = ipa.plan_id
                 WHERE ip.import_run_id = $1",
                &[&import_run_id],
            )
            .await
            .unwrap()
            .get(0);
        client.batch_execute("BEGIN").await.unwrap();
        client
            .query_one(
                "SELECT id FROM import_runs WHERE id = $1 FOR UPDATE",
                &[&import_run_id],
            )
            .await
            .unwrap();
        client
            .execute(
                "UPDATE import_runs SET state = 'committing' WHERE id = $1",
                &[&import_run_id],
            )
            .await
            .unwrap();

        let (edit_client, edit_handle) = manager.connect().await.unwrap();
        let mut edit_task = tokio::spawn(async move {
            let result =
                set_plan_album_included(&edit_client, import_run_id, album_id, false).await;
            drop(edit_client);
            edit_handle.abort();
            result
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut edit_task)
                .await
                .is_err(),
            "plan editor must block while Commit holds the run lock"
        );
        client.batch_execute("COMMIT").await.unwrap();
        let edit_error = edit_task
            .await
            .expect("plan edit task panicked")
            .expect_err("plan edit must reject the committed 'committing' state");
        assert!(edit_error.to_string().contains("state 'committing'"));
        let plan_image_count_after: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM import_plan_images ipi
                 JOIN import_plan_albums ipa ON ipa.id = ipi.plan_album_id
                 JOIN import_plans ip ON ip.id = ipa.plan_id
                 WHERE ip.import_run_id = $1",
                &[&import_run_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(plan_image_count_after, plan_image_count_before);

        let reloaded_while_committing = freeze_import_plan(&client, import_run_id)
            .await
            .expect("an existing frozen plan must remain readable after Commit advances the run");
        assert_eq!(reloaded_while_committing.total_images, plan.total_images);
        assert_eq!(
            reloaded_while_committing.excluded_count,
            plan.excluded_count
        );
        assert_eq!(reloaded_while_committing.kept_images, plan.kept_images);

        drop(client);
        db_handle.abort();
        manager.shutdown().await.unwrap();
    }
}
