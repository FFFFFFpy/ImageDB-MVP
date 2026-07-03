# Milestone 5 Report: Formal Import Loop

## Summary

Milestone 5 implements the formal import commit loop. A reviewed import plan is frozen, source snapshots are validated, kept files are staged and BLAKE3-verified, manifests are written, final album directories are published, official library rows are inserted in PostgreSQL, and the full source album is archived only after the commit is confirmed. Re-running a completed album skips it without duplicating files or database records.

## Implementation

### Domain

- Added `Committing` and `Committed` import run states.
- Added `CommitProgress`, `CommitAlbumResult`, `CommitResult`, and `FrozenPlanEntry`.
- Added `FROZEN_PLAN_KEY` for the frozen plan stored in `import_runs.statistics`.

### Repository

- Added import-run, import-album, and import-image queries needed for commit.
- Added library album lookup for target conflict detection and idempotency.
- Added file transaction and file operation insert/update methods.
- Added library root path update support.
- Added migration `0003_commit_indexes.sql` for commit-related indexes.

### Commit Service

The commit service performs the pipeline in this order:

1. Generate or load a frozen import plan.
2. Validate each source file against the stored BLAKE3 snapshot.
3. Reject unknown non-empty target album directories.
4. Skip albums already committed with a matching library record and image count.
5. Copy kept files to `<library_root>/.imagedb/staging/<tx_id>/<album>/`.
6. Verify staged files with BLAKE3.
7. Write manifest JSON under `<library_root>/.imagedb/manifests/`.
8. Publish files to `<library_root>/Albums/<album>/` and verify destination hashes.
9. Insert `library_albums` and `library_images` in one PostgreSQL transaction.
10. Copy the full source album directory to `<source_root>/.imagedb-archive/<album>/`.
11. Clean staging after success.

If the official DB transaction fails, the published directory is removed and no official library rows remain. Cancellation is checked between albums.

### Commands And GUI

- Added `start_import_commit`, `cancel_import_commit`, and `get_commit_progress`.
- Added `CommitPage` with confirmation, polling progress, cancellation, and result summary.
- Added the `commit` route, navigation entry, IPC types, and API bindings.
- Progress uses `get_commit_progress` polling; the service layer is independent of Tauri event types so real integration test binaries do not pull GUI DLL imports.

## Tests And Verification

- `pnpm typecheck`: passed.
- `pnpm test:unit`: 4 frontend tests passed.
- `pnpm rust:test`: 100 passed, 1 ignored.
- `pnpm rust:clippy`: passed with `-D warnings`.
- `pnpm build`: passed and produced `apps/desktop/src-tauri/target/release/imagedb-desktop.exe`.
- Real PostgreSQL + filesystem M5 test passed:
  `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features real-db-tests real_commit_full_pipeline -- --ignored --nocapture --test-threads=1`.
- Release executable smoke check passed.

The real commit test verifies source validation, staging, BLAKE3 verification, manifest creation, publish, PostgreSQL library records, full source album archive including sidecar files, and idempotent rerun skip without duplicate `library_images`.

## Modified Files

- `CURRENT_TASK.md`
- `apps/desktop/src-tauri/migrations/0003_commit_indexes.sql`
- `apps/desktop/src-tauri/src/commands/commit.rs`
- `apps/desktop/src-tauri/src/commands/mod.rs`
- `apps/desktop/src-tauri/src/domain/import_state.rs`
- `apps/desktop/src-tauri/src/infrastructure/postgres/migration.rs`
- `apps/desktop/src-tauri/src/lib.rs`
- `apps/desktop/src-tauri/src/repositories/import_repository.rs`
- `apps/desktop/src-tauri/src/services/commit_service.rs`
- `apps/desktop/src-tauri/src/services/mod.rs`
- `apps/desktop/src-tauri/src/state.rs`
- `apps/desktop/src/app/App.tsx`
- `apps/desktop/src/components/Layout.tsx`
- `apps/desktop/src/hooks/use-router.ts`
- `apps/desktop/src/lib/ipc/api.ts`
- `apps/desktop/src/lib/ipc/types.ts`
- `apps/desktop/src/pages/CommitPage.tsx`
- `apps/desktop/src/styles/global.css`

## Known Limitations

- Frozen plans are stored in `import_runs.statistics` rather than a dedicated table.
- Albums are committed sequentially.
- Mid-album recovery is deferred to Milestone 6.
- Archive currently copies source albums and leaves originals intact.
- Album source names are used as target relative paths.
- Commit progress is in-memory; recovery from persisted transaction evidence is Milestone 6.
