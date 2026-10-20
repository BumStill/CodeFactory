# 减法迁移 · 阶段 2：thin 会话状态收敛（四态 + 一条完成规则 + 等待必须有执行者）

## 背景
- 用户 10-20 19:2x 拍板进入阶段 2。阶段 0 清单：/Users/leo/.codefactory/agent-tools/simplify-phase0-inventory.md（§3 objective.rs / objective_supervisor.rs 处置，§4 表处置）。阶段 1 规格：task-simplify-phase1.md。
- 阶段 1 真机暴露的问题几乎都在「完成判定 / 等待」：P0-1 等待被当失败；P0-3 只读任务结束不了；P0-4 拒收后重复答案 6 次（修后仍重复 2 次）；M126 会话自造范围后挂监视「等确认」——等一个不会来的执行者；thin 仍复用 legacy 完成门禁（completion_evidence_incomplete、run_blocked、agent_loop_error 等 legacy failure_code 仍出现在 thin 会话上）。

## 需求（仅 delivery_mode=thin；legacy 不动，阶段 3 删除）
- R1 四种状态，替代 thin 会话上所有 legacy objective 状态/failure_code：
  - `running`；
  - `waiting_wake`：必须绑定至少一个**唤醒源**（运行中的 background_task、未来的 wakeup_schedule、或已登记的外部事件如「PR #N 合并/CI 结束」+ 轮询者），并记录「在等什么」一句话；
  - `waiting_user`：必须有一条展示给用户的明确问题/决策（界面可见），否则不得进入；
  - `ended(reason, outcome_location)`：完成/失败/取消，一句人话原因 + 成果在哪。
  不再使用 waiting_system、恢复预算、补救、取消归因。
- R2 一条完成规则：模型给出最终回答时——存在外部进行中事项且有唤醒源 → waiting_wake；存在外部进行中事项但无唤醒源 → 一次性提示挂监视（工具结果层面，不进 transcript），仍无则 waiting_user 并写明「需要你决定是否继续等待 X」；无外部事项 → ended(完成)。**不存在「拒收后让模型重答」的路径**，因此最终回答在 transcript 中只出现一次。
- R3 等待必须有执行者：模型若在回答中表示「等待确认/等待某人」而该对象不是用户（例如自造的「确认范围」），按 R2 处理为 waiting_user 并在界面显示问题；编排方/用户可回答。禁止无唤醒源的静默等待。
- R4 监督器变心跳：每 30 分钟（可配置）扫描 waiting_wake：唤醒源已结束/到期/事件已发生 → 唤醒；唤醒源失效（进程不存在、wakeup 过期未触发）→ 唤醒并附「你等的 X 已失效」提示。删除 thin 路径上的其他监督逻辑。
- R5 连续失败：同一真实技术失败 3 次 → ended(失败)，原因+试了什么+成果位置；no_model_available 按 M123 进入 waiting_wake（唤醒源=模型恢复时间）。
- R6 界面：会话列表/详情只展示四态与一句话（不展示内部代码词汇），参见「控制回路不进 transcript」原则。
- R7 数据：thin 会话不再写 objective_remediations / objective_recovery_attempts / objective_bindings / objective_decisions（阶段 0 §4 删除清单中与 objective 相关者）；必要的状态用 sessions 上的少量字段或一张小表。迁移不改 legacy 数据。
- R8 测试（先失败）：覆盖阶段 1 每个真机案例的回放（等待+后台任务、只读浏览器任务、拒收重复、自造等待对象、无可用模型、重启恢复）——每个都断言最终状态与 transcript 中最终回答只出现一次；心跳唤醒与失效唤醒；legacy 会话行为不变。
- R9 分片交付：允许拆成 2–3 个 PR（例如 ①状态模型+完成规则 ②心跳监督器 ③界面），每个 PR 自己可合并、行为在 thin 下一致；每个 PR 正文带 `Release-Urgency: immediate`。

