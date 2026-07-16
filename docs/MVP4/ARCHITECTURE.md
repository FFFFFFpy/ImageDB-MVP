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
→ 灰度方差、有效灰度级、边缘变化量与哈希信息量资格检查
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

感知判定还有一个先决条件：两侧图片都必须 `perceptual_eligible = true`。资格从 32×32 灰度图的标准差、16 桶有效灰度级、相邻像素平均边缘变化量，以及 BlockHash / DoubleGradient 是否退化为全零、全一或极低信息量联合得出。纯色、近空白和其他退化图片仍保留完整文件与像素 BLAKE3，因此精确重复行为不变；它们不会进入图集内或正式图库 BK-tree，零感知距离也不能触发 `auto_duplicate`。

## 图集内索引

图片按相对路径稳定排序。文件哈希和像素哈希在整个 import run 各维护一个最低 UUID 代表映射；当前图集的每个新成员只连接该代表，图集结束后只把轻量精确元信息并入 run 索引。恢复扫描时从已完成分析检查点各加载一个数据库代表，避免重新持有所有历史图片。因此同一精确组即使分散在多个图集，仍只有 `k - 1` 条边。

感知召回使用只包含已处理且通过资格检查的图片基础 BlockHash 的临时 BK-tree。当前合格图片的 8 个变换查询该树，按图片 ID 保留所有并列最小 BlockHash 距离的变换，再逐一计算 DoubleGradient 并选择精细距离最小的变换。候选对规范化为较小 UUID / 较大 UUID，避免多变换重复写入。

## 正式图库索引

`AppState` 持有 `Arc<RwLock<Option<LibraryFingerprintIndex>>>`：

1. 第一次图库比对时从 V2 数据库记录构建；所有记录进入文件/像素精确映射，只有 `perceptual_eligible` 记录进入 BK-tree。
2. 同一应用生命周期内复用。
3. Commit 前记录数据库时间边界，成功后只查询本次新增记录并批量 upsert：新 ID 增量加入，相同 ID/指纹跳过，内容变化统一重建一次。
4. Recovery 可能改变正式图库时使缓存失效。
5. 数据库初始化、切换、外部迁移或关闭后使缓存失效。
6. 删除能力接入时调用索引 `remove`；若增量操作失败则清空缓存，下次扫描重建。

召回结果按最小 BlockHash 距离、UUID 稳定排序，单图最多 256 个图片候选。每个图片候选保留所有并列最小距离变换；截断会记录告警和图集指标。精细哈希不进入 BK-tree 或索引元数据；召回后一次批量 SQL 加载。

## 数据库迁移

`0015_fingerprint_v2.sql`：

- `import_images` / `library_images` 新增 `block_hash_16`、`double_gradient_hash_32` 和非空的 `perceptual_eligible`。
- 删除旧 Gradient、Block、Median 和四个感知桶字段/索引。
- `duplicate_candidates` 新增 DoubleGradient raw distance 和两种 normalized ratio，删除旧 Gradient/Median distance。
- 对 V2 四个哈希增加显式非空与长度检查，对距离比例和 confidence 增加范围约束。
- 删除旧通用 BLAKE3 / pixel hash 索引，只保留覆盖 V2 记录的部分索引。

项目没有需要兼容的真实旧图库；迁移不会把 V1 感知哈希伪装成 V2。`fingerprint_version != '2'` 的记录不会进入 V2 索引。
