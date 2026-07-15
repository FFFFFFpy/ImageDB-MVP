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

完整文件字节、全分辨率 RGBA 和全分辨率灰度图不会进入导入任务的长期状态。导入期间只保留元信息、字节哈希、8 个小 BlockHash 和一个 32×32 灰度图，后者用于对最佳变换执行一次精细验证。

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

图片按相对路径稳定排序。文件哈希和像素哈希使用内存精确映射；感知召回使用只包含已处理图片基础 BlockHash 的临时 BK-tree。当前图片的 8 个变换查询该树，按图片 ID 合并最佳命中，候选对规范化为较小 UUID / 较大 UUID，避免多变换重复写入。

## 正式图库索引

`AppState` 持有 `Arc<RwLock<Option<LibraryFingerprintIndex>>>`：

1. 第一次图库比对时从 V2 数据库记录构建。
2. 同一应用生命周期内复用。
3. Commit 成功后增量加入本次入库记录。
4. Recovery 可能改变正式图库时使缓存失效。
5. 数据库初始化、切换、外部迁移或关闭后使缓存失效。
6. 删除能力接入时调用索引 `remove`；若增量操作失败则清空缓存，下次扫描重建。

召回结果按 BlockHash 距离、UUID 稳定排序，单图最多 256 个。截断会记录告警和图集指标。精细哈希不进入 BK-tree；召回后一次批量 SQL 加载。

## 数据库迁移

`0015_fingerprint_v2.sql`：

- `import_images` / `library_images` 新增 `block_hash_16`、`double_gradient_hash_32`。
- 删除旧 Gradient、Block、Median 和四个感知桶字段/索引。
- `duplicate_candidates` 新增 DoubleGradient raw distance 和两种 normalized ratio，删除旧 Gradient/Median distance。
- 对 V2 指纹长度、距离比例和 confidence 增加检查约束。
- 精确 BLAKE3 / pixel hash 索引只覆盖 V2 记录。

项目没有需要兼容的真实旧图库；迁移不会把 V1 感知哈希伪装成 V2。`fingerprint_version != '2'` 的记录不会进入 V2 索引。
