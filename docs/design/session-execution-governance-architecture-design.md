# 会话执行治理：架构设计

## 核心模型

### Turn capability

`TurnCapability = ReviewOnly | Implement | Deliver` 在接收用户消息时由框架确定。它对模型和权限设置不可扩大，但真实用户在运行中发送的 steer 可以在下一安全轮次边界形成 capability revision：

- ReviewOnly：读取、搜索、状态探测、无副作用验证。
- Implement：增加本地 mutation；拒绝 commit/push/PR/merge/release/deploy。
- Deliver：允许完整交付工具和明确的交付 shell。

Tool schema 每轮按当前 capability 过滤；执行前再以真实 tool name、arguments 和 command classification 做第二道门禁。未知 MCP 在 ReviewOnly 中 fail closed。一次性预过滤是错误的，因为 ReviewOnly 起步的 root turn 在用户明确批准实施后会永久看不到写工具。

### Continuation 与 steer

`继续` 是控制事件，不再依赖上一回复末尾的问号。新 root 优先继承最近可执行 proposal；运行中 steer 则通过 `SteerInbox.capability_override` 在下一模型请求前原子更新硬门禁。能力变化会清除旧的结构拒绝标记，但不会清零累计 completion recovery。

有效目标、completion gate 与 fact checker 使用同一份继承后的 instruction，避免“目标已是实施、能力仍是只读”的分裂。结构拒绝如果未被后续用户 capability revision 恢复，终态写为 blocked。

### Delivery preflight

`deliver_changes` 先构造 `DeliveryPreflight`，解析 repo/branch/remote/provider/adapter/认证及目标 ceiling 的能力。preflight 不运行 `git add`、commit、push 或远端写 API。只有 `ready=true` 才进入现有 state machine。

`ToolInvocationResult` 携带业务 outcome status；持久化与 stream 使用该值，禁止从正文解析状态。

### Completion convergence

证据账本先于最终候选决策。第一次缺失时生成一个只包含 blockers 的 targeted recovery；第二次仍不足直接形成 `verification_incomplete`。相同 workspace/command 的成功验证继续复用，mutation 后失效。

### P1 persistence

- `chat_task_segments`：root turn 内的有界上下文和 handoff。
- `chat_turn_state`：每个 root turn 一行 upsert 的最新进度。
- `task_attempts`：task_run 的 append-only 1:N attempt。

三张表均 additive、幂等建表；旧字段保留。

## 数据流

```text
user message
  -> decide turn capability
  -> resolve effective objective / proposal
  -> filter advertised tools for current capability
  -> optional user steer capability revision
  -> re-filter advertised tools
  -> AgentLoop execution gate
  -> tool outcome status
  -> evidence ledger / progress snapshot
  -> one final result
```

交付路径：

```text
Deliver capability
  -> side-effect-free preflight
  -> local commit
  -> push / review / CI / merge / release
  -> deployment observation
  -> live verification
```

## 兼容与安全

- Full access 不参与 capability 选择。
- 旧数据库启动两次迁移结果相同。
- `task_runs.sub_session_id` 保留为最近 attempt 的兼容链接。
- ReviewOnly 拒绝未知工具；Implement 拒绝专用交付工具和可识别交付 shell。
- 内部 control prompt 不作为普通用户消息回放。
