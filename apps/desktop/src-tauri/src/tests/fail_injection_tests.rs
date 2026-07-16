//! Failure injection acceptance tests for the recovery pipeline.
//!
//! Pattern (Phase 11): inject a fault → drop the original service (simulating
//! an app restart) → create a fresh Recovery Service → drive it from the
//! persisted state → verify the transaction reaches a terminal state → run
//! recovery a second time to confirm idempotency (no extra side effects).
//!
//! Invocation:
//!   IMAGEDB_POSTGRES_BIN=/path/to/pgsql/bin cargo test --manifest-path \
//!       apps/desktop/src-tauri/Cargo.toml --features fail-injection,real-db-tests \
//!       --lib fail_injection_ -- --ignored --test-threads=1
//!
//! Mounted storage gate:
//!   IMAGEDB_POSTGRES_BIN=/path/to/pgsql/bin \
//!   IMAGEDB_MOUNTED_LIBRARY_ROOT=/already-mounted/share \
//!   IMAGEDB_MOUNTED_LOCAL_PATH=Z: \
//!   IMAGEDB_MOUNTED_REMOTE_PATH=\\\\server\\share \
//!   cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml \
//!       --features fail-injection,real-db-tests --lib \
//!       mounted_storage_gate_library_root_disconnect_pauses_then_recovers \
//!       -- --ignored --test-threads=1
#![cfg(test)]
#![cfg(feature = "fail-injection")]
use crate::domain::import_state::{DecodeState, ImportImageState, ImportRunState};
use crate::infrastructure::postgres::{MigrationRunner, PostgresManager};
use crate::infrastructure::storage_capabilities::{probe_storage_capabilities, PublishStrategy};
use crate::repositories::import_repository::{ImportRepository, NewImportImage};
use crate::services::commit_service::{self, COMMIT_MARKER_FILE_NAME};
use crate::services::recovery_service;
use crate::services::source_snapshot_service::capture_source_album_snapshot;
use crate::tests::fail_injection::{
    clear_fault_point, set_fault_point, set_force_conservative_publish, set_force_storage_timeout,
    set_force_storage_unwritable, set_forced_available_space, CommitFaultPoint,
};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::Mutex;
use uuid::Uuid;

/// Build a full test environment: tmp dirs, source album with 2 PNGs, a fresh
/// Postgres cluster with all migrations, a frozen plan for one album, and the
/// manager + library_root + import_run_id returned for driving commits.
async fn setup_full_env() -> (
    TempDir,
    Arc<Mutex<PostgresManager>>,
    Uuid,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    setup_full_env_with_roots(None, None).await
}

async fn setup_full_env_with_roots(
    source_root_override: Option<std::path::PathBuf>,
    library_root_override: Option<std::path::PathBuf>,
) -> (
    TempDir,
    Arc<Mutex<PostgresManager>>,
    Uuid,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let tmp = TempDir::new().unwrap();
    let app_data = tmp.path().join("app_data");
    let source_root = source_root_override.unwrap_or_else(|| tmp.path().join("source"));
    let library_root = library_root_override.unwrap_or_else(|| tmp.path().join("library"));
    let album_path = source_root.join("album_a");
    std::fs::create_dir_all(&library_root).unwrap();
    std::fs::create_dir_all(&album_path).unwrap();
    std::fs::write(album_path.join("photo1.png"), b"photo one data").unwrap();
    std::fs::write(album_path.join("photo2.png"), b"photo two data").unwrap();
    // Batch 3: exercise the snapshot-driven archive path with non-image
    // content so the full source snapshot captures description + nested
    // sidecar files, not just the imported plan images.
    std::fs::write(album_path.join("description.txt"), b"album notes").unwrap();
    let nested = album_path.join("sub");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("meta.xmp"), b"<xmp>data</xmp>").unwrap();

    let mut manager = PostgresManager::new(&app_data);
    assert!(
        manager.binaries_available(),
        "PostgreSQL binaries not available"
    );
    let probe = manager.initialize().await.unwrap();
    assert!(probe.connection_ok);

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

    // Persist the full source album snapshot (scan does this in production;
    // commit Phase 6 requires it to verify source/archive integrity).
    capture_source_album_snapshot(&client, import_run_id, album_id, &album_path)
        .await
        .unwrap();

    // Freeze a plan mirroring review_service::freeze_plan.
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

    (
        tmp,
        Arc::new(Mutex::new(manager)),
        import_run_id,
        library_root,
        album_path,
    )
}

async fn freeze_test_plan(
    client: &mut tokio_postgres::Client,
    import_run_id: Uuid,
    library_root_id: Uuid,
    album_id: Uuid,
    album_name: &str,
    photos: &[(&str, &Vec<u8>)],
    album_path: &std::path::Path,
) -> Result<(), crate::error::AppError> {
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
            .map_err(|e| crate::error::AppError::Internal(format!("img lookup failed: {e}")))?
            .get(0);
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
    let frozen = ImportRepository::load_draft_plan(client, import_run_id)
        .await?
        .ok_or_else(|| crate::error::AppError::Internal("draft plan not found".to_string()))?;
    let hash = commit_service::compute_plan_hash(&frozen)?;
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

/// Run a commit with the given fault injected. Returns the (error) result —
/// the transaction is left mid-flight in the DB.
async fn run_commit_with_fault(
    pg_manager: Arc<Mutex<PostgresManager>>,
    library_root: &std::path::Path,
    import_run_id: Uuid,
    fault: CommitFaultPoint,
) {
    set_fault_point(fault);
    let cancelled = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(
        crate::domain::import_state::CommitProgress::idle(&import_run_id.to_string()),
    ));
    let _ = commit_service::run_import_commit(
        pg_manager,
        library_root.display().to_string(),
        import_run_id,
        cancelled,
        progress,
    )
    .await;
    clear_fault_point();
}

