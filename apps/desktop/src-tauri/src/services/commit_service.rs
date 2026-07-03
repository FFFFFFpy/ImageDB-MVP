//! Formal import commit pipeline.
//!
//! This is the *only* path that writes to the library. It reads its commit set
//! exclusively from a frozen immutable plan (`import_plans` /
//! `import_plan_albums` / `import_plan_images`), prewrites every file operation
//! before any byte is copied, streams each file to a `.part` with incremental
//! BLAKE3, verifies, writes a manifest, then publishes the whole staging album
//! directory with a single atomic rename, commits the DB records in one
//! transaction, and finally archives the source.
//!
//! Recovery is handled by [`crate::services::recovery_service`], which resumes
//! from the persisted transaction/operation state. Idempotency is decided by
//! [`verify_complete_evidence`], which checks transaction id, plan id, plan
//! hash, manifest hash, the on-disk directory + manifest, every file's path /
//! size / BLAKE3, and the DB album + image records — not just a row count.
use crate::domain::import_state::{
    CommitAlbumResult, CommitProgress, CommitResult, ImportRunState,
};
use crate::domain::state_machine::{self, FileOpState, PlanState, TransactionState};
use crate::error::AppError;
use crate::infrastructure::postgres::PostgresManager;
use crate::repositories::import_repository::{
    FrozenPlanRow, ImportRepository, PlanAlbumRow, PlanImageRow, SnapshotFileRecord,
};
use crate::services::source_snapshot_service::{
    load_source_album_snapshot, verify_source_snapshot_files,
};
#[cfg(feature = "fail-injection")]
use crate::tests::fail_injection::{maybe_fault, CommitFaultPoint};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio_postgres::Client;
use uuid::Uuid;

/// Schema version written into every album manifest.
pub const MANIFEST_SCHEMA_VERSION: &str = "1.0";
/// Maximum decoded pixel count for a single source file preview.
#[allow(dead_code)]
pub const PREVIEW_MAX_PIXELS: u64 = 8_000_000;
/// Maximum source file size (bytes) for a single source file preview.
#[allow(dead_code)]
pub const PREVIEW_MAX_SOURCE_BYTES: u64 = 80 * 1024 * 1024;

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Canonical on-disk manifest for a published album directory.
///
/// This struct is serialized into `.imagedb-manifest.json` inside both the
/// staging album dir and the published album dir. The published manifest is
/// the authoritative evidence used by idempotency verification and recovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlbumManifest {
    pub schema_version: String,
    pub transaction_id: String,
    pub plan_id: String,
    pub plan_hash: String,
    pub import_run_id: String,
    pub import_album_id: String,
    pub library_root_id: String,
    pub album_relative_path: String,
    pub image_count: u32,
    pub images: Vec<AlbumManifestImage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlbumManifestImage {
    pub relative_path: String,
    pub source_path: String,
    pub file_size: i64,
    pub blake3: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub format: Option<String>,
    pub fingerprint_version: Option<String>,
}

/// A single album ready to commit, derived from the frozen plan.
struct PlanAlbumCommit {
    plan_album: PlanAlbumRow,
    images: Vec<PlanImageRow>,
}

/// Validate a target relative path for use inside the library root.
///
/// Rejects absolute paths, `..` traversal, Windows drive letters, and empty
/// components. Normalizes separators to `/`. Returns the normalized path.
pub(crate) fn normalize_relative_path(rel: &str) -> Result<String, AppError> {
    let p = Path::new(rel);
    let mut parts: Vec<String> = Vec::new();
    for comp in p.components() {
        match comp {
            Component::Normal(os) => {
                let s = os.to_string_lossy();
                if s.is_empty() {
                    return Err(AppError::Internal(format!(
                        "empty path component in '{rel}'"
                    )));
                }
                parts.push(s.to_string());
            }
            Component::CurDir => {} // skip "."
            Component::ParentDir => {
                return Err(AppError::Internal(format!(
                    "target relative path escapes its root ('..'): {rel}"
                )));
            }
            Component::RootDir => {
                return Err(AppError::Internal(format!(
                    "target relative path must not be absolute: {rel}"
                )));
            }
            Component::Prefix(_) => {
                return Err(AppError::Internal(format!(
                    "target relative path must not contain a drive prefix: {rel}"
                )));
            }
        }
    }
    if parts.is_empty() {
        return Err(AppError::Internal(format!(
            "target relative path is empty: {rel}"
        )));
    }
    Ok(parts.join("/"))
}

/// Detect case conflicts and duplicate target paths within one album.
fn check_target_path_conflicts(images: &[PlanImageRow]) -> Result<(), AppError> {
    let mut seen: HashMap<String, String> = HashMap::new(); // lowercased -> original
    for img in images {
        let normalized = normalize_relative_path(&img.target_relative_path)?;
        let lower = normalized.to_lowercase();
        if let Some(prev) = seen.get(&lower) {
            if prev != &normalized {
                return Err(AppError::Internal(format!(
                    "target path case conflict: '{prev}' vs '{normalized}'"
                )));
            }
            return Err(AppError::Internal(format!(
                "duplicate target relative path in plan: {normalized}"
            )));
        }
        seen.insert(lower, normalized);
    }
    Ok(())
}

/// Read and parse a published manifest from disk.
pub(crate) fn read_manifest(dir: &Path) -> Result<AlbumManifest, AppError> {
    let manifest_path = dir.join(".imagedb").join(".imagedb-manifest.json");
    let json = std::fs::read_to_string(&manifest_path).map_err(|e| {
        AppError::Internal(format!(
            "cannot read manifest {}: {e}",
            manifest_path.display()
        ))
    })?;
    serde_json::from_str(&json)
        .map_err(|e| AppError::Internal(format!("cannot parse manifest: {e}")))
}

pub async fn run_import_commit(
    postgres_manager: Arc<Mutex<PostgresManager>>,
    library_root_path: String,
    import_run_id: Uuid,
    cancelled: Arc<AtomicBool>,
    progress_tracker: Arc<Mutex<CommitProgress>>,
) -> Result<CommitResult, AppError> {
    let mut progress = progress_tracker.lock().await;
    progress.state = "running".to_string();
    progress.current_stage = "preparing".to_string();
    progress.import_run_id = import_run_id.to_string();
    drop(progress);

    let (mut client, db_handle) = {
        let mgr = postgres_manager.lock().await;
        mgr.connect()
            .await
            .map_err(|e| AppError::Internal(format!("failed to connect for commit: {e}")))?
    };

    let result = execute_commit_pipeline(
        &mut client,
        &library_root_path,
        import_run_id,
        &cancelled,
        &progress_tracker,
    )
    .await;

    drop(client);
    db_handle.abort();

    let mut progress = progress_tracker.lock().await;
    match &result {
        Ok(r) => {
            progress.state = r.state.clone();
            progress.current_stage = "done".to_string();
        }
        Err(e) => {
            progress.state = "failed".to_string();
            progress.current_stage = "failed".to_string();
            progress.errors.push(e.to_string());
        }
    }
    drop(progress);
    result
}

