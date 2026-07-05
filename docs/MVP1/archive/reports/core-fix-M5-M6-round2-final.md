# Core Fix & M5/M6 Refactor — Round 2 Final Report

Branch: `core_fix_m5_m6_refactor`
Date: 2026-07-04
Executor: autonomous target-execution mode (no push, local commits only)

## 1. Summary

Round 1 of the core fix landed the immutable-plan commit pipeline, real
Recovery Service, and GUI wiring. Round 2 closed the six remaining
correctness gaps left by round 1 — the ones the round-1 report claimed
were handled but were not (verified by reading code + running real
PostgreSQL tests, not trusting the report):

- cancellation manufactured unrecoverable `failed` transactions
- API/progress/GUI returned `completed_with_errors` /
  `cancelled_pending_recovery` overlays that diverged from the DB
- an empty frozen plan bypassed transaction checks and completed directly
- archive root was derived from `source_album_dir.parent().unwrap_or(Path::new("."))`
- `capture_source_album_snapshot` blocked the async runtime on large albums
- `reconcile_import_run_state` returned a transient `now()` as `completed_at`

All six phases are implemented, verified against real PostgreSQL 18.4 +
pgvector and the real filesystem, and committed locally. **No push, no
PR, no remote branch, no release artifact upload.**

## 2. Completed phases

| Phase | What                                                                                           | Commit     |
| ----- | ---------------------------------------------------------------------------------------------- | ---------- |
| 1     | cancel preserves recoverable transaction states; failed/cancelled → terminal-but-not-recovered | b7c98c6    |
| 2     | persisted DB state is the sole source of truth (no overlays)                                   | b7c98c6    |
| 3     | empty plan validates transaction invariants before completing                                  | b7c98c6    |
| 4     | archive root from persisted `import_runs.source_root`; path identity chain                     | b7c98c6    |
| 5     | snapshot hashing in `spawn_blocking`; special files rejected                                   | 953b65b    |
| 6     | `completed_at` always the persisted DB value (read back)                                       | b7c98c6    |
| 7     | 15 new real failure tests + pgvector assertion fix                                             | ee34f57    |
| 8     | final adversarial review + full build + real test execution                                    | (verified) |

## 3. Cancellation + recovery semantics

```
cancel before any file transaction is prewritten
  → no transaction row, run → recovery_required (never silently completed)

cancel mid-flight (transaction already prewritten)
  → transaction stays at its last recoverable state (staging/verified/published/...)
  → run → recovery_required
  → Recovery can resume the original transaction after a restart

Recovery encounters failed / cancelled
  → recovered = false, terminal = true
  → run stays recovery_required (never auto-completed)

Recovery encounters source_archived
  → recovered = true, terminal = true (genuine "already done")

Recovery never creates a second active transaction for an album
  → mid-flight albums surface ResumeRequired carrying the original tx_id
```

## 4. Empty-plan closure rules

```
empty plan + (no transactions, valid plan hash, no residual rows)
  → completed

empty plan + active transaction (planned/staging/.../cleanup_required)
  → recovery_required

empty plan + conflict
  → recovery_required

empty plan + failed or cancelled
  → recovery_required

empty plan + tampered plan hash / residual plan rows
  → plan integrity error → recovery_required
```

Commit and `reconcile_import_run_state` share the same rule (the
reconciler is the single authoritative decider).

## 5. Source-path identity validation

```
1. read persisted import_runs.source_root
2. read import_albums.source_path
3. read source_album_snapshots.source_album_path
4. canonicalize existing paths (resolves symlinks + Windows case)
5. snapshot path == album path (canonical or lexical)
6. album path inside source_root (canonical or lexical)
7. reject path escape / relative paths / missing parent / `..` traversal
8. archive root = <source_root>/.imagedb-processed/<tx-id>/<album-rel-path>
```

No `unwrap_or(Path::new("."))` fallback remains anywhere in services/.

## 6. Special-file handling rules

```
symlink (file or directory) → rejected (symlink_metadata + is_symlink)
Windows directory junction / reparse point → rejected (file_attributes & 0x400)
FIFO / socket / char/block device / unknown → rejected
regular file → hashed
directory → walked (recursively)
```

