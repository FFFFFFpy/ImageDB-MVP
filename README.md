# ImageDB MVP

> 本地优先的桌面图集导入、重复检测、人工审核与安全入库工作台。

ImageDB 面向本地磁盘、外接存储和 NAS / SMB 图库整理场景。它把“选择待整理图集 → 检测重复与相似图片 → 人工确认 → 安全写入正式图库”收敛成一条可中断、可恢复、可审计的桌面工作流。

## 当前状态

| 项目 | 状态 |
| --- | --- |
| 主线分支 | `main` |
| MVP1 主链 | 功能完成，本地人工验收通过 |
| MVP2 图集工作流 | 已进入主线 |
| MVP3 / M3 UI 重设计 | M3.0–M3.8 已进入主线，继续实战验证与收口 |
| MVP4 Fingerprint V2 | 实现与审查修复完成，已进入主线 |
| 当前阶段 | Debug、真实数据验证、性能与发布硬化 |
| 正式发布 | 尚未发生，clean Windows `pnpm release:gate` 仍未签字 |

当前版本已经能完整跑通实际入库主链，但仍属于开发与实战测试版本，尚未达到正式稳定版的发布口径。

## 工作流

```text
全新开始 / 连接数据库
        ↓
选择源目录并发现图集
        ↓
按图集扫描、快照与指纹计算
        ↓
图集内部重复检测 + 历史图库比较
        ↓
人工审核重复 / 相似候选
        ↓
生成并冻结导入计划
        ↓
Staging → 文件校验 → 发布目录 → 数据库确认
        ↓
源图集归档 / 中断恢复
```

源目录的一级子目录被视为独立图集。分析阶段不会修改源文件；正式入库只读取已经冻结的导入计划，不会在 Commit 时临场重新决定要写入哪些图片。

## 核心能力

### 本地优先数据库

- 默认由应用管理私有本地 PostgreSQL，并启用 pgvector。
- 高级模式支持连接外部 PostgreSQL，包括 TLS、连接预检与迁移流程。
- Dashboard 展示图库根目录、图集与图片数量、导入任务、待审核、失败、冻结计划和恢复状态。

### 图集级可恢复工作流

- 每个图集独立记录分析进度和结果。
- 支持中断后继续 pending、stale analyzing 和 failed retry 图集。
- 失败图集可以单独重置后重新分析。
- 在文件事务开始前，可以撤销整次导入；审核决定、源快照和计划历史仍保留为审计证据。
- 图库明细只读展示已经提交的图集与图片，不向用户暴露数据库内部结构。

### Fingerprint V2 重复检测

MVP4 已将旧的 8×8 自实现感知哈希和图集内全量两两比较替换为固定生产方案。算法、尺寸、滤镜与阈值是代码常量，不提供算法设置 UI。

| 用途 | 当前实现 |
| --- | --- |
| 文件精确匹配 | 完整文件 BLAKE3，32 bytes |
| 像素精确匹配 | 应用 EXIF Orientation 后的规范化 RGBA8 + 宽高 BLAKE3 |
| 粗召回 | BlockHash 16×16 + Triangle |
| 精细验证 | DoubleGradient 32×32 + Triangle |
| 几何关系 | 旋转、镜像、Transpose、Transverse，共 8 种变换 |
| 图集内召回 | 临时 Hamming BK-tree |
| 历史图库召回 | 应用内缓存 Hamming BK-tree |
| 感知安全门 | 灰度方差、有效灰度级、边缘变化量与哈希信息量联合判定 |
| 指纹版本 | `2` |

低信息量图片仍参与文件和像素精确匹配，但不会进入感知 BK-tree，也不会仅凭感知结果自动判重。

### 人工审核

- 双图对比、缩放、叠加和候选证据展示。
- 支持保留、跳过和最终选择等审核动作。
- 前端只呈现后端事实，不在 React 中重新推导工作流状态。
- Dashboard 的主操作由后端 `next_action` 决定。

### 文件事务与恢复

- 正式写入遵循 staging、完整性校验、目录发布、数据库确认和源图集归档顺序。
- Commit 只读取 frozen plan，并校验用户确认时看到的 `plan_hash`。
- 文件事务可恢复、可重试并保持幂等。
- 不覆盖未知目标目录；无法可靠判断的候选交给用户审核。
- 对共享存储使用 PostgreSQL 图库租约限制并发写入。

