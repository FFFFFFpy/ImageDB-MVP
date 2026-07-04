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

## 2026-07-04: Database library-root lease

### 实现内容

- 新增 migration `0010_library_root_leases`，以 PostgreSQL 表 `library_root_leases` 作为同一图库根写入者的权威租约。
- 租约字段包含 `library_root_id`、`owner_instance_id`、`lease_token`、`heartbeat_at`、`expires_at`。
- `ImportRepository` 新增 acquire / heartbeat / release / read API。
- acquire 使用数据库原子 upsert：只有同 owner、同 token 或已过期租约允许续租/接管；活跃的其他 owner 会被明确拒绝。
- commit 主链在写入 album 前获取图库根租约，写入循环和关键阶段 heartbeat，完成后释放租约。
- Recovery 的 active transaction 写路径也会获取同一图库根租约，恢复动作结束后释放租约。
- 存储上的 lock 文件仍未作为权威锁。

### 修改文件

- `apps/desktop/src-tauri/migrations/0010_library_root_leases.sql`
- `apps/desktop/src-tauri/src/infrastructure/postgres/migration.rs`
- `apps/desktop/src-tauri/src/infrastructure/postgres/manager.rs`
- `apps/desktop/src-tauri/src/repositories/import_repository.rs`
- `apps/desktop/src-tauri/src/services/commit_service.rs`
- `apps/desktop/src-tauri/src/tests/protocol_integration.rs`
- `apps/desktop/src/pages/SettingsPage.test.tsx`
- `checklists/M8_DOD.md`
- `reports/milestone-8-progress.md`

### 执行命令与测试结果

- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml migration`：5 passed。
- `cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml`：passed。
- `IMAGEDB_POSTGRES_BIN=D:\MyProjects\Agent\ImageDB-MVP\.local\db-tools\postgresql-18.4\pgsql\bin cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features real-db-tests --lib real_protocol_library_root_lease -- --ignored --test-threads=1`：1 passed。

### 实际运行结果

- 真实 PostgreSQL 测试验证：owner-a 持有同一 `library_root_id` 租约时 owner-b 无法获取；owner-a 释放后 owner-b 可获取；租约过期后 owner-c 可接管。
- migration head 更新为 `0010_library_root_leases`。

### 已知限制

- Recovery 目前按动作持有租约，尚未在长时间单文件恢复复制中做分块 heartbeat。
- 断连/只读/空间不足恢复、路径逃逸增强和真实挂载共享存储故障门禁仍未完成。

## 2026-07-04: Path safety hardening

### 实现内容

- 加强目标相对路径校验：
  - 禁止绝对路径、盘符前缀、`..`、空路径；
  - 规范化分隔符为 `/`；
  - 拒绝 Windows 保留名；
  - 拒绝尾随点或空格；
  - 检查单组件长度和相对路径总长度。
- 加强同一 album 内目标路径冲突检查：
  - 检测大小写折叠冲突；
  - 检测 Unicode NFC 规范化后的冲突。
- 发布前检查目标目录现有祖先，拒绝 symlink / reparse point 祖先，避免目标路径逃逸图库根。
- `publish_verified_staging` 统一执行目标祖先检查，覆盖 StrongLocal、ConservativeMounted 以及 Recovery 重新发布路径。

### 修改文件

- `apps/desktop/src-tauri/Cargo.toml`
- `apps/desktop/src-tauri/Cargo.lock`
- `apps/desktop/src-tauri/src/services/commit_service.rs`
- `apps/desktop/src-tauri/src/services/recovery_service.rs`
- `checklists/M8_DOD.md`
- `reports/milestone-8-progress.md`

### 执行命令与测试结果

- `cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml`：passed。
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml commit_service::tests::`：35 passed。

### 实际运行结果

- 单元测试验证保留名、尾随点/空格、长组件、长相对路径被拒绝。
- 单元测试验证大小写冲突和 Unicode NFC 冲突被拒绝。
- 单元测试在平台允许创建 symlink 时验证目标祖先 symlink 会被拒绝。

