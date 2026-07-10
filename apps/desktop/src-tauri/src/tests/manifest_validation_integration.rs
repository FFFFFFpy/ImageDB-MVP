//! Real PostgreSQL + filesystem integration tests for strict manifest
//! validation (batch 5 of the core fix): the published manifest is hashed
//! over the raw on-disk bytes, never re-serialized, and idempotency /
//! recovery verify every identity field inside the manifest.
//!
//! Each test runs the full commit pipeline to establish a valid
//! source_archived transaction, then mutates either the on-disk manifest
//! or a DB row and calls [`verify_complete_evidence`] directly to assert
//! that the tampering is detected as a conflict rather than silently
//! accepted.
//!
//! Invocation:
//!   IMAGEDB_POSTGRES_BIN=/path/to/pgsql/bin cargo test --manifest-path \
//!       apps/desktop/src-tauri/Cargo.toml --features real-db-tests --lib \
//!       manifest_validation_ -- --ignored --test-threads=1
#![cfg(test)]
#![cfg(feature = "real-db-tests")]
use crate::domain::import_state::{DecodeState, ImportImageState, ImportRunState};
use crate::domain::state_machine::PlanState;
use crate::infrastructure::postgres::{MigrationRunner, PostgresManager};
use crate::repositories::import_repository::{ImportRepository, NewImportImage};
use crate::services::commit_service::{
    read_manifest_with_hash, validate_and_hash_frozen_plan, verify_complete_evidence,
    IdempotencyVerdict,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::Mutex;

/// Shared fixture: spin up a fresh PostgresManager, run migrations, and
/// persist the source/library roots. The manager is returned (moved) so
/// tests can drive commit + later reconnect through it.
struct Fixture {
    _tmp: TempDir,
    app_data: PathBuf,
    source_root: PathBuf,
    library_root: PathBuf,
    album_path: PathBuf,
    manager: PostgresManager,
    library_root_id: uuid::Uuid,
}

async fn new_fixture() -> Fixture {
    if std::env::var("IMAGEDB_POSTGRES_BIN")
        .unwrap_or_default()
        .is_empty()
    {
        panic!("IMAGEDB_POSTGRES_BIN must be set for manifest-validation tests");
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
    let (mut client, handle) = manager.connect().await.unwrap();
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

    drop(client);
    handle.abort();

    Fixture {
        _tmp: tmp,
        app_data,
        source_root,
        library_root,
        album_path,
        manager,
        library_root_id,
    }
}

/// Seed the import run + album + images + source snapshot + frozen plan so
/// the commit pipeline has a valid commit set to run against. Returns
/// (import_run_id, import_album_id, plan_id, plan_hash, images_blake3_map).
#[allow(clippy::type_complexity)]
async fn seed_and_freeze(
    manager: &PostgresManager,
    source_root: &Path,
    album_path: &Path,
    library_root_id: uuid::Uuid,
) -> (
    uuid::Uuid,
    uuid::Uuid,
    uuid::Uuid,
    Vec<u8>,
    Vec<(String, Vec<u8>)>,
) {
    let (client, handle) = manager.connect().await.unwrap();
    let run_id = ImportRepository::create_import_run(
        &client,
        &source_root.display().to_string(),
        library_root_id,
    )
    .await
    .unwrap();
    let album_id = ImportRepository::insert_import_album(
        &client,
        run_id,
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
                blake3: Some(b3.clone()),
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
    crate::services::source_snapshot_service::capture_source_album_snapshot(
        &client, run_id, album_id, album_path,
    )
    .await
    .unwrap();

    let plan_id = ImportRepository::create_import_plan(&client, run_id, 1, "2.0", library_root_id)
        .await
        .unwrap();
    let plan_album_id =
        ImportRepository::insert_plan_album(&client, plan_id, album_id, "album_a", 2)
            .await
            .unwrap();
    for (n, b3) in [
        ("photo1.png", img1_blake3.clone()),
        ("photo2.png", img2_blake3.clone()),
    ] {
        let img_id: uuid::Uuid = client
            .query_one(
                "SELECT ii.id FROM import_images ii JOIN import_albums ia ON ia.id = ii.import_album_id
                 WHERE ia.import_run_id = $1 AND ii.relative_path LIKE $2",
                &[&run_id, &format!("%/{n}")],
            )
            .await
            .unwrap()
            .get(0);
        ImportRepository::insert_plan_image(
            &client,
            plan_album_id,
            img_id,
            &album_path.join(n).display().to_string(),
            &format!("album_a/{n}"),
            n,
            14,
            &b3,
            Some(10),
            Some(10),
            Some("png"),
        )
        .await
        .unwrap();
    }
    let frozen = ImportRepository::load_draft_plan(&client, run_id)
        .await
        .unwrap()
        .unwrap();
    let plan_hash = crate::services::commit_service::compute_plan_hash(&frozen).unwrap();
    ImportRepository::set_plan_hash(&client, plan_id, &plan_hash)
        .await
        .unwrap();
    ImportRepository::update_import_plan_state(&client, plan_id, &PlanState::Frozen)
        .await
        .unwrap();
    ImportRepository::update_import_run_state(&client, run_id, &ImportRunState::ReadyToCommit)
        .await
        .unwrap();
    drop(client);
    handle.abort();

    (
        run_id,
        album_id,
        plan_id,
        plan_hash,
        vec![
            ("photo1.png".to_string(), img1_blake3),
            ("photo2.png".to_string(), img2_blake3),
        ],
    )
}

/// Run the full commit pipeline against the fixture and assert it completes.
/// Returns the same fixture so the temp dirs stay alive for follow-up checks.
async fn run_commit(fx: Fixture, run_id: uuid::Uuid) -> Fixture {
    let Fixture {
        _tmp,
        app_data,
        source_root,
        library_root,
        album_path,
        manager,
        library_root_id,
    } = fx;
    let pg = Arc::new(Mutex::new(manager));
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(
        crate::domain::import_state::CommitProgress::idle(&run_id.to_string()),
    ));
    let ok = crate::services::commit_service::run_import_commit(
        pg.clone(),
        library_root.display().to_string(),
        run_id,
        cancelled,
        progress,
    )
    .await
    .unwrap();
    assert_eq!(
        ok.state, "completed",
        "commit pipeline must complete; got {:?}",
        ok.errors
    );
    let manager = match Arc::try_unwrap(pg) {
        Ok(mutex) => mutex.into_inner(),
        Err(_) => panic!("no other Arc references should remain after commit"),
    };
    Fixture {
        _tmp,
        app_data,
        source_root,
        library_root,
        album_path,
        manager,
        library_root_id,
    }
}

/// Load the frozen plan + validated plan_hash and the latest file transaction
/// for the album so `verify_complete_evidence` can be invoked directly.
async fn load_evidence_inputs(
    client: &tokio_postgres::Client,
    run_id: uuid::Uuid,
    album_id: uuid::Uuid,
    library_root_id: uuid::Uuid,
) -> (
    crate::repositories::import_repository::FrozenPlanRow,
    Vec<u8>,
    crate::repositories::import_repository::FileTransactionFullRow,
    Vec<crate::repositories::import_repository::PlanImageRow>,
) {
    let frozen = ImportRepository::load_frozen_plan(client, run_id)
        .await
        .unwrap()
        .expect("frozen plan present");
    let plan_hash = validate_and_hash_frozen_plan(&frozen, library_root_id).unwrap();
    let tx = ImportRepository::find_latest_file_transaction(client, album_id)
        .await
        .unwrap()
        .expect("transaction present");
    let (_, images) = frozen
        .albums
        .iter()
        .find(|(a, _)| a.import_album_id == album_id)
        .expect("album in frozen plan")
        .clone();
    (frozen, plan_hash, tx, images)
}

/// Baseline: after a clean commit, verify_complete_evidence returns
/// AlreadyCommitted using the raw-byte manifest hash. Proves the new
/// hashing path is consistent with what commit persisted.
#[tokio::test]
#[ignore]
async fn manifest_validation_baseline_already_committed() {
    let fx = new_fixture().await;
    let (run_id, album_id, _plan_id, _plan_hash, _imgs) = seed_and_freeze(
        &fx.manager,
        &fx.source_root,
        &fx.album_path,
        fx.library_root_id,
    )
    .await;
    let mut fx = run_commit(fx, run_id).await;

    let (client, handle) = fx.manager.connect().await.unwrap();
    let (frozen, plan_hash, tx, images) =
        load_evidence_inputs(&client, run_id, album_id, fx.library_root_id).await;

    let verdict = verify_complete_evidence(
        &client,
        &fx.library_root,
        fx.library_root_id,
        &tx,
        frozen.plan_id,
        &plan_hash,
        "album_a",
        &images,
    )
    .await
    .unwrap();
    assert!(
        matches!(verdict, IdempotencyVerdict::AlreadyCommitted),
        "expected AlreadyCommitted, got {:?}",
        conflict_msg(&verdict)
    );
    drop(client);
    handle.abort();
    fx.manager.shutdown().await.unwrap();
}

/// Tamper the manifest bytes but keep it valid JSON (add a trailing
/// whitespace comment via an extra field). The manifest still parses, but
/// the raw-byte BLAKE3 no longer matches file_transactions.manifest_hash,
/// so idempotency must surface a manifest_hash mismatch.
#[tokio::test]
#[ignore]
async fn manifest_validation_raw_bytes_tamper_detected() {
    let fx = new_fixture().await;
    let (run_id, album_id, _plan_id, _plan_hash, _imgs) = seed_and_freeze(
        &fx.manager,
        &fx.source_root,
        &fx.album_path,
        fx.library_root_id,
    )
    .await;
    let mut fx = run_commit(fx, run_id).await;

    let publish_dir = fx.library_root.join("Albums").join("album_a");
    let manifest_path = publish_dir.join(".imagedb").join(".imagedb-manifest.json");
    let original = std::fs::read(&manifest_path).unwrap();
    let mut tampered: serde_json::Value = serde_json::from_slice(&original).unwrap();
    // Add a new JSON field — still valid JSON, still parseable into the
    // AlbumManifest struct (serde ignores unknown fields by default),
    // but the raw bytes differ.
    tampered
        .as_object_mut()
        .unwrap()
        .insert("injected".to_string(), serde_json::json!("tamper"));
    let rewritten = serde_json::to_vec_pretty(&tampered).unwrap();
    assert_ne!(original, rewritten, "tamper must change the bytes");
    std::fs::write(&manifest_path, &rewritten).unwrap();

    let (client, handle) = fx.manager.connect().await.unwrap();
    let (frozen, plan_hash, tx, images) =
        load_evidence_inputs(&client, run_id, album_id, fx.library_root_id).await;

    let verdict = verify_complete_evidence(
        &client,
        &fx.library_root,
        fx.library_root_id,
        &tx,
        frozen.plan_id,
        &plan_hash,
        "album_a",
        &images,
    )
    .await
    .unwrap();
    let msg = conflict_msg(&verdict);
    assert!(
        matches!(verdict, IdempotencyVerdict::Conflict(_)),
        "expected Conflict on raw-byte tamper, got {:?}",
        msg
    );
    assert!(
        msg.contains("manifest_hash"),
        "expected manifest_hash in conflict message, got: {msg}"
    );
    drop(client);
    handle.abort();
    fx.manager.shutdown().await.unwrap();
}

/// Tamper a manifest identity field (plan_id) without touching the file
/// contents. verify_complete_evidence must surface a plan_id mismatch.
#[tokio::test]
#[ignore]
async fn manifest_validation_plan_id_tamper_detected() {
    let fx = new_fixture().await;
    let (run_id, album_id, _plan_id, _plan_hash, _imgs) = seed_and_freeze(
        &fx.manager,
        &fx.source_root,
        &fx.album_path,
        fx.library_root_id,
    )
    .await;
    let mut fx = run_commit(fx, run_id).await;

    let publish_dir = fx.library_root.join("Albums").join("album_a");
    let manifest_path = publish_dir.join(".imagedb").join(".imagedb-manifest.json");
    let original = std::fs::read(&manifest_path).unwrap();
    let mut tampered: serde_json::Value = serde_json::from_slice(&original).unwrap();
    tampered.as_object_mut().unwrap().insert(
        "plan_id".to_string(),
        serde_json::json!(uuid::Uuid::new_v4().to_string()),
    );
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&tampered).unwrap(),
    )
    .unwrap();
    // Also update file_transactions.manifest_hash to the tampered raw bytes
    // so the hash check doesn't fire first — we want to prove the plan_id
    // field check itself catches the tamper.
    let new_hash = blake3::hash(&std::fs::read(&manifest_path).unwrap())
        .as_bytes()
        .to_vec();
    let (client, handle) = fx.manager.connect().await.unwrap();
    client
        .execute(
            "UPDATE file_transactions SET manifest_hash = $1 WHERE import_album_id = $2",
            &[&new_hash, &album_id],
        )
        .await
        .unwrap();
    client
        .execute(
            "UPDATE library_albums SET manifest_hash = $1 WHERE relative_path = 'album_a'",
            &[&new_hash],
        )
        .await
        .unwrap();
    let (frozen, plan_hash, tx, images) =
        load_evidence_inputs(&client, run_id, album_id, fx.library_root_id).await;

    let verdict = verify_complete_evidence(
        &client,
        &fx.library_root,
        fx.library_root_id,
        &tx,
        frozen.plan_id,
        &plan_hash,
        "album_a",
        &images,
    )
    .await
    .unwrap();
    let msg = conflict_msg(&verdict);
    assert!(
        matches!(verdict, IdempotencyVerdict::Conflict(_)),
        "expected Conflict on plan_id tamper, got {:?}",
        msg
    );
    assert!(
        msg.contains("plan_id"),
        "expected plan_id in conflict message, got: {msg}"
    );
    drop(client);
    handle.abort();
    fx.manager.shutdown().await.unwrap();
}

