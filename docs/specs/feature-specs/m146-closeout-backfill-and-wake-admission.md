# M146：收尾回填不得给历史会话起回合，唤醒没开始回合就不得显示 running

## 现场（v1.91.14，安装于 10-22 01:11:29）
- 01:12:35–01:13:20 一次性唤醒 23 个**早已 ended** 的旧会话做 §8 收尾，全部零产出、只消耗模型调用。
- 其中 `d632e17a`（PR #776）01:12:51 被唤醒后 **running 30 分钟无回合、无助手回复**。

## 根因（均为运行事实，可复放）
1. **回填不看事实**：`heartbeat_external` 用缓存的 `thin_session_state` 判定「是否还在等待」，而
   `derive_state` 对任何带 `thin_delivery_refs` 的会话在观察未落库前都推导为 `waiting_wake`。
   只要升级后跑一次心跳，历史会话就都变成「等待收尾」。现场事实：PR #776 `mergedAt=2026-10-20T13:50:02Z`，
   该会话的最终回答在 13:50:35（合并后 35 秒）——它**早就收尾完了**。
2. **唤醒没有回合却置 running**：`send_message` 的准入把已结算会话判为 `already_settled`，唤醒文本被
   持久化成一条 `user` 消息后直接返回 `Ok`，没有回合运行；`derive_state` 见「最新用户消息晚于终态」
   即返回 `Running`，于是永久 running。

## 规则
- R1 收尾唤醒只给「交付后仍在等待」的会话：`derive_state` 必须是 `waiting_wake` / `waiting_user`，
  且**合并时间之后没有任何最终回答**。已 ended、已 close_objective、已收尾的会话只把外部事项
  标记完成（`closeout_queued=1`），**不起任何模型回合**；升级回填同理，产出为零回合。
- R2 唤醒要么真正开始回合，要么不置 running：最新用户消息是固定形状的唤醒文本
  （`定时唤醒 <id>：…\n节奏：…`）、其后没有助手回答、且执行者已死时，投影为 ended 而不是 running。
- R3 收尾/修复回合限流：同一时刻最多 2 个（`thin-closeout-*` / `thin-repair-*` 处于 `pending`，
  或 `done` 且 600 秒内投递过）。让出额度时不标记完成，留待下一轮心跳重试。
- R4 失败原因补全：PR `mergeable=CONFLICTING` / `mergeStateStatus=DIRTY`（合并冲突）与
  `merge-queue: blocked`（队列退回）都要带原因唤醒会话。

## 证据
- `src-tauri/src/agent/thin_derive.rs` 新增 `m146_*` 五个测试，已登记到 `docs/testing/scenario-registry.json` 的 HLT-004。
- 真实事实复放：对生产库副本断言 `d632e17a` 满足 R2 判定（唤醒文本 + 零助手回答），且最终回答
  时间戳（1792504235625）晚于 PR #776 的 `mergedAt`（1792504202000），因此只会静默标记完成。
