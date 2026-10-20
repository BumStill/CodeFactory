# thin 会话遗留 objective-supervisor 守卫（R1/R3 补充）

状态：已落地（同一 PR #782 的第二个 commit；补丁 A/B 与回归断言均已验证）

## 现象（v1.90.1，2026-10-21 01:00）

- 01:00:23 thin 会话 `d2a14e59`（只读浏览器任务）已正确回答并结束。
- 01:00:57 `objective-supervisor:chat` 写入 `decision_applied / waiting_system /
  objective_progress_stalled`。
- 01:01:13、01:02:12 该会话被唤醒两次，模型两次回复「没有需要继续的东西」，
  同一答案共出现 3 次。

结论：R1/R3 的守卫只覆盖了 `DecisionRouter` 结算路径，遗留 supervisor 的
「扫描 → 决策 → 唤醒」两条链路仍会作用在 thin 会话上。

## 根因（三处，按影响排序）

1. **唤醒源** `src-tauri/src/agent/objective_supervisor.rs:701`

   ```rust
   if super::thin::is_thin(&pool, session).await {
       return crate::commands::chat::resume_chat_objective(app, objective, execution_permit).await;
   }
   ```

   thin 会话没有可用 adapter，于是代码把「走不了 adapter」降级成「照样用遗留
   chat 恢复路径唤醒」。thin 的等待归 `thin_state` 心跳与四态行所有，这条唤醒
   正是 01:01:13 / 01:02:12 两次重复回答的来源。

2. **决策源** `objective_supervisor.rs:1189 reap_active_objectives_whose_run_control_settled`

   该扫描的 SQL 只排除 `lease_owner`、`next_observation_at`、
   未完成 remediation 和活着的 run control，**没有 thin 过滤**。姐妹函数
   `ObjectiveStore::reap_stalled_active_objectives`（`objective.rs:2519`）已有
   R7 的 `NOT EXISTS (SELECT 1 FROM sessions thin ... delivery_mode='thin')`，
   此函数漏掉，于是 thin 会话在 run control 结算后 5 秒内被判定为 stall。

3. **兜底** `apply_decision_inner`（本 PR 已加）对 thin 直接 retire，能在决策
   写入前拦下；但 v1.90.1 没有该守卫，且即使有守卫，1、2 两处仍会先产生
   「唤醒 → 无内容」的噪声，因此必须在源头修。

## 补丁

### 补丁 A：扫描层加 thin 过滤（与 R7 同语义、同 schema 容错）

`objective_supervisor.rs`，在 `reap_active_objectives_whose_run_control_settled`
的 SQL 之前：

```rust
let thin_guard = if table_exists(pool, "sessions").await? {
    "AND NOT EXISTS (SELECT 1 FROM sessions thin \
     WHERE thin.id = objective.session_id AND thin.delivery_mode = 'thin')"
} else {
    ""
};
```

并把它插入 `WHERE` 子句（建议紧接 `AND objective.next_observation_at IS NULL`），
SQL 主体改为 `format!` 拼接；`table_exists` 从 `super::objective` 引入（该模块
已有同名 helper，若可见性不足则在本文件内 `sqlx::query_scalar` 查
`sqlite_master` 一次）。

### 补丁 B：唤醒层不再唤醒 thin 会话

同一文件 `drive_adapter` 的闭包内，把 701 行分支从「照样 resume」改为
「thin 不进入遗留恢复，直接放弃本次唤醒」：

```rust
if let Some(session) = objective.session_id.as_deref() {
    if super::thin::is_thin(&pool, session).await {
        return Ok(());   // thin 等待由 thin_state 心跳负责，遗留闭环不得唤醒
    }
}
```

若 `drive_adapter` 的返回类型是 `anyhow::Result<()>` 之外的具体类型，按该
类型返回「未处理」的等价值；不要改用 `bail!`（会把 thin 会话记成故障）。

### 补丁 C：保留 `apply_decision_inner` 的 thin retire 兜底

已在本 PR 中，无需改动；它保证任何漏网决策在 thin 会话上都不会落成
`waiting_system`。

## 回归测试

1. **扫描层**：扩展现有 `supervisor_reaps_an_active_objective_whose_run_control_already_settled`
   （`objective_supervisor.rs:1517` 起）。在同一个 pool 里额外插入
   `sessions(id='session-thin', delivery_mode='thin')`，再为它建一个
   active objective + 已 `completed` 的 `chat_run_controls`，断言：
   `reaped` 仍只含原 `orphan`，thin 目标 `status == Active` 且
   `next_observation_at.is_none()`、`failure_code.is_none()`。
2. **唤醒层**：`drive_adapter` 的分支难以单测，用行为断言替代——在 thin 目标上
   运行同一扫描后断言 `objective_decisions` 无新行
   （`SELECT COUNT(*) FROM objective_decisions WHERE objective_id=?` 为 0）。
   这条断言同时覆盖 A 和 B 的意图：thin 既不结算也不唤醒。
3. 反例（必须仍工作）：非 thin 会话的 `session-healthy` 断言保持不变，
   证明过滤没有把整条回收链关掉。

## 验证命令

```bash
node scripts/cargo-shared.mjs test --manifest-path src-tauri/Cargo.toml thin
node scripts/cargo-shared.mjs test --manifest-path src-tauri/Cargo.toml objective
node scripts/cargo-shared.mjs test --manifest-path src-tauri/Cargo.toml --lib
python tools/governance/run_scenario_harness_gate.py --stage local --repo . --policy-repo .
```

## 交付

- 与 R1/R3/R4 同一分支、同一 PR（#782），不新开 PR。
- PR 正文补一节「真机证据 v1.90.1 01:00」：stall 决策时间戳、两次唤醒时间戳、
  重复答案次数，以及 A/B/C 三处防线各自对应的证据。
- Scenario-Test 已覆盖 `objective`/`thin` 相关场景；若补丁 B 改变了 thin 的
  recovery 行为，需复核 `docs/testing/scenario-registry.json` 中 thin 场景的
  `change_patterns` 是否已包含 `objective_supervisor.rs`。
