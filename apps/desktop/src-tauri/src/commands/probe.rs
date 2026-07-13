use crate::infrastructure::file_transaction;
use crate::infrastructure::file_transaction::FileTransactionProbeResult;
use crate::infrastructure::image_fingerprint;
use crate::infrastructure::image_fingerprint::ImageFingerprintProbeResult;
use crate::infrastructure::postgres::PostgresProbeResult;
use crate::state::{AppState, CriticalOperationGuard, CriticalOperationKind};
use serde::Serialize;
use std::future::Future;
use std::path::Path;
use tauri::State;

#[tauri::command]
pub async fn probe_postgres(state: State<'_, AppState>) -> Result<PostgresProbeResult, String> {
    probe_postgres_for_state(&state).await
}

async fn probe_postgres_for_state(state: &AppState) -> Result<PostgresProbeResult, String> {
    run_postgres_probe_with_guard(&state.critical_operation_guard, || async {
        let mut mgr = state.postgres_manager.lock().await;
        mgr.initialize().await.map_err(|e| format!("{e}"))
    })
    .await
}

async fn run_postgres_probe_with_guard<T, F, Fut>(
    guard: &CriticalOperationGuard,
    probe: F,
) -> Result<T, String>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, String>>,
{
    let _operation = guard.begin_operation(CriticalOperationKind::ProbeManagedDatabase)?;
    probe().await
}

#[tauri::command]
pub async fn probe_image_fingerprint(
    state: State<'_, AppState>,
) -> Result<ImageFingerprintProbeResult, String> {
    run_image_fingerprint_probe(&state.fixture_dir)
}

fn run_image_fingerprint_probe(fixture_dir: &Path) -> Result<ImageFingerprintProbeResult, String> {
    if !fixture_dir.exists() {
        std::fs::create_dir_all(fixture_dir)
            .map_err(|e| format!("Cannot create fixture dir: {e}"))?;
    }

    let needs_samples = !fixture_dir.join("test-sample.png").exists();
    if needs_samples {
        match image_fingerprint::generate_test_samples(fixture_dir) {
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

    Ok(image_fingerprint::run_probe(fixture_dir))
}

#[tauri::command]
pub async fn probe_file_transaction(
    state: State<'_, AppState>,
) -> Result<FileTransactionProbeResult, String> {
    run_file_transaction_probe(&state.fixture_dir)
}

fn run_file_transaction_probe(fixture_dir: &Path) -> Result<FileTransactionProbeResult, String> {
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
    run_all_probes_for_state(&state).await
}

async fn run_all_probes_for_state(state: &AppState) -> Result<AllProbeResults, String> {
    let pg = probe_postgres_for_state(state).await?;
    let fp = run_image_fingerprint_probe(&state.fixture_dir)?;
    let ft = run_file_transaction_probe(&state.fixture_dir)?;
    Ok(AllProbeResults {
        postgres: pg,
        fingerprint: fp,
        file_transaction: ft,
    })
}

#[cfg(test)]
mod critical_probe_tests {
    use super::*;
    use crate::state::CriticalTaskKind;
    use tempfile::TempDir;

    fn app_state(temp: &TempDir) -> AppState {
        AppState::new(temp.path(), temp.path().join("fixtures")).unwrap()
    }

    async fn assert_postgres_probe_rejected(task_kind: CriticalTaskKind, expected_error: &str) {
        let temp = TempDir::new().unwrap();
        let state = app_state(&temp);
        let task = state
            .critical_operation_guard
            .begin_task(task_kind)
            .unwrap();

        let error = probe_postgres_for_state(&state).await.unwrap_err();
        assert_eq!(error, expected_error);
        assert_eq!(
            state.critical_operation_guard.snapshot().active_task_kinds,
            vec![task_kind.as_str()]
        );

        drop(task);
        assert!(!state.critical_operation_guard.snapshot().is_blocked);
    }

    #[tokio::test]
    async fn active_commit_rejects_postgres_probe_and_preserves_task() {
        assert_postgres_probe_rejected(
            CriticalTaskKind::Commit,
            "cannot probe managed database while import commit is running",
        )
        .await;
    }

    #[tokio::test]
    async fn active_scan_rejects_postgres_probe_and_preserves_task() {
        assert_postgres_probe_rejected(
            CriticalTaskKind::Scan,
            "cannot probe managed database while import scan is running",
        )
        .await;
    }

    #[tokio::test]
    async fn active_recovery_rejects_postgres_probe_and_preserves_task() {
        assert_postgres_probe_rejected(
            CriticalTaskKind::Recovery,
            "cannot probe managed database while transaction recovery is running",
        )
        .await;
    }

    #[tokio::test]
    async fn active_external_migration_rejects_postgres_probe_and_preserves_task() {
        assert_postgres_probe_rejected(
            CriticalTaskKind::ExternalMigration,
            "cannot probe managed database while external database migration is running",
        )
        .await;
    }

    #[tokio::test]
    async fn active_task_rejects_run_all_probes_before_other_probes_run() {
        let temp = TempDir::new().unwrap();
        let state = app_state(&temp);
        let commit = state
            .critical_operation_guard
            .begin_task(CriticalTaskKind::Commit)
            .unwrap();

        let error = run_all_probes_for_state(&state).await.unwrap_err();
        assert_eq!(
            error,
            "cannot probe managed database while import commit is running"
        );
        assert!(!state.fixture_dir.exists());
        assert_eq!(
            state.critical_operation_guard.snapshot().active_task_kinds,
            vec!["commit"]
        );

        drop(commit);
    }

    #[tokio::test]
    async fn postgres_probe_guard_runs_probe_without_active_tasks() {
        let guard = CriticalOperationGuard::default();
        let guard_during_probe = guard.clone();

        let result = run_postgres_probe_with_guard(&guard, || async move {
            assert_eq!(
                guard_during_probe.snapshot().active_operation.as_deref(),
                Some("probe_managed_database")
            );
            let error = guard_during_probe
                .begin_task(CriticalTaskKind::Scan)
                .expect_err("tasks must not start while the database probe is running");
            assert_eq!(
                error,
                "cannot start import scan while managed database probe is running"
            );
            Ok::<_, String>("probe completed")
        })
        .await
        .unwrap();

        assert_eq!(result, "probe completed");
        assert!(!guard.snapshot().is_blocked);
    }
}
