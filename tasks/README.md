# 任务顺序

按编号顺序执行。`CURRENT_TASK.md` 指向唯一当前任务。

当前任务序列：

| 任务 | 文件 |
| ---- | ---- |
| 0 | `00-technical-probe.md` |
| 1 | `01-app-and-database.md` |
| 2 | `02-scan-and-exact-match.md` |
| 3 | `03-perceptual-match.md` |
| 4 | `04-review-gui.md` |
| 5 | `05-import-loop.md` |
| 6 | `06-recovery.md` |
| 6.5 | `06.5-managed-postgres-runtime.md` |
| 7 | `07-external-postgres.md` |
| 8 | `08-mounted-storage.md` |
| 9 | `09-release-closure.md` |

每个任务完成后：

1. 运行该任务列出的全部验收命令。
2. 记录真实运行结果。
3. 单独提交。
4. 更新 `CURRENT_TASK.md`。
