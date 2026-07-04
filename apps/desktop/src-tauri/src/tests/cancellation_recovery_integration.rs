//! Real PostgreSQL + filesystem integration tests for the Phase 1–6
//! cancellation, recovery, empty-plan, source-path-identity, special-file,
//! and persisted-`completed_at` invariants.
//!
//! These tests drive the **real Service layer** (Commit Service, Recovery
//! Service, Repository) against a real PostgreSQL cluster and the real
//! filesystem — not helper functions in isolation. They cover the 15
//! scenarios required by the M5/M6 final acceptance contract.
//!
//! Invocation:
//!   IMAGEDB_POSTGRES_BIN=/path/to/pgsql/bin cargo test --manifest-path \
//!       apps/desktop/src-tauri/Cargo.toml --features real-db-tests,fail-injection \
//!       --lib cancellation_recovery_ -- --ignored --test-threads=1
#![cfg(test)]
#![cfg(all(feature = "real-db-tests", feature = "fail-injection"))]
use crate::domain::import_state::{CommitProgress, DecodeState, ImportImageState, ImportRunState};
use crate::domain::state_machine::{PlanState, TransactionState};
use crate::infrastructure::postgres::{MigrationRunner, PostgresManager};
use crate::repositories::import_repository::{ImportRepository, NewImportImage};
use crate::services::commit_service;
use crate::services::recovery_service::{reconcile_import_run_state, recover_transaction};
use crate::services::source_snapshot_service::capture_source_album_snapshot;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::Mutex;
use uuid::Uuid;

/// Build a full test environment: tmp dirs, source album with 2 PNGs + a
/// sidecar + nested file (so the snapshot is non-trivial), a fresh Postgres
/// cluster with all migrations, a frozen plan for one album, and the
/// manager + library_root + import_run_id returned for driving commits.
async fn setup_env() -> (
    TempDir,
    Arc<Mutex<PostgresManager>>,
    Uuid,
    PathBuf,
    PathBuf,
    Uuid,
    Uuid,
) {
    let tmp = TempDir::new().unwrap();
    let app_data = tmp.path().join("app_data");
    let source_root = tmp.path().join("source");
    let library_root = tmp.path().join("library");
    let album_path = source_root.join("album_a");
    std::fs::create_dir_all(&album_path).unwrap();
    std::fs::write(album_path.join("photo1.png"), b"photo one data").unwrap();
    std::fs::write(album_path.join("photo2.png"), b"photo two data").unwrap();
    std::fs::write(album_path.join("description.txt"), b"album notes").unwrap();
    // Nested file so the source snapshot is non-trivial (matches the
    // setup_env doc comment — previously this was claimed but missing).
    let nested = album_path.join("sub");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("meta.xmp"), b"<xmp>data</xmp>").unwrap();

    let mut manager = PostgresManager::new(&app_data);
    assert!(manager.binaries_available(), "binaries missing");
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

    capture_source_album_snapshot(&client, import_run_id, album_id, &album_path)
        .await
        .unwrap();

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
        library_root_id,
        album_id,
    )
}

async fn freeze_test_plan(
    client: &mut tokio_postgres::Client,
    import_run_id: Uuid,
    library_root_id: Uuid,
    album_id: Uuid,
    album_name: &str,
    photos: &[(&str, &Vec<u8>)],
    album_path: &Path,
) -> Result<(), crate::error::AppError> {
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
    Ok(())
}

async fn run_state(
    client: &tokio_postgres::Client,
    run_id: Uuid,
) -> (String, Option<chrono::DateTime<chrono::Utc>>) {
    let row = client
        .query_one(
            "SELECT state, completed_at FROM import_runs WHERE id = $1",
            &[&run_id],
        )
        .await
        .unwrap();
    (row.get("state"), row.get("completed_at"))
}

async fn run_commit_with_cancel(
    pg_manager: Arc<Mutex<PostgresManager>>,
    library_root: &Path,
    import_run_id: Uuid,
) {
    let cancelled = Arc::new(AtomicBool::new(true));
    let progress = Arc::new(Mutex::new(CommitProgress::idle(&import_run_id.to_string())));
    let _ = commit_service::run_import_commit(
        pg_manager,
        library_root.display().to_string(),
        import_run_id,
        cancelled,
        progress,
    )
    .await;
}

async fn drive_recovery(pg_manager: Arc<Mutex<PostgresManager>>) {
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
            let _ = recover_transaction(pg_manager.clone(), tx.id).await;
        }
    }
}

