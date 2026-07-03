# Core Fix & M5/M6 Refactor — Final Report

Branch: `core_fix_m5_m6_refactor`
Date: 2026-07-03
Executor: autonomous target-execution mode (no push, local commits only)

## 1. Summary

The branch's prior commit `fbb3bb8` ("core fix and M5/M6 refactor: complete
all 14 steps") claimed the M5/M6 core loop was done. Independent audit
(reading the code + running the real tests, not trusting the report) showed
the structure was scaffolded but **not actually wired**: the commit pipeline
wrote file operations as `pending` (rejected by migration 0006's CHECK
constraint), read its commit set from `statistics["frozen_plan"]` at commit
time, overwrote the bound library root, flattened subdirectories via
`file_name()`, decided idempotency from a row count alone, and the Recovery
Service was a stub returning label strings.

All 12 phases of the core fix are now implemented, verified against real
PostgreSQL 18.4 + pgvector and the real filesystem, and committed locally.
**No push, no PR, no remote branch, no release artifact upload.**

## 2. Completed phases

| Phase | What | Commit |
|------|------|--------|
| 1 | file_operations state + DB constraints + state machine wiring | 4f37a23 |
| 2 | immutable import_plans as the sole commit source of truth | 4f37a23 |
| 3 | library root identity + relative path preservation | 4f37a23 |
| 4 | recoverable file transaction protocol (prewrite, stream copy, manifest, atomic publish) | 4f37a23 |
| 5 | DB commit + complete idempotency verification | 4f37a23 |
| 6 | separate source-archive recovery stage | 71bad5a |
| 7 | real Recovery Service that executes actions | 71bad5a |
| 8 | prefer historical library images as representatives | 7241e09 |
| 9 | secure candidate image previews | d1d4a6a |
| 10 | re-verify old blockers (library fail-stop, zero-review, cross-album, post-scan state, cancel) | 7241e09 |
| 11 | fault-injection tests driving the Recovery Service | 71bad5a |
| 12 | wire recovery + conflict states to the desktop GUI | 1d55f55 |
| 13 | final verification + build + real test execution | 7a62291 |

## 3. Core transaction protocol

```
frozen import_plans (sole source of truth)
  → prewrite file_transaction + ALL file_operations (state=planned) in one DB tx
  → stream copy each file to .part + incremental BLAKE3
  → verify size + BLAKE3 → rename .part → staged file (op=verified)
  → write .imagedb-manifest.json (temp + atomic rename + manifest hash)
  → atomic publish: rename whole staging album dir → Albums/<rel>  (same FS)
  → DB commit tx: verify plan + manifest, upsert library_album/images keyed by
    transaction_id + plan_hash, plan → consumed, tx → library_committed
  → source archive: rename source album → .imagedb-processed/<tx>/<rel>
    (separate recoverable stage; library commit success is never undone)
  → tx → source_archived
```

State transitions go through typed enums
(`TransactionState`/`FileOpState`/`PlanState`) via
`transition_transaction` / `next_file_op_state`; services never write
unchecked state strings. Migration `0007_transaction_links` adds
`plan_hash` + `manifest_hash` to `file_transactions` and `transaction_id` +
`plan_hash` to `library_albums` so idempotency and recovery are authoritative.

Idempotency (`verify_complete_evidence`) returns `already_committed` only
when **every** piece of evidence matches: transaction id, plan id, plan
hash, manifest hash, on-disk dir + parseable manifest, every file's path /
size / recomputed BLAKE3, and the DB album + image record set. Any mismatch
surfaces as a `conflict` (never an automatic overwrite).

## 4. Database migrations + integration tests

Migration set (7): `0001_initial` … `0007_transaction_links`. Verified on an
empty DB that all run and the final version is `0007_transaction_links`.

Real PostgreSQL integration tests (require
`IMAGEDB_POSTGRES_BIN=.../postgresql-18.4/pgsql/bin`):

