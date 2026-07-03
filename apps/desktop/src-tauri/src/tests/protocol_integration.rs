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
use crate::domain::state_machine::{FileOpState, TransactionState};
use crate::infrastructure::postgres::{MigrationRunner, PostgresManager};
use crate::repositories::import_repository::ImportRepository;
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
    assert_eq!(version.as_deref(), Some("0007_transaction_links"));
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
