# M3 视觉基线与截图夹具

## 固定视口

M3 的视觉回归使用以下 CSS viewport；截图统一使用设备像素比 1，浏览器缩放 100%，关闭动画并等待字体加载完成：

| 名称               | 宽 × 高    | 用途                         |
| ------------------ | ---------- | ---------------------------- |
| `desktop-wide`     | 1440 × 900 | 标准桌面与审核双图布局       |
| `desktop-compact`  | 1280 × 720 | 常见笔记本窗口与底部操作区   |
| `sidebar-compact`  | 960 × 720  | 紧凑侧栏、折叠详情与表格降级 |
| `minimum-fallback` | 720 × 720  | 最小支持窗口兜底             |

Windows Tauri 兼容性抽样可覆盖系统缩放 100%、125% 和 150%；浏览器截图不能替代真实系统缩放。100% / 150% 的人工切换不属于本轮 M3.8 审查修复完成门禁，也不作为阻塞项。

## 确定性场景

截图和交互测试只使用下列固定场景名。动态时间固定为 `2026-07-13 10:32:18`，路径固定为包含中文、空格和括号的测试路径，UUID 使用静态 fixture 值。

| Fixture                | 页面事实                                                       |
| ---------------------- | -------------------------------------------------------------- |
| `dashboard-empty`      | 数据库已连接，无图库、无 actionable run，下一步为 `new_import` |
| `dashboard-review`     | 6 图集、808 图片、2 个待审核，下一步为 `review`                |
| `dashboard-recovery`   | 存在可恢复事务，下一步为 `recover`                             |
| `scan-discovered`      | 已发现 6 个图集，尚未分析                                      |
| `scan-running`         | 4 已分析、1 分析中、1 待处理                                   |
| `scan-failed`          | 1 个 failed 图集，错误路径为中文长路径                         |
| `review-candidate`     | 第 2 / 34 个候选，左右图片与 metadata 完整                     |
| `review-preview-error` | 右图预览失败，但决策上下文仍可读                               |
| `plan-editable`        | 计划尚未冻结，626 保留、182 排除                               |
| `plan-frozen`          | frozen plan 只读，plan hash 放入诊断详情                       |
| `commit-running`       | staging / 校验 / 发布 / DB / 归档阶段进度                      |
| `recovery-conflict`    | 证据冲突，fail closed，无自动修复按钮                          |
| `onboarding-managed`   | 推荐托管本地 PostgreSQL                                        |
| `settings-diagnostics` | 数据库、图库、行为和诊断四个分区                               |

页面测试中的 mock 数据是这些场景的行为事实来源。M3.2–M3.6 迁移页面时，应把对应 fixture 汇总进统一视觉夹具入口；M3.7 将使用同一组场景生成最终截图证据。

开发服务器使用 `?m3-fixture=foundation` 打开 M3.1 基础组件夹具；该入口只在 `import.meta.env.DEV` 为真时生效，不进入生产业务导航。

M3.1 已在 1440×900 浏览器视口检查组件层级、中文文案、键盘 Tab 焦点与控制台错误；截图证据写入忽略版本控制的 `output/playwright/`。主 CTA 白字与绿色背景对比度为 5.27:1，其余基础状态文字对比度均大于 5.8:1。

M3.2 使用 `?m3-fixture=dashboard` 打开确定性工作台夹具。夹具固定包含长中文路径、分析中任务、待审核与失败徽标、图库统计和健康状态；已在 1440×900、960×720、720×720 检查完整侧栏、紧凑图标侧栏、主 CTA 可见性和页面纵向滚动，浏览器控制台无错误。

M3.3 使用 `?m3-fixture=scan` 打开分析中夹具，固定包含长中文源路径、438/808 图片进度、六种图集行和失败错误文案。1440×900 与 960×720 下目录区、取消入口、进度和图集表可用，窄视口保留纵向滚动且控制台无错误。

