//! Real Recovery Service: resumes interrupted import transactions from their
//! persisted state.
//!
//! Unlike the previous stub (which only returned label strings like
//! `"retry_staging"`), this module actually executes the recovery action for
//! each transaction state. Every action is idempotent: running it twice
//! produces no additional side effects.
//!
//! Recovery is driven by the frozen plan (the sole source of truth for what
//! files should exist, with what sizes and BLAKE3 hashes) and the persisted
//! `file_transactions` / `file_operations` rows. It never overwrites an
//! unknown published directory — a mismatch surfaces as a `conflict` with
//! full diagnostics instead of an automatic fix.
#![allow(dead_code)]
use crate::domain::state_machine::{self, FileOpState, TransactionState};
use crate::error::AppError;
use crate::infrastructure::postgres::PostgresManager;
use crate::repositories::import_repository::{
    FileTransactionFullRow, ImportRepository, PlanImageRow,
};
use crate::services::commit_service::{
    build_manifest, commit_library_records_transaction, normalize_relative_path, read_manifest,
    stream_copy_with_hash, sync_parent_dir, validate_and_hash_frozen_plan, verify_dir_against_plan,
    verify_staging_set, write_synced_then_rename,
};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
use tokio_postgres::Client;
use uuid::Uuid;

/// Diagnostic information for a recoverable transaction.
#[derive(Debug, Clone, Serialize)]
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
    pub plan_id: Option<Uuid>,
    pub plan_hash: Option<String>,
    pub last_error: Option<String>,
    pub diagnostics: Vec<String>,
}

/// Outcome of a single-transaction recovery.
#[derive(Debug, Clone, Serialize)]
pub struct RecoveryOutcome {
    pub transaction_id: Uuid,
    pub final_state: String,
    pub recovered: bool,
    pub message: String,
}

