# 开发环境与验证命令

当前阶段：MVP1 功能完成，进入 Debug / 实战测试。

当前文档入口：[`docs/MVP1/README.md`](docs/MVP1/README.md)

Codex 桌面版可以直接选择本项目目录作为工作目录。Codex CLI 是可选工具，不属于开发环境前置条件。

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

环境检查报告会写入根目录 `reports/`。该目录现在只作为脚本输出目录；历史报告已归档到 `docs/MVP1/archive/reports/`。

## 基础环境

基础环境必须通过：

- Git
- Node.js 22 或 24
- pnpm 10.34.4
- Rust stable
- rustfmt
- clippy
- 平台原生编译工具
- Tauri 系统运行组件

系统中预先安装的 PostgreSQL 只作为信息项，不是技术探针的前置条件。MVP1 默认应使用应用托管 PostgreSQL runtime。

## 常规验证

```bash
pnpm format:check
pnpm typecheck
pnpm test:unit
pnpm rust:test
pnpm rust:clippy
```

## 真实 PostgreSQL 验证

具备 PostgreSQL runtime 时运行：

```bash
pnpm rust:test:real
```

## Windows release 验证

```bash
pnpm build
pnpm release:verify-artifacts
pnpm release:install-gate
```

## 完整发布签字

```bash
pnpm release:gate
```

注意：完整 clean Windows `pnpm release:gate` 仍是正式发布签字项。Debug 阶段可以先跑实战测试，但不要把未跑过的 gate 写成已通过。