/// Tamper the manifest image_count so it no longer matches images.len().
/// The plan set is unchanged on disk, so this must surface as an
/// image_count mismatch conflict.
#[tokio::test]
#[ignore]
async fn manifest_validation_image_count_tamper_detected() {
    let fx = new_fixture().await;
    let (run_id, album_id, _plan_id, _plan_hash, _imgs) = seed_and_freeze(
        &fx.manager,
        &fx.source_root,
        &fx.album_path,
        fx.library_root_id,
    )
    .await;
    let mut fx = run_commit(fx, run_id).await;

    let publish_dir = fx.library_root.join("Albums").join("album_a");
    let manifest_path = publish_dir.join(".imagedb").join(".imagedb-manifest.json");
    let mut tampered: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    tampered
        .as_object_mut()
        .unwrap()
        .insert("image_count".to_string(), serde_json::json!(99));
    let new_bytes = serde_json::to_vec_pretty(&tampered).unwrap();
    std::fs::write(&manifest_path, &new_bytes).unwrap();
    let new_hash = blake3::hash(&new_bytes).as_bytes().to_vec();

    let (client, handle) = fx.manager.connect().await.unwrap();
    client
        .execute(
            "UPDATE file_transactions SET manifest_hash = $1 WHERE import_album_id = $2",
            &[&new_hash, &album_id],
        )
        .await
        .unwrap();
    client
        .execute(
            "UPDATE library_albums SET manifest_hash = $1 WHERE relative_path = 'album_a'",
            &[&new_hash],
        )
        .await
        .unwrap();
    let (frozen, plan_hash, tx, images) =
        load_evidence_inputs(&client, run_id, album_id, fx.library_root_id).await;

    let verdict = verify_complete_evidence(
        &client,
        &fx.library_root,
        fx.library_root_id,
        &tx,
        frozen.plan_id,
        &plan_hash,
        "album_a",
        &images,
    )
    .await
    .unwrap();
    let msg = conflict_msg(&verdict);
    assert!(
        matches!(verdict, IdempotencyVerdict::Conflict(_)),
        "expected Conflict on image_count tamper, got {:?}",
        msg
    );
    assert!(
        msg.contains("image_count") || msg.contains("images array length"),
        "expected image_count/length in conflict message, got: {msg}"
    );
    drop(client);
    handle.abort();
    fx.manager.shutdown().await.unwrap();
}

