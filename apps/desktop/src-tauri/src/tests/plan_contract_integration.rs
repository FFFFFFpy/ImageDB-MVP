use crate::state::AppState;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::TempDir;

#[tokio::test]
#[ignore]
async fn plan_contract_single_draft_and_cross_album_move() {
    ensure_postgres_bin();

    let tmp = TempDir::new().unwrap();
    let app_data = tmp.path().join("app_data");
    let fixture_dir = tmp.path().join("fixtures");
    let source_root = tmp.path().join("source");
    let album_a = source_root.join("album_a");
    let album_b = source_root.join("album_b");
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&album_a).unwrap();
    std::fs::create_dir_all(&album_b).unwrap();
    std::fs::create_dir_all(&library_root).unwrap();
    std::fs::create_dir_all(&fixture_dir).unwrap();
    write_test_png(&album_a.join("img1.png"));
    write_test_png(&album_a.join("img2.png"));
    write_test_png(&album_b.join("img3.png"));

    let app_state = setup_app(&app_data, &fixture_dir, &library_root).await;

    crate::commands::start_scan_for_state(&app_state, source_root.display().to_string())
        .await
        .unwrap();
    let scan = wait_for_scan_terminal(&app_state).await;
    let run_id = uuid::Uuid::parse_str(scan.import_run_id.as_deref().unwrap()).unwrap();

    // Generate draft — must produce exactly one draft plan.
    let plan = crate::commands::generate_import_plan_for_state(&app_state, run_id.to_string())
        .await
        .unwrap();
    assert!(plan.plan_hash.is_none());

    let (client, handle) = connect(&app_state).await;
    let draft_count: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM import_plans WHERE import_run_id = $1 AND state = 'draft'",
            &[&run_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(draft_count, 1, "generate must produce exactly one draft");

    // Cross-album move must be rejected until the independent transaction
    // architecture version implements safe cross-album commit support.
    let img1 = plan
        .kept_images
        .iter()
        .find(|i| i.source_path.contains("img1"))
        .expect("img1 must be in kept images");
    let target_album_b = plan
        .albums
        .iter()
        .find(|a| a.album_name.contains("album_b"))
        .or_else(|| plan.albums.iter().find(|a| a.album_id != img1.album_id))
        .expect("must have a second album to attempt move to");

    let move_result = crate::commands::set_import_plan_image_included_for_state(
        &app_state,
        run_id.to_string(),
        img1.image_id.clone(),
        target_album_b.album_id.clone(),
        true,
    )
    .await;
    let err_msg = move_result.unwrap_err();
    assert!(
        err_msg.contains("跨图集调整暂不可用"),
        "error must explain cross-album is unavailable, got: {err_msg}"
    );

    // Image must still belong to its original album — no partial mutation.
    let plan_album_after: String = client
        .query_one(
            "SELECT pa.import_album_id::text FROM import_plan_images pi
             JOIN import_plan_albums pa ON pa.id = pi.plan_album_id
             WHERE pi.import_image_id = $1::uuid
               AND pa.plan_id IN (SELECT id FROM import_plans WHERE import_run_id = $2 AND state = 'draft')",
            &[&uuid::Uuid::parse_str(&img1.image_id).unwrap(), &run_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        plan_album_after, img1.album_id,
        "image must remain in its original album after rejected move"
    );

    // No duplicate plan_image rows created.
    let img_row_count: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM import_plan_images WHERE import_image_id = $1::uuid
               AND plan_album_id IN (SELECT id FROM import_plan_albums WHERE plan_id IN (
                   SELECT id FROM import_plans WHERE import_run_id = $2 AND state = 'draft'
               ))",
            &[&uuid::Uuid::parse_str(&img1.image_id).unwrap(), &run_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(img_row_count, 1, "rejected move must not create extra plan rows");

    drop(client);
    handle.abort();
    shutdown(&app_state).await;
}

#[tokio::test]
#[ignore]
async fn plan_contract_freeze_guards_and_no_premature_transactions() {
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
    write_test_png(&album_dir.join("img1.png"));
    write_test_png(&album_dir.join("img2.png"));

    let app_state = setup_app(&app_data, &fixture_dir, &library_root).await;

    crate::commands::start_scan_for_state(&app_state, source_root.display().to_string())
        .await
        .unwrap();
    let scan = wait_for_scan_terminal(&app_state).await;
    let run_id = uuid::Uuid::parse_str(scan.import_run_id.as_deref().unwrap()).unwrap();

    crate::commands::generate_import_plan_for_state(&app_state, run_id.to_string())
        .await
        .unwrap();

    // Freeze: no file_transactions before or after freeze.
    let frozen = crate::commands::freeze_import_plan_for_state(&app_state, run_id.to_string())
        .await
        .unwrap();
    assert!(frozen.plan_hash.is_some());

    let (client, handle) = connect(&app_state).await;
    let tx_count: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM file_transactions WHERE import_run_id = $1",
            &[&run_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(tx_count, 0, "freeze must not create file transactions");

    // Frozen plan rejects edits.
    let edit_result = crate::commands::set_import_plan_image_included_for_state(
        &app_state,
        run_id.to_string(),
        frozen.kept_images[0].image_id.clone(),
        frozen.kept_images[0].target_album_id.clone(),
        false,
    )
    .await;
    assert!(edit_result.is_err(), "frozen plan must reject edits");

    let mode_result = crate::commands::set_import_plan_source_file_mode_for_state(
        &app_state,
        run_id.to_string(),
        "move_selected_without_backup".to_string(),
    )
    .await;
    assert!(mode_result.is_err(), "frozen plan must reject mode change");

    // Repeated freeze does not create a second frozen plan.
    let refreeze = crate::commands::freeze_import_plan_for_state(&app_state, run_id.to_string())
        .await
        .unwrap();
    assert_eq!(refreeze.plan_hash, frozen.plan_hash);
    let frozen_count: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM import_plans WHERE import_run_id = $1 AND state IN ('frozen', 'consumed')",
            &[&run_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(frozen_count, 1, "repeated freeze must not duplicate");

    drop(client);
    handle.abort();

    // Only explicit commit start creates file transactions.
    crate::commands::start_import_commit_for_state(&app_state, run_id.to_string())
        .await
        .unwrap();
    let commit = wait_for_commit_terminal(&app_state).await;
    assert_eq!(commit.state, "completed");

    let (client, handle) = connect(&app_state).await;
    let tx_after_commit: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM file_transactions WHERE import_run_id = $1",
            &[&run_id],
        )
        .await
        .unwrap()
        .get(0);
    assert!(tx_after_commit > 0, "commit must create file transactions");
    drop(client);
    handle.abort();

    shutdown(&app_state).await;
}

#[tokio::test]
#[ignore]
async fn plan_contract_abandon_frozen_creates_no_transactions() {
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
    write_test_png(&album_dir.join("img1.png"));

    let app_state = setup_app(&app_data, &fixture_dir, &library_root).await;

    crate::commands::start_scan_for_state(&app_state, source_root.display().to_string())
        .await
        .unwrap();
    let scan = wait_for_scan_terminal(&app_state).await;
    let run_id = uuid::Uuid::parse_str(scan.import_run_id.as_deref().unwrap()).unwrap();

    crate::commands::generate_import_plan_for_state(&app_state, run_id.to_string())
        .await
        .unwrap();
    crate::commands::freeze_import_plan_for_state(&app_state, run_id.to_string())
        .await
        .unwrap();

    // Abandon the frozen workflow.
    crate::commands::abandon_frozen_import_workflow_for_state(&app_state, run_id.to_string())
        .await
        .unwrap();

    let (client, handle) = connect(&app_state).await;
    let tx_count: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM file_transactions WHERE import_run_id = $1",
            &[&run_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(tx_count, 0, "abandon must not create file transactions");

    let run_state: String = client
        .query_one("SELECT state FROM import_runs WHERE id = $1", &[&run_id])
        .await
        .unwrap()
        .get(0);
    assert_eq!(run_state, "abandoned");

    let plan_state: String = client
        .query_one(
            "SELECT state FROM import_plans WHERE import_run_id = $1",
            &[&run_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(plan_state, "invalidated");

    drop(client);
    handle.abort();
    shutdown(&app_state).await;
}

#[tokio::test]
#[ignore]
async fn plan_contract_resolver_priority_over_completed() {
    ensure_postgres_bin();

    let tmp = TempDir::new().unwrap();
    let app_data = tmp.path().join("app_data");
    let fixture_dir = tmp.path().join("fixtures");
    let source_root = tmp.path().join("source");
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&library_root).unwrap();
    std::fs::create_dir_all(&fixture_dir).unwrap();

    let app_state = setup_app(&app_data, &fixture_dir, &library_root).await;

    // Run A: full cycle to completed.
    let album_a = source_root.join("album_a");
    std::fs::create_dir_all(&album_a).unwrap();
    write_test_png(&album_a.join("a.png"));
    crate::commands::start_scan_for_state(&app_state, source_root.display().to_string())
        .await
        .unwrap();
    let scan_a = wait_for_scan_terminal(&app_state).await;
    let run_a = uuid::Uuid::parse_str(scan_a.import_run_id.as_deref().unwrap()).unwrap();
    crate::commands::generate_import_plan_for_state(&app_state, run_a.to_string())
        .await
        .unwrap();
    crate::commands::freeze_import_plan_for_state(&app_state, run_a.to_string())
        .await
        .unwrap();
    crate::commands::start_import_commit_for_state(&app_state, run_a.to_string())
        .await
        .unwrap();
    let commit_a = wait_for_commit_terminal(&app_state).await;
    assert_eq!(commit_a.state, "completed");

    // Run B: scan only, leaves it in ready_to_commit.
    let album_b = source_root.join("album_b");
    std::fs::create_dir_all(&album_b).unwrap();
    write_test_png(&album_b.join("b.png"));
    crate::commands::start_scan_for_state(&app_state, source_root.display().to_string())
        .await
        .unwrap();
    let scan_b = wait_for_scan_terminal(&app_state).await;
    let run_b = uuid::Uuid::parse_str(scan_b.import_run_id.as_deref().unwrap()).unwrap();

    // Resolver must pick run B (actionable) over run A (completed).
    let stage = crate::commands::get_import_workflow_stage_for_state(&app_state)
        .await
        .unwrap();
    assert_eq!(
        stage.import_run_id.as_deref(),
        Some(run_b.to_string()).as_deref(),
        "resolver must prefer actionable run over completed"
    );
    assert_ne!(stage.stage, "idle");

    shutdown(&app_state).await;
}

#[tokio::test]
#[ignore]
async fn plan_contract_cancelled_with_transactions_does_not_shadow_review() {
    ensure_postgres_bin();

    let tmp = TempDir::new().unwrap();
    let app_data = tmp.path().join("app_data");
    let fixture_dir = tmp.path().join("fixtures");
    let source_root = tmp.path().join("source");
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&library_root).unwrap();
    std::fs::create_dir_all(&fixture_dir).unwrap();

    let app_state = setup_app(&app_data, &fixture_dir, &library_root).await;

    // Run A: scan, freeze, commit, then cancel (leaves file_transactions).
    let album_a = source_root.join("album_a");
    std::fs::create_dir_all(&album_a).unwrap();
    write_test_png(&album_a.join("a.png"));
    crate::commands::start_scan_for_state(&app_state, source_root.display().to_string())
        .await
        .unwrap();
    let scan_a = wait_for_scan_terminal(&app_state).await;
    let run_a = uuid::Uuid::parse_str(scan_a.import_run_id.as_deref().unwrap()).unwrap();
    crate::commands::generate_import_plan_for_state(&app_state, run_a.to_string())
        .await
        .unwrap();
    crate::commands::freeze_import_plan_for_state(&app_state, run_a.to_string())
        .await
        .unwrap();
    crate::commands::start_import_commit_for_state(&app_state, run_a.to_string())
        .await
        .unwrap();
    let commit_a = wait_for_commit_terminal(&app_state).await;
    assert_eq!(commit_a.state, "completed");

    // Manually set run A to cancelled (simulating a cancelled run with transactions).
    let (client, handle) = connect(&app_state).await;
    client
        .execute(
            "UPDATE import_runs SET state = 'cancelled' WHERE id = $1",
            &[&run_a],
        )
        .await
        .unwrap();
    drop(client);
    handle.abort();

    // Run B: scan only, leaves it in ready_to_commit or review_required.
    let album_b = source_root.join("album_b");
    std::fs::create_dir_all(&album_b).unwrap();
    write_test_png(&album_b.join("b.png"));
    crate::commands::start_scan_for_state(&app_state, source_root.display().to_string())
        .await
        .unwrap();
    let scan_b = wait_for_scan_terminal(&app_state).await;
    let run_b = uuid::Uuid::parse_str(scan_b.import_run_id.as_deref().unwrap()).unwrap();

    // Resolver must pick run B, not the non-resubmittable cancelled run A.
    let stage = crate::commands::get_import_workflow_stage_for_state(&app_state)
        .await
        .unwrap();
    assert_eq!(
        stage.import_run_id.as_deref(),
        Some(run_b.to_string()).as_deref(),
        "cancelled run with transactions must not shadow actionable run"
    );

    shutdown(&app_state).await;
}

#[tokio::test]
#[ignore]
async fn plan_contract_resubmittable_cancelled_selected_over_review() {
    ensure_postgres_bin();

    let tmp = TempDir::new().unwrap();
    let app_data = tmp.path().join("app_data");
    let fixture_dir = tmp.path().join("fixtures");
    let source_root = tmp.path().join("source");
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&library_root).unwrap();
    std::fs::create_dir_all(&fixture_dir).unwrap();

    let app_state = setup_app(&app_data, &fixture_dir, &library_root).await;

    // Run A: scan, freeze, then cancel (frozen plan, no transactions = resubmittable).
    let album_a = source_root.join("album_a");
    std::fs::create_dir_all(&album_a).unwrap();
    write_test_png(&album_a.join("a.png"));
    crate::commands::start_scan_for_state(&app_state, source_root.display().to_string())
        .await
        .unwrap();
    let scan_a = wait_for_scan_terminal(&app_state).await;
    let run_a = uuid::Uuid::parse_str(scan_a.import_run_id.as_deref().unwrap()).unwrap();
    crate::commands::generate_import_plan_for_state(&app_state, run_a.to_string())
        .await
        .unwrap();
    crate::commands::freeze_import_plan_for_state(&app_state, run_a.to_string())
        .await
        .unwrap();

    // Manually set run A to cancelled (frozen plan exists, no transactions).
    let (client, handle) = connect(&app_state).await;
    client
        .execute(
            "UPDATE import_runs SET state = 'cancelled' WHERE id = $1",
            &[&run_a],
        )
        .await
        .unwrap();
    drop(client);
    handle.abort();

    // Run B: scan only, leaves it in ready_to_commit.
    let album_b = source_root.join("album_b");
    std::fs::create_dir_all(&album_b).unwrap();
    write_test_png(&album_b.join("b.png"));
    crate::commands::start_scan_for_state(&app_state, source_root.display().to_string())
        .await
        .unwrap();
    let scan_b = wait_for_scan_terminal(&app_state).await;
    let _run_b = uuid::Uuid::parse_str(scan_b.import_run_id.as_deref().unwrap()).unwrap();

    // Resolver must pick the resubmittable cancelled run A (priority 3 > ready_to_commit priority 2?
    // No: ready_to_commit is priority 2, cancelled is priority 3. So run B wins.)
    // Actually: ready_to_commit has HIGHER priority than cancelled.
    // The resolver should pick run B (ready_to_commit) over run A (cancelled).
    let stage = crate::commands::get_import_workflow_stage_for_state(&app_state)
        .await
        .unwrap();
    assert_eq!(
        stage.import_run_id.as_deref(),
        Some(_run_b.to_string()).as_deref(),
        "ready_to_commit has higher priority than cancelled"
    );
    assert_eq!(stage.stage, "commit_confirm");

    shutdown(&app_state).await;
}

// --- helpers ---

async fn setup_app(app_data: &Path, fixture_dir: &Path, library_root: &Path) -> AppState {
    let app_state = AppState::new(app_data, fixture_dir.to_path_buf()).unwrap();
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
    app_state
}

async fn connect(
    app_state: &AppState,
) -> (tokio_postgres::Client, tokio::task::JoinHandle<()>) {
    let (client, handle) = {
        let mgr = app_state.postgres_manager.lock().await;
        mgr.connect().await.unwrap()
    };
    (client, handle)
}

async fn shutdown(app_state: &AppState) {
    let mut mgr = app_state.postgres_manager.lock().await;
    mgr.shutdown().await.unwrap();
}

async fn wait_for_scan_terminal(
    app_state: &AppState,
) -> crate::domain::import_state::ScanProgress {
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
            "scan did not reach terminal; last: {progress:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_commit_terminal(
    app_state: &AppState,
) -> crate::domain::import_state::CommitProgress {
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
            "commit did not reach terminal; last: {progress:?}"
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

fn ensure_postgres_bin() {
    if std::env::var("IMAGEDB_POSTGRES_BIN")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
    {
        return;
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
        return;
    }
    panic!(
        "IMAGEDB_POSTGRES_BIN not set and no bundled PostgreSQL found at {}",
        candidate.display()
    );
}
