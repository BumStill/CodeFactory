# Feature Spec: 交付后的状态如实（CF-TRUTH）

## 背景与用户决策
用户依据会话状态判断任务成败，状态必须如实。已经交付的不能显示失败，系统内部原因不能说成是用户操作，提交历史要能被发版流程正确识别。

## Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| CF-TRUTH-R1 | 按 ceiling 开出 PR 后（pr_only 已达成），完成门禁不得因为「无法确认检查已重跑」而把 objective 推进到恢复耗尽和失败。会话结果显示「PR #N 已开出，等待 CI」这类真实状态；CI 结果后续由交付记录反映 | 集成测试：复现「PR 已开 → 门禁找不到未满足检查」，断言 objective 不是 failed，结果文案如实 |
| CF-TRUTH-R2 | 交付产生的提交，标题使用与 PR 相同的规范标题（fix / feat / …），trailer 放在最后一段；不再出现 `objective <id>` 这类标题。只有一个提交的 PR 在 squash 合并后，main 上的标题也必须能被 plan_release 识别 | 测试：交付提交说明格式；用 plan_release 对合成提交历史验证能识别 |
| CF-TRUTH-R3 | cancellation_provenance=explicit_cancel 只能由真实的用户或编排方停止请求产生（有对应的取消意图记录）；系统内部失败（failed_internal、PROVIDER_OWNER_FENCED 等）必须记成系统原因，界面文案不得说成用户取消 | 测试：两条复现时序，断言 provenance 与文案 |
| CF-TRUTH-R4 | 归属被切走（owner fenced）时，不能让正在跑的任务直接终止：要么由新的归属方接着完成，要么明说「被系统打断，可继续」，保持可恢复 | 测试：fenced 后任务能继续，或给出明确的可继续状态 |

## Applicable Harnesses
Spec Harness；Compatibility Harness（已有的 objective 和交付记录）；Observation Harness（provenance 和结算可追溯）；Release Harness（R2 影响发版识别）；AI Collaboration Harness。

## 测试矩阵
- pr_only 交付成功后，门禁无记录、门禁有记录、CI 失败三种情况。
- 单提交 PR 与多提交 PR 的提交标题。
- failed_internal 后取消、owner fenced 后取消、用户真实停止（对照组）。

## CF-TRUTH-R5 / R6：交付身份冲突

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| CF-TRUTH-R5 | 查清 identity collision 在这两次真实时序里是哪一项身份变了（objective、worktree 还是 change-set）、为什么会变（续跑、恢复、合并 main、重新暂存等），用合成数据复现 | 失败测试复现两条时序 |
| CF-TRUTH-R6 | 合法的变化（同一任务在同一工作区里提交了新改动、合入了 main、或者恢复后接着跑）不算冲突，交付继续并接上同一次交付；只有真的换了任务或换了工作区才拒绝，并用大白话说明哪一项变了、怎么办 | 测试：合法变化继续交付；真冲突拒绝，并给出文案 |

### Implementation Notes（R5/R6）
- 生产只读证据：53686537 的记录已前进到提交并创建 PR；后续 `through_ci_green` 调用与持久化 `through_merge` 不同。它把等待检查的本次上限误作任务身份冲突，不是 objective/worktree 改变。
- b1c47692 的记录停留在原 HEAD，隔离 index 规划检测到 index/checkout 变化；随后恢复把同 HEAD 的 change-set 变化判作 identity conflict。该场景必须用真实 Git 暂存状态与 SQLite 恢复复现，不能把摘要当作固定身份。
- 安全边界仍保留 objective、repo、worktree、workspace、分支和任务来源；一次调用降低 ceiling 不改变既有授权上限、不升级权限。
- M70 落点：`create_delivery_run` 在 `status='waiting'` 的停放记录上，若外部持有者的 lease 已过期，或该记录已有一个完整 lease 周期没有任何进展（`last_progress_at` 早于 `lease_ttl`），则视为**停滞的等待持有者**而非在飞调用：前台按 `stale_waiting_holder` 回收 claim（epoch 递增、写入 `lease_takeover` 事件、保留 canonical PR），并在有界时间内接管同一交付继续入队；真正在飞且持续进展的 mutation lease 仍按原规则 fail closed。

