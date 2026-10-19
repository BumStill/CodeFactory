# 阶段 1 P0-3：thin 会话的只读任务答完后永远卡在「等待系统 / completion_evidence_incomplete」

## 证据（编排方 10-19 13:00，v1.89.7 真机）
- 会话 7f531297（thin，只读任务：「用受管浏览器打开 example.com，读标题，关闭会话，回复标题」，未授权交付）：模型已正确完成并回复「页面标题是 Example Domain」，附了 open→read→close 的证据；之后 objective 连续 4 次 `decision_applied / waiting_system / completion_evidence_incomplete`（detail_json 为空），停在 waiting_system。P0 修复后不再误判失败，但也永远不结束——用户看到的是一个回答完了却一直「等待系统」的会话。
- 同一问题很可能影响所有不涉及外部事项的 thin 任务（问答、只读调查、本地改动不交付）。

## 需求
- R1 thin 模式完成判定只看阶段 1 R5 那一条：存在外部进行中事项（PR 未合并、CI 未结束、发版未完成、后台任务仍在跑）→ 等待；否则模型给出最终回答即「已结束（完成）」。不再要求 legacy 的「可核对依据」证据包。
- R2 若 thin 模式确需保留某种证据检查，必须：给出明确的缺什么（写进工具/系统提示给模型，而不是空 detail）、有上限、超过上限即结束并说明，不得无限 waiting。
- R3 测试（先失败）：thin 只读问答 → 最终回答后状态 completed；thin + 浏览器只读任务（open/read/close 后回答）→ completed；thin + PR 未合并且无监视 → 提示挂监视；thin + PR 未合并且有后台任务 → waiting（不计失败）。
- R4 编排方真机验收：同一只读浏览器任务结束后 objective 为 completed。

## Requirements Traceability

| Req ID | 需求 | 最低证据 |
|---|---|---|
| R1 | thin 完成判定只看「外部进行中事项」；否则最终回答即完成 | `decision_for_thin_run_outcome` 单元测试（只读问答 / 浏览器只读）→ `completed`；legacy 路径不变 |
| R2 | 保留的证据检查必须写明缺什么、有上限、不得无限 waiting | thin 无最终回答时不伪造完成，回落 legacy 有界恢复；`detail` 由 legacy 结构化 blockers 提供 |
| R3 | 四类场景测试先失败 | 新增单元测试先红后绿；`external_wait_completion_hint` 的 thin 边界测试 |
| R4 | 真机只读浏览器任务结束后 objective 为 completed | 编排方真机验收（发布后，本 PR 记录交接） |

## Applicable Harnesses
- Spec Harness：本文件 + Req↔测试映射。
- Compatibility Harness：只新增 thin 判定分支，legacy 决策函数与测试保持原样。
- Observation Harness：completion 决策、失效码与 `detail_json` 继续走现有 objective 事件。
- AI Collaboration Harness：context scope / assumptions / review point / validation result 见 PR body。

## Implementation Notes
- thin 的完成判定放在 objective 决策入口（`decision_for_thin_run_outcome`），由 chat 结算层根据 `sessions.delivery_mode='thin'` 选择；`decision_for_run_outcome_with_reason` 保持 legacy 语义不变。
- thin 只对 `Informational` / `LocalMutation` 目标接受「最终回答」为完成证据（`EvidenceKind::InformationalAnswer`）；`Delivery` / `Live` 仍必须有交付回执，避免把「说了要发」当成已发。
- thin 运行轮内的外部事项等待沿用阶段 1 R5：`external_wait_completion_hint` 缺唤醒源时把 `background_task_start` / `schedule_wakeup` 提示写给模型，循环继续（等待，不记失败）。
