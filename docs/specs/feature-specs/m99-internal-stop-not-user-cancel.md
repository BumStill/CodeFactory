# M99：系统内部停止不得被记成「用户取消」

- 状态：已实现（待发布验收）
- 关联：M30（结算必须停掉 live run）、M47 / #685（用户停止围栏）、CF-TRUTH-R3（停止归因唯一权威）
- 真机现象日期：2026-10-17，版本 v1.88.12，会话 `a1ce7c0b`

## 背景与用户决策

用户看到的是一句自相矛盾的记录：屏幕锁定、HID 空闲 8.4 小时、派单入口审计日志里
没有任何 stop/cancel 请求，但会话 `a1ce7c0b` 的 objective 在 17:46:34 变成
`cancelled`，同时 `chat_session_cancel_intents` 里出现了一条
`requested_at=17:46:34` 的 `requested` 行——也就是系统替用户"按了停止"。

用户决策（本任务）：

1. 只有用户或编排方的显式停止入口有权写"用户停止"这个事实；
2. 内部结算/对账/围栏停止必须仍然能停掉 live run（M47 的停止围栏不能削弱）；
3. 自己推送的提交不能被判成 "unrecognized head"；
4. 先把"谁有权说用户停止"盘点清楚，再动手。

## R1：17:43–17:46 时间线与根因（数据）

| 时间（用户本地） | 证据 | 说明 |
| --- | --- | --- |
| 17:43:39 | `agent_loop_error`：`local commit receipt cannot absorb source edits made after the commit` | 交付轮在写后 receipt 校验处把"提交后又有源码改动"当成致命错误（`delivery.rs` 观测函数），而不是签发新的提交 receipt。 |
| 17:46:21 | `agent_loop_error`：`remote branch codefactory/objective-2f12cc53… moved to an unrecognized head 354180f8…; expected persisted 4b63f151…` | 354180f8 是**该会话自己推送的提交**。推送成功后新 head 尚未落成持久身份，只读对账就把它判成外来移动。同时该失败的 `failure_code` 被记成 `provider_endpoint_unavailable`，与实际原因无关。 |
| 17:46:34 | objective → `cancelled`；`chat_session_cancel_intents` 出现 `requested` 行 | 内部结算在停 live run 时抬起了进程内 cancel 标志，setup 收敛只看到"标志已抬起"，于是调用显式停止入口 `request_chat_run_cancel`，把用户意图写进了持久表。 |
| 全程 | 派单入口审计日志无 stop/cancel；屏幕锁定、HID 空闲 8.4h | 没有任何用户/编排方停止入口被调用。 |

根因链（代码级）：

1. `ObjectiveStore::request_live_run_stop_before_settlement`（`agent/objective.rs`）在结算前把
   `chat_run_controls.status` 置为 `cancel_requested`、写 `cancel_requested_at`，并调用
   `request_chat_run_stop` 抬起进程内 `AtomicBool`。该标志**没有来源信息**。
2. `converge_aborted_chat_setup`（`commands/chat.rs`）只判断 `control.cancel.load(..)`，
   为真就调用 `request_chat_run_cancel`——该函数的契约注释明确写着"这是显式停止入口"，
   它会 `INSERT ... chat_session_cancel_intents (status='requested')`。
3. `has_durable_stop_request` 与启动恢复都把 `chat_session_cancel_intents` 当作唯一权威，
   于是内部结算被读回成用户停止，objective 以 `cancellation_provenance=explicit_cancel` 收尾。

结论：问题不在"停止"本身，而在**停止信号的来源被丢弃**。只要内部停止与用户停止共用同一个
无法区分的信号，"系统取消"就必然会被写成"用户取消"。

## 决策

1. **停止信号带来源**：新增内部停止入口 `request_chat_run_stop_internal`，它照样抬起协同标志
   （M30 的"结算必须先停 live run"保持不变），同时登记 `run_instance_id` 的内部归因；
   `chat_run_stop_is_internal` 供收敛路径查询，标志消失时归因一并回收。
2. **只有显式入口写用户意图**：`chat_session_cancel_intents` 的**唯一**写入口是
   `ObjectiveStore::request_chat_session_cancel`，其调用者只有 `cancel_chat`（界面停止）与
   `request_chat_run_cancel`（显式停止入口，仅由 `cancel_chat` 与**非内部**的 setup 收敛调用）。
   内部停止改走 `persist_internal_chat_run_stop`：只写 `cancel_requested_at` 这一条运行事实，
   不写会话级意图，也不把 objective 的归因改成 `explicit_cancel`。
3. **内部停止不得围栏自己的结算**：`cancel_requested` 状态原本会让"非取消"决策被
   durable-cancellation guard 拒绝。内部停止因此只盖章时间戳并把 pending 状态交还给结算，
   由结算写最终状态（与 M30 `request_live_run_stop_before_settlement` 第 4 步同语义）。
4. **识别自身推送**：只读对账见到"既不是持久 head 也不是 receipted parent"的远端 head 时，
   若该提交在本仓库对象库里存在、出现在本仓库 reflog 中、且是持久 head 的后代，并且工作区干净，
   则判定为本会话自己的推送，继续对账而不是报 `unrecognized head`。未知改写或从未在本地出现过的
   外来 head 仍被拒绝（原有 fence 用例保持通过）。
