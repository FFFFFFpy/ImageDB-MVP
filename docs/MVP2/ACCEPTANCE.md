# MVP2 Acceptance

## MVP2.1 图集级断点续跑

- [x] 基础字段和状态已实现。
- [x] `resume_import_run(import_run_id)` 后端 IPC 已实现。
- [x] ScanPage 继续分析按钮调用 `resumeImportRun(importRunId)`。
- [x] ScanPage 不再无脑展示最新 run 的旧图集表。
- [x] retry failed album 只重置该图集，不影响其他图集。
- [x] 普通开始始终创建新 run；只有显式 resume 才复用旧 snapshot。
- [x] 可显式 abandoned 旧 checkpoint 并为同源目录重新分析。
- [x] 取消后的 run 状态由 album / review 持久化事实归并。
- [ ] 手工中断 / 重开 / 续跑实测通过。
- [ ] 已完成图集不重跑实测通过。

## MVP2.2 异步审核入口

- [x] Review 可从已有 candidates 进入。
- [x] 审核后 album / dashboard counters 会刷新。
- [x] `skip_review_album` 后 album summary 会刷新。
- [x] `skip_review_album` decision 与 summary 在同一事务内提交或回滚。
- [x] 同一图片对候选唯一，exact 优先且不会再生成 perceptual 平行候选。
- [x] 感知 bucket 超过 50 条时稳定完整召回。
- [ ] 手工审核后 Dashboard / Scan / Review 计数同步复验通过。

## MVP2.3 Database Info Dashboard

- [x] 后端 `get_database_info_dashboard` IPC 已实现。
- [x] Dashboard 展示数据库内大致情况。
- [x] 展示图库根目录、已入库图集、已入库图片、导入任务、待审核、失败图集、需要恢复、冻结计划。
- [ ] 空库 / 有数据 / 异常状态手工测试覆盖。

## 保持不变

- [x] Commit 仍保持 run 级 frozen plan。
- [x] Commit 不临场重算导入计划。
- [x] Recovery / file transaction schema 未被重写。
- [x] Scan 未结束或有失败图集时不能冻结部分计划。
- [x] File transaction 与全部 operations 原子预写；Recovery 拒绝不完整预写证据。

## 验证

- [x] pnpm format:check
- [x] pnpm typecheck
- [x] pnpm test:unit
- [x] pnpm rust:test
- [x] pnpm rust:clippy
- [x] pnpm rust:test:real
- [x] pnpm build
- [x] pnpm release:verify-artifacts
- [x] pnpm release:install-gate
- [x] pnpm release:dataset
- [x] pnpm release:performance

2026-07-10 自动验证结果：42 项前端测试、209 项 Rust 默认测试通过（3 项真实测试按设计 ignored）；`rust:test:real` 的 21 组、95 项真实数据库 / 文件系统 / 故障注入测试全部通过。安装门禁覆盖 silent install、Unicode/空格路径下 initdb、严格使用安装目录内 PostgreSQL 18.4 / pgvector 0.8.3、迁移到 `0012_album_workflow_repair`、主窗口关闭触发有界停库、同版本覆盖安装、silent uninstall 和安装目录完全消失；本机已有 `%LOCALAPPDATA%/ImageDB` 用户数据，因此默认数据目录卸载哨兵按安全策略跳过，隔离 app-data 保留已验证。验收数据集为 8 个源图集、44 个源文件和 1 张历史图。120 图性能门禁总耗时 6.422 秒，scan 55.17 images/s，commit 148.88 images/s。以上自动验证不替代仍未勾选的人工交互验收项，也不替代 clean Windows 默认数据目录保留签字。
