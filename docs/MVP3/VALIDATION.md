# M3 验证记录（按执行批次）

验证日期：2026-07-14

分支：`codex/mvp3-ui-redesign`

## 当前结论

M3.0–M3.6 与用户授权扩展 M3.8 已落地。Dashboard 继续只消费后端 `next_action`，Commit 继续读取 frozen plan。初始 UI 重设计未改后端；静态审查随后发现“用户确认的计划”和“真正提交的计划”之间缺少哈希契约，因此以独立契约修复补充 Rust command/service/repository 的 `plan_hash` 返回与行锁内校验。M3.8 进一步增加文件事务开始前撤销整条未入库导入工作流，以及已提交图库的只读明细查询；`abandoned` / `invalidated` 是 M3.8 唯一授权的状态语义扩展，除此之外没有修改 schema、migration、匹配算法、原工作流或文件事务发布/恢复顺序。

2026-07-13 的完成性反证审计随后补齐了 1,000 图集/10,000 图片压力、键盘与焦点、全页 WCAG、真实 Tauri UI 主链、分析中断续跑和 Commit 中断恢复证据。当前机器通过 Win32 `GetDpiForSystem` 测得 120 DPI，即真实 Windows 125% 缩放；100% 与 150% 仍不能由 WebView device scale factor 代替，但不属于本轮 M3.8 审查修复完成门禁，也不再作为阻塞项。

## 本轮审查修复验证

2026-07-14 在本分支实际执行以下门禁；这些结果只对应本轮审查修复，不借用下方历史批次：

| 命令                                       | 本轮实际结果                                               |
| ------------------------------------------ | ---------------------------------------------------------- |
| `pnpm format:check`                        | 通过，Prettier 与 `cargo fmt --check` 均无差异             |
| `pnpm typecheck`                           | 通过，`tsc --noEmit` 无错误                                |
| `pnpm test:unit`                           | 14 个文件、130 项通过，0 失败                              |
| `pnpm --filter @imagedb/desktop build:web` | 通过，231 modules；JS 379.69 KB，CSS 174.03 KB             |
| `pnpm rust:test`                           | 226 通过、0 失败、3 个 real test 按设计忽略                |
| `pnpm rust:clippy`                         | 通过，`--all-targets --all-features -D warnings`           |
| `pnpm rust:test:real`                      | 102 项真实 PostgreSQL / 文件系统测试通过，0 失败，573.8 秒 |
| `pnpm check`                               | 通过，完整串行复验类型、单测、格式、Clippy 与 Rust 单测    |

真实浏览器复验使用本地 Vite Review fixture 和浏览器原生滚轮事件：左图、右图与叠加模式均围绕各自指针位置缩放，滚轮时页面 `window.scrollY` 与 `.app-main.scrollTop` 保持不变；图片区域外滚轮仍可滚动页面。控制台无运行错误，验证结束后已关闭页面并停止临时 Vite 服务。

本轮没有新增 schema migration；图库明细继续使用现有索引与不透明 keyset cursor。未重新执行 release exe / NSIS 安装包构建，也未切换 Windows 100% / 150% 系统缩放；两项均不作为本轮审查修复完成门禁，且不沿用历史产物宣称已复验。

## 历史自动门禁（此前执行批次）

| 命令                                       | 结果                                                     |
| ------------------------------------------ | -------------------------------------------------------- |
| `pnpm format:check`                        | 通过                                                     |
| `pnpm typecheck`                           | 通过                                                     |
| `pnpm test:unit`                           | 13 个文件、100 项通过                                    |
| `pnpm --filter @imagedb/desktop build:web` | 231 modules，JS 373.17 KB，CSS 173.13 KB                 |
| `pnpm rust:test`                           | 214 通过、0 失败、3 个 real test 按设计忽略              |
| `pnpm rust:clippy`                         | 通过，warnings 作为错误处理                              |
| `pnpm rust:test:real`                      | 102 项真实 PostgreSQL/文件系统测试通过，0 失败，580.1 秒 |
| `pnpm release:performance`                 | 通过，120 张真实图片完整链路 7.956 秒                    |
| `pnpm --filter @imagedb/desktop build`     | release exe 与 NSIS 安装包构建通过                       |
| `pnpm release:verify-artifacts`            | exe、安装包与内置 PostgreSQL runtime 均通过              |
| `CI=true pnpm release:gate`                | 通过，含完整真实库、构建、产物和安装门禁，833.3 秒       |

性能门禁记录：托管 PostgreSQL 启动 4506ms，扫描 57.12 images/s，计划生成 74ms，Commit 98.28 images/s，空 Recovery 扫描 24ms。

