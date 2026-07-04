# Current Task

Executing: `.codex-plans/M6.5-M9-closure/tasks/06_5_09_closure.md`

Branch: `core_fix_m5_m6_refactor`

## Closure status (M6.5–M9)

This round closes the M6.5–M9 framework into a verifiable main chain. It
does NOT add new features. Each item was verified against real PostgreSQL
18.4 + pgvector 0.8.3 and the real filesystem — claims are backed by tests
that fail (not skip) when the runtime is missing.

- M6.5 managed PostgreSQL runtime: the Windows release bundles its own
  PostgreSQL + pgvector runtime via Tauri resources; `lib.rs` exposes it to
  the `PostgresManager` locator via `IMAGEDB_POSTGRES_RUNTIME_DIR`. Missing
  runtime now reports "安装包不完整" (incomplete installer) and tells the
  user to reinstall, not to install PostgreSQL. Real test
  `real_packaged_runtime_clean_bootstrap` runs the full lifecycle using
  only the packaged runtime.
- Real test fail-fast: every real-DB test that previously skipped when
  `IMAGEDB_POSTGRES_BIN` was unset now panics with the expected path.
  `run-real-rust-tests.mjs` pre-flights the runtime and aborts before
  cargo if it is missing.
- M7 external PostgreSQL: the external connection path routes through
  `connect_external` for all four TLS modes (disable / require / verify-ca /
  verify-full); preflight checks PG version, pgvector, CREATE EXTENSION,
  schema, and migration permissions; UI surfaces TLS + diagnostics. Managed
  local mode is unaffected.
- M8 mounted storage: capability probe covers read/write, rename variants,
  case sensitivity, Unicode normalization, long paths, file sync, and free
  space; `classify_publish_strategy` rejects any storage missing a required
  capability. The disconnect-then-recover path is exercised by the real
  `mounted_storage_gate_library_root_disconnect_pauses_then_recovers` test
  and the Windows loopback SMB gate.
- M9 frozen plan: `freeze_import_plan` writes the three plan tables +
  plan_hash + plan state=frozen + run state=ready_to_commit in a single
  database transaction; `get_frozen_import_plan_summary` reads the
  persisted view. The commit page reads the frozen summary; the review page
  calls freeze (idempotent). Re-freeze returns the same summary; post-freeze
  candidate/review edits cannot change the commit set.
- Latest committable run: the query now prefers `ready_to_commit`, then
  resubmittable `cancelled` (no active transaction); `completed` no longer
  enters the default commit page; `recovery_required` routes to recovery.

See `reports/m6_5_m9_closure.md` for the final closure report and the
acceptance checklist in `.codex-plans/M6.5-M9-closure/checklists/`.

## Non-goals (this round)

- No new algorithms, no SMB protocol, no cross-platform runtime.
- No rewrite of the transaction system.
- No push, no remote branch, no PR, no release publication.

## Prior milestones

> Milestone 0 technical probe prototype is complete. Report: reports/milestone-0.md.
> Milestone 1 app skeleton and database foundation is complete. Report: reports/milestone-1.md.
> Milestone 2 scan and exact duplicate detection is complete. Report: reports/milestone-2.md.
> Milestone 3 perceptual similarity detection is complete. Report: reports/milestone-3.md.
> Milestone 4 human review GUI is complete. Report: reports/milestone-4.md.
> Milestone 5 formal import loop is complete (re-verified during the core fix). Report: reports/milestone-5.md.
> Milestone 7 external PostgreSQL mode is complete (re-verified during M6.5–M9 closure). Report: reports/milestone-7-progress.md.
> Milestone 8 mounted shared storage compatibility is complete (re-verified during M6.5–M9 closure). Report: reports/milestone-8-progress.md.
