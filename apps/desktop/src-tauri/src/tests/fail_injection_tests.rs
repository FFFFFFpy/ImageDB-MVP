/// Failure injection acceptance tests.
///
/// These tests verify that the system can recover from failures at every
/// key point in the staged file transaction protocol. They require
/// real PostgreSQL and filesystem access.
///
/// Invocation:
///   cargo test --features fail-injection,real-db-tests -- --ignored --test-threads=1 fail_injection_
#[cfg(test)]
#[cfg(feature = "fail-injection")]
mod tests {
    use crate::domain::import_state::{DecodeState, ImportImageState, ImportRunState};
    use crate::infrastructure::postgres::{MigrationRunner, PostgresManager};
    use crate::repositories::import_repository::{ImportRepository, NewImportImage};
    use crate::services::commit_service;
    use crate::services::recovery_service;
    use crate::tests::fail_injection::{clear_fault_point, set_fault_point, CommitFaultPoint};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::Mutex;
    use uuid::Uuid;

    /// Set up a full test environment with PostgreSQL.
    async fn setup_full_env() -> (TempDir, Arc<Mutex<PostgresManager>>, Uuid, std::path::PathBuf) {
        let tmp = TempDir::new().unwrap();
        let app_data = tmp.path().join("app_data");
        let source_root = tmp.path().join("source");
        let library_root = tmp.path().join("library");
        let album_path = source_root.join("album_a");
        std::fs::create_dir_all(&album_path).unwrap();
        std::fs::write(album_path.join("photo1.png"), b"photo one data").unwrap();
        std::fs::write(album_path.join("photo2.png"), b"photo two data").unwrap();

        let mut manager = PostgresManager::new(&app_data);
        if !manager.binaries_available() {
            panic!("PostgreSQL binaries not available");
        }
        let probe = manager.initialize().await.unwrap();
        assert!(probe.connection_ok);

        let (mut client, db_handle) = manager.connect().await.unwrap();
        MigrationRunner::run_pending(&mut client).await.unwrap();

        let library_root_id = ImportRepository::upsert_default_library_root(&client).await.unwrap();
        ImportRepository::update_library_root_path(&client, library_root_id, &library_root.display().to_string()).await.unwrap();

        let import_run_id = ImportRepository::create_import_run(&client, &source_root.display().to_string(), library_root_id).await.unwrap();
        let album_id = ImportRepository::insert_import_album(&client, import_run_id, &album_path.display().to_string(), "album_a").await.unwrap();

        let img1_blake3 = blake3::hash(b"photo one data").as_bytes().to_vec();
        let img2_blake3 = blake3::hash(b"photo two data").as_bytes().to_vec();

        let _img1 = ImportRepository::insert_import_image(&client, NewImportImage {
            album_id, source_path: album_path.join("photo1.png").display().to_string(),
            relative_path: "album_a/photo1.png".to_string(), file_size: 14,
            modified_at: None, width: Some(10), height: Some(10), format: Some("png".to_string()),
            decode_state: DecodeState::Decoded, blake3: Some(img1_blake3.clone()),
            pixel_hash: Some(vec![1; 8]), gradient_hash: Some(vec![1; 8]),
            block_hash: Some(vec![1; 8]), median_hash: Some(vec![1; 8]),
            fingerprint_version: Some("test".to_string()), state: ImportImageState::Fingerprinted,
        }).await.unwrap();

        let _img2 = ImportRepository::insert_import_image(&client, NewImportImage {
            album_id, source_path: album_path.join("photo2.png").display().to_string(),
            relative_path: "album_a/photo2.png".to_string(), file_size: 14,
            modified_at: None, width: Some(10), height: Some(10), format: Some("png".to_string()),
            decode_state: DecodeState::Decoded, blake3: Some(img2_blake3.clone()),
            pixel_hash: Some(vec![2; 8]), gradient_hash: Some(vec![2; 8]),
            block_hash: Some(vec![2; 8]), median_hash: Some(vec![2; 8]),
            fingerprint_version: Some("test".to_string()), state: ImportImageState::Fingerprinted,
        }).await.unwrap();

        ImportRepository::update_import_run_state(&client, import_run_id, &ImportRunState::Completed).await.unwrap();

        drop(client);
        db_handle.abort();

        let pg_manager = Arc::new(Mutex::new(manager));
        (tmp, pg_manager, import_run_id, library_root)
    }

    /// Run a full commit and verify the result.
    async fn run_commit_and_verify(
        pg_manager: Arc<Mutex<PostgresManager>>,
        library_root: &std::path::Path,
        import_run_id: Uuid,
        expect_success: bool,
    ) {
        let cancelled = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(tokio::sync::Mutex::new(
            crate::domain::import_state::CommitProgress::idle(&import_run_id.to_string())
        ));

        let result = commit_service::run_import_commit(
            pg_manager.clone(),
            library_root.display().to_string(),
            import_run_id,
            cancelled,
            progress,
        )
        .await;

        if expect_success {
            assert!(result.is_ok(), "commit should succeed: {:?}", result.err());
            let r = result.unwrap();
            assert_eq!(r.state, "completed", "commit state should be completed");
        }
    }