/// Drop an extra file into the published album directory after commit.
/// verify_complete_evidence must reject the extra file as a conflict
/// (unknown content inside the formal dir).
#[tokio::test]
#[ignore]
async fn manifest_validation_extra_published_file_detected() {
    let fx = new_fixture().await;
    let (run_id, album_id, _plan_id, _plan_hash, _imgs) = seed_and_freeze(
        &fx.manager,
        &fx.source_root,
        &fx.album_path,
        fx.library_root_id,
    )
    .await;
    let mut fx = run_commit(fx, run_id).await;

    let publish_dir = fx.library_root.join("Albums").join("album_a");
    std::fs::write(publish_dir.join("rogue.png"), b"rogue").unwrap();

    let (client, handle) = fx.manager.connect().await.unwrap();
    let (frozen, plan_hash, tx, images) =
        load_evidence_inputs(&client, run_id, album_id, fx.library_root_id).await;

    let verdict = verify_complete_evidence(
        &client,
        &fx.library_root,
        fx.library_root_id,
        &tx,
        frozen.plan_id,
        &plan_hash,
        "album_a",
        &images,
    )
    .await
    .unwrap();
    let msg = conflict_msg(&verdict);
    assert!(
        matches!(verdict, IdempotencyVerdict::Conflict(_)),
        "expected Conflict on extra published file, got {:?}",
        msg
    );
    assert!(
        msg.contains("extra") || msg.contains("rogue"),
        "expected extra-file mention in conflict message, got: {msg}"
    );
    drop(client);
    handle.abort();
    fx.manager.shutdown().await.unwrap();
}

