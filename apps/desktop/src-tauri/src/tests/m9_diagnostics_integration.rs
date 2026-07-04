use crate::infrastructure::settings::AppSettings;
use crate::state::AppState;
use serde_json::Value;
use std::path::PathBuf;
use tempfile::TempDir;

#[tokio::test]
#[ignore]
async fn m9_diagnostics_export_redacts_secrets_and_image_content() {
    ensure_postgres_bin();

    let tmp = TempDir::new().unwrap();
    let app_data = tmp.path().join("app_data");
    let fixture_dir = tmp.path().join("fixtures");
    let library_root = tmp.path().join("library");
    std::fs::create_dir_all(&fixture_dir).unwrap();
    std::fs::create_dir_all(&library_root).unwrap();

    let image_path = library_root.join("leaky-image.png");
    std::fs::write(&image_path, b"UNIQUE_IMAGE_CONTENT_SENTINEL").unwrap();
    let log_dir = app_data.join("logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(
        log_dir.join("imagedb.log.test"),
        format!(
            "password=DIAGNOSTIC_SECRET postgres://user:URI_SECRET@localhost/db image={}",
            image_path.display()
        ),
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
                database_mode: Some("managed_local".to_string()),
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

    let result = crate::commands::export_diagnostics_for_state(&app_state)
        .await
        .unwrap();
    assert!(result.redacted);
    assert!(std::path::Path::new(&result.path).is_file());

    let content = std::fs::read_to_string(&result.path).unwrap();
    assert!(!content.contains("DIAGNOSTIC_SECRET"));
    assert!(!content.contains("URI_SECRET"));
    assert!(!content.contains("UNIQUE_IMAGE_CONTENT_SENTINEL"));
    assert!(!content.contains("leaky-image.png"));
    assert!(content.contains("<redacted>"));
    assert!(content.contains("<redacted-path>"));

    let json: Value = serde_json::from_str(&content).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(
        json["redaction"]["image_content"],
        "image bytes and previews are never included"
    );
    assert!(json["database"]["postgres_version"].is_string());
    assert!(json["database"]["pgvector_version"].is_string());
    assert_eq!(
        json["migration_state"]["current_version"],
        Value::String(
            crate::infrastructure::postgres::MigrationRunner::latest_version().to_string()
        )
    );
    assert!(json["storage_capabilities"].is_object());

    let mut mgr = app_state.postgres_manager.lock().await;
    mgr.shutdown().await.unwrap();
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