### 已知限制

- 当前路径长度阈值为应用级保守限制，不等同于每个文件系统的精确最大路径能力。
- 断连/只读/空间不足恢复、保守发布故障注入门禁和真实挂载共享存储故障测试仍未完成。

## 2026-07-04: Recovery storage reprobe guard

### 实现内容

- Recovery 在执行 active transaction 动作前重新探测当前图库根 `StorageCapabilities`。
- 若重探测结果为 `Unsupported`，Recovery 暂停并保持原 transaction 中间态，只写入 `last_error` 诊断；不把断连、只读或挂载不可写误写成完成。
- 对需要继续写入 staging / publish 的恢复状态估算本次写入字节数，并在可用空间不足或空间无法验证时暂停恢复。
- Recovery staging 分支遇到 source file 当前不可见时，保持 `staging` 状态并提示 reconnect 后重试，不再直接落为 `failed` 终态。

### 修改文件

- `apps/desktop/src-tauri/src/services/recovery_service.rs`
- `reports/milestone-8-progress.md`

### 执行命令与测试结果

- `cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml`：passed。
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml recovery_service::tests::`：9 passed。

### 实际运行结果

- 单元测试验证 Unsupported 的库根重探测会暂停恢复并保留 volume identity 诊断。
- 单元测试验证可用空间小于预计恢复写入量时会暂停恢复。
- 恢复分支现在把“source 当前不可见”视为可重试断连/挂载不可见状态，不会生成虚假完成，也不会把 transaction 变成不可恢复终态。

### 已知限制

- 当前实现会重新探测并记录 volume identity，但尚未把初始 volume identity 持久化到事务行，因此还不能严格证明“同一路径重新挂载到不同设备”。
- 真实 SMB/NAS 人工断连、重新挂载、Recovery 收敛门禁尚未执行。

## 2026-07-04: Conservative marker recovery real-db gate

### 实现内容

- 为 `fail-injection` 测试增加仅测试环境可用的 ConservativeMounted 发布策略强制开关，正式构建不暴露该开关。
- 新增 `BeforeCommitMarker` 故障点，覆盖 ConservativeMounted 已复制文件与 manifest、但尚未写入 `.imagedb-commit.json` 的中断窗口。
- 补充真实 PostgreSQL + 真实文件系统测试：commit 在 marker 前中断后，Recovery 重新验证已发布 manifest 和文件集合，补写 commit marker，继续 DB commit 与 source archive，最终收敛到 `source_archived`。
- 修正 fail-injection 测试环境：预创建 library root，使 M8 `StorageCapabilities` 探测符合正式主链前置条件。

### 修改文件

- `apps/desktop/src-tauri/src/services/commit_service.rs`
- `apps/desktop/src-tauri/src/tests/fail_injection.rs`
- `apps/desktop/src-tauri/src/tests/fail_injection_tests.rs`
- `checklists/M8_DOD.md`
- `reports/milestone-8-progress.md`

### 执行命令与测试结果

- `cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml`：passed。
- `IMAGEDB_POSTGRES_BIN=D:\MyProjects\Agent\ImageDB-MVP\.local\db-tools\postgresql-18.4\pgsql\bin cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features fail-injection,real-db-tests --lib fail_injection_conservative_before_commit_marker_recovers -- --ignored --test-threads=1`：1 passed。

### 实际运行结果

- 测试确认 marker 前中断时目标目录和 manifest 已存在，但 `.imagedb/.imagedb-commit.json` 不存在。
- Recovery 运行后 `.imagedb/.imagedb-commit.json` 被补写，事务最终达到 `source_archived`，图库记录存在。

### 已知限制

- 该测试使用本机真实文件系统强制 ConservativeMounted 策略，不等同于真实 SMB/NAS 人工断网门禁。
- 真实挂载共享存储故障测试仍未完成。
