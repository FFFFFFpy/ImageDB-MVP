# Album Workflow

## 状态

`import_albums.state` 本阶段只维护以下分析流程状态：

```text
pending
analyzing
analyzed
review_required
failed
```

旧分支曾出现过的 `scanning` / `fingerprinting` / `reviewed` / `ready_to_commit` / `committing` / `completed` 不再作为正式 album workflow 状态维护。后端 stale cleanup 仍兼容清理旧的 in-flight 字符串，避免历史数据阻塞续跑。

## 断点续跑

普通“开始分析”始终创建新的 import run，不会按 source root 暗中复用旧 checkpoint。只有显式调用 `resume_import_run(import_run_id)` 才会恢复以下状态的任务：

```text
analyzing / scanning / fingerprinting / cancelled / failed
```

命中后不会创建新 run，而是：

1. 将 stale `analyzing` / legacy `scanning` / legacy `fingerprinting` 图集恢复为 `pending`。
2. 清理这些 stale 图集的旧 `import_images` / `duplicate_candidates`，避免续跑重复写入。
3. 保留首次捕获的 immutable source snapshot，并在续跑时重新校验；不会用中断后的源目录覆盖原始证据。
4. 只处理 `pending` 图集。
5. 已经 `analyzed` / `review_required` 的图集不重跑。
6. `failed` 图集必须先通过 `retry_import_album(album_id)` 单独重置为 `pending`。

用户也可以显式放弃旧任务。`abandon_import_run(import_run_id)` 将 run 标记为 `abandoned`，保留 snapshot、图片和候选作为历史证据；存在 frozen plan 或 file transaction 时 fail closed。UI 的“放弃旧 checkpoint，重新分析”随后为同一源目录创建全新 run，因此修复过的源文件不会再被旧 snapshot 永久阻挡。

如果待清理图集已经被 frozen plan 或 file transaction 引用，续跑会 fail closed，不删除任何证据。

## 单图集 checkpoint

每个图集独立执行：

```text
mark analyzing
capture source snapshot
verify snapshot
scan images
fingerprint images
detect candidates for this album
refresh image / candidate counters
mark analyzed or review_required
```

如果图集级步骤失败，该图集进入 `failed`，并记录 `last_error_code` / `last_error_message`。其它图集继续处理。

目录遍历和图片指纹在 blocking worker 中执行；指纹池全局最多并发 4 个任务，避免占用 Tokio runtime。每个图集的图片记录以单次批量 SQL 持久化。正式 fingerprint 前先完成整批图片预计数，进度使用已处理数 / 预计总数。

取消后的 run 状态由持久化事实重新归并：存在未完成图集为 `cancelled`，存在失败图集为 `failed`，存在未审核候选为 `review_required`，否则为 `ready_to_commit`。最后一个图集 checkpoint 后到达的取消信号不会再把可提交任务困在 `cancelled`。

## 候选生成

图集完成后立即生成并持久化：

- 图集内部重复候选。
- 与同 run 已完成图集的 exact duplicate 候选。
- 与历史图库的 exact / perceptual 候选。

因此 Review 可以在整批分析结束前看到已经落库的 review candidates。

同一 import/import 或 import/library 图片对在数据库中只能有一个 candidate。重复证据按 `file_exact`、`pixel_exact`、`perceptual_near`、`perceptual_similar` 的优先级合并；历史图库 exact 命中不再重复进入 perceptual 比较。感知 bucket 使用稳定顺序并完整遍历，不再用无序 `LIMIT 50` 随机丢弃候选。

## Retry

`retry_import_album(album_id)` 只接受当前状态为 `failed` 且没有 active scan、frozen plan 或 file transaction 引用的图集。它在一个数据库事务内删除该图集关联的 import images 和 duplicate candidates，并把图集恢复为：

```text
pending
```

它不修改其它图集内容，不触碰 frozen import plan，不进入 Commit。首次捕获的 source snapshot 会保留并重新校验；被删除候选所影响的其它图集只刷新 summary counters。

## 审核计数刷新

`submit_review_decision` 和 `skip_review_album` 会在写入 review decision 后刷新对应 album summary，并重算 run statistics。`skip_review_album` 使用单事务集合写入；任一 decision 或 summary 写入失败会全部回滚。Dashboard / Scan / Review 依赖这些刷新后的计数，不需要用户刷新整个应用。

## Commit 边界

MVP2 当前不做图集级入库。Commit 仍必须读取 frozen plan，继续走：

```text
staging -> 校验 -> 发布 -> 数据库提交 -> 源归档
```

整批 scan 未结束或存在 `pending` / `analyzing` / `failed` 图集时不得冻结计划；单图损坏会记录为 failed image，冻结计划只包含已成功 fingerprint 且具有 BLAKE3 的图片。Commit 开始前，file transaction 与其全部 file operations 必须在同一个 PostgreSQL 事务内完整预写；Recovery 对不完整或与 frozen plan 不一致的预写证据按 conflict fail closed。

Generate / Freeze、计划编辑和 Commit 都会锁定同一 import run 行。Freeze 的校验、plan 构建、hash、写入和 summary 在同一个数据库事务内完成；Commit 在同一锁下重新读取并校验 frozen plan，先把 run 转为 `committing`，再进入文件发布。Recovery 还会校验 transaction `plan_hash`、规范化 staging / target 路径、reparse point / junction 边界和完整 operations 集，证据不匹配时不触碰图库或源文件。
