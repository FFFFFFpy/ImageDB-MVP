//! Source album snapshot capture + verification.
//!
//! The source archive pipeline uses these helpers to prove that a source
//! album directory (or its archive copy) is *exactly* the set of files
//! captured at scan time — same relative paths, same sizes, same BLAKE3,
//! same file-type tags, and the same deterministic snapshot hash. The
//! frozen import plan is NOT a substitute: it only lists images selected
//! for import, not the full album content (sidecars, descriptions, nested
//! files, excluded images, etc.).
//!
//! Shared by [`crate::services::scan_service`],
//! [`crate::services::commit_service`] (Phase 6 source archive), and
//! [`crate::services::recovery_service`] (resume_source_archive).
//!
//! Phase 5: the blocking work (read_dir + File::open + BLAKE3 hashing) is
//! isolated inside `spawn_blocking` so the async runtime is never blocked
//! by a large-album snapshot. Special filesystem entries (symlinks,
//! directory junctions, Windows reparse points, FIFOs, devices) are
//! **rejected explicitly** — never silently hashed or skipped.
use crate::error::AppError;
use crate::repositories::import_repository::{
    ImportRepository, NewSnapshotFile, SnapshotFileRecord,
};
use std::io::Read as _;
use std::path::{Component, Path};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use uuid::Uuid;

/// P2: a global semaphore bounding the number of concurrent snapshot
/// `spawn_blocking` tasks. Each snapshot capture + verify acquires a permit
/// before offloading the blocking directory walk + BLAKE3 hashing, so a
/// burst of concurrent snapshots (multi-album scan, parallel recovery) does
/// not exhaust the blocking thread pool or saturate disk I/O. The bound is
/// small (2) because snapshot work is I/O-heavy, not CPU-bound.
static SNAPSHOT_CONCURRENCY: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

/// A verification failure for a single snapshot check.
#[derive(Debug, Clone)]
pub enum SnapshotVerifyError {
    MissingFile(String),
    ExtraFile(String),
    SizeMismatch {
        path: String,
        expected: i64,
        actual: i64,
    },
    HashMismatch {
        path: String,
    },
    FileTypeMismatch {
        path: String,
        expected: String,
        actual: String,
    },
    SnapshotHashMismatch {
        expected: String,
        actual: String,
    },
    /// Phase 5: a special filesystem entry (symlink, directory junction,
    /// Windows reparse point, FIFO, socket, device, or any other non-regular
    /// file) was encountered. The album is rejected rather than silently
    /// hashed.
    ///
    /// Note: in the current implementation, special entries are rejected at
    /// the `collect_album_files` layer with an `AppError` (the snapshot
    /// cannot be captured at all if a special entry is present). This
    /// variant is retained so a future verifier-only mode can surface the
    /// specific entry kind without aborting the whole snapshot.
    #[allow(dead_code)]
    UnsupportedEntry {
        path: String,
        entry_kind: String,
    },
}

impl std::fmt::Display for SnapshotVerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingFile(p) => write!(f, "missing file: {p}"),
            Self::ExtraFile(p) => write!(f, "extra file: {p}"),
            Self::SizeMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "size mismatch for {path}: expected {expected}, got {actual}"
            ),
            Self::HashMismatch { path } => write!(f, "blake3 mismatch for {path}"),
            Self::FileTypeMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "file type mismatch for {path}: expected {expected}, got {actual}"
            ),
            Self::SnapshotHashMismatch { expected, actual } => {
                write!(
                    f,
                    "snapshot hash mismatch: expected {expected}, got {actual}"
                )
            }
            Self::UnsupportedEntry { path, entry_kind } => write!(
                f,
                "unsupported filesystem entry ({entry_kind}) at {path}: only regular files are allowed"
            ),
        }
    }
}

fn normalize_snapshot_relative_path(
    album_path: &Path,
    file_path: &Path,
) -> Result<String, AppError> {
    let rel = file_path.strip_prefix(album_path).map_err(|_| {
        AppError::Internal(format!(
            "file {} is not under album {}",
            file_path.display(),
            album_path.display()
        ))
    })?;
    if rel.as_os_str().is_empty() {
        return Err(AppError::Internal(
            "empty snapshot relative path".to_string(),
        ));
    }
    for component in rel.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(AppError::Internal(format!(
                    "invalid snapshot relative path component: {}",
                    rel.display()
                )));
            }
        }
    }
    let s = rel.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        return Err(AppError::Internal(format!("invalid relative path: {s}")));
    }
    Ok(s)
}

fn file_type_from_path(path: &Path) -> String {
    if path.is_file() {
        "regular_file".to_string()
    } else {
        "unknown".to_string()
    }
}

