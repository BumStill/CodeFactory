# M148：会话在等 PR/CI 时被莫名判 ended（`superseded_by_user_reprompt`），PR 失败无人处理

## 用户可见现象

一条已完成本地交付、PR 仍在合并/CI 中的会话，在收到后台任务结束通知后被判成
`已结束（superseded_by_user_reprompt）`。此后：

- 侧栏把「等外部 PR」显示成「已结束」，与真实状态相反；
- M142 的系统 PR 监视不再作用，PR 的 CI 失败没有任何回合被唤醒处理；
- 直到编排方手动发消息，失败才被发现。

## 证据（v1.91.14–v1.91.17）

- c90adee7（M142，10-21 22:50）、0a29f96b（M145，10-22 00:44）、
  ffa13bc4（M147，10-22 04:5x）：交付 PR 后处于等待 PR/CI，thin 状态变为
  `ended / superseded_by_user_reprompt`；期间没有用户或编排方消息。
- ffa13bc4 的 PR #827 随后 CI 失败，因会话已 ended，系统 PR 监视/唤醒不再作用，
  直到编排方手动发消息。

## 根因

1. **来源未区分**：`ChatWakeSink`（后台任务结束通知）走的是与用户输入完全相同的
   `commands::chat::send_message` → `admit_persisted_chat_turn` →
   `ensure_or_continue_chat_objective_in_tx` 路径。该路径无条件调用
   `settle_superseded_chat_turns_in_tx(..., "superseded_by_user_reprompt")`，
   把上一回合写成「被用户重新发起取代」。
2. **终态事实优先级**：`thin_derive::derive_state` 把任何非 legacy 的
   `chat_turn_state.terminal_reason` 当作终止事实。系统唤醒写下的
   `superseded_by_user_reprompt` 因此让会话进入 `ended`。
3. **监视随 waiting 一起消失**：M142 的系统 PR 监视只在 `waiting_wake` /
   `waiting_user` 上运行；`ended` 之后没有任何回路把它拉回工作。

## 需求（Requirements Traceability）

| Req ID | 要求 | 证据 |
| --- | --- | --- |
| R1 | 查明 `superseded_by_user_reprompt` 的写入路径与触发条件 | 见「根因」；`ChatWakeSink` → `send_message` → 准入函数无条件取代结算 |
| R2 | 系统产生的唤醒/后台任务通知/收尾消息不得 supersede 当前任务，应接续当前回合 | `TurnOrigin::SystemWake`：不写取代结算，写 `system_wake_continued`（`budget: system_continuation`） |
| R3 | 终止事实只限：最终回答且无外部进行中事项、用户/编排方 stop、连续 3 次真实失败；有未完成交付 PR 时任何路径不得判 ended | `unfinished_delivery` 守卫 + 用例 `unfinished_delivery_keeps_a_delivered_session_waiting` |
| R4 | 已被误判 ended 但仍有未完成交付 PR 的会话，系统 PR 监视仍应唤醒它 | 守卫使 `derive_state` 回到 `waiting_wake`（`wake_source`＝系统 PR 监视），M142 心跳恢复观测 |
| R5 | 先失败的测试覆盖四个场景 | `cargo test --lib -- agent::thin_derive agent::objective`（192 项）与全量 lib 套件 1905 项 |

## 决策

1. **来源是一等概念**：新增 `TurnOrigin { User, SystemWake }`。后台任务结束通知、
   定时唤醒、PR 监视心跳以 `SystemWake` 准入（`send_system_wake`）；只有
   `TurnOrigin::User` 可以取代当前回合。旧的 6 参数
   `ensure_or_continue_chat_objective_in_tx` 保留为转发 `User` 的包装，
   `admit_persisted_chat_turn` 同理，其余调用点无需改动。
2. **未完成的已交付 PR 是外部进行中事项**：`derive_state` 在终态推导前检查
   `unfinished_delivery`——存在 `thin_delivery_refs` 引用、且观测结果不是
   「已合并且 `closeout_queued=1`」时，返回 `waiting_wake` 而不是 `ended`。
3. **边界不放松**：
   - 真实的用户提问（`pending_question_at`）仍优先于监视；
   - 用户停止（`chat_session_cancel_intents`）与待审批仍在最前面处理；
   - PR 真正收口后，同一个终态事实照旧结束会话——守卫不是「永不结束」；
   - 前端 `invoke("send_message", ...)` 合同不变。

## Applicable Harnesses

| Harness | 适用性 | 覆盖方式 |
| --- | --- | --- |
| Spec Harness | 必需 | 本规格 + Req ID ↔ 用例映射 |
| Compatibility Harness | 适用 | 只读取既有列/表；`thin_external_observations` 缺表按「未收口」处理；旧 `superseded` 行照旧可推导 |
| Observation Harness | 适用 | 断言 thin 四态、`wake_source`、`system_wake_continued` 事件 |
| Release Harness | 本任务不切版 | 由后续发版任务处理 |
| Payload / Viewport Harness | 不适用 | 不涉及上传与布局 |
| AI Collaboration | 适用 | 本节 + PR 决策块记录假设与验证结果 |

## 测试矩阵

| # | 场景 | 期望 | Req |
| --- | --- | --- | --- |
| 1 | `superseded_by_user_reprompt` + 未收口 PR | `waiting_wake`，`wake_source`＝系统 PR 监视 | R3/R4 |
| 2 | 同一终态事实 + `closeout_queued=1` | `ended`（守卫有边界） | R3 |
| 3 | 用户停止 / 待审批 | 仍优先结束或等待用户 | R3 |
| 4 | 既有 M139/M140/M142/M146 场景 | 全绿，无回归 | R5 |

## 影响面

`src/agent/objective.rs`（`TurnOrigin`、系统接续事件）、
`src/commands/chat.rs`（`send_system_wake` 与来源透传）、
`src/agent/background_tasks.rs`（唤醒改用系统来源）、
`src/agent/thin_derive.rs`（`unfinished_delivery` 守卫）。

## Implementation Notes

- 测试 fixture 使用合成库表，不读取真实项目数据。
- 新增用例 `unfinished_delivery_keeps_a_delivered_session_waiting` 的判别力在于
  使用「已合并但 closeout 未入队」的观测：M142 既有分支不覆盖该组合，只有守卫能拦。
