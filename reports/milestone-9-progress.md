# M9 Progress Report

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
