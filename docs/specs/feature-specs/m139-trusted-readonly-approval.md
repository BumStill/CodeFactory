# M139：信任模式下只读组合命令仍弹审批；等待审批时 thin 状态仍显示 running

## 证据（v1.91.11，含 M138）
- 会话 e781e4a2（permission_mode=trusted，只读演练）：重启后续跑，核对现场命令 `git status --short; git log -1 --oneline; gh pr view --json number,state,title; ps -eo pid,etime,args | grep '[s]leep 60'` 不再被拒（M138 生效），但生成了审批请求（approval 3a8a7ef7），等了 15 分钟直到编排方批准。
- 同期 thin_session_state = `running`（无问题、无等待说明）——用户界面看不出会话在等人审批。
## 需求
- R1 信任模式下，被 M138 判为只读的命令（逐段均为只读）不需要审批，直接执行；标准模式下只读命令也应免审批（只读不改变任何东西）。查明为何 gh/ps/管道组合进入 ask 分支并修正；写入类命令的审批规则不变。
- R2 derive_state：存在待处理的权限审批 → `waiting_user`，问题文本为审批摘要（例如「需要你批准执行：<命令摘要>」），审批处理后回到 running。
- R3 测试（先失败）：信任模式只读组合命令 → 无审批直接执行；标准模式写命令 → 审批且状态 waiting_user；批准后 → running 并继续。
## 约束
- 安全：写入判定与高风险命令策略不放宽。授权交付（ceiling: merged）。不要超出规格自加范围。

## Implementation Notes
- Spec、Compatibility、Payload、Observation、AI Collaboration Harness：通过真实权限网关与 SQLite 状态集成测试覆盖；无布局修改。真实安装版验收由编排方发版后完成。
- 测试映射：R1/R3 `m139_readonly_compositions_skip_approval`；R2/R3 `m139_pending_approval_overrides_live_executor_and_resumes`。
- 保留标准模式已有写入策略，只对确证只读的远端查询消除交付误判，不将 M138 的未知命令放行规则当成安全证明。
