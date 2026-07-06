# MVP2 Acceptance

## MVP2.1 图集级断点续跑

- [x] 选择源目录后可发现图集列表。
- [x] 每个图集有独立持久状态。
- [x] 分析完成一个图集后立即持久化 counters 和状态。
- [x] 同源目录重新开始分析时复用可续跑 run。
- [x] 已完成图集不会因整批中断而重跑。
- [x] 中断在途图集续跑前会清理旧 `import_images` / `duplicate_candidates`，避免重复写入。
- [x] 失败图集可通过 `retry_import_album` 单独重置，不触发整批从头开始。
- [x] `resume_import_run(import_run_id)` 会按指定 run 启动后台续跑。
- [x] Scan 页面显示图集列表和每个图集状态。

## MVP2.2 异步审核入口

- [x] `get_latest_reviewable_import_run` 会选择已有 undecided candidates 的 run。
- [x] Review 不要求整批分析完成。
- [x] Dashboard / Scan / Review 显示待分析、分析中、已分析、待审核和失败信息。
- [x] Review 页面展示待审核图集数量提示。

## 保持不变

- [x] Commit 仍保持 run 级 frozen plan。
- [x] Commit 不临场重算导入计划。
- [x] Recovery / file transaction schema 未被重写。

## 已验证

- [x] 真实 PostgreSQL `pnpm rust:test:real`。
- [x] 自动化覆盖 stale resume cleanup、失败图集 retry、dashboard 汇总和 latest reviewable run。
- [ ] 手工中断 / 重开 / retry 实战流程。
- [ ] 大图库性能验证。
