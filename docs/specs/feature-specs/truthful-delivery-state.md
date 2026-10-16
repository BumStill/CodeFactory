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

## M67b 返修实施记录（CF-TRUTH-R1）

- 复现路径：`decision_for_run_outcome(Finished)` 在空 `CompletionEvidence` 下由 objective 验收仲裁转入 `completion_evidence_incomplete`；旧 `bound_system_recovery` 先创建恢复尝试，直到耗尽后才检查 canonical PR。此前测试只覆盖耗尽后的停放，没有覆盖 merged ceiling 首次等待与反复评估。
- 在预算计算前，按同一 objective 最新交付记录识别 `waiting` / `pr_opened` / `merge_queued` 且已达 `pr_open` / `pr_only` / `merge_queued` 的 canonical PR，立即进入无 owner、无 remediation、无重试的 `delivered_awaiting_ci`。不把已开 PR 当作 merged 或 live 验收。
- 没有具体未满足检查的已交付时序也不消耗恢复预算；尚未交付的真实验收缺口仍保留原有有界恢复。最新记录为 CI `blocked` 时仍交还 agent 修复，不能用旧等待记录遮蔽红灯。
- 超过既有 `SIGNATURE_RECOVERY_WINDOW_MS` 无交付进展时，可读文案明确 PR 编号及实际等待分钟数，不改成任务失败；后台 canonical PR 只读观察继续沿用既有对账路径。
- 合成 SQLite 回归 `m67b_merged_ceiling_wait_never_spends_recovery_attempts`：首次等待先失败（`completion_evidence_incomplete != delivered_awaiting_ci`），修复后连续八次门禁评估、空证据 Finished 路由、外部不确定、交付身份恢复、等待超时、新 CI/队列进展、CI 红灯修复全部断言通过。
- 仅改后端和本规格；不改 workflow、场景注册表或校验器，不读写生产数据库；授权仅到合并，真机安装验收仍由后续发版任务执行。

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
- replay 身份冲突只有在已确认 merged（或更高交付里程碑）时保留只读等待；PR 编号本身不是合并证据，未确认合并的外来身份仍按原有界 fail-closed 门禁收敛到 failed。已有真实停止记录仍保留；live/release 请求仍须真实验收。
- 测试：`m74_remote_merge_repairs_failed_cancelled_and_incident_with_stale_workspace`、`local_takeover_identity_conflict_has_a_stable_bounded_recovery_signature`；原 M71/M72 与严格清理回归保留。
- 本任务不读写生产数据库，不改会话内容，不新增 schema、workflow 或场景注册表。新版真机核对 SQL 见 PR body；本次仅授权合并，不宣称已发布或真机验收通过。

### M74b 返修实施记录（CF-TRUTH-R7/R8/R9）
- 恢复 supervisor 在规划 mutation 接管前执行只读 canonical PR 对账；已 claim 的恢复入口也先观察远端，已合并跳过所有本地 replay 身份校验，不确定则释放租约而不制造故障。
- 合并观察纠正 `failed/pr_open`、`cancelled/ci_green`、`platform_incident/pr_open`、`failed/merged`、`cancelled/merged` 与停止后 `stopped/pr_open`，delivery-only 按 R8 完成；live/release 仍等待真实验收。
- 真实停止围栏保持不变，正向合并后的只读结算不是恢复执行；取消意图不再阻止已交付事实的结算。清理继续走原严格身份与正向合并证据门禁。
- 合成回归 `m74_remote_merge_repairs_failed_cancelled_and_incident_with_stale_workspace` 先看到 `cancelled != completed`，修复后断言六类记录完成、重复观察 revision 不变、恢复规划无接管、停止围栏保留。原 M71 不确定/回退与 live/release 回归保留。
- 无 Spec Amendment；本轮仅合并交付，安装新版后的真机核对由独立发布任务完成。


## M92 返修：交付提交的标题又变成「update N file(s)」（CF-TRUTH-R2 回归）

