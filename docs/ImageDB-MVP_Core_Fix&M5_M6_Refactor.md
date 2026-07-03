# ImageDB-MVP 核心闭环修复与 M5/M6 重构计划

## 一、任务目标

先修复分析、审核、状态管理和历史图库匹配中的正确性问题，再重构正式入库事务，最后实现基于新事务协议的中断恢复。

执行顺序：

```text
分析正确性收口
→ 不可变入库计划
→ 文件事务重构
→ 中断恢复
→ GUI接入
→ 真实故障验收
```

在新事务协议完成以前，当前 Milestone 5 视为顺利路径原型，不视为安全完成。

---

# 二、阶段一：分析与审核正确性收口

## 1. 历史图库查询必须失败即停止

历史图库读取失败时，不得返回空集合继续分析。

要求：

* 数据库连接失败、查询失败或字段解析失败时，立即停止当前分析。
* 将运行状态更新为 `FAILED`。
* 保存明确的错误码和错误信息。
* 不生成候选、审核结果或入库计划。
* GUI显示真实错误，不能显示“历史图库无匹配”。
* 删除核心路径中的静默默认值。

当前历史图库查询仍使用失败后返回空集合的处理，必须优先修复。

验收测试：

* 数据库断开。
* 查询返回错误。
* 非法字段数据。
* 失败后运行不可进入审核或提交。
* 修复数据库后可以重新分析。

---

## 2. 重复图片必须先形成组，再选择代表图

不能继续通过候选左右位置或遍历顺序决定排除哪张图片。

处理流程：

```text
构建重复关系
→ 生成连通重复组
→ 为每组选择代表图
→ 排除其余高置信重复
→ 无法稳定选择时进入审核
```

代表图规则：

1. 无法解码的图片不能优先保留。
2. 字节完全相同的文件可使用稳定 ID 作为最终平局规则。
3. 非字节完全相同的图片应比较已有可靠质量信息。
4. 质量差异不足时进入人工审核。
5. 任何重复组都必须至少保留一张图片。
6. 结果不得随扫描顺序、文件排序或候选生成顺序变化。

验收测试：

* 两张完全相同图片。
* 输入顺序颠倒。
* 三张形成同一重复组。
* 链式关系 A≈B、B≈C。
* 质量差异明确。
* 质量差异不足。
* 重复组不会被全部排除。

---

## 3. 支持同一次运行中的跨图集重复检测

比较范围必须覆盖：

```text
单个图集内部
同一次运行的不同图集之间
当前运行与正式历史图库之间
```

要求：

* 本次导入的所有图片参与统一精确 Hash 索引。
* 不同图集之间发现重复时记录双方图集。
* 结果不依赖图集扫描顺序。
* 跨图集重复组仍只能保留确定的代表图片。
* 某个图集最终没有新图片时，明确标记为跳过，而不是创建空目录。

验收测试：

* 两个图集包含同一文件。
* 两个图集包含不同编码的同一图片。
* 三个图集共享图片。
* 一个图集全部被本次其他图集覆盖。
* 调换图集顺序后结果一致。

---

## 4. 历史图库匹配改为索引召回

禁止将全部历史图片加载到内存后执行：

```text
新图片数量 × 历史图片数量
```

的全量比较。

精确匹配：

* 按 BLAKE3 批量查询。
* 按像素 Hash 批量查询。
* 使用数据库索引。
* 批量插入候选。

感知匹配：

* 为感知 Hash 建立可索引的分桶键或 band key。
* 先从数据库召回有限候选。
* 再在 Rust 中计算完整汉明距离。
* 单张新图片的候选数量必须有明确上限。
* 不得恢复为全库笛卡尔比较。

验收：

* 空历史图库。
* 小型历史图库。
* 大批量精确 Hash 查询。
* 感知 Hash 候选有界。
* 查询次数不随 `N×M` 增长。
* 数据库查询计划使用对应索引。

---

## 5. 零审核候选必须直接继续

统一审核完成条件：

```text
remaining_candidates == 0
```

要求：

* 从未产生审核候选时，可以直接生成入库计划。
* 候选全部处理完成时，可以生成入库计划。
* GUI零候选页面提供“继续”入口。
* 后端不得要求候选总数大于零。
* 零候选和全部已审核走同一后续流程。

---

## 6. 后台任务必须保存真实终态

扫描、分析、提交和恢复任务使用统一的任务生命周期管理。

要求：

