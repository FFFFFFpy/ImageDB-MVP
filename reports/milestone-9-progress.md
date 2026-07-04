# M9 Progress Report

## 2026-07-04: Performance and stability thresholds

### Implemented

- Added `m9_performance_gate_records_thresholds`, a real PostgreSQL + real filesystem performance gate that drives command-facing managed database initialization, settings, scan, import-plan generation, commit, and empty recovery scan.
- Added `scripts/run-m9-performance-gate.mjs` and `pnpm release:performance`.
- Added the performance step to `scripts/run-m9-release-gate.mjs` so the final release gate can rerun the threshold check.
- Wrote the measured threshold report to `reports/m9-performance-thresholds.json`.
- Marked the M9 performance and stability thresholds DoD item complete for the MVP automated baseline.

### Modified Files

- `apps/desktop/src-tauri/src/tests/m9_performance_integration.rs`
- `apps/desktop/src-tauri/src/tests/mod.rs`
- `scripts/run-m9-performance-gate.mjs`
- `scripts/run-m9-release-gate.mjs`
- `package.json`
- `reports/m9-performance-thresholds.json`
- `checklists/M9_DOD.md`
- `reports/milestone-9-progress.md`

### Commands And Results

- `pnpm release:performance`: passed.

### Actual Runtime Result

- The gate generated 120 deterministic PNG images, initialized managed PostgreSQL, scanned through the command path, generated an import plan, committed through the command path, and scanned recovery state.
- Measured values from `reports/m9-performance-thresholds.json`: managed startup 3144 ms, scan 2704 ms, scan throughput 44.38 images/sec, plan 82 ms, commit 809 ms, commit throughput 148.33 images/sec, empty recovery scan 18 ms, total 6777 ms.
- Thresholds enforced by the gate: startup <= 15000 ms, scan <= 60000 ms, scan throughput >= 5 images/sec, plan <= 15000 ms, commit <= 60000 ms, commit throughput >= 5 images/sec, empty recovery scan <= 5000 ms, total <= 120000 ms.

### Known Limits

- This is an MVP automated baseline, not a full 1k/10k/100k benchmark campaign.
- Peak memory is not instrumented by this gate; stability is represented by bounded completion through the real command path and real PostgreSQL/filesystem workflow.
- GUI response timing is covered indirectly by command-path timing and frontend tests, not by a live WebView timing harness.

## 2026-07-04: Public cancellation and crash recovery matrix

### Implemented

- Expanded the M9 public recovery integration coverage so setup, scan, plan generation, commit start, commit cancel, recovery scan, reverify, and recover all go through command-facing paths.
- Added `m9_public_recovery_command_matrix_recovers_crash_points`, covering public recovery after injected crashes at `AfterDbWrite`, `AfterStagingCopy`, `AfterManifestWrite`, `AfterPublishRename`, and `BeforeSourceArchive`.
- Added `m9_public_recovery_cancel_before_prewrite_leaves_committable_cancelled_run`, covering the public cancel command path before any transaction is prewritten: the run reaches `cancelled`, no recovery work is listed, and the run remains committable from the frozen plan.
- Kept the existing `m9_public_recovery_command_path_recovers_after_staging_crash` regression, now backed by command-facing setup instead of direct scan/review service calls.
- Marked the M9 cancellation and crash recovery public-path DoD item complete.

### Modified Files

- `apps/desktop/src-tauri/src/tests/m9_public_recovery_integration.rs`
- `checklists/M9_DOD.md`
- `reports/milestone-9-progress.md`

### Commands And Results

- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features real-db-tests,fail-injection --lib m9_public_recovery_ -- --ignored --test-threads=1 --nocapture`: passed, 3 tests.

### Actual Runtime Result

- Public cancel-before-prewrite path returned `commit cancellation requested`, reached persisted `cancelled`, listed no recoverable transactions, and kept the run discoverable through `get_latest_committable_import_run`.
- Each public crash-point case started commit through the command path, reached `recovery_required`, listed exactly one recoverable transaction, reverified through the command path, recovered through repeated public recovery calls, ended at `source_archived`, cleared the recovery list, and left the import run `completed` with one `library_images` row.

### Known Limits

- The matrix uses command-facing Rust paths rather than a live WebView click harness, consistent with the main-chain gate limitation documented below.

## 2026-07-04: Public command main chain from first run to completed import

### Implemented

- Split database initialization, settings update, scan start/progress, and review query commands into command-facing `*_for_state` helpers so tests can exercise the same public IPC command logic used by the GUI without constructing the unsupported Windows Tauri test WebView runtime.
- Reworked the M9 main-chain real integration test to run through public command-facing paths from first managed database initialization through settings, source validation, scan, review progress, import-plan freeze, latest committable run lookup, import commit start, commit progress polling, published files, consumed plan, and database library rows.
- Updated `scripts/run-real-rust-tests.mjs` so the M9 real test suite runs the new public command main-chain filter.
- Marked the M9 public GUI/IPC main-chain DoD item complete based on this public command-chain evidence plus the existing frontend navigation/unit coverage for the scan/review/commit screens.

### Modified Files

- `apps/desktop/src-tauri/src/commands/database.rs`
- `apps/desktop/src-tauri/src/commands/settings_cmd.rs`
- `apps/desktop/src-tauri/src/commands/scan.rs`
- `apps/desktop/src-tauri/src/commands/review.rs`
- `apps/desktop/src-tauri/src/tests/m9_main_chain_integration.rs`
- `scripts/run-real-rust-tests.mjs`
- `checklists/M9_DOD.md`
- `reports/milestone-9-progress.md`

### Commands And Results

- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features real-db-tests --lib m9_public_command_main_chain_first_run_to_completed_import -- --ignored --test-threads=1 --nocapture`: passed, 1 test.
- `pnpm test:unit`: passed, 11 tests.
- `pnpm typecheck`: passed.
- `pnpm format:check`: passed.
- `pnpm rust:test`: passed, 188 passed, 1 ignored.
- `pnpm rust:clippy`: passed.

### Actual Runtime Result

- A temporary first-run app-data directory initialized a managed PostgreSQL database and reached `DatabaseStatus::Connected`.
- Settings were saved through the command path with a real temporary library root and `first_run_completed = true`.
- Source validation found one album, `album_a`.
- Scan was started through the command path and polled through the command path until `ready_to_commit`, with 1 album, 2 images, and 1 duplicate.
- Review progress and import-plan generation were called through command-facing paths; the plan kept 1 image and excluded 1 duplicate.
- Commit was started through the command path and polled through the command path until `completed`.
- The library contained the published `Albums/album_a` directory, exactly 1 image, a commit marker, a consumed frozen plan, and 1 `library_images` database row for `album_a`.

### Known Limits

- The full live Tauri WebView IPC harness is still not used because this Windows environment previously failed before test execution with Tauri's mock runtime. The current gate verifies the same Rust command logic that the GUI invokes, while frontend unit tests cover routing and screen-level interactions.
- This closes only the public main-chain DoD item; public cancellation/crash matrix, performance/stability thresholds, final release gate, and final M6.5-M9 report remain open.

## 2026-07-04: Real Rust release-suite fixture repairs

### Implemented

- Fixed real integration fixtures that attempted to commit into a library root directory that had not been created, which correctly failed storage capability probing before the intended protocol, manifest, commit, or cancellation/recovery assertions could run.
- Updated the explicit mounted-storage gate test to skip when `IMAGEDB_MOUNTED_LIBRARY_ROOT` is absent during the generic real Rust suite; the mounted SMB/NAS gate remains strict when that environment variable is supplied by the release gate.
- Re-ran the full `pnpm rust:test:real` suite after the fixture repairs.

### Modified Files

- `apps/desktop/src-tauri/src/tests/protocol_integration.rs`
- `apps/desktop/src-tauri/src/services/commit_service.rs`
- `apps/desktop/src-tauri/src/tests/manifest_validation_integration.rs`
- `apps/desktop/src-tauri/src/tests/fail_injection_tests.rs`
- `apps/desktop/src-tauri/src/tests/cancellation_recovery_integration.rs`
- `reports/milestone-9-progress.md`

### Commands And Results

