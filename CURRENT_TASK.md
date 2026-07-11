# Current Task

## 当前阶段

MVP1 已定性为：

```text
功能完成，进入 Debug / 实战测试阶段。
```

主线分支：`main`

当前 canonical 文档入口：[`docs/MVP1/README.md`](docs/MVP1/README.md)

当前显式任务包：MVP2 图集级断点续跑 + 异步审核入口 + 数据状态可见，文档入口为 [`docs/MVP2/README.md`](docs/MVP2/README.md)。

当前工作分支：`feat/mvp2-album-workflow-dashboard`

当前 Debug / 合并审查任务：修复 candidate review 规范化语义冲突与 abandoned run 当前工作流隔离，migration head 更新为 `0014_candidate_review_semantics_and_abandoned_filters`。修复范围不改变 frozen plan、Commit 或 Recovery 文件事务语义。

## 状态摘要

MVP1 本地主链已人工验收通过：

```text
全新开始
→ 初始化托管本地 PostgreSQL
→ 选择源目录
→ 导入 / 分析
→ 审核
→ 生成 / 冻结导入计划
→ 提交入库
→ 本地目录正式入库
```

当前不是继续扩功能阶段。默认只接受：

- 实战测试暴露的 bugfix。
- Debug / 诊断 / 日志增强。
- 测试补充。
- 文档收敛。
- release gate / install gate 修正。
- clean Windows 发布验收补强。

例外：用户明确要求的 MVP2 任务在独立 feature 分支上执行，仍必须保持 frozen plan / commit / recovery 文件事务安全边界。

## 发布签字状态

- MVP1 功能完成：已定性完成。
- 本地主链人工验收：已通过。
- 单项测试、Clippy、Release 构建与本地 install-gate：已记录通过。
- 完整 clean Windows `pnpm release:gate`：未签字。
- 正式 release publication：未发生。

## 文档入口

| 文档                                                         | 用途                            |
| ------------------------------------------------------------ | ------------------------------- |
| [`docs/MVP1/README.md`](docs/MVP1/README.md)                 | MVP1 文档总入口                 |
| [`docs/MVP1/STATUS.md`](docs/MVP1/STATUS.md)                 | 当前状态、DoD、剩余 Debug 项    |
| [`docs/MVP1/ARCHITECTURE.md`](docs/MVP1/ARCHITECTURE.md)     | MVP1 架构、主链、数据与文件事务 |
| [`docs/MVP1/DEBUG_PLAYBOOK.md`](docs/MVP1/DEBUG_PLAYBOOK.md) | 实战测试和 Debug 手册           |
| [`docs/MVP1/DOCUMENT_MAP.md`](docs/MVP1/DOCUMENT_MAP.md)     | 文档地图与归档口径              |

## 历史记录

旧的里程碑报告、M5/M6 修复报告、M6.5–M9 closure 报告、任务拆分和 Codex 执行计划已经归档到：

```text
docs/MVP1/archive/
```

当前状态不要再从归档文档推断，以 `docs/MVP1/` 为准。