* 成功、失败、取消和 panic 都必须生成终态。
* 必须回收并 `await JoinHandle`。
* 必须处理 `JoinError`。
* 不得通过 `is_finished()` 后直接丢弃 handle。
* 数据库保存可恢复状态。
* 内存 tracker 只负责实时进度。
* 清除 active task 前，终态必须已经落库。
* 任务失败后允许重新执行，但旧错误仍可查询。

验收测试：

* 正常完成。
* Service 返回错误。
* 用户取消。
* 任务 panic。
* 完成后再次查询。
* 失败后重新启动任务。
* 应用重启后读取上次终态。

---

## 7. 图片预览改为受控业务接口

前端不得向 Rust 传递任意绝对路径。

接口参数改为：

```text
candidate_id
image_side
```

后端负责：

1. 查询候选记录。
2. 解析对应图片记录。
3. 获取真实路径。
4. canonicalize。
5. 校验路径位于允许的源目录或图库目录内。
6. 校验格式。
7. 限制文件大小和解码像素数。
8. 生成受限尺寸的预览图。
9. 返回预览结果。

验收测试：

* 合法候选。
* 候选不存在。
* 非法 side。
* 路径逃逸。
* 非图片文件。
* 损坏图片。
* 超大图片。
* 数据库路径被替换。

---

# 三、阶段二：统一状态模型

## 1. Import Run 状态

使用以下语义：

```text
CREATED
SCANNING
ANALYZING
REVIEW_REQUIRED
READY_TO_COMMIT
COMMITTING
RECOVERY_REQUIRED
COMPLETED
FAILED
CANCELLED
```

规则：

* 分析结束不等于整个运行完成。
* 有待审核候选时进入 `REVIEW_REQUIRED`。
* 无候选或审核完成后进入 `READY_TO_COMMIT`。
* 文件和数据库操作期间进入 `COMMITTING`。
* 存在可恢复的未完成文件事务时进入 `RECOVERY_REQUIRED`。
* 所有应入库图集完成，并完成源归档后才进入 `COMPLETED`。
* 不可恢复错误才进入 `FAILED`。

## 2. 图集文件事务状态

```text
PLANNED
STAGING
VERIFYING
VERIFIED
PUBLISHING
PUBLISHED
DB_COMMITTING
LIBRARY_COMMITTED
SOURCE_ARCHIVING
SOURCE_ARCHIVED
CLEANUP_REQUIRED
CONFLICT
FAILED
CANCELLED
```

其中：

* `PUBLISHED` 表示正式目录已经出现，但数据库尚未确认。
* `LIBRARY_COMMITTED` 表示正式文件和数据库都已确认，但源图集可能尚未归档。
* 源归档失败不得把已成功入库的图集重新标记为普通失败。
* `SOURCE_ARCHIVED` 才是单个图集的最终完成状态。

## 3. 集中管理状态转换

建立单独的状态转换模块：

```text
当前状态
+ 业务动作
→ 下一状态
```

要求：

* Service 不得随意写状态字符串。
* 非法跳转必须返回错误。
* Rust enum、数据库约束和前端类型保持一致。
* 数据库增加状态 `CHECK` 约束。
* 为已有开发数据提供明确迁移。

---

# 四、阶段三：建立不可变入库计划

## 1. 新增正式计划实体

新增：

```text
import_plans
import_plan_albums
import_plan_images
```

### import_plans

保存：

* plan_id
* import_run_id
* version
* state
* policy_version
* library_root_id
* plan_hash
* created_at
* frozen_at

状态：

```text
DRAFT
FROZEN
CONSUMED
INVALIDATED
```

### import_plan_albums

保存：

* plan_id
* import_album_id
* target_relative_path
* expected_image_count
* album_plan_hash

### import_plan_images

保存：

* plan_album_id
* import_image_id
* source_path
* source_relative_path
* target_relative_path
* expected_file_size
* expected_blake3
* width
* height
* format

## 2. 冻结规则

生成计划时：

1. 所有审核必须完成。
2. 当前运行必须处于 `READY_TO_COMMIT`。
3. 确定每个图集和图片的最终目标相对路径。
4. 保存完整源文件快照。
5. 对规范化计划内容计算 plan hash。
6. 将计划更新为 `FROZEN`。
7. 冻结后不得重新从候选和审核记录动态推导提交集合。

提交时：

* 只能读取冻结计划。
* UUID解析失败必须整体失败。
* 任意计划图片缺失必须整体失败。
* 文件大小或 BLAKE3 变化必须拒绝提交。
* library root 不匹配必须拒绝提交。
* 不允许静默跳过计划条目。

当前代码存在计划 ID 解析失败后静默丢弃条目的路径，必须删除。

## 3. 图库根目录身份

