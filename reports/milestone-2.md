# Milestone 2 Report: Scan & Exact Duplicate Detection

## Summary

Milestone 2 implements the image scanning pipeline: source directory selection,
album discovery, image fingerprinting (BLAKE3 + pixel hash), exact duplicate
detection within albums and against the historical library, persistent storage of
import runs, and a real-time analysis progress page with cancellation support.

## Implementation

### Backend (Rust)

**Domain types (`domain/import_state.rs`)**

- `ImportRunState`: scanning, fingerprinting, detecting_duplicates, completed,
  cancelled, failed.
- `ImportAlbumState`: pending, scanning, fingerprinting, completed, failed.
- `ImportImageState`: pending, fingerprinted, failed.
- `DecodeState`: pending, decoded, failed.
- `DuplicateScope`: intra_album, library.
- `MatchType`: file_exact (same file_size + BLAKE3), pixel_exact (same pixel
  hash, different bytes).
- `ScanProgress` and `ScanSourceInfo` DTOs.
- `SUPPORTED_IMAGE_EXTENSIONS`: jpg, jpeg, png, webp.
- `SCAN_POLICY_VERSION`: "1.0".

**Repository (`repositories/import_repository.rs`)**

- `upsert_default_library_root` - Creates a placeholder library root for
  Milestone 2 import_runs (required by FK constraint).
- `create_import_run` / `update_import_run_state` / `update_import_run_error`
  / `update_import_run_statistics`.
- `insert_import_album` / `update_import_album_state`.
- `insert_import_image` / `update_import_image_fingerprint`.
- `get_import_images_by_album`.
- `insert_duplicate_candidate`.
- `get_library_images_for_comparison` - Reads all library_images for cross-check.
- `count_duplicates_for_run`.

**Scan service (`services/scan_service.rs`)**

- `run_scan` - Core async scan function:
  1. Creates import_run record.
  2. Scans source directory for first-level subdirectories (albums).
  3. Inserts import_album records.
  4. For each album, scans for supported image files and computes fingerprints
     in the background scan task.
  5. Detects intra-album duplicates: file_exact (same file_size + BLAKE3) and
     pixel_exact (same pixel_hash, different encoding).
  6. Detects library duplicates by comparing against all `library_images`.
  7. Persists all results and updates statistics.
- Cancellation: `Arc<AtomicBool>` checked between each image.
- Progress reporting: emits `scan-progress` Tauri events after each significant
  step.
- Does NOT create any `library_images`, `library_albums`, or
  `file_transactions` records. Milestone 2 is read-only for library comparison.

**Commands (`commands/scan.rs`)**

- `validate_source_directory` - Returns album count and names.
- `start_scan` - Spawns background scan task, stores cancellation handle.
- `cancel_scan` - Sets cancellation flag on active scan.
- `get_scan_progress` - Returns current progress snapshot.

**State (`state.rs`)**

- Added `ScanState` with active scan handle (cancellation flag, task join
  handle) and progress snapshot.

### Frontend (React + TypeScript)

**Types and API (`lib/ipc/types.ts`, `lib/ipc/api.ts`)**

- Added `ScanProgress` and `ScanSourceInfo` interfaces.
- Added `validateSourceDirectory`, `startScan`, `cancelScan`, `getScanProgress`
  API methods.

**Router (`hooks/use-router.ts`)**

- Added `scan` route.

**Layout (`components/Layout.tsx`)**

- Added navigation item for scan page.

**Scan page (`pages/ScanPage.tsx`)**

- Source directory path input with validation.
- Album discovery display.
- Start scan button.
- Real-time progress display using Tauri event listener and polling fallback.
- Progress cards: state, current album, processed images, total albums,
  duplicate count, error count.
- Cancel button.
- Error details display.
- Reset button after completion.

**Dashboard (`pages/DashboardPage.tsx`)**

- Replaced "coming soon" section with actionable import button.

## New files

| File                                              | Purpose                                             |
| ------------------------------------------------- | --------------------------------------------------- |
| `src-tauri/src/domain/import_state.rs`            | Import state enums, DTOs, constants                 |
| `src-tauri/src/repositories/import_repository.rs` | DB operations for import tables                     |
| `src-tauri/src/services/scan_service.rs`          | Scan orchestration, fingerprinting, dedup detection |
| `src-tauri/src/commands/scan.rs`                  | Scan IPC commands                                   |
| `src/pages/ScanPage.tsx`                          | Scan UI with progress and cancellation              |

## Modified files