真机证据（v1.88.4，2026-10-16 21:5x–22:0x）：会话 22a5d188（M90）的分支
`codefactory/objective-d4e455bca04f84213edbe8f8c08ea0ef` 上，提交 36502bd4 的标题是
`fix: update 8 file(s)`；会话 c94e3cfb（M91）的分支
`codefactory/objective-b643646ecabbaef221c5b2393a4c4684` 上，提交 aa3cf5f1 的标题是
`fix: update 3 file(s)`。两个任务的规格里都写明了规范标题（`fix(chat): …`、`fix(delivery): …`），
deliver_changes 却没有用上。合并队列 squash 时取提交标题，这类空泛标题就会原样进入 main。

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| CF-TRUTH-R2（M92-a） | `deliver_changes` 带 title、或任务规格 / PR 意图里有标题时，生成的提交标题必须等于这个规范标题；只有完全拿不到标题时才可回退，回退必须明确报错或要求 agent 提供标题，绝不生成「update N file(s)」 | 失败测试：无标题 + 有改动集时 `canonical_delivery_title` 不得编造文件数标题；修复后返回空并由交付在写提交前 fail closed |
| CF-TRUTH-R2（M92-b） | 交付修订提交（同一工作区追加的改动）使用同一规范标题或带上下文的标题，不能退化成空泛描述 | 失败测试：分支上已有规范标题提交、随后追加「feat: update 3 file(s)」提交，交付仍选中规范标题，且写入的提交说明首行等于该标题 |

根因有两条，都在 `agent::delivery::canonical_delivery_title` 的兜底路径上：

- 兜底会用「分支名人性化 + 暂存文件数」拼出 `fix: update N file(s)`；
- 显式标题的接受条件只看 `conventional_slot` / 非发版前缀，`fix: update 8 file(s)` 本身
  就是合规结构，于是历史提交里的空泛标题会被当成「分支自己的意图」继承到修订提交。

### Implementation Notes（M92）

- 新增 `is_canonical_delivery_title`：要求类型在 `fix`/`feat`/`docs`/`ci`/`test`/`chore`/`refactor`/`perf`/`build`/`style`/`revert` 之中、主题非空、单行，且**不是** `update N file(s)` 占位。显式标题、分支自身提交、回执标题都改用这一判据。
- `canonical_delivery_title` 与 `generate_commit_message_for_paths` 不再编造标题：无可用标题时返回空串，交留在写提交之前的既有 fail-closed 分支（返回「本次交付没有可用标题…」且不执行 stage/commit/push/PR）。`generate_commit_message`、`humanized_branch_subject`、`derived_title_prefix`、`machine_generated_title_noise` 随之删除。
- PR 标题路径未改：它仍优先使用 agent 的意图标题并在 `reconcile_pr_title` 下不夸大 release slot。
- 仅改后端与本规格；不改 `.github/` 工作流、场景注册表与校验器，不读写生产数据库。授权仅到合并，真机安装验收由编排方发版后完成。

## M79 返修：补救回合遇到「会话 / 根回合已变」时不能把任务判死

规格（来自任务说明，逐字记录）：

背景与用户决策：补救回合进行中，同会话来了新的用户消息并开始了新的 root turn，或应用在补救进行中重启时，旧实现把 `PROVIDER_DURABLE_IDENTITY_MISMATCH: remediation session/root changed` 归为 `chat_identity_unreconcilable` 失败终态。真机证据（v1.87.5，会话 7d22dc5c，objective 4946d379）：工作区改动完好、续派后会话还能接着做，用户看到的却是「失败」。

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| M79-R1 | 用合成数据复现两种时序（补救进行中来了新用户消息并开新 root turn；补救进行中应用重启），断言 objective 不以 `chat_identity_unreconcilable` 结束为 failed | 先失败的回归测试 |
| M79-R2 | 新的用户回合到来时旧补救回合让位：作废或合并进新回合，objective 归新回合继续，并如实记录「补救被新的用户指令取代」 | 合成 SQLite 断言 |
| M79-R3 | 重启后由新进程接管：接着跑，或在真不可恢复时写明原因 | 合成回归：绑定仍在的续跑；绑定消失仍然 fail closed |
| M79-R4 | 只有真的换了会话 / 任务时才可判不可恢复，并在会话里写明原因和成果所在位置 | 反例断言 |
| M80-R1 | 排队的消息不能排在一个已经不会再推进的 current_run 后面；`current_run` 已停摆时队列消息直接开始执行 | 前端派单验收 + 后端运行权威断言 |

