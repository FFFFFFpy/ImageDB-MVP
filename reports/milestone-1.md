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

- `manager.rs` - Full lifecycle: binary discovery including
  `IMAGEDB_POSTGRES_BIN`, initdb with MD5 auth via a dedicated password-only
  pwfile, staged persistence of port/credentials after cluster creation,
  pg_ctl start/stop, credential generation, port reuse, health checks,
  pgvector enablement, and tokio-postgres connections for database creation and
  extension checks.
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
| `pnpm rust:test`         | OK - 36 tests pass, 1 real DB lifecycle test ignored by default   |
| `pnpm rust:clippy`       | OK - 0 warnings                                                   |
| `pnpm build`             | OK - release exe built                                            |
| real PostgreSQL lifecycle test | OK - PostgreSQL 18.4 + pgvector 0.8.3 init, migrate, shutdown, restart |
| release exe smoke launch | OK - imagedb-desktop.exe started and stayed running for 5 seconds |

## Test results

### Rust tests (36 total)

| Module                     | Tests                                                                                                                                        | Status |
| -------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- | ------ |
| postgres::manager          | 8 (7 default tests plus ignored real PostgreSQL lifecycle test) | PASS   |
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
- Real managed lifecycle: with `IMAGEDB_POSTGRES_BIN` pointing to
  `.local/db-tools/postgresql-18.4/pgsql/bin`, the ignored integration test
  creates a PostgreSQL 18.4 cluster, enables pgvector 0.8.3, creates the
  `imagedb` database, runs migrations through `0002_indexes`, shuts down,
  restarts from the same data directory, reconnects, and confirms no migrations
  need to be re-applied.

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
3. **Runtime database operations use tokio-postgres.** The managed cluster is
   initialized with `--auth=md5`, and application database creation plus
   pgvector enablement now use tokio-postgres with bounded timeouts instead of
   invoking `psql`, avoiding interactive password prompts and child-process
   hangs.
4. **External version check accepts any major >= 14.** The previous code matched
   `"PostgreSQL 14"` through `"PostgreSQL 17"` literally, which would reject
   PostgreSQL 18 and any dev builds. The new code uses a dedicated
   `parse_postgres_major()` helper that looks for the `PostgreSQL ` prefix,
   reads the following decimal digits, and compares the integer. Parsing failures
   produce a specific diagnostic rather than a generic "not supported" warning.
   Unit tests cover PostgreSQL 13, 16, 18, 19devel, and malformed banners.
5. **Milestone report encoding.** The previous `reports/milestone-1.md` was
   saved with a non-UTF-8 encoding and showed mojibake characters. The report
   has been rewritten as clean UTF-8.
6. **Real PostgreSQL verification.** Local PostgreSQL 18.4 binaries and
   pgvector 0.8.3 were installed under `.local/db-tools` for development
   verification. `real_pgvector_full_lifecycle` is ignored by default but passes
   when `IMAGEDB_POSTGRES_BIN` is set to the local `pgsql/bin` directory.

## Known limitations

1. **Local database binaries are development-only.** The runtime verification
   uses ignored local binaries under `.local/db-tools`; packaging PostgreSQL for
   distribution remains a later delivery concern.
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
| Managed PostgreSQL lifecycle code (initdb, pg_ctl, health, pgvector)  | PASS (runtime verified)           |
| External PostgreSQL connection test code                              | PASS (compile + unit tested)      |
| Migration runner with embedded SQL                                    | PASS (runtime verified)           |
| Settings storage (TOML)                                               | PASS (unit tested)                |
| Credential storage (file-based)                                       | PASS (unit tested)                |
| Logging (daily rotation)                                              | PASS                              |
| Single instance lock                                                  | PASS (unit tested)                |
| First-run page and database settings page                             | PASS                              |
| Database state visible in GUI                                         | PASS                              |
| Initial schema + migration tests                                      | PASS (runtime verified)           |
| Runtime database initialization                                       | PASS                              |
| Runtime restart and reconnect                                         | PASS                              |

CURRENT_TASK.md is advanced to `tasks/02-scan-and-exact-match.md` because the
Milestone 1 acceptance criteria now pass, including real managed database
initialization, pgvector health, migration execution, shutdown, and reconnect to
the existing data directory.
