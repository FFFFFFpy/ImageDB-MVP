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
- [x] 0013 对反向候选按最终选择归一化，冲突与非法 selected_image_id 原子失败。
- [x] 0014 校验剩余审核结构并约束后续 review decision 写入。
- [ ] 手工审核后 Dashboard / Scan / Review 计数同步复验通过。

## 持久化多图审核组

- [x] duplicate candidate 图按连通分量生成稳定审核组，跨图集与图库成员可处于同一组。
- [x] 人工组默认所有成员 keep；图库成员只读且不可 exclude。
- [x] 已提交的 resolved 审核组全部成员决定只读，不允许只改变 React 本地状态。
- [x] 提交时必须携带组内全部导入成员，并保证至少保留一张图片。
- [x] 自动组保留图库成员或一个稳定导入代表。
- [x] frozen plan 只读取成员最终动作，不再由 pair decision 临场推导。
- [x] 未物化审核组的旧未完成任务明确要求重新分析，不做危险回填。

## 移动选中源文件（无备份）

- [x] 默认保持复制/归档模式，危险模式必须在计划页显式勾选。
- [x] source mode 进入 plan hash、manifest、file transaction、Recovery、结果与日志。
- [x] 发布文件、manifest、operation journal 和数据库记录复验完成后才允许源文件清理。
- [x] 只为 frozen plan 图片预写 cleanup operation，并逐文件校验路径、大小和 BLAKE3。
- [x] 原路径先原子 rename 到持久化的同目录唯一隔离路径；大小、BLAKE3 与删除均作用于隔离文件，隔离后原路径新建文件不受影响。
- [x] 内容、大小、BLAKE3、路径逃逸、清理集合不一致和已删除隔离文件重新出现保持永久 conflict；删除、目录同步、权限、文件占用和存储离线等 I/O 错误保持 `source_files_removing` 并允许重试。
- [x] 源图集目录、sidecar、嵌套文件和排除图片不删除。
- [x] 正常完成、数据库提交后中断恢复、重复恢复幂等、源内容变化零删除冲突，以及部分删除后 PermissionDenied 重试与 TOCTOU 原路径替换保护均由真实 PostgreSQL / 文件系统测试覆盖。

## MVP2.3 Database Info Dashboard

- [x] 后端 `get_database_info_dashboard` IPC 已实现。
- [x] Dashboard 展示数据库内大致情况。
- [x] 展示图库根目录、已入库图集、已入库图片、导入任务、待审核、失败图集、需要恢复、冻结计划。
- [ ] 空库 / 有数据 / 异常状态手工测试覆盖。
- [x] abandoned 历史证据保留，但不计入当前待审核、失败或恢复统计。
- [x] Dashboard 下一步仅基于同一个非 abandoned `latest_actionable_run`。
- [x] Review 默认选择与 ScanPage 主流程操作均排除 abandoned run。
- [x] `review_required` 的待审核归零后，Dashboard 进入入库审核 / 计划生成，不会开始新导入。
- [x] cancelled frozen plan、committing 和 active transaction 由后端显式 `next_action` 路由。
- [x] committing / recovery_required 在首事务预写前或图集间崩溃时回到幂等 Commit，不进入空 Recovery。
- [x] failed / cancelled 终态事务进入明确人工处置入口，不显示可执行的自动恢复按钮。
- [x] `source_files_removing` 计入 Dashboard / actionable run 的可恢复事务集合；父 run 为 `recovery_required` 时 `next_action = recover`。

## 保持不变

- [x] Commit 仍保持 run 级 frozen plan。
- [x] Commit 不临场重算导入计划。
- [x] Recovery / file transaction schema 未被重写。
- [x] Scan 未结束或有失败图集时不能冻结部分计划。
- [x] File transaction 与全部 operations 原子预写；Recovery 拒绝不完整预写证据。
- [x] 移动模式的 selected-source cleanup operations 与 file transaction 原子预写。

## 验证

- [x] pnpm format:check
- [x] pnpm typecheck
- [x] pnpm test:unit
- [x] pnpm rust:test
- [x] pnpm rust:clippy
- [x] pnpm rust:test:real
- [x] pnpm build
- [x] pnpm release:verify-artifacts
- [ ] pnpm release:install-gate（本轮未运行）
- [x] pnpm release:dataset
- [x] pnpm release:performance

2026-07-19 审查修复自动验证：`pnpm check` 通过（15 个前端测试文件、116 项测试；Rust 默认 246 项通过、4 项真实/大内存用例按设计忽略，并包含全特性 Clippy）；`real_migration_0012_` 3 项、`real_scan_` 4 项和完整 `fail_injection_` 30 项真实 PostgreSQL / 文件系统测试通过。新增回归覆盖 `source_files_removing + recovery_required → Dashboard recover`，以及第二张隔离文件模拟 PermissionDenied 后 Commit 与 Recovery 均保持可重试、再次恢复完成、原路径新文件和清单外文件不受影响；另覆盖 0016 旧 `verifying/removing` 清理行升级到 0017 时的安全恢复。`pnpm --filter @imagedb/desktop build:web` 通过。本轮没有重新运行包含大图内存门禁等全部 suite 的 `pnpm rust:test:real`，使用上述相关真实套件作为审查修复门禁。

2026-07-18 多图审核与移动入库任务包自动验证：`pnpm check` 通过（15 个前端测试文件、115 项测试；Rust 244 项通过、4 项真实/大内存用例按设计忽略）；`pnpm rust:test:real` 的 23 组、109 项真实 PostgreSQL / 文件系统 / 故障注入测试全部通过，耗时 567.4 秒，其中包含移动模式正常完成与幂等、数据库提交后中断恢复、源内容变化时零删除并进入 conflict。`pnpm --filter @imagedb/desktop build:web` 与 `pnpm build` 通过，生成 NSIS 安装包 `ImageDB_0.1.0_x64-setup.exe`。

2026-07-12 自动验证结果：51 项前端测试通过；Rust 默认测试 212 项通过、3 项真实测试按设计 ignored；`rust:test:real` 的 22 组、99 项真实 PostgreSQL / 文件系统 / 故障注入测试全部通过。Dashboard 真实库测试覆盖 abandoned 历史隔离、审核完成后生成计划、待审核路由、cancelled frozen plan 续提交、committing active transaction 恢复、首事务预写前续提交、source_archived 图集后的缺失事务续提交，以及 failed/cancelled 终态事务人工处置。`format:check`、`typecheck`、`rust:clippy`、生产构建和 release artifacts 验证通过，生成的应用迁移 head 为 `0014_candidate_review_semantics_and_abandoned_filters`。验收数据集为 8 个源图集、44 个源文件和 1 张历史图；120 图性能门禁总耗时 6.999 秒，scan 67.49 images/s，commit 118.81 images/s。

本轮未运行 `release:install-gate`，也未执行仍未勾选的手工中断/重开/续跑、交互计数同步和空库/异常状态人工测试；自动测试不替代这些人工验收，也不替代 clean Windows 默认数据目录保留签字。
