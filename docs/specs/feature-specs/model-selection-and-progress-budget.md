# 模型选择与进展预算（M60 + M61）

## 问题与证据
- **M60**（2026-10-10，v1.83.2 / v1.84.0）：本地派单入口的模型选择不可靠。
  - `create_and_send` 带 `"model":"gpt-6.1-sol"`，当时默认端点是 deepseek，会话却悄悄落在 deepseek/deepseek-v4-flash 上，入口照样回报 ok。
  - `set_model{session_id, model}` 只改了 model_id，端点没跟着变，出现「deepseek/gpt-6.1-sol」这种错配，下一轮必然失败。
  - v1.84.0 的 `set_model`（省略 session_id）返回 `{endpoint: chatgpt, model: gpt-5.6-sol, scope: default}`。但 settings 里 `default_model` 变成了 gpt-5.6-sol，`endpoints.chatgpt.active_model` 仍是 gpt-6.1-sol；新会话取模型时先看端点的 active_model，于是「设置了默认」和「新会话实际用的」不一致。
- **M61**（2026-10-10 16:21–16:37，会话 b63eda01，GPT-6.1-Sol）：每轮改一点就结束、汇报「尚未完成」，每次自动续跑都算一次恢复，16 分钟后 technical_recovery_exhausted。实际每一轮都有进展（测试从 3 个失败到 50 个通过），也没有真正的阻塞。

### Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| CF-MSP-R1 | 「选模型」只有一个来源：入口、界面下拉、设置页设置默认模型时，写入同一组配置（端点与模型配套，端点的当前模型与默认模型一致），新会话实际使用的模型就是设置的那个 | 测试：三种入口设置后，新会话的端点和模型都等于设置值 |
| CF-MSP-R2 | 入口的 `create_and_send` 与 `set_model` 指定任意已配置端点下的模型时，自动切到该模型所属的端点；模型不存在或所属端点不可用时，返回明确错误，不能悄悄换成别的模型 | 测试：跨端点指定、模型不存在两种情况 |
| CF-MSP-R3 | 一轮结束时如果产生了新的有效改动或新的通过验证（有进展），自动续跑不消耗「技术恢复」次数；只有没有任何进展的重复续跑才计入上限。上限依然存在，不能无限循环 | 测试：连续有进展的多轮不触发 exhausted；连续无进展的多轮仍按上限停下 |
| CF-MSP-R4 | 没有阻塞却结束回合时，系统给模型的续跑提示要明确「目标还没完成，继续做，不要只汇报进度」，并且这句提示不出现在用户可见的对话里 | 测试：续跑提示的内容，以及它不写入 messages |

### Applicable Harnesses
Spec Harness；Compatibility Harness（已有的 settings 与 objective）；Observation Harness；AI Collaboration Harness。

### 测试矩阵
- 设置默认模型：分别通过入口、界面、设置页；随后新建会话。
- 入口跨端点指定模型；指定不存在的模型。
- 有进展的多轮、无进展的多轮、两者交替。

### 约束
- R3 动的是恢复上限的计数口径：要在 PR 里写清楚新口径，并说明不会造成无限循环的证明。
- 其余约束同模板。

## Implementation Notes（不改动任何需求或验收标准）
- R3 的计数口径改为：**只有「没有任何进展」的被拒回合才 `+1`**。实现是
  `codefactory_agent_core::completion_evidence_made_progress(before, after)` 比较上一次被拒
  回合与本次的证据账本；推进了就保持计数不变，没推进才加一。函数：
  `crates/agent-loop/src/policy.rs::completion_recovery_attempts_after_rejection`。
- 「不会无限循环」的证明（三条独立的硬边界，任一条都能终止）：
  1. 无进展的回合仍然逐次计数并在 `recovery_limit` 处停下（对应
     `completion_finalization` 的 `Blocked` 分支）；
  2. objective 级兜底 `MAX_OBJECTIVE_RECOVERY_ATTEMPTS = 20`（对同一
     recovery generation 的所有签名求和，含签名不断变化的"假进展"）；
  3. 每个回合仍有 `max_iterations` 的模型轮次上限与 segment checkpoint。
- R1 的界面/设置页两个入口本来就走「端点 active_model」这一条，设置页已无独立的
  「默认模型」控件；本次让入口（`set_model` 省略 session_id）也写同一组配置：
  `default_endpoint` + `default_model` + `endpoints[ep].active_model` 三者一致。
- R4 的续跑提示是**派生的**：`resume_chat_objective_inner` 只把它 push 进内存里的
  模型历史（role=system，id 前缀 `system-continuation:`），从不写入 `messages` 表。
