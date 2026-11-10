# M272：同一次模型切换只提示一次

## 背景

2026-11-08 会话 `833ac8cb`：DeepSeek 当日预算用完后自动切到 `gpt-6.1-sol`，
对话里「已切到 gpt-6.1-sol：DeepSeek 今日预算已用完…」出现了 17 次，而实际只发生
了一次切换。

## R1：这条提示在哪里生成、为什么每次调用都写一次

链路（都在后端，前端只负责把 `turn_notice` 渲染成一条居中系统说明）：

1. **判断与文案**：`src-tauri/src/agent/usage_windows.rs:218`（`DailySpendCap`
   的「今日预算已用完，…后继续」）与 `quota_cap.rs` 的 `cap_notice` 给出原因；
   `src-tauri/src/agent/failover.rs:856` 用 `format!("已切到 {}{}", …)` 拼出
   这句话，并在 `failover.rs:162`（`skip_for_quota`）把这次切换登记进 route
   state，作为 `RouteChange { from_*, to_*, reason, notice }`。
2. **每轮重新判断**：`model_transport.rs:1970` 在**每个** loop round 里调用
   `admit_usage_window` / `admit_model_availability`。只要原路由仍然不可用（预算
   未到重置点、主模型仍在冷却），这一轮就会再返回一条「同样的」`RouteChange`，
   而 `model_transport.rs:1960` 的 `transitions` 会把它带进 `ModelResponse`。
3. **写入对话**：`crates/agent-loop/src/run.rs` 的 `persist_route_change` 收到
   `RouteChange` 就 `persist_gate_message(notice, "turn_notice")` 一次，并 emit
   一条 `CompletionGateAction`。修改前没有任何去重键，所以「每个 round 一次」＝
   「一次切换 N 条提示」，长会话里累积到十几次。

结论：重复不是前端渲染问题，而是**后端把「当前状态」当成「状态变化」每轮重放**。

## R2：同一个会话里同一次切换只提示一次

去重放在写入层（`persist_route_change`），键是**切换方向 + 原因**：

- 方向：`from_endpoint:from_model -> to_endpoint:to_model`；
- 原因：`RouteChange.reason`（空白折叠后参与键，不进入用户可见文本）。

写一次即登记 marker（`Persistence::persist_gate_message_once` 的既有语义：marker
同会话唯一，重复写入返回 `false` 且不产生消息行），重复的 switch 直接返回、也不
再 emit 事件。因此：

- 同一次切换连续 N 轮 → 对话里 1 条；
- 原因变了（额度重置后再次用尽、主模型 5 小时窗口再次触顶）→ 属于新信息，再提示一次；
- 切回原模型（方向相反）→ 方向不同，再提示一次。

路由选择本身（谁被跳过、切到谁）完全不变（R3）。已经写进历史的重复提示不删除（R4），
修复只影响此后的写入。

## 非目标

- 不改 `failover.rs` / `quota_cap.rs` 的选路与预算判断。
- 不改前端 `turn_notice` 的渲染样式。
- 不回填、不清理历史会话里的旧重复提示。

## 测试

`src-tauri/crates/agent-loop/src/run.rs`：

- `m272_same_route_change_is_announced_once`：连续 4 个 round 报同一次切换 →
  只落 1 行 notice、只 emit 1 个事件（先看到失败：修改前为 4）。
- `m272_new_reason_or_reverse_switch_is_announced_again`：同方向换原因、以及切回
  原模型，各自再提示一次 → 共 3 条。

场景映射：`src-tauri/crates/agent-loop/src/*.rs` 已由 `HLT-001` 覆盖，PR body 声明
`Scenario-Test: HLT-001`。

## 真机验收（由编排方发版安装后完成）

1. 用会触发预算上限的会话（或临时把日预算调到极低）让 DeepSeek 被跳过；
2. 连续发 2–3 轮消息，确认对话里这条切换提示只有 1 条；
3. 次日预算重置后（或临时恢复预算）发一轮，确认出现一次切回提示；
4. 打开旧会话 `833ac8cb`，确认历史里旧的重复提示仍在（R4 未回填删除）。
