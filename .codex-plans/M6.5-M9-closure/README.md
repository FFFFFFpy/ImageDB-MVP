# ImageDB-MVP M6.5–M9 收口计划包

本计划包用于当前分支：`core_fix_m5_m6_refactor`。

目标不是继续扩张功能，而是把已经存在的 M6.5–M9 框架接成可验收的主链。

## 核心原则

1. 只做当前阻断项。
2. 不重写架构。
3. 不追求理论完美。
4. 不把测试“跳过”当通过。
5. 不把文件存在当功能完成。
6. 不继续 Milestone 7/8/9 的新需求扩张，先把已写的东西接通。
7. 不推送、不合并 main、不发布 Release。

## 包内文件

- `CURRENT_TASK.md`：建议替换仓库当前任务入口。
- `PROJECT_PLAN_PATCH.md`：建议追加到项目计划中的阶段说明。
- `AGENT_GUARDRAILS.md`：给 agent 的执行边界，防止它钻牛角尖。
- `tasks/06_5_09_closure.md`：完整阶段任务。
- `checklists/ACCEPTANCE_CHECKLIST.md`：验收清单。
- `prompts/CODEX_EXECUTE_NO_QODER.md`：Codex 单独执行提示词。
- `prompts/CODEX_EXECUTE_WITH_QODER.md`：Codex + Qoder 协作提示词。
- `reports-template/M6_5_M9_CLOSURE_REPORT.md`：最终报告模板。

## 建议使用方式

把整个目录复制到仓库：

```powershell
D:\MyProjects\Agent\ImageDB-MVP\.codex-plans\M6.5-M9-closure
```

然后把 `prompts/CODEX_EXECUTE_NO_QODER.md` 或 `prompts/CODEX_EXECUTE_WITH_QODER.md` 交给 Codex。
