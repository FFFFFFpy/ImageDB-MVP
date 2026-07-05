# ImageDB MVP 项目计划

## 当前口径

MVP1 已定性为：

```text
功能完成，进入 Debug / 实战测试阶段。
```

当前 canonical 文档入口已经收敛到：[`docs/MVP1/`](docs/MVP1/README.md)

后续判断当前版本状态、Debug 任务、验收边界、架构说明时，优先看 `docs/MVP1/`，不要再从历史归档或脚本输出中拼状态。

## MVP1 主链

MVP1 的核心流程已经完成并通过本地人工验收：

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

## MVP1 功能范围

MVP1 覆盖：

- 托管本地 PostgreSQL + pgvector。
- 外部 PostgreSQL 连接、TLS、预检和迁移。
- 源目录扫描，一级子目录作为图集。
- 文件快照。
- BLAKE3 / 像素 hash / 感知 hash。
- 图集内部重复与相似检测。
- 与历史图库比较。
- 人工审核 GUI。
- frozen import plan。
- staging / 校验 / 发布 / DB 确认 / 源图集归档。
- Recovery / Reverify。
- 挂载共享目录能力探测和保守发布策略。
- Windows release runtime packaging 和 install gate。

## 当前未完成但不阻断 MVP1 定性的事项

以下属于 Debug / 发布硬化 / 正式 release sign-off：

- clean Windows 完整 `pnpm release:gate`。
- 1k / 10k / 100k 大图库性能验证。
- 24 小时稳定性 / soak。
- 备份、恢复、升级、卸载完整验收。
- 诊断包脱敏确认。
- 更多 NAS / SMB / 外接盘实战矩阵。

## 文档入口

| 文档 | 用途 |
| --- | --- |
| [`docs/MVP1/README.md`](docs/MVP1/README.md) | MVP1 文档总入口 |
| [`docs/MVP1/STATUS.md`](docs/MVP1/STATUS.md) | 当前状态、完成标准、剩余 Debug 项 |
| [`docs/MVP1/ARCHITECTURE.md`](docs/MVP1/ARCHITECTURE.md) | 架构、主链、数据库、文件事务 |
| [`docs/MVP1/DEBUG_PLAYBOOK.md`](docs/MVP1/DEBUG_PLAYBOOK.md) | 实战测试和 Debug 手册 |
| [`docs/MVP1/DOCUMENT_MAP.md`](docs/MVP1/DOCUMENT_MAP.md) | 文档地图与归档口径 |

## 历史归档

旧计划、提示词、任务拆分和历史报告已经归档到：

```text
docs/MVP1/archive/
```

根目录 `reports/` 仅保留为脚本输出目录，例如环境检查报告和性能 gate 新输出。

这些历史文件用于追溯实现过程和验收证据，不再作为当前状态入口。

## 后续开发原则

MVP1 Debug 阶段默认只做：

- bugfix；
- 诊断增强；
- 测试补充；
- 文档修正；
- release gate 修正；
- 性能和稳定性硬化。

默认不做：

- 新功能；
- 新算法方向；
- 大 UI 改版；
- 非必要数据库 schema 大改；
- 主链重构；
- 没有真实问题支撑的“顺手优化”。

## 版本结论

```text
MVP1 功能完成。
当前阶段：Debug / 实战测试。
正式发布：等待 clean Windows release gate 签字。
```
