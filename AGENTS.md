# ImageDB Agent 开发规则

## 当前项目阶段

MVP1 已定性为：

```text
功能完成，进入 Debug / 实战测试阶段。
```

当前 canonical 文档入口：[`docs/MVP1/README.md`](docs/MVP1/README.md)

用户明确要求 MVP2 工作时，当前 MVP2 文档入口为：[`docs/MVP2/README.md`](docs/MVP2/README.md)

用户明确要求 MVP3 / M3 UI 重设计工作时，当前 MVP3 文档入口为：[`docs/MVP3/README.md`](docs/MVP3/README.md)

Agent / Codex 不应再按“继续开发下一个里程碑”的方式工作。默认只处理 Debug、bugfix、诊断、测试、文档和 release gate 问题。
例外：用户明确指定 MVP2 或 MVP3 任务包时，可以在独立 feature 分支上执行，但不得破坏 frozen plan / commit / recovery 文件事务安全边界。MVP3 只重设计呈现层与交互层，不得在 React 中重建后端状态机。

## 阅读顺序

每次开始前依次阅读：

1. `AGENTS.md`
2. `docs/MVP1/README.md`
3. `docs/MVP1/STATUS.md`
4. `CURRENT_TASK.md`
5. `docs/MVP1/DEBUG_PLAYBOOK.md`
6. 若任务明确属于 MVP2，阅读 `docs/MVP2/README.md`、`docs/MVP2/ALBUM_WORKFLOW.md`、`docs/MVP2/ACCEPTANCE.md`
7. 若任务明确属于 MVP3 / M3，阅读 `docs/MVP3/README.md`、`docs/MVP3/PRODUCT_BRIEF.md`、`docs/MVP3/UI_SPEC.md`、`docs/MVP3/IMPLEMENTATION_PLAN.md`、`docs/MVP3/ACCEPTANCE.md`
8. 与当前 bug / gate / 测试相关的代码或文档

历史材料位于 `docs/MVP1/archive/`，只用于追溯，不作为当前状态入口。

## 固定技术栈

- GUI：React + TypeScript + Vite
- 桌面容器：Tauri 2
- 核心后端：Rust
- 数据库：PostgreSQL + pgvector
- 默认数据库模式：应用管理私有本地 PostgreSQL
- 高级数据库模式：连接外部 PostgreSQL

## MVP1 主链

MVP1 已跑通的主链：

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

## 允许的改动

- 修复实战测试暴露的 bug。
- 修复数据一致性、文件事务、恢复路径问题。
- 修复安装包、PostgreSQL runtime、路径、权限、编码问题。
- 性能和稳定性硬化。
- 错误提示、日志、诊断增强。
- 测试补充。
- 文档修正。
- release gate / install gate 修正。
- 用户明确指定的 MVP3 UI 重设计任务，但必须保持业务状态机与文件事务语义不变。

## 默认禁止的改动

除非用户明确要求，不要做：

- 新增 MVP1 之外的功能。
- 大改 scan / review / commit / recovery 主链。
- 重写数据库 schema 或迁移链。
- 重写匹配算法。
- 大 UI 改版。
- 非必要架构重构。
- 没有真实问题支撑的“顺手优化”。

## 架构边界

- Commands 只负责 IPC 边界，不承载业务逻辑。
- React 不直接访问数据库或文件系统。
- 业务规则放在 Rust domain / services 中。
- 数据访问统一经过 repositories。
- 长任务必须支持进度上报和取消。
- 状态转换集中定义，不在各模块中随意写状态字符串。

## 数据安全规则

1. 分析阶段不修改源文件。
2. 正式写入采用 staging、校验、发布、数据库提交的顺序。
3. 发布成功并通过完整性校验前，不归档源图集。
4. 不覆盖未知目标目录。
5. 无法可靠判断的图片交给用户审核。
6. 所有提交操作必须可恢复、可重试、保持幂等。
7. Commit 必须读取 frozen plan，不得临场重算导入计划。

## 质量要求

每个任务完成时必须提供：

- 实现内容
- 修改文件
- 执行命令
- 测试结果
- 实际运行结果
- 已知限制

单元测试不能替代真实数据库和真实文件系统集成测试。若没有运行某个验证命令，必须明确说明没有运行。