M3.4 使用 `?m3-fixture=review` 打开确定性审核夹具，固定包含同一图集内的两张近似图片、中文长路径、匹配元数据与 32 个剩余候选。已在 1440×900、960×720、720×720 检查双图、窄窗口纵向降级和决策区；实际切换叠加模式并展开详情，浏览器控制台无错误。`REVIEW_DECISION_OPTIONS` 单元测试固定左图 `keep_source`、右图 `keep_candidate` 与全部保留 `keep_all` 的业务语义。

M3.5 使用 `?m3-fixture=plan` 检查 6 图集、808 图片、626 保留与 182 排除的 frozen plan，并使用 `?m3-fixture=commit&m3-state=confirm|running|success|recovery` 分别检查提交确认、六阶段事务进度、成功和 fail-closed 恢复结果。计划与提交确认在 1440×900、960×720、720×720 检查，执行与异常结果在 1440×900 检查，浏览器控制台无错误。展开图集的图片行使用固定 24 张批次，避免 626 张夹具一次挂载并请求全部预览。

M3.6 使用 `?m3-fixture=recovery|settings|onboarding|probes` 检查事务处置、数据库与存储设置、首次配置和高级诊断。Recovery 固定包含可安全恢复、证据冲突、终态 failed 三类事务，在 1440×900、960×720、720×720 检查且不提供覆盖入口；设置在 1440×900、960×720 检查长路径与外部数据库表单，About 区域可见非商业 attribution；首次配置实际切换托管模式，技术探针三类标签均可访问。上述浏览器控制台无错误。

## 旧界面基线

M3 启动时的生产界面具有以下可复核特征：

- 220px 深蓝侧栏，激活项为高饱和蓝色。
- 主内容背景 `#f4f6f8`，大多数信息使用同尺寸白色卡片网格。
- Dashboard 顶部并列数据库、pgvector、migration 卡片，下一步动作位于页面下方。
- 页面组件大量直接使用 `.btn-primary`、`.btn-secondary`、`.status-*` 与散写颜色。
- Review 已有双图、overlay、缩放和键盘决策能力，但图片与 metadata 的视觉优先级不足。

这份文字基线与 M3 启动参考母版共同作为重设计前证据。最终验收关注信息架构、状态与操作是否改善，不把动态图片内容纳入像素级比较。

## 组件库兼容性基线

- 锁定版本：`animal-island-ui@1.2.2`。
- Peer range：React / ReactDOM `>=17.0.0`，覆盖当前 React 19。
- TypeScript：`pnpm typecheck` 通过。
- Vitest：依赖需经 Vite inline transform，包内 CSS Module 才能在 jsdom 测试加载。
- Vite 6：生产构建通过，字体与 SVG/WebP 资源离线输出。
- 当前资源成本：组件 CSS 约 125 KB；三份简体中文字库合计约 3.47 MB；其余拉丁字体和装饰资源约 0.2 MB。
- Tauri 2：release 可执行文件与 NSIS 安装包构建、产物校验通过；隔离用户目录下进程保持响应，WebView2 CDP 确认 `tauri.localhost`、Tauri IPC 与设置导航可用。

## M3.7 最终取证

- 补查 1280×720 审核与 720×720 计划页面，关键 CTA、决策区和纵向滚动可用，控制台零错误。
- 960×720 在 125% 与 150% device scale factor 下无横向溢出，三个核心审核决策按钮均可达；它们只作为既有辅助证据，不等同于真实系统缩放，也不构成本轮完成门禁。
- `prefers-reduced-motion: reduce` 下等待 300ms 后运行中动画数为 0。
- 图片预览对话框具备 `role="dialog"`、`aria-modal`、焦点锁定、Escape 关闭和焦点归还；单测与真实浏览器交互均通过。
- 1,000 图集 / 10,000 图片压力夹具首屏仅挂载 50 图集、402 个 DOM 节点；加载下一批后为 652 个，展开 10 张图片后为 723 个，未出现横向溢出或控制台错误。
- axe-core 4.12.1 对 14 个固定页面/状态执行 WCAG 2 A/AA、2.1 AA、2.2 AA 扫描，全部为 0 violation。
