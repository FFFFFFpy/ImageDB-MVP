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
    CommitAlbumResult, CommitProgress, CommitResult, ImportRunState, SourceFileMode,
};
use crate::domain::state_machine::{self, FileOpState, PlanState, TransactionState};
use crate::error::AppError;
use crate::infrastructure::postgres::{DatabaseOperationLock, PostgresManager};
use crate::infrastructure::storage_capabilities::{
    probe_storage_capabilities, PublishStrategy as StoragePublishStrategy,
};
use crate::repositories::import_repository::{
    FrozenPlanRow, ImportRepository, NewFileOperation, PlanAlbumRow, PlanImageRow,
    SnapshotFileRecord,
};
use crate::services::recovery_service::reconcile_import_run_state;
use crate::services::source_snapshot_service::{
    load_source_album_snapshot, verify_source_snapshot_files_async,
    verify_source_snapshot_files_ignoring_paths_async,
};
#[cfg(feature = "fail-injection")]
use crate::tests::fail_injection::{check_fault, maybe_fault, CommitFaultPoint};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio_postgres::Client;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

type PersistedFingerprintV2 = (Vec<u8>, Vec<u8>, Vec<u8>, bool, String);

/// Schema version written into every album manifest.
pub const MANIFEST_SCHEMA_VERSION: &str = "1.0";
pub const COMMIT_MARKER_SCHEMA_VERSION: &str = "1.0";
pub const COMMIT_MARKER_FILE_NAME: &str = ".imagedb-commit.json";
pub const LIBRARY_ROOT_LEASE_TTL_SECS: i64 = 300;
pub const MAX_TARGET_COMPONENT_CHARS: usize = 240;
pub const MAX_TARGET_RELATIVE_PATH_CHARS: usize = 512;
/// Maximum decoded pixel count for a single source file preview.
#[allow(dead_code)]
pub const PREVIEW_MAX_PIXELS: u64 = 8_000_000;
/// Maximum source file size (bytes) for a single source file preview.
#[allow(dead_code)]
pub const PREVIEW_MAX_SOURCE_BYTES: u64 = 80 * 1024 * 1024;

#[derive(Debug)]
pub(crate) enum SourceCleanupFailure {
    Conflict(String),
    Retryable(AppError),
}

impl SourceCleanupFailure {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Conflict(message) => message.clone(),
            Self::Retryable(error) => error.to_string(),
        }
    }
}

impl From<AppError> for SourceCleanupFailure {
    fn from(error: AppError) -> Self {
        Self::Retryable(error)
    }
}

fn classify_cleanup_snapshot_error(error: AppError) -> SourceCleanupFailure {
    match error {
        AppError::IoError(_) | AppError::PostgresUnavailable(_) => {
            SourceCleanupFailure::Retryable(error)
        }
        AppError::Internal(message) => SourceCleanupFailure::Conflict(message),
        other => SourceCleanupFailure::Retryable(other),
    }
}

