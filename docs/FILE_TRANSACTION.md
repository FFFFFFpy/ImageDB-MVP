# 文件事务

## 目录

```text
<library_root>/
├── Albums/
└── .imagedb/
    ├── staging/
    ├── manifests/
    └── recovery/
```

## 提交流程

1. 固化入库计划。
2. 校验源文件快照。
3. 创建 staging 图集目录。
4. 复制最终保留文件。
5. 校验文件大小与 BLAKE3。
6. 生成 manifest 临时文件。
7. 发布正式图集目录。
8. 发布正式 manifest。
9. 提交 PostgreSQL 正式图库记录。
10. 归档完整源图集。

## 恢复证据

恢复判断同时使用：

- file_transactions 状态
- file_operations 状态
- staging 内容
- 正式目录
- manifest
- PostgreSQL 正式图库记录

目标目录已存在但缺少一致证据时停止并报告冲突。
