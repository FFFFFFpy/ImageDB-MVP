# M6.5–M9 Closure Report

## Summary

Date: 2026-07-05
Branch: `core_fix_m5_m6_refactor`
Head commit: `d9b3e5c` (this report was written before the docs commit; the
final closure head is the docs commit that adds this file).

This round closed the M6.5–M9 framework into a verifiable main chain. The
prior `reports/M6.5-M9-final.md` claimed completion but several items were
not actually wired (the commit page re-derived the plan dynamically; the
latest-committable-run query let an old completed run preempt a newer
ready-to-commit run; real-DB tests skipped when the runtime was missing).
Those defects are fixed and verified against real PostgreSQL 18.4 + pgvector
0.8.3 and the real filesystem.

## Completed Work

### M6.5 Managed PostgreSQL Runtime

- Runtime packaging: `scripts/package-postgres-runtime.mjs` produces
  `binaries/windows-x86_64/postgres-runtime/{bin,lib,share}` with the
  required exes, `lib/vector.dll`, and `share/extension/vector*`.
- pgvector packaging: included in the runtime dir.
- Resource lookup: `tauri.conf.json` bundles the runtime as a Windows
  resource; `lib.rs` exposes `resource_dir/postgres-runtime` via
  `IMAGEDB_POSTGRES_RUNTIME_DIR`; `PostgresManager::locate_binaries` reads
  it first.
- Clean bootstrap: real test `real_packaged_runtime_clean_bootstrap` runs
  initdb → start → `CREATE EXTENSION vector` → full migration using only
  the packaged runtime, with no system PostgreSQL.
- Missing-runtime error: the user-facing message is now
  "安装包不完整：缺少内置 PostgreSQL 运行文件，请重新安装 ImageDB." — it
  no longer tells the user to install PostgreSQL.
- Release gate: `run-m9-release-gate.mjs` now runs
  `pnpm release:verify-artifacts` after the build, followed by the explicit
  `install-gate` step. `verify-artifacts` checks artifact and runtime file
  presence only; `install-gate` is the step that validates NSIS install,
  same-version overwrite install, launch smoke, uninstall, and data
  retention. Non-Windows runs must fail unless `--skip-install-gate` is
  passed explicitly, and that skip marks release sign-off incomplete.

### Real Test Fail-Fast

- Runtime-missing behavior: every real-DB test that previously skipped now
  panics with a message naming the expected path and the fix. Affected
  suites: M9 main chain, M9 public recovery, M9 diagnostics, M9
  performance, managed pgvector lifecycle, formal commit pipeline, scan
  persistence + snapshot, review persistence, the 5 external-PostgreSQL
  tests, and the 3 protocol integration tests.
- `run-real-rust-tests.mjs` pre-flights the runtime and aborts before cargo
  if neither `IMAGEDB_POSTGRES_BIN` nor the default
  `.local/db-tools/postgresql-18.4/pgsql/bin` exists.
- The release gate's `real` step is marked `needsPostgresBin`.

### M7 External PostgreSQL

- TLS connector service path: `infrastructure::postgres::external::connect_external`
  is used by `DatabaseService::get_state`, `test_external_connection`,
  `initialize_external`, and `migrate_managed_to_external`. `NoTls` is used
  only for `TlsMode::Disable` (and for managed-local loopback).
- TLS modes: `disable` / `require` / `verify-ca` / `verify-full` all
  implemented (`external.rs:28-46`, `build_tls_connector`).
- Preflight checks: PG version ≥14, pgvector availability, CREATE EXTENSION,
  table creation, schema creation, read-only, encoding, timezone, and
  migration state (`database_service.rs::test_external_connection`).
- UI diagnostics: `OnboardingPage.tsx` and `SettingsPage.tsx` surface TLS
  mode, CA/client cert paths, and per-check results.

### M8 Mounted Storage

- Capability checks: `probe_storage_capabilities` covers read, write,
  same-dir/cross-dir/directory/overwrite rename, file + directory fsync,
  case sensitivity, Unicode normalization, max path component, max path,
  file lock, timestamp precision, and free space.
- `classify_publish_strategy` returns `Unsupported` if any required
  capability is `Unsupported`, so a disconnect/permission error cannot
  produce a false-success publish.
- Gate command: the release gate's `mounted` step runs
  `mounted_storage_gate_library_root_disconnect_pauses_then_recovers`
  via a Windows loopback SMB mapping (or `IMAGEDB_MOUNTED_LIBRARY_ROOT`).

### M9 Public Workflow

- Freeze plan: `freeze_import_plan` IPC writes the three plan tables
  (`import_plans`, `import_plan_albums`, `import_plan_images`) + `plan_hash`
  - plan state `frozen` + run state `ready_to_commit` in a single
    `BEGIN`/`COMMIT` transaction (`ImportRepository::freeze_import_plan_transactionally`).