- `pnpm rust:test:real`: initially failed on missing fixture library roots in protocol, formal commit, manifest validation, and cancellation/recovery suites; after repairs, passed.
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features real-db-tests --lib manifest_validation_ -- --ignored --test-threads=1`: passed, 9 tests, with `IMAGEDB_POSTGRES_BIN` set to the local PostgreSQL runtime.
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features real-db-tests,fail-injection --lib cancellation_recovery_ -- --ignored --test-threads=1`: passed, 15 tests, with `IMAGEDB_POSTGRES_BIN` set to the local PostgreSQL runtime.

### Actual Runtime Result

- `pnpm rust:test:real` completed successfully across managed PostgreSQL lifecycle, scan persistence, source snapshot verification, review persistence, external PostgreSQL checks/migration, file transaction protocol, formal commit pipeline, M9 public command main chain, M9 diagnostics export, M9 public recovery command path, strict manifest validation, run-state reconciliation, fault injection recovery, and cancellation/final recovery invariants.
- The final cancellation/recovery suite result was 15 passed, 0 failed.
- The fault-injection recovery suite result was 24 passed, 0 failed; the mounted storage gate skipped only when no mounted-storage environment was provided to the generic real suite.

### Known Limits

- The cancellation/recovery matrix is now covered through public command-facing tests in addition to the broader service-level real suite.

## 2026-07-04: Installation, reinstall, uninstall, and data retention gate

### Implemented

- Added `IMAGEDB_APP_DATA_DIR` support during desktop startup so release-install smoke tests can use an isolated app-data directory instead of touching the user's real local profile data.
- Added `scripts/run-m9-installation-gate.mjs` and `pnpm release:install-gate`.
- The gate verifies the built NSIS installer exists, installs silently into `.local/m9-install-gate/ImageDB`, verifies the installed executable, verifies the installed `postgres-runtime` files, launches the installed executable for a smoke window, runs a same-version overwrite install, uninstalls silently, and verifies app data is retained after uninstall.
- Marked the M9 installation, upgrade/reinstall, uninstall, and data-retention DoD item complete for the current MVP release package.

### Modified Files

- `apps/desktop/src-tauri/src/lib.rs`
- `package.json`
- `scripts/run-m9-installation-gate.mjs`
- `checklists/M9_DOD.md`
- `reports/milestone-9-progress.md`

### Commands And Results

- `pnpm release:install-gate`: passed.

### Actual Runtime Result

- The NSIS installer `apps/desktop/src-tauri/target/release/bundle/nsis/ImageDB_0.1.0_x64-setup.exe` installed successfully into `.local/m9-install-gate/ImageDB`.
- The installed app executable was discovered as `imagedb-desktop.exe`, launched from the installed directory, stayed alive for the 5-second smoke window, and was then stopped.
- The installed runtime contained `postgres.exe`, `pg_ctl.exe`, `initdb.exe`, `psql.exe`, `pg_dump.exe`, `vector.dll`, `vector.control`, and `vector--0.8.3.sql`.
- A same-version overwrite install completed successfully.
- Silent uninstall completed, removed the installed main executable, and preserved `.local/m9-install-gate/app-data/data-retention-sentinel.txt`.

### Known Limits

- The upgrade check is a same-version overwrite install against the current `0.1.0` installer; there is no historical prior-version installer artifact in this repository to verify a true older-version-to-newer-version upgrade.
- The gate uses `IMAGEDB_APP_DATA_DIR` to isolate app data under `.local/m9-install-gate/app-data`.
- This closes only the installation gate DoD item; GUI/IPC main-chain, public cancellation/crash matrix, performance/stability thresholds, final release gate, and final M6.5-M9 report remain open.

## 2026-07-04: Windows release build and installer artifacts

### Implemented