### 安全清空开发数据库

设置页提供 Debug 用的“清空历史数据库并重新开始”操作：

- 清空 ImageDB 自己的历史记录并重新执行当前全部 migration。
- 不删除图库目录和磁盘文件。
- 不主动删除外部 PostgreSQL 中不属于 ImageDB 的表。
- 存在未完成文件事务或有效图库租约时拒绝执行。
- 扫描、提交和恢复使用 PostgreSQL advisory shared lock；重置和 schema 初始化使用 exclusive lock，跨应用实例关闭竞争窗口。
- 真实 PostgreSQL 测试会比较全新数据库与重置后数据库的表、列、约束和索引，防止重置清单随 migration 漂移。

该操作不可撤销，仅用于明确不再保留当前数据库历史的开发与调试场景。

## 技术栈

- React 19 + TypeScript + Vite 6
- TanStack Query
- Tauri 2 + Rust
- PostgreSQL + pgvector
- `animal-island-ui@1.2.2` 与 ImageDB 本地 UI 适配层
- Vitest + Rust 单元测试 + 真实 PostgreSQL / 文件系统集成测试

## 开始开发

当前发布与安装验证以 Windows 为主。开发环境需要 Node.js、pnpm 10、Rust 工具链以及 Tauri 对应的系统依赖。

```bash
pnpm install
pnpm dev
```

常规质量门：

```bash
pnpm check
```

等价的分项命令：

```bash
pnpm format:check
pnpm typecheck
pnpm test:unit
pnpm rust:clippy
pnpm rust:test
```

真实 PostgreSQL 验证：

```bash
pnpm rust:test:real
```

Windows 构建与安装验证：

```bash
pnpm build
pnpm release:verify-artifacts
pnpm release:install-gate
```

完整发布签字：

```bash
pnpm release:gate
```

只有该命令在 clean Windows 环境真实通过后，才能将正式发布状态更新为已完成。

## 文档入口

| 文档 | 用途 |
| --- | --- |
| [`CURRENT_TASK.md`](CURRENT_TASK.md) | 当前主线状态、显式任务和边界 |
| [`docs/MVP1/README.md`](docs/MVP1/README.md) | MVP1 总入口、Debug 与发布验收口径 |
| [`docs/MVP1/STATUS.md`](docs/MVP1/STATUS.md) | 完成状态、DoD 与未签字项目 |
| [`docs/MVP1/ARCHITECTURE.md`](docs/MVP1/ARCHITECTURE.md) | 主链、数据模型与文件事务架构 |
| [`docs/MVP1/DEBUG_PLAYBOOK.md`](docs/MVP1/DEBUG_PLAYBOOK.md) | 实战测试、数据库重置和诊断手册 |
| [`docs/MVP2/README.md`](docs/MVP2/README.md) | 图集级断点续跑与数据状态可见 |
| [`docs/MVP3/README.md`](docs/MVP3/README.md) | M3 桌面 UI、交互规范与验收入口 |
| [`docs/MVP4/README.md`](docs/MVP4/README.md) | Fingerprint V2 与高效重复检测引擎 |
| [`AGENTS.md`](AGENTS.md) | Agent / Codex 开发规则与阅读顺序 |

历史任务、提示词和旧报告位于 `docs/MVP1/archive/`，只用于追溯，不应再作为当前状态依据。

## Agent / Codex 阅读顺序

1. `AGENTS.md`
2. `docs/MVP1/README.md`
3. `docs/MVP1/STATUS.md`
4. `CURRENT_TASK.md`
5. 与当前任务对应的 MVP2 / MVP3 / MVP4 文档
6. `docs/MVP1/DEBUG_PLAYBOOK.md`

## 第三方组件说明

M3 使用 `animal-island-ui@1.2.2`。相关 attribution、CC BY-NC 4.0 与个人非商业使用边界见 [`docs/MVP3/THIRD_PARTY_NOTICES.md`](docs/MVP3/THIRD_PARTY_NOTICES.md)。如果项目用途改为商业、收费或企业交付，必须先处理该依赖的授权或替换问题。
