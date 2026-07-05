# Milestone 0：技术探针

## 目标

验证完整技术链路可以在桌面应用中工作。

## 实现

1. 建立可运行的 React + Tauri 最小应用。
2. Rust 启动一个应用私有 PostgreSQL 实例。
3. 初始化独立数据目录和随机本地端口。
4. 创建数据库并启用 pgvector。
5. 应用关闭后停止数据库，重新打开后复用已有数据。
6. Rust 解码 JPEG、PNG、WebP 样本。
7. 计算 BLAKE3、标准像素 Hash 和三种感知 Hash。
8. 实现一个本地文件事务探针：复制到 staging、校验、发布、记录结果。
9. GUI 展示数据库、图片指纹和文件事务探针结果。
10. 在 Windows 和 macOS 完成开发构建验证。

## 产出

- 托管 PostgreSQL 生命周期模块原型
- 图像指纹模块原型
- 文件事务模块原型
- 最小 GUI
- 探针测试报告 `reports/milestone-0.md`

## 验收

```bash
pnpm typecheck
pnpm test:unit
pnpm rust:test
pnpm rust:clippy
pnpm build
```

真实验证：

- 首次启动自动初始化数据库。
- 第二次启动读取第一次写入的数据。
- pgvector 健康检查通过。
- 三种格式图片生成稳定指纹。
- 文件事务发布后的目标文件 BLAKE3 与源文件一致。
- 强制终止后不会产生虚假的成功状态。
