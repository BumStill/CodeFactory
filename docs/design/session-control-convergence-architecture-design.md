# 会话控制收敛架构设计

## 分层模型

```text
User message / structured action
  -> Turn dispatch (Interactive | Execute)
  -> Permission policy (allow | ask | deny; Full access only here)
  -> AgentLoop
  -> Completion recovery budget
  -> StreamEvent
  -> Visible recovery summary
```

## 回合分派

`commands/chat.rs` 必须始终通过 `decide_chat_mode(prev_assistant, content)` 选择语义模式。`settings.permissions.full_access` 不再覆盖结果；它继续由工具权限层读取。

本切片不新增持久化 intent 字段，保持旧数据库兼容。明确实施/批准仍沿用现有 dispatch 规则；Full access 下普通诊断恢复 Interactive 的 30 轮上限和 plan/read-first 契约。

## Completion recovery 预算

AgentLoop 保留两个概念：

- `total_completion_recoveries`：本轮累计 recovery 次数，只增不减，用于硬上限。
- `consecutive_completion_recoveries`：连续无材料进展次数；材料进展后可以归零，仅用于收敛提示或诊断。

`completion_finalization` 只读取累计次数决定是否还能 Recover。Interactive/Execute 达到 3 次后使用现有 `ReleaseWithWarning` 路径，必须发出 `Done`。Autonomous 继续使用 Blocked/调度器重试语义。

## 可见恢复摘要

沿用 `completion_gate_action` 事件，不新增数据库 migration。事件 detail 不直接显示，前端根据 kind 生成中文脱敏状态：

- `recovery`：进入「补充验证」，累计次数 +1。
- `ready`：进入「整理最终结果」。
- `warning`：保留最终回复并显示验证不完整警告。

`UIMessage` 新增非持久化 `reviewProgress`：

```ts
type ReviewProgress = {
  phase: "recovering" | "finalizing";
  attempt: number;
  currentStep?: string;
  lastActivityAt: number;
  reason: string;
};
```

内部恢复期间：

- `text_delta` 继续进入 `internalReviewDraft`，不展示内部叙述。
- `tool_call_start` 更新 `currentStep` 为工具的安全名称，不保存/展示完整 args。
- `tool_result` 更新最近活动和安全结果状态。
- `MessageList` 渲染 `ReviewProgressCard`，不渲染内部草稿和 recovery detail。

## Hydration 与兼容

旧数据库已经保存 `rejected_candidate`、`gate_recovery`、`gate_ready` 与 tool call replay。Hydration 不恢复内部正文，但把连续 recovery 记录折叠为一条只读摘要：

- attempt = 当前用户回合内 `gate_recovery` 数量；
- phase = 最后一条为 `gate_ready` 时 finalizing，否则 recovering；
- lastActivityAt = 相关记录最后时间；
- currentStep = 不从历史完整命令推断，避免泄漏和错误归因。

旧会话没有 recovery 记录时行为不变。匿名会话仅依赖 live event，不写 DB。

## 取消边界

本切片保持现有 cooperative cancellation：当前工具调用完成后在模型轮次边界停止。UI 文案明确「停止后续生成」，并说明不会撤销已经执行、提交或推送的变更。立即杀进程组属于后续独立能力，不在本切片伪装完成。

## 兼容风险

- Full access 用户可能依赖「所有消息直接执行」的非规格行为；本修复以仓库业务规格为准，明确实施仍无额外确认。
- recovery 事件只显示安全枚举，不显示 raw blocker；调试证据继续保留在 DB/日志。
- Hydration 折叠必须按用户回合分组，不能把上一轮 recovery 挂到下一轮。