pub(crate) fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn ensure_expected_plan_hash(
    import_run_id: Uuid,
    validated_plan_hash: &[u8],
    expected_plan_hash: Option<&str>,
) -> Result<(), AppError> {
    let Some(expected_plan_hash) = expected_plan_hash else {
        return Ok(());
    };
    let actual_plan_hash = bytes_to_hex(validated_plan_hash);
    if actual_plan_hash.eq_ignore_ascii_case(expected_plan_hash) {
        return Ok(());
    }
    Err(AppError::Internal(format!(
        "frozen import plan changed after confirmation for run {import_run_id}; expected hash {expected_plan_hash}, actual hash {actual_plan_hash}; review the plan again before committing"
    )))
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
    #[serde(default)]
    pub source_file_mode: SourceFileMode,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitMarker {
    pub schema_version: String,
    pub transaction_id: String,
    pub plan_hash: String,
    pub manifest_hash: String,
    pub publish_strategy_version: String,
    pub album_relative_path: String,
    pub image_count: u32,
    pub files: Vec<CommitMarkerFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitMarkerFile {
    pub relative_path: String,
    pub file_size: i64,
    pub blake3: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitPublishStrategy {
    StrongLocal,
    ConservativeMounted,
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
                validate_target_component(&s, rel)?;
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
    let normalized = parts.join("/");
    if normalized.chars().count() > MAX_TARGET_RELATIVE_PATH_CHARS {
        return Err(AppError::Internal(format!(
            "target relative path is too long ({} > {}): {normalized}",
            normalized.chars().count(),
            MAX_TARGET_RELATIVE_PATH_CHARS
        )));
    }
    Ok(normalized)
}

fn validate_target_component(component: &str, original: &str) -> Result<(), AppError> {
    if component.chars().count() > MAX_TARGET_COMPONENT_CHARS {
        return Err(AppError::Internal(format!(
            "target path component is too long ({} > {}): {component}",
            component.chars().count(),
            MAX_TARGET_COMPONENT_CHARS
        )));
    }
    if component.ends_with(' ') || component.ends_with('.') {
        return Err(AppError::Internal(format!(
            "target path component must not end with space or dot: {component}"
        )));
    }
    if is_windows_reserved_component(component) {
        return Err(AppError::Internal(format!(
            "target path component uses Windows reserved name '{component}' in '{original}'"
        )));
    }
    Ok(())
}

fn is_windows_reserved_component(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn normalized_conflict_key(path: &str) -> String {
    path.nfc().collect::<String>().to_lowercase()
}

/// Detect case conflicts and duplicate target paths within one album.
fn check_target_path_conflicts(images: &[PlanImageRow]) -> Result<(), AppError> {
    let mut seen: HashMap<String, String> = HashMap::new(); // folded+NFC -> original
    for img in images {
        let normalized = normalize_relative_path(&img.target_relative_path)?;
        let key = normalized_conflict_key(&normalized);
        if let Some(prev) = seen.get(&key) {
            if prev != &normalized {
                return Err(AppError::Internal(format!(
                    "target path case/Unicode conflict: '{prev}' vs '{normalized}'"
                )));
            }
            return Err(AppError::Internal(format!(
                "duplicate target relative path in plan: {normalized}"
            )));
        }
        seen.insert(key, normalized);
    }
    Ok(())
}

/// Read, parse, and BLAKE3-hash a published manifest from disk using the
/// on-disk bytes verbatim. The hash covers the exact byte sequence that was
/// written by `commit_single_album` / `publish_from_staging` — re-serializing
/// is forbidden because JSON whitespace is not canonical and would produce
/// a different hash from the persisted `file_transactions.manifest_hash`.
///
pub(crate) fn read_manifest_with_hash(dir: &Path) -> Result<(AlbumManifest, Vec<u8>), AppError> {
    let manifest_path = dir.join(".imagedb").join(".imagedb-manifest.json");
    let raw = std::fs::read(&manifest_path).map_err(|e| {
        AppError::Internal(format!(
            "cannot read manifest {}: {e}",
            manifest_path.display()
        ))
    })?;
    let hash = blake3::hash(&raw).as_bytes().to_vec();
    let manifest: AlbumManifest = serde_json::from_slice(&raw)
        .map_err(|e| AppError::Internal(format!("cannot parse manifest: {e}")))?;
    Ok((manifest, hash))
}

#[allow(dead_code)]
pub async fn run_import_commit(
    postgres_manager: Arc<Mutex<PostgresManager>>,
    library_root_path: String,
    import_run_id: Uuid,
    cancelled: Arc<AtomicBool>,
    progress_tracker: Arc<Mutex<CommitProgress>>,
) -> Result<CommitResult, AppError> {
    run_import_commit_with_expected_plan_hash(
        postgres_manager,
        library_root_path,
        import_run_id,
        cancelled,
        progress_tracker,
        None,
    )
    .await
}

pub async fn run_import_commit_with_expected_plan_hash(
    postgres_manager: Arc<Mutex<PostgresManager>>,
    library_root_path: String,
    import_run_id: Uuid,
    cancelled: Arc<AtomicBool>,
    progress_tracker: Arc<Mutex<CommitProgress>>,
    expected_plan_hash: Option<String>,
) -> Result<CommitResult, AppError> {
    let started_at = Instant::now();
    tracing::info!(
        %import_run_id,
        library_root_path = %library_root_path,
        "commit started"
    );

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
    DatabaseOperationLock::acquire_shared(&client, "import commit").await?;

    let result = execute_commit_pipeline(
        &mut client,
        &library_root_path,
        import_run_id,
        &cancelled,
        &progress_tracker,
        expected_plan_hash.as_deref(),
    )
    .await;

    drop(client);
    db_handle.abort();

    let mut progress = progress_tracker.lock().await;
    match &result {
        Ok(r) => {
            progress.state = r.state.clone();
            progress.current_stage = "done".to_string();
            tracing::info!(
                %import_run_id,
                final_state = %r.state,
                albums_total = r.albums_total,
                albums_committed = r.albums_committed,
                albums_skipped = r.albums_skipped,
                albums_failed = r.albums_failed,
                images_committed = r.images_committed,
                elapsed_ms = started_at.elapsed().as_millis(),
                "commit finished"
            );
        }
        Err(e) => {
            progress.state = "failed".to_string();
            progress.current_stage = "failed".to_string();
            progress.errors.push(e.to_string());
            tracing::error!(
                %import_run_id,
                error = %e,
                elapsed_ms = started_at.elapsed().as_millis(),
                "commit failed"
            );
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
    expected_plan_hash: Option<&str>,
) -> Result<CommitResult, AppError> {
    let import_run = ImportRepository::get_import_run_by_id(client, import_run_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("import run {import_run_id} not found")))?;

    let library_root_id = import_run.library_root_id;
    tracing::info!(
        %import_run_id,
        import_run_state = %import_run.state,
        %library_root_id,
        "commit import run loaded"
    );

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

    // Capture and hash the frozen plan under the same per-run row lock used
    // by every plan edit. The guarded transition to `committing` is committed
    // before releasing that lock, so an editor either finishes first (and we
    // read its new plan) or wakes after us and rejects the committing state.
    client
        .batch_execute("BEGIN")
        .await
        .map_err(|e| AppError::Internal(format!("failed to begin commit plan capture: {e}")))?;
    let capture_result = async {
        let locked_run = client
            .query_opt(
                "SELECT state FROM import_runs WHERE id = $1 FOR UPDATE",
                &[&import_run_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to lock import run for commit: {e}")))?
            .ok_or_else(|| AppError::Internal(format!("import run {import_run_id} not found")))?;
        let locked_run_state: String = locked_run.get("state");
        if !matches!(
            locked_run_state.as_str(),
            "ready_to_commit"
                | "cancelled"
                | "committing"
                | "recovery_required"
                | "failed"
                | "completed"
        ) {
            return Err(AppError::Internal(format!(
                "cannot commit import run {import_run_id} from state '{locked_run_state}'"
            )));
        }

        // Rule 3: the frozen plan is the sole commit source of truth.
        let frozen = ImportRepository::load_frozen_plan(client, import_run_id)
            .await?
            .ok_or_else(|| AppError::Internal(format!(
                "no frozen import plan for run {import_run_id}; generate and freeze a plan before committing"
            )))?;

        // Rule 4: validate and hash the immutable plan while the edit lock is
        // held. Commit and Recovery share this canonical hash validation.
        let validated_plan_hash = validate_and_hash_frozen_plan(&frozen, library_root_id)?;
        ensure_expected_plan_hash(import_run_id, &validated_plan_hash, expected_plan_hash)?;
        let publish_strategy = if frozen.albums.is_empty() {
            None
        } else {
            Some(select_commit_publish_strategy(&library_root)?)
        };
        ImportRepository::update_import_run_state(
            client,
            import_run_id,
            &ImportRunState::Committing,
        )
        .await?;
        Ok::<_, AppError>((frozen, validated_plan_hash, publish_strategy))
    }
    .await;
    let (frozen, validated_plan_hash, publish_strategy) = match capture_result {
        Ok(captured) => {
            client
                .batch_execute("COMMIT")
                .await
                .map_err(|e| AppError::Internal(format!("failed to commit plan capture: {e}")))?;
            captured
        }
        Err(error) => {
            let _ = client.batch_execute("ROLLBACK").await;
            return Err(error);
        }
    };
    tracing::info!(
        %import_run_id,
        plan_id = %frozen.plan_id,
        plan_state = %frozen.plan_state,
        source_file_mode = %frozen.source_file_mode,
        album_count = frozen.albums.len(),
        "commit frozen plan loaded"
    );

    if frozen.albums.is_empty() {
        // Phase 3: an empty plan must NOT bypass transaction checks. The
        // reconciler is the single authoritative decider: it completes the
        // run only if the full invariant set (plan state/hash, album/image
        // counts, and the complete file-transaction set) passes. An empty
        // plan with an active/conflict/failed/cancelled transaction routes
        // to `recovery_required` instead.
        let reconciled = reconcile_import_run_state(client, import_run_id).await?;
        let final_state = match reconciled.state {
            ImportRunState::Completed => "completed",
            ImportRunState::RecoveryRequired => "recovery_required",
            other => {
                return Err(AppError::Internal(format!(
                    "unexpected run state after empty-plan reconcile: {other}"
                )))
            }
        };
        return Ok(CommitResult {
            import_run_id: import_run_id.to_string(),
            source_file_mode: frozen.source_file_mode,
            albums_total: 0,
            albums_committed: 0,
            albums_skipped: 0,
            albums_failed: 0,
            images_committed: 0,
            album_results: Vec::new(),
            errors: Vec::new(),
            state: final_state.to_string(),
        });
    }

    let publish_strategy = publish_strategy.ok_or_else(|| {
        AppError::Internal("non-empty frozen plan has no publish strategy".to_string())
    })?;
    tracing::info!(
        %import_run_id,
        plan_id = %frozen.plan_id,
        source_file_mode = %frozen.source_file_mode,
        publish_strategy = ?publish_strategy,
        album_count = frozen.albums.len(),
        "commit immutable plan validated"
    );

    {
        let mut p = progress_tracker.lock().await;
        p.current_stage = "committing".to_string();
        p.albums_total = frozen.albums.len() as u32;
    }
    let mut album_results = Vec::new();
    let mut total_committed = 0u32;
    let mut albums_committed = 0u32;
    let mut albums_skipped = 0u32;
    let mut albums_failed = 0u32;
    let mut all_errors = Vec::new();
    let lease_owner = format!("imagedb-commit-{}", Uuid::new_v4());
    let lease_token = Uuid::new_v4();
    ImportRepository::acquire_library_root_lease(
        client,
        library_root_id,
        &lease_owner,
        lease_token,
        LIBRARY_ROOT_LEASE_TTL_SECS,
    )
    .await?;
    tracing::info!(
        %import_run_id,
        %library_root_id,
        %lease_token,
        lease_owner = %lease_owner,
        "commit library root lease acquired"
    );

    for (plan_album, images) in &frozen.albums {
        ImportRepository::heartbeat_library_root_lease(
            client,
            library_root_id,
            lease_token,
            LIBRARY_ROOT_LEASE_TTL_SECS,
        )
        .await?;

        if cancelled.load(Ordering::Relaxed) {
            all_errors.push("commit cancelled by user".to_string());
            tracing::warn!(%import_run_id, "commit cancellation observed before next album");
            break;
        }

        {
            let mut p = progress_tracker.lock().await;
            p.current_album = Some(plan_album.target_relative_path.clone());
            p.current_stage = "processing_album".to_string();
        }
        tracing::info!(
            %import_run_id,
            album = %plan_album.target_relative_path,
            image_count = images.len(),
            "commit album started"
        );

        let commit = PlanAlbumCommit {
            plan_album: plan_album.clone(),
            images: images.clone(),
        };

        match commit_single_album(
            client,
            &library_root,
            publish_strategy,
            library_root_id,
            import_run_id,
            frozen.plan_id,
            &validated_plan_hash,
            frozen.source_file_mode,
            cancelled,
            lease_token,
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
                    if let Some(error) = &result.error {
                        all_errors.push(error.clone());
                    }
                }
                tracing::info!(
                    %import_run_id,
                    album = %result.album_name,
                    status = %result.status,
                    images_committed = result.images_committed,
                    "commit album finished"
                );
                album_results.push(result);
            }
            Err(e) => {
                if let AppError::ResumeRequired(transaction_id) = e {
                    let msg = format!(
                        "detected incomplete transaction {transaction_id}; route to recovery"
                    );
                    all_errors.push(msg.clone());
                    tracing::warn!(
                        %import_run_id,
                        album = %plan_album.target_relative_path,
                        %transaction_id,
                        "commit album requires recovery"
                    );
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
                tracing::error!(
                    %import_run_id,
                    album = %plan_album.target_relative_path,
                    error = %e,
                    "commit album failed"
                );
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

        // Mid-pipeline reconcile: after each album attempt, the parent run
        // must already reflect the current transaction set. If this album
        // just entered conflict or stayed active, the run must flip to
        // recovery_required now, not at the end of the pipeline. The final
        // reconcile at the bottom is the authoritative pass; this one is a
        // correctness hook for progress observers.
        reconcile_import_run_state(client, import_run_id).await?;
    }

    ImportRepository::release_library_root_lease(client, library_root_id, lease_token).await?;
    tracing::info!(
        %import_run_id,
        %lease_token,
        "commit library root lease released"
    );

    // Phase 1: cancellation must NOT manufacture unrecoverable transactions.
    // Previously this block flipped every non-terminal, non-conflict
    // transaction to `failed`, which (a) made them un-recoverable (Recovery
    // rejects `failed`), and (b) leaked the failed state into the run. The
    // correct semantics: a recoverable transaction stays at its last
    // recoverable state so Recovery can resume it; the run is then marked
    // `recovery_required` by the reconciler (not `failed`). Only the run
    // itself may receive a user-initiated terminal label, and even then we
    // do not write it here — the reconciler owns run state.
    //
    // We intentionally do NOT mutate transaction states on cancel. The
    // mid-flight transaction is left at e.g. `staging`/`verified`/`published`
    // so `recover_transaction` can drive it forward on the next launch.

    // Final authoritative run-state decision. The reconciler inspects
    // every transaction against the frozen plan and writes the only state
    // the product allows: `completed` iff every frozen-plan album is
    // source_archived (or the plan is empty + invariants pass),
    // `recovery_required` otherwise. This replaces the previous
    // counter-based heuristic so a completed run cannot mask a surviving
    // recoverable transaction.
    //
    // Phase 2: the persisted DB state is the single source of truth. The
    // API/progress/GUI render exactly this state — no
    // `completed_with_errors` / `cancelled_pending_recovery` overlay. A
    // cancelled commit surfaces as `recovery_required` (there is a
    // mid-flight transaction to recover) unless the run had no
    // transactions at all, in which case reconcile leaves it at the
    // user-explicit state.
    let reconciled = reconcile_import_run_state(client, import_run_id).await?;

    // P0 fix: when the commit was cancelled and the reconciler would leave
    // the run at `recovery_required` but there are NO file_transactions for
    // this run, `recovery_required` is a GUI deadlock — the recovery page
    // shows "no recoverable transactions" and the commit page won't
    // re-select the run (it only picks up `completed` runs). The correct
    // state for "cancelled before any transaction was prewritten" is
    // `cancelled` (a user-initiated terminal label), which lets the user
    // re-enter the commit page for the same frozen plan.
    //
    // If at least one transaction exists (even mid-flight), `recovery_required`
    // is correct: Recovery can drive it forward.
    let final_state = if cancelled.load(Ordering::Relaxed)
        && reconciled.state == ImportRunState::RecoveryRequired
    {
        let tx_count = ImportRepository::get_all_transactions_for_run(client, import_run_id)
            .await?
            .len();
        if tx_count == 0 {
            // No transaction to recover → user-explicit terminal label so the
            // run can be re-committed from the frozen plan.
            ImportRepository::update_import_run_state(
                client,
                import_run_id,
                &ImportRunState::Cancelled,
            )
            .await?;
            ImportRunState::Cancelled
        } else {
            reconciled.state
        }
    } else {
        reconciled.state
    };

    let final_state = match final_state {
        ImportRunState::Completed => "completed",
        ImportRunState::RecoveryRequired => "recovery_required",
        ImportRunState::Cancelled => "cancelled",
        ImportRunState::Failed => "failed",
        other => {
            return Err(AppError::Internal(format!(
                "unexpected run state after commit reconcile: {other}"
            )))
        }
    };
    tracing::info!(
        %import_run_id,
        final_state,
        albums_committed,
        albums_skipped,
        albums_failed,
        images_committed = total_committed,
        error_count = all_errors.len(),
        "commit reconciled final state"
    );

    // Surface non-blocking diagnostics (album failure counts) on the run
    // row WITHOUT mutating the authoritative state. A run with failed
    // albums is `recovery_required` per the reconciler; this only writes
    // error_code/error_message metadata for operator diagnostics. It
    // deliberately does not use update_import_run_error (which would
    // overwrite state to `failed` and undo the reconciler's decision).
    if !all_errors.is_empty() {
        client
            .execute(
                "UPDATE import_runs SET error_code = $1, error_message = $2 WHERE id = $3",
                &[
                    &"commit_partial",
                    &format!(
                        "{albums_failed} album(s) failed, {} error(s)",
                        all_errors.len()
                    ),
                    &import_run_id,
                ],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to update partial commit diagnostics: {e}"))
            })?;
    }

    Ok(CommitResult {
        import_run_id: import_run_id.to_string(),
        source_file_mode: frozen.source_file_mode,
        albums_total: frozen.albums.len() as u32,
        albums_committed,
        albums_skipped,
        albums_failed,
        images_committed: total_committed,
        album_results,
        errors: all_errors,
        state: final_state.to_string(),
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
    canonical.extend_from_slice(frozen.source_file_mode.to_string().as_bytes());

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

pub(crate) fn ensure_no_symlink_or_reparse_escape(
    library_root: &Path,
    target_path: &Path,
) -> Result<(), AppError> {
    let root = library_root.canonicalize().map_err(|e| {
        AppError::IoError(format!(
            "cannot canonicalize library root {}: {e}",
            library_root.display()
        ))
    })?;
    let rel = target_path.strip_prefix(library_root).map_err(|_| {
        AppError::Internal(format!(
            "target path {} is outside library root {}",
            target_path.display(),
            library_root.display()
        ))
    })?;

    let mut current = library_root.to_path_buf();
    for component in rel.components() {
        match component {
            Component::Normal(part) => current.push(part),
            Component::CurDir => continue,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AppError::Internal(format!(
                    "target path escapes library root: {}",
                    target_path.display()
                )));
            }
        }

        match std::fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(AppError::Internal(format!(
                        "target ancestor is a symlink/reparse point: {}",
                        current.display()
                    )));
                }
                let canonical = current.canonicalize().map_err(|e| {
                    AppError::IoError(format!("cannot canonicalize {}: {e}", current.display()))
                })?;
                if !canonical.starts_with(&root) {
                    return Err(AppError::Internal(format!(
                        "target ancestor escapes library root: {} -> {}",
                        current.display(),
                        canonical.display()
                    )));
                }
            }
            Err(e) if e.kind() == ErrorKind::NotFound => break,
            Err(e) => {
                return Err(AppError::IoError(format!(
                    "cannot inspect target ancestor {}: {e}",
                    current.display()
                )));
            }
        }
    }

    Ok(())
}

/// Commit a single album using the staged file transaction protocol.
#[allow(clippy::too_many_arguments)]
async fn commit_single_album(
    client: &mut Client,
    library_root: &Path,
    publish_strategy: CommitPublishStrategy,
    library_root_id: Uuid,
    import_run_id: Uuid,
    plan_id: Uuid,
    plan_hash: &[u8],
    source_file_mode: SourceFileMode,
    cancelled: &Arc<AtomicBool>,
    lease_token: Uuid,
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
        if existing_tx.source_file_mode != source_file_mode {
            return Err(AppError::Internal(format!(
                "source file mode mismatch for existing transaction {}: stored {}, frozen plan {}",
                existing_tx.id, existing_tx.source_file_mode, source_file_mode
            )));
        }
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
                if !matches!(
                    tx_state,
                    Some(TransactionState::SourceArchived | TransactionState::SourceFilesRemoved)
                ) {
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
    ensure_no_symlink_or_reparse_escape(library_root, &publish_dir)?;
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
    ensure_no_symlink_or_reparse_escape(library_root, &staging_dir)?;

    let initial = state_machine::transition_transaction(TransactionState::Planned, "stage")?;
    let mut operations = Vec::with_capacity(images.len());
    for img in &images {
        let target_rel = normalize_relative_path(&img.target_relative_path)?;
        let staged_path = staging_dir.join(&target_rel);
        let target_path = publish_dir.join(&target_rel);
        operations.push(NewFileOperation {
            source_path: img.source_path.clone(),
            staging_path: staged_path.display().to_string(),
            target_path: target_path.display().to_string(),
            expected_size: img.expected_file_size,
            expected_blake3: img.expected_blake3.clone(),
            source_cleanup_quarantine_path: if source_file_mode
                == SourceFileMode::MoveSelectedWithoutBackup
            {
                Some(
                    build_source_quarantine_path(
                        Path::new(&img.source_path),
                        tx_id,
                        Uuid::new_v4(),
                    )?
                    .display()
                    .to_string(),
                )
            } else {
                None
            },
        });
    }

    let operation_ids = ImportRepository::prewrite_file_transaction(
        client,
        tx_id,
        import_run_id,
        plan_album.import_album_id,
        &initial,
        &staging_dir.display().to_string(),
        &publish_dir.display().to_string(),
        plan_hash,
        source_file_mode,
        &operations,
    )
    .await?;
    let op_ids: Vec<(Uuid, PathBuf, Vec<u8>)> = operation_ids
        .into_iter()
        .zip(operations.iter())
        .map(|(operation_id, operation)| {
            (
                operation_id,
                PathBuf::from(&operation.staging_path),
                operation.expected_blake3.clone(),
            )
        })
        .collect();

    #[cfg(feature = "fail-injection")]
    maybe_fault(CommitFaultPoint::AfterDbWrite, "after DB write")?;

    // ── Phase 2: stream copy to staging with .part + incremental BLAKE3. ──
    tokio::fs::create_dir_all(&staging_dir)
        .await
        .map_err(|e| AppError::IoError(format!("cannot create staging dir: {e}")))?;

    for (i, img) in images.iter().enumerate() {
        ImportRepository::heartbeat_library_root_lease(
            client,
            library_root_id,
            lease_token,
            LIBRARY_ROOT_LEASE_TTL_SECS,
        )
        .await?;

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

        // P1: pass the cancel token into the stream copy so a mid-copy
        // cancel stops between read chunks and leaves the operation in
        // `copying` (recoverable) rather than running the whole file to
        // completion.
        let actual_blake3 = match stream_copy_with_hash(src, &part_path, Some(cancelled)).await {
            Ok(hash) => hash,
            Err(e) => {
                // Cancel mid-copy: leave the op in `copying` so Recovery
                // can resume. Do NOT mark the transaction `failed`.
                let msg = e.to_string();
                if msg.contains("cancelled during file copy") {
                    return Err(e);
                }
                // Any other error propagates (caller's match arm handles
                // it as a recovery_required album result).
                return Err(e);
            }
        };
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
    ImportRepository::heartbeat_library_root_lease(
        client,
        library_root_id,
        lease_token,
        LIBRARY_ROOT_LEASE_TTL_SECS,
    )
    .await?;
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
        source_file_mode,
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

    // ── Phase 4: publish. StrongLocal uses the historical whole-directory
    // rename. ConservativeMounted copies verified files and writes an
    // immutable commit marker last.
    ImportRepository::heartbeat_library_root_lease(
        client,
        library_root_id,
        lease_token,
        LIBRARY_ROOT_LEASE_TTL_SECS,
    )
    .await?;
    let publishing = state_machine::transition_transaction(TransactionState::Verified, "publish")?;
    ImportRepository::update_file_transaction_state(client, tx_id, &publishing, None).await?;

    #[cfg(feature = "fail-injection")]
    maybe_fault(
        CommitFaultPoint::BeforePublishRename,
        "before publish rename",
    )?;

    publish_verified_staging(
        publish_strategy,
        library_root,
        &staging_dir,
        &publish_dir,
        tx_id,
        plan_hash,
        &manifest_hash,
        &album_relative_path,
        &images,
    )
    .await?;

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
    ImportRepository::heartbeat_library_root_lease(
        client,
        library_root_id,
        lease_token,
        LIBRARY_ROOT_LEASE_TTL_SECS,
    )
    .await?;
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

    // Phase 4: verify the persisted source snapshot's album path agrees
    // with the import album's source_path. Mismatch → conflict (never an
    // auto-fix), so a snapshot captured for a different album cannot vouch
    // for this archive.
    if let Err(e) =
        validate_snapshot_album_path_identity(&snapshot.source_album_path, &source_album_dir)
    {
        let msg = e.to_string();
        ImportRepository::update_file_transaction_state(
            client,
            tx_id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Err(e);
    }

    if source_file_mode == SourceFileMode::MoveSelectedWithoutBackup {
        // The destructive stage is allowed only after the already-published
        // files, manifest, file-operation journal and committed DB records
        // have all been re-verified from persisted evidence. This mirrors the
        // recovery gate and keeps the direct path from relying only on the
        // success of the immediately preceding calls.
        let persisted_tx = ImportRepository::get_file_transaction(client, tx_id)
            .await?
            .ok_or_else(|| {
                AppError::Internal(format!(
                    "file transaction {tx_id} disappeared before source cleanup"
                ))
            })?;
        match verify_committed_evidence_before_source_cleanup(
            client,
            library_root,
            library_root_id,
            &persisted_tx,
            plan_id,
            plan_hash,
            &album_relative_path,
            &images,
        )
        .await?
        {
            IdempotencyVerdict::AlreadyCommitted => {}
            IdempotencyVerdict::Conflict(message) => {
                ImportRepository::update_file_transaction_state(
                    client,
                    tx_id,
                    &TransactionState::Conflict,
                    Some(&message),
                )
                .await?;
                return Err(AppError::Internal(format!(
                    "refusing source cleanup because committed evidence conflicts: {message}"
                )));
            }
            IdempotencyVerdict::Resume { .. } => {
                return Err(AppError::Internal(
                    "committed evidence verifier unexpectedly requested recovery before source cleanup"
                        .to_string(),
                ));
            }
        }

        let removing = state_machine::transition_transaction(
            TransactionState::LibraryCommitted,
            "remove_source_files",
        )?;
        ImportRepository::update_file_transaction_state(client, tx_id, &removing, None).await?;
        if let Err(error) = remove_selected_source_files(
            client,
            tx_id,
            &source_album_dir,
            &snapshot.snapshot_hash,
            &snapshot_files,
            &images,
        )
        .await
        {
            let message = error.message().to_string();
            match error {
                SourceCleanupFailure::Conflict(_) => {
                    ImportRepository::update_file_transaction_state(
                        client,
                        tx_id,
                        &TransactionState::Conflict,
                        Some(&message),
                    )
                    .await?;
                    return Err(AppError::Internal(message));
                }
                SourceCleanupFailure::Retryable(error) => {
                    ImportRepository::update_file_transaction_state(
                        client,
                        tx_id,
                        &TransactionState::SourceFilesRemoving,
                        Some(&message),
                    )
                    .await?;
                    return Err(error);
                }
            }
        }
        let removed = state_machine::transition_transaction(
            TransactionState::SourceFilesRemoving,
            "removed",
        )?;
        ImportRepository::update_file_transaction_state(client, tx_id, &removed, None).await?;

        if let Err(error) = tokio::fs::remove_dir_all(&staging_base).await {
            if staging_base.exists() {
                let msg = format!("staging cleanup failed: {error}");
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
        return Ok(CommitAlbumResult {
            album_name: album_relative_path,
            status: "committed".to_string(),
            images_committed: image_count,
            target_path: Some(publish_dir.display().to_string()),
            manifest_path: Some(published_manifest_path.display().to_string()),
            error: None,
        });
    }

    // Phase 4: archive root is derived from the **persisted
    // import_runs.source_root** — never from `source_album_dir.parent()`
    // (which can be empty / `.` for root-level albums) and never from a
    // plan image parent. The album relative path is computed against
    // source_root so the archive always lives under the user's source tree.
    let archive_dir = compute_archive_dir(
        client,
        import_run_id,
        &source_album_dir,
        &album_relative_path,
        tx_id,
    )
    .await?;

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

fn build_source_quarantine_path(
    source_path: &Path,
    transaction_id: Uuid,
    token: Uuid,
) -> Result<PathBuf, AppError> {
    let parent = source_path.parent().ok_or_else(|| {
        AppError::Internal(format!(
            "selected source has no parent directory: {}",
            source_path.display()
        ))
    })?;
    if source_path.file_name().is_none() {
        return Err(AppError::Internal(format!(
            "selected source has no file name: {}",
            source_path.display()
        )));
    }
    Ok(parent.join(format!(".imagedb-moving-{transaction_id}-{token}")))
}

fn cleanup_relative_path(
    source_album_dir: &Path,
    path: &Path,
    label: &str,
) -> Result<String, SourceCleanupFailure> {
    let relative = path.strip_prefix(source_album_dir).map_err(|_| {
        SourceCleanupFailure::Conflict(format!(
            "{label} {} is outside source album {}",
            path.display(),
            source_album_dir.display()
        ))
    })?;
    normalize_relative_path(&relative.to_string_lossy()).map_err(|error| {
        SourceCleanupFailure::Conflict(format!("invalid {label} {}: {error}", path.display()))
    })
}

fn validate_cleanup_quarantine_path(
    source_album_dir: &Path,
    source_path: &Path,
    quarantine_path: &Path,
    transaction_id: Uuid,
) -> Result<(String, String), SourceCleanupFailure> {
    let source_relative = cleanup_relative_path(source_album_dir, source_path, "cleanup source")?;
    let quarantine_relative =
        cleanup_relative_path(source_album_dir, quarantine_path, "cleanup quarantine")?;
    let source_parent = source_path.parent().ok_or_else(|| {
        SourceCleanupFailure::Conflict(format!(
            "cleanup source has no parent: {}",
            source_path.display()
        ))
    })?;
    let quarantine_parent = quarantine_path.parent().ok_or_else(|| {
        SourceCleanupFailure::Conflict(format!(
            "cleanup quarantine has no parent: {}",
            quarantine_path.display()
        ))
    })?;
    let expected_prefix = format!(".imagedb-moving-{transaction_id}-");
    let quarantine_name = quarantine_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if !path_eq(source_parent, quarantine_parent)
        || source_path == quarantine_path
        || !quarantine_name.starts_with(&expected_prefix)
    {
        return Err(SourceCleanupFailure::Conflict(format!(
            "invalid persisted quarantine path '{}' for selected source '{}'",
            quarantine_path.display(),
            source_path.display()
        )));
    }
    Ok((source_relative, quarantine_relative))
}

async fn cleanup_path_exists(path: &Path) -> Result<bool, SourceCleanupFailure> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(SourceCleanupFailure::Retryable(AppError::IoError(format!(
            "cannot inspect cleanup path {}: {error}",
            path.display()
        )))),
    }
}

async fn ensure_cleanup_album_accessible(
    source_album_dir: &Path,
) -> Result<(), SourceCleanupFailure> {
    match tokio::fs::symlink_metadata(source_album_dir).await {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
            Ok(())
        }
        Ok(_) => Err(SourceCleanupFailure::Conflict(format!(
            "source album is no longer a regular directory: {}",
            source_album_dir.display()
        ))),
        Err(error) => Err(SourceCleanupFailure::Retryable(AppError::IoError(format!(
            "source album is temporarily unavailable for cleanup {}: {error}",
            source_album_dir.display()
        )))),
    }
}

async fn cleanup_conflict(
    client: &Client,
    operation_id: Uuid,
    message: String,
) -> SourceCleanupFailure {
    match ImportRepository::update_source_file_cleanup_operation(
        client,
        operation_id,
        "conflict",
        Some(&message),
    )
    .await
    {
        Ok(()) => SourceCleanupFailure::Conflict(message),
        Err(error) => SourceCleanupFailure::Retryable(error),
    }
}

async fn cleanup_retryable(
    client: &Client,
    operation_id: Uuid,
    state: &str,
    message: String,
) -> SourceCleanupFailure {
    let persist_result = ImportRepository::update_source_file_cleanup_operation(
        client,
        operation_id,
        state,
        Some(&message),
    )
    .await;
    match persist_result {
        Ok(()) => SourceCleanupFailure::Retryable(AppError::IoError(message)),
        Err(error) => SourceCleanupFailure::Retryable(error),
    }
}

/// Remove only frozen-plan source files after publish and library DB evidence
/// have succeeded. Each persisted source path is first atomically renamed to
/// its unique same-directory quarantine path. Size and BLAKE3 verification,
/// and the eventual unlink, operate on that quarantined identity rather than
/// reopening the original path. A replacement created at the original path
/// after rename is therefore never deleted.
pub(crate) async fn remove_selected_source_files(
    client: &Client,
    tx_id: Uuid,
    source_album_dir: &Path,
    snapshot_hash: &[u8],
    snapshot_files: &[SnapshotFileRecord],
    images: &[PlanImageRow],
) -> Result<(), SourceCleanupFailure> {
    ensure_cleanup_album_accessible(source_album_dir).await?;
    if let Err(error) = validate_plan_image_sources(source_album_dir, images) {
        return Err(match error {
            AppError::Internal(message) => SourceCleanupFailure::Conflict(message),
            other => SourceCleanupFailure::Retryable(other),
        });
    }
    let mut operations = ImportRepository::get_source_file_cleanup_operations(client, tx_id)
        .await
        .map_err(SourceCleanupFailure::Retryable)?;
    if operations.len() != images.len() {
        return Err(SourceCleanupFailure::Conflict(format!(
            "source cleanup set mismatch for transaction {tx_id}: {} rows for {} frozen images",
            operations.len(),
            images.len()
        )));
    }

    let plan_by_source: HashMap<&str, &PlanImageRow> = images
        .iter()
        .map(|image| (image.source_path.as_str(), image))
        .collect();
    if plan_by_source.len() != images.len() {
        return Err(SourceCleanupFailure::Conflict(format!(
            "frozen plan for transaction {tx_id} contains duplicate source paths"
        )));
    }
    for operation in &operations {
        let Some(image) = plan_by_source.get(operation.source_path.as_str()) else {
            let message = format!(
                "cleanup operation {} is not a frozen-plan source: {}",
                operation.id, operation.source_path
            );
            return Err(cleanup_conflict(client, operation.id, message).await);
        };
        if operation.expected_size != image.expected_file_size
            || operation.expected_blake3 != image.expected_blake3
        {
            let message = format!(
                "cleanup evidence mismatch for frozen source {}",
                operation.source_path
            );
            return Err(cleanup_conflict(client, operation.id, message).await);
        }
    }

    // 0016 rows may predate quarantine persistence. Normalize their old
    // state semantics and assign a unique path in one PostgreSQL update so a
    // crash can never expose a new quarantine path with an old `verifying`
    // meaning. Old `removing` means the file was already hash-verified: if it
    // is absent, the legacy unlink completed; if present, restart isolation.
    for operation in &mut operations {
        if operation.quarantine_path.is_none() {
            let source_exists = cleanup_path_exists(Path::new(&operation.source_path)).await?;
            let legacy_normalized_state = match operation.state.as_str() {
                "pending" | "verifying" | "removing" if source_exists => "pending",
                "removing" => "removed",
                "removed" if !source_exists => "removed",
                "pending" | "verifying" => {
                    return Err(cleanup_conflict(
                        client,
                        operation.id,
                        format!(
                            "legacy selected source disappeared before verified removal: {}",
                            operation.source_path
                        ),
                    )
                    .await);
                }
                "removed" => {
                    return Err(cleanup_conflict(
                        client,
                        operation.id,
                        format!(
                            "legacy selected source reappeared after persisted removal: {}",
                            operation.source_path
                        ),
                    )
                    .await);
                }
                "conflict" => {
                    return Err(SourceCleanupFailure::Conflict(format!(
                        "source cleanup operation {} is already in conflict",
                        operation.id
                    )));
                }
                other => {
                    return Err(SourceCleanupFailure::Conflict(format!(
                        "invalid legacy source cleanup state '{other}' for {}",
                        operation.source_path
                    )));
                }
            };
            let generated = build_source_quarantine_path(
                Path::new(&operation.source_path),
                tx_id,
                operation.id,
            )
            .map_err(|error| SourceCleanupFailure::Conflict(error.to_string()))?;
            let (persisted_path, persisted_state) =
                ImportRepository::initialize_source_cleanup_quarantine_if_missing(
                    client,
                    operation.id,
                    &generated.display().to_string(),
                    legacy_normalized_state,
                )
                .await
                .map_err(SourceCleanupFailure::Retryable)?;
            operation.quarantine_path = Some(persisted_path);
            operation.state = persisted_state;
        }
    }

    let mut ignored_paths = HashSet::new();
    for operation in &operations {
        let source_path = PathBuf::from(&operation.source_path);
        let quarantine_path = PathBuf::from(
            operation
                .quarantine_path
                .as_deref()
                .expect("quarantine path assigned above"),
        );
        let (source_relative, quarantine_relative) = validate_cleanup_quarantine_path(
            source_album_dir,
            &source_path,
            &quarantine_path,
            tx_id,
        )?;
        let source_exists = cleanup_path_exists(&source_path).await?;
        let quarantine_exists = cleanup_path_exists(&quarantine_path).await?;
        match operation.state.as_str() {
            "pending" if quarantine_exists => {
                return Err(cleanup_conflict(
                    client,
                    operation.id,
                    format!(
                        "quarantine path existed before cleanup began: {}",
                        quarantine_path.display()
                    ),
                )
                .await);
            }
            "removing" if !source_exists && !quarantine_exists => {
                return Err(cleanup_conflict(
                    client,
                    operation.id,
                    format!(
                        "selected source and quarantine both disappeared before isolation completed: {}",
                        source_path.display()
                    ),
                )
                .await);
            }
            "verifying" | "removed" => {
                ignored_paths.insert(source_relative);
                ignored_paths.insert(quarantine_relative);
            }
            "removing" if quarantine_exists => {
                ignored_paths.insert(source_relative);
                ignored_paths.insert(quarantine_relative);
            }
            "pending" | "removing" => {}
            "conflict" => {
                return Err(SourceCleanupFailure::Conflict(format!(
                    "source cleanup operation {} is already in conflict",
                    operation.id
                )));
            }
            other => {
                return Err(SourceCleanupFailure::Conflict(format!(
                    "invalid source cleanup state '{other}' for {}",
                    source_path.display()
                )));
            }
        }
    }

    if ignored_paths.is_empty() {
        let errors = verify_source_snapshot_files_async(
            source_album_dir,
            snapshot_hash.to_vec(),
            snapshot_files.to_vec(),
        )
        .await
        .map_err(classify_cleanup_snapshot_error)?;
        if !errors.is_empty() {
            return Err(SourceCleanupFailure::Conflict(format!(
                "source album before selected-file removal {} does not match captured snapshot: {}",
                source_album_dir.display(),
                errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            )));
        }
    } else {
        let errors = verify_source_snapshot_files_ignoring_paths_async(
            source_album_dir,
            snapshot_files.to_vec(),
            ignored_paths,
        )
        .await
        .map_err(classify_cleanup_snapshot_error)?;
        if !errors.is_empty() {
            return Err(SourceCleanupFailure::Conflict(format!(
                "source album changed during selected-file recovery: {}",
                errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            )));
        }
    }

    for operation in operations {
        let source_path = PathBuf::from(&operation.source_path);
        let quarantine_path = PathBuf::from(
            operation
                .quarantine_path
                .as_deref()
                .expect("quarantine path assigned above"),
        );
        let mut quarantine_exists = cleanup_path_exists(&quarantine_path).await?;

        if operation.state == "removed" {
            if quarantine_exists {
                return Err(cleanup_conflict(
                    client,
                    operation.id,
                    format!(
                        "quarantined file reappeared after persisted removal: {}",
                        quarantine_path.display()
                    ),
                )
                .await);
            }
            continue;
        }

        if operation.state == "verifying" && !quarantine_exists {
            ImportRepository::update_source_file_cleanup_operation(
                client,
                operation.id,
                "removed",
                None,
            )
            .await
            .map_err(SourceCleanupFailure::Retryable)?;
            continue;
        }

        if !quarantine_exists {
            let source_metadata = match tokio::fs::symlink_metadata(&source_path).await {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    return Err(cleanup_conflict(
                        client,
                        operation.id,
                        format!(
                            "selected source disappeared before quarantine rename: {}",
                            source_path.display()
                        ),
                    )
                    .await);
                }
                Err(error) => {
                    return Err(cleanup_retryable(
                        client,
                        operation.id,
                        "removing",
                        format!(
                            "cannot inspect selected source {}: {error}",
                            source_path.display()
                        ),
                    )
                    .await);
                }
            };
            if !source_metadata.file_type().is_file() || source_metadata.file_type().is_symlink() {
                return Err(cleanup_conflict(
                    client,
                    operation.id,
                    format!(
                        "selected source is not a regular file: {}",
                        source_path.display()
                    ),
                )
                .await);
            }
            ImportRepository::update_source_file_cleanup_operation(
                client,
                operation.id,
                "removing",
                None,
            )
            .await
            .map_err(SourceCleanupFailure::Retryable)?;
            if let Err(error) = tokio::fs::rename(&source_path, &quarantine_path).await {
                quarantine_exists = cleanup_path_exists(&quarantine_path).await?;
                let source_exists = cleanup_path_exists(&source_path).await?;
                if !quarantine_exists && !source_exists {
                    return Err(cleanup_conflict(
                        client,
                        operation.id,
                        format!(
                            "selected source and quarantine both disappeared during rename: {}",
                            source_path.display()
                        ),
                    )
                    .await);
                }
                if !quarantine_exists {
                    return Err(cleanup_retryable(
                        client,
                        operation.id,
                        "removing",
                        format!(
                            "cannot atomically quarantine selected source {}: {error}",
                            source_path.display()
                        ),
                    )
                    .await);
                }
            }
            if let Err(error) = sync_parent_dir(&quarantine_path).await {
                return Err(cleanup_retryable(
                    client,
                    operation.id,
                    "removing",
                    format!(
                        "cannot sync quarantine rename for {}: {error}",
                        quarantine_path.display()
                    ),
                )
                .await);
            }
            ImportRepository::update_source_file_cleanup_operation(
                client,
                operation.id,
                "verifying",
                None,
            )
            .await
            .map_err(SourceCleanupFailure::Retryable)?;
        }

        let metadata = match tokio::fs::symlink_metadata(&quarantine_path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                ImportRepository::update_source_file_cleanup_operation(
                    client,
                    operation.id,
                    "removed",
                    None,
                )
                .await
                .map_err(SourceCleanupFailure::Retryable)?;
                continue;
            }
            Err(error) => {
                return Err(cleanup_retryable(
                    client,
                    operation.id,
                    "verifying",
                    format!(
                        "cannot inspect quarantined source {}: {error}",
                        quarantine_path.display()
                    ),
                )
                .await);
            }
        };
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(cleanup_conflict(
                client,
                operation.id,
                format!(
                    "quarantined source is not a regular file: {}; original path {} is never overwritten automatically; quarantined entry retained for manual resolution",
                    quarantine_path.display(),
                    source_path.display()
                ),
            )
            .await);
        }
        if metadata.len() as i64 != operation.expected_size {
            return Err(cleanup_conflict(
                client,
                operation.id,
                format!(
                    "quarantined source size changed before removal: {}; original path {} is never overwritten automatically; quarantined file retained for manual resolution",
                    quarantine_path.display(),
                    source_path.display()
                ),
            )
            .await);
        }

        let actual_hash = match hash_existing_file(&quarantine_path).await {
            Ok(hash) => hash,
            Err(error) => {
                return Err(cleanup_retryable(
                    client,
                    operation.id,
                    "verifying",
                    format!(
                        "cannot hash quarantined source {}: {error}",
                        quarantine_path.display()
                    ),
                )
                .await);
            }
        };
        if actual_hash != operation.expected_blake3 {
            return Err(cleanup_conflict(
                client,
                operation.id,
                format!(
                    "quarantined source BLAKE3 changed before removal: {}; original path {} is never overwritten automatically; quarantined file retained for manual resolution",
                    quarantine_path.display(),
                    source_path.display()
                ),
            )
            .await);
        }

        #[cfg(feature = "fail-injection")]
        if check_fault(CommitFaultPoint::DuringSelectedSourceRemoval) {
            return Err(cleanup_retryable(
                client,
                operation.id,
                "verifying",
                format!(
                    "cannot remove quarantined source {}: injected PermissionDenied sharing violation",
                    quarantine_path.display()
                ),
            )
            .await);
        }

        tracing::info!(
            transaction_id = %tx_id,
            source_path = %source_path.display(),
            quarantine_path = %quarantine_path.display(),
            "removing verified quarantined frozen-plan source file"
        );
        if let Err(error) = tokio::fs::remove_file(&quarantine_path).await {
            return Err(cleanup_retryable(
                client,
                operation.id,
                "verifying",
                format!(
                    "cannot remove quarantined source {}: {error}",
                    quarantine_path.display()
                ),
            )
            .await);
        }
        if let Err(error) = sync_parent_dir(&quarantine_path).await {
            return Err(cleanup_retryable(
                client,
                operation.id,
                "verifying",
                format!(
                    "cannot sync quarantined source removal {}: {error}",
                    quarantine_path.display()
                ),
            )
            .await);
        }
        ImportRepository::update_source_file_cleanup_operation(
            client,
            operation.id,
            "removed",
            None,
        )
        .await
        .map_err(SourceCleanupFailure::Retryable)?;
    }
    Ok(())
}

pub(crate) async fn verify_source_snapshot_or_conflict(
    client: &Client,
    tx_id: Uuid,
    dir: &Path,
    snapshot_hash: &[u8],
    snapshot_files: &[SnapshotFileRecord],
    label: &str,
) -> Result<Option<String>, AppError> {
    // Phase 5: the blocking directory walk + BLAKE3 hashing is isolated on
    // a spawn_blocking task so the async runtime is never blocked while
    // verifying a large album against its captured snapshot.
    let errors = match verify_source_snapshot_files_async(
        dir,
        snapshot_hash.to_vec(),
        snapshot_files.to_vec(),
    )
    .await
    {
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
///
/// P1: accepts a cancel token and checks it **between read chunks** so a
/// large file copy responds promptly to cancellation instead of running to
/// completion. On cancel, the `.part` destination is removed (caller is
/// responsible for this in the commit path; here we only stop copying and
/// return `Err`), and the operation is left in `copying`/`planned` state
/// so Recovery can resume it. The caller must propagate the cancel error
/// and NOT mark the transaction `failed`.
pub(crate) async fn stream_copy_with_hash(
    src: &Path,
    dst: &Path,
    cancelled: Option<&Arc<AtomicBool>>,
) -> Result<Vec<u8>, AppError> {
    let mut src_file = tokio::fs::File::open(src)
        .await
        .map_err(|e| AppError::IoError(format!("cannot open source {}: {e}", src.display())))?;
    let mut dst_file = tokio::fs::File::create(dst).await.map_err(|e| {
        AppError::IoError(format!("cannot create staging part {}: {e}", dst.display()))
    })?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 65536];
    loop {
        // Check cancellation before each read chunk so a mid-copy cancel
        // stops promptly rather than draining the whole source file.
        if let Some(flag) = cancelled {
            if flag.load(Ordering::Relaxed) {
                // Best-effort cleanup of the partial destination so the next
                // attempt starts clean. A failure to remove is not fatal —
                // the caller also removes `.part` before re-copying.
                let _ = tokio::fs::remove_file(dst).await;
                return Err(AppError::Internal(
                    "commit cancelled during file copy".to_string(),
                ));
            }
        }
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

pub(crate) fn select_commit_publish_strategy(
    library_root: &Path,
) -> Result<CommitPublishStrategy, AppError> {
    #[cfg(feature = "fail-injection")]
    if crate::tests::fail_injection::force_conservative_publish() {
        return Ok(CommitPublishStrategy::ConservativeMounted);
    }

    let capabilities = probe_storage_capabilities(library_root);
    match capabilities.publish_strategy {
        StoragePublishStrategy::StrongLocal => Ok(CommitPublishStrategy::StrongLocal),
        StoragePublishStrategy::ConservativeMounted => {
            Ok(CommitPublishStrategy::ConservativeMounted)
        }
        StoragePublishStrategy::Unsupported => Err(AppError::Internal(format!(
            "library root '{}' is unsupported for commit: {}",
            library_root.display(),
            capabilities.strategy_reasons.join("; ")
        ))),
    }
}

pub(crate) fn build_commit_marker(
    tx_id: Uuid,
    plan_hash: &[u8],
    manifest_hash: &[u8],
    album_relative_path: &str,
    images: &[PlanImageRow],
) -> CommitMarker {
    let mut files: Vec<CommitMarkerFile> = images
        .iter()
        .map(|img| CommitMarkerFile {
            relative_path: normalize_relative_path(&img.target_relative_path)
                .unwrap_or_else(|_| img.target_relative_path.clone()),
            file_size: img.expected_file_size,
            blake3: bytes_to_hex(&img.expected_blake3),
        })
        .collect();
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

    CommitMarker {
        schema_version: COMMIT_MARKER_SCHEMA_VERSION.to_string(),
        transaction_id: tx_id.to_string(),
        plan_hash: bytes_to_hex(plan_hash),
        manifest_hash: bytes_to_hex(manifest_hash),
        publish_strategy_version: "m8-conservative-marker-v1".to_string(),
        album_relative_path: album_relative_path.to_string(),
        image_count: images.len() as u32,
        files,
    }
}

pub(crate) fn read_commit_marker(publish_dir: &Path) -> Result<CommitMarker, AppError> {
    let marker_path = publish_dir.join(".imagedb").join(COMMIT_MARKER_FILE_NAME);
    let raw = std::fs::read(&marker_path).map_err(|e| {
        AppError::Internal(format!(
            "cannot read commit marker {}: {e}",
            marker_path.display()
        ))
    })?;
    serde_json::from_slice(&raw)
        .map_err(|e| AppError::Internal(format!("cannot parse commit marker: {e}")))
}

pub(crate) fn validate_commit_marker(
    marker: &CommitMarker,
    tx_id: Uuid,
    plan_hash: &[u8],
    manifest_hash: &[u8],
    album_relative_path: &str,
    images: &[PlanImageRow],
) -> Result<(), String> {
    let expected =
        build_commit_marker(tx_id, plan_hash, manifest_hash, album_relative_path, images);
    if marker.schema_version != expected.schema_version {
        return Err(format!(
            "commit marker schema_version {} != expected {}",
            marker.schema_version, expected.schema_version
        ));
    }
    if marker.transaction_id != expected.transaction_id {
        return Err(format!(
            "commit marker transaction_id {} != expected {}",
            marker.transaction_id, expected.transaction_id
        ));
    }
    if marker.plan_hash != expected.plan_hash {
        return Err(format!(
            "commit marker plan_hash {} != expected {}",
            marker.plan_hash, expected.plan_hash
        ));
    }
    if marker.manifest_hash != expected.manifest_hash {
        return Err(format!(
            "commit marker manifest_hash {} != expected {}",
            marker.manifest_hash, expected.manifest_hash
        ));
    }
    if marker.album_relative_path != expected.album_relative_path {
        return Err(format!(
            "commit marker album_relative_path '{}' != expected '{}'",
            marker.album_relative_path, expected.album_relative_path
        ));
    }
    if marker.image_count != expected.image_count {
        return Err(format!(
            "commit marker image_count {} != expected {}",
            marker.image_count, expected.image_count
        ));
    }
    if marker.files != expected.files {
        return Err("commit marker file set does not match frozen plan".to_string());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn publish_verified_staging(
    strategy: CommitPublishStrategy,
    library_root: &Path,
    staging_dir: &Path,
    publish_dir: &Path,
    tx_id: Uuid,
    plan_hash: &[u8],
    manifest_hash: &[u8],
    album_relative_path: &str,
    images: &[PlanImageRow],
) -> Result<(), AppError> {
    ensure_no_symlink_or_reparse_escape(library_root, publish_dir)?;
    if publish_dir.exists() {
        return Err(AppError::Internal(format!(
            "target directory appeared during publish: {}",
            publish_dir.display()
        )));
    }

    match strategy {
        CommitPublishStrategy::StrongLocal => {
            tokio::fs::rename(staging_dir, publish_dir)
                .await
                .map_err(|e| AppError::IoError(format!("atomic publish rename failed: {e}")))?;
            sync_parent_dir(publish_dir).await?;
        }
        CommitPublishStrategy::ConservativeMounted => {
            publish_verified_staging_conservatively(
                staging_dir,
                publish_dir,
                tx_id,
                plan_hash,
                manifest_hash,
                album_relative_path,
                images,
            )
            .await?;
            return Ok(());
        }
    }

    write_commit_marker(
        publish_dir,
        tx_id,
        plan_hash,
        manifest_hash,
        album_relative_path,
        images,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn publish_verified_staging_conservatively(
    staging_dir: &Path,
    publish_dir: &Path,
    tx_id: Uuid,
    plan_hash: &[u8],
    manifest_hash: &[u8],
    album_relative_path: &str,
    images: &[PlanImageRow],
) -> Result<(), AppError> {
    tokio::fs::create_dir_all(publish_dir)
        .await
        .map_err(|e| AppError::IoError(format!("cannot create conservative target dir: {e}")))?;

    for img in images {
        let target_rel = normalize_relative_path(&img.target_relative_path)?;
        let staged_path = staging_dir.join(&target_rel);
        let target_path = publish_dir.join(&target_rel);
        if let Some(parent) = target_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| AppError::IoError(format!("cannot create target subdir: {e}")))?;
        }
        let part_path = publish_dir.join(format!("{target_rel}.part"));
        let _ = tokio::fs::remove_file(&part_path).await;
        let actual = stream_copy_with_hash(&staged_path, &part_path, None).await?;
        if actual != img.expected_blake3 {
            let _ = tokio::fs::remove_file(&part_path).await;
            return Err(AppError::Internal(format!(
                "conservative publish hash mismatch for {}",
                target_path.display()
            )));
        }
        tokio::fs::rename(&part_path, &target_path)
            .await
            .map_err(|e| AppError::IoError(format!("publish file rename failed: {e}")))?;
        sync_parent_dir(&target_path).await?;
    }

    let staging_manifest = staging_dir.join(".imagedb").join(".imagedb-manifest.json");
    let target_manifest = publish_dir.join(".imagedb").join(".imagedb-manifest.json");
    if let Some(parent) = target_manifest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| AppError::IoError(format!("cannot create target manifest dir: {e}")))?;
    }
    let target_manifest_part = publish_dir
        .join(".imagedb")
        .join(".imagedb-manifest.json.part");
    let actual_manifest_hash =
        stream_copy_with_hash(&staging_manifest, &target_manifest_part, None).await?;
    if actual_manifest_hash != manifest_hash {
        let _ = tokio::fs::remove_file(&target_manifest_part).await;
        return Err(AppError::Internal(
            "conservative publish manifest hash mismatch".to_string(),
        ));
    }
    tokio::fs::rename(&target_manifest_part, &target_manifest)
        .await
        .map_err(|e| AppError::IoError(format!("publish manifest rename failed: {e}")))?;
    sync_parent_dir(&target_manifest).await?;

    #[cfg(feature = "fail-injection")]
    maybe_fault(CommitFaultPoint::BeforeCommitMarker, "before commit marker")?;

    write_commit_marker(
        publish_dir,
        tx_id,
        plan_hash,
        manifest_hash,
        album_relative_path,
        images,
    )
    .await?;
    tokio::fs::remove_dir_all(staging_dir)
        .await
        .map_err(|e| AppError::IoError(format!("cannot remove conservative staging dir: {e}")))?;
    sync_parent_dir(staging_dir).await?;
    Ok(())
}

pub(crate) async fn write_commit_marker(
    publish_dir: &Path,
    tx_id: Uuid,
    plan_hash: &[u8],
    manifest_hash: &[u8],
    album_relative_path: &str,
    images: &[PlanImageRow],
) -> Result<(), AppError> {
    let marker = build_commit_marker(tx_id, plan_hash, manifest_hash, album_relative_path, images);
    let marker_json = serde_json::to_string_pretty(&marker)
        .map_err(|e| AppError::Internal(format!("commit marker serialize failed: {e}")))?;
    let marker_dir = publish_dir.join(".imagedb");
    tokio::fs::create_dir_all(&marker_dir)
        .await
        .map_err(|e| AppError::IoError(format!("cannot create marker dir: {e}")))?;
    let marker_tmp = marker_dir.join(".imagedb-commit.json.tmp");
    let marker_file = marker_dir.join(COMMIT_MARKER_FILE_NAME);
    write_synced_then_rename(&marker_tmp, &marker_file, marker_json.as_bytes()).await
}

pub(crate) async fn verify_published_file_set(
    publish_dir: &Path,
    images: &[PlanImageRow],
) -> Result<(), AppError> {
    for img in images {
        let target_rel = normalize_relative_path(&img.target_relative_path)?;
        let file_path = publish_dir.join(&target_rel);
        let meta = tokio::fs::metadata(&file_path).await.map_err(|e| {
            AppError::IoError(format!(
                "published file missing {}: {e}",
                file_path.display()
            ))
        })?;
        if meta.len() != img.expected_file_size as u64 {
            return Err(AppError::Internal(format!(
                "published file size mismatch for {}: expected {} got {}",
                file_path.display(),
                img.expected_file_size,
                meta.len()
            )));
        }
        let actual = hash_existing_file(&file_path).await?;
        if actual != img.expected_blake3 {
            return Err(AppError::Internal(format!(
                "published BLAKE3 mismatch for {}: expected {} got {}",
                file_path.display(),
                bytes_to_hex(&img.expected_blake3),
                bytes_to_hex(&actual)
            )));
        }
    }
    Ok(())
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
            Err(e) if is_known_unsupported_dir_sync_error(&e) => Ok(()),
            Err(e) => Err(AppError::IoError(format!(
                "parent directory sync failed for {}: {e}",
                parent.display()
            ))),
        },
        Err(e) if is_known_unsupported_dir_open_error(&e, parent) => Ok(()),
        Err(e) => Err(AppError::IoError(format!(
            "cannot open parent directory for sync {}: {e}",
            parent.display()
        ))),
    }
}

fn is_known_unsupported_dir_open_error(e: &std::io::Error, parent: &Path) -> bool {
    if is_known_unsupported_dir_sync_error(e) {
        return true;
    }
    #[cfg(windows)]
    {
        // Tokio/std File::open can report ERROR_ACCESS_DENIED when the path is
        // an existing directory because directory handles require different
        // Windows flags. Treat only this directory-open case as unsupported;
        // permission errors from sync_all() or missing/non-directory paths
        // still propagate.
        if e.kind() == ErrorKind::PermissionDenied && e.raw_os_error() == Some(5) && parent.is_dir()
        {
            return true;
        }
    }
    #[cfg(not(windows))]
    let _ = parent;
    false
}

/// Decide whether an I/O error from `sync_all()`-ing a directory represents a
/// *known, deterministic* "directory fsync is not supported by this
/// platform/filesystem" signal that we may safely downgrade to success.
///
/// Only `ErrorKind::Unsupported` is accepted unconditionally. Otherwise we
/// require a `raw_os_error` that is on the platform-specific whitelist of
/// "invalid request / not supported" codes:
///
/// - Windows: `ERROR_INVALID_FUNCTION` (1), `ERROR_NOT_SUPPORTED` (50),
///   `ERROR_INVALID_PARAMETER` (87).
/// - Unix-like: `EINVAL` (22), `ENOSYS` (38), `EOPNOTSUPP` (45),
///   `ENOTSUP` (95).
///
/// Every error without a whitelisted OS code (notably `PermissionDenied`,
/// `NotFound`, `Interrupted`, `TimedOut`, `WouldBlock`, ...) is *not*
/// downgraded and must propagate as a real `AppError::IoError`, so genuine
/// network-drive / permission / I/O failures surface to the caller.
fn is_known_unsupported_dir_sync_error(e: &std::io::Error) -> bool {
    #[cfg(windows)]
    const UNSUPPORTED_DIR_SYNC_RAW_CODES: &[i32] = &[
        1,  // ERROR_INVALID_FUNCTION
        50, // ERROR_NOT_SUPPORTED
        87, // ERROR_INVALID_PARAMETER
    ];
    #[cfg(not(windows))]
    const UNSUPPORTED_DIR_SYNC_RAW_CODES: &[i32] = &[
        22, // EINVAL
        38, // ENOSYS
        45, // EOPNOTSUPP (Linux)
        95, // ENOTSUP (POSIX / macOS alias on some libcs)
    ];

    match e.kind() {
        ErrorKind::Unsupported => true,
        _ => match e.raw_os_error() {
            Some(code) => UNSUPPORTED_DIR_SYNC_RAW_CODES.contains(&code),
            None => false,
        },
    }
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

/// Compute the archive directory for a transaction from the **persisted
/// `import_runs.source_root`** + the transaction id + album relative path.
///
/// Result: `<source_root>/.imagedb-processed/<tx_id>/<album_relative_path>`.
///
/// This is the Phase 4 fix: the archive root must always live under the
/// persisted source root, never under `source_album_dir.parent()` (which
/// can be `.` for root-level albums) and never under a plan image parent.
/// Returns an `AppError` if the source root is missing, non-absolute, or
/// the album path escapes it.
///
/// `source_album_dir` is the authoritative album directory read from
/// `import_albums.source_path`; it is verified to live under `source_root`
/// (after canonicalization when both exist) before the archive location is
/// computed.
pub(crate) async fn compute_archive_dir(
    client: &Client,
    import_run_id: Uuid,
    source_album_dir: &Path,
    album_relative_path: &str,
    tx_id: Uuid,
) -> Result<PathBuf, AppError> {
    let run = ImportRepository::get_import_run_by_id(client, import_run_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("import run {import_run_id} not found")))?;
    let source_root = PathBuf::from(&run.source_root);
    if source_root.as_os_str().is_empty() {
        return Err(AppError::Internal(format!(
            "import run {import_run_id} has empty source_root"
        )));
    }
    if !source_root.is_absolute() {
        return Err(AppError::Internal(format!(
            "import run {import_run_id} source_root is not absolute: {}",
            source_root.display()
        )));
    }

    // Verify the album directory is contained by the persisted source root.
    validate_album_under_root(&source_root, source_album_dir)?;

    let archive_dir = source_root
        .join(".imagedb-processed")
        .join(tx_id.to_string())
        .join(normalize_relative_path(album_relative_path)?);
    // The archive dir must end up under the source root (defense-in-depth
    // against a malformed album_relative_path that somehow traversed up).
    if !archive_dir.starts_with(&source_root) {
        return Err(AppError::Internal(format!(
            "archive dir {} escaped source_root {} (album_relative_path='{}')",
            archive_dir.display(),
            source_root.display(),
            album_relative_path
        )));
    }
    Ok(archive_dir)
}

/// Verify that `source_album_dir` is contained by the persisted
/// `source_root`. Canonicalization is used when both paths exist (resolves
/// symlinks + Windows case). If `source_root` does not exist on disk, fall
/// back to lexical containment on the raw paths (after rejecting any `..`
/// traversal), so an archive-only recovery that re-points at a remounted
/// root still works. Reject relative paths, `..` traversal, and any escape.
pub(crate) fn validate_album_under_root(
    source_root: &Path,
    source_album_dir: &Path,
) -> Result<(), AppError> {
    if !source_album_dir.is_absolute() {
        return Err(AppError::Internal(format!(
            "source album dir is not absolute: {}",
            source_album_dir.display()
        )));
    }
    // Reject `..` traversal lexically first, regardless of existence.
    for comp in source_album_dir.components() {
        if matches!(comp, Component::ParentDir) {
            return Err(AppError::Internal(format!(
                "source album dir contains '..': {}",
                source_album_dir.display()
            )));
        }
    }

    if source_root.exists() && source_album_dir.exists() {
        let canonical_root = source_root.canonicalize().map_err(|e| {
            AppError::IoError(format!(
                "cannot canonicalize source_root {}: {e}",
                source_root.display()
            ))
        })?;
        let canonical_album = source_album_dir.canonicalize().map_err(|e| {
            AppError::IoError(format!(
                "cannot canonicalize source album dir {}: {e}",
                source_album_dir.display()
            ))
        })?;
        if !canonical_album.starts_with(&canonical_root) {
            return Err(AppError::Internal(format!(
                "source album dir '{}' escapes persisted source_root '{}' (resolved '{}' not under '{}')",
                source_album_dir.display(),
                source_root.display(),
                canonical_album.display(),
                canonical_root.display()
            )));
        }
        Ok(())
    } else {
        // Lexical containment fallback when paths don't exist on disk.
        if !source_album_dir.starts_with(source_root) {
            return Err(AppError::Internal(format!(
                "source album dir '{}' is not under persisted source_root '{}' (lexical)",
                source_album_dir.display(),
                source_root.display()
            )));
        }
        Ok(())
    }
}

/// Verify that the persisted `source_album_snapshots.source_album_path`
/// agrees with `import_albums.source_path`. They must denote the same
/// path (canonical equality when both exist; lexical equality otherwise).
/// Mismatch → `AppError` (never an auto-fix), so a snapshot captured for a
/// different album cannot be used to vouch for this archive.
pub(crate) fn validate_snapshot_album_path_identity(
    snapshot_album_path: &str,
    source_album_dir: &Path,
) -> Result<(), AppError> {
    if snapshot_album_path.is_empty() {
        return Err(AppError::Internal(
            "source_album_snapshots.source_album_path is empty".to_string(),
        ));
    }
    let snapshot_path = Path::new(snapshot_album_path);
    if !snapshot_path.is_absolute() {
        return Err(AppError::Internal(format!(
            "source_album_snapshots.source_album_path is not absolute: {snapshot_album_path}"
        )));
    }
    if snapshot_path.exists() && source_album_dir.exists() {
        let canonical_snapshot = snapshot_path.canonicalize().map_err(|e| {
            AppError::IoError(format!(
                "cannot canonicalize snapshot album path {}: {e}",
                snapshot_path.display()
            ))
        })?;
        let canonical_album = source_album_dir.canonicalize().map_err(|e| {
            AppError::IoError(format!(
                "cannot canonicalize source album dir {}: {e}",
                source_album_dir.display()
            ))
        })?;
        if canonical_snapshot != canonical_album {
            return Err(AppError::Internal(format!(
                "source snapshot path '{}' does not match import album source_path '{}' (resolved '{}' vs '{}')",
                snapshot_album_path,
                source_album_dir.display(),
                canonical_snapshot.display(),
                canonical_album.display()
            )));
        }
    } else if !path_eq(snapshot_path, source_album_dir) {
        return Err(AppError::Internal(format!(
            "source snapshot path '{}' does not match import album source_path '{}' (lexical)",
            snapshot_album_path,
            source_album_dir.display()
        )));
    }
    Ok(())
}

/// Verify that a directory contains *exactly* the file set prescribed by the
/// frozen plan: same relative paths, same sizes, same BLAKE3 hashes, and no
/// extra or missing entries. Subdirectories not referenced by any plan image
/// are treated as unexpected entries.
///
/// Test helper retained to exercise exact directory validation semantics used
/// by the source snapshot verifier.
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
    source_file_mode: SourceFileMode,
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
        source_file_mode,
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
                fingerprint_version: Some("2".to_string()),
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

    let import_image_ids: Vec<Uuid> = images.iter().map(|image| image.import_image_id).collect();
    let fingerprint_rows = transaction
        .query(
            "SELECT id, pixel_hash, block_hash_16, double_gradient_hash_32,
                    perceptual_eligible, fingerprint_version
             FROM import_images
             WHERE id = ANY($1)",
            &[&import_image_ids],
        )
        .await
        .map_err(|error| {
            AppError::Internal(format!(
                "failed to batch load frozen-plan fingerprints: {error}"
            ))
        })?;
    let fingerprints: HashMap<Uuid, PersistedFingerprintV2> = fingerprint_rows
        .iter()
        .map(|row| {
            let id: Uuid = row.get("id");
            let pixel_hash: Option<Vec<u8>> = row.get("pixel_hash");
            let block_hash: Option<Vec<u8>> = row.get("block_hash_16");
            let double_gradient_hash: Option<Vec<u8>> = row.get("double_gradient_hash_32");
            let perceptual_eligible: bool = row.get("perceptual_eligible");
            let version: Option<String> = row.get("fingerprint_version");
            match (pixel_hash, block_hash, double_gradient_hash, version) {
                (Some(pixel), Some(block), Some(fine), Some(version)) if version == "2" => {
                    Ok((id, (pixel, block, fine, perceptual_eligible, version)))
                }
                _ => Err(AppError::Internal(format!(
                    "frozen plan image {id} does not have a complete Fingerprint V2"
                ))),
            }
        })
        .collect::<Result<_, _>>()?;
    if fingerprints.len() != images.len() {
        return Err(AppError::Internal(format!(
            "frozen plan fingerprint count {} does not match image count {}",
            fingerprints.len(),
            images.len()
        )));
    }

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
        let (
            pixel_hash,
            block_hash_16,
            double_gradient_hash_32,
            perceptual_eligible,
            fingerprint_version,
        ) = fingerprints.get(&img.import_image_id).ok_or_else(|| {
            AppError::Internal(format!(
                "frozen plan image {} is missing persisted Fingerprint V2 evidence",
                img.import_image_id
            ))
        })?;
        let image_id = Uuid::new_v4();
        transaction
            .execute(
                "INSERT INTO library_images
                 (id, album_id, relative_path, file_size, width, height, format,
                  blake3, pixel_hash, block_hash_16, double_gradient_hash_32,
                  perceptual_eligible, fingerprint_version, state)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, 'committed')",
                &[
                    &image_id,
                    &library_album_id,
                    &target_rel,
                    &img.expected_file_size,
                    &img.width,
                    &img.height,
                    &img.format,
                    &img.expected_blake3,
                    pixel_hash,
                    block_hash_16,
                    double_gradient_hash_32,
                    perceptual_eligible,
                    fingerprint_version,
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

/// Run the same complete publish/manifest/file-operation/database evidence
/// validation while a transaction is between `library_committed` and its
/// source-file terminal state. The verifier normally gates on the terminal
/// state for commit idempotency; recovery uses this view only to prove that
/// deleting frozen source files is safe before it resumes cleanup.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn verify_committed_evidence_before_source_cleanup(
    client: &Client,
    library_root: &Path,
    library_root_id: Uuid,
    existing_tx: &crate::repositories::import_repository::FileTransactionFullRow,
    plan_id: Uuid,
    plan_hash: &[u8],
    album_relative_path: &str,
    images: &[PlanImageRow],
) -> Result<IdempotencyVerdict, AppError> {
    let mut terminal_view = existing_tx.clone();
    terminal_view.state = match existing_tx.source_file_mode {
        SourceFileMode::CopyAndArchive => TransactionState::SourceArchived,
        SourceFileMode::MoveSelectedWithoutBackup => TransactionState::SourceFilesRemoved,
    }
    .to_string();
    verify_complete_evidence(
        client,
        library_root,
        library_root_id,
        &terminal_view,
        plan_id,
        plan_hash,
        album_relative_path,
        images,
    )
    .await
}

/// Rule 12: complete idempotency verification. Returns `AlreadyCommitted`
/// only when every piece of evidence matches — transaction id, plan id, plan
/// hash, the raw-byte manifest hash, schema version, every identity field
/// inside the manifest, the published directory + manifest + every file's
/// path/size/BLAKE3 (no extra files), file_operations rows, and the DB
/// album + image records.
///
/// The manifest hash is computed over the on-disk bytes verbatim, never over
/// a re-serialization, so a whitespace/content edit of the manifest file is
/// detected as a mismatch.
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
    let import_run_id = existing_tx.import_run_id;
    let import_album_id = existing_tx.import_album_id;

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
    let expected_terminal = match existing_tx.source_file_mode {
        SourceFileMode::CopyAndArchive => TransactionState::SourceArchived,
        SourceFileMode::MoveSelectedWithoutBackup => TransactionState::SourceFilesRemoved,
    };
    if tx_state != expected_terminal {
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
            "transaction {} is {} but published dir {} is missing",
            existing_tx.id,
            expected_terminal,
            publish_dir.display()
        )));
    }

    // Manifest must parse from raw bytes and its raw-byte BLAKE3 must match
    // the persisted manifest_hash. Re-serialization is forbidden here.
    let (manifest, raw_manifest_hash) = match read_manifest_with_hash(&publish_dir) {
        Ok(pair) => pair,
        Err(e) => {
            return Ok(IdempotencyVerdict::Conflict(format!(
                "manifest unreadable/unparseable: {e}"
            )));
        }
    };
    match &existing_tx.manifest_hash {
        Some(stored) => {
            if stored != &raw_manifest_hash {
                return Ok(IdempotencyVerdict::Conflict(format!(
                    "manifest_hash mismatch: stored {} raw-byte {}",
                    bytes_to_hex(stored),
                    bytes_to_hex(&raw_manifest_hash)
                )));
            }
        }
        None => {
            return Ok(IdempotencyVerdict::Conflict(
                "transaction has no manifest_hash".to_string(),
            ));
        }
    }

    // Strict identity: every frozen field inside the manifest must agree
    // with the transaction row, the frozen plan, and the call parameters.
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return Ok(IdempotencyVerdict::Conflict(format!(
            "manifest schema_version {} != expected {}",
            manifest.schema_version, MANIFEST_SCHEMA_VERSION
        )));
    }
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
    if manifest.plan_hash != bytes_to_hex(plan_hash) {
        return Ok(IdempotencyVerdict::Conflict(format!(
            "manifest plan_hash {} != expected {}",
            manifest.plan_hash,
            bytes_to_hex(plan_hash)
        )));
    }
    if manifest.import_run_id != import_run_id.to_string() {
        return Ok(IdempotencyVerdict::Conflict(format!(
            "manifest import_run_id {} != expected {}",
            manifest.import_run_id, import_run_id
        )));
    }
    if manifest.import_album_id != import_album_id.to_string() {
        return Ok(IdempotencyVerdict::Conflict(format!(
            "manifest import_album_id {} != expected {}",
            manifest.import_album_id, import_album_id
        )));
    }
    if manifest.library_root_id != library_root_id.to_string() {
        return Ok(IdempotencyVerdict::Conflict(format!(
            "manifest library_root_id {} != expected {}",
            manifest.library_root_id, library_root_id
        )));
    }
    if manifest.album_relative_path != album_relative_path {
        return Ok(IdempotencyVerdict::Conflict(format!(
            "manifest album_relative_path '{}' != expected '{}'",
            manifest.album_relative_path, album_relative_path
        )));
    }
    if manifest.source_file_mode != existing_tx.source_file_mode {
        return Ok(IdempotencyVerdict::Conflict(format!(
            "manifest source_file_mode {} != transaction {}",
            manifest.source_file_mode, existing_tx.source_file_mode
        )));
    }
    if manifest.image_count != images.len() as u32 {
        return Ok(IdempotencyVerdict::Conflict(format!(
            "manifest image_count {} != plan {}",
            manifest.image_count,
            images.len()
        )));
    }
    if manifest.images.len() != images.len() {
        return Ok(IdempotencyVerdict::Conflict(format!(
            "manifest images array length {} != plan {}",
            manifest.images.len(),
            images.len()
        )));
    }

    // Build a manifest-by-relative lookup and verify every plan image has a
    // manifest entry with matching source_path / file_size / blake3. Then
    // verify every manifest entry is also in the plan (no extras).
    let mut manifest_by_rel: HashMap<String, &AlbumManifestImage> = HashMap::new();
    for m in &manifest.images {
        if manifest_by_rel.insert(m.relative_path.clone(), m).is_some() {
            return Ok(IdempotencyVerdict::Conflict(format!(
                "manifest has duplicate relative_path '{}'",
                m.relative_path
            )));
        }
    }

    // Every expected file must exist on disk with the right size + BLAKE3,
    // AND every manifest entry must resolve to a plan image with matching
    // source_path / file_size / blake3. Track seen rels so we can detect
    // extra on-disk entries below.
    let mut seen_rels: std::collections::HashSet<String> = std::collections::HashSet::new();
    for img in images {
        let target_rel = normalize_relative_path(&img.target_relative_path)?;
        seen_rels.insert(target_rel.clone());

        let m_entry = match manifest_by_rel.get(&target_rel) {
            Some(e) => e,
            None => {
                return Ok(IdempotencyVerdict::Conflict(format!(
                    "file {} missing from manifest",
                    target_rel
                )));
            }
        };
        if m_entry.source_path != img.source_path {
            return Ok(IdempotencyVerdict::Conflict(format!(
                "manifest source_path mismatch for {target_rel}: manifest '{}' plan '{}'",
                m_entry.source_path, img.source_path
            )));
        }
        if m_entry.file_size != img.expected_file_size {
            return Ok(IdempotencyVerdict::Conflict(format!(
                "manifest file_size mismatch for {target_rel}: manifest {} plan {}",
                m_entry.file_size, img.expected_file_size
            )));
        }
        if m_entry.blake3 != bytes_to_hex(&img.expected_blake3) {
            return Ok(IdempotencyVerdict::Conflict(format!(
                "manifest blake3 mismatch for {target_rel}"
            )));
        }

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

    // No extra manifest entries beyond the plan set (already implied by
    // length equality above, but be explicit for defense-in-depth).
    for rel in manifest_by_rel.keys() {
        if !seen_rels.contains(rel) {
            return Ok(IdempotencyVerdict::Conflict(format!(
                "manifest has extra entry not in frozen plan: {rel}"
            )));
        }
    }

    // No extra on-disk files beyond the plan + management files. Allowed
    // names inside the publish dir: the plan images plus `.imagedb/` and
    // `.imagedb/.imagedb-manifest.json`.
    if let Some(msg) = detect_extra_published_files(&publish_dir, &seen_rels).await {
        return Ok(IdempotencyVerdict::Conflict(msg));
    }

    let marker = match read_commit_marker(&publish_dir) {
        Ok(marker) => marker,
        Err(e) => {
            return Ok(IdempotencyVerdict::Conflict(format!(
                "commit marker missing or invalid: {e}"
            )));
        }
    };
    if let Err(msg) = validate_commit_marker(
        &marker,
        existing_tx.id,
        plan_hash,
        &raw_manifest_hash,
        album_relative_path,
        images,
    ) {
        return Ok(IdempotencyVerdict::Conflict(msg));
    }

    // file_operations rows must cover the plan images exactly and match
    // expected_size / expected_blake3 / target_path for this transaction.
    let ops = ImportRepository::get_file_operations(client, existing_tx.id).await?;
    if ops.len() != images.len() {
        return Ok(IdempotencyVerdict::Conflict(format!(
            "file_operations count {} != plan {}",
            ops.len(),
            images.len()
        )));
    }
    let mut ops_by_target: HashMap<
        String,
        &crate::repositories::import_repository::FileOperationRow,
    > = HashMap::new();
    for op in &ops {
        let rel = match Path::new(&op.target_path).strip_prefix(&publish_dir) {
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => {
                return Ok(IdempotencyVerdict::Conflict(format!(
                    "file_operation target_path outside published album: {}",
                    op.target_path
                )));
            }
        };
        // Guard against duplicate ops targeting the same rel.
        if ops_by_target.insert(rel.clone(), op).is_some() {
            return Ok(IdempotencyVerdict::Conflict(format!(
                "file_operations has duplicate target_path for rel '{rel}'"
            )));
        }
    }
    for img in images {
        let target_rel = normalize_relative_path(&img.target_relative_path)?;
        let op = match ops_by_target.get(&target_rel) {
            Some(o) => o,
            None => {
                return Ok(IdempotencyVerdict::Conflict(format!(
                    "file_operation missing for {target_rel}"
                )));
            }
        };
        if op.expected_size != img.expected_file_size {
            return Ok(IdempotencyVerdict::Conflict(format!(
                "file_operation expected_size mismatch for {target_rel}: op {} plan {}",
                op.expected_size, img.expected_file_size
            )));
        }
        if op.expected_blake3 != img.expected_blake3 {
            return Ok(IdempotencyVerdict::Conflict(format!(
                "file_operation expected_blake3 mismatch for {target_rel}"
            )));
        }
        let expected_target = publish_dir.join(&target_rel).display().to_string();
        if op.target_path != expected_target {
            return Ok(IdempotencyVerdict::Conflict(format!(
                "file_operation target_path mismatch for {target_rel}: op '{}' expected '{}'",
                op.target_path, expected_target
            )));
        }
    }
    // No extra ops beyond the plan set (length already equal, but be explicit).
    for rel in ops_by_target.keys() {
        if !seen_rels.contains(rel) {
            return Ok(IdempotencyVerdict::Conflict(format!(
                "file_operation has extra target_path not in frozen plan: {rel}"
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
    if lib_album.manifest_hash != raw_manifest_hash {
        return Ok(IdempotencyVerdict::Conflict(
            "library_album.manifest_hash != raw-byte manifest hash".to_string(),
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
    let mut db_by_rel: HashMap<String, (i64, Vec<u8>)> = db_images
        .iter()
        .map(|r| (r.relative_path.clone(), (r.file_size, r.blake3.clone())))
        .collect();
    for img in images {
        let target_rel = normalize_relative_path(&img.target_relative_path)?;
        match db_by_rel.remove(&target_rel) {
            Some((size, blake3))
                if blake3 == img.expected_blake3 && size == img.expected_file_size => {}
            _ => {
                return Ok(IdempotencyVerdict::Conflict(format!(
                    "library_image record mismatch for {target_rel}"
                )));
            }
        }
    }

    Ok(IdempotencyVerdict::AlreadyCommitted)
}

/// Walk the published album directory and reject any regular file that is
/// neither a plan image nor the canonical manifest. Directories are allowed
/// only as containers; every regular file under them must still be planned.
/// Returns `Some(conflict_message)` on the first conflict.
pub(crate) async fn detect_extra_published_files(
    publish_dir: &Path,
    plan_rels: &std::collections::HashSet<String>,
) -> Option<String> {
    let mut stack: Vec<PathBuf> = vec![publish_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => return Some(format!("cannot read_dir {}", dir.display())),
        };
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(e)) => e,
                Ok(None) => break,
                Err(_) => return Some(format!("read_dir error at {}", dir.display())),
            };
            let ft = match entry.file_type().await {
                Ok(ft) => ft,
                Err(_) => return Some(format!("file_type failed for {}", entry.path().display())),
            };
            let path = entry.path();
            if ft.is_dir() {
                stack.push(path);
                continue;
            }
            if !ft.is_file() {
                return Some(format!(
                    "unexpected non-regular file in published album: {}",
                    path.display()
                ));
            }
            // Compute the relative path inside publish_dir (normalized to `/`).
            let rel = match path.strip_prefix(publish_dir) {
                Ok(r) => r.to_string_lossy().replace('\\', "/"),
                Err(_) => return Some(format!("strip_prefix failed for {}", path.display())),
            };
            // Management files are allowed.
            if rel == ".imagedb/.imagedb-manifest.json"
                || rel == ".imagedb-manifest.json"
                || rel == format!(".imagedb/{COMMIT_MARKER_FILE_NAME}")
            {
                continue;
            }
            if rel.starts_with(".imagedb/") {
                return Some(format!(
                    "unexpected management file in published album: {rel}"
                ));
            }
            if !plan_rels.contains(&rel) {
                return Some(format!(
                    "extra file in published album not in frozen plan: {rel}"
                ));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn cleanup_snapshot_io_is_retryable_but_evidence_errors_conflict() {
        assert!(matches!(
            classify_cleanup_snapshot_error(AppError::IoError("storage offline".to_string())),
            SourceCleanupFailure::Retryable(_)
        ));
        assert!(matches!(
            classify_cleanup_snapshot_error(AppError::Internal(
                "unsupported filesystem entry (symlink)".to_string()
            )),
            SourceCleanupFailure::Conflict(_)
        ));
    }

    #[tokio::test]
    async fn missing_cleanup_album_is_retryable_storage_unavailability() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("offline-album");
        assert!(matches!(
            ensure_cleanup_album_accessible(&missing).await,
            Err(SourceCleanupFailure::Retryable(_))
        ));
    }

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
    fn normalize_rejects_reserved_trailing_and_long_components() {
        assert!(normalize_relative_path("album/CON.jpg").is_err());
        assert!(normalize_relative_path("album/NUL").is_err());
        assert!(normalize_relative_path("album/name.").is_err());
        assert!(normalize_relative_path("album/name ").is_err());
        let long = format!("album/{}.jpg", "a".repeat(MAX_TARGET_COMPONENT_CHARS + 1));
        assert!(normalize_relative_path(&long).is_err());
    }

    #[test]
    fn normalize_rejects_long_relative_paths() {
        let mut rel = String::new();
        while rel.chars().count() <= MAX_TARGET_RELATIVE_PATH_CHARS {
            if !rel.is_empty() {
                rel.push('/');
            }
            rel.push_str("segment");
        }
        assert!(normalize_relative_path(&rel).is_err());
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
    fn detect_unicode_normalization_conflict() {
        let images = vec![
            plan_image("album/cafe\u{00e9}.jpg", &[1; 32]),
            plan_image("album/cafee\u{0301}.jpg", &[2; 32]),
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

    #[test]
    fn target_symlink_ancestor_is_rejected_when_platform_allows_symlink() {
        let tmp = TempDir::new().unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let albums = tmp.path().join("Albums");

        if create_dir_symlink(&outside, &albums).is_err() {
            return;
        }

        let target = albums.join("album_a");
        let err = ensure_no_symlink_or_reparse_escape(tmp.path(), &target)
            .expect_err("symlink ancestor must be rejected")
            .to_string();
        assert!(
            err.contains("symlink") || err.contains("escape"),
            "unexpected error: {err}"
        );
    }

    #[cfg(unix)]
    fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

    fn plan_image(rel: &str, blake3: &[u8]) -> PlanImageRow {
        PlanImageRow {
            included: true,
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
        let m = build_manifest(
            &tx,
            plan,
            &[1; 32],
            run,
            album,
            root,
            "a",
            SourceFileMode::CopyAndArchive,
            &images,
        );
        let json = serde_json::to_string_pretty(&m).unwrap();
        let back: AlbumManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.schema_version, MANIFEST_SCHEMA_VERSION);
        assert_eq!(back.transaction_id, tx.to_string());
        assert_eq!(back.plan_hash, bytes_to_hex(&[1u8; 32]));
        assert_eq!(back.image_count, 1);
        assert_eq!(back.images[0].blake3, bytes_to_hex(&[7u8; 32]));
    }

    #[test]
    fn commit_rejects_a_plan_hash_changed_after_confirmation() {
        let run_id = Uuid::new_v4();
        let actual = [7u8; 32];
        ensure_expected_plan_hash(run_id, &actual, Some(&bytes_to_hex(&actual)))
            .expect("the confirmed hash should be accepted");

        let error = ensure_expected_plan_hash(run_id, &actual, Some(&bytes_to_hex(&[8u8; 32])))
            .expect_err("a stale confirmed hash must block commit");
        assert!(error.to_string().contains("changed after confirmation"));
    }

    #[test]
    fn commit_marker_binds_transaction_plan_manifest_and_files() {
        let tx = Uuid::new_v4();
        let images = vec![plan_image("a/1.jpg", &[7; 32])];
        let marker = build_commit_marker(tx, &[1; 32], &[2; 32], "album_a", &images);

        validate_commit_marker(&marker, tx, &[1; 32], &[2; 32], "album_a", &images)
            .expect("marker should match its source data");

        let err = validate_commit_marker(&marker, tx, &[9; 32], &[2; 32], "album_a", &images)
            .unwrap_err();
        assert!(
            err.contains("plan_hash"),
            "marker must reject a mismatched plan hash: {err}"
        );
    }

    #[tokio::test]
    async fn conservative_publish_copies_files_writes_marker_and_removes_staging() {
        let tmp = TempDir::new().unwrap();
        let staging = tmp.path().join("staging").join("album_a");
        let publish = tmp.path().join("Albums").join("album_a");
        tokio::fs::create_dir_all(staging.join("a")).await.unwrap();
        tokio::fs::create_dir_all(staging.join(".imagedb"))
            .await
            .unwrap();

        let image_bytes = b"published image bytes";
        let image_hash = blake3::hash(image_bytes).as_bytes().to_vec();
        tokio::fs::write(staging.join("a/1.jpg"), image_bytes)
            .await
            .unwrap();
        let manifest_bytes = br#"{"schema_version":"test"}"#;
        let manifest_hash = blake3::hash(manifest_bytes).as_bytes().to_vec();
        tokio::fs::write(
            staging.join(".imagedb/.imagedb-manifest.json"),
            manifest_bytes,
        )
        .await
        .unwrap();

        let mut image = plan_image("a/1.jpg", &image_hash);
        image.expected_file_size = image_bytes.len() as i64;
        let tx = Uuid::new_v4();

        publish_verified_staging(
            CommitPublishStrategy::ConservativeMounted,
            tmp.path(),
            &staging,
            &publish,
            tx,
            &[1; 32],
            &manifest_hash,
            "album_a",
            &[image.clone()],
        )
        .await
        .unwrap();

        assert!(publish.join("a/1.jpg").exists());
        assert!(publish.join(".imagedb/.imagedb-manifest.json").exists());
        assert!(publish
            .join(".imagedb")
            .join(COMMIT_MARKER_FILE_NAME)
            .exists());
        assert!(!staging.exists(), "conservative staging should be cleaned");

        let marker = read_commit_marker(&publish).unwrap();
        validate_commit_marker(&marker, tx, &[1; 32], &manifest_hash, "album_a", &[image]).unwrap();
    }

    #[test]
    fn missing_commit_marker_is_not_valid_publish_evidence() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".imagedb")).unwrap();

        let err = read_commit_marker(tmp.path()).unwrap_err().to_string();
        assert!(
            err.contains("commit marker"),
            "missing marker must be surfaced as invalid publish evidence: {err}"
        );
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
            source_file_mode: SourceFileMode::CopyAndArchive,
            albums: vec![(album, vec![img])],
        };
        let h1 = compute_plan_hash(&frozen).unwrap();
        let h2 = compute_plan_hash(&frozen).unwrap();
        assert_eq!(h1, h2, "plan hash must be deterministic");
    }

    #[test]
    fn source_file_mode_is_bound_into_frozen_plan_hash() {
        let album_id = Uuid::new_v4();
        let album = PlanAlbumRow {
            plan_album_id: Uuid::new_v4(),
            import_album_id: album_id,
            target_relative_path: "album".to_string(),
            expected_image_count: 1,
            album_plan_hash: None,
        };
        let mut frozen = FrozenPlanRow {
            plan_id: Uuid::new_v4(),
            import_run_id: Uuid::new_v4(),
            library_root_id: Uuid::new_v4(),
            plan_state: "frozen".to_string(),
            plan_hash: None,
            policy_version: "2.0".to_string(),
            source_file_mode: SourceFileMode::CopyAndArchive,
            albums: vec![(album, vec![plan_image("image.jpg", &[1; 32])])],
        };
        let copy_hash = compute_plan_hash(&frozen).unwrap();
        frozen.source_file_mode = SourceFileMode::MoveSelectedWithoutBackup;
        let move_hash = compute_plan_hash(&frozen).unwrap();
        assert_ne!(copy_hash, move_hash);
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
            // Fail-fast: a real-DB test that skips when the runtime is missing
            // reports green without exercising the real commit pipeline.
            // See the M6.5-M9 closure plan: real tests must fail, not skip.
            panic!(
                "IMAGEDB_POSTGRES_BIN is not set; cannot run the real commit integration test. \
                 Set IMAGEDB_POSTGRES_BIN to a PostgreSQL 18.x bin directory, or run \
                 `node scripts/package-postgres-runtime.mjs` to populate the packaged runtime \
                 at .local/db-tools/postgresql-18.4/pgsql/bin."
            );
        }

        let tmp = TempDir::new().unwrap();
        let app_data = tmp.path().join("app_data");
        let source_root = tmp.path().join("source");
        let library_root = tmp.path().join("library");
        let album_path = source_root.join("album_a");
        std::fs::create_dir_all(&album_path).unwrap();
        std::fs::create_dir_all(&library_root).unwrap();
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
                    pixel_hash: Some(vec![1; 32]),
                    block_hash_16: Some(vec![1; 32]),
                    double_gradient_hash_32: Some(vec![1; 68]),
                    perceptual_eligible: true,
                    fingerprint_version: Some("2".to_string()),
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
        let transactions_before_confirm: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM file_transactions WHERE import_run_id = $1",
                &[&import_run_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(
            transactions_before_confirm, 0,
            "a Frozen plan must not create file transactions before Commit is called"
        );

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
        let (manifest, _raw_hash) = read_manifest_with_hash(&publish_dir).unwrap();
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
        let library_row = client2
            .query_one(
                "SELECT COUNT(*) AS image_count,
                        COUNT(*) FILTER (WHERE li.perceptual_eligible) AS eligible_count
                 FROM library_images li
                 JOIN library_albums la ON la.id = li.album_id
                 WHERE la.relative_path = 'album_a'",
                &[],
            )
            .await
            .unwrap();
        let transactions_after_confirm: i64 = client2
            .query_one(
                "SELECT COUNT(*) FROM file_transactions WHERE import_run_id = $1",
                &[&import_run_id],
            )
            .await
            .unwrap()
            .get(0);
        assert!(
            transactions_after_confirm > 0,
            "calling Commit must persist at least one file transaction"
        );
        let count: i64 = library_row.get("image_count");
        assert_eq!(
            count, 2,
            "exactly two library images after idempotent rerun"
        );
        assert_eq!(
            library_row.get::<_, i64>("eligible_count"),
            2,
            "perceptual eligibility must be copied into committed library rows"
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
        ImportRepository::update_import_run_state(
            client,
            import_run_id,
            &ImportRunState::ReadyToCommit,
        )
        .await?;
        Ok(())
    }

    fn plan_image_full(
        source_path: &str,
        target_rel: &str,
        size: i64,
        blake3: &[u8],
    ) -> PlanImageRow {
        PlanImageRow {
            included: true,
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

    /// Album root is the parent of both subdirectories; verify that the
    /// album root (not a subdirectory parent) is the accepted containment
    /// boundary even when chapter-1 and chapter-2 are distinct parents.
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

    // --- is_known_unsupported_dir_sync_error whitelist tests -----------------

    #[cfg(windows)]
    const DIR_SYNC_WHITELISTED_RAW_CODE: i32 = 87; // ERROR_INVALID_PARAMETER
    #[cfg(not(windows))]
    const DIR_SYNC_WHITELISTED_RAW_CODE: i32 = 22; // EINVAL
    const DIR_SYNC_NON_WHITELISTED_RAW_CODE: i32 = 500; // not on any platform list

    #[test]
    fn unsupported_kind_downgrades_to_success() {
        let err = std::io::Error::new(ErrorKind::Unsupported, "dir sync unsupported");
        assert!(is_known_unsupported_dir_sync_error(&err));
    }

    #[test]
    fn permission_denied_is_not_downgraded() {
        // PermissionDenied used to be swallowed by the old helper; after the
        // tightening it must propagate so real ACL failures surface.
        let err = std::io::Error::new(ErrorKind::PermissionDenied, "acl denied");
        assert!(!is_known_unsupported_dir_sync_error(&err));

        // Same when the OS attached a raw code (e.g. Windows ERROR_ACCESS_DENIED=5).
        let err_with_raw = std::io::Error::from_raw_os_error(5);
        assert!(!is_known_unsupported_dir_sync_error(&err_with_raw));
    }

    #[cfg(windows)]
    #[test]
    fn windows_existing_directory_access_denied_open_is_downgraded() {
        let tmp = TempDir::new().unwrap();
        let err = std::io::Error::from_raw_os_error(5);
        assert!(is_known_unsupported_dir_open_error(&err, tmp.path()));
        assert!(!is_known_unsupported_dir_open_error(
            &err,
            &tmp.path().join("missing")
        ));
    }

    #[test]
    fn other_without_raw_os_error_is_not_downgraded() {
        // Generic "Other" errors with no OS code are ambiguous and must NOT
        // be treated as a known unsupported-dir-sync condition.
        let err = std::io::Error::other("some opaque io failure");
        assert_eq!(err.kind(), ErrorKind::Other);
        assert!(err.raw_os_error().is_none());
        assert!(!is_known_unsupported_dir_sync_error(&err));
    }

    #[test]
    fn other_with_whitelisted_raw_os_error_downgrades() {
        let err = std::io::Error::from_raw_os_error(DIR_SYNC_WHITELISTED_RAW_CODE);
        assert!(is_known_unsupported_dir_sync_error(&err));
    }

    #[test]
    fn other_with_non_whitelisted_raw_os_error_is_not_downgraded() {
        let err = std::io::Error::from_raw_os_error(DIR_SYNC_NON_WHITELISTED_RAW_CODE);
        // 500 is not on any whitelist; regardless of the kind stdlib maps it
        // to, the helper must refuse to downgrade.
        assert!(!is_known_unsupported_dir_sync_error(&err));
    }

    #[test]
    fn non_whitelisted_kinds_are_not_downgraded() {
        // Regression: the old helper used to accept any ErrorKind::Other,
        // which included every error the OS couldn't classify. Now only
        // explicitly supported kinds / codes are accepted.
        for kind in [
            ErrorKind::NotFound,
            ErrorKind::TimedOut,
            ErrorKind::Interrupted,
            ErrorKind::WouldBlock,
            ErrorKind::BrokenPipe,
        ] {
            let err = std::io::Error::new(kind, "x");
            assert!(
                !is_known_unsupported_dir_sync_error(&err),
                "kind {kind:?} should not downgrade"
            );
        }
    }

    #[tokio::test]
    async fn sync_parent_dir_propagates_io_error_for_missing_parent() {
        // A path whose parent does not exist must surface as AppError::IoError;
        // it must NOT be silently swallowed as "unsupported dir sync".
        let bogus =
            std::path::PathBuf::from("__nonexistent_dir_imagedb_unit_test__/missing_child.bin");
        let result = sync_parent_dir(&bogus).await;
        let err = result.expect_err("expected IoError for missing parent");
        let msg = err.to_string();
        assert!(
            msg.contains("io error"),
            "expected AppError::IoError, got: {msg}"
        );
        assert!(
            msg.contains("cannot open parent directory for sync"),
            "unexpected error message: {msg}"
        );
    }
}
