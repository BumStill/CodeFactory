# M236：会话运行中发出的消息必须是「可投递的排队」，不是「内存里的排队」

- 状态：已实现（待真机验证）
- 关联：M63（steer 幂等）、CF-HDE-R0/R7/R8（按会话寻址的 send/queue）、M112/M113（run 阻塞与恢复）
- 代码：`src-tauri/src/commands/queued_delivery.rs`、`src/lib/queuedSend.ts`、`src/stores/chat.ts`

## 1. 现场缺陷

2026-11-05 00:19，dispatch `send`（`mode: "queue"`）把一条消息发给会话 `38c0a9fe`，
当时该会话正在回合中。工具返回 `status: "queued"`，之后消息再没出现，也没有任何报错。

根因是**排队只发生在渲染进程的内存里**：

1. `enqueueMessage` / `sendOrQueue` 只往 `runtime[sessionId].queue` 这个数组 push，
   没有落库，也没有任何持久化字段；
2. 唯一的消费点是本渲染进程 reduce 到 `wasStreaming && !nowStreaming`（`turn_settled`）
   之后调用的 `drainNextQueuedMessage`；
3. 该回合是通过定时唤醒/外部路径结束的，本渲染进程没有观察到这一次迁移，
   于是 drain 从未触发；
4. 队列随后在 `selectSession` 重新 hydrate 时被丢掉（历史加载不恢复队列）。

于是「已排队」这个应答是**假的送达承诺**：既没有落库，也没有独立于事件流的投递者。

## 2. 需求

- **R1** 会话运行中发出的消息必须先持久化，再回 `status: "queued"`；回包必须携带可查询的
  消息 id 与队列位置。
- **R2** 持久化队列的投递者不得依赖某一个具体事件名或某一个渲染进程。
- **R3** 用户/编排消息优先于系统定时唤醒：唤醒处理器先 drain，drain 到消息则本次唤醒不再起回合。
- **R4** 会话转入 `waiting_wake`、`waiting_user` 或结束（idle/ended）时，队列里还有消息就必须
  投递，或转为清晰的终态失败。
- **R5** 任何已接受的消息都能被只读查询投递状态（`queued` / `delivering` / `delivered` / `failed`）。
- **R6** 失败的投递必须可见（状态 + 原因），不允许静默消失。

## 3. 设计

队列表落在应用数据库（`state.db`）：

```
queued_dispatch_messages(
  id, session_id, content, root_turn_id,
  status,        -- queued | delivering | delivered | failed
  attempts, created_at, updated_at, delivered_at, failure_reason
)
```

投递是一次 **claim → send → confirm** 握手：

- `claim_next` 用 `UPDATE ... WHERE status='queued'` 原子领取，两个 drain 并发也不会重复投递；
- `mark_delivered` 只在真正开跑下一回合后调用；
- `mark_failed` 写入原因，成为可见终态；
- `delivering` 超过 5 分钟未确认视为交接中断，退回 `queued` 重试（`attempts` 递增留痕）；
- `root_turn_id` 在入队时生成并持久化，跨重启保持同一条消息的身份。

前端 `runtime[sessionId].queue` 降级为**只读镜像**：条目 id 就是持久化行 id，
drain 之后按 id 移除；重启或 hydrate 时从库里恢复镜像。

drain 触发点（任一即可，不再依赖单一事件）：回合结算、会话 hydrate、取消成功、
应用启动时 `list_sessions_with_queued_dispatch_messages` 的恢复。

## 4. 验收

| 编号 | 场景 | 期望 |
| --- | --- | --- |
| T1 | 回合中入队 → 回合结束 | 消息开跑下一回合，状态 `delivered` |
| T1b | 连发两条 | 只有第一条成为下一回合，第二条等下一轮结算 |
| T2 | 存在待投递消息时定时唤醒到点 | 先投递用户消息，本次唤醒不起回合 |
| T3 | 入队后重启应用 | 消息仍送达，且只送达一次 |
| T4 | 回合以 `waiting_wake` 结束 | 待投递消息照常投递 |
| T5 | 向已结束/已停止的会话入队 | 返回明确失败，不产生 `queued` 假承诺 |
| T5b | 投递过程中后端拒绝 | 记录为 `failed` + 原因，可见 |
| T6 | 队列上限 | 第 6 条返回 `full`，不无限增长 |

后端单元测试覆盖持久化（重开同一 DB 文件即模拟重启）、FIFO、不可重复领取、上限、
失败可见、陈旧交接回收。

## 5. 兼容与边界

- 队列上限仍是 5（前后端各校验一次），超限返回 `full`，不回 `queued`。
- `status: "queued"` 的语义在本次修正后**变强**：从「记在内存里」变成「已提交事务」，
  调用方可依赖它做后续 `dispatch_message_status` 查询。
- 不做「消息合并」或「自动改写」：投递内容与入队内容逐字节一致（仅去首尾空白）。
- 已知限制：投递是 at-least-once + 原子领取；`root_turn_id` 跨重启复用保证同一条消息
  不会因为重试而成两回合。若未来后端支持按 `root_turn_id` 去重，可进一步收紧为强 exactly-once。
