# M145 — 停止的取消意图不得变成永久的准入栅栏

## 事故

2026-10-21，用户配置自托管 DeepSeek 端点后，界面停在「正在做 · 等待中」，
没有 thinking、没有工具调用、没有错误，直到手动点停止；重启后恢复。
截图里还有连续两条「已停止…」通知。用户确认不是模型问题。

## 根因（编排方定位）

1. `cancel_chat`（`src-tauri/src/commands/chat.rs`）先写会话级取消意图
   `chat_session_cancel_intents(status='requested')`，再在
   `cancel_system_owned_chat` → `consume_chat_session_cancel` 里结算。
2. 每次新消息准入（`objective.rs` 的
   `ensure_or_continue_chat_objective_in_tx`）只要存在 `requested` 意图就
   `bail!("chat admission fenced by durable session cancellation")`。
3. 若第 1 步的结算失败（Windows 上 `database is locked`），意图永久停在
   `requested`：之后每条消息准入都被拒，而界面已经进入「正在做 · 等待中」，
   于是永久卡住。只有启动时的 `consume_pending_chat_session_cancellations`
   才清理，所以重启能恢复。
4. 每次停止都会无条件插入一条「已停止…」消息，同一次停止被重复记多次。

## 需求

- **R1** 用户停止后发出的新消息本身就是「继续」的明确意图：准入时若发现
  **早于本消息**的 `requested` 意图，先完成该取消（结束仍存活的旧回合/对象），
  把意图结算为 `settled`，再正常准入新回合；绝不因旧的取消意图拒绝新消息。
- **R2** 停止结算失败要重试（DB 忙），且失败不得留下永久栅栏；
  准入失败不得把界面/thin 状态置为 running——要么成功开始，要么一句人话错误。
- **R3** 同一次停止只插入一条通知；无交付的会话用不提 PR 的文案
  （与 M144 R2 协调，避免改同一处冲突时先合入 main）。
- **R4** 测试（先失败）：停止 → 结算失败 → 再发消息 → 新回合正常开始；
  连续两次停止只有一条通知；停止后立即发消息 → 正常开始；Windows CI 跑一遍。

## 实现

- `ObjectiveStore::settle_stale_chat_session_cancel(session_id, before_ms)`：
  只有当存在 `status='requested' AND requested_at < before_ms` 的意图时才结算；
  结算走 `consume_chat_session_cancel_until`，把该会话仍存活的 Objective
  结束掉并把**早于本消息**的意图置为 `settled`，晚于本消息的并发停止仍保留栅栏。
- `admit_persisted_chat_turn` 在进入台账事务**之前**调用它：新消息先完成遗留停止，
  再由事务内的栅栏判断是否存在并发停止。
- `settle_chat_stop_with_retry`：停止结算按 0/150/400ms 退避重试 3 次；
  重试耗尽时如实记录并返回 0，不把整个停止命令变成失败，
  意图保持 `requested` 并由 R1 在下一条消息完成结算。
- 栅栏错误改为一句人话：`会话刚被停止，这条消息没有启动新回合，请再发一次`。
- `insert_stop_notice_once`：同一「用户回合」内已有 `已停止%` 通知时不再插入；
  新用户消息之后重新允许一条。`stopped_delivery_notice(None)`
  改为「这一步不再继续」，不再声称远端有一个不存在的 PR。

## 验证

- `cargo test --lib stale_stop_intent`：遗留 `requested` 意图被新消息结算、
  旧 Objective 变为 `cancelled`、闸门解除；晚于本消息的并发停止仍保持 `requested`。
- `cargo test --lib stop_notice_is_inserted_once`：同一回合两次停止只产生一条通知，
  新用户消息之后重新产生一条。
- `cargo test --lib stop_fence`：11 passed，含「无交付会话的文案不提 PR」。

## 附带（次要）

模型请求的有界等待（连接 10s / 首字节 60s / 流空闲 120s，超时
`MODEL_FIRST_BYTE_TIMEOUT` 走既有回落）与本任务同一 PR，见
`http_util::model_http_client`。它不是本次卡死的主因。