/// Scan all non-terminal transactions and generate recovery diagnostics.
pub async fn scan_recoverable_transactions(
    client: &Client,
) -> Result<Vec<RecoveryDiagnostic>, AppError> {
    let rows = ImportRepository::get_recoverable_transactions(client).await?;
    let mut diagnostics = Vec::new();
    for tx in rows {
        let staging_exists = tx
            .staging_path
            .as_ref()
            .map(|p| Path::new(p).exists())
            .unwrap_or(false);
        let target_exists = tx
            .target_path
            .as_ref()
            .map(|p| Path::new(p).exists())
            .unwrap_or(false);
        let manifest_exists = tx
            .manifest_path
            .as_ref()
            .map(|p| Path::new(p).exists())
            .unwrap_or(false);

        let mut diags = Vec::new();
        match tx.state.as_str() {
            "planned" | "staging" => {
                diags.push("staging incomplete: clean .part files, resume copy".to_string());
            }
            "verifying" | "verified" => {
                diags.push("staging complete: re-verify and publish".to_string());
            }
            "publishing" => {
                if staging_exists && !target_exists {
                    diags.push("staging ready, target missing: retry rename".to_string());
                } else if target_exists {
                    diags.push("target exists: verify manifest then mark published".to_string());
                } else {
                    diags.push("staging missing: re-stage from source".to_string());
                }
            }
            "published" | "db_committing" => {
                diags.push("published: retry database commit".to_string());
            }
            "library_committed" | "source_archiving" => {
                diags.push("library committed: resume source archive".to_string());
            }
            "cleanup_required" => {
                diags.push("cleanup required: staging dir left behind".to_string());
            }
            "conflict" => {
                diags.push("conflict: manual resolution required".to_string());
            }
            other => {
                diags.push(format!("unhandled state: {other}"));
            }
        }

        diagnostics.push(RecoveryDiagnostic {
            transaction_id: tx.id,
            import_run_id: tx.import_run_id,
            import_album_id: tx.import_album_id,
            current_state: tx.state.clone(),
            staging_path: tx.staging_path.clone(),
            target_path: tx.target_path.clone(),
            manifest_path: tx.manifest_path.clone(),
            staging_exists,
            target_exists,
            manifest_exists,
            plan_id: None,
            plan_hash: tx.plan_hash.as_ref().map(|b| hex(b)),
            last_error: tx.last_error.clone(),
            diagnostics: diags,
        });
    }
    Ok(diagnostics)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Attempt to recover a single transaction based on its current state.
///
/// This is the real recovery driver. It loads the frozen plan, the persisted
/// transaction + operations, and the on-disk state, then executes the
/// appropriate resume action. Idempotent: running twice is a no-op the second
/// time.
pub async fn recover_transaction(
    postgres_manager: Arc<Mutex<PostgresManager>>,
    transaction_id: Uuid,
) -> Result<RecoveryOutcome, AppError> {
    let (mut client, handle) = {
        let mgr = postgres_manager.lock().await;
        mgr.connect()
            .await
            .map_err(|e| AppError::Internal(format!("failed to connect for recovery: {e}")))?
    };

    let result = recover_transaction_with_client(&mut client, transaction_id).await;

    drop(client);
    handle.abort();
    result
}

async fn recover_transaction_with_client(
    client: &mut Client,
    transaction_id: Uuid,
) -> Result<RecoveryOutcome, AppError> {
    let tx = ImportRepository::get_file_transaction(client, transaction_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("transaction {transaction_id} not found")))?;

    let current = match TransactionState::parse(&tx.state) {
        Ok(s) => s,
        Err(_) => {
            let state = tx.state.clone();
            return Ok(RecoveryOutcome {
                transaction_id,
                final_state: state.clone(),
                recovered: false,
                message: format!("unparseable transaction state '{state}'"),
            });
        }
    };

    if current.is_terminal() {
        return Ok(RecoveryOutcome {
            transaction_id,
            final_state: current.to_string(),
            recovered: true,
            message: "transaction already terminal".to_string(),
        });
    }

    if current == TransactionState::Conflict {
        return Ok(RecoveryOutcome {
            transaction_id,
            final_state: current.to_string(),
            recovered: false,
            message: "conflict requires manual resolution".to_string(),
        });
    }

    // Load the frozen plan to know what files should exist.
    let frozen = ImportRepository::load_frozen_plan(client, tx.import_run_id)
        .await?
        .ok_or_else(|| {
            AppError::Internal(format!(
                "no frozen plan for run {} of transaction {transaction_id}",
                tx.import_run_id
            ))
        })?;

    // Find this transaction's album in the plan.
    let (plan_album, plan_images) = frozen
        .albums
        .iter()
        .find(|(a, _)| a.import_album_id == tx.import_album_id)
        .ok_or_else(|| {
            AppError::Internal(format!(
                "transaction {} album {} not found in frozen plan",
                transaction_id, tx.import_album_id
            ))
        })?
        .clone();

    let album_relative_path = normalize_relative_path(&plan_album.target_relative_path)?;
    let library_root_id = frozen.library_root_id;
    let library_root_path =
        ImportRepository::get_library_root_path(client, library_root_id).await?;
    let library_root = PathBuf::from(&library_root_path);
    let validated_plan_hash = validate_and_hash_frozen_plan(&frozen, library_root_id)?;

    // Dispatch by state.
    let outcome = match current {
        TransactionState::Planned | TransactionState::Staging => {
            resume_staging(
                client,
                &tx,
                &frozen.plan_id,
                &validated_plan_hash,
                &plan_images,
            )
            .await?
        }
        TransactionState::Verifying | TransactionState::Verified => {
            resume_verify_and_publish(
                client,
                &tx,
                &frozen.plan_id,
                &validated_plan_hash,
                &library_root,
                library_root_id,
                tx.import_run_id,
                &album_relative_path,
                plan_album,
                &plan_images,
            )
            .await?
        }
        TransactionState::Publishing => {
            resume_publishing(
                client,
                &tx,
                &frozen.plan_id,
                &validated_plan_hash,
                &library_root,
                library_root_id,
                tx.import_run_id,
                &album_relative_path,
                plan_album,
                &plan_images,
            )
            .await?
        }
        TransactionState::Published | TransactionState::DbCommitting => {
            resume_db_commit(
                client,
                &tx,
                &frozen.plan_id,
                &validated_plan_hash,
                &library_root,
                library_root_id,
                tx.import_run_id,
                &album_relative_path,
                plan_album,
                &plan_images,
            )
            .await?
        }
        TransactionState::LibraryCommitted | TransactionState::SourceArchiving => {
            resume_source_archive(
                client,
                &tx,
                &library_root,
                &album_relative_path,
                &plan_images,
            )
            .await?
        }
        TransactionState::CleanupRequired => resume_cleanup(client, &tx, &library_root).await?,
        _ => {
            return Ok(RecoveryOutcome {
                transaction_id,
                final_state: current.to_string(),
                recovered: false,
                message: format!("no recovery action for state {}", current),
            });
        }
    };

    Ok(RecoveryOutcome {
        transaction_id,
        final_state: outcome.0,
        recovered: outcome.1,
        message: outcome.2,
    })
}