- Frozen plan summary: `get_frozen_import_plan_summary` IPC reads the
  persisted view (`ImportRepository::load_frozen_plan_summary`) — kept
  images, total albums/images, excluded count, and skipped albums are all
  derived from the frozen rows + run's import albums/images, never
  re-derived from candidates/reviews.
- Commit plan consistency: the commit page now reads the frozen summary
  (`api.getFrozenImportPlanSummary`); the commit service loads the same
  frozen plan via `load_frozen_plan` and validates `plan_hash`. The review
  page calls `freezeImportPlan` (idempotent) instead of `generateImportPlan`.
- Latest committable run: the query now restricts to `ready_to_commit` and
  resubmittable `cancelled` (no active file transaction), ordered
  ready_to_commit-first then `started_at DESC`. `completed` no longer
  enters the default commit page; `recovery_required` routes to recovery.
- Scan completion entry: `review_required` and `ready_to_commit` both enter
  Review / import review. A scan result no longer jumps straight to Commit
  before the frozen plan is created.
- New real tests: `m9_freeze_plan_idempotent_and_summary_matches_commit_set`
  (idempotent re-freeze + summary matches commit set) and
  `m9_committable_run_prefers_ready_over_old_completed` (old completed run
  does not preempt a newer ready_to_commit run).

## Manual Main-Chain Acceptance

On 2026-07-05 the local MVP main chain was manually run from a fresh start
through managed local PostgreSQL initialization, source directory selection,
import / analysis, review, import-plan generation / freeze, commit, and
formal admission into the local library directory.

Current status: MVP main-chain local manual acceptance is complete at the
code level. Release-grade acceptance still requires a clean Windows
`pnpm release:gate` run, including `pnpm release:install-gate`, before
release sign-off.

## Verification Commands

| Command                                    | Result | Notes                                                                                              |
| ------------------------------------------ | ------ | -------------------------------------------------------------------------------------------------- |
| `pnpm install`                             | pass   |                                                                                                    |
| `pnpm format:check`                        | pass   | prettier + cargo fmt clean                                                                         |
| `pnpm typecheck`                           | pass   | `tsc --noEmit`                                                                                     |
| `pnpm test:unit`                           | pass   | 12 frontend tests across 3 files                                                                   |
| `pnpm rust:test`                           | pass   | 196 lib tests pass (2 ignored real-db)                                                             |
| `pnpm rust:clippy`                         | pass   | `-D warnings` clean, all targets/features                                                          |
| `pnpm rust:test:real`                      | pass   | real-DB suite passed (incl. M9 main chain, diagnostics, recovery, fault injection, reconciliation) |
| `pnpm build`                               | pass   | release exe + NSIS installer built                                                                 |
| `pnpm release:verify-artifacts`            | pass   | release exe, NSIS installer, and packaged runtime presence verified                                |
| `pnpm release:install-gate`                | pass   | local installer gate: install / overwrite install / launch smoke / uninstall / data retention      |
| `pnpm release:gate -- --only=install-gate` | pass   | wrapper wiring confirmed for the new `install-gate` step                                           |

`pnpm release:gate` was not run end-to-end in this session because it
re-runs the full real suite + build (already verified individually above)
and the Windows loopback SMB gate requires PowerShell admin elevation in
this environment. The local install gate passed and is now part of the
release gate; a full clean Windows `pnpm release:gate` run remains the
release sign-off step.

## Release Artifact Check

- Windows exe: `apps/desktop/src-tauri/target/release/imagedb-desktop.exe` (~23 MB)
- Installer: `apps/desktop/src-tauri/target/release/bundle/nsis/ImageDB_0.1.0_x64-setup.exe` (~31 MB)
- postgres-runtime location: bundled via `tauri.conf.json` `resources` from
  `binaries/windows-x86_64/postgres-runtime/` → installed at
  `<resource_dir>/postgres-runtime/`
- pgvector files: `lib/vector.dll`, `share/extension/vector.control`,
  `share/extension/vector--0.8.3.sql`

## Known Limitations

- Cross-platform runtime packaging (Linux/macOS) is out of scope; this
  round guarantees Windows x64 only.
- The `pnpm release:gate` end-to-end run (including the loopback SMB gate)
  was not executed in this session for the reasons noted above; the
  clean-Windows full gate, including install-gate, remains the release
  sign-off step.
- The 24-hour stability run and the 1k/10k/100k performance campaigns
  from `tasks/09-release-closure.md` remain future hardening work; the
  MVP 120-image performance baseline is recorded in
  `reports/m9-performance-thresholds.json`.

## Local Commits

```text
9b925a0 docs: add M6.5-M9 closure plan
4ca87ab fix: prefer ready import runs for commit over old completed runs
7c5a60a feat: freeze import plans for public commit workflow
c84c475 fix: package managed postgres runtime as release resource
d9b3e5c fix: fail real tests when postgres runtime is missing
<docs commit adding this report>
```

## Push Status

Remote branch `origin/core_fix_m5_m6_refactor` exists. This report does not
record a created PR, release, or artifact upload.
