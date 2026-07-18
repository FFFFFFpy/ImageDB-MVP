# Current Task

## 当前阶段

MVP1 已定性为：

```text
功能完成，进入 Debug / 实战测试阶段。
```

主线分支：`main`

当前 canonical 文档入口：[`docs/MVP1/README.md`](docs/MVP1/README.md)

当前显式任务包：多图审核、大分辨率解码与移动入库；横跨 MVP2 审核/入库体验和 MVP4 Fingerprint V2 解码边界。

当前工作分支：`codex/group-review-large-image-move-import`

当前实施阶段：任务包实现完成并进入审查修复收口。审核由 pair 决策升级为持久化连通组，组内逐图 keep/exclude 是 frozen plan 的唯一审核事实，resolved 组成员在界面上全部只读；大图解码保持 Fingerprint V2 算法、哈希和阈值不变，产品像素上限提高到 5 亿并对 1 亿像素以上解码实行单槽限流；导入计划新增默认关闭的 `move_selected_without_backup`，模式绑定 plan hash、manifest、文件事务、恢复和结果，且仅在发布与数据库证据复验后，将 frozen plan 选中源图原子隔离到持久化同目录临时路径并验证删除。`source_files_removing` 统一进入 Dashboard Recovery；临时删除 I/O 错误保持可重试，证据冲突才进入永久 conflict。

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

MVP1 主线仍不是继续扩功能阶段。当前 feature 分支额外接受用户明确要求的 MVP4 指纹与重复检测替换：

- 实战测试暴露的 bugfix。
- Debug / 诊断 / 日志增强。
- 测试补充。
- 文档收敛。
- release gate / install gate 修正。
- clean Windows 发布验收补强。
- 固定 Fingerprint V2、BK-tree 召回、批量候选读写和审核证据展示。
- 不改变审核动作、frozen plan、Commit、发布、归档和 Recovery 文件事务语义。

例外：用户明确要求的 MVP2 / MVP3 / MVP4 任务在独立 feature 分支上执行，仍必须保持 frozen plan / commit / recovery 文件事务安全边界。

## 发布签字状态

- MVP1 功能完成：已定性完成。
- 本地主链人工验收：已通过。
- 单项测试、Clippy、Release 构建与本地 install-gate：已记录通过。
- 完整 clean Windows `pnpm release:gate`：未签字。
- 正式 release publication：未发生。
- MVP3 UI 重设计：M3.0–M3.8 已进入审查修复与验证收口；Windows 100% / 150% 系统缩放不是本轮完成门禁或阻塞项。
- 多图审核 / 大图解码 / 移动入库：实现完成；审查发现的 Dashboard 恢复路由、临时删除错误分类、删除 TOCTOU 和 resolved 组只读问题已修复，默认 Rust、前端、真实 PostgreSQL / 文件系统 / 完整故障注入门禁通过，待提交。

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
