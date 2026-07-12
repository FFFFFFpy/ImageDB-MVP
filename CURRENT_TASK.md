# Current Task

## 当前阶段

MVP1 已定性为：

```text
功能完成，进入 Debug / 实战测试阶段。
```

主线分支：`main`

当前 canonical 文档入口：[`docs/MVP1/README.md`](docs/MVP1/README.md)

当前显式任务包：MVP3 / M3 桌面端 UI 重设计，文档入口为 [`docs/MVP3/README.md`](docs/MVP3/README.md)。

当前工作分支：`codex/mvp3-ui-redesign`

当前实施阶段：M3.5，迁移导入计划与 Commit；M3.0–M3.4 的视觉基线、基础设计系统、应用壳、工作台、新建导入、分析进度与图片审核工作台已完成。

M3 固定边界：Dashboard 下一步继续由后端 `next_action` 统一路由；React 不根据零散计数猜测状态机。M3 不修改 frozen plan、Commit、Recovery、数据库 migration 或匹配算法语义。

MVP2 图集级断点续跑、异步审核入口和数据状态可见已经作为 M3 的业务基线保留；其 canonical 文档仍为 [`docs/MVP2/README.md`](docs/MVP2/README.md)。

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

MVP1 主线仍不是继续扩功能阶段。当前 feature 分支额外接受用户明确要求的 M3 UI 重设计：

- 实战测试暴露的 bugfix。
- Debug / 诊断 / 日志增强。
- 测试补充。
- 文档收敛。
- release gate / install gate 修正。
- clean Windows 发布验收补强。
- 不改变业务语义的呈现层、交互层和设计系统迁移。

例外：用户明确要求的 MVP2 / MVP3 任务在独立 feature 分支上执行，仍必须保持 frozen plan / commit / recovery 文件事务安全边界。

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
