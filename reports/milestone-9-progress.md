# M9 Progress Report

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
