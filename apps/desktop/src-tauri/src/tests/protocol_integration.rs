//! Real PostgreSQL integration tests for the new transaction protocol.
//!
//! These verify Phase 1 acceptance criteria:
//! 1. All migrations run on an empty database.
//! 2. A file transaction and `planned` file operation can be created.
//! 3. All legal transaction/file-op state transitions succeed.
//! 4. The database CHECK constraints reject illegal states.
//! 5. `pending` is rejected for file operations (must be `planned`).
//!
//! Invocation:
//!   IMAGEDB_POSTGRES_BIN=/path/to/pgsql/bin cargo test --manifest-path \
//!       apps/desktop/src-tauri/Cargo.toml --features real-db-tests --lib \
//!       real_protocol_ -- --ignored --test-threads=1
#![cfg(test)]
#![cfg(feature = "real-db-tests")]
use crate::domain::import_state::ImportRunState;
use crate::domain::state_machine::{FileOpState, TransactionState};
use crate::infrastructure::postgres::{MigrationRunner, PostgresManager};
use crate::repositories::import_repository::ImportRepository;
use crate::services::library_service;
use crate::services::review_service;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::Mutex;

async fn fresh_db() -> (TempDir, Arc<Mutex<PostgresManager>>) {
    if std::env::var("IMAGEDB_POSTGRES_BIN")
        .unwrap_or_default()
        .is_empty()
    {
        panic!("IMAGEDB_POSTGRES_BIN must be set for real protocol tests");
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

const RUN: &str = "00000000-0000-0000-0000-0000000000b2";
const ALBUM: &str = "00000000-0000-0000-0000-0000000000c3";
const TX: &str = "00000000-0000-0000-0000-0000000000d4";

async fn seed_run(client: &tokio_postgres::Client) {
    client
        .batch_execute(
            "INSERT INTO library_roots (id, path, display_name) VALUES \
             ('00000000-0000-0000-0000-0000000000a1','/lib','default'); \
             INSERT INTO import_runs (id, source_root, library_root_id, state, policy_version) VALUES \
             ('00000000-0000-0000-0000-0000000000b2','/src','00000000-0000-0000-0000-0000000000a1','created','1'); \
             INSERT INTO import_albums (id, import_run_id, source_path, source_name, state) VALUES \
             ('00000000-0000-0000-0000-0000000000c3','00000000-0000-0000-0000-0000000000b2','/src/a','album_a','pending');",
        )
        .await
        .unwrap();
}

fn blake3_placeholder() -> Vec<u8> {
    vec![0x11u8; 32]
}

#[tokio::test]
#[ignore]
async fn real_protocol_migrations_run_on_empty_db() {
    let (_tmp, mgr) = fresh_db().await;
    let (client, handle) = mgr.lock().await.connect().await.unwrap();
    let version = MigrationRunner::current_version(&client).await.unwrap();
    assert_eq!(version.as_deref(), Some(MigrationRunner::latest_version()));
    // All state columns now exist with CHECK constraints.
    let count: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_name IN \
             ('import_plans','import_plan_albums','import_plan_images','file_transactions','file_operations')",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 5);
    drop(client);
    handle.abort();
    let mut m = mgr.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn real_protocol_library_root_lease_excludes_second_writer_and_allows_expiry_takeover() {
    let (_tmp, mgr) = fresh_db().await;
    let (client, handle) = mgr.lock().await.connect().await.unwrap();
    seed_run(&client).await;
    let root_id = uuid::Uuid::parse_str("00000000-0000-0000-0000-0000000000a1").unwrap();

    let token1 = uuid::Uuid::new_v4();
    let lease1 =
        ImportRepository::acquire_library_root_lease(&client, root_id, "owner-a", token1, 60)
            .await
            .expect("owner-a should acquire lease");
    assert_eq!(lease1.lease_token, token1);

    let token2 = uuid::Uuid::new_v4();
    let blocked =
        ImportRepository::acquire_library_root_lease(&client, root_id, "owner-b", token2, 60)
            .await
            .expect_err("owner-b must not acquire an active lease");
    assert!(
        blocked.to_string().contains("owner-a"),
        "conflict should name current owner: {blocked}"
    );

    ImportRepository::release_library_root_lease(&client, root_id, token1)
        .await
        .expect("owner-a release");
    let lease2 =
        ImportRepository::acquire_library_root_lease(&client, root_id, "owner-b", token2, 60)
            .await
            .expect("owner-b should acquire after release");
    assert_eq!(lease2.lease_token, token2);

    client
        .execute(
            "UPDATE library_root_leases
             SET heartbeat_at = now() - interval '2 seconds',
                 expires_at = now() - interval '1 second'
             WHERE library_root_id = $1",
            &[&root_id],
        )
        .await
        .unwrap();
    let token3 = uuid::Uuid::new_v4();
    let lease3 =
        ImportRepository::acquire_library_root_lease(&client, root_id, "owner-c", token3, 60)
            .await
            .expect("expired lease should be taken over");
    assert_eq!(lease3.owner_instance_id, "owner-c");
    assert_eq!(lease3.lease_token, token3);

    drop(client);
    handle.abort();
    let mut m = mgr.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn real_protocol_creates_planned_file_operation() {
    let (_tmp, mgr) = fresh_db().await;
    let (client, handle) = mgr.lock().await.connect().await.unwrap();
    seed_run(&client).await;
    let tx_id = uuid::Uuid::parse_str(TX).unwrap();
    ImportRepository::insert_file_transaction(
        &client,
        tx_id,
        uuid::Uuid::parse_str(RUN).unwrap(),
        uuid::Uuid::parse_str(ALBUM).unwrap(),
        &TransactionState::Planned,
        Some("/staging"),
        Some("/target"),
        None,
    )
    .await
    .unwrap();
    let op_id = ImportRepository::insert_file_operation(
        &client,
        tx_id,
        "/src/a.png",
        "/staging/a.png",
        "/target/a.png",
        100,
        &blake3_placeholder(),
    )
    .await
    .unwrap();
    let ops = ImportRepository::get_file_operations(&client, tx_id)
        .await
        .unwrap();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].state, "planned");
    assert_eq!(ops[0].expected_size, 100);
    let _ = op_id;
    drop(client);
    handle.abort();
    let mut m = mgr.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn real_protocol_rejects_pending_file_operation() {
    let (_tmp, mgr) = fresh_db().await;
    let (client, handle) = mgr.lock().await.connect().await.unwrap();
    seed_run(&client).await;
    let tx_id = uuid::Uuid::parse_str(TX).unwrap();
    let id = uuid::Uuid::new_v4();
    // Direct SQL insert of the forbidden 'pending' state must be rejected.
    let result = client
        .execute(
            "INSERT INTO file_operations (id, transaction_id, source_path, staging_path, target_path, expected_size, expected_blake3, state)
             VALUES ($1, $2, '/s', '/st', '/t', 1, $3, 'pending')",
            &[&id, &tx_id, &blake3_placeholder()],
        )
        .await;
    assert!(
        result.is_err(),
        "DB must reject 'pending' state for file_operations"
    );
    drop(client);
    handle.abort();
    let mut m = mgr.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn real_protocol_rejects_illegal_transaction_state() {
    let (_tmp, mgr) = fresh_db().await;
    let (client, handle) = mgr.lock().await.connect().await.unwrap();
    seed_run(&client).await;
    let tx_id = uuid::Uuid::parse_str(TX).unwrap();
    // 'bogus' is not in the CHECK constraint.
    let result = client
        .execute(
            "INSERT INTO file_transactions (id, import_run_id, import_album_id, state)
             VALUES ($1, $2, $3, 'bogus')",
            &[
                &tx_id,
                &uuid::Uuid::parse_str(RUN).unwrap(),
                &uuid::Uuid::parse_str(ALBUM).unwrap(),
            ],
        )
        .await;
    assert!(result.is_err(), "DB must reject bogus transaction state");
    drop(client);
    handle.abort();
    let mut m = mgr.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn real_protocol_all_legal_transitions() {
    let (_tmp, mgr) = fresh_db().await;
    let (client, handle) = mgr.lock().await.connect().await.unwrap();
    seed_run(&client).await;
    let tx_id = uuid::Uuid::parse_str(TX).unwrap();
    ImportRepository::insert_file_transaction(
        &client,
        tx_id,
        uuid::Uuid::parse_str(RUN).unwrap(),
        uuid::Uuid::parse_str(ALBUM).unwrap(),
        &TransactionState::Planned,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    // Walk the full legal path via typed transitions.
    let transitions: &[(&str, TransactionState)] = &[
        ("stage", TransactionState::Staging),
        ("verify", TransactionState::Verifying),
        ("verified", TransactionState::Verified),
        ("publish", TransactionState::Publishing),
        ("published", TransactionState::Published),
        ("db_commit", TransactionState::DbCommitting),
        ("library_committed", TransactionState::LibraryCommitted),
        ("archive", TransactionState::SourceArchiving),
        ("archived", TransactionState::SourceArchived),
    ];
    for (action, expected) in transitions {
        let cur = ImportRepository::get_file_transaction(&client, tx_id)
            .await
            .unwrap()
            .unwrap();
        let prev = TransactionState::parse(&cur.state).unwrap();
        let next = crate::domain::state_machine::transition_transaction(prev, action).unwrap();
        assert_eq!(next, *expected, "transition {action} from {prev:?}");
        ImportRepository::update_file_transaction_state(&client, tx_id, &next, None)
            .await
            .unwrap();
    }
    // File op legal path.
    let op_id = ImportRepository::insert_file_operation(
        &client,
        tx_id,
        "/s",
        "/st",
        "/t",
        1,
        &blake3_placeholder(),
    )
    .await
    .unwrap();
    for (action, expected) in [
        ("copy", FileOpState::Copying),
        ("copied", FileOpState::Copied),
        ("verify", FileOpState::Verifying),
        ("verified", FileOpState::Verified),
        ("publish", FileOpState::Published),
    ] {
        let cur = FileOpState::parse(
            &ImportRepository::get_file_operations(&client, tx_id)
                .await
                .unwrap()
                .into_iter()
                .find(|o| o.id == op_id)
                .unwrap()
                .state,
        )
        .unwrap();
        let next = crate::domain::state_machine::next_file_op_state(&cur, action).unwrap();
        assert_eq!(next, expected);
        ImportRepository::update_file_operation_state(&client, op_id, &next, None, None)
            .await
            .unwrap();
    }
    drop(client);
    handle.abort();
    let mut m = mgr.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn real_protocol_invalid_transition_rejected() {
    use crate::domain::state_machine::{next_file_op_state, transition_transaction, FileOpState};
    // planned -> publish is not a legal transaction transition.
    assert!(transition_transaction(TransactionState::Planned, "publish").is_err());
    // planned -> verified is legal (recovery skip); planned -> published is not for file ops.
    assert!(next_file_op_state(&FileOpState::Planned, "publish").is_err());
    // file op: verified -> copy is illegal.
    assert!(next_file_op_state(&FileOpState::Verified, "copy").is_err());
}

/// Phase 2 / rule 4: a frozen plan whose stored plan_hash was tampered with
/// must be rejected wholesale — no silent partial commit.
#[tokio::test]
#[ignore]
async fn real_protocol_tampered_plan_hash_rejected() {
    use crate::domain::import_state::{DecodeState, ImportImageState};
    use crate::domain::state_machine::PlanState;
    use crate::infrastructure::postgres::MigrationRunner;
    use crate::repositories::import_repository::NewImportImage;
    use std::sync::Arc;
    use tempfile::TempDir;

    if std::env::var("IMAGEDB_POSTGRES_BIN")
        .unwrap_or_default()
        .is_empty()
    {
        panic!(
            "IMAGEDB_POSTGRES_BIN is not set; cannot run the real protocol integration test.              Set IMAGEDB_POSTGRES_BIN to a PostgreSQL 18.x bin directory, or run              `node scripts/package-postgres-runtime.mjs` to populate the packaged runtime              at .local/db-tools/postgresql-18.4/pgsql/bin."
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
    let b3 = blake3::hash(b"photo one data").as_bytes().to_vec();

    let mut manager = PostgresManager::new(&app_data);
    assert!(manager.binaries_available());
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
    ImportRepository::insert_import_image(
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

    // Persist the full source album snapshot (scan does this in production;
    // commit Phase 6 requires it to verify source/archive integrity).
    crate::services::source_snapshot_service::capture_source_album_snapshot(
        &client,
        run_id,
        album_id,
        &album_path,
    )
    .await
    .unwrap();

    let plan_id = ImportRepository::create_import_plan(&client, run_id, 1, "2.0", library_root_id)
        .await
        .unwrap();
    let plan_album_id =
        ImportRepository::insert_plan_album(&client, plan_id, album_id, "album_a", 1)
            .await
            .unwrap();
    let img_id: uuid::Uuid = client
        .query_one(
            "SELECT id FROM import_images WHERE relative_path = 'album_a/photo1.png'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    ImportRepository::insert_plan_image(
        &client,
        plan_album_id,
        img_id,
        &album_path.join("photo1.png").display().to_string(),
        "album_a/photo1.png",
        "photo1.png",
        14,
        &b3,
        Some(10),
        Some(10),
        Some("png"),
    )
    .await
    .unwrap();
    let frozen = ImportRepository::load_draft_plan(&client, run_id)
        .await
        .unwrap()
        .unwrap();
    let hash = crate::services::commit_service::compute_plan_hash(&frozen).unwrap();
    ImportRepository::set_plan_hash(&client, plan_id, &hash)
        .await
        .unwrap();
    ImportRepository::update_import_plan_state(&client, plan_id, &PlanState::Frozen)
        .await
        .unwrap();
    ImportRepository::update_import_run_state(&client, run_id, &ImportRunState::ReadyToCommit)
        .await
        .unwrap();
    drop(client);
    db_handle.abort();

    let pg = Arc::new(tokio::sync::Mutex::new(manager));
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let progress = Arc::new(tokio::sync::Mutex::new(
        crate::domain::import_state::CommitProgress::idle(&run_id.to_string()),
    ));
    let ok = crate::services::commit_service::run_import_commit(
        pg.clone(),
        library_root.display().to_string(),
        run_id,
        cancelled.clone(),
        progress.clone(),
    )
    .await
    .unwrap();
    assert_eq!(ok.state, "completed");

    // Tamper the plan_hash; a rerun must reject it (no silent skip).
    let (client, db_handle) = pg.lock().await.connect().await.unwrap();
    client
        .execute(
            "UPDATE import_plans SET plan_hash = decode('00', 'hex') WHERE import_run_id = $1",
            &[&run_id],
        )
        .await
        .unwrap();
    drop(client);
    db_handle.abort();

    let cancelled2 = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let progress2 = Arc::new(tokio::sync::Mutex::new(
        crate::domain::import_state::CommitProgress::idle(&run_id.to_string()),
    ));
    let result = crate::services::commit_service::run_import_commit(
        pg.clone(),
        library_root.display().to_string(),
        run_id,
        cancelled2,
        progress2,
    )
    .await;
    assert!(
        result.is_err(),
        "tampered plan hash must be rejected, not silently skipped"
    );
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn real_protocol_library_catalog_returns_album_and_image_pages() {
    let (_tmp, mgr) = fresh_db().await;
    let (client, handle) = mgr.lock().await.connect().await.unwrap();
    let root_id = uuid::Uuid::new_v4();
    let album_ids = [
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
    ];
    let result = async {
        client
            .execute(
                "INSERT INTO library_roots (id, path, display_name) VALUES ($1, '/library', 'default')",
                &[&root_id],
            )
            .await
            .map_err(|error| error.to_string())?;

        for (index, album_id) in album_ids.iter().enumerate() {
            let display_name = format!("album-{index}");
            let image_count = if index == 0 { 2 } else { 1 };
            client
                .execute(
                    "INSERT INTO library_albums
                        (id, library_root_id, display_name, relative_path, manifest_version,
                         manifest_hash, image_count, committed_at, state)
                     VALUES ($1, $2, $3, $3, '1', $4, $5,
                             now(), 'committed')",
                    &[
                        album_id,
                        &root_id,
                        &display_name,
                        &vec![index as u8; 32],
                        &image_count,
                    ],
                )
                .await
                .map_err(|error| error.to_string())?;
            for image_index in 0..image_count {
                let relative_path = format!("image-{image_index}.jpg");
                let file_size = 100_i64 + i64::from(image_index);
                client
                    .execute(
                        "INSERT INTO library_images
                            (id, album_id, relative_path, file_size, width, height, format,
                             blake3, fingerprint_version, state)
                         VALUES ($1, $2, $3, $4, 800, 600, 'jpg', $5, '2.0', 'committed')",
                        &[
                            &uuid::Uuid::new_v4(),
                            album_id,
                            &relative_path,
                            &file_size,
                            &vec![image_index as u8; 32],
                        ],
                    )
                    .await
                    .map_err(|error| error.to_string())?;
            }
        }

        let first_page = library_service::list_library_albums(&client, 0, 2)
            .await
            .map_err(|error| error.to_string())?;
        let second_page = library_service::list_library_albums(&client, 2, 2)
            .await
            .map_err(|error| error.to_string())?;
        let image_page = library_service::list_library_images(&client, album_ids[0], 0, 1)
            .await
            .map_err(|error| error.to_string())?;
        Ok::<_, String>((first_page, second_page, image_page))
    }
    .await;

    drop(client);
    handle.abort();
    mgr.lock().await.shutdown().await.unwrap();

    let (first_page, second_page, image_page) = result.unwrap();

    assert_eq!(first_page.albums.len(), 2);
    assert_eq!(first_page.total_albums, 3);
    assert_eq!(first_page.total_images, 4);
    assert_eq!(first_page.total_size, 401);
    assert_eq!(second_page.albums.len(), 1);
    assert_eq!(second_page.total_albums, 3);
    assert_eq!(image_page.images.len(), 1);
    assert_eq!(image_page.total_images, 2);
    assert_eq!(image_page.total_size, 201);
}

#[tokio::test]
#[ignore]
async fn real_protocol_frozen_workflow_abandon_is_auditable_and_stops_at_transaction_boundary() {
    let (_tmp, mgr) = fresh_db().await;
    let (client, handle) = mgr.lock().await.connect().await.unwrap();
    let root_id = uuid::Uuid::new_v4();
    let ready_run_id = uuid::Uuid::new_v4();
    let ready_album_id = uuid::Uuid::new_v4();
    let ready_plan_id = uuid::Uuid::new_v4();
    let blocked_run_id = uuid::Uuid::new_v4();
    let blocked_album_id = uuid::Uuid::new_v4();
    let blocked_plan_id = uuid::Uuid::new_v4();

    client
        .execute(
            "INSERT INTO library_roots (id, path, display_name) VALUES ($1, '/library', 'default')",
            &[&root_id],
        )
        .await
        .unwrap();

    for (run_id, album_id, plan_id, source_name) in [
        (ready_run_id, ready_album_id, ready_plan_id, "ready_album"),
        (
            blocked_run_id,
            blocked_album_id,
            blocked_plan_id,
            "blocked_album",
        ),
    ] {
        client
            .execute(
                "INSERT INTO import_runs
                    (id, source_root, library_root_id, state, policy_version)
                 VALUES ($1, '/source', $2, 'ready_to_commit', '2.0')",
                &[&run_id, &root_id],
            )
            .await
            .unwrap();
        client
            .execute(
                "INSERT INTO import_albums
                    (id, import_run_id, source_path, source_name, state)
                 VALUES ($1, $2, '/source/album', $3, 'analyzed')",
                &[&album_id, &run_id, &source_name],
            )
            .await
            .unwrap();
        client
            .execute(
                "INSERT INTO import_plans
                    (id, import_run_id, version, state, policy_version, library_root_id, plan_hash)
                 VALUES ($1, $2, 1, 'frozen', '2.0', $3, $4)",
                &[&plan_id, &run_id, &root_id, &vec![0x42u8; 32]],
            )
            .await
            .unwrap();
    }

    review_service::abandon_frozen_import_workflow(&client, ready_run_id)
        .await
        .expect("a frozen workflow with no transaction should be abandonable");
    let ready_row = client
        .query_one(
            "SELECT r.state AS run_state, p.state AS plan_state
             FROM import_runs r JOIN import_plans p ON p.import_run_id = r.id
             WHERE r.id = $1",
            &[&ready_run_id],
        )
        .await
        .unwrap();
    assert_eq!(ready_row.get::<_, String>("run_state"), "abandoned");
    assert_eq!(ready_row.get::<_, String>("plan_state"), "invalidated");
    let preserved_album_count: i64 = client
        .query_one(
            "SELECT COUNT(*)::BIGINT FROM import_albums WHERE import_run_id = $1",
            &[&ready_run_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(preserved_album_count, 1);
    assert_eq!(
        ImportRepository::get_latest_reviewable_run(&client)
            .await
            .unwrap(),
        Some(blocked_run_id)
    );

    client
        .execute(
            "INSERT INTO file_transactions
                (id, import_run_id, import_album_id, state, plan_hash)
             VALUES ($1, $2, $3, 'planned', $4)",
            &[
                &uuid::Uuid::new_v4(),
                &blocked_run_id,
                &blocked_album_id,
                &vec![0x42u8; 32],
            ],
        )
        .await
        .unwrap();
    let error = review_service::abandon_frozen_import_workflow(&client, blocked_run_id)
        .await
        .expect_err("transaction evidence must make workflow abandon fail closed");
    assert!(error
        .to_string()
        .contains("after commit transactions have been created"));
    let blocked_row = client
        .query_one(
            "SELECT r.state AS run_state, p.state AS plan_state
             FROM import_runs r JOIN import_plans p ON p.import_run_id = r.id
             WHERE p.id = $1",
            &[&blocked_plan_id],
        )
        .await
        .unwrap();
    assert_eq!(blocked_row.get::<_, String>("run_state"), "ready_to_commit");
    assert_eq!(blocked_row.get::<_, String>("plan_state"), "frozen");

    drop(client);
    handle.abort();
    mgr.lock().await.shutdown().await.unwrap();
}

/// Phase 8 / 10: cross-album exact duplicates + a historical library image.
/// Verifies the indexed match queries (find_sibling_images_by_blake3 for
/// cross-album, find_library_images_by_blake3 for history) and the duplicate
/// group representative preference. This does NOT call run_scan (which links
/// the tauri runtime in the test binary); it exercises the same repository
/// + duplicate-group logic directly.
#[tokio::test]
#[ignore]
async fn real_protocol_cross_album_and_history_duplicates() {
    use crate::domain::duplicate_group::{build_duplicate_groups, DuplicateEdge};
    use crate::infrastructure::postgres::MigrationRunner;
    use crate::repositories::import_repository::ImportRepository;
    use std::sync::Arc;
    use tempfile::TempDir;

    if std::env::var("IMAGEDB_POSTGRES_BIN")
        .unwrap_or_default()
        .is_empty()
    {
        panic!(
            "IMAGEDB_POSTGRES_BIN is not set; cannot run the real protocol integration test.              Set IMAGEDB_POSTGRES_BIN to a PostgreSQL 18.x bin directory, or run              `node scripts/package-postgres-runtime.mjs` to populate the packaged runtime              at .local/db-tools/postgresql-18.4/pgsql/bin."
        );
    }
    let tmp = TempDir::new().unwrap();
    let app_data = tmp.path().join("app_data");
    let mut manager = PostgresManager::new(&app_data);
    assert!(manager.binaries_available());
    let probe = manager.initialize().await.unwrap();
    assert!(probe.connection_ok);
    let (mut client, handle) = manager.connect().await.unwrap();
    MigrationRunner::run_pending(&mut client).await.unwrap();

    let library_root_id = ImportRepository::upsert_default_library_root(&client)
        .await
        .unwrap();
    let source_root = "/src";
    let run_id = ImportRepository::create_import_run(&client, source_root, library_root_id)
        .await
        .unwrap();
    let album_a = ImportRepository::insert_import_album(&client, run_id, "/src/album_a", "album_a")
        .await
        .unwrap();
    let album_b = ImportRepository::insert_import_album(&client, run_id, "/src/album_b", "album_b")
        .await
        .unwrap();

    let b3 = blake3::hash(b"shared bytes").as_bytes().to_vec();
    // Two import images (one per album) with the SAME blake3.
    let img_a =
        ImportRepository::insert_import_image(&client, new_img(album_a, "album_a/x.png", &b3))
            .await
            .unwrap();
    let img_b =
        ImportRepository::insert_import_image(&client, new_img(album_b, "album_b/x.png", &b3))
            .await
            .unwrap();

    // A pre-existing library image with the SAME blake3 (historical dup).
    let lib_album_id = uuid::Uuid::new_v4();
    client.execute(
        "INSERT INTO library_albums (id, library_root_id, display_name, relative_path, manifest_version, manifest_hash, image_count, state)
         VALUES ($1, $2, 'album_a', 'album_a', '1.0', decode('00','hex'), 1, 'committed')",
        &[&lib_album_id, &library_root_id],
    ).await.unwrap();
    let lib_img_id = uuid::Uuid::new_v4();
    client.execute(
        "INSERT INTO library_images (id, album_id, relative_path, file_size, width, height, format, blake3, fingerprint_version, state)
         VALUES ($1, $2, 'x.png', 12, 1, 1, 'png', $3, 'test', 'committed')",
        &[&lib_img_id, &lib_album_id, &b3],
    ).await.unwrap();

    // Cross-album: siblings share blake3 across albums.
    let siblings =
        ImportRepository::find_sibling_images_by_blake3(&client, run_id, std::slice::from_ref(&b3))
            .await
            .unwrap();
    assert!(
        siblings.iter().any(|(id, _, _, _)| *id == img_a),
        "img_a should be a sibling"
    );
    assert!(
        siblings.iter().any(|(id, _, _, _)| *id == img_b),
        "img_b should be a sibling"
    );
    assert_eq!(siblings.len(), 2, "exactly two siblings across albums");

    // Historical: library image matched by blake3.
    let lib_matches =
        ImportRepository::find_library_images_by_blake3(&client, std::slice::from_ref(&b3))
            .await
            .unwrap();
    assert_eq!(lib_matches.len(), 1, "exactly one historical library match");
    assert_eq!(lib_matches[0].id, lib_img_id);

    // Duplicate group representative: library image wins over import images.
    let edges = vec![
        DuplicateEdge {
            image_a: img_a,
            image_b: lib_img_id,
            a_is_import: true,
            b_is_import: false,
            confidence: 1.0,
            blake3_equal: true,
            pixel_hash_equal: true,
        },
        DuplicateEdge {
            image_a: img_a,
            image_b: img_b,
            a_is_import: true,
            b_is_import: true,
            confidence: 1.0,
            blake3_equal: true,
            pixel_hash_equal: true,
        },
    ];
    let groups = build_duplicate_groups(&edges);
    assert_eq!(groups.len(), 1, "one connected duplicate group");
    assert_eq!(
        groups[0].representative_id, lib_img_id,
        "library image must be the representative"
    );
    assert!(
        !groups[0].representative_is_import,
        "representative must be a library image, not an import image"
    );

    drop(client);
    handle.abort();
    let mut m = manager;
    m.shutdown().await.unwrap();
    let _ = Arc::new(0u32); // silence unused import
}

fn new_img(
    album_id: uuid::Uuid,
    rel: &str,
    b3: &[u8],
) -> crate::repositories::import_repository::NewImportImage {
    use crate::domain::import_state::{DecodeState, ImportImageState};
    crate::repositories::import_repository::NewImportImage {
        album_id,
        source_path: format!("/{rel}"),
        relative_path: rel.to_string(),
        file_size: 12,
        modified_at: None,
        width: Some(1),
        height: Some(1),
        format: Some("png".to_string()),
        decode_state: DecodeState::Decoded,
        blake3: Some(b3.to_vec()),
        pixel_hash: Some(vec![1; 8]),
        gradient_hash: Some(vec![1; 8]),
        block_hash: Some(vec![1; 8]),
        median_hash: Some(vec![1; 8]),
        fingerprint_version: Some("test".to_string()),
        state: ImportImageState::Fingerprinted,
    }
}

/// M5-C: after a successful commit, the transaction's persisted manifest_path
/// must be the canonical published path (`<publish_dir>/.imagedb/
/// .imagedb-manifest.json`), never the pre-rename staging location.
#[tokio::test]
#[ignore]
async fn real_protocol_manifest_path_is_published() {
    use crate::domain::import_state::{DecodeState, ImportImageState};
    use crate::domain::state_machine::PlanState;
    use crate::repositories::import_repository::NewImportImage;
    use std::sync::Arc;
    use tempfile::TempDir;

    if std::env::var("IMAGEDB_POSTGRES_BIN")
        .unwrap_or_default()
        .is_empty()
    {
        panic!(
            "IMAGEDB_POSTGRES_BIN is not set; cannot run the real protocol integration test.              Set IMAGEDB_POSTGRES_BIN to a PostgreSQL 18.x bin directory, or run              `node scripts/package-postgres-runtime.mjs` to populate the packaged runtime              at .local/db-tools/postgresql-18.4/pgsql/bin."
        );
    }
    let tmp = TempDir::new().unwrap();
    let app_data = tmp.path().join("app_data");
    let source_root = tmp.path().join("source");
    let library_root = tmp.path().join("library");
    let album_path = source_root.join("album_a");
    std::fs::create_dir_all(&album_path).unwrap();
    std::fs::create_dir_all(&library_root).unwrap();
    std::fs::write(album_path.join("p.png"), b"payload").unwrap();
    let b3 = blake3::hash(b"payload").as_bytes().to_vec();

    let mut manager = PostgresManager::new(&app_data);
    assert!(manager.binaries_available());
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
    ImportRepository::insert_import_image(
        &client,
        NewImportImage {
            album_id,
            source_path: album_path.join("p.png").display().to_string(),
            relative_path: "album_a/p.png".to_string(),
            file_size: 7,
            modified_at: None,
            width: Some(1),
            height: Some(1),
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

    // Persist the full source album snapshot (scan does this in production;
    // commit Phase 6 requires it to verify source/archive integrity).
    crate::services::source_snapshot_service::capture_source_album_snapshot(
        &client,
        run_id,
        album_id,
        &album_path,
    )
    .await
    .unwrap();

    let plan_id = ImportRepository::create_import_plan(&client, run_id, 1, "2.0", library_root_id)
        .await
        .unwrap();
    let plan_album_id =
        ImportRepository::insert_plan_album(&client, plan_id, album_id, "album_a", 1)
            .await
            .unwrap();
    let img_id: uuid::Uuid = client
        .query_one(
            "SELECT id FROM import_images WHERE relative_path = 'album_a/p.png'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    ImportRepository::insert_plan_image(
        &client,
        plan_album_id,
        img_id,
        &album_path.join("p.png").display().to_string(),
        "album_a/p.png",
        "p.png",
        7,
        &b3,
        Some(1),
        Some(1),
        Some("png"),
    )
    .await
    .unwrap();
    let frozen = ImportRepository::load_draft_plan(&client, run_id)
        .await
        .unwrap()
        .unwrap();
    let hash = crate::services::commit_service::compute_plan_hash(&frozen).unwrap();
    ImportRepository::set_plan_hash(&client, plan_id, &hash)
        .await
        .unwrap();
    ImportRepository::update_import_plan_state(&client, plan_id, &PlanState::Frozen)
        .await
        .unwrap();
    ImportRepository::update_import_run_state(&client, run_id, &ImportRunState::ReadyToCommit)
        .await
        .unwrap();
    drop(client);
    db_handle.abort();

    let pg = Arc::new(tokio::sync::Mutex::new(manager));
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let progress = Arc::new(tokio::sync::Mutex::new(
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
    assert_eq!(ok.state, "completed");
    let returned_path = ok.album_results[0]
        .manifest_path
        .as_deref()
        .expect("manifest_path must be set");

    let publish_dir = library_root.join("Albums").join("album_a");
    let published_manifest = publish_dir.join(".imagedb").join(".imagedb-manifest.json");
    assert!(
        published_manifest.exists(),
        "published manifest file missing at {}",
        published_manifest.display()
    );
    let returned_pathbuf = std::path::PathBuf::from(returned_path);
    assert_eq!(
        returned_pathbuf, published_manifest,
        "CommitAlbumResult.manifest_path must be the published path, got {returned_path}"
    );
    // The file_transactions row must also point at the published path.
    let (client, db_handle) = pg.lock().await.connect().await.unwrap();
    let db_manifest_path: Option<String> = client
        .query_one(
            "SELECT manifest_path FROM file_transactions WHERE import_album_id = $1",
            &[&album_id],
        )
        .await
        .unwrap()
        .get(0);
    drop(client);
    db_handle.abort();
    let db_manifest_path = db_manifest_path.expect("manifest_path must be persisted");
    assert_eq!(
        std::path::PathBuf::from(&db_manifest_path),
        published_manifest,
        "file_transactions.manifest_path must be the published path, got {db_manifest_path}"
    );
    // The path must NOT contain the staging marker after rename.
    assert!(
        !db_manifest_path.contains(".imagedb\\staging")
            && !db_manifest_path.contains(".imagedb/staging"),
        "manifest_path must not reference the staging directory: {db_manifest_path}"
    );
    let mut m = pg.lock().await;
    m.shutdown().await.unwrap();
}
