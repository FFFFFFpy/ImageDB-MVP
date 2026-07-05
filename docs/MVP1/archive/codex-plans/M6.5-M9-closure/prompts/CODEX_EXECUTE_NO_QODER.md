你现在接管当前仓库：

`D:\MyProjects\Agent\ImageDB-MVP`

当前分支应为：

`core_fix_m5_m6_refactor`

进入目标执行模式。你是唯一执行者。不要调用 Qoder 或其他代码代理。

## 目标

执行 `.codex-plans/M6.5-M9-closure/` 中的收口计划。

只做 M6.5–M9 主链收口，不扩张功能，不钻牛角尖，不扣无关细节。

## 严格禁止

```text
git push
git push --force
gh pr create
创建远程分支
合并 main
发布 Release
上传构建产物
继续新功能
重写整体架构
```

不得使用：

```text
git reset --hard
git clean -fd
git checkout -- .
```

## 开始前

执行：

```powershell
git status
git branch --show-current
git log --oneline --decorate -20
git diff main...HEAD --stat
```

完整阅读：

```text
AGENTS.md
CURRENT_TASK.md
PROJECT_PLAN.md
tasks/
reports/
.codex-plans/M6.5-M9-closure/README.md
.codex-plans/M6.5-M9-closure/AGENT_GUARDRAILS.md
.codex-plans/M6.5-M9-closure/tasks/06_5_09_closure.md
.codex-plans/M6.5-M9-closure/checklists/ACCEPTANCE_CHECKLIST.md
```

先按计划包更新项目任务文档，创建提交：

```text
docs: add M6.5-M9 closure plan
```

## 工作原则

1. 每次只解决一个主链阻断项。
2. 代码必须接入 GUI / IPC / Service / Repository / DB / 测试链路。
3. 不做理论完美，只做到 MVP 可验收。
4. 不改无关代码。
5. 不因为小问题无限重构。
6. 测试不能 skip 后算通过。
7. 报告不能宣称未完成内容已完成。

## 必做任务

### 1. M6.5 runtime 打包闭环

修正：

```text
package-postgres-runtime.mjs
PostgresManager runtime search
tauri.conf.json resources/bundle
verify-release-artifacts.mjs
release gate
```

目标：Release 包内自带 PostgreSQL + pgvector，干净 Windows 环境无需安装 PostgreSQL。

不要实现跨平台 runtime。

### 2. 真实测试 fail-fast

`pnpm rust:test:real` 缺 runtime 必须失败，不能 skip。

managed database lifecycle、M9 public main chain、recovery、commit、manifest 测试不能缺 PostgreSQL 后早退。

### 3. M7 外部 PostgreSQL TLS 主链

把 database service 外部连接测试/初始化接到已有 TLS connector。

不要另写一套连接器。

### 4. Frozen Plan 主链

实现或修正：

```text
freeze_import_plan
get_frozen_import_plan_summary
Review页调用 freeze
Commit页读取 frozen summary
Commit Service consume same frozen plan
```

Commit 页不得动态重算计划。

### 5. latest committable run 查询

ready_to_commit 优先。

completed 不进入默认提交页。

recovery_required 进入恢复页。

### 6. M8 mounted storage 最小门禁

只做已挂载路径能力探测，不做 SMB 协议。

检测：读写、rename、大小写、Unicode、长路径、权限/断连错误。

### 7. 文档和报告同步

更新 PROJECT_PLAN、CURRENT_TASK、tasks、reports。

不要写空泛报告。

## 每阶段提交建议

```text
fix: package managed postgres runtime as release resource
fix: fail real tests when postgres runtime is missing
fix: route external postgres checks through tls connector
feat: freeze import plans for public commit workflow
fix: prefer ready import runs for commit
feat: add mounted storage capability gate
reports: document M6.5-M9 closure status
```

可根据实际合并，但提交必须边界清楚。

## 最终验证

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

若某项无法运行，报告必须写明原因和影响，不得伪装通过。

## 完成定义

全部满足才完成：

- 默认本地模式不需要系统 PostgreSQL。
- Release 找得到 packaged runtime。
- pgvector 初始化成功。
- 真实测试缺 runtime fail-fast。
- 外部 PostgreSQL 使用 TLS connector。
- Commit 页读取 frozen plan summary。
- Commit 执行同一个 frozen plan。
- latest committable run 不选旧 completed run。
- mounted storage gate 有真实能力探测。
- 文档状态与代码一致。
- 所有验证命令完成或明确记录无法运行。
- 本地提交完成。
- 未推送。

现在开始执行。不要先复述计划，不要询问是否继续。