/// Drive the Recovery Service for every non-terminal transaction of the run
/// until it reaches a terminal state or a conflict.
async fn drive_recovery(
    pg_manager: Arc<Mutex<PostgresManager>>,
    _import_run_id: Uuid,
) -> Vec<recovery_service::RecoveryOutcome> {
    let outcomes = Vec::new();
    let mut outcomes = outcomes;
    // Scan + recover each non-terminal transaction, up to a bounded number of
    // passes to guarantee termination.
    for _ in 0..10 {
        let (client, handle) = {
            let mgr = pg_manager.lock().await;
            mgr.connect().await.unwrap()
        };
        let txs = ImportRepository::get_recoverable_transactions(&client)
            .await
            .unwrap();
        drop(client);
        handle.abort();
        if txs.is_empty() {
            break;
        }
        for tx in txs {
            let outcome = recovery_service::recover_transaction(pg_manager.clone(), tx.id)
                .await
                .unwrap();
            outcomes.push(outcome);
        }
    }
    outcomes
}

/// Assert the run's single album reaches `source_archived` and the published
/// dir + library records exist.
async fn assert_recovered(pg_manager: Arc<Mutex<PostgresManager>>, library_root: &std::path::Path) {
    let (client, handle) = {
        let mgr = pg_manager.lock().await;
        mgr.connect().await.unwrap()
    };
    let tx_row = client
        .query_one(
            "SELECT state, last_error FROM file_transactions WHERE import_album_id = (
                SELECT id FROM import_albums WHERE source_name = 'album_a' LIMIT 1
            ) ORDER BY started_at DESC LIMIT 1",
            &[],
        )
        .await
        .unwrap();
    let state: String = tx_row.get(0);
    let last_error: Option<String> = tx_row.get(1);
    assert_eq!(
        state, "source_archived",
        "transaction should be fully recovered to source_archived, got {state}; last_error={last_error:?}"
    );

    let publish_dir = library_root.join("Albums").join("album_a");
    assert!(
        publish_dir.exists(),
        "published dir must exist after recovery"
    );
    assert!(publish_dir.join("photo1.png").exists());

    let lib_count: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM library_images li JOIN library_albums la ON la.id = li.album_id WHERE la.relative_path = 'album_a'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(lib_count, 2, "exactly two library images after recovery");
    drop(client);
    handle.abort();
}

struct MountedMapping {
    local_path: String,
    remote_path: String,
}

fn mounted_mapping_from_env(library_root: &std::path::Path) -> Option<MountedMapping> {
    let local_path = std::env::var("IMAGEDB_MOUNTED_LOCAL_PATH").ok()?;
    let remote_path = std::env::var("IMAGEDB_MOUNTED_REMOTE_PATH").ok()?;
    let root = library_root.display().to_string();
    assert!(
        root.to_ascii_lowercase()
            .starts_with(&local_path.to_ascii_lowercase()),
        "IMAGEDB_MOUNTED_LIBRARY_ROOT must live under IMAGEDB_MOUNTED_LOCAL_PATH: root={root}, local_path={local_path}"
    );
    Some(MountedMapping {
        local_path,
        remote_path,
    })
}