* import run 创建时绑定固定 `library_root_id`。
* 冻结计划保存同一个 `library_root_id`。
* 提交时从数据库读取该 root 的固定路径。
* 当前设置指向其他 root 时不得改写旧 root。
* 切换图库应创建或选择另一个 `library_root`。

---

# 五、阶段四：重构正式文件事务

## 1. 提交前一次性写入事务证据

在复制任何文件以前，使用一个数据库事务写入：

* `file_transaction`
* 全部 `file_operations`
* 每条操作的源路径
* staging 路径
* 正式目标路径
* 预期大小
* 预期 BLAKE3
* 初始状态 `PLANNED`

不得在文件复制并校验成功后才创建 operation。

文件 operation 状态：

```text
PLANNED
COPYING
COPIED
VERIFYING
VERIFIED
PUBLISHED
FAILED
CANCELLED
```

---

## 2. 保留相对目录结构

所有目标路径使用冻结计划中的 `target_relative_path`。

要求：

* 不得只使用 `file_name()`。
* 支持图集内部子目录。
* 拒绝绝对路径。
* 拒绝 `..` 路径逃逸。
* 规范化路径分隔符。
* 检测大小写冲突。
* 检测同一图集内目标路径重复。

当前实现通过文件名构造 staging、manifest和正式图片路径，会丢失子目录信息，必须重构。

---

## 3. Staging 文件复制

staging 必须位于目标图库根目录所在的同一文件系统：

```text
<library_root>/.imagedb/staging/<transaction_id>/<album_relative_path>
```

每个文件：

1. 写入临时 `.part` 文件。
2. 使用流式复制。
3. 复制过程中增量计算 BLAKE3。
4. 支持取消检查。
5. 文件写入完成后 flush、关闭。
6. 校验大小和 BLAKE3。
7. 将 `.part` 重命名为正式 staging 文件。
8. operation 更新为 `VERIFIED`。

不得把整个大文件反复读取进内存。

---

## 4. Manifest

在 staging 图集目录内生成：

```text
.imagedb-manifest.json
```

内容至少包含：

* schema_version
* transaction_id
* plan_id
* plan_hash
* import_run_id
* import_album_id
* library_root_id
* 图集目标相对路径
* 所有图片目标相对路径
* 文件大小
* BLAKE3
* 图片尺寸
* 指纹版本

Manifest：

* 先写临时文件。
* 写完后 flush。
* 原子重命名为正式 manifest。
* 计算并保存 manifest hash。
* Manifest 与冻结计划必须完全一致。

---

## 5. 原子发布

所有 staging 文件和 manifest 验证完成后：

```text
完整 staging 图集目录
→ 同文件系统 rename
→ 正式图集目录
```

正式目录在发布前不得存在。

不得继续采用：

```text
先创建正式目录
→ 逐张复制文件
```

当前 M5 正是逐文件写入已可见的正式目录，需要整体替换。

发布冲突规则：

* 目标不存在：允许发布。
* 目标存在且 manifest 与当前 transaction、plan hash 完全一致：进入恢复判断。
* 目标存在但 manifest 不匹配：标记 `CONFLICT`。
* 目标存在且没有 manifest：标记 `CONFLICT`。
* 不得自动覆盖、删除或改名继续。

---

## 6. 数据库确认落库

发布成功后进入 `PUBLISHED`。

然后使用一个 PostgreSQL事务：

1. 校验冻结计划。
2. 校验正式 manifest。
3. 插入或确认 `library_album`。
4. 插入或确认全部 `library_images`。
5. 保存 plan hash 和 manifest hash。
6. 将 file transaction 更新为 `LIBRARY_COMMITTED`。
7. 将 plan 更新为 `CONSUMED`。

若数据库事务失败：

* 不删除已经发布的正式目录。
* 保持 `PUBLISHED`。
* 保存错误。
* 由恢复流程重试数据库提交。

不得使用“尽力删除正式目录”伪装回滚。当前实现对删除失败结果进行了忽略，需要取消这种处理。

---

## 7. 完整幂等判断

不得再只比较：

```text
数据库状态
+ 图片数量
```

完整幂等判断必须验证：

* transaction_id
* plan_id
* plan hash
* manifest hash
* 正式目录存在
* manifest 可解析
* 文件集合一致
* 每个文件路径一致
* 每个文件大小一致
* 每个文件 BLAKE3 一致
* 数据库 album 和 image 记录一致

只有全部一致，才能判定已经完成。

---

## 8. 源图集归档

数据库确认后进入 `LIBRARY_COMMITTED`。

归档目录位于源根目录内部：

```text
<source_root>/.imagedb-processed/<run-id>/<album-relative-path>
```