## 退出标准（编排方真机验收）
- 新 thin 任务连续 10 单：交付环节人工介入 ≤1、无重复推送/PR、最终回答不重复、没有处于「无唤醒源的等待」的会话；任务完成率 ≥80%；GitHub REST ≤600/h。

## 约束
- 合成 fixture，不写生产 DB。授权交付（ceiling: merged）。不要超出规格自加范围；需要决定的事按规格最小化处理并在 PR 说明，不要挂起等确认。

## Implementation Notes
- 当前 managed branch 为独立执行工作区，不创建其他分支。交付 API 的合并上限枚举是 through_merge（等价于任务中的 merged）；不发版。
- 第一片针对 R2/R8：thin 最终回答不得经过 legacy fact-check/拒收循环；缺监视提示只能一次，保留已有回答且只允许工具补挂监视。其余 R1/R3–R7 的完整状态收敛在后续片接入。
- Applicable Harnesses：Spec、Compatibility、Observation、Release（合并交接）、AI Collaboration；UI 片追加 Viewport。
- AI Collaboration：上下文限于阶段 0–2、任务模板和相关生产接缝；仅使用合成 fixture。并行只读审查不修改文件，实施者执行测试并复核结论。
- 真机连续 10 单由编排方发版安装后验收，本地通过不声明 live。


## Implementation Notes（阶段2剩余分片，2026-10-20）

- **R1/R7 四态权威表**：`thin_session_state`（migration 0024）是 thin 会话唯一状态源：
  `running / waiting_wake / waiting_user / ended`，带 `waiting_for`、`wake_source`、
  `question`、`outcome_location`、`failure_signature`、`consecutive_failures`。
  thin 结算在 `apply_chat_objective_outcome` 分叉，直接写这张表并投影
  `chat_turn_state`（`thin_running|thin_waiting_wake|thin_waiting_user|thin_ended`），
  不再写 `objective_decisions` / `objective_remediations` / recovery 行，也不把
  legacy `failure_code` 投给 thin 会话。Objective 行仅在 `ended` 时以
  `legacy_orphan` 收口（该状态不声明任何 legacy 完成证据，避免伪造 completed）。
- **R4 心跳**：supervisor 每轮调用 `thin_state::sweep`；`waiting_wake` 有唤醒源就保持，
  唤醒源消失且超过 30 分钟则转 `waiting_user` 并提出「你等的 X 已失效」的问题。
  另外 stalled-active reaper 排除 `sessions.delivery_mode='thin'`，不再把 thin 等待当
  legacy 停滞处理。
- **R5 连续失败/模型恢复**：同一技术失败签名累计 3 次 → `ended`，reason 写明签名与次数；
  不同失败重置计数；`no_model_available` → `waiting_wake`，唤醒源是模型恢复时间，
  既不判技术失败也不追问用户。
- **R6 界面投影**：`src/lib/thinState.ts` 只从 activity kind 判四态并给一句人话；
  `MessageList` 显示四态标签，并把四态映射到结果卡既有的完成/等待语义。
- **R8 回放**：`thin.rs phase2_state_tests`（四态结算 + 3 次失败 + 模型恢复）、
  `thin_state::settle_tests`（权威表落库、legacy 失败码不出现）、
  `src/lib/thinState.test.ts`（界面投影与 legacy 不误标）。


## R10 消息入口按四态收敛（2026-10-20 补充）

- 入口不再是任务身份的裁决者：用户消息永远不被「找不到任务」或「任务身份有歧义」拒绝。
- 缺失的 root turn / objective 绑定按自愈处理：丢掉悬空绑定并创建或复用一个可用任务。
- 续接提示指向的 root/objective 不存在时忽略该提示，继续用当前会话可确定的任务。
- 多个候选打开任务时收敛到「当前轮已绑定的任务」；没有当前绑定时按确定性规则取一个，
  只记录内部告警，不向用户暴露身份决策。
- 与 M129（会话 3dbfe52c 在入口删除拒绝分支止血）同向：M129 先合入后，本项在其基础上
  保留「自愈 + 收敛」，不恢复任何拒绝分支。
