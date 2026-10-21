# M150 终态 PR 不再反复唤醒修复回合

## 问题

现场：PR #829 已 `MERGED`（merge commit `32df938c` 在 `origin/main`），但会话仍每 60 秒收到一次
`thin-repair` 唤醒，提示「PR #829 检查失败：scenario-gate-pr」——那是被取代的旧 head
（`f1ebfa82`）的历史失败。用户实际数据库里可复现：

```
completed_reason = 'PR #829 检查失败：scenario-gate-pr'
closeout_queued  = 1
merged_at        = 1792622995000   -- 合并事实已经落库
```

## 根因

`src-tauri/src/agent/thin_derive.rs` 的观测入库语句（旧第 172 行）：

```sql
UPDATE thin_external_observations
   SET closeout_queued = CASE WHEN completed_reason IS ? THEN closeout_queued ELSE 0 END,
       completed_reason = COALESCE(?, completed_reason),
       merged_at = COALESCE(?, merged_at)
```

当 `ceiling = ThroughRelease` 且 PR 已合并、版本尚未发布时，最新观测既不是失败
（`failure_reason` 要求 `state == OPEN`）也不是完成（`completion_reason` 要求已发布），
于是绑定值为 `NULL`：

1. `closeout_queued` 的 `CASE` 因 `completed_reason IS NULL` 不成立而归零；
2. `COALESCE` 把**旧失败原因原样保留**。

两者叠加 = 每轮心跳都重新排队一次修复唤醒，永不收敛。失败原因本来是「当下事实的投影」，
却被当成历史资产保存。

## 变更

抽出 `apply_observation()`（`thin_derive.rs`），心跳只调用它：

- 观测结果**不含任何失败**且库里现存原因属于失败类（`%检查失败：%` / `%已关闭但未合并%`）
  → 清除 `completed_reason`，保留 `merged_at`，**不动** `closeout_queued`（不再排唤醒）；
- 其他情况沿用原语义：原因变化时重置 `closeout_queued` 以便重新排队，原因相同则保持不变
  （避免同一失败重复唤醒）；
- 非失败原因（如「已合并」）永不被自动清除，它是已经发生的事实。

## 验收

- `m150_terminal_pr_clears_stale_failure_instead_of_requeueing_repair`：终态观测后旧失败被清除、
  `closeout_queued` 保持 1、`merged_at` 落库；
- `m150_live_pr_new_failure_still_requeues`：PR 仍打开且出现新失败时照旧重新排队一次修复回合；
- `cargo test --lib thin`：110 通过。

Scenario-Test: HLT-001, HLT-004
