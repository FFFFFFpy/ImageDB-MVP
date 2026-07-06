# MVP2 Acceptance

## MVP2.1 图集级断点续跑

- [x] 基础字段和状态已实现。
- [x] `resume_import_run(import_run_id)` 后端 IPC 已实现。
- [x] ScanPage 继续分析按钮调用 `resumeImportRun(importRunId)`。
- [x] ScanPage 不再无脑展示最新 run 的旧图集表。
- [x] retry failed album 只重置该图集，不影响其他图集。
- [ ] 手工中断 / 重开 / 续跑实测通过。
- [ ] 已完成图集不重跑实测通过。

## MVP2.2 异步审核入口

- [x] Review 可从已有 candidates 进入。
- [x] 审核后 album / dashboard counters 会刷新。
- [x] `skip_review_album` 后 album summary 会刷新。
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

## 验证

- [ ] pnpm format:check
- [x] pnpm typecheck
- [x] pnpm test:unit
- [x] pnpm rust:test
- [x] pnpm rust:clippy
- [x] pnpm rust:test:real
