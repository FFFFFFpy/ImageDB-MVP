# MVP2 文档入口

MVP2 目标：

```text
图集级断点续跑 + 异步审核入口 + 数据状态可见。
```

MVP2 不是 SQL 控制台、表浏览器，也不是新匹配算法或图集级 Commit。数据库仍是后台账本；Dashboard 只展示用户可理解的数据库概览、图集处理状态、下一步动作和异常入口。

## 当前范围

本阶段只覆盖 MVP2.1 + MVP2.2 基础：

- 选择源目录后发现图集列表。
- `import_albums` 成为分析阶段的持久进度单位。
- 单个图集分析完成后立即落库为 `analyzed` 或 `review_required`。
- 中断后通过显式 resume 继续 pending / stale analyzing / failed retry 图集；普通开始创建新 run。
- 可显式 abandoned 旧 checkpoint，并在源文件修复后重新分析。
- 失败图集可单独重置为待分析。
- Dashboard / Scan / Review 显示图集状态、待审核、失败入口。
- Dashboard 显示数据库概览：图库根目录、已入库图集/图片、导入任务、待审核、失败、恢复和冻结计划数量。
- Commit 仍保持 run 级 frozen plan / commit 主链。

## 文档

| 文档                                     | 用途                              |
| ---------------------------------------- | --------------------------------- |
| [`ALBUM_WORKFLOW.md`](ALBUM_WORKFLOW.md) | 图集级状态、断点续跑和 retry 行为 |
| [`ACCEPTANCE.md`](ACCEPTANCE.md)         | MVP2.1 / MVP2.2 验收项            |

## 非目标

- 不新增 SQL 控制台或表浏览器。
- 不新增图集级 Commit。
- 不重写 Commit / Recovery 文件事务链。
- 不引入 Plus 版模型网关、标注 DAG 或搜索平台。
- 不用 mock 假装真实断点续跑。