fn hash_file_sync_with_cancel(
    path: &Path,
    cancelled: Option<&AtomicBool>,
) -> Result<Vec<u8>, AppError> {
    use std::sync::atomic::Ordering;
    let mut f = std::fs::File::open(path)
        .map_err(|e| AppError::IoError(format!("cannot open {}: {e}", path.display())))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 65536];
    loop {
        if cancelled.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(AppError::Internal("snapshot walk cancelled".to_string()));
        }
        let n = f
            .read(&mut buf)
            .map_err(|e| AppError::IoError(format!("read error on {}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().as_bytes().to_vec())
}

/// Classify a `FileType`-style entry kind for the `UnsupportedEntry` error.
/// Returns `None` for regular files + directories (directories are walked,
/// regular files are hashed; everything else is rejected).
fn classify_special_entry(ft: &std::fs::FileType) -> Option<&'static str> {
    if ft.is_symlink() {
        Some("symlink")
    } else if ft.is_dir() {
        None
    } else if ft.is_file() {
        // On Unix, a regular file. On Windows, `is_file()` is true for the
        // normal case but `is_symlink()` already caught reparse-point
        // symlinks above. Other reparse point types (junctions) are reported
        // as `is_dir()` by std on Windows, so we additionally check via
        // metadata kind below in `walk`.
        None
    } else {
        // FIFO, socket, char/block device, or unknown.
        Some("special_file")
    }
}

/// Recursively walk `album_path` and return every regular file with its
/// relative path, size, and BLAKE3. Sidecars, descriptions, nested files,
/// and any other non-directory entry are included — this is the full album
/// image used to prove source/archive integrity later.
///
/// Phase 5: **special filesystem entries are rejected explicitly** —
/// symlinks, directory junctions, Windows reparse points, FIFOs, sockets,
/// and devices surface as an `AppError` rather than being silently hashed
/// or skipped. Only regular files + directories are walked.
#[cfg(test)]
pub fn collect_album_files(album_path: &Path) -> Result<Vec<NewSnapshotFile>, AppError> {
    collect_album_files_with_cancel(album_path, None)
}

/// Variant that accepts an optional cancel flag (P2). The walk checks the
/// flag before each directory entry so a snapshot of a very large album
/// can be aborted promptly rather than walking the whole tree. The flag is
/// `Option<&AtomicBool>` (sync) so it can be threaded through from a
/// `spawn_blocking` task.
pub fn collect_album_files_with_cancel(
    album_path: &Path,
    cancelled: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Vec<NewSnapshotFile>, AppError> {
    use std::sync::atomic::Ordering;
    fn walk(
        dir: &Path,
        album_path: &Path,
        out: &mut Vec<NewSnapshotFile>,
        cancelled: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<(), AppError> {
        use std::sync::atomic::Ordering;
        let entries = std::fs::read_dir(dir)?;
        for entry in entries {
            // Check cancellation before each entry so a large-album walk
            // aborts promptly.
            if let Some(flag) = cancelled {
                if flag.load(Ordering::Relaxed) {
                    return Err(AppError::Internal("snapshot walk cancelled".to_string()));
                }
            }
            let entry = entry?;
            let path = entry.path();
            // Use symlink_metadata so we inspect the link itself, not its
            // target. A symlink must NEVER be followed into during a
            // snapshot — it would silently pull in unrelated files.
            let meta = std::fs::symlink_metadata(&path)?;
            let ft = meta.file_type();
            if ft.is_symlink() {
                return Err(AppError::Internal(format!(
                    "unsupported filesystem entry (symlink) at {}: only regular files are allowed",
                    path.display()
                )));
            }
            if ft.is_dir() {
                // On Windows, directory junctions and some reparse points
                // are reported as `is_dir()` by std. `symlink_metadata`
                // already gave us the reparse info; if the entry is a
                // reparse point that is NOT a normal directory, reject it.
                // std::fs::FileType does not expose reparse tag directly,
                // so we rely on the canonicalize-then-compare check: if
                // canonicalizing the dir yields a path that does not start
                // with this dir, it is almost certainly a junction.
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    // FILE_ATTRIBUTE_REPARSE_POINT = 0x400. If set, this is
                    // a reparse point (junction or mounted folder, not a
                    // plain directory).
                    if meta.file_attributes() & 0x400 != 0 {
                        return Err(AppError::Internal(format!(
                            "unsupported filesystem entry (reparse point / junction) at {}: only regular files are allowed",
                            path.display()
                        )));
                    }
                }
                walk(&path, album_path, out, cancelled)?;
            } else if ft.is_file() {
                let relative_path = normalize_snapshot_relative_path(album_path, &path)?;
                let metadata = std::fs::metadata(&path)?;
                let blake3 = hash_file_sync_with_cancel(&path, cancelled)?;
                out.push(NewSnapshotFile {
                    relative_path,
                    file_type: file_type_from_path(&path),
                    file_size: metadata.len() as i64,
                    blake3,
                });
            } else {
                // FIFO, socket, char/block device, or unknown.
                let kind = classify_special_entry(&ft).unwrap_or("unknown");
                return Err(AppError::Internal(format!(
                    "unsupported filesystem entry ({kind}) at {}: only regular files are allowed",
                    path.display()
                )));
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(album_path, album_path, &mut files, cancelled)?;
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    let _ = Ordering::Relaxed;
    Ok(files)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Deterministic BLAKE3 hash over the canonical snapshot content.
/// Albums/images are sorted by relative path; each record contributes
/// relative_path, file_type, file_size, and blake3 (length-prefixed).
pub fn compute_snapshot_hash(files: &[NewSnapshotFile]) -> Vec<u8> {
    let mut sorted: Vec<&NewSnapshotFile> = files.iter().collect();
    sorted.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    let mut hasher = blake3::Hasher::new();
    for f in sorted {
        for field in [f.relative_path.as_bytes(), f.file_type.as_bytes()] {
            hasher.update(&(field.len() as u64).to_le_bytes());
            hasher.update(field);
        }
        hasher.update(&f.file_size.to_le_bytes());
        hasher.update(&(f.blake3.len() as u64).to_le_bytes());
        hasher.update(&f.blake3);
    }
    hasher.finalize().as_bytes().to_vec()
}

/// Capture a full snapshot of `source_album_path` and persist it under
/// `import_album_id`. Returns the new snapshot id and its hash.
///
/// Phase 5: the blocking work (`read_dir` + `File::open` + BLAKE3 hashing
/// over potentially many large files) is isolated inside
/// `spawn_blocking` so the async runtime worker is never held hostage by
/// a slow snapshot.
///
/// P2: a global semaphore (`SNAPSHOT_CONCURRENCY`) bounds the number of
/// concurrent snapshot tasks so a burst of albums cannot exhaust the
/// blocking thread pool or saturate disk I/O.
#[cfg(all(test, feature = "real-db-tests"))]
pub async fn capture_source_album_snapshot(
    client: &tokio_postgres::Client,
    import_run_id: Uuid,
    import_album_id: Uuid,
    source_album_path: &Path,
) -> Result<(Uuid, Vec<u8>), AppError> {
    capture_source_album_snapshot_with_cancel(
        client,
        import_run_id,
        import_album_id,
        source_album_path,
        None,
    )
    .await
}

/// Variant that accepts an optional cancel flag (P2). The flag is threaded
/// into the blocking walk so a snapshot of a very large album can be
/// aborted promptly.
pub async fn capture_source_album_snapshot_with_cancel(
    client: &tokio_postgres::Client,
    import_run_id: Uuid,
    import_album_id: Uuid,
    source_album_path: &Path,
    cancelled: Option<Arc<AtomicBool>>,
) -> Result<(Uuid, Vec<u8>), AppError> {
    // A snapshot is an immutable checkpoint, not a per-attempt cache.  A
    // cancelled scan can already have persisted it before fingerprinting;
    // re-inserting on resume would violate UNIQUE(import_album_id), while
    // replacing it would silently accept source changes made during the
    // interruption.  Reuse the original evidence and let the caller verify
    // the current directory against it.
    if let Some(existing) =
        ImportRepository::get_source_album_snapshot(client, import_album_id).await?
    {
        let requested_path = source_album_path.display().to_string();
        if existing.import_run_id != import_run_id || existing.source_album_path != requested_path {
            return Err(AppError::Internal(format!(
                "snapshot identity mismatch for album {import_album_id}: stored run/path is {}/{}, requested {}/{}",
                existing.import_run_id,
                existing.source_album_path,
                import_run_id,
                requested_path
            )));
        }
        return Ok((existing.snapshot_id, existing.snapshot_hash));
    }

    let album_path = source_album_path.to_path_buf();
    // Acquire a concurrency permit before offloading. This bounds the
    // number of simultaneous snapshot walks so the blocking thread pool
    // is not exhausted by a multi-album scan or parallel recovery.
    let _permit = SNAPSHOT_CONCURRENCY
        .acquire()
        .await
        .map_err(|e| AppError::Internal(format!("snapshot semaphore closed: {e}")))?;
    // Offload the blocking directory walk + hashing to a dedicated thread.
    let cancel_for_task = cancelled.clone();
    let files = tokio::task::spawn_blocking(move || {
        collect_album_files_with_cancel(&album_path, cancel_for_task.as_deref())
    })
    .await
    .map_err(|e| AppError::Internal(format!("source album snapshot task failed: {e}")))??;
    let snapshot_hash = compute_snapshot_hash(&files);
    let snapshot_id = Uuid::new_v4();
    let source_album_path = source_album_path.display().to_string();
    ImportRepository::insert_source_album_snapshot(
        client,
        snapshot_id,
        import_run_id,
        import_album_id,
        &source_album_path,
        &snapshot_hash,
        &files,
    )
    .await?;
    Ok((snapshot_id, snapshot_hash))
}

/// Load the persisted snapshot header + files for `import_album_id`.
///
/// Used by commit Phase 6 and recovery to avoid re-querying the header and
/// file list twice when they need to verify both the source directory and
/// (after rename) the archive directory against the same snapshot.
pub async fn load_source_album_snapshot(
    client: &tokio_postgres::Client,
    import_album_id: Uuid,
) -> Result<
    Option<(
        crate::repositories::import_repository::SourceAlbumSnapshotRecord,
        Vec<SnapshotFileRecord>,
    )>,
    AppError,
> {
    let Some(snapshot) =
        ImportRepository::get_source_album_snapshot(client, import_album_id).await?
    else {
        return Ok(None);
    };
    let files = ImportRepository::get_snapshot_files(client, snapshot.snapshot_id).await?;
    Ok(Some((snapshot, files)))
}

/// Load the persisted snapshot header + files for `import_album_id` and
/// verify the on-disk directory against it.
///
/// Phase 5: the blocking directory walk + hashing is isolated inside
/// `spawn_blocking` so the async runtime is never blocked by a large
/// album verification.
///
/// P2: acquires the same `SNAPSHOT_CONCURRENCY` permit as capture so the
/// combined snapshot load is bounded.
#[cfg(all(test, feature = "real-db-tests"))]
pub async fn verify_source_album_snapshot(
    client: &tokio_postgres::Client,
    import_album_id: Uuid,
    source_album_path: &Path,
) -> Result<Vec<SnapshotVerifyError>, AppError> {
    verify_source_album_snapshot_with_cancel(client, import_album_id, source_album_path, None).await
}

pub async fn verify_source_album_snapshot_with_cancel(
    client: &tokio_postgres::Client,
    import_album_id: Uuid,
    source_album_path: &Path,
    cancelled: Option<Arc<AtomicBool>>,
) -> Result<Vec<SnapshotVerifyError>, AppError> {
    let snapshot =
        match ImportRepository::get_source_album_snapshot(client, import_album_id).await? {
            Some(s) => s,
            None => {
                return Err(AppError::Internal(format!(
                    "no snapshot found for album {import_album_id}"
                )));
            }
        };
    let stored_files = ImportRepository::get_snapshot_files(client, snapshot.snapshot_id).await?;

    // Acquire a concurrency permit before offloading the blocking walk.
    let _permit = SNAPSHOT_CONCURRENCY
        .acquire()
        .await
        .map_err(|e| AppError::Internal(format!("snapshot semaphore closed: {e}")))?;
    // Move the blocking walk + hash off the async worker.
    let album_path = source_album_path.to_path_buf();
    let snapshot_hash = snapshot.snapshot_hash.clone();
    let stored_files_owned: Vec<SnapshotFileRecord> = stored_files;
    let cancel_for_task = cancelled.clone();
    tokio::task::spawn_blocking(move || {
        verify_source_snapshot_files_with_cancel(
            &album_path,
            &snapshot_hash,
            &stored_files_owned,
            cancel_for_task.as_deref(),
        )
    })
    .await
    .map_err(|e| AppError::Internal(format!("snapshot verify task failed: {e}")))?
}

/// Async wrapper for [`verify_source_snapshot_files`] that isolates the
/// blocking directory walk + hashing on a `spawn_blocking` task. Used by
/// async callers (commit Phase 6 source-archive verify, recovery). The
/// synchronous [`verify_source_snapshot_files`] is kept for unit tests and
/// any synchronous caller.
pub async fn verify_source_snapshot_files_async(
    source_album_path: &Path,
    snapshot_hash: Vec<u8>,
    stored_files: Vec<SnapshotFileRecord>,
) -> Result<Vec<SnapshotVerifyError>, AppError> {
    let album_path = source_album_path.to_path_buf();
    // Acquire a concurrency permit so the combined snapshot load (capture +
    // verify across many albums) is bounded.
    let _permit = SNAPSHOT_CONCURRENCY
        .acquire()
        .await
        .map_err(|e| AppError::Internal(format!("snapshot semaphore closed: {e}")))?;
    tokio::task::spawn_blocking(move || {
        verify_source_snapshot_files(&album_path, &snapshot_hash, &stored_files)
    })
    .await
    .map_err(|e| AppError::Internal(format!("snapshot verify task failed: {e}")))?
}

/// Verify a source album after selected frozen-plan files may already have
/// been removed. Every file that remains must still match the original full
/// snapshot, there may be no new files, and only paths explicitly listed in
/// `allowed_missing` may be absent. This is the recovery-safe counterpart to
/// the exact snapshot verifier used before the first removal.
pub async fn verify_source_snapshot_files_allowing_missing_async(
    source_album_path: &Path,
    stored_files: Vec<SnapshotFileRecord>,
    allowed_missing: std::collections::HashSet<String>,
) -> Result<Vec<SnapshotVerifyError>, AppError> {
    let album_path = source_album_path.to_path_buf();
    let _permit = SNAPSHOT_CONCURRENCY
        .acquire()
        .await
        .map_err(|e| AppError::Internal(format!("snapshot semaphore closed: {e}")))?;
    tokio::task::spawn_blocking(move || {
        let actual_files = collect_album_files_with_cancel(&album_path, None)?;
        let stored_map: std::collections::HashMap<&str, &SnapshotFileRecord> = stored_files
            .iter()
            .map(|file| (file.relative_path.as_str(), file))
            .collect();
        let actual_map: std::collections::HashMap<&str, &NewSnapshotFile> = actual_files
            .iter()
            .map(|file| (file.relative_path.as_str(), file))
            .collect();
        let mut errors = Vec::new();

        for (path, stored) in &stored_map {
            match actual_map.get(path) {
                None if allowed_missing.contains(*path) => {}
                None => errors.push(SnapshotVerifyError::MissingFile(path.to_string())),
                Some(actual) => {
                    if stored.file_size != actual.file_size {
                        errors.push(SnapshotVerifyError::SizeMismatch {
                            path: path.to_string(),
                            expected: stored.file_size,
                            actual: actual.file_size,
                        });
                    }
                    if stored.blake3 != actual.blake3 {
                        errors.push(SnapshotVerifyError::HashMismatch {
                            path: path.to_string(),
                        });
                    }
                    if stored.file_type != actual.file_type {
                        errors.push(SnapshotVerifyError::FileTypeMismatch {
                            path: path.to_string(),
                            expected: stored.file_type.clone(),
                            actual: actual.file_type.clone(),
                        });
                    }
                }
            }
        }
        for path in actual_map.keys() {
            if !stored_map.contains_key(path) {
                errors.push(SnapshotVerifyError::ExtraFile(path.to_string()));
            }
        }
        Ok(errors)
    })
    .await
    .map_err(|e| AppError::Internal(format!("snapshot verify task failed: {e}")))?
}

/// Pure verifier: compare `source_album_path` against a previously captured
/// set of snapshot files + expected snapshot hash. Used by commit Phase 6
/// and recovery once the snapshot is already loaded into memory.
pub fn verify_source_snapshot_files(
    source_album_path: &Path,
    snapshot_hash: &[u8],
    stored_files: &[SnapshotFileRecord],
) -> Result<Vec<SnapshotVerifyError>, AppError> {
    verify_source_snapshot_files_with_cancel(source_album_path, snapshot_hash, stored_files, None)
}

fn verify_source_snapshot_files_with_cancel(
    source_album_path: &Path,
    snapshot_hash: &[u8],
    stored_files: &[SnapshotFileRecord],
    cancelled: Option<&AtomicBool>,
) -> Result<Vec<SnapshotVerifyError>, AppError> {
    let actual_files = collect_album_files_with_cancel(source_album_path, cancelled)?;

    let mut errors = Vec::new();

    let stored_map: std::collections::HashMap<&str, &SnapshotFileRecord> = stored_files
        .iter()
        .map(|f| (f.relative_path.as_str(), f))
        .collect();
    let actual_map: std::collections::HashMap<&str, &NewSnapshotFile> = actual_files
        .iter()
        .map(|f| (f.relative_path.as_str(), f))
        .collect();

    for (path, stored) in &stored_map {
        match actual_map.get(path) {
            None => errors.push(SnapshotVerifyError::MissingFile(path.to_string())),
            Some(actual) => {
                if stored.file_size != actual.file_size {
                    errors.push(SnapshotVerifyError::SizeMismatch {
                        path: path.to_string(),
                        expected: stored.file_size,
                        actual: actual.file_size,
                    });
                }
                if stored.blake3 != actual.blake3 {
                    errors.push(SnapshotVerifyError::HashMismatch {
                        path: path.to_string(),
                    });
                }
                if stored.file_type != actual.file_type {
                    errors.push(SnapshotVerifyError::FileTypeMismatch {
                        path: path.to_string(),
                        expected: stored.file_type.clone(),
                        actual: actual.file_type.clone(),
                    });
                }
            }
        }
    }
    for path in actual_map.keys() {
        if !stored_map.contains_key(path) {
            errors.push(SnapshotVerifyError::ExtraFile(path.to_string()));
        }
    }

    let stored_hash = compute_snapshot_hash(
        &stored_files
            .iter()
            .map(|f| NewSnapshotFile {
                relative_path: f.relative_path.clone(),
                file_type: f.file_type.clone(),
                file_size: f.file_size,
                blake3: f.blake3.clone(),
            })
            .collect::<Vec<_>>(),
    );
    if stored_hash != snapshot_hash {
        errors.push(SnapshotVerifyError::SnapshotHashMismatch {
            expected: hex_encode(snapshot_hash),
            actual: hex_encode(&stored_hash),
        });
    }

    let actual_hash = compute_snapshot_hash(&actual_files);
    if actual_hash != snapshot_hash {
        errors.push(SnapshotVerifyError::SnapshotHashMismatch {
            expected: hex_encode(snapshot_hash),
            actual: hex_encode(&actual_hash),
        });
    }

    Ok(errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_snapshot_file(path: &str, ft: &str, size: i64, hash: u8) -> NewSnapshotFile {
        NewSnapshotFile {
            relative_path: path.to_string(),
            file_type: ft.to_string(),
            file_size: size,
            blake3: vec![hash; 32],
        }
    }

    fn make_record(path: &str, ft: &str, size: i64, hash: &[u8]) -> SnapshotFileRecord {
        SnapshotFileRecord {
            id: Uuid::new_v4(),
            snapshot_id: Uuid::new_v4(),
            relative_path: path.to_string(),
            file_type: ft.to_string(),
            file_size: size,
            blake3: hash.to_vec(),
        }
    }

    #[test]
    fn snapshot_hash_stable_and_order_independent() {
        let a = vec![
            make_snapshot_file("a.jpg", "regular_file", 100, 1),
            make_snapshot_file("b.png", "regular_file", 200, 2),
        ];
        let b = vec![
            make_snapshot_file("b.png", "regular_file", 200, 2),
            make_snapshot_file("a.jpg", "regular_file", 100, 1),
        ];
        let ha = compute_snapshot_hash(&a);
        let hb = compute_snapshot_hash(&b);
        assert_eq!(ha, hb);
        assert_eq!(ha.len(), 32);
    }

    #[test]
    fn verify_observes_cancellation_before_hashing() {
        let tmp = TempDir::new().unwrap();
        let album = tmp.path().join("cancelled_verify");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("large.bin"), vec![0xAB; 1024 * 1024]).unwrap();
        let cancelled = AtomicBool::new(true);

        let error = verify_source_snapshot_files_with_cancel(&album, &[], &[], Some(&cancelled))
            .expect_err("cancelled verification must stop before hashing the album");
        assert!(error.to_string().contains("cancelled"));
    }

    #[test]
    fn snapshot_hash_sensitive_to_size_and_path() {
        let a = vec![make_snapshot_file("a.jpg", "regular_file", 100, 1)];
        let b = vec![make_snapshot_file("a.jpg", "regular_file", 101, 1)];
        let c = vec![make_snapshot_file("b.jpg", "regular_file", 100, 1)];
        assert_ne!(compute_snapshot_hash(&a), compute_snapshot_hash(&b));
        assert_ne!(compute_snapshot_hash(&a), compute_snapshot_hash(&c));
    }

    #[test]
    fn collect_album_files_includes_nested_and_sidecars() {
        let tmp = TempDir::new().unwrap();
        let album = tmp.path().join("album_x");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("photo.jpg"), b"jpg-bytes").unwrap();
        std::fs::write(album.join("description.txt"), b"notes").unwrap();
        let nested = album.join("sub");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("meta.xmp"), b"<x/>").unwrap();

        let files = collect_album_files(&album).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.relative_path.as_str()).collect();
        assert!(paths.contains(&"photo.jpg"));
        assert!(paths.contains(&"description.txt"));
        assert!(paths.contains(&"sub/meta.xmp"));
        for f in &files {
            assert!(!f.relative_path.starts_with('/'));
            assert!(!f.relative_path.contains(".."));
            assert!(!f.relative_path.contains('\\'));
            assert!(!f.blake3.is_empty());
        }
        assert_eq!(files.len(), 3);
    }

    #[test]
    fn verify_detects_extra_missing_and_hash_mismatch() {
        let tmp = TempDir::new().unwrap();
        let album = tmp.path().join("v_album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("keep.jpg"), b"keep-bytes").unwrap();
        std::fs::write(album.join("extra.jpg"), b"extra-bytes").unwrap();

        let stored = vec![
            make_record("keep.jpg", "regular_file", 10, &[0xAA; 32]),
            make_record("missing.jpg", "regular_file", 7, &[0xBB; 32]),
        ];
        let snapshot_hash = compute_snapshot_hash(&[
            make_snapshot_file("keep.jpg", "regular_file", 10, 0xAA),
            make_snapshot_file("missing.jpg", "regular_file", 7, 0xBB),
        ]);

        let errors = verify_source_snapshot_files(&album, &snapshot_hash, &stored).unwrap();
        let has_extra = errors
            .iter()
            .any(|e| matches!(e, SnapshotVerifyError::ExtraFile(p) if p == "extra.jpg"));
        let has_missing = errors
            .iter()
            .any(|e| matches!(e, SnapshotVerifyError::MissingFile(p) if p == "missing.jpg"));
        assert!(has_extra, "extra file must be reported: {errors:?}");
        assert!(has_missing, "missing file not reported: {errors:?}");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, SnapshotVerifyError::SnapshotHashMismatch { .. })),
            "snapshot hash mismatch not reported: {errors:?}"
        );
    }

    #[test]
    fn verify_accepts_exact_match() {
        let tmp = TempDir::new().unwrap();
        let album = tmp.path().join("ok_album");
        std::fs::create_dir_all(&album).unwrap();
        let data_a = b"alpha-data";
        let data_b = b"beta-data";
        std::fs::write(album.join("a.jpg"), data_a).unwrap();
        let nested = album.join("sub");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("b.txt"), data_b).unwrap();

        let files = collect_album_files(&album).unwrap();
        let snapshot_hash = compute_snapshot_hash(&files);
        let stored: Vec<SnapshotFileRecord> = files
            .iter()
            .map(|f| SnapshotFileRecord {
                id: Uuid::new_v4(),
                snapshot_id: Uuid::new_v4(),
                relative_path: f.relative_path.clone(),
                file_type: f.file_type.clone(),
                file_size: f.file_size,
                blake3: f.blake3.clone(),
            })
            .collect();
        let errors = verify_source_snapshot_files(&album, &snapshot_hash, &stored).unwrap();
        assert!(errors.is_empty(), "expected clean verify, got: {errors:?}");
    }

    #[test]
    fn verify_detects_size_and_hash_mismatch() {
        let tmp = TempDir::new().unwrap();
        let album = tmp.path().join("mix_album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("a.jpg"), b"actual").unwrap();

        let files = collect_album_files(&album).unwrap();
        assert_eq!(files.len(), 1);
        let actual = &files[0];

        let wrong_size_stored = vec![make_record(
            "a.jpg",
            "regular_file",
            actual.file_size + 999,
            &actual.blake3,
        )];
        let wrong_size_hash = compute_snapshot_hash(&[make_snapshot_file(
            "a.jpg",
            "regular_file",
            actual.file_size + 999,
            actual.blake3[0],
        )]);
        let errs =
            verify_source_snapshot_files(&album, &wrong_size_hash, &wrong_size_stored).unwrap();
        assert!(errs.iter().any(
            |e| matches!(e, SnapshotVerifyError::SizeMismatch { path, .. } if path == "a.jpg")
        ));

        let wrong_hash_stored = vec![make_record(
            "a.jpg",
            "regular_file",
            actual.file_size,
            &[0u8; 32],
        )];
        let wrong_hash_hash = compute_snapshot_hash(&[make_snapshot_file(
            "a.jpg",
            "regular_file",
            actual.file_size,
            0,
        )]);
        let errs2 =
            verify_source_snapshot_files(&album, &wrong_hash_hash, &wrong_hash_stored).unwrap();
        assert!(
            errs2.iter().any(
                |e| matches!(e, SnapshotVerifyError::HashMismatch { path } if path == "a.jpg")
            ),
            "hash mismatch not reported: {errs2:?}"
        );
    }

    /// Batch 3: a real album with nested sidecars, an image, a description
    /// file, and an unrelated "ordinary" sidecar — all must be captured, and
    /// every deviation must be reported:
    ///   - nested file missing
    ///   - extra file on disk
    ///   - excluded sidecar missing from snapshot (extra on disk after capture)
    ///   - BLAKE3 mismatch on the image
    ///   - overall snapshot hash mismatch
    #[test]
    fn verify_full_album_with_nested_sidecars_catches_every_deviation() {
        let tmp = TempDir::new().unwrap();
        let album = tmp.path().join("full_album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("photo.jpg"), b"jpg-bytes-original").unwrap();
        std::fs::write(album.join("description.txt"), b"album notes").unwrap();
        let nested = album.join("sub");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("meta.xmp"), b"<xmp/>").unwrap();

        // Capture the pristine snapshot.
        let pristine = collect_album_files(&album).unwrap();
        let snapshot_hash = compute_snapshot_hash(&pristine);
        let stored: Vec<SnapshotFileRecord> = pristine
            .iter()
            .map(|f| SnapshotFileRecord {
                id: Uuid::new_v4(),
                snapshot_id: Uuid::new_v4(),
                relative_path: f.relative_path.clone(),
                file_type: f.file_type.clone(),
                file_size: f.file_size,
                blake3: f.blake3.clone(),
            })
            .collect();

        // Baseline: clean verify.
        let clean = verify_source_snapshot_files(&album, &snapshot_hash, &stored).unwrap();
        assert!(clean.is_empty(), "baseline must pass: {clean:?}");

        // Now tamper with the album on disk in multiple ways at once:
        //   * nested file removed (MissingFile)
        //   * photo.jpg overwritten (HashMismatch + SnapshotHashMismatch)
        //   * a rogue extra file appears (ExtraFile)
        std::fs::remove_file(nested.join("meta.xmp")).unwrap();
        std::fs::write(album.join("photo.jpg"), b"jpg-bytes-TAMPERED").unwrap();
        std::fs::write(album.join("rogue.bin"), b"something new").unwrap();

        let errors = verify_source_snapshot_files(&album, &snapshot_hash, &stored).unwrap();
        let has_missing = errors
            .iter()
            .any(|e| matches!(e, SnapshotVerifyError::MissingFile(p) if p == "sub/meta.xmp"));
        let has_extra = errors
            .iter()
            .any(|e| matches!(e, SnapshotVerifyError::ExtraFile(p) if p == "rogue.bin"));
        let has_hash = errors.iter().any(
            |e| matches!(e, SnapshotVerifyError::HashMismatch { path } if path == "photo.jpg"),
        );
        let has_snapshot_mismatch = errors
            .iter()
            .any(|e| matches!(e, SnapshotVerifyError::SnapshotHashMismatch { .. }));
        assert!(has_missing, "missing nested file not reported: {errors:?}");
        assert!(has_extra, "extra rogue file not reported: {errors:?}");
        assert!(has_hash, "hash mismatch not reported: {errors:?}");
        assert!(
            has_snapshot_mismatch,
            "snapshot hash mismatch not reported: {errors:?}"
        );
    }

    #[tokio::test]
    async fn partial_snapshot_verifier_allows_only_persisted_selected_file_removals() {
        let tmp = TempDir::new().unwrap();
        let album = tmp.path().join("album");
        std::fs::create_dir_all(album.join("meta")).unwrap();
        std::fs::write(album.join("selected.jpg"), b"selected").unwrap();
        std::fs::write(album.join("excluded.jpg"), b"excluded").unwrap();
        std::fs::write(album.join("meta").join("notes.xmp"), b"sidecar").unwrap();
        let stored: Vec<SnapshotFileRecord> = collect_album_files(&album)
            .unwrap()
            .into_iter()
            .map(|file| SnapshotFileRecord {
                id: Uuid::new_v4(),
                snapshot_id: Uuid::new_v4(),
                relative_path: file.relative_path,
                file_type: file.file_type,
                file_size: file.file_size,
                blake3: file.blake3,
            })
            .collect();

        std::fs::remove_file(album.join("selected.jpg")).unwrap();
        let allowed = std::collections::HashSet::from(["selected.jpg".to_string()]);
        let errors = verify_source_snapshot_files_allowing_missing_async(
            &album,
            stored.clone(),
            allowed.clone(),
        )
        .await
        .unwrap();
        assert!(errors.is_empty(), "{errors:?}");

        std::fs::write(album.join("meta").join("notes.xmp"), b"changed").unwrap();
        let errors = verify_source_snapshot_files_allowing_missing_async(&album, stored, allowed)
            .await
            .unwrap();
        assert!(errors.iter().any(|error| matches!(
            error,
            SnapshotVerifyError::SizeMismatch { path, .. }
                | SnapshotVerifyError::HashMismatch { path }
                if path == "meta/notes.xmp"
        )));
        assert!(album.join("excluded.jpg").exists());
    }

    /// Batch 3: a plan-only view of the album (only imported images) is NOT
    /// a valid substitute for the full snapshot. Prove it by passing a
    /// stored set that omits the sidecar/description files — the verifier
    /// must report them as ExtraFile even though the plan image still matches.
    #[test]
    fn plan_images_are_not_full_snapshot_evidence() {
        let tmp = TempDir::new().unwrap();
        let album = tmp.path().join("plan_only");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("photo.jpg"), b"jpg-bytes").unwrap();
        std::fs::write(album.join("description.txt"), b"album notes").unwrap();

        let full = collect_album_files(&album).unwrap();
        let full_hash = compute_snapshot_hash(&full);

        // Build a plan-like stored set that only knows about the image.
        let plan_only: Vec<SnapshotFileRecord> = full
            .iter()
            .filter(|f| f.relative_path == "photo.jpg")
            .map(|f| SnapshotFileRecord {
                id: Uuid::new_v4(),
                snapshot_id: Uuid::new_v4(),
                relative_path: f.relative_path.clone(),
                file_type: f.file_type.clone(),
                file_size: f.file_size,
                blake3: f.blake3.clone(),
            })
            .collect();
        assert_eq!(plan_only.len(), 1);

        let errors = verify_source_snapshot_files(&album, &full_hash, &plan_only).unwrap();
        let has_extra = errors
            .iter()
            .any(|e| matches!(e, SnapshotVerifyError::ExtraFile(p) if p == "description.txt"));
        assert!(
            has_extra,
            "sidecar not flagged as extra when the stored set only knows plan images: {errors:?}"
        );
    }

    /// Batch 3: nested-only albums (no top-level images, only a nested file)
    /// must be fully captured and verified.
    #[test]
    fn verify_nested_only_album() {
        let tmp = TempDir::new().unwrap();
        let album = tmp.path().join("nested_only");
        let nested = album.join("chapter-1");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("001.jpg"), b"nested-only").unwrap();

        let files = collect_album_files(&album).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].relative_path, "chapter-1/001.jpg");

        let snapshot_hash = compute_snapshot_hash(&files);
        let stored: Vec<SnapshotFileRecord> = files
            .iter()
            .map(|f| SnapshotFileRecord {
                id: Uuid::new_v4(),
                snapshot_id: Uuid::new_v4(),
                relative_path: f.relative_path.clone(),
                file_type: f.file_type.clone(),
                file_size: f.file_size,
                blake3: f.blake3.clone(),
            })
            .collect();
        let errors = verify_source_snapshot_files(&album, &snapshot_hash, &stored).unwrap();
        assert!(errors.is_empty(), "expected clean verify: {errors:?}");

        std::fs::remove_file(nested.join("001.jpg")).unwrap();
        let errs2 = verify_source_snapshot_files(&album, &snapshot_hash, &stored).unwrap();
        assert!(
            errs2.iter().any(
                |e| matches!(e, SnapshotVerifyError::MissingFile(p) if p == "chapter-1/001.jpg")
            ),
            "missing nested file not reported: {errs2:?}"
        );
    }
}