/// planned/staging: clean .part files, verify reusable staged files, resume
/// copying the rest, then continue through verify/publish/commit.
async fn resume_staging(
    client: &mut Client,
    tx: &FileTransactionFullRow,
    _plan_id: &Uuid,
    _plan_hash: &[u8],
    plan_images: &[PlanImageRow],
) -> Result<(String, bool, String), AppError> {
    let staging_dir = tx
        .staging_path
        .as_ref()
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Internal("transaction has no staging_path".to_string()))?;
    let ops = ImportRepository::get_file_operations(client, tx.id).await?;

    ImportRepository::update_file_transaction_state(
        client,
        tx.id,
        &TransactionState::Staging,
        None,
    )
    .await?;

    if !staging_dir.exists() {
        tokio::fs::create_dir_all(&staging_dir)
            .await
            .map_err(|e| AppError::IoError(format!("cannot recreate staging dir: {e}")))?;
    }

    for img in plan_images {
        let target_rel = normalize_relative_path(&img.target_relative_path)?;
        let staged = staging_dir.join(&target_rel);
        let part = staging_dir.join(format!("{target_rel}.part"));

        if let Some(parent) = staged.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| AppError::IoError(format!("cannot create staging subdir: {e}")))?;
        }

        // Remove any leftover .part from an interrupted copy.
        let _ = tokio::fs::remove_file(&part).await;

        // If the staged file already exists and verifies, reuse it.
        if let Ok(meta) = tokio::fs::metadata(&staged).await {
            if meta.len() == img.expected_file_size as u64 {
                if let Ok(actual) = hash_file(&staged).await {
                    if actual == img.expected_blake3 {
                        // Already verified — mark the op verified if not already.
                        if let Some(op) = ops.iter().find(|o| o.target_path.ends_with(&target_rel))
                        {
                            if FileOpState::parse(&op.state).ok() != Some(FileOpState::Verified) {
                                ImportRepository::update_file_operation_state(
                                    client,
                                    op.id,
                                    &FileOpState::Verified,
                                    Some(&actual),
                                    None,
                                )
                                .await?;
                            }
                        }
                        continue;
                    }
                }
            }
            // Size matches but hash doesn't, or size wrong — re-copy.
            let _ = tokio::fs::remove_file(&staged).await;
        }

        let src = Path::new(&img.source_path);
        if !src.exists() {
            let msg = format!("source file missing during recovery: {}", src.display());
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Failed,
                Some(&msg),
            )
            .await?;
            return Ok((TransactionState::Failed.to_string(), false, msg));
        }

        let op_id = ops
            .iter()
            .find(|o| o.target_path.ends_with(&target_rel))
            .map(|o| o.id);
        if let Some(op_id) = op_id {
            ImportRepository::update_file_operation_state(
                client,
                op_id,
                &FileOpState::Copying,
                None,
                None,
            )
            .await?;
        }
        let actual = stream_copy_with_hash(src, &part).await?;
        if actual != img.expected_blake3 {
            let _ = tokio::fs::remove_file(&part).await;
            let msg = format!("BLAKE3 mismatch recovering {}", src.display());
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Failed,
                Some(&msg),
            )
            .await?;
            return Ok((TransactionState::Failed.to_string(), false, msg));
        }
        tokio::fs::rename(&part, &staged)
            .await
            .map_err(|e| AppError::IoError(format!("rename part failed: {e}")))?;
        if let Some(op_id) = op_id {
            ImportRepository::update_file_operation_state(
                client,
                op_id,
                &FileOpState::Verified,
                Some(&actual),
                None,
            )
            .await?;
        }
    }

    // Staging is now complete; continue through verify + publish + commit.
    ImportRepository::update_file_transaction_state(
        client,
        tx.id,
        &TransactionState::Verifying,
        None,
    )
    .await?;
    verify_staging_set(&staging_dir, plan_images).await?;
    ImportRepository::update_file_transaction_state(
        client,
        tx.id,
        &TransactionState::Verified,
        None,
    )
    .await?;

    Ok((
        TransactionState::Verified.to_string(),
        true,
        "staging resumed and verified; call recovery again to publish".to_string(),
    ))
}