/// Drop an extra file_operation row pointing at the same transaction with
/// a target not in the plan. verify_complete_evidence must reject the
/// extra op as a conflict.
#[tokio::test]
#[ignore]
async fn manifest_validation_extra_file_operation_detected() {
    let fx = new_fixture().await;
    let (run_id, album_id, _plan_id, _plan_hash, _imgs) = seed_and_freeze(
        &fx.manager,
        &fx.source_root,
        &fx.album_path,
        fx.library_root_id,
    )
    .await;
    let mut fx = run_commit(fx, run_id).await;

    let (client, handle) = fx.manager.connect().await.unwrap();
    let tx_id: uuid::Uuid = client
        .query_one(
            "SELECT id FROM file_transactions WHERE import_album_id = $1",
            &[&album_id],
        )
        .await
        .unwrap()
        .get(0);
    let bogus_target = fx
        .library_root
        .join("Albums")
        .join("album_a")
        .join("bogus.png");
    client
        .execute(
            "INSERT INTO file_operations \
             (id, transaction_id, source_path, staging_path, target_path, \
              expected_size, expected_blake3, state) \
             VALUES ($1, $2, '/src/x', '/src/x', $3, 0, $4, 'verified')",
            &[
                &uuid::Uuid::new_v4(),
                &tx_id,
                &bogus_target.display().to_string(),
                &vec![0u8; 32],
            ],
        )
        .await
        .unwrap();

    let (frozen, plan_hash, tx, images) =
        load_evidence_inputs(&client, run_id, album_id, fx.library_root_id).await;
    let verdict = verify_complete_evidence(
        &client,
        &fx.library_root,
        fx.library_root_id,
        &tx,
        frozen.plan_id,
        &plan_hash,
        "album_a",
        &images,
    )
    .await
    .unwrap();
    let msg = conflict_msg(&verdict);
    assert!(
        matches!(verdict, IdempotencyVerdict::Conflict(_)),
        "expected Conflict on extra file_operation, got {:?}",
        msg
    );
    assert!(
        msg.contains("file_operation") || msg.contains("file_operations count"),
        "expected file_operation mention in conflict message, got: {msg}"
    );
    drop(client);
    handle.abort();
    fx.manager.shutdown().await.unwrap();
}

