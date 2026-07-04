# M7 progress report: external PostgreSQL mode

Date: 2026-07-04

## Implemented

- Added external PostgreSQL connection profiles with TLS mode, CA/client certificate path references, profile name, and connect/query timeouts.
- Moved external database passwords into the OS credential store; normal settings persist only non-secret connection metadata.
- Added structured external preflight checks for connection, PostgreSQL version, TLS mode, pgvector availability/creation, table/schema permissions, read-write state, UTF-8 encoding, time functions, read-only replica detection, and migration/schema compatibility.
- Routed active external profiles through `PostgresManager::connect()` so existing repository and service code can share the same database access path.
- Added managed-to-external migration:
  - refuses non-empty ImageDB targets before switching;
  - creates a managed SQL backup with `pg_dump`;
  - initializes external schema and pgvector;
  - imports data through `psql`;
  - verifies row counts for key ImageDB tables;
  - switches active profile only after verification succeeds.
- Added an explicit settings-page action to switch back to the managed database.
- Added GUI fields and diagnostics for TLS/preflight/migration status.
- Added a real PostgreSQL integration test for managed-to-external migration and included it in `pnpm rust:test:real`.

## Commits

- `8eb63b3 docs: format execution plan docs`
- `bcbfe39 feat: support external postgres preflight and tls`
- `68a7766 feat: migrate managed database to external postgres`
- `f7eb620 feat: allow switching back to managed database`

## Commands run

- `pnpm format:check`
- `pnpm --filter @imagedb/desktop typecheck`
- `pnpm --filter @imagedb/desktop test:unit`
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml`
- `cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets --all-features -- -D warnings`
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features real-db-tests --lib real_migrate_managed_to_external_ -- --ignored --nocapture --test-threads=1`
- `pnpm rust:test:real`

## Test result summary

- Frontend typecheck passed.
- Frontend unit tests passed.
- Rust unit tests passed: 165 passed, 1 ignored.
- Clippy passed with `-D warnings`.
- Real PostgreSQL suite passed, including the new external migration test.

## Actual runtime result

- A real managed PostgreSQL source and a real PostgreSQL target were started from `.local/db-tools/postgresql-18.4/pgsql/bin`.
- The migration test wrote an `app_meta` probe row into the managed source, exported a SQL backup, imported it into the target, verified table counts, switched the active profile, and confirmed the migrated row in the external target.

## Known remaining M7 gaps

- Migration is still a direct command, not a background task with progress events and cancellation.
- TLS negative cases (bad CA, bad hostname, client certificate/key handling) are not yet automated.
- The current migration path supports empty external targets; richer upgrade/merge behavior for an existing populated ImageDB database still needs coverage.
- Failure rollback is covered by refusing to switch on failed preflight, non-empty target, import failure, or row-count mismatch, but there is not yet a dedicated real interruption/cancel test.

M7 is not closed yet. `CURRENT_TASK.md` should remain on `tasks/07-external-postgres.md` until these gaps are resolved.
