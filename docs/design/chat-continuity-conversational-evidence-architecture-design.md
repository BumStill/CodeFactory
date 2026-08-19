# 会话连续执行与自然工具证据架构设计

## 1. 连续执行状态机

```text
User turn
  -> ActiveSegment
      -> ObjectiveCompleted
      -> UserCancelled
      -> Checkpointed -> ContinuationQueued -> ActiveSegment
                            | scheduling/panic/restart failure
                            v
                       WaitingSystem -> RemediationSupervisor -> ActiveSegment
```

`max_iterations` 从“用户回合终止上限”降级为 `segment_iteration_budget`。一个用户回合可包含多个内部 segment，但只有一个 root goal/objective；turn/stream 可以 settled，只有 CompletionArbiter 或显式用户取消能产生业务终态。

分段边界处理顺序必须为：

1. 确认本轮最后一个工具结果已经持久化；
2. 写入连续性检查点；
3. 发出安全的进度事件；
4. 由 objective supervisor 调度下一 segment；
5. 下一 segment 接管成功后把检查点标为 resumed。

任务未完成时不得在第 2 步前后发出成功 `Done`。最终 `Done` 只表示已生成可见最终回复并关闭当前 root turn。

## 2. 连续性检查点

持久化层必须为每个 root turn 保留以下最小语义；可以使用专用表，也可以使用现有 completion journal，但原子性和 hydration 契约必须一致：

```text
root_turn_id
session_id
segment_index
status              checkpointed | queued | running | waiting_system | completed | cancelled
reason              segment_boundary | panic | process_restart | transport | no_progress
last_message_id
last_tool_call_id
goal_digest          脱敏目标摘要或稳定引用
attempted_strategy   有界、脱敏的策略签名
updated_at
```

- 检查点与最后一个工具 outcome 必须按顺序提交，避免工具已执行但游标仍指向工具之前。
- `goal_digest` 只用于续跑归属和用户提示，不保存新的隐藏用户指令。
- 启动时把长时间停在 `running`/`queued` 且没有活跃 owner 的记录归为 `waiting_system`，身份可证明时由 supervisor 自动 claim；身份不足时只读投影为 `legacy_orphan`，不提供技术恢复入口。
- 续段重放模型上下文时复用现有 provider replay；不得把 continuity journal 伪装成 `role=user`。

## 3. 分段续跑与无进展收敛

续段使用同一 session、root turn、权限决策和取消句柄。新的 segment 获得新的内部预算，但不得重置以下累计状态：

- 已尝试的失败签名；
- completion recovery 次数；
- 已完成工具及其 outcome；
- 用户取消状态；
- 当前任务的 wall-clock 与成本计量。

是否保持当前策略由材料进展决定，而不是无条件无限循环。连续出现相同 failure signature 或无新增文件、命令、测试、外部状态证据时，系统应先换策略；达到策略收敛规则后持久化 `failed_internal/platform_incident` remediation 与下一次观察，不能写 user-blocked 最终回复。该规则没有内部轮次耗尽等用户文案。

## 4. 后台 task 终态监控

聊天命令不得 fire-and-forget 后丢弃 `JoinHandle`。每个 spawned agent future 必须有 owner/watcher：

- 正常返回：AgentLoop 返回 typed outcome，由 CompletionArbiter/Decision Router 决定 completed、waiting 或 cancelled；
- `JoinError::is_panic()` 或 unwind：记录 `waiting_system(reason=panic)`，发出可见恢复事件，释放失效 running/cancel owner 并排队 remediation；
- task 被 abort：只有显式用户 cancel 才记录 `Cancelled`；其它 abort 记录 waiting_system，不能保持 running；
- watcher 自身无法写库时仍发送前端 error，并写本机诊断日志。

panic 文案只说明“执行意外中断，已保留完成内容”，详细 backtrace 留在诊断日志，不进入聊天正文。

## 5. Stream 与 hydration 契约

新增或等价表达以下产品事件：

- `continuity_checkpointed`：内部保存成功，用户可见“继续处理中”；
- `continuity_resumed`：下一分段接管，更新同一 streaming tail；
- `turn_interrupted`：包含脱敏原因、objective owner 和下一次观察；不携带技术继续入口；
- `turn_settled`：只表示当前 stream/turn future 已关闭；
- `objective_terminal`：只有 completed/cancelled，必须来自 CompletionArbiter 或显式用户动作。

前端 reducer 按 `root_turn_id` 更新同一个回合，而不是为每个 segment 创建新的用户目标。`Done` 或 error 到达后，前端应执行一次有 revision 门禁的尾页重同步；迟到响应不得覆盖已经开始的排队消息。

历史 hydration 按真实用户回合重建：

- assistant narration、tool declaration/replay、continuity 和 completion state 归入同一 turn；
- 中间 assistant 文本标记为 step，最后一个符合展示条件的正文标记为 final；
- 悬空工具尾部若没有 objective completed/cancelled，合成为 `WaitingSystem` 或 identity 不足的 `LegacyOrphan`；
- 旧数据库没有 continuity 字段时，依据持久化 tool outcome 和缺失终态做保守识别，不声称任务仍在运行。

## 6. 对话式工具证据视图模型

工具数据保留现有审计粒度，渲染层派生展示密度：

```ts
type ToolEvidencePresentation =
  | { kind: "quiet"; summary: string }
  | { kind: "routine_group"; count: number; categories: { read: number; search: number; file: number } }
  | { kind: "key"; summary: string }
  | { kind: "attention"; tone: "running" | "permission" | "error"; summary: string };
```

