use crate::domain::import_state::{CommitProgress, ImportRunState, ScanProgress};
use crate::infrastructure::settings::AppSettings;
use crate::services::{review_service, scan_service};
use crate::state::AppState;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::Mutex;

const COMMAND_TIMEOUT_SECS: u64 = 90;

#[tokio::test]
#[ignore]
async fn m9_public_recovery_command_path_recovers_after_staging_crash() {
    if !ensure_postgres_bin() {
        eprintln!("IMAGEDB_POSTGRES_BIN not set and no bundled test PostgreSQL found; skipping");
        return;
    }

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
    write_test_png(&album_dir.join("second.png"), [180, 96, 16]);

    let app_state = AppState::new(&app_data, fixture_dir).unwrap();
    app_state
        .database_service
        .initialize_managed()
        .await
        .unwrap();
    {
        let mut settings = app_state.settings.lock().await;
        settings
            .update(AppSettings {
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
            })
            .unwrap();
    }

    let scan = scan_service::run_scan(
        app_state.postgres_manager.clone(),
        app_state.settings.clone(),
        source_root.display().to_string(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(Mutex::new(ScanProgress::idle())),
    )
    .await
    .unwrap();
    assert_eq!(
        scan.state,
        ImportRunState::ReadyToCommit.to_string(),
        "{scan:?}"
    );
    let import_run_id = uuid::Uuid::parse_str(scan.import_run_id.as_deref().unwrap()).unwrap();

    {
        let (client, handle) = {
            let mgr = app_state.postgres_manager.lock().await;
            mgr.connect().await.unwrap()
        };
        let plan = review_service::generate_import_plan(&client, import_run_id)
            .await
            .unwrap();
        assert_eq!(plan.kept_images.len(), 1);
        handle.abort();
    }

    crate::tests::fail_injection::set_fault_point(
        crate::tests::fail_injection::CommitFaultPoint::AfterStagingCopy,
    );
    let started =
        crate::commands::start_import_commit_for_state(&app_state, import_run_id.to_string())
            .await
            .unwrap();
    assert_eq!(started, "commit started");
    let failed_progress = poll_commit_terminal(&app_state).await;
    crate::tests::fail_injection::clear_fault_point();
    assert_eq!(
        failed_progress.state, "recovery_required",
        "{failed_progress:?}"
    );

    let diagnostics = crate::commands::scan_recoverable_transactions_for_state(&app_state)
        .await
        .unwrap();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].import_run_id, import_run_id.to_string());
    assert_ne!(diagnostics[0].current_state, "failed");
    assert_ne!(diagnostics[0].current_state, "cancelled");

    let reverify = crate::commands::reverify_transaction_for_state(
        &app_state,
        diagnostics[0].transaction_id.clone(),
    )
    .await
    .unwrap();
    assert_eq!(reverify.verdict, "resume", "{reverify:?}");

    let mut outcome = crate::commands::recover_transaction_for_state(
        &app_state,
        diagnostics[0].transaction_id.clone(),
    )
    .await
    .unwrap();
    for _ in 0..5 {
        if outcome.terminal {
            break;
        }
        outcome = crate::commands::recover_transaction_for_state(
            &app_state,
            diagnostics[0].transaction_id.clone(),
        )
        .await
        .unwrap();
    }
    assert!(outcome.recovered, "{outcome:?}");
    assert!(outcome.terminal, "{outcome:?}");
    assert_eq!(outcome.final_state, "source_archived");

    let remaining = crate::commands::scan_recoverable_transactions_for_state(&app_state)
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
    false
}
