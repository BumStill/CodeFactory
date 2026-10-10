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