- `quiet`：成功且无需立即处理，默认无全周边框；
- `routine_group`：只接受成功的 `read`/`read_file`/`grep`/`glob`/`list_files`，相邻三个及以上才折叠；摘要只由总数和读取/搜索/文件计数构成；
- `key`：助手正文、edit/write、bash/exec、delegate/subagent 和未知工具，始终平铺；
- `attention`：运行、权限、失败、中断，仅使用轻背景/左侧状态线；
- 展开时才解析大 diff、知识结果和完整输出，继续满足超长会话惰性解析契约。

分组只能跨相邻工具记录，不能跨助手正文、失败、权限或用户消息。

展示状态与 objective 状态分离，且只存在于前端派生层：

```text
active/system-owned -> fresh_expanded -> compact_history <-> expanded_history
```

- `active/system-owned` 与 `fresh_expanded` 完全按原 timeline 平铺；active→completed/cancelled 不改变已挂载 DOM 的信息密度。
- 下一条用户消息使当前回合成为历史时进入 `compact_history`；历史会话初次 mount 也从该状态开始。
- disclosure 只在原位置切换 `compact_history`/`expanded_history`，结果卡不持有第二套过程状态。
- 会话 key 变化必须重置展示状态，不能把上一会话的展开偏好带入新会话。
- 缺少 segments 或 objectiveStatus 的旧会话默认保守展示；本变更不修改 DB、stream event 或 `TurnSegment` schema。
- 未知工具、未知状态一律按 `key`/`attention` 可见；安全摘要不得读取 args、path、prompt、stdout 或 result。

## 7. 主题透明度契约

Tailwind 颜色 token 必须支持 `<alpha-value>`。需要透明度修饰符的 token 使用 RGB channel：

```js
border: "rgb(var(--border-color) / <alpha-value>)"
```

对应 CSS 变量使用 `R G B` channel，而不是 hex。`border-border/25`、`bg-surface-1/30`、`bg-accent/5` 等类必须在生产 CSS 中真实生成；不存在的透明度类不得依赖浏览器回退。增加编译产物断言，防止组件测试因 jsdom 不计算真实 CSS 而漏掉黑框回归。

## 8. 自动项目记忆与最终回复判定

聊天气泡不再提供手动 `Remember` 入口。长期记忆由会话后学习链路从真实对话和工具证据中提取候选，并只自动物化安全、稳定、可复用的 project-scope memory；Profile 仍是查看、编辑和纠错入口。

最终回复判定仍用于耗时等辅助元信息：

- 所属 objective 已由 CompletionArbiter 完成；
- 该行是回合最终可见 assistant 正文；
- completion state 不是 step、notice、checkpoint、interrupted 或 rejected candidate；
- 不是匿名内部恢复文本。

live timeline 与 hydrated rows 必须经过同一个 `isFinalAssistantForTurn` 判定；任何 step、notice、checkpoint、interrupted 或 rejected candidate 都不得出现手动记忆控件。

## 9. 兼容与回滚

- 旧会话继续可读；缺少 continuity 信息时只增加保守中断提示，不改写原始消息。
- 不删除或压缩工具审计数据；自然对话视图只是 presentation。
- 回滚版本可以忽略新的 completion/continuity 状态，不得导致消息表不可读。
- 公开发布前必须验证从旧数据库升级、执行中强制退出后重启、浅深色生产 CSS 和真实 App 用户路径。

## 10. 执行路线、结果快照与估时

### 结构化 plan event

长任务开始执行后，模型使用 `update_plan` 工具提交有界快照：

```ts
type PlanStep = {
  id: string;
  title: string;
  kind: "analysis" | "implementation" | "verification" | "delivery" | "external_job" | "other";
  status: "pending" | "in_progress" | "completed";
  externalJobId?: string;
};

type PlanEvent = {
  rootTurnId: string;
  revision: number;
  steps: PlanStep[];
  explanation?: string;
  waitingReason?: string;
  changeReason?: string;
  createdAt: number;
};
```

- `chat_plan_events` 按 revision 追加，不改写消息或伪造用户 turn。
- 首次 plan、步骤状态变化、等待原因变化和步骤增删/重排都必须形成事件。
- revision 大于 1 且步骤集合或顺序变化时，必须提供 `changeReason`。
- live reducer 与 history hydration 使用同一 `TurnPlan` 视图模型。
- `update_plan` 只暴露给有 AppHandle 且会持久化的桌面会话；匿名和 headless 路径既不展示该工具，也不注入要求调用它的提示，避免隐私写入和必然失败的工具调用。

### 进度与时间区间

- 进度百分比只等于结构化步骤中的 `completed / total`，并显示“来自 N 个计划步骤”。
- 当前步骤取唯一 `in_progress` 项，下一步取其后的首个 `pending` 项。
- 等待原因来自 plan event；外部 job 状态只通过结构化 `externalJobId` 关联现有 `task_runs`。
- 时间区间来自同项目已完成 plan 阶段、成功 build/test 工具时长和已完成外部 job 时长的分位区间。
- 历史查询取最近的有界样本；关联外部 job 的当前真实状态在进度卡中直接显示。
- 相关历史样本少于 3 个时返回 `null`；禁止用固定速度、模型猜测或伪精确单点 ETA。

### 结果快照

终态结果快照由前端在 5 秒内从最终助手正文、最终 plan 和有界工具证据派生。重新总结只计算计划完成数、修改文件、验证命令、失败/等待证据和用时，不发起新的模型请求。完整过程仍引用原消息/工具记录并只由 timeline 原位 disclosure 展开；结果卡不复制完整 stdout/diff，也不再提供重复过程按钮。