## 真实数据库与文件系统

`rust:test:real` 使用打包的 PostgreSQL 18.4 runtime 和隔离数据库，覆盖：

- 全新托管数据库初始化、pgvector、partial-init recovery 和应用重启；
- 真实图片扫描、源快照、审核持久化、frozen plan 幂等和摘要一致性；
- 完整 staging → verify → publish → DB commit → source archive 主链；
- 分析 resume/retry、Commit 取消、公共 Recovery 命令和连续恢复收敛；
- 25 个故障注入点、manifest 篡改、未知目标目录、路径逃逸和源文件保护；
- 外部 PostgreSQL 初始化、升级、超时、不可达回退和数据迁移。

所有套件均为真实 PostgreSQL 与真实临时文件系统，不以 mock 代替事务结果。

## 静态审查修复复验

2026-07-13 在提交 `1b15e5e` 完成审查修复，并以新增回归锁定以下行为：

- 两个双任务场景证明 Dashboard 显式选择的较旧 run 不会被 Review / Commit 各自查询到的较新 run 覆盖；Scan 前往审核同样携带当前 active run；
- 计划图片或图集编辑期间，返回审核、前往提交和侧栏导航全部禁用；编辑完成后同步更新两组 frozen-plan query，并失效 Dashboard 状态；
- Commit 确认页显示 `plan_hash`，IPC 传递 `expectedPlanHash`，后端在计划编辑共用的 run 行锁内比对，哈希变化即 fail closed；
- 移除跨源图集图片拖拽和对应公开 IPC，只保留“导入 / 跳过”；
- 审核加载、查询错误、冻结失败分别呈现 loading / error / empty；
- 设置页在表单可编辑和可保存前回填图库根目录及全部非秘密外部数据库字段，并区分主动清空；
- 审核滚轮缩放按鼠标指向位置修正偏移；全局快捷键避开 `select`、contenteditable、预览 modal、组合键和输入法 composing；取消扫描不再显示成功色。

本轮实际执行：

- `pnpm typecheck`：通过；
- `pnpm test:unit`：12 个文件、93 项通过；
- `pnpm format:check`：通过；
- `pnpm rust:clippy`：通过，warnings 作为错误；
- `pnpm rust:test`：213 通过、0 失败、3 个 real test 按设计忽略；
- `pnpm rust:test:real`：完整真实 PostgreSQL / 文件系统套件通过；
- `pnpm --filter @imagedb/desktop build:web`：通过；
- `CI=true pnpm release:gate`：通过，含 release/NSIS、产物校验和静默安装/卸载。

首次直接执行 `pnpm release:gate` 时，`pnpm install` 因无 TTY 拒绝清理 `node_modules`；按 pnpm 提示设置 `CI=true` 后总门禁通过。一次 10 分钟外层时限不足，最终以 20 分钟时限完成，实际耗时 705.7 秒。这两次均为执行环境/时限问题，不是测试断言失败。

## M3.8 安全撤销工作流与图库明细复验

2026-07-14 在同一 feature 分支完成两个用户明确授权的受限扩展及“整条工作流撤销”语义复验：

- Review 与 Commit 均提供二次确认的“撤销这次导入”；成功后 import run 标记为 `abandoned`、frozen plan 标记为 `invalidated`，审核决定、源快照与计划历史保留；前端清空 workflow run 上下文并失效 frozen-plan、Dashboard、latest-reviewable 与 latest-committable 缓存；
- 撤销与 Commit capture 共用 import run `FOR UPDATE` 锁。真实 PostgreSQL 用例证明零文件事务时整条工作流可撤销并从 actionable 查询中消失；前端双任务与单任务回归证明显式 run ID 不串线，单任务撤销后 Dashboard 回到“开始导入”；
- 只要存在任何 `file_transactions` 证据即拒绝撤销，原 import run 与 frozen plan 保持可提交状态并继续走 Commit / Recovery；
- 工作台图库概览进入只读“图库明细”，command → service → repository 只返回 `committed` 图集与图片；图集每批 50 个、展开图片每批 24 张，汇总与分页总数由真实 PostgreSQL 用例校验；
- 前端回归覆盖 Review 撤销、Commit 撤销、工作台回到新建导入、缓存失效、图库图集/图片增量分页、加载错误不伪装为空、真实空图库和工作台路由；
- `?m3-fixture=library` 在默认桌面视口完成截图检查；720px 视口的 DOM 边界测量无横向溢出；`?m3-fixture=plan` 与 `?m3-fixture=commit` 实测确认撤销后果、确认动作和离开入口状态，控制台 0 warning / error。

