# Objective Recovery Control Plane 规格

- 状态：已批准实施
- 顶层原则：用户授权目标和边界后，CodeFactory 必须持续持有所有可恢复的技术状态；只有不可安全推导的核心输入，或没有安全默认的不可逆业务决策，才允许中断并回交用户。
- 适用面：chat root turn、task、tool、model route、DeliveryRun、completion projection、App restart。

## Requirements Traceability

| Req ID | Requirement | Minimum evidence |
| --- | --- | --- |
| CF-ORC-R1 | 同一目标跨 root turn、tool future 与 process 保持唯一 `objective_id` | continuation + restart SQLite integration |
| CF-ORC-R2 | `objective/repo/worktree/base/head/change-set/requested ceiling` 身份冲突在任何副作用前 fail closed；revision 只能在 lease 与 receipt 证明后前进 | collision + two-worktree zero-side-effect tests |
| CF-ORC-R3 | system-owned 技术状态不得产生 terminal user CTA；只有 `core_input_required` 或 `needs_business_decision` 可回交 | DecisionEnvelope table tests |
| CF-ORC-R4 | Completion Arbiter 是业务 `completed` 的唯一写入者；transport `Done`、`noop` 或 localized summary 都不是完成证据 | completion decision + projection integration |
| CF-ORC-R5 | startup 与常驻 RecoverySupervisor 在 30 秒内 CAS claim 已失效的 system-owned lease，并先 observe/reconcile | paused-clock claim race + process restart |
| CF-ORC-R6 | `external_state_uncertain` 在证明副作用未发生前只读对账；同一 canonical PR/head/release mutation 至多一次 | fake remote call counters |
| CF-ORC-R7 | 明确 transient provider failure 在 root turn 零输出、零副作用且预算未耗尽时同回合退避或切兼容 route | scripted provider failure -> success |
| CF-ORC-R8 | 已有可见输出、tool intent、tool result、未知副作用、auth/policy/payload incompatibility 时禁止盲重放 | latch/classifier boundary tests |
| CF-ORC-R9 | turn settled、stream closed 与 objective completed 分离；阶段性内容不得投影成业务完成 | hydration + component tests |
| CF-ORC-R10 | 创建 provenance 不可变；最后观察 provenance、queue/runtime、attempt 与 stable failure code 独立记录 | migration + persistence tests |
| CF-ORC-R11 | 旧非终态记录身份不足时只标 `legacy_orphan`，不猜 objective，不执行 provider/tool/remote mutation | legacy migration test |
| CF-ORC-R12 | 下一条真实用户消息到来时才能写 `user_reprompt_driver`，并以仍开放 objective 关联，不做关键词计数 | continuation attribution test |

## Decision envelope

允许的裁决为：`continue | waiting | apply_recommended | platform_incident | failed_internal | core_input_required | needs_business_decision | complete | cancelled`。

- `platform_incident/failed_internal` 仍由系统 remediation 持有，不能要求用户回复“继续”。
- `core_input_required` 必须证明安全替代已耗尽，并把全部缺项合并成一次请求。
- `needs_business_decision` 只用于不可逆且无安全默认的互斥业务结果。
- identity 不唯一、provider 预算耗尽、CI/网络/数据库锁/进程重启都不是用户决策。

## Completion predicate

`Complete(evidence_ref)` 必须同时满足：objective identity 唯一、没有未解决 failure、最后 mutation 后有新鲜验证、agent-owned change-set 与 baseline 可区分、`reached >= requested`，以及 delivery 场景的 canonical PR/head/CI/merge/release/live 证据一致。

## Superseded semantics

- `CF-MRC-R18` 的“恢复后要求用户重发”仅保留给 auth/core input；技术恢复成功后同一 objective 自动续接。
- `CF-SCC-R4/R10` 的 recovery exhaustion 不再形成用户拥有的终态；转入 remediation。
- `CF-SCC-R28` 中 permission timeout 不是用户拒绝；它进入可恢复等待或系统 incident，只有显式拒绝才停止授权链。
- endpoint failover exhausted 的手动重试 CTA 被本规格取代；纯技术耗尽转 system-owned incident。

## Applicable Harnesses

Spec、Compatibility、Observation、Release、AI Collaboration Harness 全部适用。涉及状态 UI 时追加 Viewport Harness；涉及 provider payload 时追加 Payload Harness。
