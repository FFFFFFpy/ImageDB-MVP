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

同一个 source root 再次开始分析时，后端会优先查找可续跑 import run：

```text
analyzing / scanning / fingerprinting / cancelled / failed
```

命中后不会创建新 run，而是：

1. 将 stale `analyzing` / legacy `scanning` / legacy `fingerprinting` 图集恢复为 `pending`。
2. 清理这些 stale 图集的旧 `import_images` / `duplicate_candidates`，避免续跑重复写入。
3. 只处理 `pending` 图集。
4. 已经 `analyzed` / `review_required` 的图集不重跑。
5. `failed` 图集必须先通过 `retry_import_album(album_id)` 单独重置为 `pending`。

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

## 候选生成

图集完成后立即生成并持久化：

- 图集内部重复候选。
- 与同 run 已完成图集的 exact duplicate 候选。
- 与历史图库的 exact / perceptual 候选。

因此 Review 可以在整批分析结束前看到已经落库的 review candidates。

## Retry

`retry_import_album(album_id)` 会删除该图集关联的 import images 和 duplicate candidates，并把图集恢复为：

```text
pending
```

它不修改其它图集，不触碰 frozen import plan，不进入 Commit。

## 审核计数刷新

`submit_review_decision` 和 `skip_review_album` 会在写入 review decision 后刷新对应 album summary，并重算 run statistics。Dashboard / Scan / Review 依赖这些刷新后的计数，不需要用户刷新整个应用。

## Commit 边界

MVP2 当前不做图集级入库。Commit 仍必须读取 frozen plan，继续走：

```text
staging -> 校验 -> 发布 -> 数据库提交 -> 源归档
```
