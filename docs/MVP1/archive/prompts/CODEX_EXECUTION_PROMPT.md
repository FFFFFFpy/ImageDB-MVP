# Codex 目标模式执行提示词

你现在接管 ImageDB-MVP 当前工作分支，按以下顺序完成剩余产品化里程碑：

```text
M7 外部 PostgreSQL
→ M8 已挂载共享存储
→ M9 发布收口
```

## 规则

- 先阅读 `AGENTS.md`、`PROJECT_PLAN.md`、`CURRENT_TASK.md` 和本计划包全部文件。
- 直接修改代码、测试、文档和构建配置，不只输出建议。
- 每个里程碑必须按任务文件的完成定义验收。
- 可以创建本地 Git 提交。
- 禁止 `git push`、远程分支、PR、Release和上传构建产物。
- 不覆盖用户未提交改动。
- 不使用 `git reset --hard`、`git clean -fd` 或 `git checkout -- .`。
- 不保留新旧两套正式主链。
- 发现前置核心闭环问题时先修复，再继续后续里程碑。
- 当前任务应从 `tasks/07-external-postgres.md` 开始。M6.5 文件只作为托管运行时和发布复验依据；除非验收发现托管运行时回归，不要重新开启 M6.5 作为当前任务。

## 当前基线

当前仓库记录：

- M5/M6 核心修复与恢复闭环已完成；
- 真实 PostgreSQL 18.4 + pgvector 0.8.3 + 真实文件系统测试通过；
- 当前执行任务是 M7 外部 PostgreSQL；
- M9 仍必须覆盖干净 Windows 安装包和无预装数据库复验。

如果实际代码或报告与以上基线不一致，以代码和真实验收结果为准，并先更新 `CURRENT_TASK.md` 与阶段报告。

## M7

严格执行 `tasks/07-external-postgres.md`：

- 严格TLS；
- 凭据安全存储；
- 版本、扩展、权限和只读预检；
- 托管到外部迁移；
- 完整验证后切换；
- 失败回滚。

## M8

严格执行 `tasks/08-mounted-storage.md`：

- 存储能力探测；
- 发布策略分级；
- 数据库租约；
- 断连恢复；
- 路径冲突保护；
- 真实挂载存储故障验收。

## M9

严格执行 `tasks/09-release-closure.md`，不得使用测试辅助函数替代公开GUI/IPC主链。

## 每阶段流程

1. 审查现状和依赖。
2. 更新 `CURRENT_TASK.md`。
3. 完成最小但完整的正式实现。
4. 补单元、集成、真实环境和故障测试。
5. 运行格式、类型、Rust测试、Clippy和构建。
6. 亲自审查 `git diff`。
7. 创建范围清晰的本地提交。
8. 更新阶段报告和完成清单。
9. 达到门禁后进入下一里程碑。

## 最终验证

```powershell
pnpm install
pnpm format:check
pnpm typecheck
pnpm test:unit
pnpm rust:test
pnpm rust:clippy
pnpm rust:test:real
pnpm build
```

另外实际完成：

- 干净Windows安装；
- 无PostgreSQL初始化；
- GUI完整导入闭环；
- 外部数据库TLS与迁移；
- 真实共享存储断连恢复；
- 24小时稳定性。

完成后输出：

- 每个里程碑完成情况；
- 测试和真实环境结果；
- Windows安装包与可执行文件路径；
- 本地提交列表；
- 未推送确认；
- 剩余真实限制。

现在开始执行，不要先复述计划，不要询问是否继续。
