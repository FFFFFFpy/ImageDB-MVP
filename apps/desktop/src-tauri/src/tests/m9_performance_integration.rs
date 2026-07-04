use crate::domain::import_state::{CommitProgress, ImportRunState, ScanProgress};
use crate::domain::DatabaseStatus;
use crate::state::AppState;
use rand::{rngs::StdRng, Rng, SeedableRng};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use uuid::Uuid;

const COMMAND_TIMEOUT_SECS: u64 = 180;

#[tokio::test]
#[ignore]
async fn m9_performance_gate_records_thresholds() {
    if !ensure_postgres_bin() {
        eprintln!("IMAGEDB_POSTGRES_BIN not set and no bundled test PostgreSQL found; skipping");
        return;
    }

    let image_count = std::env::var("IMAGEDB_M9_PERF_IMAGE_COUNT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(120)
        .max(10);

    let tmp = TempDir::new().unwrap();
    let app_data = tmp.path().join("app_data");
    let fixture_dir = tmp.path().join("fixtures");
    let source_root = tmp.path().join("source");
    let album_dir = source_root.join("perf_album");
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&album_dir).unwrap();
    std::fs::create_dir_all(&library_root).unwrap();
    std::fs::create_dir_all(&fixture_dir).unwrap();

    let mut source_bytes = 0u64;
    for index in 0..image_count {
        let path = album_dir.join(format!("img_{index:04}.png"));
        write_perf_png(&path, index as u32);
        source_bytes += std::fs::metadata(&path).unwrap().len();
    }

    let total_started = Instant::now();
    let app_state = AppState::new(&app_data, fixture_dir).unwrap();

    let startup_started = Instant::now();
    let database = crate::commands::initialize_managed_database_for_state(&app_state)
        .await
        .unwrap();
    let managed_startup_ms = elapsed_ms(startup_started);
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

    let scan_started = Instant::now();
    crate::commands::start_scan_for_state(&app_state, source_root.display().to_string())
        .await
        .unwrap();
    let scan = wait_for_scan_terminal(&app_state).await;
    let scan_ms = elapsed_ms(scan_started);
    assert_eq!(
        scan.state,
        ImportRunState::ReadyToCommit.to_string(),
        "{scan:?}"
    );
    assert_eq!(scan.total_images as usize, image_count);
    assert_eq!(scan.total_albums, 1);

    let import_run_id = Uuid::parse_str(scan.import_run_id.as_deref().unwrap()).unwrap();

    let plan_started = Instant::now();
    let plan =
        crate::commands::generate_import_plan_for_state(&app_state, import_run_id.to_string())
            .await
            .unwrap();
    let plan_ms = elapsed_ms(plan_started);
    assert_eq!(plan.kept_images.len(), image_count);

    let commit_started = Instant::now();
    crate::commands::start_import_commit_for_state(&app_state, import_run_id.to_string())
        .await
        .unwrap();
    let commit = wait_for_commit_terminal(&app_state).await;
    let commit_ms = elapsed_ms(commit_started);
    assert_eq!(commit.state, "completed", "{commit:?}");
    assert_eq!(commit.images_committed as usize, image_count);

    let recovery_scan_started = Instant::now();
    let recovery = crate::commands::scan_recoverable_transactions_for_state(&app_state)
        .await
        .unwrap();
    let recovery_scan_empty_ms = elapsed_ms(recovery_scan_started);
    assert!(recovery.is_empty(), "{recovery:?}");

    let (client, handle) = {
        let mgr = app_state.postgres_manager.lock().await;
        mgr.connect().await.unwrap()
    };
    let library_images: i64 = client
        .query_one("SELECT COUNT(*) FROM library_images", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(library_images as usize, image_count);
    handle.abort();

    let total_ms = elapsed_ms(total_started);
    let metrics = json!({
        "image_count": image_count,
        "source_bytes": source_bytes,
        "managed_startup_ms": managed_startup_ms,
        "scan_ms": scan_ms,
        "scan_images_per_sec": rate(image_count, scan_ms),
        "plan_ms": plan_ms,
        "commit_ms": commit_ms,
        "commit_images_per_sec": rate(image_count, commit_ms),
        "commit_mib_per_sec": mib_rate(source_bytes, commit_ms),
        "recovery_scan_empty_ms": recovery_scan_empty_ms,
        "total_ms": total_ms,
        "library_images": library_images,
    });
    println!("M9_PERFORMANCE_METRICS_JSON={}", metrics);

    let mut mgr = app_state.postgres_manager.lock().await;
    mgr.shutdown().await.unwrap();
}

async fn wait_for_scan_terminal(app_state: &AppState) -> ScanProgress {
    let deadline = Instant::now() + Duration::from_secs(COMMAND_TIMEOUT_SECS);
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
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn wait_for_commit_terminal(app_state: &AppState) -> CommitProgress {
    let deadline = Instant::now() + Duration::from_secs(COMMAND_TIMEOUT_SECS);
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
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn write_perf_png(path: &Path, seed: u32) {
    let mut rng = StdRng::seed_from_u64(0x5EED_0000 + seed as u64);
    let mut img = image::RgbImage::new(48, 48);
    for pixel in img.pixels_mut() {
        *pixel = image::Rgb([rng.gen(), rng.gen(), rng.gen()]);
    }
    img.save(path).unwrap();
}

fn elapsed_ms(started: Instant) -> u128 {
    started.elapsed().as_millis()
}

fn rate(count: usize, ms: u128) -> f64 {
    if ms == 0 {
        return count as f64;
    }
    count as f64 / (ms as f64 / 1000.0)
}

fn mib_rate(bytes: u64, ms: u128) -> f64 {
    if ms == 0 {
        return bytes as f64 / 1024.0 / 1024.0;
    }
    (bytes as f64 / 1024.0 / 1024.0) / (ms as f64 / 1000.0)
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
