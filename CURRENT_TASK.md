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

当前实施阶段：M3.8 审查修复与验证收口。M3.0–M3.6 的视觉系统与全部生产页面迁移已经完成；用户授权的 M3.8 保留“只读图库明细”和“文件事务创建前撤销整次未入库工作流”。`abandoned` import run 与 `invalidated` frozen plan 是 M3.8 唯一授权的状态语义扩展，扫描、审核、计划、Commit、文件事务与 Recovery 的其余语义保持不变。本轮验证结果只在命令实际执行后记录；Windows 100% / 150% 系统缩放不属于本轮完成门禁，也不再作为 M3 阻塞项。

M3 固定边界：Dashboard 下一步继续由后端 `next_action` 统一路由；React 不根据零散计数猜测状态机。除 M3.8 明确授权的 `abandoned` / `invalidated` 外，M3 不修改 frozen plan、Commit、Recovery、数据库 migration、匹配算法或文件事务语义。

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
- 用户授权的 M3.8 受限扩展：只读图库明细，以及文件事务创建前撤销整次未入库工作流并保留审计证据。

例外：用户明确要求的 MVP2 / MVP3 任务在独立 feature 分支上执行，仍必须保持 frozen plan / commit / recovery 文件事务安全边界。

## 发布签字状态

- MVP1 功能完成：已定性完成。
- 本地主链人工验收：已通过。
- 单项测试、Clippy、Release 构建与本地 install-gate：已记录通过。
- 完整 clean Windows `pnpm release:gate`：未签字。
- 正式 release publication：未发生。
- MVP3 UI 重设计：M3.0–M3.8 已进入审查修复与验证收口；Windows 100% / 150% 系统缩放不是本轮完成门禁或阻塞项。

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