## Implementation Notes

以下只记录实现选用的技术位点，不改变任何需求或验收标准。

- **R1 的落点**：`ObjectiveStore::bound_system_recovery` 是唯一把「恢复预算耗尽」写成
  `technical_recovery_exhausted` 失败终态的地方。当耗尽的原因是该 Objective 的
  `completion_evidence_incomplete`（完成门禁拒绝确认检查），且该 Objective 已有 canonical PR
  时，改为写入新的**非失败**等待：`failure_code=delivered_awaiting_ci`、
  `decision_type=waiting`、无 owner、无重试、无新的 observation，可读文案（持久化在
  `attention_request.prompt`）写明「改动已交付到 PR #N，等待检查结果」。
- **「非失败停放」是独立概念**：新增 `DecisionEnvelope::is_parked_delivery_wait()`，与既有的
  `is_parked_system_incident()`（语义是「真实的失败终态」）刻意分开，因为后者决定是否写失败报告。
  `validate()` 与 `apply_decision` 的排队分支都显式承认这个停放态。
- **R2 的落点**：`agent::delivery::canonical_delivery_title()` 在写提交之前算出唯一规范标题，
  提交说明与 PR 标题使用同一个字符串。显式给定的规范标题原样保留；否则回溯分支自身提交中
  最高 slot 的规范标题；再否则按暂存路径派生保守前缀（`fix`/`docs`/`ci`/`test`，都不夸大
  发版 slot）。`generate_commit_message*` 的兜底也不再产生非规范标题。
- **R3 的落点**：`ObjectiveStore::has_durable_stop_request()` 以
  `chat_session_cancel_intents` 为唯一权威；`consume_pending_chat_cancellations` 的候选行增加
  `EXISTS(...)` 约束；`apply_chat_objective_outcome` 在写取消前先查该记录。真实停止路径
  （`request_chat_session_cancel`）本来就先写意图行，因此真实停止不受影响。
- **R4 的落点**：新增 `decision_for_interrupted_run()`，把「没有持久停止请求却停下」的 run
  映射为可恢复的系统等待，failure_code 分别为 `provider_owner_fenced`（`terminal_reason`
  含 fence）或 `interrupted_by_system`。
- **会话可见状态**：`chat_turn_projection` 对 `waiting_system + delivered_awaiting_ci` 输出
  `finalizing / delivery_awaiting_ci / 改动已交付，等待检查结果`，`is_chat_running` 不再把它
  算作仍在运行。
- **与 U1b 的关系**：U1b 把「已交付但检查未确认」写成失败终态并渲染专门的失败报告；
  CF-TRUTH-R1 判定该场景不应成为失败，故该终态在此路由下被停放态取代，失败报告渲染逻辑本身
  保留给其它失败路由。既有测试按新要求更新，未交付仍保留诚实失败文案作为对照。

## M72 返修：后台审批保持归属，隔离不得伪装用户停止

背景：本地入口批准一条 bash 审批后，provider 报 owner fenced，旧结算把无停止意图的会话记成 explicit_cancel；此前 PR 已合并。

本次沿用 CF-TRUTH-R3/R4 与 CF-CLOSEOUT，不改变需求：
- 当前进程活跃审批只持久化决定，不递增 objective revision、不创建 remediation、不切换 lease owner；真实 orphan/restart 仍走原恢复路径。
- owner-fenced 在中央 outcome 路由优先映射可恢复系统中断；chat run control 的取消消费必须查到持久停止意图，真实停止入口先写意图再写 control。
- 无真实停止意图的历史误取消仅在 canonical PR 正向合并证据下对账，delivery-only 按 merged 完成；live/release 仍不得伪造验收。清理沿用原安全 closeout。

验收使用独立 SQLite 与临时 Git 仓库，全部合成数据。测试先失败后修复：`m72_entry_allow_keeps_the_current_owner_and_revision`、`m72_fenced_run_control_without_stop_intent_is_not_cancelled`、`m72_misclassified_cancel_with_merged_pr_is_completed`、`an_unrequested_interruption_is_recoverable_and_names_its_cause`。完整 Rust workspace、前端测试与构建均执行；不修改真实会话或生产数据库。

