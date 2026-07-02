# Milestone 1 Report: App Skeleton & Database

## Summary

Milestone 1 transforms the Milestone 0 technical probe into a structured application
with proper architectural layering, working frontend routing, database lifecycle
management, and initial schema migration support. The core deliverable is the
foundation on which Milestone 2 (image scanning pipeline) will build.

## Implementation

### Backend (Rust)

**Architecture layering: commands -> services -> domain -> repositories -> infrastructure**

- `commands/` - IPC boundary only: DTO conversion, input forwarding, no business logic.
- `services/` - Use-case orchestration. `DatabaseService` coordinates managed and
  external database flows.
- `domain/` - Core types: `DatabaseMode`, `DatabaseStatus`, `ConnectionConfig`,
  `ExternalCheckResult`, `DatabaseState`, `TransactionState`.
- `repositories/` - Data access: `AppMetaRepository` for `app_meta` table CRUD.
- `infrastructure/` - Concrete implementations: PostgreSQL, settings, secrets,
  logging, single instance.

**PostgreSQL lifecycle (`infrastructure/postgres/`)**

- `manager.rs` - Full lifecycle: binary discovery, directory creation, initdb with
  MD5 auth via a dedicated password-only pwfile (the username:password credential
  store is NOT reused as the initdb pwfile), pg_ctl start/stop, credential
  generation, port persistence, health checks, pgvector detection, tokio-postgres
  connection. All `psql` invocations set the `PGPASSWORD` environment variable so
  they authenticate against the MD5-auth cluster without prompts.
- `migration.rs` - Embedded migration runner using `include_str!` for SQL files.
  Creates a `schema_migrations` tracking table, applies pending migrations in
  transactions, supports empty and existing databases.

**Settings store (`infrastructure/settings.rs`)**

- TOML-based persistent settings at `{app_data}/settings.toml`.
- Tracks database mode, library root, external connection params, first-run state.
- Handles corrupt files gracefully (falls back to defaults).

**Credential store (`infrastructure/secrets.rs`)**

- File-based credential storage at `{app_data}/credentials/`.
- Unix permission restriction (0600) when available.
- Note: production should use OS keyring (keyring crate); file-based approach is
  documented as the current-stage limitation.

**Logging (`infrastructure/logging.rs`)**

- Daily rotating log files at `{app_data}/logs/imagedb.log`.
- Uses `tracing-subscriber` with `env-filter` for configurable log levels.
- File-based output with ANSI disabled.

**Single instance (`infrastructure/single_instance.rs`)**

- File-based exclusive lock using `fs2::FileExt`.
- Prevents multiple app instances.
- Lock released on process exit.

**New Tauri commands**

- `get_database_status` - Returns full `DatabaseState` DTO.
- `initialize_managed_database` - Runs full managed init + migrations.
- `test_external_connection` - Version, pgvector, and permission checks. The
  external version check parses the major version as an integer and accepts any
  major version >= 14 (so PostgreSQL 18, 19dev, etc. work without a code change);
  parsing failures surface a clear diagnostic.
- `initialize_external_database` - External connect + run migrations.
- `shutdown_database` - Graceful pg_ctl stop.
- `get_settings` / `update_settings` - Settings CRUD.

### Frontend (React + TypeScript)

**Routing (`hooks/use-router.ts`)**

- Hash-based routing without external dependencies.
- Routes: `dashboard`, `onboarding`, `settings`, `probes`.

**Layout (`components/Layout.tsx`)**

- Sidebar + main content area.
- Navigation: Work, Settings, Tech Probes.

**Error boundary (`components/ErrorBoundary.tsx`)**

- Catches rendering errors with retry capability.

**IPC layer (`lib/ipc/`)**

- `types.ts` - TypeScript interfaces matching Rust DTOs.
- `api.ts` - Typed wrapper around `@tauri-apps/api/core` invoke.

**Pages**

- `OnboardingPage` - First-run flow: choose managed or external, initialize, show
  diagnostics.
- `DashboardPage` - Status cards for database, pgvector, migration state.
- `SettingsPage` - Database status, external connection testing, library root
  config.
- `ProbesPage` - Milestone 0 probe functionality preserved.

## Modified files

