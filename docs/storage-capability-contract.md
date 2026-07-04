# StorageCapabilities 契约

建议结构：

```text
StorageCapabilities
- readable
- writable
- file_rename
- directory_rename
- atomic_directory_publish
- file_sync
- directory_sync
- case_sensitive
- unicode_normalization
- max_path
- max_component
- locking
- timestamp_resolution
- free_space
- volume_identity
- probe_version
```

每项状态使用：

```text
supported
unsupported
unknown
```

发布策略：

```text
StrongLocal
ConservativeMounted
Unsupported
```

决策必须可解释并显示在GUI中。能力探测结果保存时间和卷身份，重新挂载后重新探测。
