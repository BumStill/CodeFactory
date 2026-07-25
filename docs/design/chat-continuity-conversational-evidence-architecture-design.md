# 会话连续执行与自然工具证据架构设计

## 1. 连续执行状态机

```text
User turn
  -> ActiveSegment
      -> Completed
      -> Blocked
      -> Cancelled
      -> FailedVisible
      -> Checkpointed -> ContinuationQueued -> ActiveSegment
                            | scheduling/panic/restart failure
                            v
                       InterruptedRecoverable
```

`max_iterations` 从“用户回合终止上限”降级为 `segment_iteration_budget`。一个用户回合可包含多个内部 segment，但只有一个 root goal 和一个最终终态。

分段边界处理顺序必须为：

1. 确认本轮最后一个工具结果已经持久化；
2. 写入连续性检查点；
3. 发出安全的进度事件；
4. 调度下一 segment；
5. 下一 segment 接管成功后把检查点标为 resumed。

任务未完成时不得在第 2 步前后发出成功 `Done`。最终 `Done` 只表示已生成可见最终回复并关闭当前 root turn。

## 2. 连续性检查点

持久化层必须为每个 root turn 保留以下最小语义；可以使用专用表，也可以使用现有 completion journal，但原子性和 hydration 契约必须一致：

```text
root_turn_id
session_id
segment_index
status              checkpointed | queued | running | interrupted | completed
reason              segment_boundary | panic | process_restart | transport | no_progress
last_message_id
last_tool_call_id
goal_digest          脱敏目标摘要或稳定引用
attempted_strategy   有界、脱敏的策略签名
updated_at
```

- 检查点与最后一个工具 outcome 必须按顺序提交，避免工具已执行但游标仍指向工具之前。
- `goal_digest` 只用于续跑归属和用户提示，不保存新的隐藏用户指令。
- 启动时把长时间停在 `running`/`queued` 且没有活跃 owner 的记录归为 `interrupted`，再按权限和安全边界自动恢复或提供继续入口。
- 续段重放模型上下文时复用现有 provider replay；不得把 continuity journal 伪装成 `role=user`。

## 3. 分段续跑与无进展收敛

续段使用同一 session、root turn、权限决策和取消句柄。新的 segment 获得新的内部预算，但不得重置以下累计状态：

- 已尝试的失败签名；
- completion recovery 次数；
- 已完成工具及其 outcome；
- 用户取消状态；
- 当前任务的 wall-clock 与成本计量。

“可继续”由材料进展决定，而不是无条件无限循环。连续出现相同 blocker 或无新增文件、命令、测试、外部状态证据时，系统应先换策略；达到策略收敛规则后持久化 `Blocked` 并生成具体最终回复。该规则没有“30/80 轮已用完”等用户文案。

## 4. 后台 task 终态监控

聊天命令不得 fire-and-forget 后丢弃 `JoinHandle`。每个 spawned agent future 必须有 owner/watcher：

- 正常返回：由 AgentLoop 产生 Completed/Blocked/Cancelled；
- `JoinError::is_panic()` 或 unwind：记录 `interrupted(reason=panic)`，发出可见错误事件，释放 running/cancel 状态；
- task 被 abort：记录 `Cancelled` 或 `InterruptedRecoverable`，不能保持 running；
- watcher 自身无法写库时仍发送前端 error，并写本机诊断日志。

panic 文案只说明“执行意外中断，已保留完成内容”，详细 backtrace 留在诊断日志，不进入聊天正文。

## 5. Stream 与 hydration 契约

新增或等价表达以下产品事件：

- `continuity_checkpointed`：内部保存成功，用户可见“继续处理中”；
- `continuity_resumed`：下一分段接管，更新同一 streaming tail；
- `turn_interrupted`：包含脱敏原因、是否可自动恢复和继续入口能力；
- `turn_terminal`：completed/blocked/cancelled/failed 的唯一终态。

前端 reducer 按 `root_turn_id` 更新同一个回合，而不是为每个 segment 创建新的用户目标。`Done` 或 error 到达后，前端应执行一次有 revision 门禁的尾页重同步；迟到响应不得覆盖已经开始的排队消息。

历史 hydration 按真实用户回合重建：

- assistant narration、tool declaration/replay、continuity 和 completion state 归入同一 turn；
- 中间 assistant 文本标记为 step，最后一个符合展示条件的正文标记为 final；
- 悬空工具尾部若没有 completed/blocked/cancelled/failed 终态，合成为 `InterruptedRecoverable`；
- 旧数据库没有 continuity 字段时，依据持久化 tool outcome 和缺失终态做保守识别，不声称任务仍在运行。

## 6. 对话式工具证据视图模型

工具数据保留现有审计粒度，渲染层派生展示密度：

```ts
type ToolEvidencePresentation =
  | { kind: "quiet"; summary: string }
  | { kind: "group"; count: number; summary: string }
  | { kind: "attention"; tone: "running" | "permission" | "error"; summary: string };
```

- `quiet`：成功且无需立即处理，默认无全周边框；
- `group`：相邻三个及以上例行成功项，折叠但不改变原始顺序和展开内容；
- `attention`：运行、权限、失败、中断，仅使用轻背景/左侧状态线；
- 展开时才解析大 diff、知识结果和完整输出，继续满足超长会话惰性解析契约。

分组只能跨相邻工具记录，不能跨助手正文、失败、权限或用户消息。

## 7. 主题透明度契约

Tailwind 颜色 token 必须支持 `<alpha-value>`。需要透明度修饰符的 token 使用 RGB channel：

```js
border: "rgb(var(--border-color) / <alpha-value>)"
```

对应 CSS 变量使用 `R G B` channel，而不是 hex。`border-border/25`、`bg-surface-1/30`、`bg-accent/5` 等类必须在生产 CSS 中真实生成；不存在的透明度类不得依赖浏览器回退。增加编译产物断言，防止组件测试因 jsdom 不计算真实 CSS 而漏掉黑框回归。

## 8. Remember 与最终回复判定

`Remember` 的显示条件不是“当前 assistant 行已停止 streaming”，而是：

- 所属真实用户回合已有终态；
- 该行是回合最终可见 assistant 正文；
- completion state 不是 step、notice、checkpoint、interrupted 或 rejected candidate；
- 不是匿名内部恢复文本。

live timeline 与 hydrated rows 必须经过同一个 `isFinalAssistantForTurn` 判定。

## 9. 兼容与回滚

- 旧会话继续可读；缺少 continuity 信息时只增加保守中断提示，不改写原始消息。
- 不删除或压缩工具审计数据；自然对话视图只是 presentation。
- 回滚版本可以忽略新的 completion/continuity 状态，不得导致消息表不可读。
- 公开发布前必须验证从旧数据库升级、执行中强制退出后重启、浅深色生产 CSS 和真实 App 用户路径。
