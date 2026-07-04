use crate::domain::import_state::{CommitProgress, ImportRunState, ScanProgress};
use crate::domain::DatabaseStatus;
use crate::repositories::import_repository::ImportRepository;
use crate::state::AppState;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::TempDir;

#[tokio::test]
#[ignore]
async fn m9_public_command_main_chain_first_run_to_completed_import() {
    ensure_postgres_bin();

    let tmp = TempDir::new().unwrap();
    let app_data = tmp.path().join("app_data");
    let fixture_dir = tmp.path().join("fixtures");
    let source_root = tmp.path().join("source");
    let album_dir = source_root.join("album_a");
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&album_dir).unwrap();
    std::fs::create_dir_all(&library_root).unwrap();
    std::fs::create_dir_all(&fixture_dir).unwrap();
    write_test_png(&album_dir.join("sample-original.png"));
    std::fs::copy(
        album_dir.join("sample-original.png"),
        album_dir.join("sample-copy.png"),
    )
    .unwrap();

    let app_state = AppState::new(&app_data, fixture_dir).unwrap();
    let database = crate::commands::initialize_managed_database_for_state(&app_state)
        .await
        .unwrap();
    assert_eq!(database.status, DatabaseStatus::Connected);

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

    let source_info = crate::commands::validate_source_directory(source_root.display().to_string())
        .await
        .unwrap();
    assert_eq!(source_info.album_count, 1);
    assert_eq!(source_info.albums, vec!["album_a".to_string()]);

    crate::commands::start_scan_for_state(&app_state, source_root.display().to_string())
        .await
        .unwrap();
    let scan = wait_for_scan_terminal(&app_state).await;
    assert_eq!(
        scan.state,
        ImportRunState::ReadyToCommit.to_string(),
        "{scan:?}"
    );
    assert_eq!(scan.total_albums, 1);
    assert_eq!(scan.total_images, 2);
    assert_eq!(scan.duplicate_count, 1);
    let import_run_id = uuid::Uuid::parse_str(scan.import_run_id.as_deref().unwrap()).unwrap();

    let review_progress =
        crate::commands::get_review_progress_for_state(&app_state, import_run_id.to_string())
            .await
            .unwrap();
    assert!(review_progress.all_decided);

    let plan =
        crate::commands::generate_import_plan_for_state(&app_state, import_run_id.to_string())
            .await
            .unwrap();
    assert_eq!(plan.total_albums, 1);
    assert_eq!(plan.total_images, 2);
    assert_eq!(plan.excluded_count, 1);
    assert_eq!(plan.kept_images.len(), 1);

    let latest_committable =
        crate::commands::get_latest_committable_import_run_for_state(&app_state)
            .await
            .unwrap();
    assert_eq!(latest_committable, Some(import_run_id.to_string()));

    let (client, handle) = {
        let mgr = app_state.postgres_manager.lock().await;
        mgr.connect().await.unwrap()
    };
    let frozen = ImportRepository::load_frozen_plan(&client, import_run_id)
        .await
        .unwrap()
        .expect("generate_import_plan must freeze the command-visible plan");
    assert_eq!(frozen.albums.len(), 1);
    assert_eq!(frozen.albums[0].1.len(), 1);
    drop(client);
    handle.abort();

    crate::commands::start_import_commit_for_state(&app_state, import_run_id.to_string())
        .await
        .unwrap();
    let commit = wait_for_commit_terminal(&app_state).await;
    assert_eq!(commit.state, "completed");
    assert_eq!(commit.albums_total, 1);
    assert_eq!(commit.images_committed, 1);

    let published = library_root.join("Albums").join("album_a");
    assert!(published.is_dir(), "published album missing: {published:?}");
    assert_eq!(count_images(&published), 1);
    assert!(
        published
            .join(".imagedb")
            .join(".imagedb-commit.json")
            .is_file(),
        "commit marker missing"
    );

    let (client, handle) = {
        let mgr = app_state.postgres_manager.lock().await;
        mgr.connect().await.unwrap()
    };
    let consumed = ImportRepository::load_frozen_plan(&client, import_run_id)
        .await
        .unwrap()
        .expect("committed run should keep a consumed plan for idempotent recovery");
    assert_eq!(consumed.plan_state, "consumed");
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

