# Release Definition of Done

当前口径：MVP 主链本地人工验收通过；发布级验收仍需 clean Windows
release gate / install gate 签字。以下只勾选已经真实完成且可证实的项。

- [x] GUI公开主链完整通过（本地人工验收：全新开始 → 托管本地 PostgreSQL → 导入 / 分析 → 审核 → 生成 / 冻结导入计划 → 提交入库 → 本地目录正式入库）。
- [ ] 干净Windows安装通过。
- [x] 托管数据库无预装环境通过（真实 packaged runtime bootstrap 测试覆盖）。
- [x] 外部PostgreSQL严格TLS与迁移通过。
- [x] 真实共享存储断连恢复通过。
- [ ] 固定验收数据集结果一致。
- [x] 全部故障注入通过。
- [ ] 大图库性能达到门禁。
- [ ] 24小时稳定性通过。
- [ ] 备份、恢复、升级和卸载行为通过。
- [ ] 诊断包脱敏通过。
- [x] 文档完整（当前状态已修正为本地验收通过、发布签字待 clean Windows gate）。
- [x] 单项测试、Clippy、Release 构建与本地 install-gate 已通过（本次 `format:check` / `typecheck` / `test:unit` / `rust:test` / `rust:clippy` / `rust:test:real` / `build` / `release:verify-artifacts` / `release:install-gate` 均通过）。
- [ ] 完整 clean Windows `pnpm release:gate` 发布签字通过。
