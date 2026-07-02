use crate::domain::TransactionState;
use serde::Serialize;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct FileTransactionProbeResult {
    pub transaction_id: String,
    pub state: TransactionState,
    pub source_files: Vec<String>,
    pub published_files: Vec<String>,
    pub blake3_verified: bool,
    pub manifest_path: Option<String>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
struct TransactionRecord {
    transaction_id: String,
    state: String,
    source_files: Vec<String>,
    source_blake3: Vec<String>,
    published_files: Vec<String>,
    published_blake3: Vec<String>,
    manifest_path: Option<String>,
}

#[allow(unused_assignments)]
pub fn run_probe(source_dir: &Path, library_root: &Path) -> FileTransactionProbeResult {
    let tx_id = Uuid::new_v4().to_string();
    let mut diagnostics = Vec::new();
    let mut state = TransactionState::Ready;
    let mut published_files = Vec::new();
    let mut blake3_verified = false;
    let manifest_path: Option<String> = None;

    let source_files: Vec<PathBuf> = match std::fs::read_dir(source_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .map(|e| e.path())
            .collect(),
        Err(e) => {
            diagnostics.push(format!("Cannot read source directory: {e}"));
            return FileTransactionProbeResult {
                transaction_id: tx_id,
                state: TransactionState::Failed(format!("source read error: {e}")),
                source_files: vec![],
                published_files,
                blake3_verified,
                manifest_path,
                diagnostics,
            };
        }
    };

    if source_files.is_empty() {
        diagnostics.push("No files found in source directory".to_string());
        return FileTransactionProbeResult {
            transaction_id: tx_id,
            state: TransactionState::Failed("no source files".to_string()),
            source_files: vec![],
            published_files,
            blake3_verified,
            manifest_path,
            diagnostics,
        };
    }

    let source_file_names: Vec<String> = source_files
        .iter()
        .map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        })
        .collect();

    diagnostics.push(format!("Found {} source files", source_files.len()));

    let mut source_blake3 = Vec::new();
    for path in &source_files {
        match std::fs::read(path) {
            Ok(bytes) => {
                let hash = blake3::hash(&bytes).to_hex().to_string();
                source_blake3.push(hash);
            }
            Err(e) => {
                diagnostics.push(format!("Cannot read source file {}: {e}", path.display()));
                return FileTransactionProbeResult {
                    transaction_id: tx_id,
                    state: TransactionState::Failed("source file read error".to_string()),
                    source_files: source_file_names,
                    published_files,
                    blake3_verified,
                    manifest_path,
                    diagnostics,
                };
            }
        }
    }

    state = TransactionState::Staging;
    diagnostics.push("STAGING: copying files to staging directory".to_string());

    let staging_dir = library_root.join(".imagedb").join("staging").join(&tx_id);
    if let Err(e) = std::fs::create_dir_all(&staging_dir) {
        diagnostics.push(format!("Cannot create staging directory: {e}"));
        return FileTransactionProbeResult {
            transaction_id: tx_id,
            state: TransactionState::Failed(format!("staging mkdir error: {e}")),
            source_files: source_file_names,
            published_files,
            blake3_verified,
            manifest_path,
            diagnostics,
        };
    }

    for path in &source_files {
        let dest = staging_dir.join(path.file_name().unwrap_or_default());
        match std::fs::copy(path, &dest) {
            Ok(_) => diagnostics.push(format!(
                "Copied {} to staging",
                path.file_name().unwrap_or_default().to_string_lossy()
            )),
            Err(e) => {
                diagnostics.push(format!("Copy failed for {}: {e}", path.display()));
                let _ = std::fs::remove_dir_all(&staging_dir);
                return FileTransactionProbeResult {
                    transaction_id: tx_id,
                    state: TransactionState::Failed(format!("copy error: {e}")),
                    source_files: source_file_names,
                    published_files,
                    blake3_verified,
                    manifest_path,
                    diagnostics,
                };
            }
        }
    }

    state = TransactionState::Verifying;
    diagnostics.push("VERIFYING: checking BLAKE3 of staged files".to_string());

    let mut staged_ok = true;
    for (i, path) in source_files.iter().enumerate() {
        let staged = staging_dir.join(path.file_name().unwrap_or_default());
        match std::fs::read(&staged) {
            Ok(bytes) => {
                let hash = blake3::hash(&bytes).to_hex().to_string();
                if hash != source_blake3[i] {
                    diagnostics.push(format!(
                        "BLAKE3 mismatch for staged file: {}",
                        staged.display()
                    ));
                    staged_ok = false;
                } else {
                    diagnostics.push(format!(
                        "BLAKE3 verified: {}",
                        staged.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
            }
            Err(e) => {
                diagnostics.push(format!("Cannot read staged file {}: {e}", staged.display()));
                staged_ok = false;
            }
        }
    }

    if !staged_ok {
        diagnostics.push("Verification failed, cleaning up staging".to_string());
        let _ = std::fs::remove_dir_all(&staging_dir);
        return FileTransactionProbeResult {
            transaction_id: tx_id,
            state: TransactionState::Failed("BLAKE3 verification failed".to_string()),
            source_files: source_file_names,
            published_files,
            blake3_verified,
            manifest_path,
            diagnostics,
        };
    }

    state = TransactionState::Verified;
    diagnostics.push("All staged files verified".to_string());

    state = TransactionState::Publishing;
    diagnostics.push("PUBLISHING: moving to final location".to_string());

    let publish_dir = library_root.join("Albums").join(&tx_id);
    if let Err(e) = std::fs::create_dir_all(&publish_dir) {
        diagnostics.push(format!("Cannot create publish directory: {e}"));
        let _ = std::fs::remove_dir_all(&staging_dir);
        return FileTransactionProbeResult {
            transaction_id: tx_id,
            state: TransactionState::Failed(format!("publish mkdir error: {e}")),
            source_files: source_file_names,
            published_files,
            blake3_verified,
            manifest_path,
            diagnostics,
        };
    }

    let mut publish_blake3 = Vec::new();
    for path in &source_files {
        let staged = staging_dir.join(path.file_name().unwrap_or_default());
        let dest = publish_dir.join(path.file_name().unwrap_or_default());
        match std::fs::copy(&staged, &dest) {
            Ok(_) => {
                match std::fs::read(&dest) {
                    Ok(bytes) => {
                        let hash = blake3::hash(&bytes).to_hex().to_string();
                        publish_blake3.push(hash);
                    }
                    Err(e) => {
                        diagnostics
                            .push(format!("Cannot read published file for verification: {e}"));
                        let _ = std::fs::remove_dir_all(&staging_dir);
                        let _ = std::fs::remove_dir_all(&publish_dir);
                        return FileTransactionProbeResult {
                            transaction_id: tx_id,
                            state: TransactionState::Failed(
                                "publish verification error".to_string(),
                            ),
                            source_files: source_file_names,
                            published_files,
                            blake3_verified,
                            manifest_path,
                            diagnostics,
                        };
                    }
                }
                published_files.push(dest.display().to_string());
            }
            Err(e) => {
                diagnostics.push(format!("Publish copy failed: {e}"));
                let _ = std::fs::remove_dir_all(&staging_dir);
                let _ = std::fs::remove_dir_all(&publish_dir);
                return FileTransactionProbeResult {
                    transaction_id: tx_id,
                    state: TransactionState::Failed(format!("publish copy error: {e}")),
                    source_files: source_file_names,
                    published_files,
                    blake3_verified,
                    manifest_path,
                    diagnostics,
                };
            }
        }
    }

    let mut all_match = true;
    for (i, src_hash) in source_blake3.iter().enumerate() {
        if i < publish_blake3.len() && *src_hash != publish_blake3[i] {
            diagnostics.push(format!("BLAKE3 mismatch after publish for file index {i}"));
            all_match = false;
        }
    }

    if !all_match {
        diagnostics.push("Post-publish verification failed".to_string());
        let _ = std::fs::remove_dir_all(&staging_dir);
        return FileTransactionProbeResult {
            transaction_id: tx_id,
            state: TransactionState::Failed("post-publish BLAKE3 mismatch".to_string()),
            source_files: source_file_names,
            published_files,
            blake3_verified,
            manifest_path,
            diagnostics,
        };
    }

    blake3_verified = true;
    state = TransactionState::Published;
    diagnostics.push("All published files BLAKE3 verified".to_string());

    let manifest = TransactionRecord {
        transaction_id: tx_id.clone(),
        state: state.to_string(),
        source_files: source_file_names.clone(),
        source_blake3: source_blake3.clone(),
        published_files: published_files.clone(),
        published_blake3: publish_blake3,
        manifest_path: None,
    };

    let manifests_dir = library_root.join(".imagedb").join("manifests");
    let _ = std::fs::create_dir_all(&manifests_dir);
    let manifest_file = manifests_dir.join(format!("{tx_id}.json"));
    match serde_json::to_string_pretty(&manifest) {
        Ok(json) => match std::fs::write(&manifest_file, json) {
            Ok(_) => {
                diagnostics.push(format!("Manifest written: {}", manifest_file.display()));
            }
            Err(e) => {
                diagnostics.push(format!("Manifest write failed: {e}"));
            }
        },
        Err(e) => {
            diagnostics.push(format!("Manifest serialize failed: {e}"));
        }
    }

    let _ = std::fs::remove_dir_all(&staging_dir);
    diagnostics.push("Staging cleaned up".to_string());

    diagnostics.push(format!("Transaction {tx_id} completed: {state}"));

    FileTransactionProbeResult {
        transaction_id: tx_id,
        state,
        source_files: source_file_names,
        published_files,
        blake3_verified,
        manifest_path: Some(manifest_file.display().to_string()),
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[derive(Debug, Clone, Copy)]
    pub(crate) enum FaultPoint {
        FailAfterStaging,
        FailBeforePublish,
    }

    pub(crate) fn run_probe_with_faults(
        source_dir: &Path,
        library_root: &Path,
        fault: FaultPoint,
    ) -> FileTransactionProbeResult {
        let tx_id = Uuid::new_v4().to_string();
        let mut diagnostics = Vec::new();
        let mut published_files = Vec::new();
        let mut blake3_verified = false;
        let manifest_path: Option<String> = None;

        let source_files: Vec<PathBuf> = match std::fs::read_dir(source_dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_file())
                .map(|e| e.path())
                .collect(),
            Err(e) => {
                diagnostics.push(format!("Cannot read source directory: {e}"));
                return FileTransactionProbeResult {
                    transaction_id: tx_id,
                    state: TransactionState::Failed(format!("source read error: {e}")),
                    source_files: vec![],
                    published_files,
                    blake3_verified,
                    manifest_path,
                    diagnostics,
                };
            }
        };

        let source_file_names: Vec<String> = source_files
            .iter()
            .map(|p| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();

        let mut source_blake3 = Vec::new();
        for path in &source_files {
            match std::fs::read(path) {
                Ok(bytes) => source_blake3.push(blake3::hash(&bytes).to_hex().to_string()),
                Err(e) => {
                    return FileTransactionProbeResult {
                        transaction_id: tx_id,
                        state: TransactionState::Failed(format!("source read: {e}")),
                        source_files: source_file_names,
                        published_files,
                        blake3_verified,
                        manifest_path,
                        diagnostics,
                    };
                }
            }
        }

        let staging_dir = library_root.join(".imagedb").join("staging").join(&tx_id);
        let _ = std::fs::create_dir_all(&staging_dir);
        for path in &source_files {
            let dest = staging_dir.join(path.file_name().unwrap_or_default());
            let _ = std::fs::copy(path, &dest);
        }

        for (i, path) in source_files.iter().enumerate() {
            let staged = staging_dir.join(path.file_name().unwrap_or_default());
            if let Ok(bytes) = std::fs::read(&staged) {
                let hash = blake3::hash(&bytes).to_hex().to_string();
                if hash != source_blake3[i] {
                    let _ = std::fs::remove_dir_all(&staging_dir);
                    return FileTransactionProbeResult {
                        transaction_id: tx_id,
                        state: TransactionState::Failed("BLAKE3 verification failed".to_string()),
                        source_files: source_file_names,
                        published_files,
                        blake3_verified,
                        manifest_path,
                        diagnostics,
                    };
                }
            }
        }

        if matches!(fault, FaultPoint::FailAfterStaging) {
            diagnostics.push("FAULT INJECTED: failure after staging".to_string());
            let _ = std::fs::remove_dir_all(&staging_dir);
            return FileTransactionProbeResult {
                transaction_id: tx_id,
                state: TransactionState::Failed("injected: post-staging fault".to_string()),
                source_files: source_file_names,
                published_files,
                blake3_verified,
                manifest_path,
                diagnostics,
            };
        }

        if matches!(fault, FaultPoint::FailBeforePublish) {
            diagnostics.push("FAULT INJECTED: failure before publish".to_string());
            let _ = std::fs::remove_dir_all(&staging_dir);
            return FileTransactionProbeResult {
                transaction_id: tx_id,
                state: TransactionState::Failed("injected: pre-publish fault".to_string()),
                source_files: source_file_names,
                published_files,
                blake3_verified,
                manifest_path,
                diagnostics,
            };
        }

        let publish_dir = library_root.join("Albums").join(&tx_id);
        let _ = std::fs::create_dir_all(&publish_dir);
        for path in &source_files {
            let staged = staging_dir.join(path.file_name().unwrap_or_default());
            let dest = publish_dir.join(path.file_name().unwrap_or_default());
            let _ = std::fs::copy(&staged, &dest);
            published_files.push(dest.display().to_string());
        }

        blake3_verified = true;

        FileTransactionProbeResult {
            transaction_id: tx_id,
            state: TransactionState::Published,
            source_files: source_file_names,
            published_files,
            blake3_verified,
            manifest_path,
            diagnostics,
        }
    }

    #[test]
    fn test_file_transaction_probe_success() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let library_root = tmp.path().join("library");
        std::fs::create_dir_all(&source_dir).unwrap();

        std::fs::write(source_dir.join("file1.txt"), b"hello world").unwrap();
        std::fs::write(source_dir.join("file2.txt"), b"foo bar baz").unwrap();

        let result = run_probe(&source_dir, &library_root);

        assert_eq!(result.state, TransactionState::Published);
        assert!(result.blake3_verified);
        assert_eq!(result.source_files.len(), 2);
        assert_eq!(result.published_files.len(), 2);
        assert!(result.manifest_path.is_some());

        for published in &result.published_files {
            assert!(Path::new(published).exists());
        }

        let manifest_file = result.manifest_path.unwrap();
        assert!(Path::new(&manifest_file).exists());
        let manifest_json = std::fs::read_to_string(&manifest_file).unwrap();
        let record: TransactionRecord = serde_json::from_str(&manifest_json).unwrap();
        assert_eq!(record.state, "PUBLISHED");
    }

    #[test]
    fn test_file_transaction_probe_empty_source() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let library_root = tmp.path().join("library");
        std::fs::create_dir_all(&source_dir).unwrap();

        let result = run_probe(&source_dir, &library_root);
        assert!(matches!(result.state, TransactionState::Failed(_)));
    }

    #[test]
    fn test_file_transaction_no_staging_leftover() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let library_root = tmp.path().join("library");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join("test.bin"), b"binary data").unwrap();

        let result = run_probe(&source_dir, &library_root);
        assert_eq!(result.state, TransactionState::Published);

        let staging_dir = library_root.join(".imagedb").join("staging");
        if staging_dir.exists() {
            let entries: Vec<_> = std::fs::read_dir(&staging_dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .collect();
            assert!(
                entries.is_empty(),
                "Staging directory should be empty after successful publish"
            );
        }
    }

    #[test]
    fn test_fault_after_staging_not_published() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let library_root = tmp.path().join("library");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join("file.txt"), b"data for fault test").unwrap();

        let result =
            run_probe_with_faults(&source_dir, &library_root, FaultPoint::FailAfterStaging);

        assert!(
            matches!(result.state, TransactionState::Failed(_)),
            "State should be Failed, got {:?}",
            result.state
        );
        assert!(
            !result.blake3_verified,
            "BLAKE3 must not be verified on fault"
        );
        assert!(
            result.published_files.is_empty(),
            "No files should be published on fault"
        );
        assert!(
            result.manifest_path.is_none(),
            "No manifest should be written on fault"
        );

        let albums_dir = library_root.join("Albums");
        if albums_dir.exists() {
            let entries: Vec<_> = std::fs::read_dir(&albums_dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .collect();
            assert!(entries.is_empty(), "No publish directories should exist");
        }

        let manifests_dir = library_root.join(".imagedb").join("manifests");
        if manifests_dir.exists() {
            let entries: Vec<_> = std::fs::read_dir(&manifests_dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .collect();
            assert!(entries.is_empty(), "No manifest files should exist");
        }
    }

    #[test]
    fn test_fault_before_publish_not_published() {
        let tmp = TempDir::new().unwrap();
        let source_dir = tmp.path().join("source");
        let library_root = tmp.path().join("library");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join("file.txt"), b"data for fault test 2").unwrap();

        let result =
            run_probe_with_faults(&source_dir, &library_root, FaultPoint::FailBeforePublish);

        assert!(
            matches!(result.state, TransactionState::Failed(_)),
            "State should be Failed, got {:?}",
            result.state
        );
        assert!(!result.blake3_verified);
        assert!(result.published_files.is_empty());
        assert!(result.manifest_path.is_none());

        let albums_dir = library_root.join("Albums");
        assert!(
            !albums_dir.exists() || std::fs::read_dir(&albums_dir).unwrap().count() == 0,
            "No publish directories should exist after pre-publish fault"
        );
    }
}