## M71 返修：接管对账失败后，交付记录与 objective 必须跟上真实进展

背景（2026-10-12，会话 0a2cfd7a，v1.87.1）：接管对账因未决 mutation intent 失败后，记录被写成
`takeover_mutation_reconciliation|platform_incident|pr_open`，objective 停在
`waiting_system + run_blocked`；随后同一会话通过其它步骤真的开出了 PR、打上 `merge-queue: armed`
并被合并队列合并，但这两处状态从未更新，界面因此显示成失败的内部故障。

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| CF-TRUTH-R7 | 接管对账失败只表示「这一次无法用 replay 证明外部副作用」，不得写成 platform_incident；并且后台只读对账必须独立于 mutation 接管，按真实观察把交付记录推进到 `pr_open` / `merge_queued`（合并队列已 armed） | 合成时序测试：接管失败 → 观察到 PR 已开 → 已入队，断言记录 stage/status/reached_ceiling 与 objective 的 decision/failure_code 都如实 |
| CF-TRUTH-R8 | PR 合并后按 CF-CLOSEOUT 结算：记录 `merged`、objective 按请求的验收口径结算（delivery-only 验收结算为完成；live/release 验收只是记录 merged，不得假装通过），并只在有正向合并证据时才清理工作区 | 合成时序测试：`pr_only` 请求合并后结算 + 工作区按既有安全 closeout 关闭；`through_release` 请求合并后仍等待真实 live 验收且不删除任何东西 |
| CF-TRUTH-R9 | 观察不确定（离线、Unknown、head 漂移、已关闭未合并）不得制造故障、不得回退已确认的入队/合并状态 | 合成测试：同一观察重复不再推进 revision；Unknown/更早状态不改变已确认的 reached_ceiling |

### Implementation Notes（R7–R9）
- 只读对账落在 `agent::execution_workspace::reconcile_parked_delivery_progress`，由既有
  closeout supervisor（`run_cleanup_pass_with` + `RemoteCanonicalPrObserver`）驱动，不经过
  mutation 接管，也不需要任何写权限；对 lead 的 `delivery_runs` 以 fail-closed 谓词更新
  （claim_epoch、expected/canonical head、PR number 必须一致，且无有效 lease）。
- `CanonicalPrState` 增加 `Queued`：`merge-queue: armed` 标签 + 同一 head 才算入队，
  由 `GhCliRemote::observed_queue_arm` 只读观察。
- 历史/最小 schema 兼容：目标表列按 `pragma_table_info` 探测后再拼写 UPDATE，缺列即不写该列。
- R8 的验收口径只认 `objectives.kind` 与 `delivery_runs.requested_ceiling`：`live` 或
  `through_release` 等更高请求不会被 merge 观察直接判成完成。

### M74 返修实施记录（CF-TRUTH-R7/R8/R9）
- 合成 SQLite + 临时 Git 仓库复现历史 `failed` / `cancelled` / `platform_incident`，包含本地身份漂移与工作区已关闭/清理；先看到 `failed != completed` 断言失败。
- 只读 canonical PR 查询不再以工作区当前 identity/head 作为观察前提；清理仍使用原严格身份校验，远端观察不会授予 mutation 权限。
- 后台纳入历史失败、错误交付终态与 closed/incident 工作区。仅正向合并证据可纠正历史终态；离线、Unknown、关闭未合并与更早状态不回退，重复观察不递增 objective revision。
- replay 身份冲突只写等待，不再写平台故障或按重复 replay 失败耗尽 objective。已有真实停止记录仍保留；live/release 请求仍须真实验收。
- 测试：`m74_remote_merge_repairs_failed_cancelled_and_incident_with_stale_workspace`、`local_takeover_identity_conflict_has_a_stable_bounded_recovery_signature`；原 M71/M72 与严格清理回归保留。
- 本任务不读写生产数据库，不改会话内容，不新增 schema、workflow 或场景注册表。新版真机核对 SQL 见 PR body；本次仅授权合并，不宣称已发布或真机验收通过。