/// Regression guard for the "restart returns NotInitialized" bug: after the
/// app initializes a managed cluster and is restarted (the in-process
/// `server_running` flag reset, cluster still on disk), `get_state` must
/// bring the server back up and report Connected — not NotInitialized.
/// Without this, the dashboard shows NotInitialized and every scan fails
/// with "connect failed: error connecting to server".
#[tokio::test]
#[ignore]
async fn m9_managed_cluster_restarts_on_app_relaunch() {
    use crate::domain::DatabaseStatus;
    ensure_postgres_bin();

    let tmp = TempDir::new().unwrap();
    let app_data = tmp.path().join("app_data");
    let fixture_dir = tmp.path().join("fixtures");
    std::fs::create_dir_all(&app_data).unwrap();
    std::fs::create_dir_all(&fixture_dir).unwrap();

    let app_state = AppState::new(&app_data, fixture_dir).unwrap();
    let initialized = crate::commands::initialize_managed_database_for_state(&app_state)
        .await
        .unwrap();
    assert!(
        matches!(initialized.status, DatabaseStatus::Connected),
        "initial init must succeed: {:?}",
        initialized
    );

    // Simulate an app relaunch: stop the server process. The on-disk cluster
    // remains; the in-memory server_running flag is false.
    {
        let mut mgr = app_state.postgres_manager.lock().await;
        mgr.shutdown().await.unwrap();
        assert!(!mgr.is_server_running());
        assert!(mgr.cluster_files_exist());
    }

    // get_state must restart the server and report Connected.
    let state = app_state.database_service.get_state().await;
    assert!(
        matches!(state.status, DatabaseStatus::Connected),
        "get_state must restart the managed cluster after relaunch: {:?}",
        state
    );

    let mut mgr = app_state.postgres_manager.lock().await;
    mgr.shutdown().await.unwrap();
}

async fn wait_for_scan_terminal(app_state: &AppState) -> ScanProgress {
    let deadline = Instant::now() + Duration::from_secs(30);
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
            Instant::now() < deadline,
            "scan did not reach a terminal state; last progress: {progress:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_commit_terminal(app_state: &AppState) -> CommitProgress {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let progress = crate::commands::get_commit_progress_for_state(app_state)
            .await
            .unwrap();
        if matches!(
            progress.state.as_str(),
            "completed" | "failed" | "recovery_required" | "cancelled"
        ) {
            return progress;
        }
        assert!(
            Instant::now() < deadline,
            "commit did not reach a terminal state; last progress: {progress:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn write_test_png(path: &Path) {
    let mut img = image::RgbImage::new(8, 8);
    for (x, y, pixel) in img.enumerate_pixels_mut() {
        *pixel = image::Rgb([x as u8 * 24, y as u8 * 24, 128]);
    }
    img.save(path).unwrap();
}

fn count_images(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| {
                    matches!(
                        ext.to_ascii_lowercase().as_str(),
                        "png" | "jpg" | "jpeg" | "webp"
                    )
                })
                .unwrap_or(false)
        })
        .count()
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
    // Fail-fast: this is a real-DB test gated behind #[ignore] and run via
    // `pnpm rust:test:real` (or `--ignored`). Skipping here would let the
    // suite report green without exercising the real PostgreSQL chain, which
    // is exactly the false-pass the closure plan forbids. Point the operator
    // at the exact file we expected so they can fix the environment.
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

