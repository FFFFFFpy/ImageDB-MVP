# 测试

## 前端

- Vitest
- React Testing Library
- MSW 模拟 Tauri IPC 客户端

## Rust

- Domain 单元测试
- Service 用例测试
- PostgreSQL 集成测试
- 文件系统集成测试
- 图像基准测试

## 图片基准集

`fixtures/` 至少覆盖：

- 完全相同
- 改名
- Metadata 变化
- 重新压缩
- 格式转换
- 缩放
- 旋转
- 镜像
- 小水印
- 相似但不同
- 损坏图片

## 故障注入

必须覆盖：

- 复制过程中退出
- 校验后退出
- 发布后数据库提交前退出
- 数据库提交后源归档前退出
- 目标目录冲突
- 目标目录不可用
- 数据库暂时不可连接