Never silently hashed or skipped. The album is rejected with an explicit
`AppError` naming the entry kind.

## 7. New tests + results

`tests/cancellation_recovery_integration.rs` — 15 real PostgreSQL +
filesystem tests driving the real Service layer (Commit Service, Recovery
Service, Repository). All 15 pass.

| #   | Scenario                                                   | Result |
| --- | ---------------------------------------------------------- | ------ |
| 1   | cancel during copy leaves a recoverable transaction        | ✓      |
| 2   | cancel before prewrite — no transaction, recovery_required | ✓      |
| 3   | recovery continues the original transaction after restart  | ✓      |
| 4   | re-running commit does not create a second transaction     | ✓      |
| 5   | failed/cancelled not reported recovered=true               | ✓      |
| 6   | recovery_required not mapped to a completion overlay       | ✓      |
| 7   | empty plan + active transaction → recovery_required        | ✓      |
| 8   | empty plan + conflict → recovery_required                  | ✓      |
| 9   | empty plan + no transactions → completed                   | ✓      |
| 10  | snapshot path mismatch → conflict                          | ✓      |
| 11  | happy-path commit still completes                          | ✓      |
| 12  | source album with symlink rejected                         | ✓      |
| 13  | repeated reconcile returns the same persisted completed_at | ✓      |
| 14  | two consecutive recovery passes converge (idempotent)      | ✓      |
| 15  | conflict does not delete source files                      | ✓      |

Also fixed a stale migration-version assertion in
`real_pgvector_full_lifecycle` (was asserting `0007_transaction_links`
after migration `0008_source_album_snapshots` landed; both check sites
now correctly assert `0008_source_album_snapshots`).

## 8. Full build results

```
pnpm install              ok
pnpm format:check         ok (all files use Prettier style; .prettierignore
                           excludes generated Tauri schemas + build artifacts)
pnpm typecheck            ok
pnpm test:unit            5 passed (frontend vitest)
pnpm rust:clippy          clean (-D warnings, all targets, all features)
pnpm rust:test            165 passed, 1 ignored (real_pgvector, needs env —
                           now runs in rust:test:real and passes)
pnpm rust:test:real       64/64 real-db tests pass across 10 suites:
                             managed PostgreSQL lifecycle: 1
                             scan persistence: 2
                             source snapshot verification: 3
                             review persistence: 1
                             file transaction protocol: 9
                             formal commit pipeline: 1
                             strict manifest validation: 9
                             run-state reconciliation: 7
                             fault injection recovery: 16
                             cancellation + final recovery invariants: 15
pnpm build                ok — Windows release exe built
```

## 9. Windows executable

```
D:\MyProjects\Agent\ImageDB-MVP\apps\desktop\src-tauri\target\release\imagedb-desktop.exe
22.5 MB, launches successfully (smoke-tested).
```

## 10. Local Git commits (no push)

```
ee34f57 test: cover cancellation and final recovery invariants
953b65b fix: isolate snapshot hashing and reject unsupported filesystem entries
b7c98c6 fix: preserve recoverable transaction states during cancellation
```

Branch is 3 commits ahead of `origin/core_fix_m5_m6_refactor`.
**Confirmed: no `git push`, no remote branch, no PR, no release.**

## 11. Remaining real limitations

- The Windows **debug test binary** cannot link the tauri runtime
  (`AppHandle`/`Emitter`) at load time, so no test calls
  `scan_service::run_scan` directly; the cancellation/recovery tests
  drive the real Commit/Recovery Service against real PostgreSQL +
  filesystem instead. The production `run_scan` path is exercised by the
  release build + the real scan unit tests that call its helpers.
- Real PostgreSQL integration tests are `#[ignore]`d by design and run via
  the documented `--features real-db-tests,fail-injection -- --ignored`
  command (they need a live cluster). The default `cargo test` stays green
  without a DB.
- The recovery "open target directory" button is present but does not yet
  invoke a shell-open IPC (no path-traversal risk since the path comes from
  the persisted transaction row, but the click handler is a no-op stub).
- External-PostgreSQL mode (Milestone 7) and mounted-share verification
  (Milestone 8) are out of scope for this core-fix round, per the task
  boundary.
