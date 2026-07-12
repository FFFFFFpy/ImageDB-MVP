# M3 验收标准

## 1. 完成口径

M3 只有在以下四类门禁同时通过后，才能定性为完成：

```text
视觉系统统一
+ 全部现有流程完成迁移
+ 业务与数据安全回归通过
+ Tauri 真实环境人工验收通过
```

仅完成工作台、仅完成 CSS 换肤、仅有设计稿或仅通过单元测试，都不算 M3 完成。

## 2. 文档与设计系统

- [x] M3 canonical 文档落在 `docs/MVP3/`。
- [x] 产品目标、范围、非目标和架构边界已定义。
- [x] 应用壳、页面、组件、状态、响应式和可访问性规范已定义。
- [x] 项目所有者已确认个人自用、非商业用途，可采用 `animal-island-ui`。
- [x] `animal-island-ui` 通过本地 UI 适配层接入，页面未与其 API 深度耦合。
- [x] 原作者 attribution、CC BY-NC 4.0 和非商业用途声明已进入 M3 许可证文档。
- [x] 第三方许可证声明已纳入应用 About 区域与最终分发检查。
- [x] 语义 design tokens 已落地，页面不再散写主要颜色、圆角和阴影。
- [x] 基础组件具备一致的 default / hover / focus / active / disabled / loading / error 状态。
- [x] 图标来源、许可证和打包策略已记录。

## 3. 页面迁移

- [x] 首次配置完成 M3 迁移。
- [x] 全局导航和应用壳完成 M3 迁移。
- [x] 工作台完成 M3 迁移。
- [x] 新建导入与分析进度完成 M3 迁移。
- [x] 审核工作台完成 M3 迁移。
- [x] 导入计划完成 M3 迁移。
- [x] 入库执行与结果完成 M3 迁移。
- [x] Recovery 完成 M3 迁移。
- [x] 设置与技术诊断完成 M3 迁移。
- [x] 不存在仍使用旧视觉词汇的生产页面或孤立组件。

## 4. 工作流与状态

- [x] Dashboard 主 CTA 对所有后端 `next_action` 映射正确。
- [x] 前端没有根据计数或零散字段重建状态机。
- [x] 普通开始创建新 run；resume 只对显式 run id 生效。
- [x] abandoned run 只作为历史证据展示，不重新进入当前工作流。
- [x] pending、analyzing、analyzed、review_required、failed 的图集状态区分清楚。
- [x] 审核左右布局或排序变化不改变最终 selected image 语义。
- [x] 生成/冻结计划与执行 Commit 是两个清楚的步骤。
- [x] Commit 展示与读取的 frozen plan 一致，不临场重算。
- [x] recover、resume_commit、inspect_transaction_failure 入口语义准确。
- [x] conflict、证据不完整和终态失败保持 fail closed。

## 5. 视觉与交互

- [x] 图片在导入、审核和计划页面是主要视觉内容。
- [x] 每页只有一个清晰的同等级主 CTA。
- [x] 状态不只依赖颜色，均有文字或图标。
- [x] 长路径、中文、空格、括号、UUID 和 hash 不破坏布局。
- [x] 空、加载、慢加载、成功、警告、错误、取消和恢复状态均有设计与实现。
- [x] 加载 skeleton 与实际布局匹配，按钮 loading 不发生宽度跳动。
- [x] 主要交互动效为 150–220ms，且服务于状态反馈。
- [x] `prefers-reduced-motion` 下无必要位移或循环动画。
- [x] 没有玻璃拟态、渐变文字、嵌套卡片海洋或无意义大面积装饰。

## 6. 响应式与可访问性

- [x] 1440×900、1280×720、960×720 和最小支持窗口无关键操作裁切。
- [ ] Windows 100%、125%、150% 缩放通过。
- [x] 审核页面在窄窗口下可完成全部决策。
- [x] 键盘可以完成导航、目录选择后的页面操作、审核、计划和确认流程。
- [x] focus-visible 清楚且不被裁切。
- [x] 对话框焦点锁定、Escape、关闭后焦点归还正确。
- [x] 正文、标签、按钮和状态颜色达到 WCAG 2.2 AA 对比度要求。
- [x] 图标按钮有 accessible name；图片具有与决策角色匹配的 alt。
- [x] 高频进度更新不会持续打断屏幕阅读器。

## 7. 性能

- [x] 1,000 个图集或 10,000 个候选的页面策略已验证，不一次渲染全部重内容。
- [x] 长列表滚动、展开图集和切换候选无明显主线程阻塞。
- [x] 图片使用合适尺寸的预览，不默认解码原图填充缩略图。
- [x] React Query 轮询不会导致整页无意义重渲染或焦点丢失。
- [x] 页面切换和主要交互的可感知响应满足桌面工具使用要求，并记录测量方式。

## 8. 自动验证

- [x] `pnpm format:check`
- [x] `pnpm typecheck`
- [x] `pnpm test:unit`
- [x] `pnpm --filter @imagedb/desktop build:web`
- [x] 若修改 Rust 契约：`pnpm rust:test`
- [x] 若修改 Rust 契约：`pnpm rust:clippy`
- [x] 若修改真实数据库/文件事务契约：`pnpm rust:test:real`

## 9. 真实运行验收

- [x] Tauri 桌面应用从全新数据库配置开始可完成完整主链。
- [x] 使用真实源目录、真实图片和真实 PostgreSQL 运行。
- [x] 分析中取消并显式 resume 通过。
- [x] failed 图集单独 retry 通过。
- [x] 审核完成后的 Dashboard 下一步正确进入计划生成。
- [x] frozen plan 入库结果与 UI 摘要一致。
- [x] Commit 中断后恢复或人工处置入口正确。
- [x] 成功确认前源图集保持完整。
- [x] 发布、数据库提交、源归档完成后的文件系统与数据库一致。

完整命令、数量、性能指标、真实事务覆盖和 Tauri WebView2 运行证据见 [`VALIDATION.md`](VALIDATION.md)。

## 10. 已知非门禁项

以下内容不属于 M3 完成条件，除非后续明确扩展范围：

- 移动端正式适配。
- 深色主题。
- 新图库浏览/搜索页面。
- 新标签、收藏、账户或云同步。
- 新匹配算法或图集级 Commit。
- `animal-island-ui` 之外的新业务功能。