/// Recovery must read the raw-byte manifest hash too: after a successful
/// commit, tamper the manifest bytes (keep JSON valid) and call
/// recover_transaction. It must surface a conflict rather than re-commit
/// the DB with a bogus manifest.
#[tokio::test]
#[ignore]
async fn manifest_validation_recovery_raw_bytes_tamper_detected() {
    let fx = new_fixture().await;
    let (run_id, album_id, _plan_id, _plan_hash, _imgs) = seed_and_freeze(
        &fx.manager,
        &fx.source_root,
        &fx.album_path,
        fx.library_root_id,
    )
    .await;
    let fx = run_commit(fx, run_id).await;

    let publish_dir = fx.library_root.join("Albums").join("album_a");
    let manifest_path = publish_dir.join(".imagedb").join(".imagedb-manifest.json");
    let original = std::fs::read(&manifest_path).unwrap();
    let mut tampered: serde_json::Value = serde_json::from_slice(&original).unwrap();
    tampered
        .as_object_mut()
        .unwrap()
        .insert("injected".to_string(), serde_json::json!("tamper"));
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&tampered).unwrap(),
    )
    .unwrap();

    let (client, handle) = fx.manager.connect().await.unwrap();
    let tx_id: uuid::Uuid = client
        .query_one(
            "SELECT id FROM file_transactions WHERE import_album_id = $1",
            &[&album_id],
        )
        .await
        .unwrap()
        .get(0);
    // Pull the transaction back to `published` so recovery has a non-terminal
    // state to drive — source_archived would short-circuit the resume path
    // before the manifest is re-read.
    client
        .execute(
            "UPDATE file_transactions SET state = 'published' WHERE id = $1",
            &[&tx_id],
        )
        .await
        .unwrap();
    client
        .execute(
            "UPDATE import_runs SET state = 'recovery_required' WHERE id = $1",
            &[&run_id],
        )
        .await
        .unwrap();
    drop(client);
    handle.abort();

    let pg = Arc::new(Mutex::new(fx.manager));
    let outcome = crate::services::recovery_service::recover_transaction(pg.clone(), tx_id)
        .await
        .unwrap();
    assert!(
        outcome.final_state == "conflict" || !outcome.recovered,
        "expected recovery to detect tamper; got state={} recovered={} msg={}",
        outcome.final_state,
        outcome.recovered,
        outcome.message
    );
    assert!(
        outcome.message.to_lowercase().contains("manifest")
            || outcome.message.to_lowercase().contains("hash"),
        "expected manifest-related conflict message, got: {}",
        outcome.message
    );
    pg.lock().await.shutdown().await.unwrap();
}

