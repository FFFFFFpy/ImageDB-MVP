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
- Converted managed-to-external migration into a background task with structured progress, polling IPC, GUI status display, and user cancellation.
- Added cancellation checks before preflight, managed source activation, target preparation, backup, import, verification, and final profile switch.
- Made `pg_dump` and `psql` import cancellable while their child processes are running; cancellation leaves the active profile unswitched and removes an incomplete temporary dump.
- Added a cancellation regression test that verifies a preflight-stage cancel does not persist or activate an external profile.
- Hardened ImageDB migration history compatibility checks for external databases:
  - contiguous known migration prefixes are accepted and upgradeable;
  - unknown or future migration versions are rejected before activation;
  - pending migrations refuse incompatible histories before applying SQL.
- Added real external existing-database tests for upgrading a `0001_initial` database to the current head and rejecting an unknown `9999_future` database.
- Added real client certificate/private key handling for external TLS profiles:
  - CA certificate bundles are parsed and loaded into the TLS connector;
  - client certificate and private key paths must be provided together;
  - client identity is loaded from PEM certificate plus unencrypted PKCS#8 PEM key;
  - malformed CA/client PEM inputs fail before any database profile can be activated.
- Added a running-child cancellation regression for the migration command runner. The test starts a long-running subprocess, requests cancellation after it has started, verifies it exits promptly, and confirms progress is marked `cancelled` without switching profiles.
- Added a local TLS server hostname-mismatch regression:
  - a trusted certificate for `localhost` succeeds under `verify_full` when connecting as `localhost`;
  - the same trusted certificate is rejected under `verify_full` when connecting as `127.0.0.1`;
  - `verify_ca` accepts the trusted certificate with the hostname mismatch, proving the TLS modes differ intentionally.

## Commits

- `8eb63b3 docs: format execution plan docs`
- `bcbfe39 feat: support external postgres preflight and tls`
- `68a7766 feat: migrate managed database to external postgres`
- `f7eb620 feat: allow switching back to managed database`
- Background migration progress and cancellation implemented in the current M7 update.
- Existing external ImageDB database upgrade/reject coverage implemented in the current M7 update.
- Client certificate/private key TLS loading and negative PEM validation implemented in the current M7 update.
- Running migration subprocess cancellation implemented in the current M7 update.
- Strict TLS hostname verification implemented in the current M7 update.

## Commands run

- `pnpm format:check`
- `pnpm --filter @imagedb/desktop typecheck`
- `pnpm --filter @imagedb/desktop test:unit`
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml`
- `cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets --all-features -- -D warnings`
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features real-db-tests --lib real_migrate_managed_to_external_ -- --ignored --nocapture --test-threads=1`
- `pnpm rust:test:real`
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml migrate_managed_to_external_cancelled_before_preflight_never_switches`
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml validate_applied_versions`
- `$env:IMAGEDB_POSTGRES_BIN=(Resolve-Path -LiteralPath .local/db-tools/postgresql-18.4/pgsql/bin).Path; cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features real-db-tests --lib real_external_existing_database_ -- --ignored --nocapture --test-threads=1`
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml tls_connector_`
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml cancellable_migration_command_kills_running_child_and_marks_progress -- --nocapture`
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml trusted_certificate_hostname_mismatch -- --nocapture`

## Test result summary

- Frontend typecheck passed.
- Frontend unit tests passed.
- Rust unit tests passed: 173 passed, 1 ignored.
- Clippy passed with `-D warnings`.
- Real PostgreSQL suite passed, including the external migration test and external existing-database compatibility tests.
- External migration cancellation regression passed.
- Migration history validation unit tests passed.
- TLS connector negative tests passed for missing client cert/key pairs, malformed CA PEM, and malformed client certificate/key PEM.
- Running migration subprocess cancellation test passed.
- Strict TLS hostname-mismatch tests passed.

## Actual runtime result

- A real managed PostgreSQL source and a real PostgreSQL target were started from `.local/db-tools/postgresql-18.4/pgsql/bin`.
- The migration test wrote an `app_meta` probe row into the managed source, exported a SQL backup, imported it into the target, verified table counts, switched the active profile, and confirmed the migrated row in the external target.
- The cancellation regression set the external migration cancellation flag before preflight, observed `cancelled` progress at the `preflight` stage, and confirmed the external profile was not stored or activated.
- A real external target seeded with only `0001_initial` preflighted as compatible, initialized through `initialize_external`, upgraded to `0009_drop_redundant_snapshot_hash`, and verified that the dropped legacy column was gone.
- A real external target seeded with `9999_future` was rejected by preflight as an unknown ImageDB migration history and was not activated.
- TLS connector unit coverage verified that invalid CA/client PEM material and incomplete client cert/key configuration are rejected before attempting an external connection.
- The cancellable command regression started a 30-second child process, cancelled it after startup, finished in under the 5-second timeout, and left migration progress in `cancelled` state.
- A local TLS test server using a `localhost` certificate was trusted by the client; `verify_full` accepted `localhost`, rejected `127.0.0.1`, and `verify_ca` accepted `127.0.0.1`.

## Known remaining M7 gaps

- Failure rollback is covered by refusing to switch on failed preflight, non-empty target, import failure, row-count mismatch, preflight cancellation, and running child-process cancellation.

M7 is not closed yet. `CURRENT_TASK.md` should remain on `tasks/07-external-postgres.md` until these gaps are resolved.
