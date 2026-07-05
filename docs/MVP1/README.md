# ImageDB MVP1 文档入口

> **当前定性：MVP1 功能完成，进入 Debug / 实战测试阶段。**
>
> 这不是“继续扩功能”的阶段。除非实战测试暴露阻断问题，否则主线只接受 bugfix、诊断、文档、测试和发布门禁修正。

## 1. 当前版本状态

- 主线分支：`main`
- MVP1 功能状态：**完成**
- 当前阶段：**Debug / 实战测试 / 发布验收补强**
- 主链人工验收：**已通过**
- 正式发布签字：**未完成**，仍需 clean Windows `pnpm release:gate`

MVP1 的核心主链已经跑通：

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

## 2. Canonical 文档

后续以 `docs/MVP1/` 为唯一主入口。旧的计划、提示词、任务拆分和历史报告已经归档到 `docs/MVP1/archive/`，不再作为当前状态入口。

| 文档 | 用途 |
| --- | --- |
| [`STATUS.md`](./STATUS.md) | MVP1 完成定性、DoD、剩余 Debug 项 |
| [`ARCHITECTURE.md`](./ARCHITECTURE.md) | 当前 MVP1 产品范围、主链、架构、数据与文件事务 |
| [`DEBUG_PLAYBOOK.md`](./DEBUG_PLAYBOOK.md) | 实战测试、Debug 记录、验证命令和问题分级 |
| [`DOCUMENT_MAP.md`](./DOCUMENT_MAP.md) | 文档地图与归档口径 |

## 3. 当前工作原则

### 允许做

- 修复实战测试暴露的 bug。
- 补充诊断信息、日志、错误提示。
- 补测试、补文档、补 release gate。
- 修 clean Windows 安装、启动、卸载、数据保留问题。
- 修大目录、长时间运行、恢复路径问题。

### 不允许默认做

- 新增 MVP1 之外的功能。
- 大改 scan / review / commit / recovery 主链。
- 重写数据库 schema 或迁移链。
- 重写匹配算法。
- 为了“顺手优化”改架构。
- 把发布验收写成已完成，除非 clean Windows gate 真的跑完。

## 4. 当前验收边界

MVP1 现在按以下口径管理：

```text
功能开发阶段：结束
本地主链验收：通过
Debug / 实战测试：进行中
正式发布签字：未完成
```

剩余未签字项主要是：

- clean Windows 完整安装与 `pnpm release:gate`
- 大图库性能测试
- 24 小时稳定性 / soak
- 备份、恢复、升级、卸载完整验收
- 诊断包脱敏确认

这些属于发布级验收和稳定性补强，不再改变 MVP1“功能完成”的定性。

## 5. 常用命令

本地常规验证：

```bash
pnpm format:check
pnpm typecheck
pnpm test:unit
pnpm rust:test
pnpm rust:clippy
```

具备 PostgreSQL runtime 时：

```bash
pnpm rust:test:real
```

Windows release 验证：

```bash
pnpm build
pnpm release:verify-artifacts
pnpm release:install-gate
```

正式发布签字：

```bash
pnpm release:gate
```

## 6. 归档处理原则

- `docs/MVP1/` 是当前版本唯一入口。
- `docs/MVP1/archive/` 保存旧计划、旧提示词、旧任务拆分和历史报告。
- 根目录 `reports/` 仅作为脚本输出目录，用于新生成的环境检查和 gate 报告。
- `checklists/RELEASE_DOD.md` 保留为 release 级 DoD 证据，但当前状态摘要以 [`STATUS.md`](./STATUS.md) 为准。
- 不再让 agent 在归档文档里散写新结论。
