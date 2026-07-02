use crate::infrastructure::file_transaction;
use crate::infrastructure::file_transaction::FileTransactionProbeResult;
use crate::infrastructure::image_fingerprint;
use crate::infrastructure::image_fingerprint::ImageFingerprintProbeResult;
use crate::infrastructure::postgres::PostgresProbeResult;
use crate::state::AppState;
use serde::Serialize;
use tauri::State;

#[tauri::command]
pub async fn probe_postgres(state: State<'_, AppState>) -> Result<PostgresProbeResult, String> {
    let mut mgr = state.postgres_manager.lock().await;
    mgr.initialize().await.map_err(|e| format!("{e}"))
}

#[tauri::command]
pub async fn probe_image_fingerprint(
    state: State<'_, AppState>,
) -> Result<ImageFingerprintProbeResult, String> {
    let fixture_dir = state.fixture_dir.clone();

    if !fixture_dir.exists() {
        std::fs::create_dir_all(&fixture_dir)
            .map_err(|e| format!("Cannot create fixture dir: {e}"))?;
    }

    let needs_samples = !fixture_dir.join("test-sample.png").exists();
    if needs_samples {
        match image_fingerprint::generate_test_samples(&fixture_dir) {
            Ok(created) => {
                tracing::info!("Generated test samples: {created:?}");
            }
            Err(e) => {
                return Ok(ImageFingerprintProbeResult {
                    fingerprints: vec![],
                    diagnostics: vec![format!("Cannot generate test samples: {e}")],
                    success: false,
                });
            }
        }
    }

    Ok(image_fingerprint::run_probe(&fixture_dir))
}

#[tauri::command]
pub async fn probe_file_transaction(
    state: State<'_, AppState>,
) -> Result<FileTransactionProbeResult, String> {
    let fixture_dir = state.fixture_dir.clone();
    let source_dir = fixture_dir.join("tx-source");

    if !source_dir.exists() {
        std::fs::create_dir_all(&source_dir)
            .map_err(|e| format!("Cannot create tx source dir: {e}"))?;
    }

    let file1 = source_dir.join("probe-file-1.txt");
    let file2 = source_dir.join("probe-file-2.bin");
    if !file1.exists() {
        std::fs::write(&file1, b"ImageDB file transaction probe - file 1")
            .map_err(|e| format!("Cannot write probe file: {e}"))?;
    }
    if !file2.exists() {
        std::fs::write(&file2, b"\x89PNG\r\n\x1a\n probe binary data for testing")
            .map_err(|e| format!("Cannot write probe file: {e}"))?;
    }

    let library_root = fixture_dir.join("tx-library");
    if !library_root.exists() {
        std::fs::create_dir_all(&library_root)
            .map_err(|e| format!("Cannot create library root: {e}"))?;
    }

    Ok(file_transaction::run_probe(&source_dir, &library_root))
}

#[derive(Debug, Clone, Serialize)]
pub struct AllProbeResults {
    pub postgres: PostgresProbeResult,
    pub fingerprint: ImageFingerprintProbeResult,
    pub file_transaction: FileTransactionProbeResult,
}

#[tauri::command]
pub async fn run_all_probes(state: State<'_, AppState>) -> Result<AllProbeResults, String> {
    let pg = probe_postgres(state.clone()).await?;
    let fp = probe_image_fingerprint(state.clone()).await?;
    let ft = probe_file_transaction(state.clone()).await?;
    Ok(AllProbeResults {
        postgres: pg,
        fingerprint: fp,
        file_transaction: ft,
    })
}