/// Recovery must validate manifest identity and image metadata after the
/// raw hash check too. If an attacker rewrites the manifest and also updates
/// file_transactions.manifest_hash, recovery must still reject the semantic
/// mismatch before DB commit.
#[tokio::test]
#[ignore]
async fn manifest_validation_recovery_image_count_tamper_detected() {
    let fx = new_fixture().await;
    let (run_id, album_id, _plan_id, _plan_hash, _imgs) = seed_and_freeze(
        &fx.manager,
        &fx.source_root,
        &fx.album_path,
        fx.library_root_id,
    )
    .await;
    let fx = run_commit(fx, run_id).await;

    let publish_dir = fx.library_root.join("Albums").join("album_a");
    let manifest_path = publish_dir.join(".imagedb").join(".imagedb-manifest.json");
    let mut tampered: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    tampered
        .as_object_mut()
        .unwrap()
        .insert("image_count".to_string(), serde_json::json!(99));
    let new_bytes = serde_json::to_vec_pretty(&tampered).unwrap();
    std::fs::write(&manifest_path, &new_bytes).unwrap();
    let new_hash = blake3::hash(&new_bytes).as_bytes().to_vec();

    let (client, handle) = fx.manager.connect().await.unwrap();
    let tx_id: uuid::Uuid = client
        .query_one(
            "SELECT id FROM file_transactions WHERE import_album_id = $1",
            &[&album_id],
        )
        .await
        .unwrap()
        .get(0);
    client
        .execute(
            "UPDATE file_transactions SET state = 'published', manifest_hash = $1 WHERE id = $2",
            &[&new_hash, &tx_id],
        )
        .await
        .unwrap();
    client
        .execute(
            "UPDATE import_runs SET state = 'recovery_required' WHERE id = $1",
            &[&run_id],
        )
        .await
        .unwrap();
    drop(client);
    handle.abort();

    let pg = Arc::new(Mutex::new(fx.manager));
    let outcome = crate::services::recovery_service::recover_transaction(pg.clone(), tx_id)
        .await
        .unwrap();
    assert_eq!(outcome.final_state, "conflict");
    assert!(
        outcome.message.contains("image_count") || outcome.message.contains("images array"),
        "expected manifest image_count conflict, got: {}",
        outcome.message
    );
    pg.lock().await.shutdown().await.unwrap();
}