流程：

1. 检查源图集快照仍与冻结计划一致。
2. 检查归档目标不存在。
3. 使用同文件系统 rename 移动完整源图集。
4. 成功后更新 `SOURCE_ARCHIVED`。

归档失败时：

* 正式图库和数据库仍保持成功。
* transaction 保持 `LIBRARY_COMMITTED` 或 `SOURCE_ARCHIVING`。
* import run进入 `RECOVERY_REQUIRED`。
* 重试时只继续源归档。
* 不重新复制或重新入库正式图片。

---

## 9. 取消语义

取消仅在安全检查点生效：

* `PLANNED`
* 文件复制块之间
* 单个图集发布前
* 图集之间

一旦进入 `PUBLISHING`，必须完成当前图集到 `LIBRARY_COMMITTED`，再停止后续图集。

取消状态不能记录为普通失败。

GUI应显示：

```text
正在安全停止
```

而不是立即宣称任务已取消。

---

## 10. 空计划

冻结计划中没有任何待入库图片时：

* 不创建文件事务。
* 所有空图集记录明确跳过原因。
* import run直接进入 `COMPLETED`。
* 设置完成时间。
* GUI显示“没有新图片需要入库”。

数据库状态与GUI结果必须一致。

---

# 六、阶段五：数据库约束

新增 migration，至少包含：

* Import run状态约束。
* Plan状态约束。
* File transaction状态约束。
* File operation状态约束。
* Candidate scope约束。
* Review decision约束。
* Decision source约束。

唯一约束：

```text
library_albums(library_root_id, relative_path)
library_images(album_id, relative_path)
import_plans(import_run_id, version)
import_plan_images(plan_album_id, target_relative_path)
```

增加部分唯一索引：

```text
同一个 import_album 同一时间只能有一个活动 file transaction
同一个 import_run 同一时间只能有一个 FROZEN plan
```

重复候选建立规范化 pair 唯一键，避免反向重复和重试重复插入。

数据库约束必须与Rust状态机保持一致。

---

# 七、阶段六：基于状态证据实现恢复

当前 M6要求恢复复制、校验、发布、数据库提交和源归档，并保证幂等、不覆盖未知文件、不产生虚假完成状态。 恢复实现必须基于重构后的事务协议。

## 1. 启动扫描

应用启动时查询所有非终态事务：

```text
PLANNED
STAGING
VERIFYING
VERIFIED
PUBLISHING
PUBLISHED
DB_COMMITTING
LIBRARY_COMMITTED
SOURCE_ARCHIVING
CLEANUP_REQUIRED
CONFLICT
```

生成恢复诊断，不自动猜测成功。

## 2. 各状态恢复规则

### PLANNED / STAGING

* 对照全部 file operations。
* 清理不完整 `.part`。
* 已验证文件可复用。
* 继续缺失文件复制。

### VERIFYING / VERIFIED

* 重新校验 staging 文件集合、大小和 BLAKE3。
* 校验 manifest。
* 完整后继续发布。

### PUBLISHING

检查 staging 和正式目录：

* staging存在、正式目录不存在：重试 rename。
* 正式目录存在且 manifest匹配：进入 `PUBLISHED`。
* 两边都存在：检查完整证据；无法唯一判断则进入 `CONFLICT`。
* 正式目录内容未知：绝不覆盖。

### PUBLISHED / DB_COMMITTING

* 完整验证正式目录和 manifest。
* 验证通过后重试数据库确认。
* 验证失败进入 `CONFLICT` 或 `FAILED`，不得写正式记录。

### LIBRARY_COMMITTED / SOURCE_ARCHIVING

* 验证正式文件和数据库记录。
* 继续源归档。
* 不重新执行正式入库。

### SOURCE_ARCHIVED

* 验证所有图集终态。
* import run更新为 `COMPLETED`。

### CLEANUP_REQUIRED

* 显示残留路径和失败原因。
* 仅清理明确属于当前 transaction 的 staging。
* 不自动删除未知正式目录。

### CONFLICT

* GUI显示冲突证据。
* 提供重新验证。
* 不提供无确认覆盖。

---

# 八、阶段七：GUI调整

## 分析页

显示：

* 当前阶段。
* 数据库读取失败。
* 跨图集重复数量。
* 历史图库候选数量。
* 真实终态。

## 审核页

* 零候选可以直接继续。
* 受控图片预览。
* 显示重复组和代表图建议。
* 显示自动判断证据。

## 提交确认页

显示冻结计划摘要：

* plan ID
* 图集数量
* 图片数量
* 跳过数量
* 目标图库
* 预计写入大小
* plan hash简写

