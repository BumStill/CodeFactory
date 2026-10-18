# M112 + M113 家族：`run_blocked` 的解除者与超时，以及「失败后回一句继续」必须真的能续做

## 背景（真机 2026-10-18）

- M112-1 `fd8fe429`（#723 已合并）：11:13 起 `waiting_system` / `run_blocked`，无任何事件；`close_objective` 被拒「task is currently running」；编排方用 stop 收尾（结果 `completed`）。
- M112-2 `89e69673`（E2E-002）：安装重启后 `process_restarted` → 11:53 `run_blocked` → 11:57 `technical_recovery_exhausted`，且没有 `technical_failure_detail`。
- M113 `89e69673`：失败摘要写「直接回一句『继续』或『把这些改动交付』就可以接着做」；照做后 12:24:50 `agent_loop_error: cannot reconcile a terminal durable delivery run` → 12:26:58 恢复耗尽。文案承诺了做不到的事。
- 同族：U27（失败任务原地续做必撞身份/交付记录问题）、「不许造用户看不懂的中间态」。

## 需求

- R1 用数据写清 `run_blocked` 的含义、谁负责解除、为何进入后无事件又在 4 分钟内耗尽预算；为何已合并 PR 的 objective 仍被视为 running。
- R2 `run_blocked` 必须有明确的解除者与超时：能继续则继续；不能则进入用户看得懂的终态并说明原因与成果位置；不消耗恢复预算地空等或直接耗尽都不行。PR 已合并的 objective 应可结案（`close_objective` 不得报 running）。
- R3 失败后续做：用户/编排方在原会话发「继续」时，若旧交付记录已终态，系统为同一分支新建交付记录并接续（同分支、头可解释即视为同一交付），不再报 `cannot reconcile a terminal durable delivery run`；若确实不能续做，失败摘要不得承诺「回一句继续即可」，应如实给出可行动作（例如一键在新会话接手）。
- R4 测试（先失败）：复现两种 `run_blocked` 序列与「失败→继续→交付」路径；断言续做成功或文案与能力一致。

## Requirements Traceability

| Req ID | 需求 | 最小证据 |
| --- | --- | --- |
| R1 | 说清 `run_blocked` 的语义、解除者、预算耗尽路径与 close_objective 拒绝原因 | 本文件「R1 结论」逐条对应到代码行（`decision_for_run_outcome_with_reason`、`DecisionRouter::route`、`claim_remediations`、`close_objective`） |
| R2 | `run_blocked` 有明确解除者与超时；已合并 objective 可结案 | `agent::objective::tests::m112_*`（决策层）+ `commands::chat::tests::m112_*`（结案层） |
| R3 | 终态交付记录可在同分支续做；文案与能力一致 | `agent::delivery_run::tests::m113_*` + `agent::objective` 失败摘要文案断言 |
| R4 | 先失败后通过的测试 | 测试矩阵；首轮运行中全部 4 个 `m112_*` / `m113_*` 用例失败 |

## R1 结论（代码与数据）