| File                                  | Change                                                 |
| ------------------------------------- | ------------------------------------------------------ |
| `src-tauri/Cargo.toml`                | Added `real-db-tests` feature for ignored real DB scan integration test |
| `src-tauri/src/lib.rs`                | Registered 4 new scan commands                         |
| `src-tauri/src/state.rs`              | Added ScanState and ScanHandle to AppState             |
| `src-tauri/src/domain/mod.rs`         | Added import_state module                              |
| `src-tauri/src/repositories/mod.rs`   | Added import_repository module                         |
| `src-tauri/src/services/mod.rs`       | Added scan_service module                              |
| `src-tauri/src/commands/mod.rs`       | Added scan module                                      |
| `src/hooks/use-router.ts`             | Added scan route                                       |
| `src/components/Layout.tsx`           | Added scan nav item                                    |
| `src/app/App.tsx`                     | Added ScanPage route                                   |
| `src/app/App.test.tsx`                | Added assertion for scan nav button                    |
| `src/pages/DashboardPage.tsx`         | Added import action, onGoScan prop                     |
| `src/lib/ipc/types.ts`                | Added ScanProgress, ScanSourceInfo types               |
| `src/lib/ipc/api.ts`                  | Added scan API methods                                 |
| `src/styles/global.css`               | Added scan page styles                                 |

## No migration changes

The existing schema (0001_initial + 0002_indexes) fully supports Milestone 2.
All required tables (import_runs, import_albums, import_images,
duplicate_candidates) are used as-is. No new migrations were needed.

## Test results

### Commands run

| Command | Result |
| --- | --- |
| `pnpm typecheck` | PASS |
| `pnpm test:unit` | PASS - 4 frontend tests |
| `pnpm rust:test` | PASS - 52 Rust tests, 1 ignored real PostgreSQL lifecycle test |
| `pnpm rust:clippy` | PASS - `-D warnings` |
| `pnpm build` | PASS - release executable built |
| real PostgreSQL lifecycle test | PASS - managed init, pgvector, migrations, shutdown, restart |
| real scan duplicate persistence test | PASS - PostgreSQL + filesystem, feature-gated with `real-db-tests` |
| release executable smoke launch | PASS - stayed running for 5 seconds |

### Rust unit tests in scan_service.rs

| Test                                             | Purpose                                           |
| ------------------------------------------------ | ------------------------------------------------- |
| test_scan_directory_for_albums                   | Discovers subdirectories as albums                 |
| test_scan_album_for_images                       | Filters by supported extensions only               |
| test_scan_empty_directory                        | Returns empty album list                           |
| test_scan_nonexistent_directory                  | Returns error for missing path                     |
| test_hex_to_bytes_roundtrip                      | Hex string to bytes conversion                     |
| test_hex_to_bytes_empty                          | Empty hex produces empty bytes                     |
| test_fingerprint_image_sync                      | Fingerprint produces valid hashes and dimensions   |
| test_duplicate_detection_file_exact              | Exact copy has same BLAKE3                         |
| test_duplicate_detection_renamed_file            | Renamed file has same BLAKE3                       |
| test_duplicate_detection_pixel_identical_different_format | Different formats have different BLAKE3   |
| test_validate_source_directory                   | Validation logic for valid/invalid paths            |
| test_scan_source_info                            | Async source info discovery                        |
| test_supported_extensions                        | Extension list correctness                          |

### Real integration tests

- `real_scan_persists_exact_duplicates` runs with
  `--features real-db-tests` and `IMAGEDB_POSTGRES_BIN` pointing at the local
  PostgreSQL 18.4 binaries. It initializes a real managed PostgreSQL database,
  creates real source files, verifies a renamed exact copy by BLAKE3, verifies a
  PNG metadata variant by pixel hash, persists import images and duplicate
  candidates, confirms no official `library_images` records are created, and
  asserts source file bytes are unchanged after analysis.

### Acceptance criteria mapping

| Criterion                                               | Status                                                              |
| ------------------------------------------------------- | ------------------------------------------------------------------- |
| File rename is still recognized as duplicate             | PASS - BLAKE3 is file-content based, rename does not change it      |
| Metadata-only change recognized by pixel hash            | PASS - pixel hash uses decoded RGBA, ignoring EXIF/metadata         |
| Repeated scans do not create duplicate official records  | PASS - Milestone 2 never writes to library_images or library_albums |
| Cancelled scans leave recoverable state                  | PASS - import_run state set to cancelled, all partial data persisted |
| Analysis does not modify source files                    | PASS - scan is read-only (std::fs::read, no writes)                 |

## Known limitations

1. **No directory picker dialog.** The source directory must be typed manually.
   Adding `tauri-plugin-dialog` or `rfd` for a native directory picker is
   recommended for UX improvement.

2. **Library comparison is empty.** Since Milestone 2 never creates library
   records, `get_library_images_for_comparison` always returns an empty set.
   This will become functional in Milestone 5 when files are published.

3. **Default library root placeholder.** A placeholder `_default_` library root
   is created to satisfy the `import_runs.library_root_id` FK constraint. This
   should be replaced with the actual library root in later milestones.

4. **Sequential fingerprinting.** Images are fingerprinted one at a time inside
   the background scan task. A worker pool could improve throughput for large
   albums.

5. **No cross-album duplicate detection.** Only intra-album and library
   comparisons are performed. Cross-album detection within the same import run
   is not yet implemented.

6. **Progress events use both push and poll.** The frontend listens for Tauri
   events AND polls get_scan_progress. The polling is a fallback; the event
   listener is the primary mechanism.
