# MVP1 文档地图

## 1. 当前 canonical 入口

后续 MVP1 当前状态、Debug 和实战测试只看：

- `docs/MVP1/README.md`
- `docs/MVP1/STATUS.md`
- `docs/MVP1/ARCHITECTURE.md`
- `docs/MVP1/DEBUG_PLAYBOOK.md`

旧计划、旧提示词、任务拆分和历史报告已经归档到 `docs/MVP1/archive/`。

## 2. 根部文件

| 文件              | 当前用途                              |
| ----------------- | ------------------------------------- |
| `README.md`       | 项目门面，指向 `docs/MVP1/`。         |
| `AGENTS.md`       | Agent / Codex 当前阶段工作规则。      |
| `CURRENT_TASK.md` | 当前 Debug 阶段摘要。                 |
| `PROJECT_PLAN.md` | 顶层项目计划入口，指向 `docs/MVP1/`。 |
| `ENVIRONMENT.md`  | 开发环境与验证命令。                  |

## 3. 当前保留的根目录输出 / 证据目录

| 路径          | 当前用途                                                                                                   |
| ------------- | ---------------------------------------------------------------------------------------------------------- |
| `reports/`    | 脚本输出目录。环境检查和 release performance 会继续写入这里。历史报告已搬到 `docs/MVP1/archive/reports/`。 |
| `checklists/` | Release / milestone DoD 证据目录。`RELEASE_DOD.md` 仍保留为 release 级证据。                               |

## 4. 归档位置

| 原路径                                 | 归档位置                                  |
| -------------------------------------- | ----------------------------------------- |
| `.codex-plans/`                        | `docs/MVP1/archive/codex-plans/`          |
| `prompts/`                             | `docs/MVP1/archive/prompts/`              |
| `tasks/`                               | `docs/MVP1/archive/tasks/`                |
| `PROJECT_PLAN_PATCH.md`                | `docs/MVP1/archive/PROJECT_PLAN_PATCH.md` |
| 历史 `reports/*.md` / `reports/*.json` | `docs/MVP1/archive/reports/`              |

## 5. Reports 口径

根目录 `reports/` 不再作为历史状态文档入口。它只作为脚本输出目录，例如：

- `scripts/check-env.ps1`
- `scripts/check-env.sh`
- `pnpm release:performance`

当前状态不要看根 `reports/`，看 `docs/MVP1/STATUS.md`。

## 6. 归档残留说明

部分根目录旧文件如果因为工具安全限制不能物理删除，会被视为归档残留，不能作为当前入口。当前状态仍以 `docs/MVP1/` 为准。

## 7. 实战 Debug 新记录应该放哪里

建议后续新增：

```text
docs/MVP1/debug/YYYY-MM-DD-<short-title>.md
```

每个实战问题按 `DEBUG_PLAYBOOK.md` 的模板记录。
