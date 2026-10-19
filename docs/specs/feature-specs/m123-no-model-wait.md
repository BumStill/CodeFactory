# M123：所有模型都暂不可用时，会话应「等到可用时间再继续」，而不是连败 3 次判结束

## 证据（编排方 10-19 15:2x，v1.89.8）
- 会话 ee6f9a24（只读任务）、f5e88050（M121 收尾）、a9e31a58：model_route_attempts 显示同一时刻三条路由全部不可用——
  - deepseek/deepseek-flash：skipped `outside_usage_window`，next_available_ms=18:00；
  - chatgpt/gpt-6.1-sol：skipped `subscription_quota_cap`，resets_at_ms=18:19；
  - openrouter/openai/gpt-6-luna：failed `CREDENTIAL_UNAVAILABLE`。
- 结果 3 次 `agent_loop_error` 后按阶段 1 R4 判「已结束：连续三次遇到相同失败…agent_loop_error」，用户看不出是「没有可用模型、18:00 才恢复」。

## 需求
- R1 路由层判定「所有候选都不可用」且至少一个给出明确恢复时间（usage window next_available、额度 resets_at）时：不发起请求、不计为失败；会话进入「等唤醒」，登记 wakeup 到最早恢复时间（+少量抖动），到点自动续跑。界面/消息用一句人话说明：「当前没有可用模型：DeepSeek 闲时 18:00 起可用，ChatGPT 额度 18:19 重置；届时自动继续。」
- R2 全部不可用且没有任何恢复时间（例如只剩缺凭据）→ 直接「已结束：没有可用模型（OpenRouter 未配置密钥…）」，说明需要用户做什么，不重试 3 次。
- R3 R4 计数（同一失败 3 次）不计入「无可用模型」；真实请求错误照旧计入。
- R4 测试（先失败）：三条路由均 skipped/有恢复时间 → waiting + wakeup 时间 = 最早恢复；到点后可用 → 续跑完成；只有缺凭据 → 立即结束并给出说明；混合（一个可用）→ 正常走可用路由。
- R5 legacy 路径不改。

## 约束
- 合成 fixture，不写生产 DB。授权交付（ceiling: merged）。PR 正文带 `Release-Urgency: immediate`（顺带验证 M122：正文 trailer 能触发自动切版）。

## Implementation Notes
- 原任务未指定规格文件名，存放于本文件。需求逐字保留。
- Harness：Spec、Compatibility、Observation、Release、AI Collaboration。仅后端消息与调度行为，不改布局。
- 验收矩阵：R1/R4 有恢复时间的全不可用组合、时间到点可用；R2/R4 仅缺凭据；R3 连续多次无模型不耗失败计数、真实错误保留门禁；R4 混合可用；R5 原入口回归。
- 补强（R1 调度边界）：`ModelUnavailable::decision` 在排期前把唤醒时间规整到未来（最少 `MIN_RESUME_LEAD_MS`=1s），`resume_at_with_lead` 负责该规整且不把 `None`（不可恢复原因）变成等待。理由：复用旧错误载荷、时钟回拨或休眠错过边界时，过去的唤醒时间会被 supervisor 立刻视为到期，表现为「等待中却持续发请求」。测试 `m123_past_resume_time_is_pushed_into_the_future` 覆盖过去时间被推后、未来时间原样保留、`None` 仍为 `None`。
- context scope：路由准入、Objective 决策与聊天收尾；assumptions：现有 Objective supervisor 的 next_observation_at 提供持久自动恢复；review point：legacy 不进入新分支；validation result：随测试和 PR 更新。
