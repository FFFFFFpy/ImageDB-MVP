use crate::domain::import_state::{CommitProgress, ImportRunState};
use crate::state::AppState;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use uuid::Uuid;

const COMMAND_TIMEOUT_SECS: u64 = 90;

#[tokio::test]
#[ignore]
async fn m9_public_recovery_command_path_recovers_after_staging_crash() {
    ensure_postgres_bin();

    let fixture = PublicRecoveryFixture::new().await;
    let app_state = &fixture.app_state;
    let import_run_id = fixture.import_run_id;

    crate::tests::fail_injection::set_fault_point(
        crate::tests::fail_injection::CommitFaultPoint::AfterStagingCopy,
    );
    let started =
        crate::commands::start_import_commit_for_state(app_state, import_run_id.to_string())
            .await
            .unwrap();
    assert_eq!(started, "commit started");
    let failed_progress = poll_commit_terminal(app_state).await;
    crate::tests::fail_injection::clear_fault_point();
    assert_eq!(
        failed_progress.state, "recovery_required",
        "{failed_progress:?}"
    );

    let diagnostics = crate::commands::scan_recoverable_transactions_for_state(app_state)
        .await
        .unwrap();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].import_run_id, import_run_id.to_string());
    assert_ne!(diagnostics[0].current_state, "failed");
    assert_ne!(diagnostics[0].current_state, "cancelled");

    let reverify = crate::commands::reverify_transaction_for_state(
        app_state,
        diagnostics[0].transaction_id.clone(),
    )
    .await
    .unwrap();
    assert_eq!(reverify.verdict, "resume", "{reverify:?}");

    let mut outcome = crate::commands::recover_transaction_for_state(
        app_state,
        diagnostics[0].transaction_id.clone(),
    )
    .await
    .unwrap();
    for _ in 0..5 {
        if outcome.terminal {
            break;
        }
        outcome = crate::commands::recover_transaction_for_state(
            app_state,
            diagnostics[0].transaction_id.clone(),
        )
        .await
        .unwrap();
    }
    assert!(outcome.recovered, "{outcome:?}");
    assert!(outcome.terminal, "{outcome:?}");
    assert_eq!(outcome.final_state, "source_archived");

    let remaining = crate::commands::scan_recoverable_transactions_for_state(app_state)
        .await
        .unwrap();
    assert!(remaining.is_empty(), "{remaining:?}");

    let (client, handle) = {
        let mgr = app_state.postgres_manager.lock().await;
        mgr.connect().await.unwrap()
    };
    let run_state: String = client
        .query_one(
            "SELECT state FROM import_runs WHERE id = $1",
            &[&import_run_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(run_state, "completed");
    let library_images: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM library_images li
             JOIN library_albums la ON la.id = li.album_id
             WHERE la.relative_path = 'album_a'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(library_images, 1);
    handle.abort();

    let mut mgr = app_state.postgres_manager.lock().await;
    mgr.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn m9_public_recovery_command_matrix_recovers_crash_points() {
    ensure_postgres_bin();

    for fault in [
        crate::tests::fail_injection::CommitFaultPoint::AfterDbWrite,
        crate::tests::fail_injection::CommitFaultPoint::AfterStagingCopy,
        crate::tests::fail_injection::CommitFaultPoint::AfterManifestWrite,
        crate::tests::fail_injection::CommitFaultPoint::AfterPublishRename,
        crate::tests::fail_injection::CommitFaultPoint::BeforeSourceArchive,
    ] {
        let fixture = PublicRecoveryFixture::new().await;
        let app_state = &fixture.app_state;

        crate::tests::fail_injection::set_fault_point(fault);
        let started = crate::commands::start_import_commit_for_state(
            app_state,
            fixture.import_run_id.to_string(),
        )
        .await
        .unwrap();
        assert_eq!(started, "commit started");
        let failed_progress = poll_commit_terminal(app_state).await;
        crate::tests::fail_injection::clear_fault_point();
        assert_eq!(
            failed_progress.state, "recovery_required",
            "fault {fault:?} should route to recovery: {failed_progress:?}"
        );

        recover_all_public_transactions(app_state, fixture.import_run_id).await;
        assert_completed_with_one_library_image(app_state, fixture.import_run_id).await;

        let mut mgr = app_state.postgres_manager.lock().await;
        mgr.shutdown().await.unwrap();
    }
}

#[tokio::test]
#[ignore]
async fn m9_public_recovery_cancel_before_prewrite_leaves_committable_cancelled_run() {
    ensure_postgres_bin();

    let fixture = PublicRecoveryFixture::new().await;
    let app_state = &fixture.app_state;

    let started = crate::commands::start_import_commit_for_state(
        app_state,
        fixture.import_run_id.to_string(),
    )
    .await
    .unwrap();
    assert_eq!(started, "commit started");
    let cancelled = crate::commands::cancel_import_commit_for_state(app_state)
        .await
        .unwrap();
    assert_eq!(cancelled, "commit cancellation requested");

    let progress = poll_commit_terminal(app_state).await;
    assert_eq!(
        progress.state, "cancelled",
        "cancel-before-prewrite should not create a recovery dead end: {progress:?}"
    );

    let diagnostics = crate::commands::scan_recoverable_transactions_for_state(app_state)
        .await
        .unwrap();
    assert!(
        diagnostics.is_empty(),
        "cancel-before-prewrite should leave no recovery work: {diagnostics:?}"
    );

    let latest = crate::commands::get_latest_committable_import_run_for_state(app_state)
        .await
        .unwrap();
    assert_eq!(latest, Some(fixture.import_run_id.to_string()));

    let mut mgr = app_state.postgres_manager.lock().await;
    mgr.shutdown().await.unwrap();
}

struct PublicRecoveryFixture {
    _tmp: TempDir,
    app_state: AppState,
    import_run_id: Uuid,
}

impl PublicRecoveryFixture {
    async fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let app_data = tmp.path().join("app_data");
        let fixture_dir = tmp.path().join("fixtures");
        let source_root = tmp.path().join("source");
        let album_dir = source_root.join("album_a");
        let library_root = tmp.path().join("library");
        std::fs::create_dir_all(&album_dir).unwrap();
        std::fs::create_dir_all(&library_root).unwrap();
        std::fs::create_dir_all(&fixture_dir).unwrap();
        write_test_png(&album_dir.join("first.png"), [16, 96, 180]);

        let app_state = AppState::new(&app_data, fixture_dir).unwrap();
        crate::commands::initialize_managed_database_for_state(&app_state)
            .await
            .unwrap();
        crate::commands::update_settings_for_state(
            &app_state,
            crate::commands::SettingsDto {
                database_mode: Some("managed".to_string()),
                library_root: Some(library_root.display().to_string()),
                external_host: None,
                external_port: None,
                external_database: None,
                external_username: None,
                external_tls_mode: None,
                external_ca_cert_path: None,
                external_client_cert_path: None,
                external_client_key_path: None,
                external_connect_timeout_secs: None,
                external_query_timeout_secs: None,
                external_profile_name: None,
                first_run_completed: true,
            },
        )
        .await
        .unwrap();

        crate::commands::start_scan_for_state(&app_state, source_root.display().to_string())
            .await
            .unwrap();
        let scan = poll_scan_terminal(&app_state).await;
        assert_eq!(
            scan.state,
            ImportRunState::ReadyToCommit.to_string(),
            "{scan:?}"
        );
        let import_run_id = Uuid::parse_str(scan.import_run_id.as_deref().unwrap()).unwrap();

        let plan =
            crate::commands::generate_import_plan_for_state(&app_state, import_run_id.to_string())
                .await
                .unwrap();
        assert_eq!(plan.kept_images.len(), 1);

        Self {
            _tmp: tmp,
            app_state,
            import_run_id,
        }
    }
}

async fn poll_scan_terminal(app_state: &AppState) -> crate::domain::import_state::ScanProgress {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(COMMAND_TIMEOUT_SECS);
    loop {
        let progress = crate::commands::get_scan_progress_for_state(app_state)
            .await
            .unwrap();
        if matches!(
            progress.state.as_str(),
            "ready_to_commit" | "review_required" | "completed" | "cancelled" | "failed"
        ) {
            return progress;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for scan progress; last={progress:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

async fn recover_all_public_transactions(app_state: &AppState, import_run_id: Uuid) {
    let diagnostics = crate::commands::scan_recoverable_transactions_for_state(app_state)
        .await
        .unwrap();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].import_run_id, import_run_id.to_string());

    let reverify = crate::commands::reverify_transaction_for_state(
        app_state,
        diagnostics[0].transaction_id.clone(),
    )
    .await
    .unwrap();
    assert!(
        matches!(reverify.verdict.as_str(), "resume" | "already_committed"),
        "{reverify:?}"
    );

    let mut outcome = crate::commands::recover_transaction_for_state(
        app_state,
        diagnostics[0].transaction_id.clone(),
    )
    .await
    .unwrap();
    for _ in 0..6 {
        if outcome.terminal {
            break;
        }
        outcome = crate::commands::recover_transaction_for_state(
            app_state,
            diagnostics[0].transaction_id.clone(),
        )
        .await
        .unwrap();
    }
    assert!(outcome.recovered, "{outcome:?}");
    assert!(outcome.terminal, "{outcome:?}");
    assert_eq!(outcome.final_state, "source_archived");

    let remaining = crate::commands::scan_recoverable_transactions_for_state(app_state)
        .await
        .unwrap();
    assert!(remaining.is_empty(), "{remaining:?}");
}

async fn assert_completed_with_one_library_image(app_state: &AppState, import_run_id: Uuid) {
    let (client, handle) = {
        let mgr = app_state.postgres_manager.lock().await;
        mgr.connect().await.unwrap()
    };
    let run_state: String = client
        .query_one(
            "SELECT state FROM import_runs WHERE id = $1",
            &[&import_run_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(run_state, "completed");
    let library_images: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM library_images li
             JOIN library_albums la ON la.id = li.album_id
             WHERE la.relative_path = 'album_a'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(library_images, 1);
    handle.abort();
}

async fn poll_commit_terminal(app_state: &AppState) -> CommitProgress {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(COMMAND_TIMEOUT_SECS);
    loop {
        let progress = crate::commands::get_commit_progress_for_state(app_state)
            .await
            .unwrap();
        if matches!(
            progress.state.as_str(),
            "completed" | "recovery_required" | "failed" | "cancelled"
        ) {
            return progress;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for commit progress; last={progress:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

fn write_test_png(path: &Path, color: [u8; 3]) {
    let mut img = image::RgbImage::new(16, 16);
    for pixel in img.pixels_mut() {
        *pixel = image::Rgb(color);
    }
    img.save(path).unwrap();
}

fn ensure_postgres_bin() -> bool {
    if std::env::var("IMAGEDB_POSTGRES_BIN")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
    {
        return true;
    }
    let candidate = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .join(".local")
        .join("db-tools")
        .join("postgresql-18.4")
        .join("pgsql")
        .join("bin");
    if candidate.join("postgres.exe").is_file() {
        std::env::set_var("IMAGEDB_POSTGRES_BIN", candidate);
        return true;
    }
    panic!(
        "IMAGEDB_POSTGRES_BIN is not set and no bundled test PostgreSQL was found.\n\
         Expected one of:\n  \
           - IMAGEDB_POSTGRES_BIN env var pointing at a pgsql/bin directory\n  \
           - {} containing postgres.exe\n\
         Run `node scripts/package-postgres-runtime.mjs` to populate the packaged runtime, or\n\
         set IMAGEDB_POSTGRES_BIN to a local PostgreSQL 18.x bin directory.\n\
         (candidate checked: {})",
        candidate.display(),
        candidate.display()
    );
}
