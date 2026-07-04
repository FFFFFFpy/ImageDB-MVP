# M8 Progress Report

## 2026-07-04: Storage capability probe

### 实现内容

- 新增 `StorageCapabilities` 结构化探测，针对已挂载路径创建专用 `.imagedb-capability-probe-*` 临时目录并在探测结束后清理。
- 探测项覆盖可读、可写、创建目录、同目录文件 rename、同根 rename、目录 rename、覆盖 rename、文件 `sync_all`、父目录 `sync_all`、大小写敏感、Unicode 规范化、长路径、长文件名、文件锁、时间戳精度、可用空间和卷身份。
- 无法可靠确认的能力返回 `unknown`，不会按支持处理。Windows 卷身份当前使用稳定 Rust 无法读取，明确返回 `unknown`。
- 新增发布策略分级结果：`strong_local`、`conservative_mounted`、`unsupported`。未知的父目录同步能力会降级为 `conservative_mounted`，不会归类为 `strong_local`。
- 新增 Tauri IPC `probe_storage_capabilities`，设置页可对当前图库根目录执行探测并显示能力报告、策略依据和诊断信息。

### 修改文件

- `apps/desktop/src-tauri/src/infrastructure/storage_capabilities.rs`
- `apps/desktop/src-tauri/src/infrastructure/mod.rs`
- `apps/desktop/src-tauri/src/commands/settings_cmd.rs`
- `apps/desktop/src-tauri/src/lib.rs`
- `apps/desktop/src/lib/ipc/types.ts`
- `apps/desktop/src/lib/ipc/api.ts`
- `apps/desktop/src/pages/SettingsPage.tsx`
- `apps/desktop/src/pages/SettingsPage.test.tsx`
- `checklists/M8_DOD.md`
- `reports/milestone-8-progress.md`

### 执行命令与测试结果

- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml storage_capabilities`：3 passed。
- `pnpm test:unit -- SettingsPage`：SettingsPage/App 相关 8 tests passed。
- `pnpm format:check`：passed。
- `pnpm typecheck`：passed。
- `pnpm test:unit`：8 tests passed。
- `pnpm rust:test`：177 passed, 1 ignored。
- `pnpm rust:clippy`：passed。

### 实际运行结果

- Rust 测试在真实临时文件系统目录中执行探测，确认专用 probe 目录会清理。
- 缺失根目录返回 `unsupported`，不会 panic。
- `parent_dir_sync = unknown` 时策略分级为 `conservative_mounted`，不会被当作 `strong_local`。
- 设置页点击“检测存储能力”后会调用 IPC 并渲染发布策略、能力明细与策略依据。

### 已知限制

- 当时尚未把 commit service 的发布流程切换为 StrongLocal 或 ConservativeMounted；后续 “Publish strategy integration” 已接入。
- 多实例数据库租约、保守发布提交标记与 Recovery、断连恢复、路径逃逸规则和真实挂载共享存储故障门禁仍未完成。
- Windows 卷身份探测当前返回 `unknown`，后续需要稳定 Win32 API 绑定或等价实现。

## 2026-07-04: Publish strategy integration

### 实现内容

- commit 主链在提交前对图库根执行 `StorageCapabilities` 探测，并根据结果选择：
  - `StrongLocal`：继续使用整目录 rename 发布；
  - `ConservativeMounted`：逐文件复制到目标目录、校验 BLAKE3、复制 manifest，最后写入 `.imagedb/.imagedb-commit.json`；
  - `Unsupported`：拒绝提交，不写图库。
- 新增 `CommitMarker`，绑定 transaction ID、plan hash、manifest hash、完整文件集合和 publish strategy version。
- 新提交的发布目录必须具备合法 commit marker；幂等验证会拒绝缺失或不匹配的 marker。
- Recovery 的 `publishing` 路径接入策略选择；目标目录已存在时会重新校验 manifest、文件集合和 commit marker，缺 marker 时在校验通过后补写 marker。

### 修改文件

- `apps/desktop/src-tauri/src/services/commit_service.rs`
- `apps/desktop/src-tauri/src/services/recovery_service.rs`
- `checklists/M8_DOD.md`
- `reports/milestone-8-progress.md`

### 执行命令与测试结果

- `cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml`：passed。
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml commit_marker`：2 passed。
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml conservative_publish`：1 passed。
- `pnpm rust:clippy`：passed。
- `pnpm rust:test`：180 passed, 1 ignored。
- `pnpm format:check`：passed。

### 实际运行结果

- 单元测试在真实临时目录中执行 ConservativeMounted 发布，确认文件和 manifest 被复制到目标目录、`.imagedb-commit.json` 被写入、staging 被清理。
- marker 校验测试确认 plan hash 不匹配会被拒绝。
- 缺失 marker 会被作为无效发布证据暴露，不会被静默视为完成。

### 已知限制

- 尚未完成真实 PostgreSQL 故障注入测试：例如 marker 前中断、目标存在但 marker 缺失后的 Recovery 收敛。
- 多实例数据库租约、断连/只读/空间不足恢复、路径逃逸增强和真实挂载共享存储门禁仍未完成。
