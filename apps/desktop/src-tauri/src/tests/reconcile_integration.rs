//! Real PostgreSQL integration tests for `reconcile_import_run_state`.
//!
//! Verifies that the parent `import_runs` row always reflects the union of
//! its child `file_transactions` rows against the frozen plan's album set,
//! regardless of whether the caller was the commit pipeline or the
//! recovery service.
//!
//! Invocation:
//!   IMAGEDB_POSTGRES_BIN=/path/to/pgsql/bin cargo test --manifest-path \
//!       apps/desktop/src-tauri/Cargo.toml --features real-db-tests --lib \
//!       real_reconcile_ -- --ignored --test-threads=1
#![cfg(test)]
#![cfg(feature = "real-db-tests")]
use crate::domain::import_state::{DecodeState, ImportImageState, ImportRunState};
use crate::domain::state_machine::{PlanState, TransactionState};
use crate::infrastructure::postgres::{MigrationRunner, PostgresManager};
use crate::repositories::import_repository::{ImportRepository, NewImportImage};
use crate::services::recovery_service::reconcile_import_run_state;
use std::sync::Arc;
use tempfile::TempDir;
use uuid::Uuid;

async fn fresh_db() -> (TempDir, Arc<tokio::sync::Mutex<PostgresManager>>) {
    if std::env::var("IMAGEDB_POSTGRES_BIN")
        .unwrap_or_default()
        .is_empty()
    {
        panic!("IMAGEDB_POSTGRES_BIN must be set for reconcile real-db tests");
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
    (tmp, Arc::new(tokio::sync::Mutex::new(manager)))
}

struct SeededRun {
    run_id: Uuid,
    _library_root_id: Uuid,
    album_id: Uuid,
    _plan_id: Uuid,
    _plan_album_id: Uuid,
    _import_image_id: Uuid,
}

/// Build a run with one frozen plan containing one album + one image,
/// ready for transaction-level test setup.
async fn seed_run_with_frozen_plan(client: &tokio_postgres::Client) -> SeededRun {
    let library_root_id = ImportRepository::upsert_default_library_root(client)
        .await
        .unwrap();
    let run_id = ImportRepository::create_import_run(client, "/src", library_root_id)
        .await
        .unwrap();
    let album_id = ImportRepository::insert_import_album(client, run_id, "/src/a", "album_a")
        .await
        .unwrap();
    let import_image_id = ImportRepository::insert_import_image(
        client,
        NewImportImage {
            album_id,
            source_path: "/src/a/p.png".to_string(),
            relative_path: "album_a/p.png".to_string(),
            file_size: 7,
            modified_at: None,
            width: Some(1),
            height: Some(1),
            format: Some("png".to_string()),
            decode_state: DecodeState::Decoded,
            blake3: Some(vec![0x11; 32]),
            pixel_hash: Some(vec![1; 32]),
            block_hash_16: Some(vec![1; 32]),
            double_gradient_hash_32: Some(vec![1; 68]),
            fingerprint_version: Some("2".to_string()),
            state: ImportImageState::Fingerprinted,
        },
    )
    .await
    .unwrap();
    let plan_id = ImportRepository::create_import_plan(client, run_id, 1, "2.0", library_root_id)
        .await
        .unwrap();
    let plan_album_id =
        ImportRepository::insert_plan_album(client, plan_id, album_id, "album_a", 1)
            .await
            .unwrap();
    ImportRepository::insert_plan_image(
        client,
        plan_album_id,
        import_image_id,
        "/src/a/p.png",
        "album_a/p.png",
        "p.png",
        7,
        &[0x11; 32],
        Some(1),
        Some(1),
        Some("png"),
    )
    .await
    .unwrap();
    let draft = ImportRepository::load_draft_plan(client, run_id)
        .await
        .unwrap()
        .unwrap();
    let hash = crate::services::commit_service::compute_plan_hash(&draft).unwrap();
    ImportRepository::set_plan_hash(client, plan_id, &hash)
        .await
        .unwrap();
    ImportRepository::update_import_plan_state(client, plan_id, &PlanState::Frozen)
        .await
        .unwrap();
    SeededRun {
        run_id,
        _library_root_id: library_root_id,
        album_id,
        _plan_id: plan_id,
        _plan_album_id: plan_album_id,
        _import_image_id: import_image_id,
    }
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

async fn force_run_state(
    client: &tokio_postgres::Client,
    run_id: Uuid,
    state: &ImportRunState,
    completed_at: Option<chrono::DateTime<chrono::Utc>>,
) {
    let s = state.to_string();
    client
        .execute(
            "UPDATE import_runs SET state = $1, completed_at = $2 WHERE id = $3",
            &[&s, &completed_at, &run_id],
        )
        .await
        .unwrap();
}

/// Rule 2: any active (recoverable) transaction forces the run to
/// `recovery_required`, regardless of the run's pre-existing state.
#[tokio::test]
#[ignore]
async fn real_reconcile_active_tx_forces_recovery_required() {
    let (_tmp, mgr) = fresh_db().await;
    let (client, handle) = mgr.lock().await.connect().await.unwrap();
    let seed = seed_run_with_frozen_plan(&client).await;
    // Run has just entered the commit phase; no transactions yet.
    force_run_state(&client, seed.run_id, &ImportRunState::Committing, None).await;

    // A single `planned` transaction means commit started but never
    // progressed — reconcile must flip the run to recovery_required.
    let tx_id = Uuid::new_v4();
    ImportRepository::insert_file_transaction(
        &client,
        tx_id,
        seed.run_id,
        seed.album_id,
        &TransactionState::Planned,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let r = reconcile_import_run_state(&client, seed.run_id)
        .await
        .unwrap();
    assert_eq!(r.state, ImportRunState::RecoveryRequired);
    assert!(
        r.changed,
        "state must change from committing to recovery_required"
    );
    assert!(r.completed_at.is_none());
    let (s, completed_at) = run_state(&client, seed.run_id).await;
    assert_eq!(s, "recovery_required");
    assert!(completed_at.is_none());

    drop(client);
    handle.abort();
    let mut m = mgr.lock().await;
    m.shutdown().await.unwrap();
}

/// Rule 4: when every frozen-plan album reaches `source_archived`, the
/// run flips to `completed` and `completed_at` is set.
#[tokio::test]
#[ignore]
async fn real_reconcile_completes_when_last_tx_archived() {
    let (_tmp, mgr) = fresh_db().await;
    let (client, handle) = mgr.lock().await.connect().await.unwrap();
    let seed = seed_run_with_frozen_plan(&client).await;
    force_run_state(
        &client,
        seed.run_id,
        &ImportRunState::RecoveryRequired,
        None,
    )
    .await;

    let tx_id = Uuid::new_v4();
    ImportRepository::insert_file_transaction(
        &client,
        tx_id,
        seed.run_id,
        seed.album_id,
        &TransactionState::SourceArchived,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let r = reconcile_import_run_state(&client, seed.run_id)
        .await
        .unwrap();
    assert_eq!(r.state, ImportRunState::Completed);
    assert!(r.changed);
    assert!(r.completed_at.is_some(), "completed_at must be set");
    let (s, completed_at) = run_state(&client, seed.run_id).await;
    assert_eq!(s, "completed");
    assert!(completed_at.is_some());

    drop(client);
    handle.abort();
    let mut m = mgr.lock().await;
    m.shutdown().await.unwrap();
}

/// Rule 2 with two albums: partial archive keeps `recovery_required`;
/// archiving the last one promotes to `completed`.
#[tokio::test]
#[ignore]
async fn real_reconcile_multi_album_partial_then_complete() {
    let (_tmp, mgr) = fresh_db().await;
    let (client, handle) = mgr.lock().await.connect().await.unwrap();
    let library_root_id = ImportRepository::upsert_default_library_root(&client)
        .await
        .unwrap();
    let run_id = ImportRepository::create_import_run(&client, "/src", library_root_id)
        .await
        .unwrap();
    let album_a = ImportRepository::insert_import_album(&client, run_id, "/src/a", "album_a")
        .await
        .unwrap();
    let album_b = ImportRepository::insert_import_album(&client, run_id, "/src/b", "album_b")
        .await
        .unwrap();
    let img_a = ImportRepository::insert_import_image(
        &client,
        NewImportImage {
            album_id: album_a,
            source_path: "/src/a/p.png".to_string(),
            relative_path: "album_a/p.png".to_string(),
            file_size: 7,
            modified_at: None,
            width: Some(1),
            height: Some(1),
            format: Some("png".to_string()),
            decode_state: DecodeState::Decoded,
            blake3: Some(vec![0x11; 32]),
            pixel_hash: Some(vec![1; 32]),
            block_hash_16: Some(vec![1; 32]),
            double_gradient_hash_32: Some(vec![1; 68]),
            fingerprint_version: Some("2".to_string()),
            state: ImportImageState::Fingerprinted,
        },
    )
    .await
    .unwrap();
    let img_b = ImportRepository::insert_import_image(
        &client,
        NewImportImage {
            album_id: album_b,
            source_path: "/src/b/q.png".to_string(),
            relative_path: "album_b/q.png".to_string(),
            file_size: 7,
            modified_at: None,
            width: Some(1),
            height: Some(1),
            format: Some("png".to_string()),
            decode_state: DecodeState::Decoded,
            blake3: Some(vec![0x22; 32]),
            pixel_hash: Some(vec![2; 32]),
            block_hash_16: Some(vec![2; 32]),
            double_gradient_hash_32: Some(vec![2; 68]),
            fingerprint_version: Some("2".to_string()),
            state: ImportImageState::Fingerprinted,
        },
    )
    .await
    .unwrap();
    let plan_id = ImportRepository::create_import_plan(&client, run_id, 1, "2.0", library_root_id)
        .await
        .unwrap();
    let plan_album_a = ImportRepository::insert_plan_album(&client, plan_id, album_a, "album_a", 1)
        .await
        .unwrap();
    let plan_album_b = ImportRepository::insert_plan_album(&client, plan_id, album_b, "album_b", 1)
        .await
        .unwrap();
    ImportRepository::insert_plan_image(
        &client,
        plan_album_a,
        img_a,
        "/src/a/p.png",
        "album_a/p.png",
        "p.png",
        7,
        &[0x11; 32],
        Some(1),
        Some(1),
        Some("png"),
    )
    .await
    .unwrap();
    ImportRepository::insert_plan_image(
        &client,
        plan_album_b,
        img_b,
        "/src/b/q.png",
        "album_b/q.png",
        "q.png",
        7,
        &[0x22; 32],
        Some(1),
        Some(1),
        Some("png"),
    )
    .await
    .unwrap();
    let draft = ImportRepository::load_draft_plan(&client, run_id)
        .await
        .unwrap()
        .unwrap();
    let hash = crate::services::commit_service::compute_plan_hash(&draft).unwrap();
    ImportRepository::set_plan_hash(&client, plan_id, &hash)
        .await
        .unwrap();
    ImportRepository::update_import_plan_state(&client, plan_id, &PlanState::Frozen)
        .await
        .unwrap();
    force_run_state(&client, run_id, &ImportRunState::Committing, None).await;

    // Both transactions start planned (active).
    let tx_a = Uuid::new_v4();
    let tx_b = Uuid::new_v4();
    ImportRepository::insert_file_transaction(
        &client,
        tx_a,
        run_id,
        album_a,
        &TransactionState::Planned,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    ImportRepository::insert_file_transaction(
        &client,
        tx_b,
        run_id,
        album_b,
        &TransactionState::Planned,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    // Both active → recovery_required.
    let r = reconcile_import_run_state(&client, run_id).await.unwrap();
    assert_eq!(r.state, ImportRunState::RecoveryRequired);

    // Archive only album A: still recovery_required (album B outstanding).
    ImportRepository::update_file_transaction_state(
        &client,
        tx_a,
        &TransactionState::SourceArchived,
        None,
    )
    .await
    .unwrap();
    let r = reconcile_import_run_state(&client, run_id).await.unwrap();
    assert_eq!(r.state, ImportRunState::RecoveryRequired);

    // Archive album B: now completed with completed_at set.
    ImportRepository::update_file_transaction_state(
        &client,
        tx_b,
        &TransactionState::SourceArchived,
        None,
    )
    .await
    .unwrap();
    let r = reconcile_import_run_state(&client, run_id).await.unwrap();
    assert_eq!(r.state, ImportRunState::Completed);
    assert!(r.completed_at.is_some());
    let (_s, completed_at) = run_state(&client, run_id).await;
    assert!(completed_at.is_some());

    drop(client);
    handle.abort();
    let mut m = mgr.lock().await;
    m.shutdown().await.unwrap();
}

/// Rule 1: any conflict transaction forces `recovery_required` and keeps
/// the run there, regardless of other archived siblings.
#[tokio::test]
#[ignore]
async fn real_reconcile_conflict_keeps_recovery_required() {
    let (_tmp, mgr) = fresh_db().await;
    let (client, handle) = mgr.lock().await.connect().await.unwrap();
    let seed = seed_run_with_frozen_plan(&client).await;
    force_run_state(
        &client,
        seed.run_id,
        &ImportRunState::RecoveryRequired,
        None,
    )
    .await;

    let tx_id = Uuid::new_v4();
    ImportRepository::insert_file_transaction(
        &client,
        tx_id,
        seed.run_id,
        seed.album_id,
        &TransactionState::Conflict,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let r = reconcile_import_run_state(&client, seed.run_id)
        .await
        .unwrap();
    assert_eq!(r.state, ImportRunState::RecoveryRequired);
    // Run was already recovery_required, so no write should happen.
    assert!(
        !r.changed,
        "reconcile must be a no-op when state already correct"
    );
    assert!(r.completed_at.is_none());

    drop(client);
    handle.abort();
    let mut m = mgr.lock().await;
    m.shutdown().await.unwrap();
}

/// Rule 5: a `completed` run that is later found to have an active
/// transaction is pulled back to `recovery_required`, and `completed_at`
/// is cleared so the row no longer claims completion.
#[tokio::test]
#[ignore]
async fn real_reconcile_active_pulls_completed_back() {
    let (_tmp, mgr) = fresh_db().await;
    let (client, handle) = mgr.lock().await.connect().await.unwrap();
    let seed = seed_run_with_frozen_plan(&client).await;
    let past = chrono::Utc::now() - chrono::TimeDelta::hours(1);
    force_run_state(&client, seed.run_id, &ImportRunState::Completed, Some(past)).await;

    let tx_id = Uuid::new_v4();
    ImportRepository::insert_file_transaction(
        &client,
        tx_id,
        seed.run_id,
        seed.album_id,
        &TransactionState::Planned,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let r = reconcile_import_run_state(&client, seed.run_id)
        .await
        .unwrap();
    assert_eq!(r.state, ImportRunState::RecoveryRequired);
    assert!(
        r.changed,
        "state must change from completed to recovery_required"
    );
    let (s, completed_at) = run_state(&client, seed.run_id).await;
    assert_eq!(s, "recovery_required");
    assert!(
        completed_at.is_none(),
        "completed_at must be cleared when run is pulled back"
    );

    drop(client);
    handle.abort();
    let mut m = mgr.lock().await;
    m.shutdown().await.unwrap();
}

/// Idempotency: calling reconcile twice in a row with no intervening
/// state change produces the same verdict and `changed = false` on the
/// second call.
#[tokio::test]
#[ignore]
async fn real_reconcile_idempotent() {
    let (_tmp, mgr) = fresh_db().await;
    let (client, handle) = mgr.lock().await.connect().await.unwrap();
    let seed = seed_run_with_frozen_plan(&client).await;
    force_run_state(
        &client,
        seed.run_id,
        &ImportRunState::RecoveryRequired,
        None,
    )
    .await;

    let tx_id = Uuid::new_v4();
    ImportRepository::insert_file_transaction(
        &client,
        tx_id,
        seed.run_id,
        seed.album_id,
        &TransactionState::SourceArchived,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    // First call: flips recovery_required → completed.
    let r1 = reconcile_import_run_state(&client, seed.run_id)
        .await
        .unwrap();
    assert_eq!(r1.state, ImportRunState::Completed);
    assert!(r1.changed);

    // Second call: no-op, changed must be false.
    let r2 = reconcile_import_run_state(&client, seed.run_id)
        .await
        .unwrap();
    assert_eq!(r2.state, ImportRunState::Completed);
    assert!(
        !r2.changed,
        "second reconcile must be a no-op (changed=false)"
    );
    assert!(r2.completed_at.is_some());

    drop(client);
    handle.abort();
    let mut m = mgr.lock().await;
    m.shutdown().await.unwrap();
}

/// Rule 3: failed or cancelled transactions block completion, even when
/// no active transaction remains. The run stays `recovery_required`.
#[tokio::test]
#[ignore]
async fn real_reconcile_failed_tx_blocks_completion() {
    let (_tmp, mgr) = fresh_db().await;
    let (client, handle) = mgr.lock().await.connect().await.unwrap();
    let seed = seed_run_with_frozen_plan(&client).await;
    force_run_state(&client, seed.run_id, &ImportRunState::Committing, None).await;

    let tx_id = Uuid::new_v4();
    ImportRepository::insert_file_transaction(
        &client,
        tx_id,
        seed.run_id,
        seed.album_id,
        &TransactionState::Failed,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let r = reconcile_import_run_state(&client, seed.run_id)
        .await
        .unwrap();
    assert_eq!(r.state, ImportRunState::RecoveryRequired);
    assert!(r.changed);
    assert!(r.completed_at.is_none());

    drop(client);
    handle.abort();
    let mut m = mgr.lock().await;
    m.shutdown().await.unwrap();
}
