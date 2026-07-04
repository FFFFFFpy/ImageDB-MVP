你现在接管当前仓库：

`D:\MyProjects\Agent\ImageDB-MVP`

当前分支应为：

`core_fix_m5_m6_refactor`

进入目标执行模式。Codex 负责拆解、审查、验收和 Git 提交；Qoder 负责代码修改和局部测试。

## 禁止

```text
git push
gh pr create
创建远程分支
合并 main
发布 Release
上传构建产物
继续新功能
重写整体架构
```

Qoder 不得执行 Git 提交。

不得使用：

```text
git reset --hard
git clean -fd
git checkout -- .
```

## 开始前

Codex执行：

```powershell
git status
git branch --show-current
git log --oneline --decorate -20
qoderclicn --version
qoderclicn --help
```

完整阅读：

```text
AGENTS.md
CURRENT_TASK.md
PROJECT_PLAN.md
tasks/
reports/
.codex-plans/M6.5-M9-closure/
```

## Qoder通用指令模板

每次交给Qoder时都必须包含：

```text
你是本批次代码执行者。

先阅读 AGENTS.md 和本批涉及的正式代码。

要求：
1. 直接修改当前工作区。
2. 只完成当前批次，不扩展无关功能。
3. 必须接入正式主链，不只写 helper。
4. 补真实有效测试。
5. 运行相关测试、format、clippy。
6. 修复本次引入的失败。
7. 不执行 git commit。
8. 不执行 git push。
9. 不合并分支。
10. 不重写整体架构。
11. 不钻牛角尖，不扣无关细节。
12. 完成后汇报修改文件、实现内容、测试命令、测试结果、剩余风险。

当前批次任务：
<填写具体任务>
```

Qoder完成后，Codex必须执行：

```powershell
git diff --check
git diff
git status
```

并亲自审查正式主链。

## 批次一：文档接入

把计划包合入项目任务体系。

提交：

```text
docs: add M6.5-M9 closure plan
```

## 批次二：M6.5 runtime打包闭环

目标：Release 包含 PostgreSQL + pgvector，PostgresManager 从 resource_dir 找到 runtime。

重点文件：

```text
scripts/package-postgres-runtime.mjs
scripts/verify-release-artifacts.mjs
apps/desktop/src-tauri/tauri.conf.json
apps/desktop/src-tauri/src/infrastructure/postgres/manager.rs
```

不要做跨平台 runtime。

提交：

```text
fix: package managed postgres runtime as release resource
```

## 批次三：真实测试 fail-fast

目标：`pnpm rust:test:real` 缺 runtime 失败，不 skip。

重点：

```text
scripts/run-real-rust-tests.mjs
apps/desktop/src-tauri/src/tests/*
```

提交：

```text
fix: fail real tests when postgres runtime is missing
```

## 批次四：M7外部 PostgreSQL TLS主链

目标：database service 外部连接路径使用现有 TLS connector。

不要另写连接器。

提交：

```text
fix: route external postgres checks through tls connector
```

## 批次五：Frozen Plan公开主链

目标：Review freeze，Commit读取 frozen summary，Commit执行同一个 frozen plan。

Commit页不得动态重算计划。

提交：

```text
feat: freeze import plans for public commit workflow
```

## 批次六：latest committable run 查询

目标：ready_to_commit优先，completed不抢占默认提交页。

提交：

```text
fix: prefer ready import runs for commit
```

## 批次七：Mounted storage 最小门禁

目标：已挂载路径能力探测，不做 SMB 协议。

提交：

```text
feat: add mounted storage capability gate
```

## 批次八：报告与最终验证

更新 reports，运行：

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

提交：

```text
reports: document M6.5-M9 closure status
```

## 完成定义

- runtime打包链路闭合。
- release环境能找到 PostgreSQL runtime。
- 真实测试缺runtime fail-fast。
- 外部PostgreSQL使用TLS connector。
- Commit页读取 frozen plan。
- ready_to_commit不会被completed抢占。
- mounted storage gate真实执行。
- 文档和报告可信。
- 全部验证命令执行或明确说明无法执行。
- Codex完成本地提交。
- Qoder没有提交。
- 未推送。

现在开始执行。不要先复述计划，不要询问是否继续。
