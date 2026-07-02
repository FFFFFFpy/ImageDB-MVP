# ImageDB MVP

ImageDB 是一个本地优先的桌面图集整理应用。

核心流程：

```text
选择源目录
→ 扫描图集
→ 图集内部重复与相似检测
→ 与历史图库比较
→ 人工审核不确定候选
→ 写入目标图库
→ 完整性校验
→ 数据库确认入库
```

## 技术栈

- React + TypeScript + Vite
- Tauri 2 + Rust
- PostgreSQL + pgvector
- 应用默认管理私有本地 PostgreSQL
- 高级模式支持连接外部 PostgreSQL

## 开始开发

```bash
pnpm install
pnpm dev
```

常用命令：

```bash
pnpm typecheck
pnpm test:unit
pnpm rust:test
pnpm rust:clippy
pnpm build
```

Codex 开始工作前依次阅读：

1. `AGENTS.md`
2. `CURRENT_TASK.md`
3. `PROJECT_PLAN.md`
4. 当前任务引用的文档

## 环境检查

Windows：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\check-env.ps1
```

macOS：

```bash
bash ./scripts/check-env.sh
```
