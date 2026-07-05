# Acceptance Checklist

## M6.5 托管数据库

- [ ] Release 包含 PostgreSQL runtime。
- [ ] Release 包含 pgvector 扩展文件。
- [ ] 应用通过 resource_dir 找到 runtime。
- [ ] 干净 Windows 环境无需系统 PostgreSQL。
- [ ] 首次启动初始化托管数据库成功。
- [ ] `CREATE EXTENSION vector` 成功。
- [ ] migration 成功。
- [ ] 缺 runtime 时 fail-fast。
- [ ] 缺 runtime 时错误文案为“安装包不完整”。

## 真实测试门禁

- [ ] `pnpm rust:test` 不依赖 runtime。
- [ ] `pnpm rust:test:real` 缺 runtime 失败。
- [ ] managed db lifecycle 不 skip。
- [ ] public main chain 不 skip。
- [ ] recovery/fail injection 不 skip。

## M7 外部 PostgreSQL

- [ ] database service 使用 TLS connector。
- [ ] NoTls 只在明确 disable TLS 时使用。
- [ ] 支持 require / verify-ca / verify-full。
- [ ] 版本预检。
- [ ] pgvector 预检。
- [ ] 权限预检。
- [ ] UI显示诊断。

## M9 Frozen Plan 主链

- [ ] Review 调用 freeze。
- [ ] plan 三张表持久化。
- [ ] plan_hash 持久化。
- [ ] run 进入 ready_to_commit。
- [ ] Commit 页读取 frozen summary。
- [ ] Commit Service 使用同一 frozen plan。
- [ ] Commit 页不动态重算计划。
- [ ] frozen 后修改审核数据不影响提交集合。
- [ ] 重复 freeze 幂等。

## 可提交 Run 查询

- [ ] ready_to_commit 优先。
- [ ] completed 不抢占默认提交页。
- [ ] recovery_required 进入恢复页。
- [ ] cancelled 只有可重新提交时才进入提交页。

## M8 挂载目录

- [ ] 读写权限探测。
- [ ] rename 探测。
- [ ] 大小写行为探测。
- [ ] Unicode 路径探测。
- [ ] 长路径探测。
- [ ] 断连/权限错误不误报成功。

## 最终门禁

- [ ] format 通过。
- [ ] typecheck 通过。
- [ ] frontend tests 通过。
- [ ] rust tests 通过。
- [ ] clippy 通过。
- [ ] real tests 通过。
- [ ] build 通过。
- [ ] release gate 通过。
- [ ] Windows exe 存在。
- [ ] 最终报告写入 reports。
- [ ] 所有改动本地提交。
- [ ] 未 push。