5. **错误分类**：上述两类错误都是交付/对账事实，不再落到 `provider_endpoint_unavailable`
   （provider 恢复只在传输层判定，本任务不改其入口，仅在规格中固定该边界）。

## Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| R1 | 用数据写清 17:43–17:46 时间线与根因 | 本文档 R1 表格 + 代码路径引用 |
| R2 | 内部结算/对账/围栏停止不得写用户取消意图，objective 保持系统归因 | `commands::chat::tests::internal_settlement_stop_does_not_become_a_user_cancel`（先失败后通过）、`admitted_setup_cancel_wins_over_technical_recovery`（用户停止仍为 cancelled） |
| R3 | 自身推送不被判为 unrecognized head | `agent::delivery::tests::own_pushed_head_is_not_an_unrecognized_remote_head`；`takeover_refuses_an_unreceipted_local_head_without_any_remote_mutation` 仍通过 |
| R4 | 测试先失败后通过，覆盖三条路径 | 见"测试矩阵"，PR 记录红→绿 |
| R5 | 家族盘点：谁有权说"用户停止" | 本文档 R5 表格 |

## R5：`chat_session_cancel_intents` 与 cancel 标志的全部写路径

| 位置 | 谁在写 | 是否有权声明"用户停止" | 处理 |
| --- | --- | --- | --- |
| `ObjectiveStore::request_chat_session_cancel` | 唯一 INSERT/置 `requested` 的地方 | 调用者决定 | 保留，作为唯一权威入口 |
| `commands::chat::cancel_chat` | 界面/菜单栏停止（含派单入口 stop） | 是（显式用户/编排方） | 保留 |
| `commands::chat::request_chat_run_cancel` | 显式停止入口，仅由 `cancel_chat` 与非内部 setup 收敛调用 | 是 | 收敛侧新增内部归因判断后才调用 |
| `commands::chat::converge_aborted_chat_setup` | 原先只看进程内标志 | 否（可能是结算） | **改为按来源分流**：内部 → `persist_internal_chat_run_stop` |
| `ObjectiveStore::request_chat_session_cancel`（内部结算） | 不存在该调用 | — | 保持不存在 |
| `chat_run_controls.status='cancel_requested'` / `cancel_requested_at`（`request_live_run_stop_before_settlement`） | 内部结算 | 否 | 只作运行事实；标志改走内部入口 |
| `chat_run_controls.status='cancel_requested'`（`request_chat_run_cancel`） | 显式停止 | 是（伴随会话意图） | 保留 |
| `consume_chat_session_cancel` / `consume_pending_*` | 消费已有意图 | 否（只消费） | 保持只读权威：无意图行则不取消 |
| `history_session_smoke` / 测试 fixture | 合成数据 | — | 合成会话/合成 DB，不触碰生产库 |

## Applicable Harnesses

- **Spec Harness**：本文件 + Requirements Traceability + 测试矩阵。
- **AI Collaboration Harness**：假设（停止信号丢失来源导致归因错误）先用代码路径与真机时间线核对，
  再由测试证伪/证实；审查点为"内部停止是否仍能真正停住 live run"。
- 未触发 Viewport / Payload / Release 之外的新面：本次不改 UI、不改发布管线。

## 测试矩阵

| 用例 | 类型 | 断言 |
| --- | --- | --- |
| `internal_settlement_stop_does_not_become_a_user_cancel` | Rust 单元（先红） | 内部停止后 `chat_session_cancel_intents` 计数为 0、`has_durable_stop_request` 为 false、objective 非 `cancelled`、无 `explicit_cancel` |
| `admitted_setup_cancel_wins_over_technical_recovery` | Rust 单元（回归） | 用户显式停止仍把 objective 置为 `cancelled` 且归因为 `explicit_cancel` |
| `own_pushed_head_is_not_an_unrecognized_remote_head` | Rust 单元（先红） | 自身推送的子提交被 takeover 接受 |
| `takeover_refuses_an_unreceipted_local_head_without_any_remote_mutation` | Rust 单元（回归） | 外来/未 receipt 的 head 仍被拒绝 |
| `cargo test --lib`（全量） | Rust 全量 | 无回归 |
| `pnpm test` / `pnpm build` | 前端 | 无回归 |

## Implementation Notes

- 内部归因是**进程内**状态：重启后不存在"内部/用户"的历史区分，此时唯一权威仍是
  `chat_session_cancel_intents`。内部停止从不写该表，所以重启后不会被误读成用户停止。
- 内部停止保留 `cancel_requested_at`，是为审计与后续对账留下"运行被指示停止"的时间点；
  它不改变 objective 的终态归属。

## 遗留（不在本次范围）

- `observe_receipted_local_commit` 目前是**只读**证明函数：当 receipt 覆盖的提交之后又出现源码改动，
  它会返回 `local commit receipt cannot absorb source edits made after the commit`。
  要"生成新的提交 receipt"必须在持 `MutationPermit` 的交付变更边界上签发新 receipt，
  不能在只读校验器里提交。本次先把该错误限定为交付/对账事实（不再落成
  `provider_endpoint_unavailable`），新 receipt 的签发作为后续任务。