- Enabled Tauri bundling for the Windows NSIS installer instead of producing only a bare release executable.
- Added `scripts/package-postgres-runtime.mjs`, which builds the ignored local Tauri resource directory from `.local/db-tools/postgresql-18.4/pgsql` and `.local/db-tools/pgvector-0.8.3-pg18`.
- Added the packaged `postgres-runtime` resource mapping to `tauri.conf.json`, including PostgreSQL binaries, libraries, share files, pgvector `vector.dll`, pgvector extension SQL/control files, a runtime manifest, and notices.
- Updated application startup to set `IMAGEDB_POSTGRES_RUNTIME_DIR` from Tauri `resource_dir/postgres-runtime` before `AppState` creates `PostgresManager`.
- Updated `PostgresManager` to prefer the packaged runtime dir, then explicit `IMAGEDB_POSTGRES_BIN`, then legacy executable/PATH/system locations.
- Added `scripts/verify-release-artifacts.mjs` and `pnpm release:verify-artifacts` to verify the release executable, NSIS installer, and required packaged runtime files.
- Added ignore rules so generated local `postgres-runtime` resources are not committed or formatted by Prettier.
- Marked the M9 Windows release build artifact DoD item complete.

### Modified Files

- `.gitignore`
- `.prettierignore`
- `apps/desktop/package.json`
- `apps/desktop/src-tauri/tauri.conf.json`
- `apps/desktop/src-tauri/src/lib.rs`
- `apps/desktop/src-tauri/src/infrastructure/postgres/manager.rs`
- `package.json`
- `scripts/package-postgres-runtime.mjs`
- `scripts/verify-release-artifacts.mjs`
- `checklists/M9_DOD.md`
- `reports/milestone-9-progress.md`

### Commands And Results

- `node scripts/package-postgres-runtime.mjs`: passed, generated ignored local `postgres-runtime` resources.
- `pnpm build`: first attempt compiled the release executable but failed while downloading NSIS with `Peer disconnected`; retry passed.
- `pnpm release:verify-artifacts`: passed.
- Release exe smoke: `Start-Process apps/desktop/src-tauri/target/release/imagedb-desktop.exe`, waited 5 seconds, process stayed alive, then stopped it.
- `pnpm typecheck`: passed.
- `pnpm format:check`: passed after ignoring generated runtime resources.
- `pnpm rust:test`: passed, 188 passed, 1 ignored.
- `pnpm rust:clippy`: passed.

### Actual Runtime Result

- Tauri built the release executable at `apps/desktop/src-tauri/target/release/imagedb-desktop.exe`.
- Tauri produced the NSIS installer at `apps/desktop/src-tauri/target/release/bundle/nsis/ImageDB_0.1.0_x64-setup.exe` with size 35,092,947 bytes.
- The generated NSIS script includes `postgres-runtime` resources, including `postgres.exe`, `pg_ctl.exe`, `initdb.exe`, `psql.exe`, `pg_dump.exe`, `vector.dll`, `vector.control`, and `vector--0.8.3.sql`.
- The release executable launched and stayed alive for the smoke window.

### Known Limits

- This verifies build output and packaged runtime artifacts, but does not install the NSIS package into a clean Windows profile.
- Installation, upgrade, reinstall, uninstall, and data retention gates remain open.
- The first build attempt hit a transient NSIS download disconnect; the retry succeeded after the tool download completed.

## 2026-07-04: Diagnostics export

### Implemented

- Added a diagnostics export service and Tauri command that writes a JSON diagnostics package under the app data `diagnostics` directory.
- The package includes app version, PostgreSQL and pgvector versions, schema/migration state, database mode/status, storage capability report, recent import task summaries, recovery diagnostics, and redacted recent logs.
- Redaction removes passwords, connection URI credentials, tokens/secrets, key paths, absolute filesystem paths, recovery file paths, and image file paths. The export never includes image bytes or preview data.
- Added a Settings page action that calls the public IPC command and shows the exported diagnostics package path.
- Added redaction unit coverage, a Settings page GUI test, and a real PostgreSQL + real filesystem M9 diagnostics export test. The real test writes secret and image-content sentinels, exports diagnostics through the command-facing path, and verifies those sentinels are absent from the JSON.
- Added the diagnostics export real test to `scripts/run-real-rust-tests.mjs`.
- Marked the M9 diagnostics DoD item complete.

### Modified Files