初始 M3.8 扩展执行过 `CI=true pnpm release:gate`，完整结果见上表。2026-07-14 将撤销语义修正为整条工作流后，重新执行 `pnpm check`，结果为 100 项前端测试、214 项 Rust 测试、格式、类型与 Clippy 全部通过；随后重新执行完整 `pnpm rust:test:real`，当前 23 个过滤套件合计 102 项真实测试全部通过，耗时 580.1 秒；`pnpm --filter @imagedb/desktop build:web` 亦通过。本次语义修正未重复构建 release/NSIS 安装包。

## 视觉、响应式与可访问性

确定性 fixture 已覆盖 1440×900、1280×720、960×720、720×720。最终补查确认：

- Windows 当前系统 DPI 为 120（125%）；100% / 150% device scale factor 只作为辅助证据，不计作系统缩放签字；
- 窄窗口审核可通过按钮和 `1`–`4` 快捷键完成决策，快捷键 `1` 的真实决策参数由测试锁定；
- Tauri WebView2 的 Tab 序列逐页覆盖工作台 9、新建导入 11、审核 8、入库 9、恢复 9、设置 30 个可交互控件，无焦点陷阱；
- semantic tokens 的主 CTA 对比度 5.27:1，其余基础文字状态大于 5.8:1；
- axe-core 4.12.1 按 WCAG 2 A/AA、2.1 AA、2.2 AA 规则扫描 14 个页面/状态 fixture，全部为 0 violation；
- 状态同时使用文字/图标，不只依赖颜色；长中文路径、空格、括号、UUID 与 hash 可换行；
- reduced-motion 下无运行中的循环动画；
- 图片预览 modal 支持焦点锁定、Escape、关闭后焦点归还和 accessible image alt；
- Dashboard 轮询更新测试确认主 CTA 焦点不会丢失；进度条不使用会持续打断读屏的 live region。

## 长列表与响应

- frozen plan 图集每批最多挂载 50 个；展开图集图片每批最多挂载 24 张，缩略图 `loading="lazy"`；
- 图库明细每批最多查询并挂载 50 个图集；仅在展开时查询图片，每批最多 24 张；
- 实际压力 fixture 固定生成 1,000 图集 / 10,000 图片；首屏只挂载 50 图集、402 个 DOM 节点，开发构建导航约 707.7ms，JS heap 约 21.5MB；
- 加载下一批 50 图集约 55.9ms，DOM 为 652 个；展开首图集 10 张图片约 93.1ms，DOM 为 723 个；各阶段均无横向溢出、控制台错误或一次性挂载全部重内容；
- 审核只挂载当前候选的左右预览，不渲染 10,000 个候选重内容；
- 性能门禁的真实 120 图扫描、计划与 Commit 均低于仓库阈值。

## Tauri release 运行

- release executable 使用隔离应用数据目录、真实托管 PostgreSQL 与真实文件目录运行；WebView2 调试目标为 `http://tauri.localhost/`；
- 从首次配置初始化 PostgreSQL 18.4 / pgvector / migration 0014，保存真实图库根目录，扫描 8 个图集 40 张图片，审核后冻结 39 张计划并完成 Commit；
- 真实扫描 3,000 张图片时在约 216ms 点击取消；随后在 100 张 checkpoint 处强制终止应用，重启后 Dashboard 显示“继续分析图片”，续跑推进至 750 张后再次取消；
- 第二条真实链路扫描并冻结 20 图集 / 20 张 / 240.1MB 计划；Commit 提交 6 图集后强制终止，重启后 Dashboard 进入 Recovery，诊断为 `staging incomplete`；
- Recovery 在旧租约未过期时明确拒绝覆盖；租约过期后按 `staging → verified → source_archived` 收敛，再提交同一 frozen plan 得到 14 个新提交、6 个幂等跳过、0 失败；
- 最终图库包含该链路 20 张图片，20 个源图集全部进入 `.imagedb-processed`，原源目录无剩余图集；
- 实测发现并修复两个呈现层缺陷：零审核候选时 frozen plan 被空状态遮挡，以及 Recovery 完成后导航计数/工作台缓存未同步；均有回归测试；
- NSIS 产物：`ImageDB_0.1.0_x64-setup.exe`。

## 非 M3 门禁边界

- clean Windows 全量安装发布门禁和正式发布仍属于 MVP1 release gate，不纳入 M3 UI 实现结论。
- 浏览器视觉截图位于忽略版本控制的 `output/playwright/`，不进入分发产物。
- Windows 100% / 150% 系统缩放可作为后续兼容性抽样，但不属于本轮 M3.8 审查修复的完成门禁，也不列为 M3 阻塞项。