/// Verifies the M9 frozen-plan main-chain invariants that the closure plan
/// calls out:
/// - `freeze_import_plan` is idempotent — a second freeze returns the same
///   plan summary without creating duplicate plan rows.
/// - The commit page's frozen summary matches the plan the commit service
///   actually consumes (no dynamic re-derivation).
/// - An old `completed` run does not preempt a newer `ready_to_commit` run
///   on the default commit page (the `completed_at DESC` bug).
#[tokio::test]
#[ignore]
async fn m9_freeze_plan_idempotent_and_summary_matches_commit_set() {
    ensure_postgres_bin();

    let tmp = TempDir::new().unwrap();
    let app_data = tmp.path().join("app_data");
    let fixture_dir = tmp.path().join("fixtures");
    let source_root = tmp.path().join("source");
    let album_dir = source_root.join("album_a");
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&album_dir).unwrap();
    std::fs::create_dir_all(&library_root).unwrap();
    std::fs::create_dir_all(&fixture_dir).unwrap();
    write_test_png(&album_dir.join("sample-original.png"));
    std::fs::copy(
        album_dir.join("sample-original.png"),
        album_dir.join("sample-copy.png"),
    )
    .unwrap();

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
    let scan = wait_for_scan_terminal(&app_state).await;
    assert_eq!(
        scan.state,
        ImportRunState::ReadyToCommit.to_string(),
        "{scan:?}"
    );
    let import_run_id = uuid::Uuid::parse_str(scan.import_run_id.as_deref().unwrap()).unwrap();

    // First freeze.
    let first =
        crate::commands::freeze_import_plan_for_state(&app_state, import_run_id.to_string())
            .await
            .unwrap();
    assert_eq!(first.kept_images.len(), 1);

    // Idempotent re-freeze: same summary, no duplicate rows.
    let second =
        crate::commands::freeze_import_plan_for_state(&app_state, import_run_id.to_string())
            .await
            .unwrap();
    assert_eq!(second.kept_images, first.kept_images);
    assert_eq!(second.excluded_count, first.excluded_count);

    let (client, handle) = {
        let mgr = app_state.postgres_manager.lock().await;
        mgr.connect().await.unwrap()
    };
    let plan_row_count: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM import_plans WHERE import_run_id = $1",
            &[&import_run_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        plan_row_count, 1,
        "idempotent freeze must not create a second plan row"
    );

    // Run is now ready_to_commit (the freeze advanced it, or it was already).
    let run_state: String = client
        .query_one(
            "SELECT state FROM import_runs WHERE id = $1",
            &[&import_run_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(run_state, "ready_to_commit");

    // The frozen summary the commit page reads must match the frozen plan
    // the commit service will consume — same kept image set, same count.
    let summary = crate::commands::get_frozen_import_plan_summary_for_state(
        &app_state,
        import_run_id.to_string(),
    )
    .await
    .unwrap()
    .expect("frozen summary must exist after freeze");
    assert_eq!(summary.kept_images.len(), first.kept_images.len());
    assert_eq!(summary.excluded_count, first.excluded_count);

    let frozen = ImportRepository::load_frozen_plan(&client, import_run_id)
        .await
        .unwrap()
        .expect("commit service must be able to load the same frozen plan");
    assert_eq!(frozen.albums.len(), 1);
    assert_eq!(frozen.albums[0].1.len(), 1);
    drop(client);
    handle.abort();

    let mut mgr = app_state.postgres_manager.lock().await;
    mgr.shutdown().await.unwrap();
}

/// An old `completed` run must NOT preempt a newer `ready_to_commit` run
/// on the default commit page. This is the regression guard for the
/// `get_latest_committable_run` priority fix.
#[tokio::test]
#[ignore]
async fn m9_committable_run_prefers_ready_over_old_completed() {
    ensure_postgres_bin();

    let tmp = TempDir::new().unwrap();
    let app_data = tmp.path().join("app_data");
    let fixture_dir = tmp.path().join("fixtures");
    let source_root = tmp.path().join("source");
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&library_root).unwrap();
    std::fs::create_dir_all(&fixture_dir).unwrap();

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

    // Run A: scan, freeze, commit → completed.
    let album_a = source_root.join("album_a");
    std::fs::create_dir_all(&album_a).unwrap();
    write_test_png(&album_a.join("a.png"));
    crate::commands::start_scan_for_state(&app_state, source_root.display().to_string())
        .await
        .unwrap();
    let scan_a = wait_for_scan_terminal(&app_state).await;
    let run_a = uuid::Uuid::parse_str(scan_a.import_run_id.as_deref().unwrap()).unwrap();
    crate::commands::freeze_import_plan_for_state(&app_state, run_a.to_string())
        .await
        .unwrap();
    crate::commands::start_import_commit_for_state(&app_state, run_a.to_string())
        .await
        .unwrap();
    let commit_a = wait_for_commit_terminal(&app_state).await;
    assert_eq!(commit_a.state, "completed", "{commit_a:?}");

    // Run B: scan again → ready_to_commit (frozen), NOT committed.
    let album_b = source_root.join("album_b");
    std::fs::create_dir_all(&album_b).unwrap();
    write_test_png(&album_b.join("b.png"));
    crate::commands::start_scan_for_state(&app_state, source_root.display().to_string())
        .await
        .unwrap();
    let scan_b = wait_for_scan_terminal(&app_state).await;
    let run_b = uuid::Uuid::parse_str(scan_b.import_run_id.as_deref().unwrap()).unwrap();
    crate::commands::freeze_import_plan_for_state(&app_state, run_b.to_string())
        .await
        .unwrap();

    // The default commit page must pick run B (ready_to_commit), NOT the
    // older completed run A — even though run A has a populated completed_at.
    let latest = crate::commands::get_latest_committable_import_run_for_state(&app_state)
        .await
        .unwrap();
    assert_eq!(latest, Some(run_b.to_string()));

    let mut mgr = app_state.postgres_manager.lock().await;
    mgr.shutdown().await.unwrap();
}