提交后不得重新动态生成计划。

## 提交进度页

显示：

```text
准备事务
复制到 staging
校验
发布
数据库确认
源图集归档
```

区分：

* 已正式入库
* 等待源归档
* 可恢复中断
* 冲突
* 普通失败
* 已取消

## 恢复页

每个事务显示：

* 当前状态
* 已完成证据
* 缺失证据
* 建议恢复动作
* 重试
* 重新验证
* 打开相关目录
* 查看错误

不得提供默认覆盖未知文件的按钮。

---

# 九、故障注入测试

建立受测试 feature 控制的故障注入点：

```text
事务和operations写入后
复制第N个文件中
单文件复制后、operation更新前
全部staging验证后
manifest写入后
rename发布前
rename发布后
数据库事务开始前
数据库事务提交后
源归档前
源归档中
任务panic
取消请求
```

每个故障点执行：

1. 强制返回错误或终止任务。
2. 重启应用或重新创建Service。
3. 运行恢复。
4. 验证文件系统、manifest、数据库和状态。
5. 再次运行恢复，确认幂等。

---

# 十、必须通过的验收场景

## 分析与审核

1. 历史图库查询失败时停止。
2. 零审核候选直接继续。
3. 重复组至少保留一张。
4. 输入顺序变化不影响代表图。
5. 跨图集重复可发现。
6. 大历史图库不执行全量笛卡尔比较。
7. 后台任务panic后终态可查询。
8. 图片预览不能读取任意路径。

## 计划

9. 冻结后修改审核记录不改变提交集合。
10. 冻结条目损坏时整体拒绝。
11. 源文件变化时拒绝提交。
12. 目标图库变化时拒绝提交。
13. 空计划正确完成。

## 文件事务

14. 复制中断后可继续。
15. staging校验中断后可继续。
16. 发布前取消不会出现正式目录。
17. 发布后数据库失败可恢复提交。
18. 数据库成功后源归档失败可继续归档。
19. 正式目录在发布完成前不可见。
20. 同名子目录文件不会互相覆盖。
21. 目标目录冲突不会覆盖。
22. 清理失败会留下明确状态。
23. 重复执行不会重复插入记录。
24. 正式文件被修改后不会被误判为已完成。

## 最终闭环

25. 图集内部重复处理。
26. 跨图集重复处理。
27. 与历史图库重复处理。
28. 人工审核。
29. 冻结计划。
30. staging与校验。
31. 原子发布。
32. PostgreSQL确认。
33. 源图集归档。
34. 应用重启后完整验证。

---

# 十一、执行和提交顺序

建议按以下本地提交拆分：

```text
fix: fail safely during historical library matching
refactor: group duplicate candidates and select stable representatives
fix: complete zero-review and background task terminal flows
perf: add indexed historical matching and cross-album detection
fix: restrict image previews to persisted candidate records
refactor: introduce validated import workflow states
feat: persist immutable import plans
refactor: rebuild staged file transaction protocol
feat: publish albums atomically from verified staging
feat: persist recoverable library commit and source archive states
feat: recover interrupted import transactions
db: enforce workflow and idempotency constraints
test: add import failure injection acceptance suite
feat: complete recovery and commit GUI
```

每个提交前运行相关测试。

最终运行：

```powershell
pnpm install
pnpm format:check
pnpm typecheck
pnpm test:unit
pnpm rust:test
pnpm rust:clippy
pnpm build
```

并单独运行：

* 真实 PostgreSQL集成测试。
* 真实文件系统集成测试。
* 全部故障注入测试。
* 完整图集闭环测试。

---

# 十二、完成定义

只有同时满足以下条件，才能重新声明 M5和M6完成：

* 分析错误不会静默降级。
* 重复选择不依赖顺序。
* 同一运行支持跨图集重复。
* 历史图库匹配不执行无界全量比较。
* 零候选流程可继续。
* 后台任务终态不丢失。
* 提交只使用不可变冻结计划。
* 文件operation在复制前完整写入。
* 相对路径不会被扁平化。
* staging与正式目录处于同一文件系统。
* 正式目录通过目录rename一次性发布。
* 幂等判断验证完整文件与manifest证据。
* 数据库提交失败后可从已发布目录恢复。
* 源归档失败不会否定已完成的正式入库。
* 取消只在安全检查点生效。
* 所有关键状态均通过强制中断恢复测试。
* 不覆盖未知文件。
* 不丢失源图集。
* 不产生虚假完成状态。
* GUI能够展示并操作全部恢复状态。
* 完整构建和真实闭环验收通过。