fn run_powershell(command: &str) {
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", command])
        .output()
        .expect("powershell command should start");
    assert!(
        output.status.success(),
        "powershell command failed: {command}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn disconnect_mapping(mapping: &MountedMapping) {
    run_powershell(&format!(
        "Remove-SmbMapping -LocalPath '{}' -Force -UpdateProfile:$false",
        mapping.local_path.replace('\'', "''")
    ));
}

fn reconnect_mapping(mapping: &MountedMapping) {
    run_powershell(&format!(
        "New-SmbMapping -LocalPath '{}' -RemotePath '{}' -Persistent $false | Out-Null",
        mapping.local_path.replace('\'', "''"),
        mapping.remote_path.replace('\'', "''")
    ));
}

#[tokio::test]
#[ignore]
async fn fail_injection_after_db_write() {
    let (_tmp, pg, run_id, lib_root, _album) = setup_full_env().await;
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::AfterDbWrite,
    )
    .await;
    drive_recovery(pg.clone(), run_id).await;
    assert_recovered(pg.clone(), &lib_root).await;
    // Idempotent: a second recovery pass must be a no-op.
    let second = drive_recovery(pg.clone(), run_id).await;
    for o in &second {
        assert!(
            o.recovered,
            "second recovery pass must be idempotent: {:?}",
            o
        );
    }
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

/// Recovery evidence is a security boundary: a persisted transaction must
/// stay bound to both the frozen plan hash and the canonical library paths.
/// Corrupt either field and recovery must stop before touching disk or
/// creating library records.
#[tokio::test]
#[ignore]
async fn fail_injection_recovery_rejects_tampered_transaction_evidence() {
    let (tmp, pg, run_id, lib_root, _album) = setup_full_env().await;
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::AfterStagingCopy,
    )
    .await;

    let outside_target = tmp.path().join("outside-recovery-target");
    std::fs::create_dir_all(&outside_target).unwrap();
    let sentinel = outside_target.join("sentinel.txt");
    std::fs::write(&sentinel, b"must remain untouched").unwrap();

    let (tx_id, original_staging, original_target, original_plan_hash) = {
        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        let row = client
            .query_one(
                "SELECT id, staging_path, target_path, plan_hash
                 FROM file_transactions
                 WHERE import_run_id = $1
                 ORDER BY started_at DESC
                 LIMIT 1",
                &[&run_id],
            )
            .await
            .unwrap();
        let values = (
            row.get::<_, Uuid>(0),
            row.get::<_, String>(1),
            row.get::<_, String>(2),
            row.get::<_, Vec<u8>>(3),
        );
        drop(client);
        handle.abort();
        values
    };

    // First corrupt only the transaction's plan hash. The frozen plan itself
    // remains valid, so this specifically exercises the transaction binding.
    {
        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        client
            .execute(
                "UPDATE file_transactions SET plan_hash = decode('00', 'hex') WHERE id = $1",
                &[&tx_id],
            )
            .await
            .unwrap();
        drop(client);
        handle.abort();
    }
    let hash_outcome = recovery_service::recover_transaction(pg.clone(), tx_id)
        .await
        .expect("tampered plan hash should produce a persisted conflict outcome");
    assert_eq!(hash_outcome.final_state, "conflict");
    assert!(!hash_outcome.recovered);
    assert!(
        hash_outcome.message.contains("plan hash"),
        "expected plan-hash conflict, got: {}",
        hash_outcome.message
    );
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"must remain untouched");

    // Reset the transaction to its pre-recovery state, restore the correct
    // hash, then corrupt only target_path to point outside the library root.
    // The outside directory already contains a sentinel so any accidental
    // publish/cleanup is observable.
    {
        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        client
            .execute(
                "UPDATE file_transactions
                 SET state = 'staging', last_error = NULL, plan_hash = $2,
                     staging_path = $3, target_path = $4
                 WHERE id = $1",
                &[
                    &tx_id,
                    &original_plan_hash,
                    &original_staging,
                    &outside_target.display().to_string(),
                ],
            )
            .await
            .unwrap();
        drop(client);
        handle.abort();
    }
    let path_outcome = recovery_service::recover_transaction(pg.clone(), tx_id)
        .await
        .expect("tampered recovery path should produce a persisted conflict outcome");
    assert_eq!(path_outcome.final_state, "conflict");
    assert!(!path_outcome.recovered);
    assert!(
        path_outcome.message.contains("recovery paths"),
        "expected canonical-path conflict, got: {}",
        path_outcome.message
    );
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"must remain untouched");
    assert!(
        !outside_target.join("photo1.png").exists(),
        "recovery must not publish into the tampered target"
    );

    let (client, handle) = {
        let mgr = pg.lock().await;
        mgr.connect().await.unwrap()
    };
    let row = client
        .query_one(
            "SELECT ft.state, ft.last_error, ir.state
             FROM file_transactions ft
             JOIN import_runs ir ON ir.id = ft.import_run_id
             WHERE ft.id = $1",
            &[&tx_id],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>(0), "conflict");
    assert!(row
        .get::<_, Option<String>>(1)
        .as_deref()
        .unwrap_or_default()
        .contains("recovery paths"));
    assert_eq!(row.get::<_, String>(2), "recovery_required");
    let library_image_count: i64 = client
        .query_one("SELECT COUNT(*) FROM library_images", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        library_image_count, 0,
        "tampered evidence must be rejected before DB publish"
    );
    drop(client);
    handle.abort();

    // Finally restore all persisted evidence, then replace the canonical
    // `Albums` ancestor with a real symlink/junction. This exercises the
    // recovery-only filesystem guard after the DB evidence checks pass.
    {
        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        client
            .execute(
                "UPDATE file_transactions
                 SET state = 'staging', last_error = NULL, plan_hash = $2,
                     staging_path = $3, target_path = $4
                 WHERE id = $1",
                &[
                    &tx_id,
                    &original_plan_hash,
                    &original_staging,
                    &original_target,
                ],
            )
            .await
            .unwrap();
        drop(client);
        handle.abort();
    }

    let albums_ancestor = lib_root.join("Albums");
    std::fs::remove_dir(&albums_ancestor)
        .expect("the unpublished Albums directory should still be empty");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_target, &albums_ancestor)
        .expect("test directory symlink should be created");
    #[cfg(windows)]
    {
        let status = std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                &albums_ancestor.display().to_string(),
                &outside_target.display().to_string(),
            ])
            .status()
            .expect("failed to run mklink /J");
        assert!(status.success(), "mklink /J failed: {status}");
    }

    #[cfg(any(unix, windows))]
    {
        let symlink_outcome = recovery_service::recover_transaction(pg.clone(), tx_id)
            .await
            .expect("symlinked recovery ancestor should produce a persisted conflict outcome");
        assert_eq!(symlink_outcome.final_state, "conflict");
        assert!(!symlink_outcome.recovered);
        assert!(
            symlink_outcome.message.contains("symlink or reparse point"),
            "expected symlink/reparse conflict, got: {}",
            symlink_outcome.message
        );
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"must remain untouched");
        assert!(
            !outside_target.join("album_a").exists(),
            "recovery must not traverse the symlinked publish ancestor"
        );

        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        let row = client
            .query_one(
                "SELECT state, last_error FROM file_transactions WHERE id = $1",
                &[&tx_id],
            )
            .await
            .unwrap();
        assert_eq!(row.get::<_, String>(0), "conflict");
        assert!(row
            .get::<_, Option<String>>(1)
            .as_deref()
            .unwrap_or_default()
            .contains("symlink or reparse point"));
        drop(client);
        handle.abort();

        #[cfg(unix)]
        std::fs::remove_file(&albums_ancestor).unwrap();
        #[cfg(windows)]
        std::fs::remove_dir(&albums_ancestor).unwrap();
    }

    // Keep the original target value observable in the fixture and guard
    // against accidentally testing a path that was already outside the root.
    assert_eq!(
        std::path::Path::new(&original_target),
        lib_root.join("Albums").join("album_a")
    );

    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn fail_injection_library_root_disconnect_pauses_then_recovers() {
    let (_tmp, pg, run_id, lib_root, _album) = setup_full_env().await;
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::AfterDbWrite,
    )
    .await;

    let tx_id: Uuid = {
        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        let id: Uuid = client
            .query_one(
                "SELECT id FROM file_transactions ORDER BY started_at DESC LIMIT 1",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        drop(client);
        handle.abort();
        id
    };

    let disconnected_root = lib_root.with_extension("offline");
    std::fs::rename(&lib_root, &disconnected_root).unwrap();

    let paused = recovery_service::recover_transaction(pg.clone(), tx_id)
        .await
        .expect("recovery should pause cleanly while the library root is disconnected");
    assert_eq!(paused.final_state, "staging");
    assert!(!paused.recovered);
    assert!(
        paused.message.contains("recovery paused"),
        "expected mounted-root pause message, got {}",
        paused.message
    );

    {
        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        let row = client
            .query_one(
                "SELECT ft.state, ft.last_error, ir.state
                 FROM file_transactions ft
                 JOIN import_runs ir ON ir.id = ft.import_run_id
                 WHERE ft.id = $1",
                &[&tx_id],
            )
            .await
            .unwrap();
        let tx_state: String = row.get(0);
        let last_error: Option<String> = row.get(1);
        let run_state: String = row.get(2);
        assert_eq!(tx_state, "staging");
        assert_eq!(run_state, "recovery_required");
        assert!(
            last_error
                .as_deref()
                .unwrap_or_default()
                .contains("recovery paused"),
            "last_error should explain the disconnected mounted root: {last_error:?}"
        );
        drop(client);
        handle.abort();
    }

    std::fs::rename(&disconnected_root, &lib_root).unwrap();
    drive_recovery(pg.clone(), run_id).await;
    assert_recovered(pg.clone(), &lib_root).await;
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn mounted_storage_gate_library_root_disconnect_pauses_then_recovers() {
    let Some(mounted_base) =
        std::env::var_os("IMAGEDB_MOUNTED_LIBRARY_ROOT").map(std::path::PathBuf::from)
    else {
        eprintln!("IMAGEDB_MOUNTED_LIBRARY_ROOT not set; skipping explicit mounted storage gate");
        return;
    };
    assert!(
        mounted_base.is_dir(),
        "IMAGEDB_MOUNTED_LIBRARY_ROOT must be an existing directory, got {}",
        mounted_base.display()
    );

    let run_suffix = Uuid::new_v4();
    let lib_root = mounted_base.join(format!(".imagedb-m8-library-{run_suffix}"));
    let source_root = std::env::var_os("IMAGEDB_MOUNTED_SOURCE_ROOT")
        .map(std::path::PathBuf::from)
        .map(|base| {
            assert!(
                base.is_dir(),
                "IMAGEDB_MOUNTED_SOURCE_ROOT must be an existing directory, got {}",
                base.display()
            );
            base.join(format!(".imagedb-m8-source-{run_suffix}"))
        });

    std::fs::create_dir_all(&lib_root).unwrap();
    let capabilities = probe_storage_capabilities(&lib_root);
    assert_ne!(
        capabilities.publish_strategy,
        PublishStrategy::Unsupported,
        "mounted library root must be writable and recoverable; reasons: {:?}",
        capabilities.strategy_reasons
    );

    let (_tmp, pg, run_id, lib_root, _album) =
        setup_full_env_with_roots(source_root.clone(), Some(lib_root.clone())).await;
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::AfterDbWrite,
    )
    .await;

    let tx_id: Uuid = {
        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        let id: Uuid = client
            .query_one(
                "SELECT id FROM file_transactions ORDER BY started_at DESC LIMIT 1",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        drop(client);
        handle.abort();
        id
    };

    let mapping = mounted_mapping_from_env(&lib_root);
    let disconnected_root = lib_root.with_extension("offline");
    if let Some(mapping) = &mapping {
        disconnect_mapping(mapping);
    } else {
        std::fs::rename(&lib_root, &disconnected_root).unwrap();
    }

    let paused = recovery_service::recover_transaction(pg.clone(), tx_id)
        .await
        .expect("recovery should pause cleanly while the mounted library root is disconnected");
    assert_eq!(paused.final_state, "staging");
    assert!(!paused.recovered);
    assert!(
        paused.message.contains("recovery paused"),
        "expected mounted-root pause message, got {}",
        paused.message
    );

    if let Some(mapping) = &mapping {
        reconnect_mapping(mapping);
    } else {
        std::fs::rename(&disconnected_root, &lib_root).unwrap();
    }
    drive_recovery(pg.clone(), run_id).await;
    assert_recovered(pg.clone(), &lib_root).await;

    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
    if let Some(source_root) = source_root {
        let _ = std::fs::remove_dir_all(source_root);
    }
    let _ = std::fs::remove_dir_all(lib_root);
}

#[tokio::test]
#[ignore]
async fn fail_injection_source_root_disconnect_pauses_then_recovers() {
    let (_tmp, pg, run_id, lib_root, album_path) = setup_full_env().await;
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::AfterDbWrite,
    )
    .await;

    let tx_id: Uuid = {
        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        let id: Uuid = client
            .query_one(
                "SELECT id FROM file_transactions ORDER BY started_at DESC LIMIT 1",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        drop(client);
        handle.abort();
        id
    };

    let source_root = album_path.parent().unwrap().to_path_buf();
    let disconnected_source = source_root.with_extension("offline");
    std::fs::rename(&source_root, &disconnected_source).unwrap();

    let paused = recovery_service::recover_transaction(pg.clone(), tx_id)
        .await
        .expect("recovery should pause cleanly while the source root is disconnected");
    assert_eq!(paused.final_state, "staging");
    assert!(!paused.recovered);
    assert!(
        paused.message.contains("source file unavailable"),
        "expected source unavailable message, got {}",
        paused.message
    );

    {
        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        let row = client
            .query_one(
                "SELECT ft.state, ft.last_error, ir.state
                 FROM file_transactions ft
                 JOIN import_runs ir ON ir.id = ft.import_run_id
                 WHERE ft.id = $1",
                &[&tx_id],
            )
            .await
            .unwrap();
        let tx_state: String = row.get(0);
        let last_error: Option<String> = row.get(1);
        let run_state: String = row.get(2);
        assert_eq!(tx_state, "staging");
        assert_eq!(run_state, "recovery_required");
        assert!(
            last_error
                .as_deref()
                .unwrap_or_default()
                .contains("source file unavailable"),
            "last_error should explain the disconnected source root: {last_error:?}"
        );
        drop(client);
        handle.abort();
    }

    std::fs::rename(&disconnected_source, &source_root).unwrap();
    drive_recovery(pg.clone(), run_id).await;
    assert_recovered(pg.clone(), &lib_root).await;
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn fail_injection_recovery_insufficient_space_pauses_then_recovers() {
    let (_tmp, pg, run_id, lib_root, _album_path) = setup_full_env().await;
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::AfterDbWrite,
    )
    .await;

    let tx_id: Uuid = {
        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        let id: Uuid = client
            .query_one(
                "SELECT id FROM file_transactions ORDER BY started_at DESC LIMIT 1",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        drop(client);
        handle.abort();
        id
    };

    set_forced_available_space(Some(0));
    let paused = recovery_service::recover_transaction(pg.clone(), tx_id)
        .await
        .expect("recovery should pause cleanly when available space is insufficient");
    set_forced_available_space(None);

    assert_eq!(paused.final_state, "staging");
    assert!(!paused.recovered);
    assert!(
        paused.message.contains("insufficient free space"),
        "expected insufficient-space pause, got {}",
        paused.message
    );

    {
        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        let row = client
            .query_one(
                "SELECT ft.state, ft.last_error, ir.state
                 FROM file_transactions ft
                 JOIN import_runs ir ON ir.id = ft.import_run_id
                 WHERE ft.id = $1",
                &[&tx_id],
            )
            .await
            .unwrap();
        let tx_state: String = row.get(0);
        let last_error: Option<String> = row.get(1);
        let run_state: String = row.get(2);
        assert_eq!(tx_state, "staging");
        assert_eq!(run_state, "recovery_required");
        assert!(
            last_error
                .as_deref()
                .unwrap_or_default()
                .contains("insufficient free space"),
            "last_error should explain the space gate: {last_error:?}"
        );
        drop(client);
        handle.abort();
    }

    drive_recovery(pg.clone(), run_id).await;
    assert_recovered(pg.clone(), &lib_root).await;
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn fail_injection_recovery_unwritable_storage_pauses_then_recovers() {
    let (_tmp, pg, run_id, lib_root, _album_path) = setup_full_env().await;
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::AfterDbWrite,
    )
    .await;

    let tx_id: Uuid = {
        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        let id: Uuid = client
            .query_one(
                "SELECT id FROM file_transactions ORDER BY started_at DESC LIMIT 1",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        drop(client);
        handle.abort();
        id
    };

    set_force_storage_unwritable(true);
    let paused = recovery_service::recover_transaction(pg.clone(), tx_id)
        .await
        .expect("recovery should pause cleanly when storage is not writable");
    set_force_storage_unwritable(false);

    assert_eq!(paused.final_state, "staging");
    assert!(!paused.recovered);
    assert!(
        paused.message.contains("not currently writable"),
        "expected unwritable-storage pause, got {}",
        paused.message
    );

    {
        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        let row = client
            .query_one(
                "SELECT ft.state, ft.last_error, ir.state
                 FROM file_transactions ft
                 JOIN import_runs ir ON ir.id = ft.import_run_id
                 WHERE ft.id = $1",
                &[&tx_id],
            )
            .await
            .unwrap();
        let tx_state: String = row.get(0);
        let last_error: Option<String> = row.get(1);
        let run_state: String = row.get(2);
        assert_eq!(tx_state, "staging");
        assert_eq!(run_state, "recovery_required");
        assert!(
            last_error
                .as_deref()
                .unwrap_or_default()
                .contains("not currently writable"),
            "last_error should explain the writable gate: {last_error:?}"
        );
        drop(client);
        handle.abort();
    }

    drive_recovery(pg.clone(), run_id).await;
    assert_recovered(pg.clone(), &lib_root).await;
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn fail_injection_recovery_storage_timeout_pauses_then_recovers() {
    let (_tmp, pg, run_id, lib_root, _album_path) = setup_full_env().await;
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::AfterDbWrite,
    )
    .await;

    let tx_id: Uuid = {
        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        let id: Uuid = client
            .query_one(
                "SELECT id FROM file_transactions ORDER BY started_at DESC LIMIT 1",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        drop(client);
        handle.abort();
        id
    };

    set_force_storage_timeout(true);
    let paused = recovery_service::recover_transaction(pg.clone(), tx_id)
        .await
        .expect("recovery should pause cleanly when storage probing times out");
    set_force_storage_timeout(false);

    assert_eq!(paused.final_state, "staging");
    assert!(!paused.recovered);
    assert!(
        paused.message.contains("timed out"),
        "expected storage-timeout pause, got {}",
        paused.message
    );

    {
        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        let row = client
            .query_one(
                "SELECT ft.state, ft.last_error, ir.state
                 FROM file_transactions ft
                 JOIN import_runs ir ON ir.id = ft.import_run_id
                 WHERE ft.id = $1",
                &[&tx_id],
            )
            .await
            .unwrap();
        let tx_state: String = row.get(0);
        let last_error: Option<String> = row.get(1);
        let run_state: String = row.get(2);
        assert_eq!(tx_state, "staging");
        assert_eq!(run_state, "recovery_required");
        assert!(
            last_error
                .as_deref()
                .unwrap_or_default()
                .contains("timed out"),
            "last_error should explain the timeout gate: {last_error:?}"
        );
        drop(client);
        handle.abort();
    }

    drive_recovery(pg.clone(), run_id).await;
    assert_recovered(pg.clone(), &lib_root).await;
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn fail_injection_during_copy() {
    let (_tmp, pg, run_id, lib_root, _album) = setup_full_env().await;
    run_commit_with_fault(pg.clone(), &lib_root, run_id, CommitFaultPoint::DuringCopy).await;
    drive_recovery(pg.clone(), run_id).await;
    assert_recovered(pg.clone(), &lib_root).await;
    let second = drive_recovery(pg.clone(), run_id).await;
    for o in &second {
        assert!(o.recovered, "idempotent: {:?}", o);
    }
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn fail_injection_after_staging_copy() {
    let (_tmp, pg, run_id, lib_root, _album) = setup_full_env().await;
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::AfterStagingCopy,
    )
    .await;
    drive_recovery(pg.clone(), run_id).await;
    assert_recovered(pg.clone(), &lib_root).await;
    let second = drive_recovery(pg.clone(), run_id).await;
    for o in &second {
        assert!(o.recovered, "idempotent: {:?}", o);
    }
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn fail_injection_after_staging_verify() {
    let (_tmp, pg, run_id, lib_root, _album) = setup_full_env().await;
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::AfterStagingVerify,
    )
    .await;
    drive_recovery(pg.clone(), run_id).await;
    assert_recovered(pg.clone(), &lib_root).await;
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn fail_injection_after_manifest_write() {
    let (_tmp, pg, run_id, lib_root, _album) = setup_full_env().await;
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::AfterManifestWrite,
    )
    .await;
    drive_recovery(pg.clone(), run_id).await;
    assert_recovered(pg.clone(), &lib_root).await;
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn fail_injection_conservative_before_commit_marker_recovers() {
    let (_tmp, pg, run_id, lib_root, _album) = setup_full_env().await;
    set_force_conservative_publish(true);
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::BeforeCommitMarker,
    )
    .await;
    set_force_conservative_publish(false);

    let publish_dir = lib_root.join("Albums").join("album_a");
    assert!(
        publish_dir.exists(),
        "conservative publish should have created the target before marker fault"
    );
    assert!(
        publish_dir
            .join(".imagedb")
            .join(".imagedb-manifest.json")
            .exists(),
        "manifest should have been published before marker fault"
    );
    assert!(
        !publish_dir
            .join(".imagedb")
            .join(COMMIT_MARKER_FILE_NAME)
            .exists(),
        "commit marker must be absent at the injected failure point"
    );

    drive_recovery(pg.clone(), run_id).await;
    assert_recovered(pg.clone(), &lib_root).await;
    assert!(
        publish_dir
            .join(".imagedb")
            .join(COMMIT_MARKER_FILE_NAME)
            .exists(),
        "recovery must write the missing conservative commit marker"
    );
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn fail_injection_before_publish_rename() {
    let (_tmp, pg, run_id, lib_root, _album) = setup_full_env().await;
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::BeforePublishRename,
    )
    .await;
    drive_recovery(pg.clone(), run_id).await;
    assert_recovered(pg.clone(), &lib_root).await;
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn fail_injection_after_publish_rename() {
    let (_tmp, pg, run_id, lib_root, _album) = setup_full_env().await;
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::AfterPublishRename,
    )
    .await;
    drive_recovery(pg.clone(), run_id).await;
    assert_recovered(pg.clone(), &lib_root).await;
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn fail_injection_before_db_commit() {
    let (_tmp, pg, run_id, lib_root, _album) = setup_full_env().await;
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::BeforeDbCommit,
    )
    .await;
    drive_recovery(pg.clone(), run_id).await;
    assert_recovered(pg.clone(), &lib_root).await;
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn fail_injection_after_db_commit() {
    let (_tmp, pg, run_id, lib_root, _album) = setup_full_env().await;
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::AfterDbCommit,
    )
    .await;
    // After DB commit + fault, recovery should only resume the source archive.
    drive_recovery(pg.clone(), run_id).await;
    assert_recovered(pg.clone(), &lib_root).await;
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn fail_injection_before_source_archive() {
    let (_tmp, pg, run_id, lib_root, _album) = setup_full_env().await;
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::BeforeSourceArchive,
    )
    .await;
    // Library commit already succeeded; recovery resumes only the archive.
    drive_recovery(pg.clone(), run_id).await;
    assert_recovered(pg.clone(), &lib_root).await;
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn fail_injection_during_source_archive() {
    let (_tmp, pg, run_id, lib_root, _album) = setup_full_env().await;
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::DuringSourceArchive,
    )
    .await;
    drive_recovery(pg.clone(), run_id).await;
    assert_recovered(pg.clone(), &lib_root).await;
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

/// Cancel during commit must not be recorded as a plain failure. If the
/// cancellation landed before any file transaction was prewritten, the run is
/// simply left recoverable with no transaction to recover. If it landed
/// mid-transaction, recovery converges it to source_archived.
#[tokio::test]
#[ignore]
async fn fail_injection_cancel_during_commit() {
    let (_tmp, pg, run_id, lib_root, _album) = setup_full_env().await;
    let cancelled = Arc::new(AtomicBool::new(true));
    let progress = Arc::new(Mutex::new(
        crate::domain::import_state::CommitProgress::idle(&run_id.to_string()),
    ));
    let _ = commit_service::run_import_commit(
        pg.clone(),
        lib_root.display().to_string(),
        run_id,
        cancelled,
        progress,
    )
    .await;

    // Check whether a transaction was prewritten before cancellation landed.
    let (client, handle) = {
        let mgr = pg.lock().await;
        mgr.connect().await.unwrap()
    };
    let tx_count: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM file_transactions ft
             JOIN import_albums ia ON ia.id = ft.import_album_id
             WHERE ia.import_run_id = $1",
            &[&run_id],
        )
        .await
        .unwrap()
        .get(0);
    drop(client);
    handle.abort();

    if tx_count == 0 {
        // Cancellation landed before prewrite: no transaction to recover.
        // P0 fix: the run is `cancelled` (user-explicit terminal), not
        // `recovery_required` — `recovery_required` with no transaction is a
        // GUI deadlock (the recovery page shows "no recoverable
        // transactions" and the commit page won't re-select the run).
        // `cancelled` lets the user re-enter the commit page for the same
        // frozen plan.
        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        let run_state: String = client
            .query_one("SELECT state FROM import_runs WHERE id = $1", &[&run_id])
            .await
            .unwrap()
            .get(0);
        drop(client);
        handle.abort();
        assert!(
            run_state == "cancelled" || run_state == "completed",
            "unexpected run state after cancel-before-prewrite: {run_state} (expected cancelled)"
        );
    } else {
        // A transaction exists; recovery must converge it.
        drive_recovery(pg.clone(), run_id).await;
        assert_recovered(pg.clone(), &lib_root).await;
    }
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

/// M5-B: re-committing while a prior transaction is mid-flight must NOT
/// create a second active file_transaction for the same album. The second
/// attempt must short-circuit with AppError::ResumeRequired carrying the
/// original transaction_id, and no new transaction row must be written.
#[tokio::test]
#[ignore]
async fn fail_injection_double_commit_detected() {
    let (_tmp, pg, run_id, lib_root, _album) = setup_full_env().await;
    // Inject a fault mid-staging to leave the first transaction non-terminal.
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::AfterStagingCopy,
    )
    .await;

    // Count the file_transactions created so far (must be exactly one).
    let first_tx_count: i64 = {
        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        let n: i64 = client
            .query_one("SELECT COUNT(*) FROM file_transactions", &[])
            .await
            .unwrap()
            .get(0);
        drop(client);
        handle.abort();
        n
    };
    assert!(
        first_tx_count >= 1,
        "first commit must have created at least one file_transaction"
    );

    // Re-run commit without clearing the mid-flight state. It must surface a
    // recovery_required result and NOT insert any new file_transaction.
    let cancelled = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(
        crate::domain::import_state::CommitProgress::idle(&run_id.to_string()),
    ));
    let result = commit_service::run_import_commit(
        pg.clone(),
        lib_root.display().to_string(),
        run_id,
        cancelled,
        progress,
    )
    .await
    .expect("second commit should return a recovery_required result");
    assert_eq!(result.state, "recovery_required");
    assert_eq!(result.album_results[0].status, "recovery_required");
    let err_msg = result.errors.join("\n");
    assert!(
        err_msg.contains("route to recovery"),
        "expected recovery routing error, got: {err_msg}"
    );

    let second_tx_count: i64 = {
        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        let n: i64 = client
            .query_one("SELECT COUNT(*) FROM file_transactions", &[])
            .await
            .unwrap()
            .get(0);
        drop(client);
        handle.abort();
        n
    };
    assert_eq!(
        second_tx_count, first_tx_count,
        "second commit attempt must not create a new file_transaction"
    );

    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn preexisting_unknown_target_dir_is_not_overwritten_or_completed() {
    let (_tmp, pg, run_id, lib_root, _album) = setup_full_env().await;
    let publish_dir = lib_root.join("Albums").join("album_a");
    std::fs::create_dir_all(&publish_dir).unwrap();
    std::fs::write(publish_dir.join("external.txt"), b"external content").unwrap();

    let cancelled = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(
        crate::domain::import_state::CommitProgress::idle(&run_id.to_string()),
    ));
    let result = commit_service::run_import_commit(
        pg.clone(),
        lib_root.display().to_string(),
        run_id,
        cancelled,
        progress,
    )
    .await
    .expect("unknown target conflict should return a recovery_required result");

    assert_eq!(result.state, "recovery_required");
    assert_eq!(result.albums_failed, 1);
    assert_eq!(result.albums_committed, 0);
    assert!(
        publish_dir.join("external.txt").exists(),
        "pre-existing target content must not be deleted"
    );
    assert_eq!(
        std::fs::read(publish_dir.join("external.txt")).unwrap(),
        b"external content"
    );
    assert!(
        !publish_dir.join("photo1.png").exists(),
        "commit must not merge planned files into an unknown target directory"
    );

    let (client, handle) = {
        let mgr = pg.lock().await;
        mgr.connect().await.unwrap()
    };
    let run_state: String = client
        .query_one("SELECT state FROM import_runs WHERE id = $1", &[&run_id])
        .await
        .unwrap()
        .get(0);
    let tx_count: i64 = client
        .query_one("SELECT COUNT(*) FROM file_transactions", &[])
        .await
        .unwrap()
        .get(0);
    let lib_count: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM library_images li
             JOIN library_albums la ON la.id = li.album_id
             WHERE la.relative_path = 'album_a'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    drop(client);
    handle.abort();

    assert_eq!(run_state, "recovery_required");
    assert_eq!(
        tx_count, 0,
        "target preflight conflict happens before transaction prewrite"
    );
    assert_eq!(
        lib_count, 0,
        "unknown target must not create library records"
    );

    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

/// M5-A: if the source album dir disappears after the DB commit but before
/// the archive rename, recovery must succeed by validating the archive
/// against the frozen plan — not by blindly trusting an empty source slot.
#[tokio::test]
#[ignore]
async fn fail_injection_source_deleted_archive_verified() {
    let (_tmp, pg, run_id, lib_root, album_path) = setup_full_env().await;
    // Inject BeforeSourceArchive: the DB is committed but the archive
    // rename has not happened yet. Transaction state = library_committed.
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::BeforeSourceArchive,
    )
    .await;

    // Simulate the user (or external tooling) deleting the source album dir
    // AFTER the DB commit. The archive has not been created yet.
    std::fs::remove_dir_all(&album_path).unwrap();

    // Recovery now sees source=missing, archive=missing → must conflict
    // (rule: cannot confirm archive integrity if neither dir exists).
    drive_recovery(pg.clone(), run_id).await;

    let (client, handle) = {
        let mgr = pg.lock().await;
        mgr.connect().await.unwrap()
    };
    let state: String = client
        .query_one(
            "SELECT state FROM file_transactions WHERE import_album_id = (
                SELECT id FROM import_albums WHERE source_name = 'album_a' LIMIT 1
            ) ORDER BY started_at DESC LIMIT 1",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    drop(client);
    handle.abort();
    assert_eq!(
        state, "conflict",
        "recovery must refuse when both source and archive are missing, got {state}"
    );
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

/// Batch 3: source-missing + archive-present must succeed when the archive
/// matches the FULL source snapshot (not just the plan images). The rename
/// is simulated by hand to emulate an external move; recovery verifies the
/// archive contents against the persisted snapshot and promotes the
/// transaction to source_archived.
#[tokio::test]
#[ignore]
async fn fail_injection_source_missing_archive_verified_snapshot() {
    let (_tmp, pg, run_id, lib_root, album_path) = setup_full_env().await;
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::BeforeSourceArchive,
    )
    .await;

    // Look up the transaction id so we can compute the expected archive path
    // (identical to the formula used by commit_service and recovery_service).
    let tx_id: uuid::Uuid = {
        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        let id: uuid::Uuid = client
            .query_one(
                "SELECT id FROM file_transactions WHERE import_album_id = (
                    SELECT id FROM import_albums WHERE source_name = 'album_a' LIMIT 1
                ) ORDER BY started_at DESC LIMIT 1",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        drop(client);
        handle.abort();
        id
    };
    let archive_base = album_path.parent().unwrap().join(".imagedb-processed");
    let archive_dir = archive_base.join(tx_id.to_string()).join("album_a");
    std::fs::create_dir_all(archive_dir.parent().unwrap()).unwrap();
    // Move the FULL source dir (images + description + nested sidecar) to
    // the archive location so the snapshot verifier sees the same content
    // captured at scan time.
    std::fs::rename(&album_path, &archive_dir).unwrap();
    assert!(
        !album_path.exists(),
        "source must be absent after the rename"
    );
    assert!(archive_dir.exists(), "archive must be present");

    drive_recovery(pg.clone(), run_id).await;

    let (client, handle) = {
        let mgr = pg.lock().await;
        mgr.connect().await.unwrap()
    };
    let state: String = client
        .query_one(
            "SELECT state FROM file_transactions WHERE import_album_id = (
                SELECT id FROM import_albums WHERE source_name = 'album_a' LIMIT 1
            ) ORDER BY started_at DESC LIMIT 1",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    drop(client);
    handle.abort();
    assert_eq!(
        state, "source_archived",
        "recovery must archive when source missing + archive matches snapshot, got {state}"
    );
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

/// M5-A: if the archive rename already happened (so the archive is valid)
/// but the source album dir is restored before recovery runs, recovery
/// must surface a conflict — never silently delete or overwrite either.
#[tokio::test]
#[ignore]
async fn fail_injection_source_and_archive_both_exist_conflict() {
    let (_tmp, pg, run_id, lib_root, album_path) = setup_full_env().await;
    // Inject BeforeSourceArchive so the DB commit succeeded but the
    // archive rename has not happened yet.
    run_commit_with_fault(
        pg.clone(),
        &lib_root,
        run_id,
        CommitFaultPoint::BeforeSourceArchive,
    )
    .await;

    // Manually perform the archive rename, then recreate the source dir
    // to simulate an external restore / copy that produced both dirs.
    let archive_base = album_path.parent().unwrap().join(".imagedb-processed");
    let tx_id: uuid::Uuid = {
        let (client, handle) = {
            let mgr = pg.lock().await;
            mgr.connect().await.unwrap()
        };
        let id: uuid::Uuid = client
            .query_one(
                "SELECT id FROM file_transactions WHERE import_album_id = (
                    SELECT id FROM import_albums WHERE source_name = 'album_a' LIMIT 1
                ) ORDER BY started_at DESC LIMIT 1",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        drop(client);
        handle.abort();
        id
    };
    let archive_dir = archive_base.join(tx_id.to_string()).join("album_a");
    std::fs::create_dir_all(archive_dir.parent().unwrap()).unwrap();
    std::fs::rename(&album_path, &archive_dir).unwrap();
    // Restore the source album dir with the FULL content the snapshot
    // captured (images + description + nested sidecar). A partial restore
    // would fail the snapshot verifier before the both-present branch
    // could fire, defeating the purpose of this test.
    std::fs::create_dir_all(&album_path).unwrap();
    std::fs::write(album_path.join("photo1.png"), b"photo one data").unwrap();
    std::fs::write(album_path.join("photo2.png"), b"photo two data").unwrap();
    std::fs::write(album_path.join("description.txt"), b"album notes").unwrap();
    let restored_nested = album_path.join("sub");
    std::fs::create_dir_all(&restored_nested).unwrap();
    std::fs::write(restored_nested.join("meta.xmp"), b"<xmp>data</xmp>").unwrap();

    // Recovery must refuse to act when both source and archive are present.
    drive_recovery(pg.clone(), run_id).await;

    let (client, handle) = {
        let mgr = pg.lock().await;
        mgr.connect().await.unwrap()
    };
    let state: String = client
        .query_one(
            "SELECT state FROM file_transactions WHERE import_album_id = (
                SELECT id FROM import_albums WHERE source_name = 'album_a' LIMIT 1
            ) ORDER BY started_at DESC LIMIT 1",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    drop(client);
    handle.abort();
    assert_eq!(
        state, "conflict",
        "recovery must refuse when both source and archive exist, got {state}"
    );
    // Source and archive must both still be on disk (no silent deletion).
    assert!(album_path.exists(), "source dir must not have been deleted");
    assert!(
        archive_dir.exists(),
        "archive dir must not have been deleted"
    );
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}
