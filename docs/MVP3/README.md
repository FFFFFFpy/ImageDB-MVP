# ImageDB MVP3 / M3 文档入口

M3 目标：

```text
在不改变现有导入、审核、冻结计划、Commit 与 Recovery 业务语义的前提下，
重设计 ImageDB 桌面端 UI，使主流程更轻松、可信、安静，并始终给出清晰的下一步。
```

M3 是一次完整的桌面端呈现层与交互层重设计，不是新业务里程碑，也不是后端状态机重写。

## 当前状态

```text
文档基线：已建立
视觉方向：已确定
代码实现：M3.0–M3.6 已完成，M3.7 仅剩系统缩放人工签字
验收状态：自动门禁、压力场景、真实 Tauri UI 全流程与中断恢复通过；Windows 100% / 150% 待签字
```

当前已锁定 `animal-island-ui@1.2.2`，完成 React 19 / Vite 6 / Tauri 2 的兼容性验证，并建立 ImageDB 语义 token、本地 UI 适配层、基础组件测试和开发期视觉夹具。全部生产页面已完成 M3 迁移：审核左右图语义由测试锁定，计划与提交保持两个步骤，Commit 只读同一份 frozen plan，Recovery 明确区分可恢复、冲突与终态事务。设置 About 已展示作者、CC BY-NC 4.0 和个人非商业用途声明。计划图集和图片分别按 50 个、24 张分批挂载。83 项前端测试、99 项真实 PostgreSQL/文件系统测试、1,000 图集/10,000 图片压力夹具、全页 WCAG 自动审计、Tauri release/NSIS 构建、真实 Tauri UI 主链以及分析/Commit 中断恢复均已通过。当前机器以 Windows 125% 系统缩放完成实测；M3.7 仅因 100% 与 150% 系统缩放尚未人工切换签字而保持打开。证据状态见 [`VALIDATION.md`](VALIDATION.md)。

M3 实现应在独立 feature 分支上进行。MVP1 与 MVP2 的数据安全、frozen plan、Commit、Recovery 和后端 `next_action` 边界继续作为强制约束。

## Canonical 文档

| 文档                                               | 用途                                   |
| -------------------------------------------------- | -------------------------------------- |
| [`PRODUCT_BRIEF.md`](PRODUCT_BRIEF.md)             | 产品目标、设计原则、范围与信息架构     |
| [`UI_SPEC.md`](UI_SPEC.md)                         | 视觉系统、应用壳、页面、状态与交互规范 |
| [`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md) | 实施顺序、任务包、迁移策略与验证要求   |
| [`ACCEPTANCE.md`](ACCEPTANCE.md)                   | M3 完成标准、视觉/交互/回归验收清单    |
| [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) | M3 第三方组件 attribution 与许可证说明 |
| [`VISUAL_BASELINE.md`](VISUAL_BASELINE.md)         | 固定视口、确定性场景与兼容性基线       |
| [`VALIDATION.md`](VALIDATION.md)                   | M3.7 自动、真实环境与桌面运行验收记录  |

## 一句话方向

ImageDB M3 是一个以图片为主角的桌面工作台：沿用参考母版的左侧导航、单任务主画布和线性工作流，吸收 `animal-island-ui` 的圆润、自然、亲切与按压反馈，但降低游戏化装饰，保证长任务、异常恢复和高密度数据场景仍然专业可信。

## 固定边界

- React 只负责呈现和交互，不直接访问数据库或文件系统。
- Dashboard 的主 CTA 继续完全服从后端 `next_action`，前端不重建状态机。
- Commit 只读取 frozen plan，不在 UI 层临场重算计划。
- M3 不修改数据库 schema、migration、匹配算法或文件事务协议。
- 长任务继续支持进度、取消、失败定位和恢复。
- 不得为了适配新 UI 隐藏真实错误、合并不同终态或弱化 fail-closed 行为。

## 参考来源与许可策略

- 布局母版：用户在 M3 启动时提供的 ImageDB 重设计参考图。
- 风格参考：[`guokaigdg/animal-island-ui`](https://github.com/guokaigdg/animal-island-ui)。
- 项目所有者已明确 ImageDB 为个人自用、非商业项目，因此 M3 允许直接安装和使用该组件库。
- 参考仓库采用 CC BY-NC 4.0。实现与分发时必须保留原作者 attribution、许可证声明和非商业限制，不得移除或改写为更宽松的许可。
- 通过 ImageDB 本地 UI 适配层使用第三方组件，业务页面不直接依赖其全部 API。布局、数据表、审核画布等 ImageDB 特有组件仍由项目自行实现。
- 如果项目用途未来变化为商业、收费、企业交付或外部商业服务，必须在继续分发前替换该依赖或取得额外授权。

## 推荐实施命令

实现阶段每个任务包至少执行：

```bash
pnpm format:check
pnpm typecheck
pnpm test:unit
pnpm --filter @imagedb/desktop build:web
```

涉及 Tauri 窗口、文件选择、预览或真实流程时，还必须运行桌面应用并进行真实数据库、真实文件系统人工验收；Web 构建与单元测试不能替代桌面实测。