| File                                  | Change                                                                                                    |
| ------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| `src-tauri/Cargo.toml`                | Added toml, chrono, fs2, rand, tracing-subscriber, tracing-appender; tokio-postgres serde_json feature    |
| `src-tauri/src/lib.rs`                | Register 12 commands, init logging, single instance lock                                                  |
| `src-tauri/src/state.rs`              | Added SettingsStore and DatabaseService to AppState                                                       |
| `src-tauri/src/error.rs`              | Unchanged                                                                                                 |
| `src-tauri/src/commands/mod.rs`       | Added database and settings_cmd modules                                                                   |
| `src-tauri/src/domain/mod.rs`         | Added DatabaseMode, DatabaseStatus, ConnectionConfig, ExternalCheckResult, DatabaseState, ManagedDbConfig |
| `src-tauri/src/infrastructure/mod.rs` | Added postgres/, settings, secrets, logging, single_instance                                              |
| `src/app/App.tsx`                     | Replaced probe-only UI with routed app shell                                                              |
| `src/app/App.test.tsx`                | Updated tests for new dashboard layout                                                                    |
| `src/styles/global.css`               | Full app layout, sidebar, pages, forms, status cards                                                      |

## New files

| File                                                 | Purpose                                                 |
| ---------------------------------------------------- | ------------------------------------------------------- |
| `src-tauri/src/commands/database.rs`                 | Database management IPC commands                        |
| `src-tauri/src/commands/settings_cmd.rs`             | Settings IPC commands                                   |
| `src-tauri/src/services/mod.rs`                      | Service layer module declarations                       |
| `src-tauri/src/services/database_service.rs`         | Database orchestration (managed + external)             |
| `src-tauri/src/repositories/mod.rs`                  | AppMetaRepository                                       |
| `src-tauri/src/infrastructure/postgres/mod.rs`       | Postgres module (manager + migration)                   |
| `src-tauri/src/infrastructure/postgres/manager.rs`   | PostgresManager lifecycle (refactored from postgres.rs) |
| `src-tauri/src/infrastructure/postgres/migration.rs` | MigrationRunner with embedded SQL                       |
| `src-tauri/src/infrastructure/settings.rs`           | TOML settings store                                     |
| `src-tauri/src/infrastructure/secrets.rs`            | File-based credential store                             |
| `src-tauri/src/infrastructure/logging.rs`            | Tracing setup with daily rotation                       |
| `src-tauri/src/infrastructure/single_instance.rs`    | File-based exclusive lock                               |
| `src/components/ErrorBoundary.tsx`                   | React error boundary                                    |
| `src/components/Layout.tsx`                          | Sidebar + main content layout                           |
| `src/hooks/use-router.ts`                            | Hash-based router hook                                  |
| `src/lib/ipc/api.ts`                                 | Typed IPC client                                        |
| `src/lib/ipc/types.ts`                               | TypeScript interfaces                                   |
| `src/pages/OnboardingPage.tsx`                       | First-run database setup                                |
| `src/pages/SettingsPage.tsx`                         | Database and app settings                               |
| `src/pages/DashboardPage.tsx`                        | Main workbench                                          |
| `src/pages/ProbesPage.tsx`                           | Technical probes (M0 preserved)                         |

## Commands run

| Command                  | Result                                                            |
| ------------------------ | ----------------------------------------------------------------- |
| `pnpm install`           | OK                                                                |
| `pnpm typecheck`         | OK - 0 errors                                                     |
| `pnpm test:unit`         | OK - 4/4 tests pass                                               |
| `pnpm rust:test`         | OK - 36/36 tests pass                                             |
| `pnpm rust:clippy`       | OK - 0 warnings                                                   |
| `pnpm build`             | OK - release exe built                                            |
| release exe smoke launch | OK - imagedb-desktop.exe started and stayed running for 5 seconds |

## Test results

### Rust tests (36 total)

| Module                     | Tests                                                                                                                                        | Status |
| -------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- | ------ |
| postgres::manager          | 7 (binary diagnostic, port persistence, invalid port, init without binaries, connection string, credential persistence, password generation) | PASS   |
| postgres::migration        | 2 (embedded SQL verification, version ordering)                                                                                              | PASS   |
| services::database_service | 5 (parse_postgres_major: standard, future 18, 19devel, old 13, malformed)                                                                    | PASS   |
| settings                   | 3 (defaults, save/reload, corrupt recovery)                                                                                                  | PASS   |
| secrets                    | 3 (store/load, missing key, delete)                                                                                                          | PASS   |
| single_instance            | 3 (acquire, double-lock fails, release on drop)                                                                                              | PASS   |
| image_fingerprint          | 8 (determinism, different-images, generate, probe)                                                                                           | PASS   |
| file_transaction           | 5 (success, empty source, staging cleanup, faults)                                                                                           | PASS   |

### Frontend tests (4 total)

