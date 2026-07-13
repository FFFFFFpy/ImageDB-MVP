# M3 实施计划

## 1. 实施原则

M3 使用渐进式迁移：先建立 token 和应用壳，再逐页替换现有界面。每个任务包结束时应用必须可构建、现有主链必须可运行，不创建一个长期不可用的“大爆炸”分支。

建议分支：

```text
codex/mvp3-ui-redesign
```

如由其他执行者创建分支，也必须使用独立 feature 分支，不直接在 MVP1/MVP2 稳定主线上展开大 UI 改造。

## 2. 技术策略

- 保持 React + TypeScript + Vite + Tauri 2。
- 允许引入 `animal-island-ui` 作为 M3 基础组件库，并锁定经过验证的版本。
- 图标库若新增，必须支持 tree-shaking、离线打包、统一 stroke 风格，并记录许可证。
- 在 `components/ui/` 建立 ImageDB 适配层，页面只依赖本地导出，不在各页面散布第三方组件 API。
- 组件库覆盖按钮、输入、提示、基础卡片等通用控件；审核、计划、Commit、Recovery 等领域组件独立实现。
- 将组件库的 CC BY-NC 4.0、原作者 attribution 和非商业用途声明纳入应用 About/许可证文档及分发产物检查。
- 现有 IPC API 和 React Query 数据流优先保持不变。
- 只有 UI 无法表达真实状态且后端缺少明确事实时，才提出单独的后端契约任务；不得在组件内猜测。

## 3. 任务包

### M3.0 视觉基线与截图夹具

交付：

- 固定用于截图的窗口尺寸和缩放比例。
- 为工作台、分析、审核、计划、入库、恢复准备确定性 UI fixture 或受控测试数据。
- 记录现有页面截图作为回归对照。
- 验证 `animal-island-ui` 与当前 React 19、Vite 6、Tauri 2 构建的兼容性、包体积和离线字体行为。
- 确定图标方案，并记录全部第三方许可证与 attribution。

门禁：没有稳定 fixture 时不开始全页面视觉验收，否则截图差异会被动态数据污染。

### M3.1 Design tokens 与基础组件

交付：

- 将 `global.css` 拆分为 token、reset/base、layout、components、pages，或采用等价的可维护组织。
- 建立 `components/ui/` 适配层，封装实际采用的 `animal-island-ui` 组件及其样式入口。
- Button、IconButton、StatusBadge、StatusBanner、Progress、Skeleton、EmptyState、PageHeader、Tooltip。
- 统一 focus、disabled、loading、error、reduced-motion 行为。
- 建立组件级单元测试与展示页；展示页可以开发期存在，不必进入生产一级导航。

门禁：所有交互组件通过键盘和对比度检查。

### M3.2 应用壳与工作台

交付：

- 新侧栏、紧凑模式、任务徽标、设置底部入口。
- 工作台“下一步”主任务面板、最近任务、图库摘要、系统健康条。
- 技术探针从一级导航迁入设置诊断区。

门禁：对每一个后端 `next_action` 验证 CTA 标签与目标路由，不能出现 React 二次推断。

### M3.3 新建导入与分析进度

交付：

- 目录选择、发现图集、预检说明、分析进度和图集列表。
- resume、retry、abandon、cancel 的完整状态和确认文案。
- 空目录、权限失败、坏图片、长路径、大列表状态。

门禁：真实目录 + 真实 Tauri 文件选择器 + 显式 resume run id 人工实测。

### M3.4 审核工作台

交付：

- 图片优先的双图/叠加布局。
- 固定决策区、键盘快捷键、缩放/拖动、元数据详情。
- 候选加载、预览失败、最后一项完成、跳过图集。

门禁：决策语义、selected image 与 candidate 方向保持现有后端规范；不得因视觉左右顺序改变最终选择。

### M3.5 导入计划与入库

交付：

- 计划摘要、图集展开、图片级保留/排除、冻结状态。
- 入库前确认、阶段进度、取消反馈、完成/部分失败结果。
- 长列表性能处理。

门禁：Commit 读取同一 frozen plan；UI 展示数量、plan summary 与真实 Commit 输入一致。

### M3.6 Recovery、设置与首次配置

交付：

- 可恢复、人工处置、历史终态分区。
- 数据库初始化/外部连接向导。
- 设置和诊断重组，保留技术探针能力。

门禁：conflict、missing operations、terminal failed/cancelled 等 fail-closed 状态无误导操作。

### M3.7 统一收尾

交付：

- 全页面视觉一致性、窄窗口、Windows 缩放、长文本、中文路径检查。
- 性能、a11y、reduced motion、焦点与错误文案审计。
- 删除确认不再使用的旧 CSS，更新测试和文档。
- 真实数据库、真实文件系统完整主链复验。

