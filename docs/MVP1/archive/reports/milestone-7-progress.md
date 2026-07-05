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
- Added explicit diagnostics for unreachable active external profiles that direct the GUI/user to the controlled switch-to-managed action without modifying external data.
- Added a real empty external PostgreSQL initialization test that:
  - starts a real empty target database;
  - verifies version, pgvector, extension creation, table/schema permissions, read-write state, UTF-8 encoding, time functions, read-only status, migration state, and schema compatibility preflight fields;
  - initializes pgvector, ImageDB migrations, and `app_meta`;
  - persists the external profile only after initialization succeeds.
- Added a real external-unreachable fallback test that:
  - activates a real external PostgreSQL target and writes an external probe row;
  - shuts down the external target and verifies `get_state` reports controlled switch-to-managed diagnostics without switching automatically;
  - explicitly switches back to the managed database and verifies managed pgvector connectivity;
  - restarts the external target and confirms the external probe row was not modified by fallback.
- Hardened managed PostgreSQL startup after an external profile was active by treating `pg_ctl start` warnings with a ready/already-running log as a usable running server before continuing health checks.
- Strengthened managed-to-external migration verification before profile switch:
  - includes `import_plan_albums` and `source_album_snapshot_files` in migration table checks;
  - verifies row counts and table content fingerprints across managed/external databases;
  - verifies migrated public constraints and indexes;
  - performs an external read/write smoke check before activation;
  - keeps the active profile unswitched if any verification step fails.
- Added Settings page GUI coverage for the external PostgreSQL flow:
  - browser-opened the Vite desktop UI and verified the Settings page renders the external database fields, TLS mode selector, test/migrate/cancel buttons, and no console errors before IPC actions;
  - added component tests for external preflight check rendering;
  - added component tests for migration progress, backup path, row-count table, diagnostics, errors, and enabled cancel state.

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
- External-unreachable managed fallback diagnostics implemented in the current M7 update.
- Empty external PostgreSQL preflight and initialization coverage implemented in the current M7 update.
- External-unreachable controlled managed fallback real coverage implemented in the current M7 update.
- Managed-to-external backup and verification hardening implemented in the current M7 update.
- Settings page external PostgreSQL GUI diagnostics coverage implemented in the current M7 update.

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
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml external_unreachable_diagnostics_points_to_controlled_managed_fallback`
- `$env:IMAGEDB_POSTGRES_BIN=(Resolve-Path -LiteralPath .local/db-tools/postgresql-18.4/pgsql/bin).Path; cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features real-db-tests --lib real_external_empty_database_ -- --ignored --nocapture --test-threads=1`
- `$env:IMAGEDB_POSTGRES_BIN=(Resolve-Path -LiteralPath .local/db-tools/postgresql-18.4/pgsql/bin).Path; cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features real-db-tests --lib real_external_unreachable_fallback_switches_to_managed_without_touching_external -- --ignored --nocapture --test-threads=1`
- `$env:IMAGEDB_POSTGRES_BIN=(Resolve-Path -LiteralPath .local/db-tools/postgresql-18.4/pgsql/bin).Path; cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features real-db-tests --lib real_migrate_managed_to_external_ -- --ignored --nocapture --test-threads=1`
- `pnpm typecheck`
- `pnpm test:unit`
- `pnpm rust:test`
- `pnpm rust:clippy`
- `pnpm build`
- `pnpm rust:test:real`
- `Start-Process apps/desktop/src-tauri/target/release/imagedb-desktop.exe`
- Browser verification against `http://127.0.0.1:1420/` Settings page

## Test result summary

- Frontend typecheck passed.
- Frontend unit tests passed.
- Rust unit tests passed: 174 passed, 1 ignored.
- Clippy passed with `-D warnings`.
- Real PostgreSQL suite passed, including the external empty database initialization test, external migration test, and external existing-database compatibility tests.
- External migration cancellation regression passed.
- Migration history validation unit tests passed.
- TLS connector negative tests passed for missing client cert/key pairs, malformed CA PEM, and malformed client certificate/key PEM.
- Running migration subprocess cancellation test passed.
- Strict TLS hostname-mismatch tests passed.
- External-unreachable fallback diagnostics unit test passed.
- Empty external PostgreSQL preflight/initialization test passed.
- External-unreachable controlled managed fallback real test passed.
- Managed-to-external migration real test passed with backup SQL inspection, row count checks, content fingerprint diagnostics, constraints/index diagnostics, and read/write smoke diagnostics.
- Tauri release build passed and produced `apps/desktop/src-tauri/target/release/imagedb-desktop.exe`.
- Release executable launch smoke passed: the process started and stayed running for 10 seconds before being stopped.
- Frontend unit tests now include Settings page external PostgreSQL GUI coverage: 7 tests passed across 2 files.
- Browser verification confirmed the Settings page renders the external database connection form, TLS selector, test/migrate/cancel controls, and diagnostics containers. Bare Vite cannot invoke Tauri IPC, so IPC execution remains covered by Tauri commands and real Rust tests.

## Actual runtime result

- A real managed PostgreSQL source and a real PostgreSQL target were started from `.local/db-tools/postgresql-18.4/pgsql/bin`.
- The migration test wrote an `app_meta` probe row into the managed source, exported a SQL backup, imported it into the target, verified table counts, switched the active profile, and confirmed the migrated row in the external target.
- The cancellation regression set the external migration cancellation flag before preflight, observed `cancelled` progress at the `preflight` stage, and confirmed the external profile was not stored or activated.
- A real external target seeded with only `0001_initial` preflighted as compatible, initialized through `initialize_external`, upgraded to `0009_drop_redundant_snapshot_hash`, and verified that the dropped legacy column was gone.
- A real external target seeded with `9999_future` was rejected by preflight as an unknown ImageDB migration history and was not activated.
- TLS connector unit coverage verified that invalid CA/client PEM material and incomplete client cert/key configuration are rejected before attempting an external connection.
- The cancellable command regression started a 30-second child process, cancelled it after startup, finished in under the 5-second timeout, and left migration progress in `cancelled` state.
- A local TLS test server using a `localhost` certificate was trusted by the client; `verify_full` accepted `localhost`, rejected `127.0.0.1`, and `verify_ca` accepted `127.0.0.1`.
- When an active external profile is unreachable, `get_state` now reports a diagnostic instructing the user to use the controlled switch-to-managed action without modifying external data.
- A real empty external PostgreSQL target preflighted with all required capability checks passing, then `initialize_external` created pgvector, applied migrations through `0009_drop_redundant_snapshot_hash`, created `app_meta`, switched the active mode to external, and persisted only non-secret external profile metadata.
- A real external target was activated, shut down, reported as unreachable with controlled fallback diagnostics, explicitly switched back to the managed database, then restarted with its external `app_meta` probe row intact.
- The real managed-to-external migration wrote a SQL backup containing the seeded `m7_migration_probe`, imported it into a real external target, verified row counts, table content fingerprints, constraints/indexes, and external read/write access, then switched the active profile only after those checks passed.
- The release executable at `apps/desktop/src-tauri/target/release/imagedb-desktop.exe` launched successfully and remained alive for the smoke window.
- The Settings page GUI displays structured external preflight checks and migration diagnostics, including backup path, verification row counts, diagnostic details, errors, and cancellation state.

## Known remaining M7 gaps

- No open M7 DoD gaps remain. M7 is ready to hand off to M8 mounted shared storage compatibility.

M7 is closed. `CURRENT_TASK.md` can advance to `tasks/08-mounted-storage.md`.
