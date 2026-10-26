# M173（高）：派单首回合启动阶段被静默丢弃（legacy_orphan），任务永远不开始

## 证据（v1.91.34，10-26）
- 10:22:11 dispatch create_and_send（M170，授权交付，trusted，模型 chatgpt/gpt-6.1-sol）返回 delivered；会话 b29ca719。
- chat_run_controls：10:22:11 创建、10:22:12 completed；chat_turn_state：phase=finalizing，status=**legacy_orphan**；model_usage_events = 0；无工作区、无 assistant 消息、无可见错误。
- 此后 4 小时无任何活动，最终 thin_session_state=ended「无活跃执行者或未决等待」。当时 DeepSeek 处于忙时、GPT 五小时窗口曾达上限（M164 应进入 waiting_wake 并按时唤醒）。
## 需求
- R1 查清首回合为何被标 legacy_orphan（路由全不可用？objective/legacy 双轨判定？），写入 PR。
- R2 首回合任何启动失败都必须：可见原因 + 进入 waiting_wake（可重试的路由/额度问题，带唤醒时间，M168 投递）或 waiting_user；绝不静默结束、绝不 legacy_orphan。
- R3 dispatch 返回 delivered 只在回合真正开始（或进入有唤醒源的等待）后；否则返回明确错误。
- R4 测试（先失败）：路由全不可用时派单 → waiting_wake 且到点唤醒执行；无凭据/额度满组合；不出现 legacy_orphan。
## 约束
- task-template；合成 fixture；授权交付（ceiling: merged）。

Release-Urgency: immediate

## Implementation Notes
- Harness：Spec、Compatibility、Observation、Release、AI Collaboration；不修改布局。
- 修复范围：启动 guard 的 thin 分支不再调用 legacy 决策退休；模型不可用沿用 M164/M168 持久唤醒，其他启动失败明确请求用户处理。
- 真实应用验收由编排方发版安装后完成：派单时模拟 GPT cap + DeepSeek 窗口外 + 无其他凭据；确认有原因及唤醒时间，重启后到点继续，无 legacy_orphan；无可恢复配置时 dispatch 明确失败。
