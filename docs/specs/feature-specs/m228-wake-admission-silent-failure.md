# M228：到点唤醒被准入层静默拒绝 100 次，会话白等 1 小时 40 分钟

## 现场（2026-11-04，v1.91.75，电脑全程开盖醒着）
- 会话 7b078d78（M223）和 f6fdddd5（M226）都挂了「没有可用模型：ChatGPT 05:52 重置」的唤醒（wakeup_schedules id `model-availability-<session>`，next_at 05:52:22）。
- 05:52 以后 ChatGPT 实际可用：06:00 另一会话 fc2dc2ff 在 chatgpt 上成功了。
- 两条唤醒到 07:32 仍是 `pending`、fire_count=0。`workspace_recovery_budget` 中的 `wake-delivery:<session>` 行 attempts=101，last_error 为「唤醒准入失败；系统自动重试/自愈」。`wakeup_alerts` 中 retry_count 约 100。这段时间里没有任何 model_route_attempts。
- 也就是说，`drain_due_wakeups` 每分钟都调用 `deliver_wake` → `ChatWakeSink::enqueue` → `send_system_wake`，每次都返回 Err，而错误只以 `tracing::debug!` 记下（background_tasks.rs 的 `impl WakeSink for ChatWakeSink`）。真实原因没有落库，界面上一直显示「届时自动继续」。
- 07:32 编排方用 dispatch `send`（mode queue）发了一条普通消息，两个会话立刻 `started`。说明会话本身能接收新回合，被拒的只是系统唤醒这条路径。
- 另外：已结束的会话 ed737422（PR #948 已合并、thin_session_state=ended）在 3 小时内产生了 165 条 `model-availability-ed737422-…-reNNN` 唤醒，每分钟创建一次、随即取消，文案为「启动阶段中断」。结束了的会话不应该再有唤醒回路。

- 同类证据：22e55d80（M192）在 11-04 03:59 前也累计 159 次准入失败；7b078d78 在手工恢复、会话已 running 后仍继续累计（08:03 达 131 次），说明 pending 唤醒在会话忙时也会每分钟重试并计数，需区分「会话忙，延后合并进当前回合」与「真正被拒」。

## 要求
- R1：找出 `send_system_wake` 在这种状态下（waiting_wake，原因是模型可用性，到点后）返回 Err 的真实原因，并修好，让到点唤醒能像用户消息一样开启新回合。先写一个能复现「waiting_wake + model-availability 唤醒到点 → enqueue 被拒」的测试，确认它先失败。
- R2：准入失败的错误文本（脱敏后）写进 `wake-delivery` 行的 last_error 和会话审计事件，不得再只打 debug 日志。
- R3：同一会话唤醒准入连续失败 ≥5 次（约 5 分钟），会话状态要换成用户看得懂的一句话，写清真实原因和下一步，例如「自动继续失败：<原因>，将在 N 分钟后重试」。连续失败 ≥30 次时，按「no limbo」原则给出明确失败终态（交代试了什么），不得无限静默重试。
- R4：会话进入 ended 时，取消它的所有 pending 唤醒，并阻止后续再创建 model-availability 唤醒。测试覆盖 ed737422 的形态（已结算会话 + 启动阶段中断）：不再产生新的唤醒行。
- R5：回归测试覆盖本次组合：ChatGPT 订阅额度到点重置 + DeepSeek 每日上限已到 → 唤醒在到点后 1 分钟内成功开启回合，并产生 chatgpt 路由尝试。

## 验收（真机）
- 构造一个短时间后到期的 model-availability 唤醒（或等下一次自然的额度重置）：到点后 2 分钟内会话自动继续，`wake-delivery` 行被清除，不需要人工发消息。
- 对已结束的会话，`wakeup_schedules` 中不再新增 `model-availability-<id>-re*` 行。

Release-Urgency: immediate

## Implementation Notes
- 任务未指定目标文件名，本文件逐字保存任务正文；以下仅补充实现证据，不改变要求。
- Applicable Harnesses：Spec、Compatibility、Observation、Release、AI Collaboration。后端行为修复，无新增 GUI 操作；沿用后台调度与 dispatch 准入。
- 真机验收由编排方发版安装后完成；会话内用 SQLite 集成测试验证准入、重试预算、审计及终态。
- R1/R5 测试必须走生产准入事务；仅依据 durable model-availability root 接续同一任务，不创建新任务或扩大用户授权。