pub(crate) async fn hash_file(path: &Path) -> Result<Vec<u8>, AppError> {
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
async fn resume_verify_and_publish(
    client: &mut Client,
    tx: &FileTransactionFullRow,
    plan_id: &Uuid,
    plan_hash: &[u8],
    library_root: &Path,
    library_root_id: Uuid,
    import_run_id: Uuid,
    album_relative_path: &str,
    plan_album: crate::repositories::import_repository::PlanAlbumRow,
    plan_images: &[PlanImageRow],
) -> Result<(String, bool, String), AppError> {
    let staging_dir = tx
        .staging_path
        .as_ref()
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Internal("transaction has no staging_path".to_string()))?;

    // Re-verify the staging set.
    verify_staging_set(&staging_dir, plan_images).await?;
    ImportRepository::update_file_transaction_state(
        client,
        tx.id,
        &TransactionState::Verified,
        None,
    )
    .await?;

    // Write the manifest if not present, then publish.
    publish_from_staging(
        client,
        tx,
        plan_id,
        plan_hash,
        library_root,
        library_root_id,
        import_run_id,
        album_relative_path,
        plan_album,
        plan_images,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn resume_publishing(
    client: &mut Client,
    tx: &FileTransactionFullRow,
    plan_id: &Uuid,
    plan_hash: &[u8],
    library_root: &Path,
    library_root_id: Uuid,
    import_run_id: Uuid,
    album_relative_path: &str,
    plan_album: crate::repositories::import_repository::PlanAlbumRow,
    plan_images: &[PlanImageRow],
) -> Result<(String, bool, String), AppError> {
    let staging_dir = tx.staging_path.as_ref().map(PathBuf::from);
    let publish_dir = library_root.join("Albums").join(album_relative_path);

    if let Some(staging) = &staging_dir {
        if staging.exists() && !publish_dir.exists() {
            // Retry the atomic rename.
            if let Some(parent) = publish_dir.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| AppError::IoError(format!("cannot create publish parent: {e}")))?;
            }
            tokio::fs::rename(staging, &publish_dir)
                .await
                .map_err(|e| AppError::IoError(format!("atomic publish rename failed: {e}")))?;
            sync_parent_dir(&publish_dir).await?;
            // Manifest moved with the rename: record the published path.
            let published_manifest = publish_dir.join(".imagedb").join(".imagedb-manifest.json");
            if !published_manifest.exists() {
                return Err(AppError::Internal(format!(
                    "published manifest missing after rename: {}",
                    published_manifest.display()
                )));
            }
            ImportRepository::set_transaction_manifest_path(
                client,
                tx.id,
                &published_manifest.display().to_string(),
            )
            .await?;
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Published,
                None,
            )
            .await?;
            return resume_db_commit(
                client,
                tx,
                plan_id,
                plan_hash,
                library_root,
                library_root_id,
                import_run_id,
                album_relative_path,
                plan_album,
                plan_images,
            )
            .await;
        }
    }

    // Target exists: verify manifest matches → published; else conflict.
    if publish_dir.exists() {
        match read_manifest(&publish_dir) {
            Ok(manifest)
                if manifest.transaction_id == tx.id.to_string()
                    && manifest.plan_id == plan_id.to_string() =>
            {
                ImportRepository::update_file_transaction_state(
                    client,
                    tx.id,
                    &TransactionState::Published,
                    None,
                )
                .await?;
                return resume_db_commit(
                    client,
                    tx,
                    plan_id,
                    plan_hash,
                    library_root,
                    library_root_id,
                    import_run_id,
                    album_relative_path,
                    plan_album,
                    plan_images,
                )
                .await;
            }
            _ => {
                let msg = format!(
                    "conflict: target {} exists with mismatched/missing manifest",
                    publish_dir.display()
                );
                ImportRepository::update_file_transaction_state(
                    client,
                    tx.id,
                    &TransactionState::Conflict,
                    Some(&msg),
                )
                .await?;
                return Ok((TransactionState::Conflict.to_string(), false, msg));
            }
        }
    }

    // Neither staging nor target — must re-stage from source.
    let msg = "staging missing during publishing; re-stage required".to_string();
    ImportRepository::update_file_transaction_state(
        client,
        tx.id,
        &TransactionState::Staging,
        Some(&msg),
    )
    .await?;
    Ok((TransactionState::Staging.to_string(), false, msg))
}