门禁：[`ACCEPTANCE.md`](ACCEPTANCE.md) 全部必需项签字后才可定性 M3 完成。

### M3.8 安全撤销未入库工作流与图库明细（用户授权扩展）

交付：

- Review 与 Commit 确认页提供“撤销这次导入”入口；成功后 import run 进入 `abandoned`、frozen plan 进入 `invalidated`，保留既有审核决定与源快照审计。
- 撤销成功后清空前端 workflow run 上下文、刷新 Dashboard 状态并返回工作台“开始导入”，原任务不再可恢复为审核、计划或提交。
- 撤销与 Commit capture 共用 import run 行锁；任何文件事务创建后拒绝撤销，不改变 Recovery 语义。
- 工作台图库概览进入只读图库明细，分页列出已提交图集，并在展开时分页读取图片元数据。
- 图库查询遵循 command → service → repository 边界，React 只消费 DTO；不增加 schema 或 migration。
- 覆盖确认交互、缓存失效、事务边界、图库分页和 loading / error / empty 回归测试。

门禁：真实 PostgreSQL 证明整条工作流撤销在事务创建前可审计且不再 actionable、事务创建后 fail closed；前端证明单任务撤销后 Dashboard 回到新建导入；图库查询只返回 committed 数据并保持分页总数一致。

## 4. 预计主要修改范围

```text
apps/desktop/src/styles/
apps/desktop/src/components/
apps/desktop/src/pages/
apps/desktop/src/app/App.tsx
apps/desktop/src/hooks/use-router.ts
apps/desktop/src/**/*.test.tsx
```

默认不应修改：

```text
apps/desktop/src-tauri/src/domain/
apps/desktop/src-tauri/src/services/
apps/desktop/src-tauri/src/repositories/
apps/desktop/src-tauri/migrations/
```

若实施中确需修改默认不应修改的范围，必须拆成独立、可解释的契约修复，并证明不是前端自行重写业务语义。

M3.8 的 Rust 改动属于用户明确授权的受限业务契约扩展：复用既有 `abandoned` run 状态、`invalidated` 计划状态、行锁与事务证据，不修改 schema、文件发布顺序或 Recovery 状态机；图库部分只新增只读查询。

`abandoned` / `invalidated` 是 M3.8 唯一授权的状态语义扩展。除该扩展外，审查修复只能恢复既有契约与交互安全，不得改变原工作流、frozen plan、Commit、文件事务或 Recovery 语义。

## 5. 每个任务包的验证

### 自动验证

```bash
pnpm format:check
pnpm typecheck
pnpm test:unit
pnpm --filter @imagedb/desktop build:web
```

影响后端契约时额外执行：

```bash
pnpm rust:test
pnpm rust:clippy
pnpm rust:test:real
```

### 人工验证

- Tauri 开发窗口启动与导航。
- 100%、125%、150% Windows 缩放可作为兼容性抽样；100% / 150% 人工切换不属于本轮 M3.8 审查修复的完成门禁。
- 1440×900、1280×720、960×720 和最小支持窗口。
- 键盘全流程、焦点可见性、Esc 与弹层焦点归还。
- 中文、空格、括号、长路径与不可断词错误文本。
- 空数据、加载、慢加载、错误、取消、恢复和成功。

### 真实主链

```text
初始化数据库
→ 选择真实源目录
→ 分析
→ 审核
→ 生成 / 冻结计划
→ 执行入库
→ 核对图库与源归档
```

必须额外执行至少一次中断恢复路径。截图或浏览器 fixture 不能替代真实文件系统结果。

## 6. 测试迁移策略

- 优先按 accessible role、name、label 查询，减少对 className 和 DOM 层级的耦合。
- 保留现有业务行为断言；视觉重构不能通过删除断言来“修复”测试。
- 为 `next_action`、审核决策、冻结计划、恢复入口建立参数化 UI 测试。
- 视觉回归覆盖稳定 fixture，不对动态时间、随机 UUID 和真实路径做像素断言。
- 大列表以交互延迟和 DOM 节点数作为性能证据，不只凭主观滚动感受。

## 7. 回滚策略

- 每个任务包独立提交，避免 CSS、路由和所有页面一次性混在同一提交。
- 新旧页面迁移期间允许内部 feature flag 或路由级切换，但合并前必须删除永久双实现。
- 不通过数据库 migration 支撑纯 UI 改造，因此回滚 UI 不应影响已有数据。
- 若新 UI 发现无法表达某个事务状态，优先恢复旧页面并补契约，不以隐藏状态作为临时解决方案。
