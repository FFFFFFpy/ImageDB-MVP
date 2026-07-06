# ImageDB MVP

ImageDB 是一个本地优先的桌面图集整理应用。

> **MVP1 当前定性：功能完成，进入 Debug / 实战测试阶段。**
>
> 当前版本状态、Debug 边界和文档入口以 [`docs/MVP1/`](docs/MVP1/README.md) 为准。
>
> 当前分支开始显式落地 MVP2 图集级流程基础，入口见 [`docs/MVP2/`](docs/MVP2/README.md)。

## 当前状态

```text
MVP1 功能完成。
当前阶段：Debug / 实战测试。
正式发布：等待 clean Windows release gate 签字。
MVP2 基础：图集级断点续跑 + 异步审核入口 + 数据状态可见。
```

MVP1 本地主链已经人工验收通过：

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

## 文档入口

| 文档                                                         | 用途                            |
| ------------------------------------------------------------ | ------------------------------- |
| [`docs/MVP1/README.md`](docs/MVP1/README.md)                 | MVP1 文档总入口                 |
| [`docs/MVP1/STATUS.md`](docs/MVP1/STATUS.md)                 | 当前状态、DoD、剩余 Debug 项    |
| [`docs/MVP1/ARCHITECTURE.md`](docs/MVP1/ARCHITECTURE.md)     | MVP1 架构、主链、数据与文件事务 |
| [`docs/MVP1/DEBUG_PLAYBOOK.md`](docs/MVP1/DEBUG_PLAYBOOK.md) | 实战测试和 Debug 手册           |
| [`docs/MVP1/DOCUMENT_MAP.md`](docs/MVP1/DOCUMENT_MAP.md)     | 文档地图与归档说明              |
| [`docs/MVP2/README.md`](docs/MVP2/README.md)                 | MVP2 图集流程文档入口           |

历史计划、提示词、任务和报告已经归档到：

```text
docs/MVP1/archive/
```

根目录 `reports/` 仅保留为脚本生成报告的输出目录。

## 技术栈

- React + TypeScript + Vite
- Tauri 2 + Rust
- PostgreSQL + pgvector
- 应用默认管理私有本地 PostgreSQL
- 高级模式支持连接外部 PostgreSQL

## 开始开发

```bash
pnpm install
pnpm dev
```

常用验证：

```bash
pnpm format:check
pnpm typecheck
pnpm test:unit
pnpm rust:test
pnpm rust:clippy
```

真实 PostgreSQL 验证：

```bash
pnpm rust:test:real
```

Windows release 验证：

```bash
pnpm build
pnpm release:verify-artifacts
pnpm release:install-gate
```

完整发布签字：

```bash
pnpm release:gate
```

## Agent / Codex 阅读顺序

1. `AGENTS.md`
2. `docs/MVP1/README.md`
3. `docs/MVP1/STATUS.md`
4. `CURRENT_TASK.md`
5. `docs/MVP1/DEBUG_PLAYBOOK.md`

不要再从 `docs/MVP1/archive/`、历史 `reports` 或旧任务文件推断当前状态。
