# M193 用「卡住的证据」替代固定工具次数（规格修订）

状态：已实现（2026-10-29）。本文件是 `docs/design/session-execution-governance-ux-design.md`
「工具收敛」一节的规格依据，取代该文档原先的「工具放大提醒」（第 40 次工具调用弹黄色提醒）。

## 问题

原实现按「本回合第 40 次工具调用」无条件弹出黄色提醒，并把
`本回合已累计 N 次工具调用…` 作为一条 `user` 消息塞进下一轮提示。两种后果都真实发生过：

1. **正常的长任务被打断**：一项任务前 40 步都在取得新证据（读文件、跑测试、改代码），
   第 41 步开始每一轮都被要求「收敛」，用户看到的是一条自己无法处理、也不涉及任何决策的警告。
2. **提示落在错误的通道上**：收敛是系统内部的执行策略，却占用了对话消息位，既影响上下文
   预算，也让「系统状态」和「用户决策」在界面上分不开。

OpenHands 的 `StuckDetector` 早已说明该用「循环证据」而不是「调用次数」判断卡住；本仓库的
完成门禁（completion gate）也已有现成的证据语义可以对接。

## 需求与实现

| Req | 需求 | 实现位置 |
| --- | --- | --- |
| R1 | 卡住判据基于证据：同一调用同一结果重复、同一调用连续错误、A/B 交替循环、长时间无新证据（阈值默认 12，可配置，配置表示「连续多少次调用没有新证据」而不是固定调用次数） | `src-tauri/crates/agent-loop/src/stuck_detector.rs`；`RunConfig::stuck_no_progress_limit` |
| R2 | 正常长任务不得被警示：40+ 步且每步都有新证据时不产生任何提示 | `run::tests::long_task_with_fresh_evidence_gets_no_convergence_signal`、`resumed_tool_call_counts_never_produce_a_convergence_notice`、`stuck_detector::tests::long_task_with_fresh_evidence_never_trips` |
| R3 | 警示不上屏：不落库 `turn_notice`、不发 `CompletionGateAction{kind:"warning"}`、不作为 `user` 消息插入；提示只追加在工具结果通道（`【卡住信号】` 前缀），内容为「已观察到的卡住事实 + 最少收敛动作（按 TurnCapability：只读检查/实施验证/交付）」 | `run.rs` 批次末尾的观察钩子；`StuckSignal::hint` |
| R4 | 回归不被削弱：既有卡死场景仍然触发（重复读同一文件、反复失败的原样重试、A/B 循环） | `stuck_detector` 9 个单元测试 + `repeated_identical_tool_calls_get_a_hidden_stuck_hint` |
| R5 | 规格随实现更新 | 本文件 + `docs/design/session-execution-governance-ux-design.md`「工具收敛」节 |

补充约定：

- 提示文案是系统自动收敛，不是「请用户决定」——不得出现「回复继续」「等待你确认」这类要求，
  符合全局 `OBJECTIVE_MANUAL_TECHNICAL_HANDOFF` 约定（技术状态一律系统自愈）。
- 旧会话里已落库的 `本回合工具调用较多…` 行由 `MessageList` 直接不渲染，避免历史记录把已经
  不存在的警告重新显示出来。
- 检测器每轮重置，只在一个回合内观察；不做跨回合持久化，避免把上一轮的历史当成这一轮的循环。

## 验证

- `node scripts/cargo-shared.mjs test -p codefactory-agent-loop --lib -- stuck convergence`：12 passed。
- `npx vitest run src/components/MessageList.gate.test.tsx`：历史提醒不再渲染为警告。
