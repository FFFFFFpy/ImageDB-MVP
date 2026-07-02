# 数据库设计

## 模式

```text
ManagedLocal
External
```

两种模式最终都生成统一的连接配置并初始化同一套 Repository。

## 托管实例

- PostgreSQL 二进制按平台随应用分发。
- 数据目录位于系统应用数据目录。
- 凭据保存到操作系统凭据存储。
- 监听本机地址和应用选择的端口。
- 应用启动后执行健康检查和迁移。

## Schema

初始表：

- app_meta
- library_roots
- import_runs
- import_albums
- import_images
- library_albums
- library_images
- duplicate_candidates
- review_decisions
- file_transactions
- file_operations
- audit_events

数据库脚本位于：

```text
apps/desktop/src-tauri/migrations/
```

## 约束

- 路径保存为规范化字符串，同时保留原始显示名称。
- 状态使用 Rust 枚举与数据库文本值一一映射。
- 正式图库记录只在文件发布成功后写入。
- 每次迁移必须可在空库和已有库上验证。
