# MVP4：Fingerprint V2 与高效重复检测引擎

MVP4 将旧的 8×8、自实现三哈希、感知前缀桶和图集内全量两两比较替换为一套固定生产方案。算法、尺寸、滤镜和阈值都是代码常量，不进入用户设置。

## 固定方案

| 用途         | 实现                                                              |
| ------------ | ----------------------------------------------------------------- |
| 文件精确匹配 | 完整 BLAKE3，32 bytes                                             |
| 像素精确匹配 | 应用 EXIF Orientation 后的规范化 RGBA8 + 宽高，完整 BLAKE3        |
| 粗召回       | `image_hasher::Blockhash`，16×16，Triangle                        |
| 精细验证     | `image_hasher::DoubleGradient`，32×32，Triangle                   |
| 几何关系     | Identity、90/180/270 度旋转、水平/垂直镜像、Transpose、Transverse |
| 图集内召回   | 每图集临时 Hamming BK-tree                                        |
| 正式图库召回 | `AppState` 缓存的 Hamming BK-tree                                 |
| 感知安全门   | 32×32 灰度方差、有效灰度级、边缘变化量与哈希信息量联合判定        |
| 指纹版本     | `2`                                                               |

实现细节见 [`ARCHITECTURE.md`](ARCHITECTURE.md)，验收记录见 [`ACCEPTANCE.md`](ACCEPTANCE.md)。

## 边界

- 每张图片在一个 `spawn_blocking` 中读取和完整解码一次。
- 运行时哈希比较使用字节数组，不使用十六进制字符串。
- 8 种变换用于查询基础方向 BlockHash；每个候选保留所有并列最小粗距离变换，并逐一计算 DoubleGradient 后选择精细距离最小者。
- 正式图库只持久化基础方向哈希。
- 图库候选 ID 在内存合并后用一次 `WHERE id = ANY($1)` 批量加载精细证据。
- 每个图集的候选在内存规范化、去重并稳定排序后批量写入。
- 低信息量图片仍执行文件/像素精确匹配，但不进入感知 BK-tree，也不能由感知规则自动判重。
- 整个 import run 为文件哈希和像素哈希分别维护最低 UUID 代表；跨图集时每个新成员也只连接代表，`k` 张重复图只写 `k - 1` 条边。
- Commit 后使用批量 upsert 更新缓存；相同 ID/指纹跳过，内容变化至多统一重建一次。
- MVP4 不修改审核动作、导入计划冻结、Commit、发布、归档和 Recovery 的事务语义。

## 文档地图

- [`ARCHITECTURE.md`](ARCHITECTURE.md)：指纹、召回、判定、索引生命周期和迁移结构。
- [`ACCEPTANCE.md`](ACCEPTANCE.md)：验收矩阵、验证命令和已知限制。