#[allow(clippy::too_many_arguments)]
async fn publish_from_staging(
    client: &mut Client,
    tx: &FileTransactionFullRow,
    plan_id: &Uuid,
    plan_hash: &[u8],
    library_root: &Path,
    library_root_id: Uuid,
    import_run_id: Uuid,
    album_relative_path: &str,
    plan_album: crate::repositories::import_repository::PlanAlbumRow,
    plan_images: &[PlanImageRow],
) -> Result<(String, bool, String), AppError> {
    let staging_dir = tx
        .staging_path
        .as_ref()
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Internal("transaction has no staging_path".to_string()))?;

    // Write the manifest into staging (temp + atomic rename).
    let manifest = build_manifest(
        &tx.id,
        *plan_id,
        plan_hash,
        import_run_id,
        tx.import_album_id,
        library_root_id,
        album_relative_path,
        plan_images,
    );
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| AppError::Internal(format!("manifest serialize failed: {e}")))?;
    let manifest_hash = blake3::hash(manifest_json.as_bytes()).as_bytes().to_vec();
    let manifest_dir = staging_dir.join(".imagedb");
    tokio::fs::create_dir_all(&manifest_dir)
        .await
        .map_err(|e| AppError::IoError(format!("cannot create manifest dir: {e}")))?;
    let tmp = manifest_dir.join(".imagedb-manifest.json.tmp");
    let final_m = manifest_dir.join(".imagedb-manifest.json");
    write_synced_then_rename(&tmp, &final_m, manifest_json.as_bytes()).await?;
    ImportRepository::set_transaction_hashes(client, tx.id, None, Some(&manifest_hash)).await?;

    // Atomic publish.
    let publishing = state_machine::transition_transaction(TransactionState::Verified, "publish")?;
    ImportRepository::update_file_transaction_state(client, tx.id, &publishing, None).await?;

    let publish_dir = library_root.join("Albums").join(album_relative_path);
    if publish_dir.exists() {
        let msg = format!(
            "target already exists during publish: {}",
            publish_dir.display()
        );
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    }
    if let Some(parent) = publish_dir.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| AppError::IoError(format!("cannot create publish parent: {e}")))?;
    }
    tokio::fs::rename(&staging_dir, &publish_dir)
        .await
        .map_err(|e| AppError::IoError(format!("atomic publish rename failed: {e}")))?;
    sync_parent_dir(&publish_dir).await?;
    // Manifest moved with the rename: record the published path.
    let published_manifest = publish_dir.join(".imagedb").join(".imagedb-manifest.json");
    if !published_manifest.exists() {
        return Err(AppError::Internal(format!(
            "published manifest missing after rename: {}",
            published_manifest.display()
        )));
    }
    ImportRepository::set_transaction_manifest_path(
        client,
        tx.id,
        &published_manifest.display().to_string(),
    )
    .await?;
    let published =
        state_machine::transition_transaction(TransactionState::Publishing, "published")?;
    ImportRepository::update_file_transaction_state(client, tx.id, &published, None).await?;

    // Continue to DB commit.
    resume_db_commit(
        client,
        tx,
        plan_id,
        plan_hash,
        library_root,
        library_root_id,
        import_run_id,
        album_relative_path,
        plan_album,
        plan_images,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn resume_db_commit(
    client: &mut Client,
    tx: &FileTransactionFullRow,
    plan_id: &Uuid,
    plan_hash: &[u8],
    library_root: &Path,
    library_root_id: Uuid,
    _import_run_id: Uuid,
    album_relative_path: &str,
    plan_album: crate::repositories::import_repository::PlanAlbumRow,
    plan_images: &[PlanImageRow],
) -> Result<(String, bool, String), AppError> {
    let publish_dir = library_root.join("Albums").join(album_relative_path);

    // Verify the published dir + manifest before touching the DB.
    if !publish_dir.exists() {
        let msg = format!("published dir missing: {}", publish_dir.display());
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    }
    let manifest = match read_manifest(&publish_dir) {
        Ok(m) if m.transaction_id == tx.id.to_string() => m,
        _ => {
            let msg = "published manifest missing or transaction id mismatch".to_string();
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            return Ok((TransactionState::Conflict.to_string(), false, msg));
        }
    };
    let published_manifest = publish_dir.join(".imagedb").join(".imagedb-manifest.json");
    if !published_manifest.exists() {
        let msg = format!(
            "published manifest missing: {}",
            published_manifest.display()
        );
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    }
    ImportRepository::set_transaction_manifest_path(
        client,
        tx.id,
        &published_manifest.display().to_string(),
    )
    .await?;
    let manifest_hash = blake3::hash(
        serde_json::to_string_pretty(&manifest)
            .unwrap_or_default()
            .as_bytes(),
    )
    .as_bytes()
    .to_vec();

    // Verify every published file still matches (content may have changed).
    for img in plan_images {
        let target_rel = normalize_relative_path(&img.target_relative_path)?;
        let file_path = publish_dir.join(&target_rel);
        let meta = match tokio::fs::metadata(&file_path).await {
            Ok(m) => m,
            Err(_) => {
                let msg = format!("published file missing: {}", file_path.display());
                ImportRepository::update_file_transaction_state(
                    client,
                    tx.id,
                    &TransactionState::Conflict,
                    Some(&msg),
                )
                .await?;
                return Ok((TransactionState::Conflict.to_string(), false, msg));
            }
        };
        if meta.len() != img.expected_file_size as u64 {
            let msg = format!("published file size mismatch: {}", file_path.display());
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            return Ok((TransactionState::Conflict.to_string(), false, msg));
        }
        let actual = hash_file(&file_path).await?;
        if actual != img.expected_blake3 {
            let msg = format!("published file blake3 mismatch: {}", file_path.display());
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            return Ok((TransactionState::Conflict.to_string(), false, msg));
        }
    }

    // Transition to db_committing and run the DB transaction.
    let db_committing =
        state_machine::transition_transaction(TransactionState::Published, "db_commit")?;
    ImportRepository::update_file_transaction_state(client, tx.id, &db_committing, None).await?;

    commit_library_records_transaction(
        client,
        library_root_id,
        tx.id,
        *plan_id,
        plan_hash,
        &manifest_hash,
        &plan_album,
        album_relative_path,
        &manifest,
        plan_images,
    )
    .await?;

    let library_committed =
        state_machine::transition_transaction(TransactionState::DbCommitting, "library_committed")?;
    ImportRepository::update_file_transaction_state(client, tx.id, &library_committed, None)
        .await?;

    // Continue to source archive.
    resume_source_archive(client, tx, library_root, album_relative_path, plan_images).await
}

/// library_committed/source_archiving: validate source and archive against
/// the frozen plan, then safely rename source → archive. Never auto-delete
/// the source album directory.
///
/// | source | archive | outcome                                                  |
/// |--------|---------|----------------------------------------------------------|
/// | ✓      | ✗       | verify source vs plan → rename → source_archived         |
/// | ✗      | ✓       | verify archive vs plan → source_archived if match        |
/// | ✗      | ✗       | conflict                                                 |
/// | ✓      | ✓       | conflict (no delete, no overwrite)                       |
async fn resume_source_archive(
    client: &mut Client,
    tx: &FileTransactionFullRow,
    library_root: &Path,
    album_relative_path: &str,
    plan_images: &[PlanImageRow],
) -> Result<(String, bool, String), AppError> {
    // The library commit is already successful. Do NOT re-copy or re-publish.
    let publish_dir = library_root.join("Albums").join(album_relative_path);
    if !publish_dir.exists() {
        // Without the published dir we cannot trust the library state; this is
        // unexpected post-library_committed. Surface as conflict.
        let msg = format!(
            "published dir missing during archive: {}",
            publish_dir.display()
        );
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    }

    // Idempotent: already archived.
    if matches!(
        TransactionState::parse(&tx.state),
        Ok(TransactionState::SourceArchived)
    ) {
        return Ok((
            TransactionState::SourceArchived.to_string(),
            true,
            "already archived".to_string(),
        ));
    }

    // Derive the source album dir from the plan images. We cannot infer a
    // reliable source location when plan_images is empty or the images span
    // multiple parents, so surface those as conflicts rather than guessing.
    let source_album_dir = match derive_source_album_dir(plan_images)? {
        Some(d) => d,
        None => {
            let msg = "cannot derive source album directory from frozen plan".to_string();
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            return Ok((TransactionState::Conflict.to_string(), false, msg));
        }
    };

    let archive_base = source_album_dir
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".imagedb-processed");
    let archive_dir = archive_base
        .join(tx.id.to_string())
        .join(album_relative_path);

    let source_exists = source_album_dir.exists();
    let archive_exists = archive_dir.exists();

    let archiving =
        state_machine::transition_transaction(TransactionState::LibraryCommitted, "archive")?;
    ImportRepository::update_file_transaction_state(client, tx.id, &archiving, None).await?;

    match (source_exists, archive_exists) {
        (true, false) => {
            // Source present, archive missing: verify source contents against
            // the frozen plan, then same-filesystem rename to archive.
            if let Err(e) =
                verify_dir_against_plan(&source_album_dir, album_relative_path, plan_images).await
            {
                let msg = format!(
                    "source album {} does not match frozen plan: {e}",
                    source_album_dir.display()
                );
                ImportRepository::update_file_transaction_state(
                    client,
                    tx.id,
                    &TransactionState::Conflict,
                    Some(&msg),
                )
                .await?;
                return Ok((TransactionState::Conflict.to_string(), false, msg));
            }
            tokio::fs::create_dir_all(archive_dir.parent().unwrap())
                .await
                .map_err(|e| AppError::IoError(format!("cannot create archive base: {e}")))?;
            tokio::fs::rename(&source_album_dir, &archive_dir)
                .await
                .map_err(|e| AppError::IoError(format!("source archive rename failed: {e}")))?;
            sync_parent_dir(&archive_dir).await?;
            let archived = state_machine::transition_transaction(
                TransactionState::SourceArchiving,
                "archived",
            )?;
            ImportRepository::update_file_transaction_state(client, tx.id, &archived, None).await?;
            Ok((
                TransactionState::SourceArchived.to_string(),
                true,
                "source archived".to_string(),
            ))
        }
        (false, true) => {
            // Source missing, archive present: only trust the archive if its
            // contents exactly match the frozen plan; otherwise conflict.
            if let Err(e) =
                verify_dir_against_plan(&archive_dir, album_relative_path, plan_images).await
            {
                let msg = format!(
                    "archive {} does not match frozen plan: {e}",
                    archive_dir.display()
                );
                ImportRepository::update_file_transaction_state(
                    client,
                    tx.id,
                    &TransactionState::Conflict,
                    Some(&msg),
                )
                .await?;
                return Ok((TransactionState::Conflict.to_string(), false, msg));
            }
            let archived = state_machine::transition_transaction(
                TransactionState::SourceArchiving,
                "archived",
            )?;
            ImportRepository::update_file_transaction_state(client, tx.id, &archived, None).await?;
            Ok((
                TransactionState::SourceArchived.to_string(),
                true,
                "archive verified against plan; source already archived".to_string(),
            ))
        }
        (false, false) => {
            // Neither exists: cannot confirm the source was preserved. Conflict.
            let msg = format!(
                "source {} and archive {} both missing; cannot confirm archive integrity",
                source_album_dir.display(),
                archive_dir.display()
            );
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            Ok((TransactionState::Conflict.to_string(), false, msg))
        }
        (true, true) => {
            // Both exist: ambiguous state — do NOT delete or overwrite either.
            let msg = format!(
                "source {} and archive {} both present; refusing to overwrite or delete",
                source_album_dir.display(),
                archive_dir.display()
            );
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            Ok((TransactionState::Conflict.to_string(), false, msg))
        }
    }
}