async fn execute_commit_pipeline(
    client: &mut Client,
    library_root_path: &str,
    import_run_id: Uuid,
    cancelled: &Arc<AtomicBool>,
    progress_tracker: &Arc<Mutex<CommitProgress>>,
) -> Result<CommitResult, AppError> {
    let import_run = ImportRepository::get_import_run_by_id(client, import_run_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("import run {import_run_id} not found")))?;

    let library_root_id = import_run.library_root_id;

    // Rule 5: never overwrite the bound library root with the current settings path.
    // The bound root's path is read from the DB at commit time. If the current
    // settings point at a different root, the user must switch roots explicitly.
    let bound_root_path = ImportRepository::get_library_root_path(client, library_root_id).await?;
    let bound = PathBuf::from(&bound_root_path);
    let current = PathBuf::from(library_root_path);
    if !path_eq(&bound, &current) {
        return Err(AppError::Internal(format!(
            "current settings library root '{}' does not match the import run's bound library root '{}' (id {library_root_id}); \
             switch library roots instead of overwriting the bound root",
            current.display(),
            bound.display()
        )));
    }
    let library_root = bound;

    // Rule 3: the frozen plan is the sole commit source of truth.
    let frozen = ImportRepository::load_frozen_plan(client, import_run_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!(
            "no frozen import plan for run {import_run_id}; generate and freeze a plan before committing"
        )))?;

    if frozen.albums.is_empty() {
        // Rule: empty plan completes the run directly.
        ImportRepository::update_import_run_state(
            client,
            import_run_id,
            &ImportRunState::Completed,
        )
        .await?;
        return Ok(CommitResult {
            import_run_id: import_run_id.to_string(),
            albums_total: 0,
            albums_committed: 0,
            albums_skipped: 0,
            albums_failed: 0,
            images_committed: 0,
            album_results: Vec::new(),
            errors: Vec::new(),
            state: "completed".to_string(),
        });
    }

    // Rule 4: validate and hash the immutable plan up front. Commit and
    // recovery both use this function so a tampered plan fails consistently.
    let validated_plan_hash = validate_and_hash_frozen_plan(&frozen, library_root_id)?;

    {
        let mut p = progress_tracker.lock().await;
        p.current_stage = "committing".to_string();
        p.albums_total = frozen.albums.len() as u32;
    }
    ImportRepository::update_import_run_state(client, import_run_id, &ImportRunState::Committing)
        .await?;

    let mut album_results = Vec::new();
    let mut total_committed = 0u32;
    let mut albums_committed = 0u32;
    let mut albums_skipped = 0u32;
    let mut albums_failed = 0u32;
    let mut all_errors = Vec::new();
    let mut recovery_required_detected = false;
    let mut cleanup_required_detected = false;

    for (plan_album, images) in &frozen.albums {
        if cancelled.load(Ordering::Relaxed) {
            all_errors.push("commit cancelled by user".to_string());
            break;
        }

        {
            let mut p = progress_tracker.lock().await;
            p.current_album = Some(plan_album.target_relative_path.clone());
            p.current_stage = "processing_album".to_string();
        }

        let commit = PlanAlbumCommit {
            plan_album: plan_album.clone(),
            images: images.clone(),
        };

        match commit_single_album(
            client,
            &library_root,
            library_root_id,
            import_run_id,
            frozen.plan_id,
            &validated_plan_hash,
            cancelled,
            commit,
        )
        .await
        {
            Ok(result) => {
                if result.status == "skipped" {
                    albums_skipped += 1;
                } else {
                    albums_committed += 1;
                    total_committed += result.images_committed;
                }
                if result.status == "cleanup_required" {
                    cleanup_required_detected = true;
                    if let Some(error) = &result.error {
                        all_errors.push(error.clone());
                    }
                }
                album_results.push(result);
            }
            Err(e) => {
                if let AppError::ResumeRequired(transaction_id) = e {
                    recovery_required_detected = true;
                    let msg = format!(
                        "detected incomplete transaction {transaction_id}; route to recovery"
                    );
                    all_errors.push(msg.clone());
                    album_results.push(CommitAlbumResult {
                        album_name: plan_album.target_relative_path.clone(),
                        status: "recovery_required".to_string(),
                        images_committed: 0,
                        target_path: None,
                        manifest_path: None,
                        error: Some(msg),
                    });
                    continue;
                }
                albums_failed += 1;
                let err_msg = format!("album {}: {e}", plan_album.target_relative_path);
                all_errors.push(err_msg.clone());
                album_results.push(CommitAlbumResult {
                    album_name: plan_album.target_relative_path.clone(),
                    status: "failed".to_string(),
                    images_committed: 0,
                    target_path: None,
                    manifest_path: None,
                    error: Some(err_msg),
                });
            }
        }

        {
            let mut p = progress_tracker.lock().await;
            p.albums_completed = albums_committed + albums_skipped + albums_failed;
            p.albums_skipped = albums_skipped;
            p.albums_failed = albums_failed;
            p.images_committed = total_committed;
            p.errors = all_errors.clone();
        }
    }

    let final_state = if recovery_required_detected || cleanup_required_detected {
        ImportRepository::update_import_run_state(
            client,
            import_run_id,
            &ImportRunState::RecoveryRequired,
        )
        .await?;
        "recovery_required".to_string()
    } else if albums_failed == 0 && !cancelled.load(Ordering::Relaxed) {
        ImportRepository::update_import_run_state(
            client,
            import_run_id,
            &ImportRunState::Completed,
        )
        .await?;
        "completed".to_string()
    } else if cancelled.load(Ordering::Relaxed) {
        ImportRepository::update_import_run_state(
            client,
            import_run_id,
            &ImportRunState::RecoveryRequired,
        )
        .await?;
        "cancelled_pending_recovery".to_string()
    } else {
        ImportRepository::update_import_run_error(
            client,
            import_run_id,
            "commit_partial",
            &format!(
                "{albums_failed} album(s) failed, {} error(s)",
                all_errors.len()
            ),
        )
        .await?;
        ImportRepository::update_import_run_state(
            client,
            import_run_id,
            &ImportRunState::RecoveryRequired,
        )
        .await?;
        "completed_with_errors".to_string()
    };

    Ok(CommitResult {
        import_run_id: import_run_id.to_string(),
        albums_total: frozen.albums.len() as u32,
        albums_committed,
        albums_skipped,
        albums_failed,
        images_committed: total_committed,
        album_results,
        errors: all_errors,
        state: final_state,
    })
}

/// Rule 4: validate the frozen/consumed immutable plan and return its verified
/// hash. Any inconsistency rejects the whole plan — never silently drop entries
/// or substitute an empty hash.
pub(crate) fn validate_and_hash_frozen_plan(
    frozen: &FrozenPlanRow,
    expected_library_root_id: Uuid,
) -> Result<Vec<u8>, AppError> {
    match PlanState::parse(&frozen.plan_state)? {
        PlanState::Frozen | PlanState::Consumed => {}
        other => {
            return Err(AppError::Internal(format!(
                "plan {} is in state {other}; expected frozen or consumed",
                frozen.plan_id
            )));
        }
    }
    if frozen.plan_hash.is_none() {
        return Err(AppError::Internal(format!(
            "plan {} has no plan_hash; cannot commit or recover",
            frozen.plan_id
        )));
    }
    if frozen.library_root_id != expected_library_root_id {
        return Err(AppError::Internal(format!(
            "frozen plan library_root_id {} != import run library_root_id {expected_library_root_id}",
            frozen.library_root_id
        )));
    }
    for (album, images) in &frozen.albums {
        // Every album must resolve to a valid relative path.
        let _ = normalize_relative_path(&album.target_relative_path)?;
        check_target_path_conflicts(images)?;
        // Expected image count must match the actual persisted image rows.
        if album.expected_image_count != images.len() as i32 {
            return Err(AppError::Internal(format!(
                "plan album '{}' expected_image_count={} but {} image rows persisted",
                album.target_relative_path,
                album.expected_image_count,
                images.len()
            )));
        }
        for img in images {
            if img.expected_blake3.len() != 32 {
                return Err(AppError::Internal(format!(
                    "plan image '{}' has invalid expected_blake3 length {} (expected 32)",
                    img.target_relative_path,
                    img.expected_blake3.len()
                )));
            }
            if img.expected_file_size < 0 {
                return Err(AppError::Internal(format!(
                    "plan image '{}' has negative expected_file_size {}",
                    img.target_relative_path, img.expected_file_size
                )));
            }
        }
    }
    let recomputed = compute_plan_hash(frozen)?;
    let stored = frozen.plan_hash.as_ref().expect("checked above");
    if stored != &recomputed {
        return Err(AppError::Internal(format!(
            "frozen plan hash mismatch: stored {} but recomputed {} - plan rows were modified after freezing",
            bytes_to_hex(stored),
            bytes_to_hex(&recomputed)
        )));
    }
    Ok(recomputed)
}

/// Compute a deterministic BLAKE3 hash over the canonical serialized plan
/// content. The canonical form sorts albums and images by target path and
/// includes all fields that define the commit set.
pub(crate) fn compute_plan_hash(frozen: &FrozenPlanRow) -> Result<Vec<u8>, AppError> {
    let mut canonical: Vec<u8> = Vec::new();
    canonical.extend_from_slice(frozen.import_run_id.as_bytes());
    canonical.extend_from_slice(frozen.library_root_id.as_bytes());
    canonical.extend_from_slice(frozen.policy_version.as_bytes());

    // Albums + images sorted by normalized target path.
    let mut albums: Vec<&(PlanAlbumRow, Vec<PlanImageRow>)> = frozen.albums.iter().collect();
    albums.sort_by(|a, b| a.0.target_relative_path.cmp(&b.0.target_relative_path));
    for (album, images) in &albums {
        canonical.extend_from_slice(album.import_album_id.as_bytes());
        canonical.extend_from_slice(album.target_relative_path.as_bytes());
        canonical.extend_from_slice(&album.expected_image_count.to_le_bytes());
        let mut imgs: Vec<&PlanImageRow> = images.iter().collect();
        imgs.sort_by(|a, b| a.target_relative_path.cmp(&b.target_relative_path));
        for img in imgs {
            canonical.extend_from_slice(img.import_image_id.as_bytes());
            canonical.extend_from_slice(img.source_path.as_bytes());
            canonical.extend_from_slice(img.target_relative_path.as_bytes());
            canonical.extend_from_slice(&img.expected_file_size.to_le_bytes());
            canonical.extend_from_slice(&img.expected_blake3);
        }
    }
    Ok(blake3::hash(&canonical).as_bytes().to_vec())
}

