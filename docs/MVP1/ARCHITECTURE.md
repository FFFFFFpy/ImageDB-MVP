# MVP1 架构与主链说明

## 1. 产品范围

ImageDB MVP1 是一个桌面端本地图集导入工具。它接受一个源目录，并把源目录下的每个一级子目录视为一个图集。

每个图集的目标流程：

1. 扫描图片文件。
2. 建立源文件快照。
3. 计算文件与图像指纹。
4. 检测图集内部重复与相似图片。
5. 与历史图库中的图片比较。
6. 自动处理证据明确的重复项。
7. 将不确定候选交给用户审核。
8. 生成不可变 frozen import plan。
9. 将保留文件复制到目标图库 staging 目录。
10. 逐文件校验。
11. 发布正式图集目录和 manifest。
12. 在 PostgreSQL 中确认正式入库。
13. 归档源图集。
14. 中断后恢复未完成事务。

## 2. 技术栈

```text
React + TypeScript
TanStack Query
Tauri IPC
Rust Commands
Application Services
Domain Rules
Repositories / Infrastructure
PostgreSQL + pgvector
Filesystem
```

## 3. 前端职责

- 页面与交互。
- 表单校验。
- 查询缓存。
- 进度展示。
- 审核操作。
- 错误和诊断信息展示。

主要页面：

| 页面       | 职责                                       |
| ---------- | ------------------------------------------ |
| Onboarding | 初始化托管数据库或连接外部 PostgreSQL。    |
| Dashboard  | 展示数据库状态和导入入口。                 |
| Scan       | 选择源目录、验证图集、启动扫描、展示进度。 |
| Review     | 审核候选、复核 draft 计划、显式锁定 frozen plan。 |
| Commit     | 展示 frozen plan summary，执行正式入库。   |
| Recovery   | 扫描并恢复未完成文件事务。                 |
| Settings   | 数据库模式、图库根目录、诊断相关设置。     |
| Probes     | PostgreSQL、指纹、文件事务、存储能力探测。 |

## 4. Rust 后端职责

- 托管 PostgreSQL 生命周期。
- 外部 PostgreSQL 连接与迁移。
- 源目录扫描。
- 源文件快照。
- 图片解码与指纹计算。
- 重复 / 相似候选生成。
- 审核状态持久化。
- frozen import plan 生成与校验。
- 文件事务、staging、发布和归档。
- Recovery / Reverify。
- 诊断导出。

## 5. 数据库模式

### 托管本地模式

应用管理独立 PostgreSQL 实例：

- 初始化数据目录。
- 生成凭据。
- 选择本地端口。
- 启动与停止进程。
- 启用 pgvector。
- 执行迁移。
- 健康检查。

托管实例只监听本机地址，数据目录位于系统应用数据目录。Windows release 包内置 PostgreSQL + pgvector runtime。

### 外部连接模式

用户提供连接参数，应用负责：

- 测试连接。
- 检查 PostgreSQL 版本。
- 检查 pgvector。
- 检查权限。
- 检查 TLS 模式。
- 执行应用 Schema 迁移。

外部模式和托管模式共用 Repository 和业务主链。

## 6. 图像匹配流程

### 文件级

- 文件大小。
- BLAKE3。

### 像素级

解码后执行固定标准化：

- 应用方向信息。
- 固定颜色和通道规则。
- 固定 Alpha 处理。
- 固定像素排列。
- 计算标准像素 hash。

### 感知级

固定并版本化：

- Gradient Hash。
- Block Hash。
- Median Hash。
- 缩放尺寸。
- 缩放滤镜。
- 灰度规则。
- Hash 位数。
- Bit 顺序。

支持原图、旋转和镜像变换的候选比较。

## 7. 决策与审核

- 明确重复：自动排除新图片中的重复版本。
- 不确定候选：进入审核队列。
- 历史图库只参与比较，不在导入分析阶段被修改。
- 完整被覆盖图集：入库计划阶段可跳过被覆盖图集，保留完整图集或超集图集。
- 用户审核后先生成可编辑 draft import plan，复核完成后再显式锁定。

## 8. Frozen Import Plan

MVP1 中，Commit 不再临场重算计划。

正确链路：

```text
Scan
→ Review
→ generateImportPlan（持久化 draft，不生成 hash）
→ 人工调整图集 / 图片的导入或跳过
→ freezeImportPlan（锁定当前 draft）
→ import_plans / import_plan_albums / import_plan_images
→ plan_hash
→ Commit 读取 frozen summary
→ Commit service 加载同一 frozen plan 并校验 hash
```

关键规则：

- Review 页先生成并人工复核 draft；每次导入 / 跳过切换只更新 draft，不计算 hash。
- 任何审核组决定变更都在同一 `import_runs` 行锁事务内将已有 draft 标记为 `invalidated`；用户必须重新生成并复核完整计划。
- 用户显式“锁定导入计划”时才计算 `plan_hash` 并将计划转为 frozen。
- Commit 页只读取 frozen plan summary。
- Commit service 读取同一 frozen plan。
- post-freeze 的候选 / 审核变化不能改变 commit set。
- `completed` run 不进入默认 Commit 页。

## 9. 文件事务

```text
READY
→ STAGING
→ VERIFYING
→ VERIFIED
→ PUBLISHING
→ PUBLISHED
→ DB_COMMITTING
→ LIBRARY_COMMITTED
→ SOURCE_ARCHIVING
→ SOURCE_ARCHIVED
```

规则：

1. 分析期间源文件保持不变。
2. 提交前重新验证源文件快照。
3. 文件复制到目标根目录内的 staging。
4. staging 文件逐个重新计算 BLAKE3。
5. 全部校验通过后发布正式目录。
6. 写入 manifest。
7. PostgreSQL 事务写入正式图库记录。
8. 数据库成功后归档完整源图集。
9. 每个状态都可通过持久化证据恢复。

## 10. 存储能力策略

目标图库根目录可以位于：

- 本地磁盘。
- 外接磁盘。
- 操作系统已经挂载的共享目录。

应用通过 `StorageCapabilities` 探测：

- 读写。
- 文件 / 目录创建。
- rename 能力。
- fsync 能力。
- 大小写敏感性。
- Unicode 路径。
- 长路径。
- 文件锁。
- 时间戳精度。
- 剩余空间。

策略：

- `StrongLocal`：能力足够，走强本地发布策略。
- `ConservativeMounted`：能力保守，走更谨慎的 mounted 发布策略。
- `Unsupported`：缺必要能力，拒绝发布。

## 11. MVP1 边界

MVP1 不包含：

- 自实现 SMB / NAS 协议。
- 跨平台 PostgreSQL runtime 打包。
- 云同步。
- 多用户协作。
- 大规模 UI 改版。
- 新算法方向。

这些如果以后要做，应进入 MVP2 或后续版本，不要污染 MVP1 Debug。人类给范围命名，就是为了别把范围当橡皮泥。
