# 架构

## 前端

```text
components → hooks → TanStack Query → typed IPC client
```

- Components 只负责展示与交互。
- Hooks 组织页面业务流程。
- TanStack Query 管理服务端状态缓存。
- 长任务进度通过 Tauri Channel/Event 传递。

## 后端

```text
commands → services → domain → repositories → infrastructure
```

- Commands：输入验证、调用服务、DTO 转换。
- Services：用例编排和事务边界。
- Domain：状态、规则和决策。
- Repositories：持久化接口。
- Infrastructure：PostgreSQL、文件系统、图像实现。

## 并发

- 扫描和指纹任务使用受控任务池。
- 数据库写入通过事务完成。
- 同一导入运行的提交操作串行执行。
- 长任务支持取消令牌和进度事件。

## 应用状态

Tauri State 只持有长期服务：

- DatabaseManager
- TaskManager
- SettingsStore
- RepositorySet

页面临时状态不进入全局 Rust State。
