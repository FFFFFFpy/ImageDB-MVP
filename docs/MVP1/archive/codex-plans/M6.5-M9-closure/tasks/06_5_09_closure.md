# M6.5–M9 Closure Task

## 0. 工作方式

本轮只做收口，不做扩张。

每个任务完成后必须满足：

```text
代码接入正式主链
测试覆盖真实路径
脚本与构建产物一致
文档反映真实状态
```

不要因为某个边缘细节还能继续优化就无限停留。当前目标是让 MVP 可安装、可初始化、可完成公开主链。

---

## 1. M6.5：托管 PostgreSQL Runtime 打包闭环

### 当前问题

代码已有 runtime locator 和打包脚本，但以下链条未闭合：

```text
package-postgres-runtime.mjs
→ tauri.conf.json resource/bundle
→ release artifact
→ PostgresManager runtime search
→ clean Windows bootstrap
```

### 必做

1. 确认 runtime 目录标准：

```text
postgres-runtime/
  bin/postgres.exe
  bin/pg_ctl.exe
  bin/initdb.exe
  bin/psql.exe
  bin/pg_dump.exe
  lib/vector.dll
  share/extension/vector.control
  share/extension/vector--*.sql
```

2. 修改 Tauri 配置，把 `postgres-runtime` 作为 Windows resource 打包。
3. 确保 release 后 exe 能通过 `resource_dir` 找到 runtime。
4. 保留开发路径 fallback，但 release 首选 resource runtime。
5. `verify-release-artifacts.mjs` 与实际 bundle 配置一致。
6. 缺 runtime 时显示“安装包不完整”，不要提示普通用户安装 PostgreSQL。
7. 写测试验证 locator 在模拟 release resource 目录能找到 runtime。
8. 写 clean bootstrap 测试：无系统 PostgreSQL，仅使用 packaged runtime。

### 不做

- 不解决 Linux/macOS 打包。
- 不重写 PostgreSQL manager。
- 不做动态下载 PostgreSQL。
- 不引入 Docker。

---

## 2. 真实测试 fail-fast

### 当前问题

部分真实测试找不到 PostgreSQL runtime 时会 skip，导致“通过”不可信。

### 必做

1. `pnpm rust:test:real` 缺 `.local/db-tools` 或 packaged runtime 时必须失败。
2. release gate 缺 runtime 必须失败。
3. M9 public main chain、managed database lifecycle、commit pipeline、manifest/recovery 不能 skip。
4. 允许普通 `pnpm rust:test` 保持快速、无 runtime 依赖。
5. 测试输出明确告诉用户缺哪个文件。

### 不做

- 不要求所有普通单元测试依赖 PostgreSQL。
- 不做 CI 平台搭建。

---

## 3. M7：外部 PostgreSQL TLS 主链接入

### 当前问题

TLS connector 代码存在，但 database service 外部连接路径仍可能使用 `NoTls`。

### 必做

1. 让 database service 外部连接测试和初始化使用统一 external connector。
2. 支持 TLS mode：disable / require / verify-ca / verify-full。
3. 预检 PostgreSQL 版本。
4. 预检 pgvector 可用性。
5. 预检 CREATE EXTENSION / schema / migration 权限。
6. UI 显示 TLS 与外部连接诊断。
7. 不破坏托管本地模式。

### 不做

- 不做复杂证书生命周期管理。
- 不实现云数据库向导。
- 不做多数据库同步。

---

## 4. M9：Frozen Plan 公开主链收口

### 当前问题

Review/Commit 页仍可能动态重算计划，而正式 Commit 使用 frozen plan。

### 必做

1. 新增或明确 IPC：`freeze_import_plan(import_run_id)`。
2. Review 完成后调用 freeze，事务内写入三张 plan 表和 plan_hash。
3. 冻结后 run 状态进入 `ready_to_commit`。
4. 新增 IPC：`get_frozen_import_plan_summary(import_run_id)`。
5. Commit 页只读取 frozen plan summary，不再调用动态 `generateImportPlan()`。
6. Commit Service 使用同一 frozen plan。
7. 冻结后修改候选或审核结果，不改变 Commit 页和提交集合。
8. 重复 freeze 幂等：已有 frozen plan 时返回同一 plan summary。

### 不做

- 不重写 Review UI。
- 不增加复杂 diff UI。
- 不新增审批系统。

---

## 5. Latest Committable Run 查询修正

### 当前问题

旧 completed run 可能因为 `completed_at DESC` 抢在新的 ready_to_commit run 前。

### 必做

1. 默认提交页优先选择 `ready_to_commit`。
2. 其次才考虑可重新提交的 `cancelled` 且无活动事务 run。
3. `completed/consumed` 不应进入默认提交页。
4. recovery_required 进入恢复页，不进入提交页。
5. 按 `started_at DESC` 选最新待提交 run。

### 不做

- 不重写历史记录页。
- 不实现多 run 管理器。

---

## 6. M8 Mounted Storage 最小能力门禁

### 当前问题

已有 mounted storage gate 入口，但产品能力证据不足。

### 必做

1. 检测目标目录读写权限。
2. 检测同目录 rename 是否原子或至少是否可用。
3. 检测大小写行为。
4. 检测长路径。
5. 检测 Unicode 文件名。
6. 检测临时断连/权限错误时不会误报成功。
7. 结果写入诊断报告。
8. release gate 在设置 mounted root 时真实执行。

### 不做

- 不实现 SMB 协议。
- 不实现分布式锁。
- 不实现 NAS 探测。
- 不保证所有网络文件系统都完美工作。

---

## 7. 项目任务账本同步

### 必做

1. 更新 `PROJECT_PLAN.md`。
2. 更新 `CURRENT_TASK.md`。
3. 补充 `tasks/06_5-managed-postgres-runtime.md`。
4. 更新 `tasks/07-external-postgres.md` 为真实任务，不保留旧空壳。
5. 更新 `tasks/08-mounted-storage.md`。
6. 新增 `tasks/09-release-gate.md`。
7. 写最终报告到 `reports/m6_5_m9_closure.md`。

### 不做

- 不写空泛宣传稿。
- 不让文档宣称未完成内容已完成。

---

## 8. 最终验证

必须运行：

```powershell
pnpm install
pnpm format:check
pnpm typecheck
pnpm test:unit
pnpm rust:test
pnpm rust:clippy
pnpm rust:test:real
pnpm build
pnpm release:gate
```

如果某个命令在当前环境无法运行，必须在报告中明确：

```text
未运行
原因
影响
下一步
```

不得写“应该可以”。
