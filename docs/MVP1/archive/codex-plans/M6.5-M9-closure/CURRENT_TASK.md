# Current Task: M6.5–M9 Closure

## 当前目标

收口 M6.5–M9 已存在框架，不扩张新需求。

当前分支：`core_fix_m5_m6_refactor`

## 必修项

1. M6.5：打通托管 PostgreSQL + pgvector runtime 的 Windows release 打包和定位。
2. 测试门禁：真实测试缺 runtime 时必须失败，不得 skip 后通过。
3. M7：外部 PostgreSQL 服务主链使用 TLS connector，不再绕回 NoTls。
4. M9：Review → Frozen Plan → Commit 公开主链一致，提交页读取 frozen plan。
5. 查询：可提交 run 不能被旧 completed run 抢占。
6. 文档：tasks/reports 与真实状态一致。

## 非目标

- 不做新算法。
- 不做 SMB 协议。
- 不做跨平台 runtime。
- 不重写事务系统。
- 不追求完美 CI。
- 不继续新里程碑。

## 完成标准

见 `checklists/ACCEPTANCE_CHECKLIST.md`。