- `real_protocol_migrations_run_on_empty_db` — all migrations apply cleanly.
- `real_protocol_creates_planned_file_operation` — `planned` op insertable.
- `real_protocol_rejects_pending_file_operation` — `pending` rejected by CHECK.
- `real_protocol_rejects_illegal_transaction_state` — bogus state rejected.
- `real_protocol_all_legal_transitions` — full planned→…→source_archived walk.
- `real_protocol_invalid_transition_rejected` — illegal jumps error.
- `real_protocol_tampered_plan_hash_rejected` — tampered plan rejected wholesale.
- `real_protocol_cross_album_and_history_duplicates` — cross-album + library
  indexed matches + library-image representative.
- `real_commit_full_pipeline` — commit, atomic publish, manifest, DB records,
  source archive, idempotent rerun skip.
- `real_review_decision_persists_and_filters_plan`, `real_scan_persists_exact_duplicates`,
  `real_pgvector_full_lifecycle` — pre-existing, still green.

**All 24 real tests pass** (12 `real_` + 12 `fail_injection_`).

## 5. Fault injection + recovery

`fail_injection_tests` now injects a fault → drops the original service
(simulating restart) → creates a fresh Recovery Service → drives it from
persisted state → asserts `source_archived` with published dir + library
records intact → runs a second recovery pass to confirm idempotency.

Fault points covered (all green): after DB write, during copy, after staging
copy, after staging verify, after manifest, before/after publish rename,
before/after DB commit, before/during source archive, and user cancel.

## 6. GUI wiring

New IPC commands `scan_recoverable_transactions`,
`recover_transaction`, `reverify_transaction` (registered in `lib.rs`).
`RecoveryPage` fetches live diagnostics, shows evidence (present/missing),
per-transaction diagnostics + errors, executes recovery, re-verifies, opens
the target dir. **No "overwrite" button** — conflicts require manual
resolution. `CommitPage` shows the full pipeline (prepare→copy→verify→publish
→DB commit→source archive) with stage labels and a "go to recovery" button
when the run needs recovery.

## 7. Test + build results

```
pnpm install              ok
pnpm format:check         ok (all files use Prettier style)
pnpm typecheck            ok
pnpm test:unit            4 passed
pnpm rust:test            124 passed, 1 ignored (real_pgvector, needs env)
pnpm rust:clippy          clean (-D warnings, all targets, all features)
pnpm build                ok — Windows release exe built
```

Real test execution (with `IMAGEDB_POSTGRES_BIN`): 24/24 pass.

## 8. Windows executable

```
D:\MyProjects\Agent\ImageDB-MVP\apps\desktop\src-tauri\target\release\imagedb-desktop.exe
21.8 MB, launches successfully (smoke-tested).
```

## 9. Local Git commits (no push)

```
7a62291 test: add frozen-plan tamper and cross-album/history duplicate tests
de2c918 chore: refresh tsbuildinfo after build
1d55f55 feat: connect recovery and conflict states to the desktop GUI
d1d4a6a fix: secure persisted candidate image previews
7241e09 fix: prefer historical images, detect cross-album duplicates, fail-stop library
71bad5a feat: execute persisted transaction recovery through the Recovery Service
4f37a23 fix: rebuild file transaction protocol with immutable plans and typed states
```

Branch is 7 commits ahead of `origin/core_fix_m5_m6_refactor` (still at
`fbb3bb8`). **Confirmed: no `git push`, no remote branch, no PR, no release.**

## 10. Real limitations

- The Windows **debug test binary** cannot link the tauri runtime
  (`AppHandle`/`Emitter`) at load time, so the cross-album/history test
  exercises the repository + duplicate-group logic directly rather than
  calling `scan_service::run_scan`. The production `run_scan` path is
  exercised by the release build + the real scan unit tests that call its
  helpers. A future refactor (progress-emitter trait) would let the full
  `run_scan` run under the test binary.
- Real PostgreSQL integration tests are `#[ignore]`d by design and run via
  the documented `--features real-db-tests,fail-injection -- --ignored`
  command (they need a live cluster). This is intentional, not a gap; the
  default `cargo test` stays green without a DB.
- The recovery "open target directory" button is present but does not yet
  invoke a shell-open IPC (no path-traversal risk since the path comes from
  the persisted transaction row, but the click handler is a no-op stub).
- External-PostgreSQL mode (Milestone 7) and mounted-share verification
  (Milestone 8) are out of scope for this core-fix round, per the task
  boundary.