    #[tokio::test]
    #[ignore]
    async fn fail_injection_after_db_write() {
        let (_tmp, pg_manager, import_run_id, library_root) = setup_full_env().await;
        set_fault_point(CommitFaultPoint::AfterDbWrite);

        let cancelled = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(tokio::sync::Mutex::new(
            crate::domain::import_state::CommitProgress::idle(&import_run_id.to_string())
        ));
        let result = commit_service::run_import_commit(
            pg_manager.clone(), library_root.display().to_string(),
            import_run_id, cancelled, progress,
        ).await;
        assert!(result.is_err(), "should fail after DB write");

        clear_fault_point();
        run_commit_and_verify(pg_manager, &library_root, import_run_id, true).await;
    }

    #[tokio::test]
    #[ignore]
    async fn fail_injection_before_publish_rename() {
        let (_tmp, pg_manager, import_run_id, library_root) = setup_full_env().await;
        set_fault_point(CommitFaultPoint::BeforePublishRename);

        let cancelled = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(tokio::sync::Mutex::new(
            crate::domain::import_state::CommitProgress::idle(&import_run_id.to_string())
        ));
        let result = commit_service::run_import_commit(
            pg_manager.clone(), library_root.display().to_string(),
            import_run_id, cancelled, progress,
        ).await;
        assert!(result.is_err(), "should fail before publish rename");

        clear_fault_point();
        run_commit_and_verify(pg_manager, &library_root, import_run_id, true).await;
    }

    #[tokio::test]
    #[ignore]
    async fn fail_injection_after_publish_rename() {
        let (_tmp, pg_manager, import_run_id, library_root) = setup_full_env().await;
        set_fault_point(CommitFaultPoint::AfterPublishRename);

        let cancelled = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(tokio::sync::Mutex::new(
            crate::domain::import_state::CommitProgress::idle(&import_run_id.to_string())
        ));
        let result = commit_service::run_import_commit(
            pg_manager.clone(), library_root.display().to_string(),
            import_run_id, cancelled, progress,
        ).await;
        assert!(result.is_err(), "should fail after publish rename");

        clear_fault_point();
        run_commit_and_verify(pg_manager, &library_root, import_run_id, true).await;
    }

    #[tokio::test]
    #[ignore]
    async fn fail_injection_before_db_commit() {
        let (_tmp, pg_manager, import_run_id, library_root) = setup_full_env().await;
        set_fault_point(CommitFaultPoint::BeforeDbCommit);

        let cancelled = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(tokio::sync::Mutex::new(
            crate::domain::import_state::CommitProgress::idle(&import_run_id.to_string())
        ));
        let result = commit_service::run_import_commit(
            pg_manager.clone(), library_root.display().to_string(),
            import_run_id, cancelled, progress,
        ).await;
        assert!(result.is_err(), "should fail before DB commit");

        clear_fault_point();
        run_commit_and_verify(pg_manager, &library_root, import_run_id, true).await;
    }

    #[tokio::test]
    #[ignore]
    async fn fail_injection_before_source_archive() {
        let (_tmp, pg_manager, import_run_id, library_root) = setup_full_env().await;
        set_fault_point(CommitFaultPoint::BeforeSourceArchive);

        let cancelled = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(tokio::sync::Mutex::new(
            crate::domain::import_state::CommitProgress::idle(&import_run_id.to_string())
        ));
        let result = commit_service::run_import_commit(
            pg_manager.clone(), library_root.display().to_string(),
            import_run_id, cancelled, progress,
        ).await;
        assert!(result.is_err(), "should fail before source archive");

        clear_fault_point();
        run_commit_and_verify(pg_manager, &library_root, import_run_id, true).await;
    }

    #[tokio::test]
    #[ignore]
    async fn fail_injection_cancel_during_commit() {
        let (_tmp, pg_manager, import_run_id, library_root) = setup_full_env().await;

        let cancelled = Arc::new(AtomicBool::new(true));
        let progress = Arc::new(tokio::sync::Mutex::new(
            crate::domain::import_state::CommitProgress::idle(&import_run_id.to_string())
        ));
        let result = commit_service::run_import_commit(
            pg_manager.clone(), library_root.display().to_string(),
            import_run_id, cancelled, progress,
        ).await;
        assert!(result.is_ok(), "cancel should not hard-fail");

        let publish_dir = library_root.join("Albums").join("album_a");
        assert!(!publish_dir.exists(), "no publish dir on cancellation");

        run_commit_and_verify(pg_manager, &library_root, import_run_id, true).await;
    }

    #[tokio::test]
    #[ignore]
    async fn fail_injection_idempotent_rerun() {
        let (_tmp, pg_manager, import_run_id, library_root) = setup_full_env().await;

        run_commit_and_verify(pg_manager.clone(), &library_root, import_run_id, true).await;

        run_commit_and_verify(pg_manager.clone(), &library_root, import_run_id, true).await;

        let (client, handle) = {
            let mgr = pg_manager.lock().await;
            mgr.connect().await.unwrap()
        };
        let count: i64 = client.query_one(
            "SELECT COUNT(*) FROM library_albums WHERE display_name = 'album_a'", &[]
        ).await.unwrap().get(0);
        assert_eq!(count, 1, "should have exactly one library album record");
        drop(client);
        handle.abort();
    }
}
