# MVP4 验收记录

## 功能矩阵

- [x] 完整文件与规范化像素 BLAKE3（32 bytes）。
- [x] EXIF Orientation 1–8，缺失或解析失败安全回退。
- [x] BlockHash 16×16、DoubleGradient 32×32、Triangle。
- [x] 8 种几何变换粗召回；最佳变换单次精细验证。
- [x] 图集内临时 BK-tree，无全量两两感知比较。
- [x] 正式图库缓存 BK-tree、稳定 256 候选上限、批量精细加载。
- [x] 图片与候选批量写入；哈希字段使用 `BYTEA[]`，无 hex 往返。
- [x] Commit 永久保存 V2 指纹；Commit 增量更新索引，Recovery/数据库切换使索引失效。
- [x] 审核页显示粗/细 raw distance、实际位数、综合相似度和中文变换标签。
- [x] 每图集输出指纹、召回、判定、截断和分阶段耗时指标。
- [x] 不新增算法设置 UI，不改变审核动作和文件事务状态机。

## 自动验证

完成时必须重新执行并记录最终结果：

```powershell
pnpm typecheck
pnpm test:unit
pnpm --filter @imagedb/desktop build:web
pnpm format:check
pnpm rust:clippy
pnpm rust:test
pnpm rust:test:real
```

2026-07-16 最终结果：

- `pnpm typecheck`：通过。
- `pnpm test:unit`：15 个测试文件、136 个测试通过。
- `pnpm --filter @imagedb/desktop build:web`：通过，Vite 生产构建完成。
- `pnpm format:check`：Prettier 与 `cargo fmt --check` 通过。
- `pnpm rust:clippy`：`--all-targets --all-features -- -D warnings` 通过。
- `pnpm rust:test`：225 通过、0 失败、3 个真实数据库用例按设计忽略。
- `pnpm rust:test:real`：23 个真实测试组、102 个用例全部通过；新增候选证据批量写入断言后又定向重跑 `real_scan_persists_exact_duplicates` 并通过。

算法回归夹具全部为程序生成的小图片，不包含大规模压力测试。覆盖文件/像素精确匹配、透明像素、EXIF、哈希长度、归一化距离、几何变换、缩放、JPEG 质量、亮度、水印、主体位移、相邻动作、明显不同图片、索引构建/增量/移除/稳定排序/稳定截断及非 V2 排除。

## 已知限制

- DoubleGradient 32×32 的实际位数由 `image_hasher` 输出字节长度决定；当前版本为 544 bits，代码和 UI 均使用该实际长度。
- 内存图库索引不是持久化缓存；应用重启或失效后从 PostgreSQL 自动重建。
- 当前产品没有“删除已提交图库记录”的公开命令。索引已提供 `remove`，Recovery 和数据库生命周期变更走安全失效；未来接入删除命令时必须同步调用或使缓存失效。
- 本任务只执行小规模功能与集成测试，未加入库存压力测试。
