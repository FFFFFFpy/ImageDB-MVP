# M3.7 最终验证记录

验证日期：2026-07-13

分支：`codex/mvp3-ui-redesign`

## 结论

M3.0–M3.7 已完成。UI 重设计未修改 Rust domain、service、repository、migration、匹配算法或文件事务协议；Dashboard 继续只消费后端 `next_action`，Commit 继续读取 frozen plan。

M3 验收通过不代表正式发布已经发生，也不替代 clean Windows 完整 `release:gate` / install gate 的发布签字。

## 自动门禁

| 命令                                       | 结果                                                  |
| ------------------------------------------ | ----------------------------------------------------- |
| `pnpm format:check`                        | 通过                                                  |
| `pnpm typecheck`                           | 通过                                                  |
| `pnpm test:unit`                           | 11 个文件、78 项通过                                  |
| `pnpm --filter @imagedb/desktop build:web` | 227 modules，JS 356.70 KB，CSS 169.66 KB              |
| `pnpm rust:test`                           | 212 通过、0 失败、3 个 real test 按设计忽略           |
| `pnpm rust:clippy`                         | 通过，warnings 作为错误处理                           |
| `pnpm rust:test:real`                      | 99 项真实 PostgreSQL/文件系统测试通过，0 失败，541 秒 |
| `pnpm release:performance`                 | 通过，120 张真实图片完整链路 6.388 秒                 |
| `pnpm --filter @imagedb/desktop build`     | release exe 与 NSIS 安装包构建通过                    |
| `pnpm release:verify-artifacts`            | exe、安装包与内置 PostgreSQL runtime 均通过           |

性能门禁记录：托管 PostgreSQL 启动 3346ms，扫描 58.97 images/s，计划生成 157ms，Commit 147.97 images/s，空 Recovery 扫描 19ms。

## 真实数据库与文件系统

`rust:test:real` 使用打包的 PostgreSQL 18.4 runtime 和隔离数据库，覆盖：

- 全新托管数据库初始化、pgvector、partial-init recovery 和应用重启；
- 真实图片扫描、源快照、审核持久化、frozen plan 幂等和摘要一致性；
- 完整 staging → verify → publish → DB commit → source archive 主链；
- 分析 resume/retry、Commit 取消、公共 Recovery 命令和连续恢复收敛；
- 25 个故障注入点、manifest 篡改、未知目标目录、路径逃逸和源文件保护；
- 外部 PostgreSQL 初始化、升级、超时、不可达回退和数据迁移。

所有套件均为真实 PostgreSQL 与真实临时文件系统，不以 mock 代替事务结果。

## 视觉、响应式与可访问性

确定性 fixture 已覆盖 1440×900、1280×720、960×720、720×720。最终补查确认：

- 125% / 150% device scale factor 下审核页无横向溢出，三项主要决策可达；
- 窄窗口审核可通过按钮和 `1`–`4` 快捷键完成决策；
- semantic tokens 的主 CTA 对比度 5.27:1，其余基础文字状态大于 5.8:1；
- 状态同时使用文字/图标，不只依赖颜色；长中文路径、空格、括号、UUID 与 hash 可换行；
- reduced-motion 下无运行中的循环动画；
- 图片预览 modal 支持焦点锁定、Escape、关闭后焦点归还和 accessible image alt；
- 轮询状态使用稳定查询键，进度条不使用会持续打断读屏的 live region。

## 长列表与响应

- frozen plan 图集每批最多挂载 50 个；展开图集图片每批最多挂载 24 张，缩略图 `loading="lazy"`；
- 审核只挂载当前候选的左右预览，不渲染 10,000 个候选重内容；
- 6 图集 / 808 图片 fixture 首屏为 181 个 DOM 节点，浏览器导航约 40ms；
- 性能门禁的真实 120 图扫描、计划与 Commit 均低于仓库阈值。

## Tauri release 运行

- release executable 在隔离 `LOCALAPPDATA` / `APPDATA` 下启动并保持响应 12 秒，工作集约 36.5 MB；
- WebView2 调试目标为 `http://tauri.localhost/`，`window.__TAURI_INTERNALS__` 存在；
- 在真实 Tauri WebView 中点击“设置”后进入 `#/settings`，About 中可见 `animal-island-ui` attribution；
- NSIS 产物：`ImageDB_0.1.0_x64-setup.exe`。

## 已知边界

- 本轮没有执行 clean Windows 全量安装发布门禁，也没有发布安装包；该事项仍属于 MVP1 release gate，而不是 M3 UI 实现门禁。
- 125% / 150% 使用 WebView 等效 device scale factor 自动取证，未修改宿主机系统级缩放设置。
- 浏览器视觉截图位于忽略版本控制的 `output/playwright/`，不进入分发产物。
