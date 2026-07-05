# MVP1 文档地图

## 1. 当前 canonical 入口

后续 MVP1 当前状态、Debug 和实战测试只看：

- `docs/MVP1/README.md`
- `docs/MVP1/STATUS.md`
- `docs/MVP1/ARCHITECTURE.md`
- `docs/MVP1/DEBUG_PLAYBOOK.md`

其它文档保留为历史证据、任务过程或详细实现记录。不要再把新状态散写到各处。文档会自己繁殖，像潮湿角落里的霉菌，必须管。

## 2. 根部文件

| 文件 | 当前用途 |
| --- | --- |
| `CURRENT_TASK.md` | 当前工作阶段摘要。应指向 `docs/MVP1/`，不再承载长篇完成报告。 |
| `PROJECT_PLAN.md` | 顶层项目计划入口。MVP1 当前状态以 `docs/MVP1/` 为准。 |
| `PROJECT_PLAN_PATCH.md` | 历史计划补丁，保留为任务过程记录。 |

## 3. Release / DoD 文档

| 文件 | 当前用途 |
| --- | --- |
| `checklists/RELEASE_DOD.md` | Release 级 DoD 证据。当前明确：MVP 主链本地人工验收通过，但完整 clean Windows `pnpm release:gate` 未签字。 |
| `checklists/M6.5_DOD.md` | 托管 PostgreSQL runtime / Windows 安装包阶段 DoD。 |
| `checklists/M7_DOD.md` | 外部 PostgreSQL / TLS / 迁移阶段 DoD。 |
| `checklists/M8_DOD.md` | 挂载共享目录能力阶段 DoD。 |
| `checklists/M9_DOD.md` | 发布收口阶段 DoD。历史证据，不作为当前唯一入口。 |

## 4. Reports

| 文件 | 当前用途 |
| --- | --- |
| `reports/m6_5_m9_closure.md` | M6.5–M9 收口报告，记录 runtime、real-db fail-fast、external PostgreSQL、mounted storage、frozen plan、release gate 状态。 |
| `reports/m9-performance-thresholds.json` | 120 图 MVP performance baseline。1k/10k/100k 仍属后续硬化。 |
| `reports/core-fix-M5-M6-final.md` | M5/M6 核心修复报告。 |
| `reports/core-fix-M5-M6-round2-final.md` | M5/M6 二轮修复报告。 |
| `reports/core-fix-M5-M6-round3-final.md` | M5/M6 三轮修复报告。 |
| `reports/M6.5-M9-final.md` | 旧版 M6.5–M9 总结，后续被 `m6_5_m9_closure.md` 修正。 |
| `reports/milestone-*.md` | 各里程碑历史报告。 |

## 5. docs/ 下其它文件

| 文件 | 当前用途 |
| --- | --- |
| `docs/acceptance-matrix.md` | 验收矩阵细节。当前摘要以 `docs/MVP1/STATUS.md` 为准。 |
| `docs/runtime-layout.md` | PostgreSQL runtime / release resource 布局细节。 |
| `docs/storage-capability-contract.md` | 存储能力探测与策略契约。 |
| `docs/ImageDB-MVP_Core_Fix&M5_M6_Refactor.md` | 核心修复和 M5/M6 重构历史说明。 |

## 6. tasks/ 目录

`tasks/` 保留为任务拆分和执行过程记录。

| 文件 | 当前用途 |
| --- | --- |
| `tasks/06.5-managed-postgres-runtime.md` | 托管 PostgreSQL runtime 任务。 |
| `tasks/07-external-postgres.md` | 外部 PostgreSQL 任务。 |
| `tasks/08-mounted-storage.md` | 挂载共享目录任务。 |
| `tasks/09-release-closure.md` | 发布收口任务。 |
| `tasks/README.md` | 任务目录入口。 |

MVP1 当前状态不要再直接从这些任务文件判断，以 `docs/MVP1/STATUS.md` 为准。

## 7. .codex-plans/

`.codex-plans/M6.5-M9-closure/` 是执行计划和 agent 提示词归档：

- `AGENT_GUARDRAILS.md`
- `CURRENT_TASK.md`
- `PROJECT_PLAN_PATCH.md`
- `README.md`
- `checklists/ACCEPTANCE_CHECKLIST.md`
- `prompts/*`
- `reports-template/*`
- `tasks/06_5_09_closure.md`

这些文件是过程记录，不是当前状态入口。

## 8. 实战 Debug 新记录应该放哪里

建议后续新增：

```text
docs/MVP1/debug/YYYY-MM-DD-<short-title>.md
```

每个实战问题按 `DEBUG_PLAYBOOK.md` 的模板记录。

不要把 Debug 记录散落到：

- 根目录临时文件。
- reports 根目录。
- tasks 目录。
- `.codex-plans`。

让文档归位，就是为了少给未来的自己挖坑。未来的自己已经够惨了。
