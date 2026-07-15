# Fingerprint V2 架构

## 单图处理

```text
读取一次文件字节
→ 完整文件 BLAKE3
→ 从同一字节缓冲解析 EXIF Orientation
→ 完整解码一次并应用方向
→ 规范化 RGBA8（alpha=0 时 RGB=0）
→ 宽高 + 完整 RGBA 的 BLAKE3
→ 灰度图
→ Triangle 16×16 / 32×32
→ 基础 BlockHash / 基础 DoubleGradient
→ 16×16 图的 8 种变换 BlockHash
```

完整文件字节、全分辨率 RGBA 和全分辨率灰度图不会进入导入任务的长期状态。单个图集检测期间只保留元信息、字节哈希、8 个小 BlockHash 和一个 32×32 灰度图，后者用于对并列最小粗距离变换执行精细验证；图集完成后这些细粒度对象立即释放。

`image_hasher` 的 BlockHash 16×16 实际为 256 bits。DoubleGradient 32×32 的当前实现实际为 544 bits；所有距离都从字节长度计算实际位数，决策不依赖固定总距离分母。

## 判定

```text
file_size + BLAKE3 相同                 → file_exact / auto_duplicate
pixel_hash 相同                         → pixel_exact / auto_duplicate
block ≤ 0.04 且 fine ≤ 0.04             → perceptual_near / auto_duplicate
block ≤ 0.12 且 fine ≤ 0.08             → perceptual_similar / 人工审核
其余                                     → 不写候选
```

综合相似度为：

```text
1 - (block_distance_ratio × 0.40 + double_gradient_distance_ratio × 0.60)
```

数据库同时保存 raw distance、normalized distance、最佳变换和综合相似度。

## 图集内索引

图片按相对路径稳定排序。文件哈希和像素哈希先按组选择最低 UUID 代表，每个其余成员只连接到该代表；感知召回使用只包含已处理图片基础 BlockHash 的临时 BK-tree。当前图片的 8 个变换查询该树，按图片 ID 保留所有并列最小 BlockHash 距离的变换，再逐一计算 DoubleGradient 并选择精细距离最小的变换。候选对规范化为较小 UUID / 较大 UUID，避免多变换重复写入。

## 正式图库索引

`AppState` 持有 `Arc<RwLock<Option<LibraryFingerprintIndex>>>`：

1. 第一次图库比对时从 V2 数据库记录构建。
2. 同一应用生命周期内复用。
3. Commit 前记录数据库时间边界，成功后只查询本次新增记录并批量 upsert：新 ID 增量加入，相同 ID/指纹跳过，内容变化统一重建一次。
4. Recovery 可能改变正式图库时使缓存失效。
5. 数据库初始化、切换、外部迁移或关闭后使缓存失效。
6. 删除能力接入时调用索引 `remove`；若增量操作失败则清空缓存，下次扫描重建。

召回结果按最小 BlockHash 距离、UUID 稳定排序，单图最多 256 个图片候选。每个图片候选保留所有并列最小距离变换；截断会记录告警和图集指标。精细哈希不进入 BK-tree 或索引元数据；召回后一次批量 SQL 加载。

## 数据库迁移

`0015_fingerprint_v2.sql`：

- `import_images` / `library_images` 新增 `block_hash_16`、`double_gradient_hash_32`。
- 删除旧 Gradient、Block、Median 和四个感知桶字段/索引。
- `duplicate_candidates` 新增 DoubleGradient raw distance 和两种 normalized ratio，删除旧 Gradient/Median distance。
- 对 V2 四个哈希增加显式非空与长度检查，对距离比例和 confidence 增加范围约束。
- 删除旧通用 BLAKE3 / pixel hash 索引，只保留覆盖 V2 记录的部分索引。

项目没有需要兼容的真实旧图库；迁移不会把 V1 感知哈希伪装成 V2。`fingerprint_version != '2'` 的记录不会进入 V2 索引。