### Implementation Notes（M79/M80）

- 交接的第一判据从「界面/投影里有活跃回合」改为**持久绑定**：`stale_remediation_is_superseded_by_live_turn` 先按 `objective_bindings(resource_kind='chat_root_turn', resource_id=该 objective 的 live turn)` + 同一 session + objective 非终态判定。投影（`chat_turn_state`）只在没有持久绑定时才作为次判据，因此重启后尚未重建投影、或新用户回合刚刚接管的情形都能交接。
- 反例保留：会话变了、objective 终态、或既无绑定又无活跃回合时，仍返回 fail-closed 的 `PROVIDER_DURABLE_IDENTITY_MISSING`。
- 过期补救 runner 结算时不再 `UPDATE objective_remediations ... WHERE objective_id=?`（那会连带作废新回合的恢复记录），改为按 `id + objective_id + lease_owner + claim_epoch + status='claimed'` 比较并仅作废自己；并把投影根切到 `COALESCE(resume_cursor, root_turn_id)`。该分支同时放宽了「先比 revision」的顺序，使已被用户接管（revision 已前进）的 runner 报告交接而不是抛 stale revision。
- `is_chat_running` 改为 `has_progressing_system_chat_run`：`waiting_system` 且 `next_observation_at` 已过期、也没有有效 claimed 租约时不再算「正在运行」，后台入口 `dispatchSend` 在投递前用该权威标志纠正本地 `streaming` 残留，于是过期排队消息直接开新回合，而不是排在一个不会再推进的 run 后面。
- 未改变 `chat_identity_unreconcilable` 的语义：真的换会话/换任务时仍收敛到该终态并给出可读原因。

- 合成回归（全部临时 SQLite，不触生产数据）：`agent::model_transport::tests::m79_handoff_uses_durable_binding_before_projection_and_after_restart`（先失败）、`commands::chat::tests::m80_expired_wait_does_not_block_but_future_retry_and_claim_do`、`src/lib/dispatchActions.test.ts` 的 M80 用例（先失败）；既有 `a_stale_remediation_after_restart_lets_the_continuation_run` 继续覆盖「绑定消失仍 fail closed + 接管方真的发出模型请求」。
- 不读写生产数据库，不改 `.github/`、场景注册表与校验器。仅授权合并，不宣称已发布或真机验收通过。


## M74c 返修：已合并的交付在真机上仍无法结算（R7/R8/R9 第三轮）

真机 v1.87.4 证据（2026-10-13 13:43:34 对账）：#573、#616、#635 仍写成
`takeover_reconciliation` / `platform_incident`；#637（failed/pr_open）、#638
（cancelled/merged）、#639（failed/merged）、#641（cancelled/pr_open）未变化；#644 本身
已合并却记为 failed/merge_queued。这些 PR 在 GitHub 上全部 MERGED。

只读观察的三处根因与落点（不改变任何需求文本）：

- **合并队列推进 head 后不再拒绝合并证据**：`parse_github_merge_observation` 原先先比
  `headRefOid`，队列在合并前同步分支必然让它与记录的 expected head 不同，于是 MERGED 被
  判成 `HeadChanged`。现在 `state=MERGED` 且带合并提交时直接给出合并证据，未合并的 PR
  仍要求 head 与记录逐字一致（`HeadChanged`/`Queued` 语义不变）。
- **只读观察不再依赖工作区存在**：`gh pr view` 增加 `-R owner/repo`，`RemoteCanonicalPrObserver`
  在记录的仓库根不可解析时回退到进程目录，因此被 closeout 清掉工作区的记录也能被观察。