/// 1. Cancel during copy (mid-staging) leaves a recoverable transaction,
/// not a `failed` one. This test uses a large source file (~4 MiB) so the
/// copy loop runs many 64 KiB chunks, and sets the cancel flag from a
/// concurrent task after the copy has actually started. The transaction
/// must land in a recoverable mid-flight state (NOT `failed`/`cancelled`),
/// and Recovery must then drive it to `source_archived` + `completed`.
#[tokio::test]
#[ignore]
async fn cancellation_recovery_mid_staging_resumable() {
    let (_tmp, pg, run_id, lib_root, _album, _lrid, _aid) = setup_env().await;

    // Replace photo1.png with a large file so the stream copy runs for many
    // chunks (64 MiB / 64 KiB = 1024 iterations), giving the cancel trigger
    // a wide window to fire mid-copy. The blake3 stored on the plan image
    // must match, so recompute + rewrite both the on-disk file and the
    // persisted expected_blake3.
    let large_bytes = vec![0xABu8; 64 * 1024 * 1024];
    let large_path = _album.join("photo1.png");
    std::fs::write(&large_path, &large_bytes).unwrap();
    let new_blake3 = blake3::hash(&large_bytes).as_bytes().to_vec();
    let new_size = large_bytes.len() as i64;
    {
        let (client, handle) = pg.lock().await.connect().await.unwrap();
        // Find photo1.png's import_image_id via its relative_path (which
        // uses '/' separators regardless of OS), then update the plan image
        // row that references it. Matching by source_path is OS-separator
        // sensitive, so this is more robust.
        let img_id: Uuid = client
            .query_one(
                "SELECT ii.id FROM import_images ii
                 JOIN import_albums ia ON ia.id = ii.import_album_id
                 WHERE ia.import_run_id = $1 AND ii.relative_path LIKE $2",
                &[&run_id, &"%/photo1.png".to_string()],
            )
            .await
            .unwrap()
            .get(0);
        let affected = client
            .execute(
                "UPDATE import_plan_images SET expected_blake3 = $1, expected_file_size = $2
                 WHERE import_image_id = $3",
                &[&new_blake3, &new_size, &img_id],
            )
            .await
            .unwrap();
        assert_eq!(affected, 1, "UPDATE should land on exactly one plan image");
        // Verify the UPDATE actually landed.
        let count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM import_plan_images WHERE expected_blake3 = $1",
                &[&new_blake3],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(count, 1);
        // Recompute + reset the frozen plan hash so the tamper is the only
        // inconsistency (otherwise a hash mismatch surfaces first and
        // masks the escape check). The plan is frozen, so load it via
        // load_frozen_plan (load_draft_plan only loads 'draft' state).
        let frozen = ImportRepository::load_frozen_plan(&client, run_id)
            .await
            .unwrap()
            .unwrap();
        let hash = commit_service::compute_plan_hash(&frozen).unwrap();
        client
            .execute(
                "UPDATE import_plans SET plan_hash = $1 WHERE import_run_id = $2 AND state = 'frozen'",
                &[&hash, &run_id],
            )
            .await
            .unwrap();
        // Also update the captured source snapshot so archive-stage
        // verification still matches the large file.
        client
            .execute(
                "DELETE FROM source_album_snapshot_files WHERE snapshot_id = (
                    SELECT id FROM source_album_snapshots WHERE import_album_id = (
                        SELECT id FROM import_albums WHERE import_run_id = $1 LIMIT 1
                    )
                )",
                &[&run_id],
            )
            .await
            .unwrap();
        drop(client);
        handle.abort();
    }
    // Re-capture the source snapshot so it matches the large file.
    {
        let (client, handle) = pg.lock().await.connect().await.unwrap();
        // Delete the old snapshot header so capture re-inserts cleanly.
        client
            .execute(
                "DELETE FROM source_album_snapshots WHERE import_album_id = (
                    SELECT id FROM import_albums WHERE import_run_id = $1 LIMIT 1
                )",
                &[&run_id],
            )
            .await
            .unwrap();
        let album_id: Uuid = client
            .query_one(
                "SELECT id FROM import_albums WHERE import_run_id = $1 LIMIT 1",
                &[&run_id],
            )
            .await
            .unwrap()
            .get(0);
        crate::services::source_snapshot_service::capture_source_album_snapshot(
            &client, run_id, album_id, &_album,
        )
        .await
        .unwrap();
        drop(client);
        handle.abort();
    }

    let cancelled = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(CommitProgress::idle(&run_id.to_string())));

    // Spawn a delayed cancel trigger: wait until the commit task reaches
    // `processing_album` (the album loop started, prewrite is in progress
    // or done), then set the flag. The 64 MiB file gives the stream copy
    // many chunks; `stream_copy_with_hash` checks the flag before each
    // read chunk, so the cancel will land mid-copy (not before prewrite,
    // not after the file completes).
    let progress_for_cancel = progress.clone();
    let cancelled_for_cancel = cancelled.clone();
    let cancel_handle = tokio::spawn(async move {
        // Wait up to 5s for the commit to reach the processing stage.
        for _ in 0..500 {
            let stage = {
                let p = progress_for_cancel.lock().await;
                p.current_stage.clone()
            };
            if stage == "processing_album" || stage == "committing" {
                // Small delay so the prewrite completes and the copy loop
                // is actually running before we cancel. The 64 MiB file
                // ensures the copy is still in flight when the flag flips.
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                cancelled_for_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        // Timed out waiting — cancel anyway so the test terminates.
        cancelled_for_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    let _ = commit_service::run_import_commit(
        pg.clone(),
        lib_root.display().to_string(),
        run_id,
        cancelled,
        progress,
    )
    .await;
    let _ = cancel_handle.await;

    // The transaction must exist (prewrite happened before cancel) and be
    // in a recoverable mid-flight state — NOT `failed`/`cancelled`.
    let (client, handle) = pg.lock().await.connect().await.unwrap();
    let tx_state: String = client
        .query_one(
            "SELECT state FROM file_transactions ORDER BY started_at DESC LIMIT 1",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    drop(client);
    handle.abort();
    assert_ne!(
        tx_state, "failed",
        "cancel mid-copy must NOT manufacture a `failed` terminal transaction"
    );
    assert_ne!(tx_state, "cancelled");
    assert_ne!(
        tx_state, "source_archived",
        "cancel must stop before archive"
    );

    // Simulate app restart: drive recovery to convergence.
    drive_recovery(pg.clone()).await;

    // Recovery must converge the mid-flight transaction to source_archived
    // and the run to `completed`.
    let (client, handle) = pg.lock().await.connect().await.unwrap();
    let (state, _completed_at) = run_state(&client, run_id).await;
    let final_tx_state: String = client
        .query_one(
            "SELECT state FROM file_transactions ORDER BY started_at DESC LIMIT 1",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let final_tx_error: Option<String> = client
        .query_one(
            "SELECT last_error FROM file_transactions ORDER BY started_at DESC LIMIT 1",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    drop(client);
    handle.abort();
    assert_eq!(
        final_tx_state, "source_archived",
        "recovery must converge the mid-copy-cancelled transaction to source_archived, got {final_tx_state} (last_error={final_tx_error:?})"
    );
    assert_eq!(
        state, "completed",
        "run should complete after recovery drives the cancelled transaction forward"
    );
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

/// 2. Cancel before any file transaction is prewritten: the run is left
/// `cancelled` (P0 fix — not `recovery_required`, which is a GUI deadlock
/// with no transaction to recover), with NO transaction row. The frozen
/// plan is intact so the user can re-enter the commit page.
#[tokio::test]
#[ignore]
async fn cancellation_before_prewrite_no_transaction() {
    let (_tmp, pg, run_id, lib_root, _album, _lrid, _aid) = setup_env().await;
    // Cancel immediately; the very first iteration of the album loop sees
    // `cancelled` and breaks before inserting any file_transaction.
    run_commit_with_cancel(pg.clone(), &lib_root, run_id).await;

    let (client, handle) = pg.lock().await.connect().await.unwrap();
    let tx_count: i64 = client
        .query_one("SELECT COUNT(*) FROM file_transactions", &[])
        .await
        .unwrap()
        .get(0);
    let (state, _completed_at) = run_state(&client, run_id).await;
    drop(client);
    handle.abort();
    assert_eq!(tx_count, 0, "no transaction should have been prewritten");
    // P0: with no transaction, the run must be `cancelled` (user-explicit
    // terminal), NOT `recovery_required` (which has no recovery path).
    assert_eq!(
        state, "cancelled",
        "cancel-before-prewrite with no transactions must be `cancelled`, got {state}"
    );
    // The frozen plan is intact, so the run is re-committable.
    let (client, handle) = pg.lock().await.connect().await.unwrap();
    let latest: Option<String> = client
        .query_opt(
            "SELECT id::text FROM import_runs
             WHERE id = $1 AND state = 'cancelled'",
            &[&run_id],
        )
        .await
        .unwrap()
        .map(|r| r.get::<_, String>(0));
    drop(client);
    handle.abort();
    assert!(latest.is_some());
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

/// 3. Recovery continues the original transaction after a mid-staging
/// cancel + simulated restart. (Covered together with #1.)
#[tokio::test]
#[ignore]
async fn recovery_continues_original_transaction() {
    let (_tmp, pg, run_id, lib_root, _album, _lrid, _aid) = setup_env().await;
    // Land a transaction mid-staging via fault, then recover.
    crate::tests::fail_injection::set_fault_point(
        crate::tests::fail_injection::CommitFaultPoint::AfterStagingCopy,
    );
    let cancelled = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(CommitProgress::idle(&run_id.to_string())));
    let _ = commit_service::run_import_commit(
        pg.clone(),
        lib_root.display().to_string(),
        run_id,
        cancelled,
        progress,
    )
    .await;
    crate::tests::fail_injection::clear_fault_point();

    // Capture the original transaction id; recovery must NOT create a second
    // transaction for the same album.
    let original_tx_id: Uuid = {
        let (client, handle) = pg.lock().await.connect().await.unwrap();
        let id: Uuid = client
            .query_one(
                "SELECT id FROM file_transactions ORDER BY started_at LIMIT 1",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        drop(client);
        handle.abort();
        id
    };

    drive_recovery(pg.clone()).await;

    let (client, handle) = pg.lock().await.connect().await.unwrap();
    let tx_count: i64 = client
        .query_one("SELECT COUNT(*) FROM file_transactions", &[])
        .await
        .unwrap()
        .get(0);
    let (state, _completed_at) = run_state(&client, run_id).await;
    let final_tx_id: Uuid = client
        .query_one(
            "SELECT id FROM file_transactions WHERE state = 'source_archived' LIMIT 1",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    drop(client);
    handle.abort();
    assert_eq!(tx_count, 1, "recovery must not create a second transaction");
    assert_eq!(state, "completed");
    assert_eq!(
        final_tx_id, original_tx_id,
        "recovery must continue the original transaction, not create a new one"
    );
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

/// 4. Re-running commit while a transaction is mid-flight surfaces
/// `recovery_required` and does NOT create a second transaction. (Mirrors
/// the existing fail_injection_double_commit_detected test but through the
/// post-fix path.)
#[tokio::test]
#[ignore]
async fn rerun_commit_does_not_create_second_transaction() {
    let (_tmp, pg, run_id, lib_root, _album, _lrid, _aid) = setup_env().await;
    crate::tests::fail_injection::set_fault_point(
        crate::tests::fail_injection::CommitFaultPoint::AfterStagingCopy,
    );
    let cancelled = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(CommitProgress::idle(&run_id.to_string())));
    let _ = commit_service::run_import_commit(
        pg.clone(),
        lib_root.display().to_string(),
        run_id,
        cancelled,
        progress,
    )
    .await;
    crate::tests::fail_injection::clear_fault_point();

    let first_count: i64 = {
        let (client, handle) = pg.lock().await.connect().await.unwrap();
        let n: i64 = client
            .query_one("SELECT COUNT(*) FROM file_transactions", &[])
            .await
            .unwrap()
            .get(0);
        drop(client);
        handle.abort();
        n
    };

    let cancelled2 = Arc::new(AtomicBool::new(false));
    let progress2 = Arc::new(Mutex::new(CommitProgress::idle(&run_id.to_string())));
    let result = commit_service::run_import_commit(
        pg.clone(),
        lib_root.display().to_string(),
        run_id,
        cancelled2,
        progress2,
    )
    .await
    .expect("second commit should return a recovery_required result");
    assert_eq!(result.state, "recovery_required");

    let second_count: i64 = {
        let (client, handle) = pg.lock().await.connect().await.unwrap();
        let n: i64 = client
            .query_one("SELECT COUNT(*) FROM file_transactions", &[])
            .await
            .unwrap()
            .get(0);
        drop(client);
        handle.abort();
        n
    };
    assert_eq!(second_count, first_count);
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

/// 5. failed/cancelled transactions are NOT reported as `recovered=true`.
#[tokio::test]
#[ignore]
async fn failed_cancelled_not_reported_recovered() {
    let (_tmp, mgr) = fresh_db().await;
    let library_root_id = {
        let (client, handle) = mgr.lock().await.connect().await.unwrap();
        let id = ImportRepository::upsert_default_library_root(&client)
            .await
            .unwrap();
        drop(client);
        handle.abort();
        id
    };
    let (run_id, album_id) = {
        let (client, _handle) = mgr.lock().await.connect().await.unwrap();
        let run_id = ImportRepository::create_import_run(&client, "/src", library_root_id)
            .await
            .unwrap();
        let album_id = ImportRepository::insert_import_album(&client, run_id, "/src/a", "album_a")
            .await
            .unwrap();
        (run_id, album_id)
    };

    // Insert a `failed` transaction and recover it — the outcome must be
    // terminal + NOT recovered. Use a distinct album per insert so the
    // unique-active partial index (which excludes failed/cancelled) is not
    // violated by a second terminal insert for the same album.
    let tx_id = Uuid::new_v4();
    {
        let (client, handle) = mgr.lock().await.connect().await.unwrap();
        ImportRepository::insert_file_transaction(
            &client,
            tx_id,
            run_id,
            album_id,
            &TransactionState::Failed,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        drop(client);
        handle.abort();
    }

    let outcome = recover_transaction(mgr.clone(), tx_id).await.unwrap();
    assert_eq!(outcome.final_state, "failed");
    assert!(
        !outcome.recovered,
        "failed transaction must NOT be reported as recovered"
    );
    assert!(outcome.terminal, "failed transaction is terminal");

    // Same for `cancelled`, using a second album so the unique-active index
    // (which excludes cancelled) does not collide.
    let album_id2 = {
        let (client, handle) = mgr.lock().await.connect().await.unwrap();
        let id = ImportRepository::insert_import_album(&client, run_id, "/src/b", "album_b")
            .await
            .unwrap();
        drop(client);
        handle.abort();
        id
    };
    let tx_id2 = Uuid::new_v4();
    {
        let (client, handle) = mgr.lock().await.connect().await.unwrap();
        ImportRepository::insert_file_transaction(
            &client,
            tx_id2,
            run_id,
            album_id2,
            &TransactionState::Cancelled,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        drop(client);
        handle.abort();
    }

    let outcome2 = recover_transaction(mgr.clone(), tx_id2).await.unwrap();
    assert_eq!(outcome2.final_state, "cancelled");
    assert!(!outcome2.recovered);
    assert!(outcome2.terminal);
    let mut m = mgr.lock().await;
    m.shutdown().await.unwrap();
}

/// 6. `recovery_required` is NOT mapped to `completed_with_errors` or any
/// completion overlay — the persisted DB state is the sole source of truth.
#[tokio::test]
#[ignore]
async fn recovery_required_not_mapped_to_completion() {
    let (_tmp, pg, run_id, _lib_root, _album, _lrid, album_id) = setup_env().await;
    // Insert an active transaction so reconcile forces recovery_required.
    let tx_id = Uuid::new_v4();
    {
        let (client, _handle) = pg.lock().await.connect().await.unwrap();
        ImportRepository::insert_file_transaction(
            &client,
            tx_id,
            run_id,
            album_id,
            &TransactionState::Staging,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        // Run must be in a reconcilable state.
        client
            .execute(
                "UPDATE import_runs SET state = 'committing' WHERE id = $1",
                &[&run_id],
            )
            .await
            .unwrap();
    }

    let r = {
        let (client, handle) = pg.lock().await.connect().await.unwrap();
        let r = reconcile_import_run_state(&client, run_id).await.unwrap();
        drop(client);
        handle.abort();
        r
    };
    assert_eq!(r.state, ImportRunState::RecoveryRequired);
    // The persisted state must be recovery_required — never a
    // `completed_with_errors` overlay.
    let (client, handle) = pg.lock().await.connect().await.unwrap();
    let (state, _completed_at) = run_state(&client, run_id).await;
    drop(client);
    handle.abort();
    assert_eq!(state, "recovery_required");
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

/// 7. Empty plan but an active transaction → recovery_required (not completed).
#[tokio::test]
#[ignore]
async fn empty_plan_with_active_tx_routes_to_recovery() {
    let (_tmp, mgr) = fresh_db().await;
    let library_root_id = {
        let (client, handle) = mgr.lock().await.connect().await.unwrap();
        let id = ImportRepository::upsert_default_library_root(&client)
            .await
            .unwrap();
        drop(client);
        handle.abort();
        id
    };
    let (run_id, album_id, plan_id) = {
        let (client, _handle) = mgr.lock().await.connect().await.unwrap();
        let run_id = ImportRepository::create_import_run(&client, "/src", library_root_id)
            .await
            .unwrap();
        let album_id = ImportRepository::insert_import_album(&client, run_id, "/src/a", "album_a")
            .await
            .unwrap();
        let plan_id =
            ImportRepository::create_import_plan(&client, run_id, 1, "2.0", library_root_id)
                .await
                .unwrap();
        (run_id, album_id, plan_id)
    };
    {
        let (client, handle) = mgr.lock().await.connect().await.unwrap();
        let draft = ImportRepository::load_draft_plan(&client, run_id)
            .await
            .unwrap()
            .unwrap();
        let hash = commit_service::compute_plan_hash(&draft).unwrap();
        ImportRepository::set_plan_hash(&client, plan_id, &hash)
            .await
            .unwrap();
        ImportRepository::update_import_plan_state(&client, plan_id, &PlanState::Frozen)
            .await
            .unwrap();
        client
            .execute(
                "UPDATE import_runs SET state = 'committing' WHERE id = $1",
                &[&run_id],
            )
            .await
            .unwrap();
        let tx_id = Uuid::new_v4();
        ImportRepository::insert_file_transaction(
            &client,
            tx_id,
            run_id,
            album_id,
            &TransactionState::Planned,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        drop(client);
        handle.abort();
    }

    let r = {
        let (client, handle) = mgr.lock().await.connect().await.unwrap();
        let r = reconcile_import_run_state(&client, run_id).await.unwrap();
        drop(client);
        handle.abort();
        r
    };
    assert_eq!(r.state, ImportRunState::RecoveryRequired);
    let mut m = mgr.lock().await;
    m.shutdown().await.unwrap();
}

/// 8. Empty plan but a conflict transaction → recovery_required.
#[tokio::test]
#[ignore]
async fn empty_plan_with_conflict_routes_to_recovery() {
    let (_tmp, mgr) = fresh_db().await;
    let library_root_id = {
        let (client, handle) = mgr.lock().await.connect().await.unwrap();
        let id = ImportRepository::upsert_default_library_root(&client)
            .await
            .unwrap();
        drop(client);
        handle.abort();
        id
    };
    let (run_id, album_id, plan_id) = {
        let (client, _handle) = mgr.lock().await.connect().await.unwrap();
        let run_id = ImportRepository::create_import_run(&client, "/src", library_root_id)
            .await
            .unwrap();
        let album_id = ImportRepository::insert_import_album(&client, run_id, "/src/a", "album_a")
            .await
            .unwrap();
        let plan_id =
            ImportRepository::create_import_plan(&client, run_id, 1, "2.0", library_root_id)
                .await
                .unwrap();
        (run_id, album_id, plan_id)
    };
    {
        let (client, handle) = mgr.lock().await.connect().await.unwrap();
        let draft = ImportRepository::load_draft_plan(&client, run_id)
            .await
            .unwrap()
            .unwrap();
        let hash = commit_service::compute_plan_hash(&draft).unwrap();
        ImportRepository::set_plan_hash(&client, plan_id, &hash)
            .await
            .unwrap();
        ImportRepository::update_import_plan_state(&client, plan_id, &PlanState::Frozen)
            .await
            .unwrap();
        client
            .execute(
                "UPDATE import_runs SET state = 'committing' WHERE id = $1",
                &[&run_id],
            )
            .await
            .unwrap();
        let tx_id = Uuid::new_v4();
        ImportRepository::insert_file_transaction(
            &client,
            tx_id,
            run_id,
            album_id,
            &TransactionState::Conflict,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        drop(client);
        handle.abort();
    }

    let r = {
        let (client, handle) = mgr.lock().await.connect().await.unwrap();
        let r = reconcile_import_run_state(&client, run_id).await.unwrap();
        drop(client);
        handle.abort();
        r
    };
    assert_eq!(r.state, ImportRunState::RecoveryRequired);
    let mut m = mgr.lock().await;
    m.shutdown().await.unwrap();
}

/// 9. Empty plan, no transactions → completed (the legitimate empty case).
#[tokio::test]
#[ignore]
async fn empty_plan_no_transactions_completes() {
    let (_tmp, mgr) = fresh_db().await;
    let library_root_id = {
        let (client, handle) = mgr.lock().await.connect().await.unwrap();
        let id = ImportRepository::upsert_default_library_root(&client)
            .await
            .unwrap();
        drop(client);
        handle.abort();
        id
    };
    let (run_id, plan_id) = {
        let (client, _handle) = mgr.lock().await.connect().await.unwrap();
        let run_id = ImportRepository::create_import_run(&client, "/src", library_root_id)
            .await
            .unwrap();
        let plan_id =
            ImportRepository::create_import_plan(&client, run_id, 1, "2.0", library_root_id)
                .await
                .unwrap();
        (run_id, plan_id)
    };
    {
        let (client, handle) = mgr.lock().await.connect().await.unwrap();
        // Compute the real plan hash for the empty plan so
        // validate_and_hash_frozen_plan passes.
        let draft = ImportRepository::load_draft_plan(&client, run_id)
            .await
            .unwrap()
            .unwrap();
        let hash = commit_service::compute_plan_hash(&draft).unwrap();
        ImportRepository::set_plan_hash(&client, plan_id, &hash)
            .await
            .unwrap();
        ImportRepository::update_import_plan_state(&client, plan_id, &PlanState::Frozen)
            .await
            .unwrap();
        client
            .execute(
                "UPDATE import_runs SET state = 'committing' WHERE id = $1",
                &[&run_id],
            )
            .await
            .unwrap();
        drop(client);
        handle.abort();
    }

    let r = {
        let (client, handle) = mgr.lock().await.connect().await.unwrap();
        let r = reconcile_import_run_state(&client, run_id).await.unwrap();
        drop(client);
        handle.abort();
        r
    };
    assert_eq!(r.state, ImportRunState::Completed);
    assert!(r.completed_at.is_some());
    let mut m = mgr.lock().await;
    m.shutdown().await.unwrap();
}

/// 10. Source snapshot path disagrees with import album source_path →
/// recovery surfaces a conflict (not auto-fix). The commit's archive
/// stage calls `validate_snapshot_album_path_identity`, which must reject
/// the mismatch as a conflict — never silently `source_archived`.
#[tokio::test]
#[ignore]
async fn snapshot_path_mismatch_surfaces_conflict() {
    let (_tmp, pg, run_id, lib_root, _album, _lrid, album_id) = setup_env().await;
    // Tamper with the persisted snapshot's source_album_path so it
    // disagrees with import_albums.source_path.
    {
        let (client, handle) = pg.lock().await.connect().await.unwrap();
        client
            .execute(
                "UPDATE source_album_snapshots SET source_album_path = '/different/path' WHERE import_album_id = $1",
                &[&album_id],
            )
            .await
            .unwrap();
        drop(client);
        handle.abort();
    }

    // Run commit; the archive phase must surface a conflict.
    let cancelled = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(CommitProgress::idle(&run_id.to_string())));
    let _ = commit_service::run_import_commit(
        pg.clone(),
        lib_root.display().to_string(),
        run_id,
        cancelled,
        progress,
    )
    .await;

    let (client, handle) = pg.lock().await.connect().await.unwrap();
    let tx_state: String = client
        .query_one(
            "SELECT state FROM file_transactions ORDER BY started_at DESC LIMIT 1",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    drop(client);
    handle.abort();
    // P1 fix: previously this accepted `source_archived` as a valid
    // outcome, which would mask a real validation failure. The identity
    // check must reject the mismatch as a conflict — never silently
    // mark the album source_archived.
    assert_eq!(
        tx_state, "conflict",
        "snapshot path mismatch must surface as conflict, got {tx_state}"
    );
    // The run must NOT be completed silently.
    let (client, handle) = pg.lock().await.connect().await.unwrap();
    let (state, _) = run_state(&client, run_id).await;
    drop(client);
    handle.abort();
    assert_ne!(state, "completed");
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

/// 11. Plan image source_path escapes source_root → commit rejects it
/// and the run does NOT silently complete. This drives the real commit
/// pipeline with a plan whose second image's source_path points outside
/// the source album root, so `validate_plan_image_sources` must reject it.
#[tokio::test]
#[ignore]
async fn plan_image_escape_does_not_complete() {
    let (_tmp, pg, run_id, lib_root, album_path, _lrid, _aid) = setup_env().await;
    // Tamper: rewrite the second plan image's source_path to point at a
    // file outside the source album root. The frozen plan hash was set
    // during setup_env, so this also breaks the plan hash — but the
    // commit pipeline calls validate_plan_image_sources AFTER loading the
    // frozen plan, and a hash mismatch would surface first. To test the
    // escape check in isolation, we recompute and reset the plan hash
    // after the tamper so the only failure is the escape.
    //
    // The outside file is a byte-exact copy of photo2.png so the staging
    // BLAKE3 check passes (otherwise the copy rejects the file before the
    // archive-stage escape check can fire).
    let outside_path = album_path.parent().unwrap().join("outside.png");
    std::fs::write(&outside_path, b"photo two data").unwrap();

    {
        let (client, handle) = pg.lock().await.connect().await.unwrap();
        // Point photo2.png's plan image source_path at the outside file.
        // Find the import image by relative_path (OS-separator-safe),
        // then update its plan image row.
        let img_id: Uuid = client
            .query_one(
                "SELECT ii.id FROM import_images ii
                 JOIN import_albums ia ON ia.id = ii.import_album_id
                 WHERE ia.import_run_id = $1 AND ii.relative_path LIKE $2",
                &[&run_id, &"%/photo2.png".to_string()],
            )
            .await
            .unwrap()
            .get(0);
        let affected = client
            .execute(
                "UPDATE import_plan_images SET source_path = $1
                 WHERE import_image_id = $2",
                &[&outside_path.display().to_string(), &img_id],
            )
            .await
            .unwrap();
        assert_eq!(affected, 1, "UPDATE should land on exactly one plan image");
        // Recompute + reset the frozen plan hash so the tamper is the only
        // inconsistency (otherwise a hash mismatch surfaces first and
        // masks the escape check). The plan is frozen.
        let frozen = ImportRepository::load_frozen_plan(&client, run_id)
            .await
            .unwrap()
            .unwrap();
        let hash = commit_service::compute_plan_hash(&frozen).unwrap();
        // Update the plan row's hash directly.
        client
            .execute(
                "UPDATE import_plans SET plan_hash = $1 WHERE import_run_id = $2 AND state = 'frozen'",
                &[&hash, &run_id],
            )
            .await
            .unwrap();
        drop(client);
        handle.abort();
    }

    let cancelled = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(CommitProgress::idle(&run_id.to_string())));
    let result = commit_service::run_import_commit(
        pg.clone(),
        lib_root.display().to_string(),
        run_id,
        cancelled,
        progress,
    )
    .await
    // The escape surfaces as a failed album result (recovery_required),
    // not a top-level Err — the pipeline isolates per-album failures.
    .expect("commit should return Ok with a failed album, not a top-level Err");
    // Collect the error from album_results + top-level errors and confirm
    // the escape check fired.
    let combined: Vec<String> = result
        .album_results
        .iter()
        .filter_map(|r| r.error.clone())
        .chain(result.errors.iter().cloned())
        .collect();
    let msg = combined.join("; ");
    assert!(
        msg.contains("escapes") || msg.contains("escape"),
        "expected escape error, got: {msg}"
    );
    assert_eq!(result.albums_failed, 1);

    // The run must NOT be completed.
    let (client, handle) = pg.lock().await.connect().await.unwrap();
    let (state, _) = run_state(&client, run_id).await;
    drop(client);
    handle.abort();
    assert_ne!(state, "completed");
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

/// 12. A source album containing a symlink (Unix) or directory junction
/// (Windows) is rejected (snapshot capture fails with an explicit error,
/// not silent hashing).
#[tokio::test]
#[ignore]
async fn source_album_with_symlink_rejected() {
    let tmp = TempDir::new().unwrap();
    let album = tmp.path().join("sym_album");
    std::fs::create_dir_all(&album).unwrap();
    std::fs::write(album.join("real.png"), b"data").unwrap();
    // A real directory outside the album for the link/junction to point at.
    let outside_dir = tmp.path().join("outside_dir");
    std::fs::create_dir_all(&outside_dir).unwrap();
    std::fs::write(outside_dir.join("secret.png"), b"secret").unwrap();

    #[cfg(unix)]
    {
        // File symlink → must be rejected.
        std::os::unix::fs::symlink(outside_dir.join("secret.png"), album.join("link.png")).unwrap();
        let err = crate::services::source_snapshot_service::collect_album_files(&album)
            .expect_err("file symlink must be rejected");
        assert!(
            err.to_string().contains("symlink"),
            "expected symlink rejection, got: {err}"
        );

        // Directory symlink → also rejected.
        let album2 = tmp.path().join("sym_album2");
        std::fs::create_dir_all(&album2).unwrap();
        std::fs::write(album2.join("real.png"), b"data").unwrap();
        std::os::unix::fs::symlink(&outside_dir, album2.join("linkdir")).unwrap();
        let err2 = crate::services::source_snapshot_service::collect_album_files(&album2)
            .expect_err("dir symlink must be rejected");
        assert!(err2.to_string().contains("symlink"));
    }

    #[cfg(windows)]
    {
        // Create a directory junction (reparse point) pointing outside the
        // album. Junctions are created via `mklink /J` (cmd builtin) — they
        // do NOT require the SeCreateSymbolicLinkPrivilege that real
        // symlinks need, so this runs unprivileged.
        let junction = album.join("linkdir");
        let status = std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                &junction.display().to_string(),
                &outside_dir.display().to_string(),
            ])
            .status()
            .expect("failed to run mklink");
        assert!(status.success(), "mklink /J failed: {status}");

        let err = crate::services::source_snapshot_service::collect_album_files(&album)
            .expect_err("directory junction / reparse point must be rejected");
        // On Windows, `mklink /J` junctions are reported as symlinks by
        // `std::fs::FileType::is_symlink()`, so the rejection message names
        // "symlink". Other reparse-point types hit the 0x400 attribute
        // branch and name "reparse point / junction". Either is an
        // acceptable rejection of a special entry.
        assert!(
            err.to_string().contains("symlink")
                || err.to_string().contains("reparse point")
                || err.to_string().contains("junction"),
            "expected reparse-point rejection, got: {err}"
        );
    }

    #[cfg(not(any(unix, windows)))]
    {
        // No special-file test on other platforms; just confirm the happy
        // path so the test is not a no-op.
        let _ = crate::services::source_snapshot_service::collect_album_files(&album)
            .expect("regular album must snapshot cleanly");
    }
}

/// 13. Repeated reconcile on an already-completed run returns the same
/// persisted `completed_at` (Phase 6: never a transient value).
#[tokio::test]
#[ignore]
async fn repeated_reconcile_completed_at_stable() {
    let (_tmp, pg, run_id, lib_root, _album, _lrid, _aid) = setup_env().await;
    // Complete the run via the real commit pipeline.
    let cancelled = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(CommitProgress::idle(&run_id.to_string())));
    let _ = commit_service::run_import_commit(
        pg.clone(),
        lib_root.display().to_string(),
        run_id,
        cancelled,
        progress,
    )
    .await
    .unwrap();

    let (client, handle) = pg.lock().await.connect().await.unwrap();
    let r1 = reconcile_import_run_state(&client, run_id).await.unwrap();
    let r2 = reconcile_import_run_state(&client, run_id).await.unwrap();
    drop(client);
    handle.abort();
    assert_eq!(r1.state, ImportRunState::Completed);
    assert_eq!(r2.state, ImportRunState::Completed);
    assert!(!r2.changed, "second reconcile must be a no-op");
    assert_eq!(
        r1.completed_at, r2.completed_at,
        "completed_at must be the persisted value, not a transient now()"
    );
    assert!(r1.completed_at.is_some());
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

/// 14. Two consecutive recovery passes converge (idempotent).
#[tokio::test]
#[ignore]
async fn two_consecutive_recovery_passes_converge() {
    let (_tmp, pg, run_id, lib_root, _album, _lrid, _aid) = setup_env().await;
    crate::tests::fail_injection::set_fault_point(
        crate::tests::fail_injection::CommitFaultPoint::AfterStagingCopy,
    );
    let cancelled = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(CommitProgress::idle(&run_id.to_string())));
    let _ = commit_service::run_import_commit(
        pg.clone(),
        lib_root.display().to_string(),
        run_id,
        cancelled,
        progress,
    )
    .await;
    crate::tests::fail_injection::clear_fault_point();

    drive_recovery(pg.clone()).await;
    // Second pass must be a no-op (no new transactions, run stays completed).
    drive_recovery(pg.clone()).await;

    let (client, handle) = pg.lock().await.connect().await.unwrap();
    let tx_count: i64 = client
        .query_one("SELECT COUNT(*) FROM file_transactions", &[])
        .await
        .unwrap()
        .get(0);
    let (state, _completed_at) = run_state(&client, run_id).await;
    drop(client);
    handle.abort();
    assert_eq!(
        tx_count, 1,
        "second recovery pass must not add transactions"
    );
    assert_eq!(state, "completed");
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

/// 15. Conflict scenarios do NOT delete source files (defense-in-depth:
/// the source album dir is preserved even when recovery surfaces a
/// conflict).
#[tokio::test]
#[ignore]
async fn conflict_does_not_delete_source() {
    let (_tmp, pg, run_id, lib_root, album_path, _lrid, _aid) = setup_env().await;
    // Land in library_committed, then make both source + archive exist to
    // trigger the both-present conflict branch.
    crate::tests::fail_injection::set_fault_point(
        crate::tests::fail_injection::CommitFaultPoint::BeforeSourceArchive,
    );
    let cancelled = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(CommitProgress::idle(&run_id.to_string())));
    let _ = commit_service::run_import_commit(
        pg.clone(),
        lib_root.display().to_string(),
        run_id,
        cancelled,
        progress,
    )
    .await;
    crate::tests::fail_injection::clear_fault_point();

    // Manually perform the archive rename, then recreate the source dir
    // (simulating external restore) so both exist.
    let tx_id: Uuid = {
        let (client, handle) = pg.lock().await.connect().await.unwrap();
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
    let archive_dir = album_path
        .parent()
        .unwrap()
        .join(".imagedb-processed")
        .join(tx_id.to_string())
        .join("album_a");
    std::fs::create_dir_all(archive_dir.parent().unwrap()).unwrap();
    std::fs::rename(&album_path, &archive_dir).unwrap();
    // Recreate source with full content (so snapshot verifies).
    std::fs::create_dir_all(&album_path).unwrap();
    std::fs::write(album_path.join("photo1.png"), b"photo one data").unwrap();
    std::fs::write(album_path.join("photo2.png"), b"photo two data").unwrap();
    std::fs::write(album_path.join("description.txt"), b"album notes").unwrap();

    drive_recovery(pg.clone()).await;

    // Both source and archive must still exist (no silent deletion).
    assert!(
        album_path.exists(),
        "source dir must NOT be deleted on conflict"
    );
    assert!(
        archive_dir.exists(),
        "archive dir must NOT be deleted on conflict"
    );
    let (client, handle) = pg.lock().await.connect().await.unwrap();
    let tx_state: String = client
        .query_one(
            "SELECT state FROM file_transactions ORDER BY started_at DESC LIMIT 1",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    drop(client);
    handle.abort();
    assert_eq!(tx_state, "conflict");
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

async fn fresh_db() -> (TempDir, Arc<Mutex<PostgresManager>>) {
    if std::env::var("IMAGEDB_POSTGRES_BIN")
        .unwrap_or_default()
        .is_empty()
    {
        panic!("IMAGEDB_POSTGRES_BIN must be set for cancellation_recovery real-db tests");
    }
    let tmp = TempDir::new().unwrap();
    let app_data = tmp.path().join("app_data");
    std::fs::create_dir_all(&app_data).unwrap();
    let mut manager = PostgresManager::new(&app_data);
    assert!(manager.binaries_available(), "binaries missing");
    let probe = manager.initialize().await.unwrap();
    assert!(probe.connection_ok, "diagnostics: {:?}", probe.diagnostics);
    let (mut client, handle) = manager.connect().await.unwrap();
    MigrationRunner::run_pending(&mut client).await.unwrap();
    drop(client);
    handle.abort();
    (tmp, Arc::new(Mutex::new(manager)))
}