/// Compare two paths case-insensitively on Windows and exactly elsewhere.
pub(crate) fn path_eq(a: &Path, b: &Path) -> bool {
    if cfg!(target_os = "windows") {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// Commit a single album using the staged file transaction protocol.
#[allow(clippy::too_many_arguments)]
async fn commit_single_album(
    client: &mut Client,
    library_root: &Path,
    library_root_id: Uuid,
    import_run_id: Uuid,
    plan_id: Uuid,
    plan_hash: &[u8],
    cancelled: &Arc<AtomicBool>,
    commit: PlanAlbumCommit,
) -> Result<CommitAlbumResult, AppError> {
    let PlanAlbumCommit { plan_album, images } = commit;
    let album_relative_path = normalize_relative_path(&plan_album.target_relative_path)?;
    let image_count = images.len() as u32;

    // ── Idempotency: verify complete evidence before doing anything. ──
    // If a prior run fully committed this album (matching transaction id, plan
    // id, plan hash, manifest hash, on-disk dir + manifest + every file's
    // size/BLAKE3, and the DB album + image records), skip it.
    if let Some(existing_tx) =
        ImportRepository::find_latest_file_transaction(client, plan_album.import_album_id).await?
    {
        match verify_complete_evidence(
            client,
            library_root,
            library_root_id,
            &existing_tx,
            plan_id,
            plan_hash,
            &album_relative_path,
            &images,
        )
        .await?
        {
            IdempotencyVerdict::AlreadyCommitted => {
                return Ok(CommitAlbumResult {
                    album_name: album_relative_path.clone(),
                    status: "skipped".to_string(),
                    images_committed: image_count,
                    target_path: Some(
                        library_root
                            .join("Albums")
                            .join(&album_relative_path)
                            .display()
                            .to_string(),
                    ),
                    manifest_path: existing_tx.manifest_path,
                    error: None,
                });
            }
            IdempotencyVerdict::Conflict(msg) => {
                // Only flip a non-terminal transaction to conflict; a terminal
                // transaction (source_archived) is already complete and should
                // not be disturbed.
                let tx_state = TransactionState::parse(&existing_tx.state).ok();
                if !matches!(tx_state, Some(TransactionState::SourceArchived)) {
                    ImportRepository::update_file_transaction_state(
                        client,
                        existing_tx.id,
                        &TransactionState::Conflict,
                        Some(&msg),
                    )
                    .await?;
                }
                return Err(AppError::Internal(format!(
                    "target conflict for album '{album_relative_path}': {msg}"
                )));
            }
            IdempotencyVerdict::Resume { transaction_id } => {
                // Do NOT fall through to create a second active transaction
                // for the same album. Surface a distinct error so the caller
                // (command / GUI) can route to recovery with this id.
                return Err(AppError::ResumeRequired(transaction_id));
            }
        }
    }

    // The publish dir must not already exist as an unknown directory.
    let publish_dir = library_root.join("Albums").join(&album_relative_path);
    // Ensure the publish dir's parent exists so the atomic rename can land.
    // The publish dir itself must NOT exist (created atomically by rename).
    if let Some(parent) = publish_dir.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| AppError::IoError(format!("cannot create publish parent dir: {e}")))?;
    }
    if publish_dir.exists() {
        // It might be a previously-published dir with a matching manifest; that
        // is handled by the idempotency check above. If we reach here with an
        // existing dir, treat it as a conflict rather than overwriting.
        return Err(AppError::Internal(format!(
            "target directory already exists with no matching committed transaction: {}",
            publish_dir.display()
        )));
    }

    // ── Phase 1: prewrite file transaction + all operations in one DB tx. ──
    // Every operation is persisted as `planned` BEFORE any file is copied.
    let tx_id = Uuid::new_v4();
    let staging_base = library_root
        .join(".imagedb")
        .join("staging")
        .join(tx_id.to_string());
    let staging_dir = staging_base.join(&album_relative_path);

    let initial = state_machine::transition_transaction(TransactionState::Planned, "stage")?;
    ImportRepository::insert_file_transaction(
        client,
        tx_id,
        import_run_id,
        plan_album.import_album_id,
        &initial,
        Some(&staging_dir.display().to_string()),
        Some(&publish_dir.display().to_string()),
        None,
    )
    .await?;

    ImportRepository::set_transaction_hashes(client, tx_id, Some(plan_hash), None).await?;

    // Build manifest metadata common to all operations.
    let mut op_ids: Vec<(Uuid, PathBuf, Vec<u8>)> = Vec::with_capacity(images.len());
    for img in &images {
        let target_rel = normalize_relative_path(&img.target_relative_path)?;
        let staged_path = staging_dir.join(&target_rel);
        let target_path = publish_dir.join(&target_rel);
        let op_id = ImportRepository::insert_file_operation(
            client,
            tx_id,
            &img.source_path,
            &staged_path.display().to_string(),
            &target_path.display().to_string(),
            img.expected_file_size,
            &img.expected_blake3,
        )
        .await?;
        op_ids.push((op_id, staged_path, img.expected_blake3.clone()));
    }

    ImportRepository::update_file_transaction_state(
        client,
        tx_id,
        &TransactionState::Staging,
        None,
    )
    .await?;

    #[cfg(feature = "fail-injection")]
    maybe_fault(CommitFaultPoint::AfterDbWrite, "after DB write")?;

    // ── Phase 2: stream copy to staging with .part + incremental BLAKE3. ──
    tokio::fs::create_dir_all(&staging_dir)
        .await
        .map_err(|e| AppError::IoError(format!("cannot create staging dir: {e}")))?;

    for (i, img) in images.iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(AppError::Internal(
                "commit cancelled before staging".to_string(),
            ));
        }
        let src = Path::new(&img.source_path);
        let target_rel = normalize_relative_path(&img.target_relative_path)?;
        let staged_path = staging_dir.join(&target_rel);
        let part_path = staging_dir.join(format!("{target_rel}.part"));

        if let Some(parent) = staged_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| AppError::IoError(format!("cannot create staging subdir: {e}")))?;
        }

        // A prior interrupted run may have left a `.part`; remove it so the
        // copy starts clean. (A fully-`verified` staging file is reused below.)
        let _ = tokio::fs::remove_file(&part_path).await;

        ImportRepository::update_file_operation_state(
            client,
            op_ids[i].0,
            &FileOpState::Copying,
            None,
            None,
        )
        .await?;

        #[cfg(feature = "fail-injection")]
        maybe_fault(CommitFaultPoint::DuringCopy, "during copy")?;

        let actual_blake3 = stream_copy_with_hash(src, &part_path).await?;
        let expected = &op_ids[i].2;
        if actual_blake3 != *expected {
            let _ = tokio::fs::remove_file(&part_path).await;
            let msg = format!(
                "BLAKE3 mismatch for {}: expected {} got {}",
                src.display(),
                bytes_to_hex(expected),
                bytes_to_hex(&actual_blake3)
            );
            ImportRepository::update_file_operation_state(
                client,
                op_ids[i].0,
                &FileOpState::Failed,
                Some(&actual_blake3),
                Some(&msg),
            )
            .await?;
            ImportRepository::update_file_transaction_state(
                client,
                tx_id,
                &TransactionState::Failed,
                Some(&msg),
            )
            .await?;
            return Err(AppError::Internal(msg));
        }

        // Atomically promote the .part to the staged file.
        tokio::fs::rename(&part_path, &staged_path)
            .await
            .map_err(|e| AppError::IoError(format!("rename part file failed: {e}")))?;

        ImportRepository::update_file_operation_state(
            client,
            op_ids[i].0,
            &FileOpState::Verified,
            Some(&actual_blake3),
            None,
        )
        .await?;

        #[cfg(feature = "fail-injection")]
        maybe_fault(CommitFaultPoint::AfterStagingCopy, "after staging copy")?;
    }

    // ── Phase 3: verify the staging set, then write the manifest. ──
    ImportRepository::update_file_transaction_state(
        client,
        tx_id,
        &TransactionState::Verifying,
        None,
    )
    .await?;
    verify_staging_set(&staging_dir, &images).await?;

    let verified = state_machine::transition_transaction(TransactionState::Verifying, "verified")?;
    ImportRepository::update_file_transaction_state(client, tx_id, &verified, None).await?;

    #[cfg(feature = "fail-injection")]
    maybe_fault(CommitFaultPoint::AfterStagingVerify, "after staging verify")?;

    // Manifest: write to a temp file, flush, atomic rename, then hash.
    let manifest = build_manifest(
        &tx_id,
        plan_id,
        plan_hash,
        import_run_id,
        plan_album.import_album_id,
        library_root_id,
        &album_relative_path,
        &images,
    );
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| AppError::Internal(format!("manifest serialize failed: {e}")))?;
    let manifest_hash = blake3::hash(manifest_json.as_bytes()).as_bytes().to_vec();

    let staging_manifest_dir = staging_dir.join(".imagedb");
    tokio::fs::create_dir_all(&staging_manifest_dir)
        .await
        .map_err(|e| AppError::IoError(format!("cannot create staging manifest dir: {e}")))?;
    let staging_manifest_tmp = staging_manifest_dir.join(".imagedb-manifest.json.tmp");
    let staging_manifest_file = staging_manifest_dir.join(".imagedb-manifest.json");
    write_synced_then_rename(
        &staging_manifest_tmp,
        &staging_manifest_file,
        manifest_json.as_bytes(),
    )
    .await?;

    ImportRepository::set_transaction_hashes(client, tx_id, None, Some(&manifest_hash)).await?;

    #[cfg(feature = "fail-injection")]
    maybe_fault(CommitFaultPoint::AfterManifestWrite, "after manifest write")?;

    // ── Phase 4: atomic publish (rename whole staging dir → publish dir). ──
    let publishing = state_machine::transition_transaction(TransactionState::Verified, "publish")?;
    ImportRepository::update_file_transaction_state(client, tx_id, &publishing, None).await?;

    #[cfg(feature = "fail-injection")]
    maybe_fault(
        CommitFaultPoint::BeforePublishRename,
        "before publish rename",
    )?;

    // The publish dir must not exist (checked above; re-check defensively).
    if publish_dir.exists() {
        return Err(AppError::Internal(format!(
            "target directory appeared during publish: {}",
            publish_dir.display()
        )));
    }
    tokio::fs::rename(&staging_dir, &publish_dir)
        .await
        .map_err(|e| AppError::IoError(format!("atomic publish rename failed: {e}")))?;
    sync_parent_dir(&publish_dir).await?;

    // The manifest moved with the rename: record the published path on the
    // transaction and expose it via CommitAlbumResult. The staging path no
    // longer exists and must never be returned as the manifest location.
    let published_manifest_path = publish_dir.join(".imagedb").join(".imagedb-manifest.json");
    if !published_manifest_path.exists() {
        return Err(AppError::Internal(format!(
            "published manifest missing after rename: {}",
            published_manifest_path.display()
        )));
    }
    ImportRepository::set_transaction_manifest_path(
        client,
        tx_id,
        &published_manifest_path.display().to_string(),
    )
    .await?;

    let published =
        state_machine::transition_transaction(TransactionState::Publishing, "published")?;
    ImportRepository::update_file_transaction_state(client, tx_id, &published, None).await?;

    #[cfg(feature = "fail-injection")]
    maybe_fault(CommitFaultPoint::AfterPublishRename, "after publish rename")?;

    // ── Phase 5: DB commit (do NOT delete the publish dir on failure). ──
    let db_committing =
        state_machine::transition_transaction(TransactionState::Published, "db_commit")?;
    ImportRepository::update_file_transaction_state(client, tx_id, &db_committing, None).await?;

    #[cfg(feature = "fail-injection")]
    maybe_fault(CommitFaultPoint::BeforeDbCommit, "before DB commit")?;

    if let Err(e) = commit_library_records_transaction(
        client,
        library_root_id,
        tx_id,
        plan_id,
        plan_hash,
        &manifest_hash,
        &plan_album,
        &album_relative_path,
        &manifest,
        &images,
    )
    .await
    {
        // DB failed: keep the published dir, stay PUBLISHED, hand to recovery.
        ImportRepository::update_file_transaction_state(
            client,
            tx_id,
            &TransactionState::Published,
            Some(&e.to_string()),
        )
        .await?;
        return Err(e);
    }

    let library_committed =
        state_machine::transition_transaction(TransactionState::DbCommitting, "library_committed")?;
    ImportRepository::update_file_transaction_state(client, tx_id, &library_committed, None)
        .await?;

    #[cfg(feature = "fail-injection")]
    maybe_fault(CommitFaultPoint::AfterDbCommit, "after DB commit")?;

    // ── Phase 6: source archive (separate recoverable stage). ──
    //
    // Source album root comes from import_albums.source_path (never from
    // plan image parents) so commit and recovery stay in lockstep.
    //
    // Archive integrity is proved with the FULL source snapshot captured
    // at scan time — the frozen import plan is NOT used here because it
    // only lists images selected for import, not the whole album (sidecars,
    // descriptions, nested files, excluded images).
    let import_album = ImportRepository::get_import_album_by_id(client, plan_album.import_album_id)
        .await?
        .ok_or_else(|| {
            AppError::Internal(format!(
                "import_album {} missing; cannot determine source album directory",
                plan_album.import_album_id
            ))
        })?;
    if import_album.source_path.is_empty() {
        return Err(AppError::Internal(format!(
            "import_album {} has empty source_path",
            plan_album.import_album_id
        )));
    }
    let source_album_dir = PathBuf::from(&import_album.source_path);

    // Load the full source snapshot persisted during scan. A missing
    // snapshot means we cannot prove archive integrity, so the archive
    // stage is rejected rather than silently trusted.
    let snapshot_pair = load_source_album_snapshot(client, plan_album.import_album_id).await?;
    let Some((snapshot, snapshot_files)) = snapshot_pair else {
        let msg = format!(
            "no source snapshot for album {}; cannot archive safely",
            plan_album.import_album_id
        );
        ImportRepository::update_file_transaction_state(
            client,
            tx_id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Err(AppError::Internal(msg));
    };

    // Defense-in-depth: every plan image whose source still exists must
    // resolve inside the source album root. This does NOT substitute for
    // the snapshot check — it catches forged plan entries that reference
    // unrelated paths on disk.
    validate_plan_image_sources(&source_album_dir, &images)?;

    let archive_base = source_album_dir
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".imagedb-processed");
    let archive_dir = archive_base
        .join(tx_id.to_string())
        .join(&album_relative_path);

    let source_exists = source_album_dir.exists();
    let archive_exists = archive_dir.exists();

    // Transition to source_archiving only when we actually have work to do.
    // Both-missing / both-present are conflicts surfaced before any rename.
    match (source_exists, archive_exists) {
        (true, false) => {
            #[cfg(feature = "fail-injection")]
            maybe_fault(
                CommitFaultPoint::BeforeSourceArchive,
                "before source archive",
            )?;

            let archiving = state_machine::transition_transaction(
                TransactionState::LibraryCommitted,
                "archive",
            )?;
            ImportRepository::update_file_transaction_state(client, tx_id, &archiving, None)
                .await?;

            // Verify source directory against the full snapshot BEFORE rename.
            if let Some(msg) = verify_source_snapshot_or_conflict(
                client,
                tx_id,
                &source_album_dir,
                &snapshot.snapshot_hash,
                &snapshot_files,
                "source album",
            )
            .await?
            {
                return Err(AppError::Internal(msg));
            }

            tokio::fs::create_dir_all(archive_dir.parent().unwrap())
                .await
                .map_err(|e| AppError::IoError(format!("cannot create archive base dir: {e}")))?;

            #[cfg(feature = "fail-injection")]
            maybe_fault(
                CommitFaultPoint::DuringSourceArchive,
                "during source archive",
            )?;

            tokio::fs::rename(&source_album_dir, &archive_dir)
                .await
                .map_err(|e| AppError::IoError(format!("source archive rename failed: {e}")))?;
            sync_parent_dir(&archive_dir).await?;

            // Re-verify AFTER rename: archive content must still match.
            if let Some(msg) = verify_source_snapshot_or_conflict(
                client,
                tx_id,
                &archive_dir,
                &snapshot.snapshot_hash,
                &snapshot_files,
                "archive after rename",
            )
            .await?
            {
                return Err(AppError::Internal(msg));
            }

            let archived = state_machine::transition_transaction(
                TransactionState::SourceArchiving,
                "archived",
            )?;
            ImportRepository::update_file_transaction_state(client, tx_id, &archived, None).await?;
        }
        (false, true) => {
            // Archive already exists (e.g. from an interrupted prior run).
            // Trust it only if it exactly matches the captured snapshot.
            if let Some(msg) = verify_source_snapshot_or_conflict(
                client,
                tx_id,
                &archive_dir,
                &snapshot.snapshot_hash,
                &snapshot_files,
                "existing archive",
            )
            .await?
            {
                return Err(AppError::Internal(msg));
            }
            let archiving = state_machine::transition_transaction(
                TransactionState::LibraryCommitted,
                "archive",
            )?;
            ImportRepository::update_file_transaction_state(client, tx_id, &archiving, None)
                .await?;
            let archived = state_machine::transition_transaction(
                TransactionState::SourceArchiving,
                "archived",
            )?;
            ImportRepository::update_file_transaction_state(client, tx_id, &archived, None).await?;
        }
        (false, false) => {
            let msg = format!(
                "source {} and archive {} both missing; cannot confirm archive integrity",
                source_album_dir.display(),
                archive_dir.display()
            );
            ImportRepository::update_file_transaction_state(
                client,
                tx_id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            return Err(AppError::Internal(msg));
        }
        (true, true) => {
            // Ambiguous state: do NOT delete or overwrite either directory.
            let msg = format!(
                "source {} and archive {} both present; refusing to overwrite or delete",
                source_album_dir.display(),
                archive_dir.display()
            );
            ImportRepository::update_file_transaction_state(
                client,
                tx_id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            return Err(AppError::Internal(msg));
        }
    }

    // Best-effort staging cleanup. A failure here leaves cleanup_required but
    // does not invalidate the commit.
    if let Err(e) = tokio::fs::remove_dir_all(&staging_base).await {
        if staging_base.exists() {
            let msg = format!("staging cleanup failed: {e}");
            ImportRepository::update_file_transaction_state(
                client,
                tx_id,
                &TransactionState::CleanupRequired,
                Some(&msg),
            )
            .await?;
            return Ok(CommitAlbumResult {
                album_name: album_relative_path,
                status: "cleanup_required".to_string(),
                images_committed: image_count,
                target_path: Some(publish_dir.display().to_string()),
                manifest_path: Some(published_manifest_path.display().to_string()),
                error: Some(msg),
            });
        }
    }

    Ok(CommitAlbumResult {
        album_name: album_relative_path,
        status: "committed".to_string(),
        images_committed: image_count,
        target_path: Some(publish_dir.display().to_string()),
        manifest_path: Some(published_manifest_path.display().to_string()),
        error: None,
    })
}

pub(crate) async fn verify_source_snapshot_or_conflict(
    client: &Client,
    tx_id: Uuid,
    dir: &Path,
    snapshot_hash: &[u8],
    snapshot_files: &[SnapshotFileRecord],
    label: &str,
) -> Result<Option<String>, AppError> {
    let errors = match verify_source_snapshot_files(dir, snapshot_hash, snapshot_files) {
        Ok(errors) => errors,
        Err(e) => {
            let msg = format!(
                "{} {} could not be verified against captured snapshot: {e}",
                label,
                dir.display()
            );
            ImportRepository::update_file_transaction_state(
                client,
                tx_id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            return Ok(Some(msg));
        }
    };

    if errors.is_empty() {
        return Ok(None);
    }

    let msg = format!(
        "{} {} does not match captured snapshot: {}",
        label,
        dir.display(),
        errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    );
    ImportRepository::update_file_transaction_state(
        client,
        tx_id,
        &TransactionState::Conflict,
        Some(&msg),
    )
    .await?;
    Ok(Some(msg))
}

/// Stream-copy a file while incrementally computing BLAKE3. Returns the hash.
pub(crate) async fn stream_copy_with_hash(src: &Path, dst: &Path) -> Result<Vec<u8>, AppError> {
    let mut src_file = tokio::fs::File::open(src)
        .await
        .map_err(|e| AppError::IoError(format!("cannot open source {}: {e}", src.display())))?;
    let mut dst_file = tokio::fs::File::create(dst).await.map_err(|e| {
        AppError::IoError(format!("cannot create staging part {}: {e}", dst.display()))
    })?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 65536];
    loop {
        let n = src_file
            .read(&mut buf)
            .await
            .map_err(|e| AppError::IoError(format!("read error during staging: {e}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        dst_file
            .write_all(&buf[..n])
            .await
            .map_err(|e| AppError::IoError(format!("write error during staging: {e}")))?;
    }
    dst_file
        .flush()
        .await
        .map_err(|e| AppError::IoError(format!("flush error: {e}")))?;
    dst_file
        .sync_all()
        .await
        .map_err(|e| AppError::IoError(format!("sync error: {e}")))?;
    Ok(hasher.finalize().as_bytes().to_vec())
}

pub(crate) async fn write_synced_then_rename(
    tmp_path: &Path,
    final_path: &Path,
    bytes: &[u8],
) -> Result<(), AppError> {
    let mut file = tokio::fs::File::create(tmp_path).await.map_err(|e| {
        AppError::IoError(format!(
            "cannot create temp file {}: {e}",
            tmp_path.display()
        ))
    })?;
    file.write_all(bytes)
        .await
        .map_err(|e| AppError::IoError(format!("temp file write failed: {e}")))?;
    file.flush()
        .await
        .map_err(|e| AppError::IoError(format!("temp file flush failed: {e}")))?;
    file.sync_all()
        .await
        .map_err(|e| AppError::IoError(format!("temp file sync failed: {e}")))?;
    drop(file);

    tokio::fs::rename(tmp_path, final_path)
        .await
        .map_err(|e| AppError::IoError(format!("atomic rename failed: {e}")))?;
    sync_parent_dir(final_path).await?;
    Ok(())
}

pub(crate) async fn sync_parent_dir(path: &Path) -> Result<(), AppError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    match tokio::fs::File::open(parent).await {
        Ok(dir) => match dir.sync_all().await {
            Ok(()) => Ok(()),
            Err(e) if is_unsupported_dir_sync(&e) => Ok(()),
            Err(e) => Err(AppError::IoError(format!(
                "parent directory sync failed for {}: {e}",
                parent.display()
            ))),
        },
        Err(e) if is_unsupported_dir_sync(&e) => Ok(()),
        Err(e) => Err(AppError::IoError(format!(
            "cannot open parent directory for sync {}: {e}",
            parent.display()
        ))),
    }
}

fn is_unsupported_dir_sync(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        ErrorKind::Unsupported | ErrorKind::PermissionDenied | ErrorKind::Other
    )
}

/// Re-verify the staging file set: every expected file exists, has the right
/// size, and matches its expected BLAKE3.
pub(crate) async fn verify_staging_set(
    staging_dir: &Path,
    images: &[PlanImageRow],
) -> Result<(), AppError> {
    for img in images {
        let target_rel = normalize_relative_path(&img.target_relative_path)?;
        let staged = staging_dir.join(&target_rel);
        let meta = tokio::fs::metadata(&staged).await.map_err(|e| {
            AppError::IoError(format!("staged file missing {}: {e}", staged.display()))
        })?;
        if meta.len() != img.expected_file_size as u64 {
            return Err(AppError::Internal(format!(
                "staged file size mismatch for {}: expected {} got {}",
                staged.display(),
                img.expected_file_size,
                meta.len()
            )));
        }
        // Recompute BLAKE3 from the staged file.
        let mut f = tokio::fs::File::open(&staged)
            .await
            .map_err(|e| AppError::IoError(format!("cannot open staged file: {e}")))?;
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; 65536];
        loop {
            let n = f
                .read(&mut buf)
                .await
                .map_err(|e| AppError::IoError(format!("staged read error: {e}")))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        let actual = hasher.finalize().as_bytes().to_vec();
        if actual != img.expected_blake3 {
            return Err(AppError::Internal(format!(
                "staged BLAKE3 mismatch for {}: expected {} got {}",
                staged.display(),
                bytes_to_hex(&img.expected_blake3),
                bytes_to_hex(&actual)
            )));
        }
    }
    Ok(())
}

/// Validate that every plan image whose source_path still exists on disk
/// resolves to a location inside `source_album_dir`. The source_album_dir is
/// the authoritative album root (read from `import_albums.source_path`) and
/// must NOT be re-derived from an image's parent.
///
/// Canonicalization is used when paths exist to resolve symlinks and
/// case-mismatched Windows paths; a source whose canonicalization fails is
/// rejected because we cannot trust it, while a source that does not exist
/// at all is tolerated (the archive-only branch does not need it).
pub(crate) fn validate_plan_image_sources(
    source_album_dir: &Path,
    plan_images: &[PlanImageRow],
) -> Result<(), AppError> {
    let canonical_root = if source_album_dir.exists() {
        source_album_dir.canonicalize().map_err(|e| {
            AppError::IoError(format!(
                "cannot canonicalize source album dir {}: {e}",
                source_album_dir.display()
            ))
        })?
    } else {
        source_album_dir.to_path_buf()
    };

    for img in plan_images {
        let src = Path::new(&img.source_path);
        if !src.exists() {
            continue;
        }
        let canonical_src = src.canonicalize().map_err(|e| {
            AppError::IoError(format!(
                "cannot canonicalize source path {}: {e}",
                src.display()
            ))
        })?;
        if !canonical_src.starts_with(&canonical_root) {
            return Err(AppError::Internal(format!(
                "plan image source '{}' escapes source album root '{}' (resolved '{}' not under '{}')",
                img.source_path,
                source_album_dir.display(),
                canonical_src.display(),
                canonical_root.display()
            )));
        }
    }
    Ok(())
}

/// Verify that a directory contains *exactly* the file set prescribed by the
/// frozen plan: same relative paths, same sizes, same BLAKE3 hashes, and no
/// extra or missing entries. Subdirectories not referenced by any plan image
/// are treated as unexpected entries.
///
/// Used by the source-archive recovery path to validate the source album dir
/// before renaming it, and to validate the archive dir before trusting it in
/// lieu of the source.
#[cfg(test)]
pub(crate) async fn verify_dir_against_plan(
    dir: &Path,
    album_relative_path: &str,
    plan_images: &[PlanImageRow],
) -> Result<(), AppError> {
    use std::collections::HashMap;

    if !dir.exists() {
        return Err(AppError::Internal(format!(
            "directory does not exist: {}",
            dir.display()
        )));
    }

    let mut expected: HashMap<String, &PlanImageRow> = HashMap::new();
    let album_rel = normalize_relative_path(album_relative_path)?;
    for img in plan_images {
        let source_rel = normalize_relative_path(&img.source_relative_path)?;
        let rel = source_rel
            .strip_prefix(&(album_rel.clone() + "/"))
            .unwrap_or(&source_rel)
            .to_string();
        expected.insert(rel, img);
    }

    // Walk the directory recursively and match every file against the plan.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    walk_and_verify(dir, dir, &expected, &mut seen).await?;

    // Any plan entry not observed on disk is a missing file.
    for rel in expected.keys() {
        if !seen.contains(rel) {
            return Err(AppError::Internal(format!(
                "plan file missing from directory: {rel}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
async fn walk_and_verify(
    root: &Path,
    current: &Path,
    expected: &std::collections::HashMap<String, &PlanImageRow>,
    seen: &mut std::collections::HashSet<String>,
) -> Result<(), AppError> {
    let mut entries = tokio::fs::read_dir(current)
        .await
        .map_err(|e| AppError::IoError(format!("cannot read_dir {}: {e}", current.display())))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| AppError::IoError(format!("read_dir next failed: {e}")))?
    {
        let ft = entry
            .file_type()
            .await
            .map_err(|e| AppError::IoError(format!("file_type failed: {e}")))?;
        let path = entry.path();
        if ft.is_dir() {
            Box::pin(walk_and_verify(root, &path, expected, seen)).await?;
            continue;
        }
        if !ft.is_file() {
            return Err(AppError::Internal(format!(
                "unexpected non-regular file: {}",
                path.display()
            )));
        }
        let rel_os = path
            .strip_prefix(root)
            .map_err(|e| AppError::Internal(format!("strip_prefix failed: {e}")))?;
        let rel = rel_os.to_string_lossy().replace('\\', "/");
        if seen.contains(&rel) {
            return Err(AppError::Internal(format!(
                "duplicate file encountered: {rel}"
            )));
        }
        seen.insert(rel.clone());
        let img = expected.get(&rel).ok_or_else(|| {
            AppError::Internal(format!("extra file on disk not in frozen plan: {rel}"))
        })?;
        let meta = tokio::fs::metadata(&path).await.map_err(|e| {
            AppError::IoError(format!("metadata failed for {}: {e}", path.display()))
        })?;
        if meta.len() != img.expected_file_size as u64 {
            return Err(AppError::Internal(format!(
                "file size mismatch for {rel}: expected {} got {}",
                img.expected_file_size,
                meta.len()
            )));
        }
        let actual = hash_existing_file(&path).await?;
        if actual != img.expected_blake3 {
            return Err(AppError::Internal(format!(
                "BLAKE3 mismatch for {rel}: expected {} got {}",
                bytes_to_hex(&img.expected_blake3),
                bytes_to_hex(&actual)
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
async fn hash_existing_file(path: &Path) -> Result<Vec<u8>, AppError> {
    let mut f = tokio::fs::File::open(path)
        .await
        .map_err(|e| AppError::IoError(format!("cannot open {}: {e}", path.display())))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 65536];
    loop {
        let n = f
            .read(&mut buf)
            .await
            .map_err(|e| AppError::IoError(format!("read error: {e}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().as_bytes().to_vec())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_manifest(
    tx_id: &Uuid,
    plan_id: Uuid,
    plan_hash: &[u8],
    import_run_id: Uuid,
    import_album_id: Uuid,
    library_root_id: Uuid,
    album_relative_path: &str,
    images: &[PlanImageRow],
) -> AlbumManifest {
    AlbumManifest {
        schema_version: MANIFEST_SCHEMA_VERSION.to_string(),
        transaction_id: tx_id.to_string(),
        plan_id: plan_id.to_string(),
        plan_hash: bytes_to_hex(plan_hash),
        import_run_id: import_run_id.to_string(),
        import_album_id: import_album_id.to_string(),
        library_root_id: library_root_id.to_string(),
        album_relative_path: album_relative_path.to_string(),
        image_count: images.len() as u32,
        images: images
            .iter()
            .map(|img| AlbumManifestImage {
                relative_path: normalize_relative_path(&img.target_relative_path)
                    .unwrap_or_else(|_| img.target_relative_path.clone()),
                source_path: img.source_path.clone(),
                file_size: img.expected_file_size,
                blake3: bytes_to_hex(&img.expected_blake3),
                width: img.width,
                height: img.height,
                format: img.format.clone(),
                fingerprint_version: None,
            })
            .collect(),
    }
}

/// Insert (or confirm) the library album + images in one DB transaction.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn commit_library_records_transaction(
    client: &mut Client,
    library_root_id: Uuid,
    transaction_id: Uuid,
    plan_id: Uuid,
    plan_hash: &[u8],
    manifest_hash: &[u8],
    plan_album: &PlanAlbumRow,
    album_relative_path: &str,
    _manifest: &AlbumManifest,
    images: &[PlanImageRow],
) -> Result<Uuid, AppError> {
    let transaction = client.transaction().await.map_err(|e| {
        AppError::Internal(format!("failed to begin library record transaction: {e}"))
    })?;

    // If a library album already exists for this exact transaction, this is a
    // recovery retry — confirm it matches rather than re-inserting.
    let existing: Option<Uuid> = transaction
        .query_opt(
            "SELECT id FROM library_albums
             WHERE library_root_id = $1 AND relative_path = $2
               AND transaction_id = $3",
            &[&library_root_id, &album_relative_path, &transaction_id],
        )
        .await
        .map_err(|e| AppError::Internal(format!("failed to query existing library album: {e}")))?
        .map(|r| r.get("id"));

    let library_album_id = match existing {
        Some(id) => id,
        None => {
            let id = Uuid::new_v4();
            transaction
                .execute(
                    "INSERT INTO library_albums
                     (id, library_root_id, display_name, relative_path, manifest_version,
                      manifest_hash, image_count, state, transaction_id, plan_hash)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, 'committed', $8, $9)",
                    &[
                        &id,
                        &library_root_id,
                        &plan_album.target_relative_path,
                        &album_relative_path,
                        &MANIFEST_SCHEMA_VERSION,
                        &manifest_hash,
                        &(images.len() as i32),
                        &transaction_id,
                        &plan_hash,
                    ],
                )
                .await
                .map_err(|e| AppError::Internal(format!("failed to insert library album: {e}")))?;
            id
        }
    };

    // Confirm existing images or insert missing ones. Idempotent: re-running
    // a DB commit for the same transaction must not duplicate rows.
    for img in images {
        let target_rel = normalize_relative_path(&img.target_relative_path)?;
        let exists: bool = transaction
            .query_one(
                "SELECT EXISTS(
                    SELECT 1 FROM library_images
                    WHERE album_id = $1 AND relative_path = $2
                )",
                &[&library_album_id, &target_rel],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to check library image: {e}")))?
            .get(0);
        if exists {
            continue;
        }
        let image_id = Uuid::new_v4();
        let fp_version = img.format.as_deref().unwrap_or("unknown");
        transaction
            .execute(
                "INSERT INTO library_images
                 (id, album_id, relative_path, file_size, width, height, format,
                  blake3, fingerprint_version, state)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'committed')",
                &[
                    &image_id,
                    &library_album_id,
                    &target_rel,
                    &img.expected_file_size,
                    &img.width,
                    &img.height,
                    &img.format,
                    &img.expected_blake3,
                    &fp_version,
                ],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to insert library image: {e}")))?;
    }

    transaction
        .commit()
        .await
        .map_err(|e| AppError::Internal(format!("failed to commit library records: {e}")))?;

    // Mark the frozen plan as consumed (single DB statement outside the tx).
    ImportRepository::update_import_plan_state(
        client,
        plan_id,
        &state_machine::PlanState::Consumed,
    )
    .await?;

    Ok(library_album_id)
}

/// Idempotency verdict for an existing transaction.
pub enum IdempotencyVerdict {
    /// All evidence matches — this album is already fully committed. Skip it.
    AlreadyCommitted,
    /// The on-disk state conflicts with the persisted transaction. Do not
    /// overwrite; surface the conflict.
    Conflict(String),
    /// The transaction is mid-flight (any non-terminal state); the caller
    /// must route to recovery for this transaction_id rather than creating a
    /// second active transaction for the same album.
    Resume { transaction_id: Uuid },
}

/// Rule 12: complete idempotency verification. Returns `AlreadyCommitted`
/// only when every piece of evidence matches — transaction id, plan id, plan
/// hash, manifest hash, the published directory + parseable manifest, every
/// file's path/size/BLAKE3, and the DB album + image records.
#[allow(clippy::too_many_arguments)]
pub async fn verify_complete_evidence(
    client: &Client,
    library_root: &Path,
    library_root_id: Uuid,
    existing_tx: &crate::repositories::import_repository::FileTransactionFullRow,
    plan_id: Uuid,
    plan_hash: &[u8],
    album_relative_path: &str,
    images: &[PlanImageRow],
) -> Result<IdempotencyVerdict, AppError> {
    let tx_state = match TransactionState::parse(&existing_tx.state) {
        Ok(s) => s,
        Err(_) => {
            return Ok(IdempotencyVerdict::Conflict(format!(
                "transaction {} has unparseable state '{}'",
                existing_tx.id, existing_tx.state
            )));
        }
    };

    // Any non-terminal state other than SourceArchived is mid-flight: the
    // caller must resume the existing transaction through recovery rather
    // than creating a second active file_transaction for the same album.
    // Only SourceArchived can possibly be AlreadyCommitted (full evidence
    // check below).
    if !matches!(tx_state, TransactionState::SourceArchived) {
        return Ok(IdempotencyVerdict::Resume {
            transaction_id: existing_tx.id,
        });
    }

    // Plan hash must match.
    match &existing_tx.plan_hash {
        Some(stored) if stored == plan_hash => {}
        other => {
            return Ok(IdempotencyVerdict::Conflict(format!(
                "plan_hash mismatch: stored {:?} expected {}",
                other,
                bytes_to_hex(plan_hash)
            )));
        }
    }

    let publish_dir = library_root.join("Albums").join(album_relative_path);
    if !publish_dir.exists() {
        // SourceArchived without the published dir is evidence tampering:
        // surface as conflict rather than auto-resuming.
        return Ok(IdempotencyVerdict::Conflict(format!(
            "transaction {} is source_archived but published dir {} is missing",
            existing_tx.id,
            publish_dir.display()
        )));
    }

    // Manifest must parse and match.
    let manifest = match read_manifest(&publish_dir) {
        Ok(m) => m,
        Err(e) => {
            return Ok(IdempotencyVerdict::Conflict(format!(
                "manifest unreadable/unparseable: {e}"
            )));
        }
    };
    if manifest.transaction_id != existing_tx.id.to_string() {
        return Ok(IdempotencyVerdict::Conflict(format!(
            "manifest transaction_id {} != persisted {}",
            manifest.transaction_id, existing_tx.id
        )));
    }
    if manifest.plan_id != plan_id.to_string() {
        return Ok(IdempotencyVerdict::Conflict(format!(
            "manifest plan_id {} != expected {}",
            manifest.plan_id, plan_id
        )));
    }
    match &existing_tx.manifest_hash {
        Some(stored) => {
            let recomputed = blake3::hash(
                serde_json::to_string_pretty(&manifest)
                    .unwrap_or_default()
                    .as_bytes(),
            )
            .as_bytes()
            .to_vec();
            if stored != &recomputed {
                return Ok(IdempotencyVerdict::Conflict(format!(
                    "manifest_hash mismatch: stored {} recomputed {}",
                    bytes_to_hex(stored),
                    bytes_to_hex(&recomputed)
                )));
            }
        }
        None => {
            return Ok(IdempotencyVerdict::Conflict(
                "transaction has no manifest_hash".to_string(),
            ));
        }
    }
    if manifest.image_count != images.len() as u32 {
        return Ok(IdempotencyVerdict::Conflict(format!(
            "manifest image_count {} != plan {}",
            manifest.image_count,
            images.len()
        )));
    }

    // Every expected file must exist on disk with the right size + BLAKE3.
    let mut manifest_by_rel: HashMap<String, &AlbumManifestImage> = HashMap::new();
    for m in &manifest.images {
        manifest_by_rel.insert(m.relative_path.clone(), m);
    }
    for img in images {
        let target_rel = normalize_relative_path(&img.target_relative_path)?;
        let file_path = publish_dir.join(&target_rel);
        let meta = match tokio::fs::metadata(&file_path).await {
            Ok(m) => m,
            Err(_) => {
                return Ok(IdempotencyVerdict::Conflict(format!(
                    "published file missing: {}",
                    file_path.display()
                )));
            }
        };
        if meta.len() != img.expected_file_size as u64 {
            return Ok(IdempotencyVerdict::Conflict(format!(
                "published file size mismatch for {}: expected {} got {}",
                file_path.display(),
                img.expected_file_size,
                meta.len()
            )));
        }
        let m_entry = match manifest_by_rel.get(&target_rel) {
            Some(e) => e,
            None => {
                return Ok(IdempotencyVerdict::Conflict(format!(
                    "file {} missing from manifest",
                    target_rel
                )));
            }
        };
        if m_entry.blake3 != bytes_to_hex(&img.expected_blake3) {
            return Ok(IdempotencyVerdict::Conflict(format!(
                "manifest blake3 mismatch for {target_rel}"
            )));
        }
        // Recompute the on-disk BLAKE3.
        let mut f = match tokio::fs::File::open(&file_path).await {
            Ok(f) => f,
            Err(e) => {
                return Ok(IdempotencyVerdict::Conflict(format!(
                    "cannot open published file {}: {e}",
                    file_path.display()
                )));
            }
        };
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; 65536];
        loop {
            let n = match f.read(&mut buf).await {
                Ok(n) => n,
                Err(e) => {
                    return Ok(IdempotencyVerdict::Conflict(format!(
                        "published file read error: {e}"
                    )));
                }
            };
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        let actual = hasher.finalize().as_bytes().to_vec();
        if actual != img.expected_blake3 {
            return Ok(IdempotencyVerdict::Conflict(format!(
                "published file blake3 mismatch for {}: content was modified",
                file_path.display()
            )));
        }
    }

    // DB records must exist and match.
    let Some(lib_album) =
        ImportRepository::get_library_album(client, library_root_id, album_relative_path).await?
    else {
        return Ok(IdempotencyVerdict::Conflict(
            "library_album record missing".to_string(),
        ));
    };
    if lib_album.transaction_id != Some(existing_tx.id) {
        return Ok(IdempotencyVerdict::Conflict(format!(
            "library_album.transaction_id {:?} != {}",
            lib_album.transaction_id, existing_tx.id
        )));
    }
    if lib_album.plan_hash.as_deref() != Some(plan_hash) {
        return Ok(IdempotencyVerdict::Conflict(
            "library_album.plan_hash mismatch".to_string(),
        ));
    }
    if lib_album.image_count != images.len() as i32 {
        return Ok(IdempotencyVerdict::Conflict(format!(
            "library_album.image_count {} != plan {}",
            lib_album.image_count,
            images.len()
        )));
    }
    let db_images = ImportRepository::get_library_images_for_album(client, lib_album.id).await?;
    if db_images.len() != images.len() {
        return Ok(IdempotencyVerdict::Conflict(format!(
            "library_images count {} != plan {}",
            db_images.len(),
            images.len()
        )));
    }
    let mut db_by_rel: HashMap<String, Vec<u8>> = db_images
        .iter()
        .map(|r| (r.relative_path.clone(), r.blake3.clone()))
        .collect();
    for img in images {
        let target_rel = normalize_relative_path(&img.target_relative_path)?;
        match db_by_rel.remove(&target_rel) {
            Some(db_blake3) if db_blake3 == img.expected_blake3 => {}
            _ => {
                return Ok(IdempotencyVerdict::Conflict(format!(
                    "library_image record mismatch for {target_rel}"
                )));
            }
        }
    }

    Ok(IdempotencyVerdict::AlreadyCommitted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn normalize_rejects_absolute_path() {
        assert!(normalize_relative_path("/etc/passwd").is_err());
        assert!(normalize_relative_path("C:\\Users\\x").is_err());
    }

    #[test]
    fn normalize_rejects_traversal() {
        assert!(normalize_relative_path("../escape").is_err());
        assert!(normalize_relative_path("a/../../b").is_err());
    }

    #[test]
    fn normalize_keeps_subdirs() {
        assert_eq!(
            normalize_relative_path("chapter-a/001.jpg").unwrap(),
            "chapter-a/001.jpg"
        );
        assert_eq!(normalize_relative_path("a\\b\\c.png").unwrap(), "a/b/c.png");
    }

    #[test]
    fn detect_case_conflict() {
        let images = vec![
            plan_image("AAA/001.jpg", &[1; 32]),
            plan_image("aaa/001.jpg", &[1; 32]),
        ];
        assert!(check_target_path_conflicts(&images).is_err());
    }

    #[test]
    fn detect_duplicate_target_path() {
        let images = vec![
            plan_image("album/x.jpg", &[1; 32]),
            plan_image("album/x.jpg", &[2; 32]),
        ];
        assert!(check_target_path_conflicts(&images).is_err());
    }

    #[test]
    fn distinct_subdirs_ok() {
        let images = vec![
            plan_image("chapter-a/001.jpg", &[1; 32]),
            plan_image("chapter-b/001.jpg", &[2; 32]),
        ];
        assert!(check_target_path_conflicts(&images).is_ok());
    }

    fn plan_image(rel: &str, blake3: &[u8]) -> PlanImageRow {
        PlanImageRow {
            id: Uuid::new_v4(),
            plan_album_id: Uuid::new_v4(),
            import_image_id: Uuid::new_v4(),
            source_path: format!("/src/{rel}"),
            source_relative_path: rel.to_string(),
            target_relative_path: rel.to_string(),
            expected_file_size: 100,
            expected_blake3: blake3.to_vec(),
            width: Some(10),
            height: Some(10),
            format: Some("png".to_string()),
        }
    }

    #[test]
    fn manifest_round_trip() {
        let tx = Uuid::new_v4();
        let plan = Uuid::new_v4();
        let run = Uuid::new_v4();
        let album = Uuid::new_v4();
        let root = Uuid::new_v4();
        let images = vec![plan_image("a/1.jpg", &[7; 32])];
        let m = build_manifest(&tx, plan, &[1; 32], run, album, root, "a", &images);
        let json = serde_json::to_string_pretty(&m).unwrap();
        let back: AlbumManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.schema_version, MANIFEST_SCHEMA_VERSION);
        assert_eq!(back.transaction_id, tx.to_string());
        assert_eq!(back.plan_hash, bytes_to_hex(&[1u8; 32]));
        assert_eq!(back.image_count, 1);
        assert_eq!(back.images[0].blake3, bytes_to_hex(&[7u8; 32]));
    }

    #[test]
    fn empty_dir_normalize_rejected() {
        assert!(normalize_relative_path("").is_err());
        assert!(normalize_relative_path("./.").is_err());
    }

    #[test]
    fn path_eq_windows_case_insensitive() {
        if cfg!(target_os = "windows") {
            assert!(path_eq(
                Path::new("C:\\Users\\x"),
                Path::new("c:\\users\\x")
            ));
        } else {
            assert!(path_eq(Path::new("/tmp/a"), Path::new("/tmp/a")));
            assert!(!path_eq(Path::new("/tmp/a"), Path::new("/tmp/b")));
        }
    }

    #[test]
    fn compute_plan_hash_stable() {
        let album_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let root_id = Uuid::new_v4();
        let img = plan_image("a/1.jpg", &[1; 32]);
        let album = PlanAlbumRow {
            plan_album_id: Uuid::new_v4(),
            import_album_id: album_id,
            target_relative_path: "a".to_string(),
            expected_image_count: 1,
            album_plan_hash: None,
        };
        let frozen = FrozenPlanRow {
            plan_id: Uuid::new_v4(),
            import_run_id: run_id,
            library_root_id: root_id,
            plan_state: "frozen".to_string(),
            plan_hash: None,
            policy_version: "2.0".to_string(),
            albums: vec![(album, vec![img])],
        };
        let h1 = compute_plan_hash(&frozen).unwrap();
        let h2 = compute_plan_hash(&frozen).unwrap();
        assert_eq!(h1, h2, "plan hash must be deterministic");
    }

    /// Real PostgreSQL + filesystem integration test for the full new commit
    /// pipeline (immutable plan, prewrite ops, stream copy, manifest, atomic
    /// publish, DB commit, source archive).
    ///
    /// Invocation:
    ///   IMAGEDB_POSTGRES_BIN=/path/to/pgsql/bin cargo test --manifest-path \
    ///       apps/desktop/src-tauri/Cargo.toml --features real-db-tests --lib \
    ///       real_commit_full_pipeline -- --ignored --test-threads=1
    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_commit_full_pipeline() {
        use crate::domain::import_state::{DecodeState, ImportImageState};
        use crate::infrastructure::postgres::{MigrationRunner, PostgresManager};
        use crate::repositories::import_repository::NewImportImage;
        use std::sync::Arc;

        if std::env::var("IMAGEDB_POSTGRES_BIN")
            .unwrap_or_default()
            .is_empty()
        {
            eprintln!("IMAGEDB_POSTGRES_BIN not set; skipping real commit integration test");
            return;
        }

        let tmp = TempDir::new().unwrap();
        let app_data = tmp.path().join("app_data");
        let source_root = tmp.path().join("source");
        let library_root = tmp.path().join("library");
        let album_path = source_root.join("album_a");
        std::fs::create_dir_all(&album_path).unwrap();
        std::fs::write(album_path.join("photo1.png"), b"photo one data").unwrap();
        std::fs::write(album_path.join("photo2.png"), b"photo two data").unwrap();

        let mut manager = PostgresManager::new(&app_data);
        assert!(manager.binaries_available());
        let probe = manager.initialize().await.unwrap();
        assert!(probe.connection_ok, "diagnostics: {:?}", probe.diagnostics);

        let (mut client, db_handle) = manager.connect().await.unwrap();
        MigrationRunner::run_pending(&mut client).await.unwrap();

        let library_root_id = ImportRepository::upsert_default_library_root(&client)
            .await
            .unwrap();
        ImportRepository::update_library_root_path(
            &client,
            library_root_id,
            &library_root.display().to_string(),
        )
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

        let img1_blake3 = blake3::hash(b"photo one data").as_bytes().to_vec();
        let img2_blake3 = blake3::hash(b"photo two data").as_bytes().to_vec();

        for (n, b3) in [
            ("photo1.png", img1_blake3.clone()),
            ("photo2.png", img2_blake3.clone()),
        ] {
            ImportRepository::insert_import_image(
                &client,
                NewImportImage {
                    album_id,
                    source_path: album_path.join(n).display().to_string(),
                    relative_path: format!("album_a/{n}"),
                    file_size: 14,
                    modified_at: None,
                    width: Some(10),
                    height: Some(10),
                    format: Some("png".to_string()),
                    decode_state: DecodeState::Decoded,
                    blake3: Some(b3),
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
        }

        // Persist the full source album snapshot (run_scan does this in
        // production; commit Phase 6 requires it to verify source/archive
        // integrity).
        crate::services::source_snapshot_service::capture_source_album_snapshot(
            &client,
            import_run_id,
            album_id,
            &album_path,
        )
        .await
        .unwrap();

        // Freeze a plan directly (mirrors what review_service::freeze_plan does).
        freeze_test_plan(
            &mut client,
            import_run_id,
            library_root_id,
            album_id,
            "album_a",
            &[("photo1.png", &img1_blake3), ("photo2.png", &img2_blake3)],
            &album_path,
        )
        .await
        .unwrap();

        drop(client);
        db_handle.abort();

        let cancelled = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(CommitProgress::idle(&import_run_id.to_string())));
        let pg_manager = Arc::new(Mutex::new(manager));
        let result = run_import_commit(
            pg_manager.clone(),
            library_root.display().to_string(),
            import_run_id,
            cancelled,
            progress,
        )
        .await
        .unwrap();
        assert_eq!(result.state, "completed");
        assert_eq!(result.albums_committed, 1);
        assert_eq!(result.images_committed, 2);

        let publish_dir = library_root.join("Albums").join("album_a");
        assert!(publish_dir.exists());
        assert!(publish_dir.join("photo1.png").exists());
        // Manifest lives inside the published dir now.
        let manifest = read_manifest(&publish_dir).unwrap();
        assert_eq!(manifest.image_count, 2);

        // Idempotent rerun: second commit skips the album.
        let cancelled2 = Arc::new(AtomicBool::new(false));
        let progress2 = Arc::new(Mutex::new(CommitProgress::idle(&import_run_id.to_string())));
        let rerun = run_import_commit(
            pg_manager.clone(),
            library_root.display().to_string(),
            import_run_id,
            cancelled2,
            progress2,
        )
        .await
        .unwrap();
        assert_eq!(rerun.albums_skipped, 1);
        assert_eq!(rerun.albums_committed, 0);

        // Source archived.
        let archive_dir = source_root.join(".imagedb-processed");
        assert!(archive_dir.exists(), "source should be archived");

        let (client2, handle2) = {
            let mgr = pg_manager.lock().await;
            mgr.connect().await.unwrap()
        };
        let count: i64 = client2
            .query_one(
                "SELECT COUNT(*) FROM library_images li JOIN library_albums la ON la.id = li.album_id WHERE la.relative_path = 'album_a'",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(
            count, 2,
            "exactly two library images after idempotent rerun"
        );
        drop(client2);
        handle2.abort();
        let mut mgr = pg_manager.lock().await;
        mgr.shutdown().await.unwrap();
    }

    /// Helper: freeze a minimal plan for the real commit test.
    #[cfg(feature = "real-db-tests")]
    async fn freeze_test_plan(
        client: &mut Client,
        import_run_id: Uuid,
        library_root_id: Uuid,
        album_id: Uuid,
        album_name: &str,
        photos: &[(&str, &Vec<u8>)],
        album_path: &Path,
    ) -> Result<(), AppError> {
        use crate::domain::state_machine::PlanState;
        let plan_id =
            ImportRepository::create_import_plan(client, import_run_id, 1, "2.0", library_root_id)
                .await?;
        let plan_album_id = ImportRepository::insert_plan_album(
            client,
            plan_id,
            album_id,
            album_name,
            photos.len() as i32,
        )
        .await?;
        for (n, b3) in photos {
            let img_id: Uuid = client
                .query_one(
                    "SELECT ii.id FROM import_images ii JOIN import_albums ia ON ia.id = ii.import_album_id
                     WHERE ia.import_run_id = $1 AND ii.relative_path LIKE $2",
                    &[&import_run_id, &format!("%/{n}")],
                )
                .await
                .map_err(|e| AppError::Internal(format!("img lookup failed: {e}")))?
                .get(0);
            // target_relative_path is relative to the album root, so a file
            // directly in the album is just its filename.
            ImportRepository::insert_plan_image(
                client,
                plan_album_id,
                img_id,
                &album_path.join(n).display().to_string(),
                &format!("album_a/{n}"),
                n,
                14,
                b3,
                Some(10),
                Some(10),
                Some("png"),
            )
            .await?;
        }
        // Load the (still-draft) plan to compute its hash, store the hash,
        // then freeze.
        let frozen = ImportRepository::load_draft_plan(client, import_run_id)
            .await?
            .ok_or_else(|| AppError::Internal("draft plan not found after insert".to_string()))?;
        let hash = compute_plan_hash(&frozen)?;
        ImportRepository::set_plan_hash(client, plan_id, &hash).await?;
        ImportRepository::update_import_plan_state(client, plan_id, &PlanState::Frozen).await?;
        Ok(())
    }

    fn plan_image_full(
        source_path: &str,
        target_rel: &str,
        size: i64,
        blake3: &[u8],
    ) -> PlanImageRow {
        PlanImageRow {
            id: Uuid::new_v4(),
            plan_album_id: Uuid::new_v4(),
            import_image_id: Uuid::new_v4(),
            source_path: source_path.to_string(),
            source_relative_path: target_rel.to_string(),
            target_relative_path: target_rel.to_string(),
            expected_file_size: size,
            expected_blake3: blake3.to_vec(),
            width: Some(10),
            height: Some(10),
            format: Some("png".to_string()),
        }
    }

    #[tokio::test]
    async fn verify_dir_against_plan_valid() {
        let tmp = TempDir::new().unwrap();
        let data_a = b"alpha";
        let data_b = b"beta";
        std::fs::write(tmp.path().join("a.png"), data_a).unwrap();
        std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("sub/b.png"), data_b).unwrap();
        let imgs = vec![
            plan_image_full("/s/a.png", "a.png", 5, blake3::hash(data_a).as_bytes()),
            plan_image_full(
                "/s/sub/b.png",
                "sub/b.png",
                4,
                blake3::hash(data_b).as_bytes(),
            ),
        ];
        assert!(verify_dir_against_plan(tmp.path(), "album", &imgs)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn verify_dir_against_plan_missing_file() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.png"), b"alpha").unwrap();
        let imgs = vec![
            plan_image_full("/s/a.png", "a.png", 5, blake3::hash(b"alpha").as_bytes()),
            plan_image_full("/s/b.png", "b.png", 4, blake3::hash(b"beta").as_bytes()),
        ];
        let err = verify_dir_against_plan(tmp.path(), "album", &imgs)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("missing"),
            "expected missing-file error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn verify_dir_against_plan_extra_file() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.png"), b"alpha").unwrap();
        std::fs::write(tmp.path().join("extra.png"), b"x").unwrap();
        let imgs = vec![plan_image_full(
            "/s/a.png",
            "a.png",
            5,
            blake3::hash(b"alpha").as_bytes(),
        )];
        let err = verify_dir_against_plan(tmp.path(), "album", &imgs)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("extra"));
    }

    #[tokio::test]
    async fn verify_dir_against_plan_size_mismatch() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.png"), b"alpha").unwrap();
        let imgs = vec![plan_image_full(
            "/s/a.png",
            "a.png",
            999,
            blake3::hash(b"alpha").as_bytes(),
        )];
        let err = verify_dir_against_plan(tmp.path(), "album", &imgs)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("size mismatch"));
    }

    #[tokio::test]
    async fn verify_dir_against_plan_blake3_mismatch() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.png"), b"alpha").unwrap();
        let imgs = vec![plan_image_full("/s/a.png", "a.png", 5, &[0u8; 32])];
        let err = verify_dir_against_plan(tmp.path(), "album", &imgs)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("BLAKE3"));
    }

    #[tokio::test]
    async fn verify_dir_against_plan_empty_dir_rejected() {
        let tmp = TempDir::new().unwrap();
        let imgs = vec![plan_image_full(
            "/s/a.png",
            "a.png",
            5,
            blake3::hash(b"alpha").as_bytes(),
        )];
        assert!(verify_dir_against_plan(tmp.path(), "album", &imgs)
            .await
            .is_err());
    }

    /// Album root = tmp/AlbumA; plan images live in AlbumA/chapter-1 and
    /// AlbumA/chapter-2. Because both paths canonicalize under the album
    /// root, the distinct subdirectories must not conflict.
    #[tokio::test]
    async fn validate_plan_image_sources_subdirs_ok() {
        let tmp = TempDir::new().unwrap();
        let album_root = tmp.path().join("AlbumA");
        std::fs::create_dir_all(album_root.join("chapter-1")).unwrap();
        std::fs::create_dir_all(album_root.join("chapter-2")).unwrap();
        std::fs::write(album_root.join("chapter-1/001.jpg"), b"a").unwrap();
        std::fs::write(album_root.join("chapter-2/002.jpg"), b"b").unwrap();

        let imgs = vec![
            plan_image_full(
                &album_root.join("chapter-1/001.jpg").display().to_string(),
                "AlbumA/chapter-1/001.jpg",
                1,
                blake3::hash(b"a").as_bytes(),
            ),
            plan_image_full(
                &album_root.join("chapter-2/002.jpg").display().to_string(),
                "AlbumA/chapter-2/002.jpg",
                1,
                blake3::hash(b"b").as_bytes(),
            ),
        ];
        assert!(validate_plan_image_sources(&album_root, &imgs).is_ok());
    }

    /// A source that resolves outside the album root is rejected even when
    /// the file exists on disk — this is the security property that
    /// prevents a forged plan image from pulling in an unrelated file.
    #[tokio::test]
    async fn validate_plan_image_sources_escapes_root_rejected() {
        let tmp = TempDir::new().unwrap();
        let album_root = tmp.path().join("AlbumA");
        let outside = tmp.path().join("Outside");
        std::fs::create_dir_all(&album_root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(album_root.join("ok.png"), b"x").unwrap();
        std::fs::write(outside.join("evil.png"), b"y").unwrap();

        let imgs = vec![
            plan_image_full(
                &album_root.join("ok.png").display().to_string(),
                "AlbumA/ok.png",
                1,
                blake3::hash(b"x").as_bytes(),
            ),
            plan_image_full(
                &outside.join("evil.png").display().to_string(),
                "AlbumA/evil.png",
                1,
                blake3::hash(b"y").as_bytes(),
            ),
        ];
        let err = validate_plan_image_sources(&album_root, &imgs)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("escapes"),
            "expected escapes-root error, got: {err}"
        );
    }

    /// Archive-only recovery: the source album root no longer exists, and
    /// neither do any source files. validate_plan_image_sources must still
    /// succeed because the archive branch computes its location without
    /// canonicalizing the source.
    #[tokio::test]
    async fn validate_plan_image_sources_all_missing_ok() {
        let tmp = TempDir::new().unwrap();
        let album_root = tmp.path().join("gone");
        let imgs = vec![
            plan_image_full(
                &album_root.join("chapter-1/001.jpg").display().to_string(),
                "AlbumA/chapter-1/001.jpg",
                1,
                &[0u8; 32],
            ),
            plan_image_full(
                &album_root.join("chapter-2/002.jpg").display().to_string(),
                "AlbumA/chapter-2/002.jpg",
                1,
                &[0u8; 32],
            ),
        ];
        assert!(validate_plan_image_sources(&album_root, &imgs).is_ok());
    }

    /// Album root is the parent of both subdirectories — verify that the
    /// album root (not a subdirectory parent) is the accepted containment
    /// boundary. Previously, derive_source_album_dir would have rejected
    /// this because chapter-1 and chapter-2 are distinct parents.
    #[tokio::test]
    async fn validate_plan_image_sources_distinct_subdirs_share_root() {
        let tmp = TempDir::new().unwrap();
        let album_root = tmp.path().join("AlbumA");
        std::fs::create_dir_all(album_root.join("chapter-1")).unwrap();
        std::fs::create_dir_all(album_root.join("chapter-2")).unwrap();
        std::fs::write(album_root.join("chapter-1/001.jpg"), b"a").unwrap();
        std::fs::write(album_root.join("chapter-2/002.jpg"), b"b").unwrap();

        let imgs = vec![
            plan_image_full(
                &album_root.join("chapter-1/001.jpg").display().to_string(),
                "AlbumA/chapter-1/001.jpg",
                1,
                blake3::hash(b"a").as_bytes(),
            ),
            plan_image_full(
                &album_root.join("chapter-2/002.jpg").display().to_string(),
                "AlbumA/chapter-2/002.jpg",
                1,
                blake3::hash(b"b").as_bytes(),
            ),
        ];
        // Using a chapter subdir as the root must reject, because the
        // other image escapes that subdir — proving that the album root
        // (AlbumA) is the single accepted authority.
        let chapter1_root = album_root.join("chapter-1");
        let err = validate_plan_image_sources(&chapter1_root, &imgs)
            .unwrap_err()
            .to_string();
        assert!(err.contains("escapes"), "expected escape error, got: {err}");

        // Using the album root must accept both.
        assert!(validate_plan_image_sources(&album_root, &imgs).is_ok());
    }
}