- **终态记录重新进入观察范围**：`reconcile_parked_delivery_progress` 的候选查询不再要求
  `worktree_identity`/`head_sha` 非空（改用 `COALESCE(...,'')` 并显式别名），并把单轮上限
  从 16 提到 64，使历史终态（failed/cancelled/platform_incident 且工作区状态已结束）仍被
  观察；结算仍要求正向合并证据与既有的 fail-closed 谓词（claim_epoch、canonical PR 号、
  无有效 lease、objective/run 状态）。

合成回归（临时 Git 仓库 + 合成观察，全部不触生产数据）：

- `agent::delivery::tests::m74c_merged_queue_head_is_positive_evidence`：先失败（`HeadChanged`
  而非 `Merged`），修复后通过。
- `agent::execution_workspace::tests::m74c_settles_merged_pr_after_the_queue_moved_the_head_and_the_workspace_is_gone`：
  head 被队列推进 + 工作区身份被清空 + 记录为 failed/merge_queued，观察 MERGED 后
  objective 结算为 completed、记录为 completed/merged。
- 既有 `m71_*`、`m72_*`、`m74_*`、`externally_closed_unmerged_pr_keeps_everything_and_says_where`
  全部保持通过（工作区 42 项、交付 134 项）。

反复观察仍幂等：同一合并证据重复观察不再推进 revision；不确定（Unknown/更早状态/已关闭
未合并）保持原状（R9）。

## M8 / M28 / M84：验收口径与等待结案

批准任务要求（来自 task-ui-truth-m8-m28.md）：

1. requested_acceptance 必须与授权的交付上限一致。授权 ceiling=merged 时，验收口径就是 merged（delivery 回执），合并即完成；只有用户或编排方明确要求真机验收时，才是 live_verification。
2. 修正推断后，对历史记录做一次幂等的重新结算：授权 ceiling=merged、只是被误推断为 live_verification 的任务，按 merged 完成。界面和本地派单入口新增「确认验收 / 结案」操作，按 CF-CLOSEOUT 结算并写审计。
3. 只有确实在运行的任务才转圈。等人的任务用静态标识并写明等待内容。顶栏 PR 状态以 delivery_runs 最新记录为准并及时刷新；横幅不显示内部计时；没有运行的会话不显示停止按钮。
4. 组件测试、headless 截图与结算集成测试使用合成数据；不改 workflow、场景注册表或校验器，不读写生产数据库，历史纠正由应用运行时结算。

### Implementation Notes

- 根因：正文 `contains("live")` 命中 `delivery` 路径。明确验收要求与 merged 授权分别决定 kind 与 requested_acceptance。
- 历史纠正使用原始 root 请求、持久授权、canonical PR 正向合并观察；缺来源保留，claim/head/PR/lease 围栏不放宽。
- close_objective 只接受明确 accepted，要求 merged 等待或幂等完成；同事务写审计与 evidence_ref，清理仍走 CF-CLOSEOUT 包含性门禁。
- 证据与命令见 docs/testing/m8-m28-m84-acceptance.md。

## M76 / M77 / M81 返修：交付不再挂起无人推进

背景与用户决策（真机证据，2026-10-13，v1.87.4 / v1.87.5）：

- **M81**：两个会话都已在本地提交，调用 `deliver_changes` 后返回「DeliveryRun fencing permit was lost
  before `git_stage` … epoch 42 … The stale owner issued no later mutation」，随后进入
  `waiting / external_state_uncertain`「只读核对同一远端对象」。远端既没有分支也没有 PR，工具自己也说
  「不曾发生写操作」，却挂了 20 分钟，没有任何人或流程推进。
- **M77**：ceiling=merged 的交付 CI 已全绿，`delivery_runs` 停在
  `complete | awaiting_completion_arbitration | ci_green`，`merge-queue: armed` 始终没打上，最后由编排方手动打标签。
- **M76**：PR 正文被传成字面字符串 `$(cat .codefactory-cache/m74c-body.md)`，deliver_changes 既没有展开也没有拒绝；
  正文文件里写的 `Scenario-Test: none` 也是模板明令禁止的写法。

## Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| CF-TRUTH-R10 | 交付在某个 mutation rung 前丢失 permit、且能确认「旧持有者没有发出写操作」时，不得直接进入 `external_state_uncertain` 等待：必须在有界次数内重新取得授权，并从**同一个 rung** 续接同一次交付；只有有界次数用尽才如实报告失败 | 合成测试：permit 先失败后恢复的时序下，同一 rung 的交付继续（断言返回 Ok，且两次验证都发生在同一个 rung）；次数用尽时返回原有 fail-closed 文案 |
| CF-TRUTH-R11 | `ceiling` 为 through_merge（merged）或更高时，CI 全绿后必须由交付流程自己为 PR 打上 `merge-queue: armed`，不能停在 completion arbitration 等别人来做；打标签失败时在有界次数内重试，并在重试耗尽后如实报告「尚未交给合并队列」 | 纯判定函数断言 + 有界重试驱动：前两次失败、第三次成功时返回第 3 次尝试；全部失败时报告文案含 PR 号、标签名与「尚未交给合并队列」，且调用次数等于预算 |
| CF-TRUTH-R12 | `deliver_changes` 收到的 PR 正文里如果含未展开的 shell 片段（`$(`、反引号命令替换），或缺少模板 §5 必需字段（`Scenario-Test` 不能是 `none`/散文，`README-Update` 为 `required`/`reviewed` 且带非空 `README-Update-Reason`），直接拒绝并说明应该怎么写；不得用这种正文开 PR | 合成测试：M76 的真实正文先失败（断言返回 Err 且文案给出改法），逐条字段缺失与 shell 片段各一条断言；合规正文通过 |

## Applicable Harnesses

Spec Harness；Compatibility Harness（既有交付阶梯、durable 记录与 stop fence 语义不变）；
Observation Harness（重试次数、打标签结果与失败原因都可追溯）；AI Collaboration Harness。

## 测试矩阵

- permit 丢失：第一次验证即失败后恢复（成功续接同一 rung）／连续失败到预算用尽（如实失败）。
- 打标签：owed 与 not-owed 的 ceiling 组合（pr_only / through_ci_green 不 owe；through_merge 起 owe）；
  重试在第 2、3 次成功；全部失败（文案 + 调用次数）。
- 正文：未展开 `$(`、反引号、`Scenario-Test: none`、缺 `README-Update`、缺 `README-Update-Reason`、
  合规正文（对照组）。

## Implementation Notes

以下只记录实现选用的技术位点，不改变任何需求或验收标准。

- **R10 的落点**：`DeliveryMutationPermitVerifier::reacquire`（默认实现：在
  `DELIVERY_PERMIT_REACQUIRE_ATTEMPTS` 内重新调用 `verify(rung)`，每次都是对 durable 存储的重新读取），
  `DeliveryMutationPermit::reacquire` 转发，`verify_mutation_permit` 改用它。成功即从原 rung 继续；
  成功路径不改变既有 verify 调用次数（仍然只验证一次），因此不改变既有 fail-closed 语义。
- **R11 的落点**：`merge_queue_arm_is_owed`（`ceiling.rank() >= ThroughMerge.rank()`）、
  `arm_merge_queue_with_bounded_retry`（通过小型 `MergeQueueArmer` 抽象驱动
  `DeliveryRemote::arm_merge_queue`，便于合成测试），以及 `merge_queue_arm_failure_report`。
  接线点在 CI 通过之后、merge 阶梯之前，使用的是 `outcome.requested_ceiling`（用户请求的上限），
  因此即使 effective ceiling 因能力缺口被降级，已请求 merged 的交付仍会自己把绿 PR 交给队列。
  `merge_pr` 内原有的 best-effort 打标签保留（幂等）。
- **R12 的落点**：`validate_pr_body_intent`，接线点在 `requested_pr_body` 计算之后、任何远端 mutation 之前。
  只校验调用方显式给出的正文；未提供正文（沿用自动生成文案）不受影响。工具不代替调用方计算
  Scenario-Test ID（校验器仍在 `tools/governance/`，本次不修改它），而是在拒绝文案里给出该运行的命令。
- 仅改后端与本规格；不改 `.github/` 工作流、场景注册表与校验器，不读写生产数据库。授权仅到合并。