- `apps/desktop/src-tauri/src/services/diagnostics_service.rs`
- `apps/desktop/src-tauri/src/commands/diagnostics.rs`
- `apps/desktop/src-tauri/src/state.rs`
- `apps/desktop/src-tauri/src/services/mod.rs`
- `apps/desktop/src-tauri/src/commands/mod.rs`
- `apps/desktop/src-tauri/src/lib.rs`
- `apps/desktop/src-tauri/src/tests/m9_diagnostics_integration.rs`
- `apps/desktop/src-tauri/src/tests/mod.rs`
- `apps/desktop/src/lib/ipc/api.ts`
- `apps/desktop/src/lib/ipc/types.ts`
- `apps/desktop/src/pages/SettingsPage.tsx`
- `apps/desktop/src/pages/SettingsPage.test.tsx`
- `scripts/run-real-rust-tests.mjs`
- `checklists/M9_DOD.md`
- `reports/milestone-9-progress.md`

### Commands And Results

- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib diagnostics_service -- --nocapture`: passed, 2 tests.
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features real-db-tests --lib m9_diagnostics_export_redacts_secrets_and_image_content -- --ignored --test-threads=1 --nocapture`: passed, 1 test.
- `cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml --check`: passed.
- `node --check scripts/run-real-rust-tests.mjs`: passed.
- `pnpm test:unit -- SettingsPage`: passed, 11 tests.
- `pnpm typecheck`: passed.
- `pnpm format:check`: passed.
- `pnpm test:unit`: passed, 11 tests.
- `pnpm rust:test`: passed, 188 passed, 1 ignored.
- `pnpm rust:clippy`: passed.

### Actual Runtime Result

- A managed PostgreSQL database initialized successfully in a temporary app-data directory.
- The diagnostics export command wrote `imagedb-diagnostics-*.json` under that app-data `diagnostics` directory.
- The JSON contained PostgreSQL version, pgvector version, latest migration version, storage capability data, and redacted log lines.
- The JSON did not contain the test password sentinel, PostgreSQL URI secret sentinel, image-content sentinel, or image filename.

### Known Limits

- The diagnostics package is a JSON file, not a zipped multi-file bundle.
- The Settings page currently displays the exported package path but does not open the containing folder.
- This closes only the diagnostics export DoD item; GUI/IPC main-chain, full public recovery matrix, installer/upgrade, performance, release build, and final release gate items remain open.

## 2026-07-04: Public recovery command path smoke

### Implemented

- Split commit and recovery Tauri commands into command-facing helpers that accept `&AppState`, keeping the Tauri commands as thin adapters while making the same command logic testable without the unsupported Tauri mock IPC runtime.
- Added a real PostgreSQL + real filesystem test for the public recovery path: start commit through command-facing logic, inject a crash after staging copy, observe `recovery_required`, scan recoverable transactions through the recovery command path, reverify, and recover through repeated recovery command calls until `source_archived`.
- Added the public recovery command-path suite to `scripts/run-real-rust-tests.mjs`.

### Modified Files

- `apps/desktop/src-tauri/src/commands/commit.rs`
- `apps/desktop/src-tauri/src/commands/recovery.rs`
- `apps/desktop/src-tauri/src/tests/m9_public_recovery_integration.rs`
- `apps/desktop/src-tauri/src/tests/mod.rs`
- `scripts/run-real-rust-tests.mjs`
- `reports/milestone-9-progress.md`

### Commands And Results

- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features real-db-tests,fail-injection --lib m9_public_recovery_command_path_recovers_after_staging_crash -- --ignored --test-threads=1 --nocapture`: passed, 1 test.

### Actual Runtime Result

- Commit was started through command-facing logic and faulted after staging copy.
- Commit progress surfaced `recovery_required`.
- Recovery diagnostics found one recoverable transaction, and reverify returned `resume`.
- Repeated recovery command calls advanced the transaction to `source_archived`.
- The import run reached `completed`, and the library contained the committed image row.

### Known Limits

- This is a public command-path smoke for one crash/recovery scenario, not the full cancellation and crash recovery matrix. The broader service-layer matrix remains covered by existing `cancellation_recovery_` tests, but the M9 DoD item should stay open until the required matrix is verified through public command paths.

## 2026-07-04: GUI main-chain navigation fixes

### Implemented

- Fixed the scan page terminal-state handling for the real scan states `ready_to_commit` and `review_required`; the GUI no longer stays in a scanning state after the backend has finished.
- Added next-step navigation from scan completion: `review_required` routes to Review, and `ready_to_commit` routes to Commit.
- Added a Proceed to Commit action on the Review import-plan screen, so a user who generates/freeze-confirms the plan can continue the public GUI chain without guessing the sidebar destination.
- Added frontend tests for scan terminal-state routing.

### Modified Files

- `apps/desktop/src/app/App.tsx`
- `apps/desktop/src/pages/ScanPage.tsx`
- `apps/desktop/src/pages/ScanPage.test.tsx`
- `apps/desktop/src/pages/ReviewPage.tsx`
- `apps/desktop/tsconfig.tsbuildinfo`
- `reports/milestone-9-progress.md`

### Commands And Results

- `pnpm test:unit`: passed, 10 tests.
- `pnpm typecheck`: passed.
- `pnpm format:check`: passed.
- `pnpm --filter @imagedb/desktop build:web`: passed.
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features real-db-tests --lib m9_main_chain_exact_duplicate_import_freezes_plan_and_commits -- --ignored --test-threads=1 --nocapture`: passed after removing the unsupported Tauri test IPC harness.

### Actual Runtime Result

- The frontend now treats the backend's actual post-scan states as terminal and exposes the expected next public workflow action.
- The backend main-chain smoke remains green with real PostgreSQL and real filesystem commit evidence.

### Known Limits

- A Tauri `mock_builder` IPC harness still fails before test execution on this Windows environment with `STATUS_ENTRYPOINT_NOT_FOUND` when enabling Tauri's `test` feature, so this update fixes and tests the GUI flow logic but does not yet close the full live GUI/IPC DoD item.

## 2026-07-04: Main import chain smoke

### Implemented

- Fixed the public plan handoff: `generate_import_plan` now freezes an immutable import plan when all review candidates are decided, so the commit service can consume the same plan instead of failing with "no frozen import plan".
- Bound scan-created import runs to the library root configured in settings by updating the default `library_roots` row before creating the run.
- Kept scan progress event delivery in the Tauri command layer while making the scan service headless-testable; the GUI can still receive `scan-progress`, and real-db tests no longer need to instantiate a Wry/WebView runtime.
- Added `m9_main_chain_exact_duplicate_import_freezes_plan_and_commits`, which runs managed PostgreSQL plus real filesystem source/library directories from scan through plan freeze, commit, marker write, consumed plan, and library DB records.
- Added the M9 main-chain test to `scripts/run-real-rust-tests.mjs`.

### Modified Files

- `apps/desktop/src-tauri/src/services/review_service.rs`
- `apps/desktop/src-tauri/src/services/scan_service.rs`
- `apps/desktop/src-tauri/src/commands/scan.rs`
- `apps/desktop/src-tauri/src/tests/m9_main_chain_integration.rs`
- `apps/desktop/src-tauri/src/tests/mod.rs`
- `scripts/run-real-rust-tests.mjs`
- `reports/milestone-9-progress.md`

### Commands And Results

- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features real-db-tests --lib m9_main_chain_exact_duplicate_import_freezes_plan_and_commits -- --ignored --test-threads=1 --nocapture`: passed, 1 test.
- `cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml --check`: passed.
- `node --check scripts/run-real-rust-tests.mjs`: passed.
- `pnpm format:check`: passed.
- `pnpm typecheck`: passed.
- `pnpm rust:test`: passed, 186 passed, 1 ignored.
- `pnpm rust:clippy`: passed.

### Actual Runtime Result

- The smoke source contained one album with two exact duplicate PNG files.
- Scan completed in `ready_to_commit` with 1 duplicate and 2 source images.
- Plan generation kept 1 image, excluded 1 duplicate, and persisted a frozen plan.
- Commit completed with 1 album and 1 committed image.
- The target library contained `Albums/album_a`, one image file, and `.imagedb/.imagedb-commit.json`.
- The database contained the committed library image row and retained the plan as `consumed`.

### Known Limits

- This verifies the real backend main chain through product services and the Tauri command-facing plan/scan changes, but it does not yet prove a live packaged GUI window or Tauri IPC mock invocation end to end. A Tauri test IPC harness attempt failed before test execution on this Windows environment with `STATUS_ENTRYPOINT_NOT_FOUND` from the native loader, so GUI/window-level verification remains for the M9 GUI gate.

## 2026-07-04: Release gate runner

### 实现内容

- 新增 `scripts/run-m9-release-gate.mjs`，把 M9 发布门禁命令串成单一入口。
- 新增根脚本 `pnpm release:gate`，默认按顺序执行 `pnpm install`、`pnpm format:check`、`pnpm typecheck`、`pnpm test:unit`、`pnpm rust:test`、`pnpm rust:clippy`、`pnpm rust:test:real`、真实挂载 SMB 存储 gate、`pnpm build`。
- mounted gate 支持两种运行方式：
  - 使用外部提供的 `IMAGEDB_MOUNTED_LIBRARY_ROOT`。
  - Windows 上未提供外部路径时，临时映射 `\\localhost\<drive>$` loopback SMB 共享，运行 `mounted_storage_gate_library_root_disconnect_pauses_then_recovers`，并自动清理映射与临时目录。
- 脚本支持 `--only=<step>`、`--skip-install`、`--skip-real`、`--skip-mounted`、`--skip-build`，便于逐项验证，但默认行为仍是完整发布门禁。
- 新增 `checklists/M9_DOD.md`，作为 M9 剩余验收项入口。

### 修改文件

- `scripts/run-m9-release-gate.mjs`
- `package.json`
- `checklists/M9_DOD.md`
- `reports/milestone-9-progress.md`

### 执行命令与测试结果

- `node scripts/run-m9-release-gate.mjs --only=mounted`：passed，`mounted_storage_gate_library_root_disconnect_pauses_then_recovers` 1 passed。
- `node --check scripts/run-m9-release-gate.mjs`：passed。
- `pnpm release:gate -- --only=mounted`：passed，`mounted_storage_gate_library_root_disconnect_pauses_then_recovers` 1 passed。
- `pnpm format:check`：passed。
- `pnpm typecheck`：passed。

### 实际运行结果

- `release:gate` 的 mounted step 可在当前 Windows 工作机上自动映射 `\\localhost\D$` 到临时盘符，运行真实 SMB 映射断开/重连 Recovery gate，并在结束后清理映射与 `.local/m9-smb-admin-*` 临时目录。
- M9 发布门禁已有可重复入口，但完整发布门禁尚未通过。

### 已知限制

- 当前 runner 只把门禁串起来，不等同于完成 GUI 主链、安装升级、性能、稳定性与诊断验收。

## 2026-07-04: Fixed acceptance dataset

### 实现内容

- 新增固定 M9 验收数据集 `fixtures/m9-acceptance`。
- 数据集包含源导入根 `source/`、历史图库种子 `history-library/`、数据说明 `README.md` 和机器可校验预期 `expected-results.json`。
- 覆盖 M9 数据集要求中的关键样本：图集内完全重复、同样例不同编码、Unicode 路径、sidecar/nested 文件、损坏图片、空图集、跨图集重复、历史图库重复种子、小规模多图 smoke。
- 新增 `scripts/verify-m9-acceptance-dataset.mjs` 和 `pnpm release:dataset`，校验 `expected-results.json` 与实际文件数量、sidecar、历史图库种子一致。
- `checklists/M9_DOD.md` 已勾选固定验收数据集项。

### 修改文件

- `fixtures/m9-acceptance/**`
- `scripts/verify-m9-acceptance-dataset.mjs`
- `package.json`
- `checklists/M9_DOD.md`
- `reports/milestone-9-progress.md`

### 执行命令与测试结果

- `pnpm release:dataset`：passed，verified 8 source albums, 44 source files, 1 history image。

### 实际运行结果

- 固定验收数据集可由脚本重复校验，不再只依赖文档描述。

### 已知限制

- 当前数据集是 release acceptance smoke 数据集；1k/10k/100k 级性能数据仍需在 M9 性能门禁中单独生成和记录。
- 数据集尚未通过完整 GUI/IPC 主链跑完导入；后续 M9 项继续补该验收。
