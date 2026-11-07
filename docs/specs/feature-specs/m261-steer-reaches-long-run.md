# M261：卡在长回合里的插话（steer）送不到

## 现场（2026-11-07，v1.103.0，会话 4269df42）

- 18:15–18:47，编排方用 dispatch `send`（`mode: steer` 与 `mode: queue` 都试过）给
  一个**已经在跑**的会话发消息，5 条全部返回 `status: queued`、`ahead: current_run`、
  `queue_position: N`，但会话里一条都没出现。
- 18:47 编排方 `stop` 掉这个 run，再手发一条，**通了**。
- 证据盘点：
  - `messages` 表：5 条都没有。
  - `queued_dispatch_messages` 表：只有更早一条 `delivered` 的，没有这 5 条。
  - `chat_run_controls`：这段时间 1 条 `active`，中间多次等审批。
  - App 日志无错误 —— 没有报错，也没有交付记录，属于**静默丢消息**。

## 根因

steer 走的是 `queue_interjection` → 内存 `InterjectionQueue`。agent loop 只在
**回合顶部**（`run.rs` 每轮模型调用前）`steer.drain()`。当一个 run 整段都待在
**同一个 round** 里（审批等待 + 工具执行，正是 18:15 那次），它永远走不到那个
边界：

- 消息只在 RAM 里，所以 `messages` 没有；
- 它也不是 queued send，所以 `queued_dispatch_messages` 没有；
- run 被 `stop` 时 HashMap 随之丢弃 —— 消息凭空消失。

这解释了「停止后再发就通」：停止结束了那一轮，新消息进的是一个全新回合。

## 决定

steer 的接受必须是一次**落盘**，而不是一次内存写入。落盘发生在命令返回之前，
所以「回执说已排队」与「消息真的还在」是同一件事，不存在中间态。

## 落地（R1/R3/R5 的最小可用切片）

1. **持久化**：新增 `pending_steer_messages`
   （`id, session_id, content, client_message_id, status, created_at, delivered_at`）。
   `queue_interjection` 先写库、再进内存队列；写库失败就是接受失败，绝不返回
   「已排队」而消息只存在 RAM 里。
   - `status='pending'` = 已接受、还没有任何模型调用读到；
   - `status='delivered'` = 已经进入该会话的模型历史。
   - `(session_id, client_message_id)` 上有唯一索引，重复提交幂等。
2. **恢复**：`SessionSteerInbox::drain()` 先取该会话所有 `pending` 行（上一个/卡住的
   run 没读走的），再合并内存队列，按行 id 去重，交出去后标记 `delivered`。
   于是「回合卡住 → stop → 下一回合」这条路径上，消息会被捡回来而不是消失。
3. **如实回执（R3）**：steer 的 `queued` 回执改成
   `已持久化，等当前步骤结束后读取（本回合下一次模型调用前送达）`，
   不再用只描述内存排队的「当前步骤结束后读取」糊过去。

改动文件：

- `src-tauri/src/commands/interjections.rs`：表结构、`persist_steer`、`unrecovered_steers`、
  `drain()` 的合并顺序与幂等。
- `src-tauri/src/agent/mod.rs`：启动时建表。
- `src-tauri/src/agent/delivery_scenario_repair.rs`：`Interjection` 新增 `id` 字段后的构造修正。
- `src/lib/dispatchActions.ts`：回执文案。

## Requirements

| ID | 要求 | 状态 |
| --- | --- | --- |
| R1 | steer 在当前回合**下一次模型调用之前**送达，包括等审批/工具执行中 | 部分：loop 顶部 drain（已有）+ `pending` 行保证不丢；工具内部仍不可中断（见「未做」） |
| R2 | 回合结束后到达的消息，开新回合送达；不中途插进工具结果 | 已有（settle 后 drain queued send），本次未改 |
| R3 | `queued` 回执必须说清「什么时候会送达」，不能只说「已排队」 | 完成 |
| R4 | 审批回执能带一句说明，写进工具结果给模型看 | 未做 |
| R5 | 这 5 条去哪了：`messages` 和 `queued_dispatch_messages` 都查不到，最终被 stop 丢掉 | 已定位 + 现在可查（`pending_steer_messages`） |

## 明确未做（后续任务，不夹带）

- **R4 审批说明**：`resolve_approval` 还没有 `note` 字段。
- **工具内部不可中断**：一个 30 分钟不返回的工具调用期间，steer 仍然只能等到它返回。
  本次保证的是「不丢 + 可查 + 下一回合一定送到」，不是「当场打断工具」。
- **`dispatch_message_status` 查 steer 行**：目前只能查 queued send 的行，steer 行状态
  需要按 `pending_steer_messages.id` 查；对外回执尚未带上该 id。

## 治理说明

- `docs/testing/scenario-registry.json` 是信任根文件，普通 PR 不得修改；
  本次没有动它。
- `src-tauri/src/commands/interjections.rs` 与
  `src-tauri/src/agent/delivery_scenario_repair.rs` 原先没有场景映射，按仓库规则在
  `docs/testing/scenario-mappings.json` **追加**了映射（HLT-004 / CXD-002），未改动已有条目。

验收记录：`docs/acceptance/m261.md`。
