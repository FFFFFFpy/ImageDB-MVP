use crate::domain::import_state::{
    CommitAlbumResult, CommitProgress, CommitResult, FrozenPlanEntry, ImportPlan, ImportRunState,
    FROZEN_PLAN_KEY,
};
use crate::error::AppError;
use crate::infrastructure::postgres::PostgresManager;
use crate::repositories::import_repository::{
    ImportAlbumFullRow, ImportImageFullRow, ImportRepository,
};
use crate::services::review_service;
#[cfg(feature = "fail-injection")]
use crate::tests::fail_injection::{maybe_fault, CommitFaultPoint};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_postgres::Client;
use uuid::Uuid;

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AlbumManifest {
    album_name: String,
    relative_path: String,
    image_count: u32,
    images: Vec<AlbumManifestImage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AlbumManifestImage {
    relative_path: String,
    file_size: i64,
    blake3: String,
    width: Option<i32>,
    height: Option<i32>,
    format: Option<String>,
}

struct AlbumCommitGroup {
    album_name: String,
    import_album_id: Uuid,
    source_path: String,
    images: Vec<CommitImageEntry>,
}

struct CommitImageEntry {
    import_image: ImportImageFullRow,
}

struct AlbumCommitInput<'a> {
    library_root: &'a Path,
    library_root_id: Uuid,
    import_run_id: Uuid,
    import_album_id: Uuid,
    source_path: &'a str,
    album_name: &'a str,
    images: &'a [CommitImageEntry],
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
            progress.state = if r.state == "completed" {
                "completed".to_string()
            } else {
                "completed_with_errors".to_string()
            };
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

    ImportRepository::update_library_root_path(client, library_root_id, library_root_path).await?;

    let plan = get_or_freeze_plan(client, import_run_id, &import_run.statistics).await?;

    if plan.kept_images.is_empty() {
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

    let image_ids: Vec<Uuid> = plan
        .kept_images
        .iter()
        .filter_map(|ki| Uuid::parse_str(&ki.image_id).ok())
        .collect();

    let import_images = ImportRepository::get_import_images_by_ids(client, &image_ids).await?;
    let image_map: HashMap<Uuid, ImportImageFullRow> =
        import_images.into_iter().map(|img| (img.id, img)).collect();

    let import_albums =
        ImportRepository::get_import_albums_with_source_for_run(client, import_run_id).await?;
    let album_name_to_info: HashMap<String, &ImportAlbumFullRow> = import_albums
        .iter()
        .map(|a| (a.source_name.clone(), a))
        .collect();

    let mut album_groups: HashMap<String, Vec<CommitImageEntry>> = HashMap::new();
    for ki in &plan.kept_images {
        if let Some(img) = image_map.get(&Uuid::parse_str(&ki.image_id).unwrap_or_default()) {
            album_groups
                .entry(ki.album_name.clone())
                .or_default()
                .push(CommitImageEntry {
                    import_image: ImportImageFullRow {
                        id: img.id,
                        source_path: img.source_path.clone(),
                        relative_path: img.relative_path.clone(),
                        file_size: img.file_size,
                        width: img.width,
                        height: img.height,
                        format: img.format.clone(),
                        blake3: img.blake3.clone(),
                        pixel_hash: img.pixel_hash.clone(),
                        gradient_hash: img.gradient_hash.clone(),
                        block_hash: img.block_hash.clone(),
                        median_hash: img.median_hash.clone(),
                        fingerprint_version: img.fingerprint_version.clone(),
                        import_album_id: img.import_album_id,
                    },
                });
        }
    }

    let mut groups: Vec<AlbumCommitGroup> = Vec::new();
    for (album_name, images) in album_groups {
        let album_info = album_name_to_info.get(&album_name).ok_or_else(|| {
            AppError::Internal(format!("album {album_name} not found in import_albums"))
        })?;
        groups.push(AlbumCommitGroup {
            album_name: album_name.clone(),
            import_album_id: album_info.id,
            source_path: album_info.source_path.clone(),
            images,
        });
    }

    groups.sort_by(|a, b| a.album_name.cmp(&b.album_name));

    let library_root = PathBuf::from(library_root_path);

    {
        let mut p = progress_tracker.lock().await;
        p.current_stage = "committing".to_string();
        p.albums_total = groups.len() as u32;
    }
    ImportRepository::update_import_run_state(client, import_run_id, &ImportRunState::Committing)
        .await?;

    let mut album_results = Vec::new();
    let mut total_committed = 0u32;
    let mut albums_committed = 0u32;
    let mut albums_skipped = 0u32;
    let mut albums_failed = 0u32;
    let mut all_errors = Vec::new();

    for group in &groups {
        if cancelled.load(Ordering::Relaxed) {
            all_errors.push("commit cancelled by user".to_string());
            break;
        }

        {
            let mut p = progress_tracker.lock().await;
            p.current_album = Some(group.album_name.clone());
            p.current_stage = "processing_album".to_string();
        }
        match commit_single_album(
            client,
            AlbumCommitInput {
                library_root: &library_root,
                library_root_id,
                import_run_id,
                import_album_id: group.import_album_id,
                source_path: &group.source_path,
                album_name: &group.album_name,
                images: &group.images,
            },
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
                album_results.push(result);
            }
            Err(e) => {
                albums_failed += 1;
                let err_msg = format!("album {}: {e}", group.album_name);
                all_errors.push(err_msg.clone());
                album_results.push(CommitAlbumResult {
                    album_name: group.album_name.clone(),
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

    let final_state = if albums_failed == 0 && all_errors.is_empty() {
        ImportRepository::update_import_run_state(
            client,
            import_run_id,
            &ImportRunState::Completed,
        )
        .await?;
        "completed".to_string()
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
        "completed_with_errors".to_string()
    };

    Ok(CommitResult {
        import_run_id: import_run_id.to_string(),
        albums_total: groups.len() as u32,
        albums_committed,
        albums_skipped,
        albums_failed,
        images_committed: total_committed,
        album_results,
        errors: all_errors,
        state: final_state,
    })
}

async fn get_or_freeze_plan(
    client: &mut Client,
    import_run_id: Uuid,
    statistics: &serde_json::Value,
) -> Result<ImportPlan, AppError> {
    if let Some(frozen) = statistics.get(FROZEN_PLAN_KEY) {
        let entries: Vec<FrozenPlanEntry> = serde_json::from_value(frozen.clone())
            .map_err(|e| AppError::Internal(format!("failed to parse frozen plan: {e}")))?;

        let all_images =
            ImportRepository::get_all_import_images_with_album(client, import_run_id).await?;
        let all_candidates =
            ImportRepository::get_all_candidates_for_import_plan(client, import_run_id).await?;
        let albums = ImportRepository::get_albums_for_run(client, import_run_id).await?;

        let mut plan = crate::services::review_service::build_import_plan(
            import_run_id.to_string(),
            &all_images,
            &all_candidates,
            &albums,
        );

        let frozen_ids: std::collections::HashSet<String> =
            entries.iter().map(|e| e.image_id.clone()).collect();
        plan.kept_images
            .retain(|ki| frozen_ids.contains(&ki.image_id));
        plan.excluded_count = plan
            .total_images
            .saturating_sub(plan.kept_images.len() as u32);

        return Ok(plan);
    }

    let plan = review_service::generate_import_plan(client, import_run_id).await?;

    let frozen_entries: Vec<FrozenPlanEntry> = plan
        .kept_images
        .iter()
        .map(|ki| FrozenPlanEntry {
            image_id: ki.image_id.clone(),
            source_path: ki.source_path.clone(),
            relative_path: ki.relative_path.clone(),
            file_size: ki.file_size,
            album_name: ki.album_name.clone(),
        })
        .collect();

    let mut stats = statistics.clone();
    stats[FROZEN_PLAN_KEY] = serde_json::to_value(&frozen_entries)
        .map_err(|e| AppError::Internal(format!("failed to serialize frozen plan: {e}")))?;
    ImportRepository::update_import_run_statistics(client, import_run_id, &stats).await?;

    Ok(plan)
}

/// Commit a single album using the staged file transaction protocol.
async fn commit_single_album(
    client: &mut Client,
    input: AlbumCommitInput<'_>,
) -> Result<CommitAlbumResult, AppError> {
    let AlbumCommitInput { library_root, library_root_id, import_run_id, import_album_id,
        source_path, album_name, images } = input;
    let album_relative_path = album_name;

    // Idempotent skip or conflict check.
    if let Some(existing) = ImportRepository::find_library_album_by_path(client, library_root_id, album_relative_path).await? {
        if decide_idempotent_skip(Some(&existing), images.len() as u32).is_some() {
            return Ok(CommitAlbumResult {
                album_name: album_name.to_string(), status: "skipped".to_string(),
                images_committed: images.len() as u32,
                target_path: Some(library_root.join("Albums").join(album_relative_path).display().to_string()),
                manifest_path: None, error: None,
            });
        }
        return Err(AppError::Internal(format!("target conflict: library album already exists at {album_relative_path}")));
    }

    let publish_dir = library_root.join("Albums").join(album_relative_path);
    if publish_dir.exists() {
        return Err(AppError::Internal(format!("target directory already exists: {}", publish_dir.display())));
    }

    // Verify source files.
    for entry in images {
        let src = Path::new(&entry.import_image.source_path);
        if !src.exists() {
            return Err(AppError::IoError(format!("source file missing: {}", src.display())));
        }
        if let Some(expected_blake3) = &entry.import_image.blake3 {
            let data = std::fs::read(src).map_err(|e| AppError::IoError(format!("cannot read source file {}: {e}", src.display())))?;
            let actual = blake3::hash(&data).as_bytes().to_vec();
            if actual != *expected_blake3 {
                return Err(AppError::Internal(format!("source snapshot mismatch for {}", src.display())));
            }
        }
    }

    // Phase 1: Create transaction + all operations before any copy
    let tx_id = Uuid::new_v4();
    let staging_base = library_root.join(".imagedb").join("staging").join(tx_id.to_string());
    let staging_dir = staging_base.join(album_relative_path);

    let tx_db_id = ImportRepository::insert_file_transaction(client, import_run_id, import_album_id,
        "planned", Some(&staging_dir.display().to_string()), Some(&publish_dir.display().to_string()), None).await?;

    let mut op_ids: Vec<(Uuid, PathBuf, Vec<u8>)> = Vec::new();
    for entry in images {
        let img = &entry.import_image;
        let target_rel = &img.relative_path;
        let staged_path = staging_dir.join(target_rel);
        let target_path = publish_dir.join(target_rel);
        let expected_blake3 = img.blake3.as_deref().unwrap_or(&[]);
        let op_id = ImportRepository::insert_file_operation(client, tx_db_id,
            &img.source_path, &staged_path.display().to_string(),
            &target_path.display().to_string(), img.file_size, expected_blake3).await?;
        op_ids.push((op_id, staged_path, expected_blake3.to_vec()));
    }
    ImportRepository::update_file_transaction_state(client, tx_db_id, "staging", None).await?;

    #[cfg(feature = "fail-injection")]
    maybe_fault(CommitFaultPoint::AfterDbWrite, "after DB write")?;

    // Phase 2: Stream copy to staging with .part files
    tokio::fs::create_dir_all(&staging_dir).await
        .map_err(|e| AppError::IoError(format!("cannot create staging dir: {e}")))?;

    for (i, entry) in images.iter().enumerate() {
        let img = &entry.import_image;
        let src = Path::new(&img.source_path);
        let target_rel = &img.relative_path;
        let staged_path = staging_dir.join(target_rel);
        let part_path = staging_dir.join(format!("{}.part", target_rel));

        if let Some(parent) = staged_path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| AppError::IoError(format!("cannot create staging subdir: {e}")))?;
        }

        let mut src_file = tokio::fs::File::open(src).await
            .map_err(|e| AppError::IoError(format!("cannot open source for staging: {e}")))?;
        let mut dst_file = tokio::fs::File::create(&part_path).await
            .map_err(|e| AppError::IoError(format!("cannot create part file: {e}")))?;

        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; 65536];
        use tokio::io::AsyncReadExt;
        use tokio::io::AsyncWriteExt;
        loop {
            let n = src_file.read(&mut buf).await
                .map_err(|e| AppError::IoError(format!("read error during staging: {e}")))?;
            if n == 0 { break; }
            hasher.update(&buf[..n]);
            dst_file.write_all(&buf[..n]).await
                .map_err(|e| AppError::IoError(format!("write error during staging: {e}")))?;
        }
        dst_file.flush().await.map_err(|e| AppError::IoError(format!("flush error: {e}")))?;
        drop(dst_file);

        let actual_blake3 = hasher.finalize().as_bytes().to_vec();
        let expected_blake3 = img.blake3.as_deref().unwrap_or(&[]);

        if !expected_blake3.is_empty() && actual_blake3 != expected_blake3 {
            let _ = tokio::fs::remove_dir_all(&staging_base).await;
            ImportRepository::update_file_transaction_state(client, tx_db_id, "failed",
                Some("BLAKE3 mismatch during staging")).await?;
            return Err(AppError::Internal(format!("BLAKE3 mismatch for staged file {}", staged_path.display())));
        }

        tokio::fs::rename(&part_path, &staged_path).await
            .map_err(|e| AppError::IoError(format!("rename part file failed: {e}")))?;

        ImportRepository::update_file_operation_state(client, op_ids[i].0, "verified",
            Some(&actual_blake3), None).await?;

        #[cfg(feature = "fail-injection")]
        maybe_fault(CommitFaultPoint::AfterStagingCopy, "after staging copy")?;
    }

    ImportRepository::update_file_transaction_state(client, tx_db_id, "verified", None).await?;

    #[cfg(feature = "fail-injection")]
    maybe_fault(CommitFaultPoint::AfterStagingVerify, "after staging verify")?;

    // Phase 3: Write manifest (temp file + atomic rename)
    let manifest = AlbumManifest {
        album_name: album_name.to_string(),
        relative_path: album_relative_path.to_string(),
        image_count: images.len() as u32,
        images: images.iter().map(|e| AlbumManifestImage {
            relative_path: e.import_image.relative_path.clone(),
            file_size: e.import_image.file_size,
            blake3: e.import_image.blake3.as_ref().map(|b| bytes_to_hex(b)).unwrap_or_default(),
            width: e.import_image.width,
            height: e.import_image.height,
            format: e.import_image.format.clone(),
        }).collect(),
    };

    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| AppError::Internal(format!("manifest serialize failed: {e}")))?;
    let manifest_hash = blake3::hash(manifest_json.as_bytes()).as_bytes().to_vec();

    let staging_manifest_dir = staging_dir.join(".imagedb");
    tokio::fs::create_dir_all(&staging_manifest_dir).await
        .map_err(|e| AppError::IoError(format!("cannot create staging manifest dir: {e}")))?;
    let staging_manifest_tmp = staging_manifest_dir.join(".imagedb-manifest.json.tmp");
    let staging_manifest_file = staging_manifest_dir.join(".imagedb-manifest.json");
    tokio::fs::write(&staging_manifest_tmp, &manifest_json).await
        .map_err(|e| AppError::IoError(format!("manifest tmp write failed: {e}")))?;
    tokio::fs::rename(&staging_manifest_tmp, &staging_manifest_file).await
        .map_err(|e| AppError::IoError(format!("manifest atomic rename failed: {e}")))?;

    #[cfg(feature = "fail-injection")]
    maybe_fault(CommitFaultPoint::AfterManifestWrite, "after manifest write")?;

    ImportRepository::update_file_transaction_state(client, tx_db_id, "publishing", None).await?;

    // Phase 4: Atomic publish (rename staging dir -> publish dir)
    #[cfg(feature = "fail-injection")]
    maybe_fault(CommitFaultPoint::BeforePublishRename, "before publish rename")?;

    tokio::fs::rename(&staging_dir, &publish_dir).await
        .map_err(|e| AppError::IoError(format!("atomic publish rename failed: {e}")))?;

    let manifests_dir = library_root.join(".imagedb").join("manifests");
    tokio::fs::create_dir_all(&manifests_dir).await
        .map_err(|e| AppError::IoError(format!("cannot create manifests dir: {e}")))?;
    let manifest_file = manifests_dir.join(format!("{album_name}.json"));
    let manifest_tmp = manifests_dir.join(format!("{album_name}.json.tmp"));
    tokio::fs::write(&manifest_tmp, &manifest_json).await
        .map_err(|e| AppError::IoError(format!("manifest write failed: {e}")))?;
    tokio::fs::rename(&manifest_tmp, &manifest_file).await
        .map_err(|e| AppError::IoError(format!("manifest atomic rename failed: {e}")))?;

    ImportRepository::update_file_transaction_state(client, tx_db_id, "published", None).await?;

    #[cfg(feature = "fail-injection")]
    maybe_fault(CommitFaultPoint::AfterPublishRename, "after publish rename")?;

    // Phase 5: DB commit (do NOT delete publish_dir on failure)
    ImportRepository::update_file_transaction_state(client, tx_db_id, "db_committing", None).await?;

    #[cfg(feature = "fail-injection")]
    maybe_fault(CommitFaultPoint::BeforeDbCommit, "before DB commit")?;

    if let Err(e) = commit_library_records_transaction(client, library_root_id, album_name,
        album_relative_path, &manifest_hash, images).await
    {
        let _ = ImportRepository::update_file_transaction_state(client, tx_db_id, "published",
            Some(&e.to_string())).await;
        return Err(e);
    }

    ImportRepository::update_file_transaction_state(client, tx_db_id, "library_committed", None).await?;

    #[cfg(feature = "fail-injection")]
    maybe_fault(CommitFaultPoint::AfterDbCommit, "after DB commit")?;

    // Phase 6: Source archive
    let source_dir = Path::new(source_path);
    if source_dir.exists() {
        #[cfg(feature = "fail-injection")]
        maybe_fault(CommitFaultPoint::BeforeSourceArchive, "before source archive")?;
        let archive_base = source_dir.parent().unwrap_or(Path::new(".")).join(".imagedb-processed");
        let archive_dir = archive_base.join(tx_id.to_string()).join(album_relative_path);

        ImportRepository::update_file_transaction_state(client, tx_db_id, "source_archiving", None).await?;

        if archive_dir.exists() {
            return Err(AppError::Internal(format!("archive target already exists: {}", archive_dir.display())));
        }

        tokio::fs::create_dir_all(archive_dir.parent().unwrap()).await
            .map_err(|e| AppError::IoError(format!("cannot create archive base dir: {e}")))?;

        tokio::fs::rename(source_dir, &archive_dir).await
            .map_err(|e| AppError::IoError(format!("source archive rename failed: {e}")))?;

        ImportRepository::update_file_transaction_state(client, tx_db_id, "source_archived", None).await?;
    }

    let _ = tokio::fs::remove_dir_all(&staging_base).await;

    Ok(CommitAlbumResult {
        album_name: album_name.to_string(), status: "committed".to_string(),
        images_committed: images.len() as u32,
        target_path: Some(publish_dir.display().to_string()),
        manifest_path: Some(manifest_file.display().to_string()),
        error: None,
    })
}

async fn commit_library_records_transaction(
    client: &mut Client,
    library_root_id: Uuid,
    album_name: &str,
    album_relative_path: &str,
    manifest_hash: &[u8],
    images: &[CommitImageEntry],
) -> Result<Uuid, AppError> {
    let transaction = client.transaction().await.map_err(|e| {
        AppError::Internal(format!("failed to begin library record transaction: {e}"))
    })?;

    let library_album_id = Uuid::new_v4();
    transaction
        .execute(
            "INSERT INTO library_albums
             (id, library_root_id, display_name, relative_path, manifest_version,
              manifest_hash, image_count, state)
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'committed')",
            &[
                &library_album_id,
                &library_root_id,
                &album_name,
                &album_relative_path,
                &"1.0",
                &manifest_hash,
                &(images.len() as i32),
            ],
        )
        .await
        .map_err(|e| AppError::Internal(format!("failed to insert library album: {e}")))?;

    for entry in images {
        let img = &entry.import_image;
        let image_id = Uuid::new_v4();
        let blake3_bytes = img
            .blake3
            .as_ref()
            .ok_or_else(|| AppError::Internal("missing blake3 for library image".to_string()))?;
        let format = img.format.as_deref().unwrap_or("unknown");
        let width = img.width.unwrap_or(0);
        let height = img.height.unwrap_or(0);
        let fp_version = img.fingerprint_version.as_deref().unwrap_or("unknown");
        let file_name = Path::new(&img.source_path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        transaction
            .execute(
                "INSERT INTO library_images
                 (id, album_id, relative_path, file_size, width, height, format,
                  blake3, pixel_hash, gradient_hash, block_hash, median_hash,
                  fingerprint_version, state)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, 'committed')",
                &[
                    &image_id,
                    &library_album_id,
                    &file_name,
                    &img.file_size,
                    &width,
                    &height,
                    &format,
                    &blake3_bytes,
                    &img.pixel_hash.as_deref(),
                    &img.gradient_hash.as_deref(),
                    &img.block_hash.as_deref(),
                    &img.median_hash.as_deref(),
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

    Ok(library_album_id)
}

fn copy_dir_recursive(source: &Path, target: &Path) -> Result<(), AppError> {
    std::fs::create_dir_all(target).map_err(|e| {
        AppError::IoError(format!(
            "cannot create archive directory {}: {e}",
            target.display()
        ))
    })?;

    for entry in std::fs::read_dir(source).map_err(|e| {
        AppError::IoError(format!(
            "cannot read source archive directory {}: {e}",
            source.display()
        ))
    })? {
        let entry =
            entry.map_err(|e| AppError::IoError(format!("cannot read archive entry: {e}")))?;
        let entry_type = entry.file_type().map_err(|e| {
            AppError::IoError(format!(
                "cannot read archive entry type {}: {e}",
                entry.path().display()
            ))
        })?;
        let target_path = target.join(entry.file_name());
        if entry_type.is_dir() {
            copy_dir_recursive(&entry.path(), &target_path)?;
        } else if entry_type.is_file() {
            std::fs::copy(entry.path(), &target_path).map_err(|e| {
                AppError::IoError(format!(
                    "cannot archive {} to {}: {e}",
                    entry.path().display(),
                    target_path.display()
                ))
            })?;
        }
    }

    Ok(())
}

pub fn check_target_conflict(
    library_root: &Path,
    album_relative_path: &str,
    has_library_record: bool,
) -> Result<(), AppError> {
    let publish_dir = library_root.join("Albums").join(album_relative_path);
    if publish_dir.exists() && !has_library_record {
        let entries: Vec<_> = std::fs::read_dir(&publish_dir)
            .map_err(|e| AppError::IoError(format!("cannot read target dir: {e}")))?
            .filter_map(|e| e.ok())
            .collect();
        if !entries.is_empty() {
            return Err(AppError::Internal(format!(
                "target conflict: directory {} exists but has no library record",
                publish_dir.display()
            )));
        }
    }
    Ok(())
}

pub fn decide_idempotent_skip(
    existing: Option<&crate::repositories::import_repository::LibraryAlbumRow>,
    expected_image_count: u32,
) -> Option<String> {
    match existing {
        Some(row) if row.state == "committed" && row.image_count as u32 == expected_image_count => {
            Some("already_committed".to_string())
        }
        Some(_) => None,
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repositories::import_repository::LibraryAlbumRow;
    use tempfile::TempDir;

    #[test]
    fn idempotent_skip_when_committed_matching_count() {
        let existing = LibraryAlbumRow {
            id: Uuid::new_v4(),
            image_count: 5,
            state: "committed".to_string(),
        };
        assert_eq!(
            decide_idempotent_skip(Some(&existing), 5),
            Some("already_committed".to_string())
        );
    }

    #[test]
    fn idempotent_no_skip_when_count_mismatch() {
        let existing = LibraryAlbumRow {
            id: Uuid::new_v4(),
            image_count: 3,
            state: "committed".to_string(),
        };
        assert_eq!(decide_idempotent_skip(Some(&existing), 5), None);
    }

    #[test]
    fn idempotent_no_skip_when_not_committed() {
        let existing = LibraryAlbumRow {
            id: Uuid::new_v4(),
            image_count: 5,
            state: "failed".to_string(),
        };
        assert_eq!(decide_idempotent_skip(Some(&existing), 5), None);
    }

    #[test]
    fn idempotent_no_skip_when_no_record() {
        assert_eq!(decide_idempotent_skip(None, 5), None);
    }

    #[test]
    fn target_conflict_detected_when_dir_exists_no_record() {
        let tmp = TempDir::new().unwrap();
        let library_root = tmp.path().join("library");
        let album_dir = library_root.join("Albums").join("test_album");
        std::fs::create_dir_all(&album_dir).unwrap();
        std::fs::write(album_dir.join("file.txt"), b"data").unwrap();

        let result = check_target_conflict(&library_root, "test_album", false);
        assert!(result.is_err());
    }

    #[test]
    fn target_no_conflict_when_dir_exists_with_record() {
        let tmp = TempDir::new().unwrap();
        let library_root = tmp.path().join("library");
        let album_dir = library_root.join("Albums").join("test_album");
        std::fs::create_dir_all(&album_dir).unwrap();
        std::fs::write(album_dir.join("file.txt"), b"data").unwrap();

        let result = check_target_conflict(&library_root, "test_album", true);
        assert!(result.is_ok());
    }

    #[test]
    fn target_no_conflict_when_dir_empty() {
        let tmp = TempDir::new().unwrap();
        let library_root = tmp.path().join("library");
        let album_dir = library_root.join("Albums").join("test_album");
        std::fs::create_dir_all(&album_dir).unwrap();

        let result = check_target_conflict(&library_root, "test_album", false);
        assert!(result.is_ok());
    }

    #[test]
    fn target_no_conflict_when_dir_missing() {
        let tmp = TempDir::new().unwrap();
        let library_root = tmp.path().join("library");

        let result = check_target_conflict(&library_root, "test_album", false);
        assert!(result.is_ok());
    }

    #[test]
    fn frozen_plan_serialization() {
        let entries = vec![
            FrozenPlanEntry {
                image_id: "id-1".to_string(),
                source_path: "/src/a.jpg".to_string(),
                relative_path: "a.jpg".to_string(),
                file_size: 100,
                album_name: "album_a".to_string(),
            },
            FrozenPlanEntry {
                image_id: "id-2".to_string(),
                source_path: "/src/b.png".to_string(),
                relative_path: "b.png".to_string(),
                file_size: 200,
                album_name: "album_a".to_string(),
            },
        ];

        let json = serde_json::to_value(&entries).unwrap();
        let back: Vec<FrozenPlanEntry> = serde_json::from_value(json).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].image_id, "id-1");
        assert_eq!(back[1].file_size, 200);
    }

    #[test]
    fn manifest_serialization() {
        let manifest = AlbumManifest {
            album_name: "test_album".to_string(),
            relative_path: "test_album".to_string(),
            image_count: 2,
            images: vec![
                AlbumManifestImage {
                    relative_path: "img1.jpg".to_string(),
                    file_size: 1024,
                    blake3: "abcdef".to_string(),
                    width: Some(800),
                    height: Some(600),
                    format: Some("jpeg".to_string()),
                },
                AlbumManifestImage {
                    relative_path: "img2.png".to_string(),
                    file_size: 2048,
                    blake3: "123456".to_string(),
                    width: Some(1024),
                    height: Some(768),
                    format: Some("png".to_string()),
                },
            ],
        };

        let json = serde_json::to_string_pretty(&manifest).unwrap();
        let back: AlbumManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.album_name, "test_album");
        assert_eq!(back.image_count, 2);
        assert_eq!(back.images[0].relative_path, "img1.jpg");
        assert_eq!(back.images[1].blake3, "123456");
    }

    /// Real PostgreSQL + filesystem integration test for the full commit pipeline.
    ///
    /// Invocation:
    ///   IMAGEDB_POSTGRES_BIN=/path/to/pgsql/bin cargo test \
    ///       --manifest-path apps/desktop/src-tauri/Cargo.toml \
    ///       --features real-db-tests real_commit_full_pipeline \
    ///       -- --ignored --nocapture --test-threads=1
    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_commit_full_pipeline() {
        use crate::domain::import_state::{DecodeState, ImportImageState, ImportRunState};
        use crate::infrastructure::postgres::{MigrationRunner, PostgresManager};
        use crate::repositories::import_repository::NewImportImage;
        use tempfile::TempDir;

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
        std::fs::write(album_path.join("notes.txt"), b"album sidecar").unwrap();

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

        let _img1_id = ImportRepository::insert_import_image(
            &client,
            NewImportImage {
                album_id,
                source_path: album_path.join("photo1.png").display().to_string(),
                relative_path: "album_a/photo1.png".to_string(),
                file_size: 14,
                modified_at: None,
                width: Some(10),
                height: Some(10),
                format: Some("png".to_string()),
                decode_state: DecodeState::Decoded,
                blake3: Some(img1_blake3.clone()),
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

        let _img2_id = ImportRepository::insert_import_image(
            &client,
            NewImportImage {
                album_id,
                source_path: album_path.join("photo2.png").display().to_string(),
                relative_path: "album_a/photo2.png".to_string(),
                file_size: 14,
                modified_at: None,
                width: Some(10),
                height: Some(10),
                format: Some("png".to_string()),
                decode_state: DecodeState::Decoded,
                blake3: Some(img2_blake3.clone()),
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

        ImportRepository::update_import_run_state(
            &client,
            import_run_id,
            &ImportRunState::Completed,
        )
        .await
        .unwrap();

        drop(client);
        db_handle.abort();

        let cancelled = Arc::new(AtomicBool::new(false));
        let progress_tracker =
            Arc::new(Mutex::new(CommitProgress::idle(&import_run_id.to_string())));
        let pg_manager = Arc::new(Mutex::new(manager));

        let result = run_import_commit(
            pg_manager.clone(),
            library_root.display().to_string(),
            import_run_id,
            cancelled,
            progress_tracker,
        )
        .await;

        assert!(result.is_ok(), "commit failed: {:?}", result.err());
        let commit_result = result.unwrap();
        assert_eq!(commit_result.state, "completed");
        assert_eq!(commit_result.albums_committed, 1);
        assert_eq!(commit_result.images_committed, 2);
        assert_eq!(commit_result.albums_failed, 0);

        let publish_dir = library_root.join("Albums").join("album_a");
        assert!(publish_dir.exists(), "publish dir should exist");
        assert!(publish_dir.join("photo1.png").exists());
        assert!(publish_dir.join("photo2.png").exists());

        let published_data = std::fs::read(publish_dir.join("photo1.png")).unwrap();
        let published_hash = blake3::hash(&published_data).as_bytes().to_vec();
        assert_eq!(published_hash, img1_blake3);

        let manifest_file = library_root
            .join(".imagedb")
            .join("manifests")
            .join("album_a.json");
        assert!(manifest_file.exists(), "manifest should exist");
        let manifest_json = std::fs::read_to_string(&manifest_file).unwrap();
        let manifest: AlbumManifest = serde_json::from_str(&manifest_json).unwrap();
        assert_eq!(manifest.album_name, "album_a");
        assert_eq!(manifest.image_count, 2);

        let (client2, db_handle2) = {
            let mgr = pg_manager.lock().await;
            mgr.connect().await.unwrap()
        };
        let lib_album =
            ImportRepository::find_library_album_by_path(&client2, library_root_id, "album_a")
                .await
                .unwrap();
        assert!(lib_album.is_some(), "library album should be committed");
        let lib_album = lib_album.unwrap();
        assert_eq!(lib_album.state, "committed");
        assert_eq!(lib_album.image_count, 2);

        let archive_dir = source_root.join(".imagedb-archive").join("album_a");
        assert!(archive_dir.exists(), "source should be archived");
        assert!(archive_dir.join("photo1.png").exists());
        assert!(
            archive_dir.join("notes.txt").exists(),
            "archive should include the full source album"
        );

        assert!(
            album_path.join("photo1.png").exists(),
            "source files should remain intact"
        );

        let rerun_cancelled = Arc::new(AtomicBool::new(false));
        let rerun_progress = Arc::new(Mutex::new(CommitProgress::idle(&import_run_id.to_string())));
        let rerun = run_import_commit(
            pg_manager.clone(),
            library_root.display().to_string(),
            import_run_id,
            rerun_cancelled,
            rerun_progress,
        )
        .await
        .unwrap();
        assert_eq!(rerun.albums_committed, 0);
        assert_eq!(rerun.albums_skipped, 1);
        assert_eq!(rerun.albums_failed, 0);

        let library_image_count: i64 = client2
            .query_one(
                "SELECT COUNT(*) FROM library_images WHERE album_id = $1",
                &[&lib_album.id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(library_image_count, 2);

        drop(client2);
        db_handle2.abort();
        let mut mgr = pg_manager.lock().await;
        mgr.shutdown().await.unwrap();
    }
}
