# M160：授权交付的会话不得在「还差交付」时以最终回答结束

## 证据（10-22 M153 adb96f9b）
- 14:36 assistant：「进展：视觉桥已经落地并通过验证，现在只剩交付这一步。」随后 thin_session_state=ended（最终回答已完成），未开 PR；编排方 15:5x 手动再派「完成交付」。
- 同族：10-21/22 多次 ended(superseded_by_user_reprompt) 后 PR 无人跟进（M148/M149 已修部分）。
## 需求
- R1 会话 delivery_authorized 且 ceiling≥pr 时，若存在未交付的本地提交/改动（托管工作区相对 base 有差异且没有对应 open/merged PR），回合不得以「最终回答」结束：系统自动继续一轮「完成交付」（不计用户介入，不写入对话的内部控制文本），或在确有阻塞时进入 waiting_user 并给出人话原因。
- R2 判定以事实为准（工作区 diff + delivery_runs/PR 状态），不靠关键词匹配回答文本。
- R3 有上限（同一回合自动续交付最多 2 次），超出给出可见原因。
- R4 测试（先失败）：合成 fixture「已提交未开 PR + 模型给出最终回答」→ 自动续一轮并开 PR；「无改动的只读任务」→ 正常结束；「delivery 不授权」→ 正常结束。
## 约束
- task-template；合成 fixture；授权交付（ceiling: merged）。

Release-Urgency: default

## Implementation Notes
- 门禁在共享循环最终回答出口执行，包括 thin 模式；内部续交付指令只进模型上下文，拒绝的候选回答不作为完成事实。
- Spec Harness、Compatibility Harness、Observation Harness、AI Collaboration Harness；无布局改动。真实安装版验收由编排方发版后执行：授权交付任务有提交但未开 PR 时确认自动继续，内部指令不出现在聊天记录，无改动与未授权任务仍正常结束。
- 测试矩阵覆盖授权/未授权、净工作区/未提交/已提交、有对应 PR、两次续交付上限及内部消息隔离。
