# M197（紧急）：`waiting_wake` 没有任何唤醒源——会话永久挂起

## 1. 问题（用户看到的）

v1.91.51 现场：30 个 thin 会话停在 `waiting_wake`，但 `wakeup_schedules` 里没有
属于它们的 `pending` 行，`background_tasks` 里也没有运行中的任务。用户点开这些
会话，只看到「等唤醒：PR #xxx 合并」，然后**永远**不会有下一句。它们的
`thin-repair` 唤醒都已经 `fire_count=1` 完成，之后系统再没给它们挂任何来源。

结论不是「某个入口写错了状态」，而是**没有任何运行中的东西在执行
「`waiting_wake` 必须有唤醒源」这条不变量**：推导（及修复）函数只存在于启动路径
和测试里，主循环从不周期对账；而 `wake_source_cover` 只认「运行中的后台任务 +
pending 定时唤醒」，把**已经登记、有界心跳的 PR/CI 观测**排除在外，于是刚开完 PR
的正常等待在推导层看就是「没有唤醒源」。

## 2. 需求

- **R1 派生即判定（fail-closed）**：任何把会话派生成 `waiting_wake` 的路径，
  写库前必须先确认存在具体唤醒源。没有源时不得留在 `waiting_wake`：
  - 系统自己能推进（PR/CI/交付对象、后台任务、定时唤醒）→ 立刻补一个真实唤醒源
    并写明来源（系统自动重试/自愈），仍是 `waiting_wake`，但这次有真来源；
  - 需要人（或来源已彻底消失且自愈次数用尽）→ `waiting_user` + 人话原因；
  - 已无事可做 → `ended`。
- **R2 外部观测是唤醒源**：已登记、由系统按 ≥180s 有界心跳观测的 PR/CI
  （`thin_external_observations.completed_reason IS NULL`）必须算作唤醒源；
  PR 已合并时明确挂出收尾唤醒。
- **R3 启动与周期对账**：应用启动后立即对账一次，此后按周期（180s）持续对账
  全部 `waiting_wake` 会话；对账幂等，重启不会让存量脏状态永久存活。
- **R4 界面人话**：「等待中」必须写明**在等什么**和**预计何时**被唤醒，不允许
  空原因（禁止只有「等唤醒」四个字）。
- **R5 回归（合成数据）**：唤醒触发后不再挂新唤醒的会话不会卡在 `waiting_wake`；
  等 PR 合并的会话合并后立即收尾；存量脏状态在一次对账内被修复且幂等。
- **治理**：本条属于 release-facing 主路径缺陷，`HLT-004`（thin 状态跨重启
  收敛，P0）作为硬门槛场景覆盖 `src-tauri/src/agent/thin_derive.rs` 的改动。

## 3. 设计

### 3.1 唤醒源的口径（`background_tasks::wake_source_cover`）

```rust
pub struct WakeSourceCover {
    pub running_tasks: usize,
    pub pending_wakeups: usize,
    pub external_observations: usize, // 新增：活着的 PR/CI 观测行
}
```

`is_covered()` = 三者之和 > 0。观测行由心跳按 GH 配额策略（≥180s）推进，本身就是
durable 的唤醒源；把它算进来，才不会把「刚开完 PR 的正常等待」误判成无源等待。

### 3.2 派生守卫（`thin_derive::enforce_wake_source`）

`enforce_wake_source(db, session)` 是 R1 的唯一落地点，返回
`WakeGuard::{NotWaiting, Covered, Rearmed, Escalated, Ended}`：

1. 不是 `waiting_wake` → `NotWaiting`；有执行器存活时跳过（不抢正在跑的回合）。
2. `wake_source_cover` 已覆盖 → `Covered`。
3. 没有 `waiting_for` → `ended`（无事可做），审计 `thin_wait_guard`。
4. `is_system_followed`（PR/CI/交付对象/后台任务/定时唤醒）：
   - 先 `ensure_pr_monitor` 认领 PR 并补登记观测；补完有源 → `Covered`（系统自愈）；
   - 否则挂一个 due-now 的 durable 自愈唤醒（`id = thin-invariant-<session>`，
     固定 id 保证不重复堆积），审计并保持 `waiting_wake` → `Rearmed`；
   - 自愈次数达到 `MAX_SELF_CONTINUE = 2` → `waiting_user` + 人话 → `Escalated`。
5. 其余（要人确认、来源认不出）→ `waiting_user` + 人话 → `Escalated`。

人话里不出现「唤醒源 / 外部事项 / waiting_*」等内部词。

### 3.3 周期对账（`thin_derive::reconcile_waiting_wake_sources`）

扫描 `thin_session_state` 中 `state='waiting_wake'` 的会话，逐个
`ensure_pr_monitor` → `heartbeat_external`（内部已有 ≥180s 新鲜度门与 GH 配额）→
`enforce_wake_source` → `refresh`；最后统一 `thin_state::sweep`（30 分钟失效守卫）。
返回 `WakeReconcileReport { scanned, covered, rearmed, escalated, ended, swept }`。

装配：`background_tasks::start_runtime` 启动时先对账一次，之后每 `180s` 跑一次
（与 R2 的有界观测节奏一致）。对账全部走 durable 状态，进程重启不丢。

### 3.4 界面（R4）

`thin_state::project_turn` 的 `waiting_wake` 标签改为
`等唤醒：<在等什么>（<预计何时/谁会叫回来>）`，缺失字段时用人话兜底，
不再输出空原因的「等唤醒：」。

## 4. 验收（可执行）

- `cargo test --manifest-path src-tauri/Cargo.toml m197`：
  - `m197_wake_without_source_is_never_left_waiting`（R1/R5）
  - `m197_merged_pr_gets_closeout_wake`（R2）
  - `m197_stock_dirty_states_repaired_and_idempotent`（R3/R5）
  - `m197_self_continue_is_bounded_then_asks_user`（R1/R5）
  - `m197_active_observation_counts_as_wake_source`（R2）
- `thin_state` 单测断言等待标签同时包含等待对象与唤醒时点（R4）。
- `python tools/governance/validate_repo_governance_baseline.py` 与
  `python tools/governance/run_scenario_harness_gate.py --stage local` 通过。

## 5. 证据与限制

- 上述用例均使用合成 fixture（内存/临时 SQLite），不依赖真实 `gh`、不联网。
- 真实应用验收（打开 app 观察 30 个存量会话在一个对账周期内离开「无源等待」）
  由用户在真机执行；本条 PR 的证据是单测 + 治理门禁 + headless 契约，不做
  虚假 live 声明。
