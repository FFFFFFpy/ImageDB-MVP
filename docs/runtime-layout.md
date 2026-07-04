# 托管 PostgreSQL 运行时布局契约

## 安装资源

```text
<ImageDB install>/resources/postgres-runtime/
├─ runtime-manifest.json
├─ bin/
├─ lib/
├─ share/
│  └─ extension/
└─ THIRD_PARTY_NOTICES.txt
```

安装资源只读，不保存数据库数据。

## 用户数据

```text
<User App Data>/ImageDB/managed-postgres/
├─ data/
├─ logs/
├─ backups/
├─ runtime-state.json
└─ instance.lock
```

## runtime-state.json

建议字段：

```json
{
  "schema_version": 1,
  "instance_id": "uuid",
  "runtime_version": "project-pinned-version",
  "postgres_major": 17,
  "pgvector_version": "project-pinned-version",
  "data_dir": "absolute path",
  "port": 0,
  "database": "imagedb",
  "username": "generated user",
  "initialized_at": "timestamp",
  "last_clean_shutdown": "timestamp or null"
}
```

密码不进入该文件。

## 解析原则

正式安装只使用 Tauri `resource_dir` 中的运行时。开发覆盖必须显式启用并在诊断页标记为开发来源。
