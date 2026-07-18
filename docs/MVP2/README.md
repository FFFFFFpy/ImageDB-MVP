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
- abandoned run 保留 albums、images、candidates 和 decisions 作为历史证据，但不参与当前待办统计、Review/Commit 自动选择或 Dashboard 下一步导航。
- 失败图集可单独重置为待分析。
- Dashboard / Scan / Review 显示图集状态、待审核、失败入口。
- Dashboard 显示数据库概览：图库根目录、已入库图集/图片、导入任务、待审核、失败、恢复和冻结计划数量。
- Dashboard 的主 CTA 由后端根据 run、frozen plan 和 file transaction 事实返回显式 `next_action`，前端不再拼凑工作流状态。
- Dashboard 区分可恢复事务、终态未解决事务和 frozen plan 尚未创建事务的图集；事务预写前 / 图集间崩溃回到幂等 Commit，终态失败进入人工处置。
- Commit 仍保持 run 级 frozen plan / commit 主链。
- 重复候选按连通分量持久化为审核组；人工组默认全保留，用户对组内每张导入图明确 keep / exclude，图库成员只读且始终 keep。
- frozen plan 只读取审核组成员的最终动作；旧的未完成 pair 审核任务没有自动回填，必须重新分析。
- frozen plan 可选择默认关闭的“移动选中源文件（无备份）”；该模式逐文件校验并删除选中图片，不删除源图集目录、sidecar、嵌套文件或排除图片。

当前数据库 migration head：

```text
0016_group_review_large_image_move_import
```

从 0012 升级时，0013 会先校验人工审核结构与规范化最终选择，再对方向无关的候选 pair 去重；0014 为后续写入增加审核语义约束。曾运行旧开发版 0013 的测试库无法由 0014 恢复已经删除的行，需要从 0012 fixture 或全新数据库重建后验证迁移语义。

## 文档

| 文档                                     | 用途                              |
| ---------------------------------------- | --------------------------------- |
| [`ALBUM_WORKFLOW.md`](ALBUM_WORKFLOW.md) | 图集级状态、断点续跑和 retry 行为 |
| [`ACCEPTANCE.md`](ACCEPTANCE.md)         | MVP2.1 / MVP2.2 验收项            |

0016 新增持久化审核组、源文件模式和逐文件清理操作日志，不回填旧的未完成审核任务。

## 非目标

- 不新增 SQL 控制台或表浏览器。
- 不新增图集级 Commit。
- 不改变 staging → 校验 → 发布 → 数据库提交的安全顺序；移动模式只在其后增加可恢复的逐文件清理阶段。
- 不引入 Plus 版模型网关、标注 DAG 或搜索平台。
- 不用 mock 假装真实断点续跑。