/// read_manifest_with_hash must hash the exact on-disk bytes, not a
/// re-serialization. Two files that parse to the same AlbumManifest but
/// differ by whitespace must produce different hashes.
#[tokio::test]
#[ignore]
async fn manifest_validation_raw_byte_hash_differs_from_reserialization() {
    let fx = new_fixture().await;
    let (run_id, _album_id, _plan_id, _plan_hash, _imgs) = seed_and_freeze(
        &fx.manager,
        &fx.source_root,
        &fx.album_path,
        fx.library_root_id,
    )
    .await;
    let mut fx = run_commit(fx, run_id).await;

    let publish_dir = fx.library_root.join("Albums").join("album_a");
    let (manifest, raw_hash) = read_manifest_with_hash(&publish_dir).unwrap();

    let reserialized = serde_json::to_string_pretty(&manifest).unwrap();
    let reserialized_hash = blake3::hash(reserialized.as_bytes()).as_bytes().to_vec();
    let original_bytes =
        std::fs::read(publish_dir.join(".imagedb").join(".imagedb-manifest.json")).unwrap();
    assert_eq!(
        raw_hash,
        blake3::hash(&original_bytes).as_bytes().to_vec(),
        "raw hash must equal hash of on-disk bytes"
    );
    // The reserialized form may (or may not) equal the on-disk bytes; the
    // safety property is that verify_complete_evidence uses the raw hash,
    // not the reserialized one.
    if original_bytes != reserialized.as_bytes() {
        assert_ne!(
            raw_hash, reserialized_hash,
            "when bytes differ, raw vs re-serialized hashes must differ"
        );
    }
    fx.manager.shutdown().await.unwrap();
}

fn conflict_msg(v: &IdempotencyVerdict) -> String {
    match v {
        IdempotencyVerdict::AlreadyCommitted => "AlreadyCommitted".to_string(),
        IdempotencyVerdict::Conflict(m) => format!("Conflict({m})"),
        IdempotencyVerdict::Resume { transaction_id } => {
            format!("Resume({transaction_id})")
        }
    }
}
