# 阶段 1 P0：thin 模式下「挂了后台监视在等」被完成门禁当失败，3 次后误判结束

## 证据（编排方 10-19 08:4x，v1.89.5 真机首单）
- 会话 589df7cb（delivery_mode=thin，任务 M120，PR #753）：交付后正确起了 background_task（`while true; gh pr checks 753 ...` 仍在 running）和 schedule_wakeup ×2；每次回复「后台监视已启动，CI 结束后自动继续」后，objective_events 记 `decision_applied / waiting_system / completion_evidence_incomplete`；第三次后按 R4「同一失败 3 次」判 `failed / technical_recovery_exhausted`（08:43:51），最后一条消息「已结束：连续三次遇到相同失败…completion_evidence_incomplete」。后台任务此时仍在跑。
- 规格冲突：task-simplify-phase1.md R5「存在外部进行中事项且本会话**有**后台任务/定时唤醒在等它 → 放行（进入等唤醒）」；R4 的「失败」本应只计真实技术失败。

## 需求
- R1 thin 模式完成门禁：外部事项未结束 + 本会话存在 running 的 background_task 或未来的 wakeup_schedule → 会话进入「等唤醒」状态，**不计为失败、不追加系统催促、不消耗 R4 计数**。
- R2 R4 计数只统计真实技术失败（工具/模型/网络错误等），门禁的「请挂监视」提示与正常等待都不计入。
- R3 等唤醒的会话在后台任务结束或唤醒时间到时自动续跑（现有 CF-BG 唤醒链），续跑后若 CI 绿且任务授权 ceiling≥merged，按 thin R2 第 4 步确保 `merge-queue: armed`。
- R4 模型在 thin 交付时传的 `ceiling` 低于会话授权（本例授权 merged、模型传 through_ci_green）→ 以授权为准或在工具结果中明确提示，别让「授权交付」的任务停在 CI 绿。
- R5 测试（先失败）：合成会话 thin + 运行中后台任务 → 连续 5 次「我在等」回合 → 状态始终 waiting、不 failed；后台任务结束 → 自动续跑；无后台任务且外部未结束 → 提示挂监视（不计失败）；真实工具错误 3 次 → 已结束。
- R6 修复已错误结束的会话：不需要迁移旧数据，但若可安全实现，「已结束」后其仍在运行的后台任务完成时，应能重新唤醒该会话（或至少界面明确显示「仍有后台任务在运行」）。

## 约束
- 这是阶段 1 自身的缺陷（允许，属 P0）。不改 legacy 路径。合成 fixture，不写生产 DB。授权交付（ceiling: merged）。

## Implementation Notes
- 规格原文保留于上；测试、验收与 Harness 证据在 PR 中逐项记录。
- 不调用 delegate_tasks，遵循任务模板的单执行流限制。
- 不操作生产数据库；真机验收由编排方发版后完成，本任务只合并、不发版。