| Test                              | Status |
| --------------------------------- | ------ |
| renders dashboard page with title | PASS   |
| renders sidebar navigation        | PASS   |
| renders ImageDB brand in sidebar  | PASS   |
| renders status cards section      | PASS   |

### Database / migration tests

- Without PostgreSQL binary: `test_initialize_without_binaries_returns_unavailable`
  passes and returns a clear diagnostic when binaries are missing.
- Migration embedding: `test_migrations_embedded` passes and verifies SQL files
  are embedded via `include_str!`.
- Migration runner: code compiles and links; runtime execution requires a
  PostgreSQL binary.

## Post-review fixes applied in this revision

This revision addresses the Codex review findings from the previous Milestone 1
submission:

1. **Data directory creation order.** `PostgresManager::initialize` used to call
   `save_port()` and `save_credentials()` before `create_dir_all()`. Those helpers
   write into `self.data_dir`, so the first call failed with "file not found" on
   a fresh install. The code now creates the data directory first, then persists
   the port and credentials.
2. **Dedicated pwfile for `initdb`.** The previous code passed the persistent
   credential file (format `username:password`) to `initdb --pwfile`, but
   `initdb` expects a file containing only the raw password. A dedicated
   `initdb_pwfile` is now written inside the data directory with Unix 0600
   permissions, removed immediately after `initdb` returns. The persistent
   credential store continues to store `username:password` for application use.
3. **`PGPASSWORD` on psql invocations.** The managed cluster is initialized with
   `--auth=md5`, which requires a password. All `psql`-based commands
   (`create_database`, `check_pgvector`, and the existence-check probe) now go
   through a new `psql_command()` helper that sets `PGPASSWORD` in the child
   environment when a password is known. The tokio-postgres path already passes
   the password in the connection string and was unaffected.
4. **External version check accepts any major >= 14.** The previous code matched
   `"PostgreSQL 14"` through `"PostgreSQL 17"` literally, which would reject
   PostgreSQL 18 and any dev builds. The new code uses a dedicated
   `parse_postgres_major()` helper that looks for the `PostgreSQL ` prefix,
   reads the following decimal digits, and compares the integer. Parsing failures
   produce a specific diagnostic rather than a generic "not supported" warning.
   Unit tests cover PostgreSQL 13, 16, 18, 19devel, and malformed banners.
5. **Milestone report encoding.** The previous `reports/milestone-1.md` was
   saved with a non-UTF-8 encoding and showed mojibake characters (for example
   `鈫?` in place of arrows). The report has been rewritten as clean UTF-8.

## Known limitations

1. **No PostgreSQL binary on this machine.** The PostgreSQL lifecycle (initdb,
   pg_ctl, connection, migration execution) cannot be runtime-verified. All code
   paths are implemented and compile-tested; the diagnostic path (binaries
   missing -> clear GUI state) IS verified via unit tests.
2. **Credential storage.** Uses file-based storage with Unix permission
   restriction. Production should migrate to OS keyring (for example, the
   `keyring` crate). This is documented in the code.
3. **No router dependency.** Routing uses hash-based navigation without
   react-router to minimize dependencies. Sufficient for the current page count.

## Acceptance status

| Criterion                                                             | Status                            |
| --------------------------------------------------------------------- | --------------------------------- |
| Frontend routing, layout, error boundary                              | PASS                              |
| Commands / Services / Domain / Repositories / Infrastructure layering | PASS                              |
| Managed PostgreSQL lifecycle code (initdb, pg_ctl, health, pgvector)  | IMPLEMENTED, not runtime-verified |
| External PostgreSQL connection test code                              | IMPLEMENTED, not runtime-verified |
| Migration runner with embedded SQL                                    | IMPLEMENTED, not runtime-verified |
| Settings storage (TOML)                                               | PASS (unit tested)                |
| Credential storage (file-based)                                       | PASS (unit tested)                |
| Logging (daily rotation)                                              | PASS                              |
| Single instance lock                                                  | PASS (unit tested)                |
| First-run page and database settings page                             | PASS                              |
| Database state visible in GUI                                         | PASS                              |
| Initial schema + migration tests                                      | PASS (compile + embed verified)   |
| Runtime database initialization                                       | BLOCKED - no PostgreSQL binary    |
| Runtime restart and reconnect                                         | BLOCKED - no PostgreSQL binary    |

CURRENT_TASK.md is not advanced. Runtime acceptance of the database lifecycle
("first launch completes database initialization" and "post-restart reconnects to
the existing database") is blocked by the absence of a PostgreSQL binary on this
machine; the review explicitly requires real database verification, not just
compilation and unit tests.
