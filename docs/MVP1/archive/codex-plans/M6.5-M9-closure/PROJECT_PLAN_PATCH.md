# PROJECT_PLAN Patch: M6.5–M9 收口

## M6.5 Managed PostgreSQL Runtime Closure

目标：默认本地模式不要求用户安装 PostgreSQL。

范围：

- Windows x64 PostgreSQL portable runtime。
- pgvector runtime 文件。
- Tauri resource / bundle 配置。
- runtime完整性校验。
- 首次启动自动初始化。
- release产物校验。
- 干净环境 bootstrap 测试。

不做：

- Linux/macOS runtime 打包。
- 自建 PostgreSQL 分发系统。
- Docker依赖。

## M7 External PostgreSQL Closure

目标：已有外部 PostgreSQL TLS connector 接入正式 service 主链。

范围：

- database service 外部连接测试使用 TLS connector。
- 权限/版本/pgvector预检。
- 连接错误清晰展示。

不做：

- 复杂证书管理器。
- 多租户数据库平台。

## M8 Mounted Storage Closure

目标：已挂载共享目录具备最小安全能力探测。

范围：

- 原子rename探测。
- 长路径/Unicode/大小写探测。
- 断连或权限错误 fail-fast。
- release gate 真实执行。

不做：

- SMB协议实现。
- 分布式锁系统。
- NAS管理。

## M9 Final Public Workflow Closure

目标：公开 GUI/IPC 主链可完成真实导入。

范围：

- Review 冻结计划。
- Commit 页读取 frozen plan summary。
- Commit 执行相同 frozen plan。
- latest committable run 查询正确。
- 真实端到端测试。
- 最终报告。

不做：

- UI重设计。
- 新看板。
- 大规模性能重构。
