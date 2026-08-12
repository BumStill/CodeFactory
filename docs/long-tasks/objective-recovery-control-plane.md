# Objective Recovery Control Plane 长任务记录

- Task ID: CF-ORC-20260811
- Feature spec: `docs/specs/feature-specs/objective-recovery-control-plane.md`
- 当前阶段：规格与契约已收敛，failure-first 实现进行中；尚未进入完整验收或交付
- 完成标准：CF-ORC-R1..R21 的反向追踪、focused/full tests、跨进程故障注入、CodeFactoryDev 成功与边界路径、PR/CI/merge、正式 release、安装版复验与 24 小时生产指标全部成立。
- 停止边界：只有不可替代核心输入、无安全默认的不可逆业务决策、显式拒绝或取消才可停在用户边界；普通技术失败、恢复耗尽和外部状态不确定进入系统 remediation/只读对账，不回交用户。

## Checklist

- [x] 正式版历史摩擦链、当前 origin/main、开放 PR/release 状态与重复实现复核。
- [x] 独立规划、架构/UX/测试策略审视；明确 identity/provider/permission/task/auth/release 的用户回交语义。
- [x] 业务、架构、UX、测试策略与 CF-ORC-R1..R21 权威规格。
- [x] 旧领域规格与设计的权威顺序、人工技术恢复语义和真实完成状态收敛。
- [ ] Failure-first：identity collision、非法状态转换、Completion Arbiter、continuous supervisor、provider/permission/task/auth/release latches；先观察红测。
- [ ] `objectives`/decision/remediation 持久层、CompletionArbiter、RemediationSupervisor 与各 domain adapter 实现和迁移。
- [ ] focused/full：Rust、前端、workflow/semantic governance、build、迁移与回滚验证。
- [ ] CodeFactoryDev：成功路径、所有技术故障边界、显式拒绝/取消、核心输入/业务决定和跨重启故障注入。
- [ ] PR/CI/merge/release/formal App。
- [ ] 正式版 24 小时指标：avoidable reprompt、system-owned handoff、ownerless past lease、duplicate side effect、false complete、requested-ceiling downgrade 全为 0。