1. **`run_blocked` 是「agent loop 自己停下、且没有类型化原因」的兜底码。** `decision_for_run_outcome_with_reason` 里 `StopReason::Blocked => ("run_blocked", RecoveryDomain::Chat)`，只有 `browser_pairing_required` 和 `permission_denied_by_user` 两个类型化分支被单独处理。因此一次普通的 `Blocked` 会被当成 **Chat 域的临时技术故障**。
2. **它的「解除者」被登记成 objective-supervisor，但 chat 域的 remediation 只是把同一轮再跑一次。** `DecisionRouter::route(TechnicalFailure)` 写 `recovery_owner = objective-supervisor:chat`、`remediation_id = <新 uuid>`、`next_action_authorized = true`、`next_observation_at = now + 5s`。于是 `run_blocked` 进入 `waiting_system` 并**排了一条不改变任何外部条件的重试**：模型拿着同样的上下文、同样的阻塞原因再跑一遍——这正是「进入后无事件」的观感（事件表里只有 `waiting` 决策，没有外部动作）。
3. **4 分钟内耗尽来自签名重试预算，不是来自任何超时。** 每次被 claim 的 remediation 都累加 `signature_attempts`；纯 `Blocked` 的 `failure_signature` 是常量 `"{objective}:Blocked:none"`，因此每次重复都记在同一个签名上，`MAX_SIGNATURE_RECOVERY_ATTEMPTS = 5` 次（5s→20s→80s→320s 退避）后直接进入 `failed / technical_recovery_exhausted`。这段时间的价值是「等一个外部条件变化」，但没有任何东西在等：chat 域没有 capability 恢复信号、没有 lease、没有外部观察。
4. **`close_objective` 报 running 与本记录的真实状态无关。** `close_objective` 先看内存里的 `state.chat_cancels[session_id]`，只要该会话还挂着任何一次 chat run 控制块就拒绝，早于读取 objective 状态。M112-1 里 objective 已经是 `waiting_system` 且 `reached_acceptance='merged'`（一次真正的可结案终态），但控制块还在，于是被一句「task is currently running」挡住——用户看到的是「已合并的任务不能再结案」。
5. **M113 的错误来自「终态记录不能做身份修订」。** `reconcile_delivery_run_identity` 在 `status IN ('completed','failed','cancelled','rejected')` 时直接返回 `cannot reconcile a terminal durable delivery run`；紧接着这条路径的 `UPDATE ... WHERE status NOT IN (terminal)` 也会 0 行。也就是说：旧一次交付已经失败（终态），用户回一句「继续」时，只要 head 有推进，就必然撞在这句错误上，然后同样被 chat 域签名预算拖到耗尽。

## R2 实现决策（Implementation Notes，不改需求）

- `run_blocked` 不再被当成「可以自己恢复的技术故障」。它的解除者是**用户**（或编排方）：决策路由为 `CoreInputRequired`，`requires_user_action = true`，`recovery_owner = None`，`remediation_id = None`，`next_observation_at = None`——不排队、不空等、不消耗恢复预算。用户回一句「继续」即从 `resume_cursor`（root turn）继续，与 R3 的续做路径对齐。
- 类型化的阻塞原因（浏览器配对、用户拒绝权限）保持原有语义，不降级为提问。
- 「能继续则继续」由续做路径承担：`run_blocked` 交接后，用户在**同一会话**里继续即可原地接续，不需要新开任务。

## R3 实现决策（Implementation Notes，不改需求）

- 旧的交付记录已是终态时，同分支的下一次交付**重开同一条 durable 记录为第 N+1 次尝试**（保留 canonical PR、repo/worktree/base/head 身份），写入 `delivery_run_continuation` 事件并推进 `claim_epoch`；不新建一条与 PR 身份割裂的记录，也不放宽终态语义（终态事实留在事件流里）。这满足「同分支、头可解释即视为同一交付」。
- 终态判定的位置从「函数入口直接拒绝」改为「先做身份校验，再按 continuation 处理」，避免把真正的身份冲突（换 repo/worktree/branch）也一并放过。

## Applicable Harnesses

- Spec Harness：本文件。
- Compatibility Harness：`OBJECTIVE` 决策码、`close_objective` 语义对既有会话前端保持兼容（`waiting_core_input` 已存在）；交付记录状态机不新增对外状态名。
- Observation Harness：`run_blocked` 交接后事件流里必须有可读原因与 `resume_cursor`。
- 不涉及 Viewport / Payload / Release Harness：无 UI 布局、无文件载荷、无发版路径变更。

## 测试矩阵

| 用例 | 复现场景 | 断言 |
| --- | --- | --- |
| `m112_blocked_run_hands_off_to_the_user` | 纯 `Blocked`、无类型化原因 | `WaitingCoreInput` + `requires_user_action` + 无 remediation + 无观察时间 + `resume_cursor=root turn` |
| `m112_typed_blocked_reasons_keep_their_owner` | `browser_pairing_required` / `permission_denied_by_user` | 不被降级成提问（原语义不变） |
| `m112_merged_objective_is_closable_while_chat_control_lingers` | 已合并 + 残留 chat 控制块 | 结案成功，不报 `task is currently running` |
| `m113_terminal_delivery_run_continues_on_same_branch` | 终态交付记录 + head 推进 | `Ok`、状态回到非终态、canonical PR 保留、写入 continuation 事件 |
| `m113_identity_conflict_still_refuses` | 终态记录 + 换 branch/worktree | 仍然拒绝（不放宽） |

## 约束

- 合成 fixture，不写生产 DB。
