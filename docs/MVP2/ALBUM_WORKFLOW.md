# Album Workflow

## 状态

`import_albums.state` 支持以下分析流程状态：

```text
pending
analyzing
analyzed
review_required
failed
completed
```

兼容旧阶段状态：

```text
scanning
fingerprinting
reviewed
ready_to_commit
committing
```

## 断点续跑

同一个 source root 再次开始分析时，后端会优先查找最新的可续跑 import run：

```text
analyzing / scanning / fingerprinting / cancelled / failed
```

命中后不会创建新 run，而是：

1. 把 stale `analyzing` / `scanning` / `fingerprinting` 图集恢复为 `pending`。
2. 只查询 `pending` / `failed` / stale 图集继续分析。
3. 已经 `analyzed` / `review_required` / `completed` 的图集不重跑。

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
- 与已完成同 run 图集的 exact duplicate 候选。
- 与历史图库的 exact / perceptual 候选。

因此 Review 可以在整批分析完成前看到已经落库的 review candidates。

## Retry

`retry_import_album(album_id)` 会删除该图集关联的 import images 和 duplicate candidates，并把图集恢复为：

```text
pending
```

它不修改其它图集，不触碰 frozen import plan，不进入 Commit。

## Commit 边界

MVP2 当前不做图集级入库。Commit 仍必须读取 frozen plan，继续走：

```text
staging -> 校验 -> 发布 -> 数据库提交 -> 源归档
```
