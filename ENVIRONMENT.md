# 开发环境检查

## Windows

在项目根目录打开 PowerShell：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\check-env.ps1
```

执行完整构建验证：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\check-env.ps1 -BuildProbe
```

## macOS

在项目根目录打开终端：

```bash
bash ./scripts/check-env.sh
```

执行完整构建验证：

```bash
bash ./scripts/check-env.sh --build
```

检查报告保存在 `reports/`。

基础环境必须通过：

- Git
- Node.js 22 或 24
- pnpm 10
- Rust stable
- rustfmt
- clippy
- 平台原生编译工具
- Tauri 系统运行组件

系统中预先安装的 PostgreSQL 只作为信息项，不是技术探针的前置条件。