/// Derive the source album directory from the frozen plan images. Returns
/// None if plan_images is empty or images span multiple parent directories.
fn derive_source_album_dir(plan_images: &[PlanImageRow]) -> Result<Option<PathBuf>, AppError> {
    let Some(first) = plan_images.first() else {
        return Ok(None);
    };
    let Some(first_parent) = Path::new(&first.source_path).parent().map(PathBuf::from) else {
        return Ok(None);
    };
    for img in &plan_images[1..] {
        let parent = Path::new(&img.source_path).parent().map(PathBuf::from);
        if parent.as_ref() != Some(&first_parent) {
            return Err(AppError::Internal(format!(
                "plan images span multiple source parents ({} and {:?}); refusing to archive",
                first_parent.display(),
                parent
            )));
        }
    }
    Ok(Some(first_parent))
}

/// cleanup_required: remove only this transaction's staging dir. Failures
/// preserve the state + error.
async fn resume_cleanup(
    client: &mut Client,
    tx: &FileTransactionFullRow,
    _library_root: &Path,
) -> Result<(String, bool, String), AppError> {
    if let Some(staging) = &tx.staging_path {
        let staging_base = Path::new(staging)
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(staging));
        // Only remove the transaction's staging root
        // (`.imagedb/staging/<tx_id>`), never anything broader.
        if staging_base.exists() {
            if let Err(e) = tokio::fs::remove_dir_all(&staging_base).await {
                let msg = format!("cleanup failed: {e}");
                ImportRepository::update_file_transaction_state(
                    client,
                    tx.id,
                    &TransactionState::CleanupRequired,
                    Some(&msg),
                )
                .await?;
                return Ok((TransactionState::CleanupRequired.to_string(), false, msg));
            }
        }
    }
    let archived =
        state_machine::transition_transaction(TransactionState::CleanupRequired, "cleaned")?;
    ImportRepository::update_file_transaction_state(client, tx.id, &archived, None).await?;
    Ok((
        TransactionState::SourceArchived.to_string(),
        true,
        "cleanup complete".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(source_path: &str, target_rel: &str) -> PlanImageRow {
        PlanImageRow {
            id: Uuid::new_v4(),
            plan_album_id: Uuid::new_v4(),
            import_image_id: Uuid::new_v4(),
            source_path: source_path.to_string(),
            source_relative_path: target_rel.to_string(),
            target_relative_path: target_rel.to_string(),
            expected_file_size: 1,
            expected_blake3: vec![0; 32],
            width: None,
            height: None,
            format: None,
        }
    }

    #[test]
    fn derive_source_album_dir_empty_returns_none() {
        let out = derive_source_album_dir(&[]).unwrap();
        assert!(out.is_none());
    }

    #[test]
    fn derive_source_album_dir_single_parent() {
        let imgs = vec![img("/src/album/photo1.png", "photo1.png")];
        let out = derive_source_album_dir(&imgs).unwrap();
        assert_eq!(out.as_deref(), Some(Path::new("/src/album")));
    }

    #[test]
    fn derive_source_album_dir_multiple_same_parent() {
        let imgs = vec![
            img("/src/album/photo1.png", "photo1.png"),
            img("/src/album/photo2.png", "photo2.png"),
        ];
        let out = derive_source_album_dir(&imgs).unwrap();
        assert_eq!(out.as_deref(), Some(Path::new("/src/album")));
    }

    #[test]
    fn derive_source_album_dir_conflicting_parents_rejected() {
        let imgs = vec![
            img("/src/album_a/photo1.png", "photo1.png"),
            img("/src/album_b/photo2.png", "photo2.png"),
        ];
        let err = derive_source_album_dir(&imgs).unwrap_err();
        assert!(err.to_string().contains("multiple source parents"));
    }
}
