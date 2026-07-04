use crate::domain::import_state::{CommitProgress, ImportRunState};
use crate::infrastructure::settings::AppSettings;
use crate::repositories::import_repository::ImportRepository;
use crate::services::{commit_service, review_service, scan_service};
use crate::state::AppState;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::Mutex;

#[tokio::test]
#[ignore]
async fn m9_main_chain_exact_duplicate_import_freezes_plan_and_commits() {
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
    write_test_png(&album_dir.join("sample-original.png"));
    std::fs::copy(
        album_dir.join("sample-original.png"),
        album_dir.join("sample-copy.png"),
    )
    .unwrap();

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
        Arc::new(Mutex::new(crate::domain::import_state::ScanProgress::idle())),
    )
    .await
    .unwrap();
    assert_eq!(
        scan.state,
        ImportRunState::ReadyToCommit.to_string(),
        "{scan:?}"
    );
    assert_eq!(scan.total_albums, 1);
    assert_eq!(scan.total_images, 2);
    assert_eq!(scan.duplicate_count, 1);
    let import_run_id = uuid::Uuid::parse_str(scan.import_run_id.as_deref().unwrap()).unwrap();

    let (client, handle) = {
        let mgr = app_state.postgres_manager.lock().await;
        mgr.connect().await.unwrap()
    };

    let review_progress = review_service::get_review_progress(&client, import_run_id)
        .await
        .unwrap();
    assert!(review_progress.all_decided);

    let plan = review_service::generate_import_plan(&client, import_run_id)
        .await
        .unwrap();
    assert_eq!(plan.total_albums, 1);
    assert_eq!(plan.total_images, 2);
    assert_eq!(plan.excluded_count, 1);
    assert_eq!(plan.kept_images.len(), 1);

    let frozen = ImportRepository::load_frozen_plan(&client, import_run_id)
        .await
        .unwrap()
        .expect("generate_import_plan must freeze the command-visible plan");
    assert_eq!(frozen.albums.len(), 1);
    assert_eq!(frozen.albums[0].1.len(), 1);

    drop(client);
    handle.abort();

    let progress_tracker = Arc::new(Mutex::new(CommitProgress::idle(&import_run_id.to_string())));
    let commit = commit_service::run_import_commit(
        app_state.postgres_manager.clone(),
        library_root.display().to_string(),
        import_run_id,
        Arc::new(AtomicBool::new(false)),
        progress_tracker,
    )
    .await
    .unwrap();
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
    false
}
