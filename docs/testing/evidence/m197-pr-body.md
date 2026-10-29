## 问题

v1.91.51 现场：一批 thin 会话停在 `waiting_wake`，但 `wakeup_schedules` 里没有属于它们的
`pending` 行，`background_tasks` 里也没有运行中的任务——它们**永远不会再被唤醒**。用户看到的是
「等唤醒：PR #xxx 合并」然后会话再无下文。

根因不是某个入口写错状态，而是**没有任何运行中的东西在执行「`waiting_wake` 必须有唤醒源」
这条不变量**：推导/修复函数只挂在启动路径上，主循环从不周期对账；而唤醒源口径
（`wake_source_cover`）只认「运行中的后台任务 + pending 定时唤醒」，把**已登记、有界心跳的
PR/CI 观测**排除在外，于是刚开完 PR 的正常等待在推导层看就是「无源等待」。

## 改动

- **R2 观测即唤醒源**：`background_tasks::WakeSourceCover` 增加 `external_observations`
  （`thin_external_observations.completed_reason IS NULL`），与后台任务、pending 唤醒一并计入。
- **R1 派生即判定（fail-closed）**：`thin_derive::repair_waiting_wake_source` 成为唯一落地点。
  无源等待不再原地停留：
  - 没有等待对象 → `ended`；
  - 外部事实已完成（PR 已合并）→ 幂等挂出收尾唤醒；
  - 系统自己能推进（PR/CI/后台任务/定时唤醒）→ 补真实来源（系统自动重试/自愈），
    审计 `thin_wait_guard`，自愈上限 `MAX_SELF_CONTINUE = 2`；
  - 要人决定或自愈用尽 → `waiting_user` + 人话提问（不含「唤醒源 / 外部事项 / waiting_*」）。
- **R3 周期对账**：新增 `thin_derive::reconcile_waiting_wake_sources`（返回
  `WakeReconcileReport`），启动时跑一次、之后每 180 秒跑一次（`background_tasks::start_runtime`）。
  启动对账改为复用同一函数，重启不会让存量脏状态永久存活。
- **R4 界面人话**：`waiting_wake` 标签改为 `等唤醒：<在等什么>（<预计何时回来>）`，缺失字段有
  人话兜底，不再输出空原因。

## 验证

- `cargo test --manifest-path src-tauri/Cargo.toml m197` → **6 passed**：
  - `m197_wake_without_source_is_never_left_waiting`（R1/R5）
  - `m197_self_continue_is_bounded_then_asks_user`（R1/R5，自愈有界 + 人话提问无内部词）
  - `m197_merged_pr_gets_closeout_wake`（R2）
  - `m197_active_observation_counts_as_wake_source`（R2）
  - `m197_stock_dirty_states_repaired_and_idempotent`（R3/R5，存量脏状态一次修完且幂等）
  - `m197_waiting_label_names_the_wait_and_the_wake_time`（R4）
- `cargo test --manifest-path src-tauri/Cargo.toml --lib`：全量回归（见 CI）。
- `m183_startup_repairs_waiting_wake_without_a_source` 的 fixture 按 M197/R1 修订为「系统能推进」
  的等待，因为「要人决定的等待自动补来源」正是本次修复掉的 bug 行为。
- 规格：`docs/specs/feature-specs/m197-waiting-wake-without-waker.md`（R1–R5）。

## 限制

- 真实主路径验收（打开应用，看存量会话在一个 180s 对账周期内离开「无源等待」）由用户在真机执行；
  本 PR 的证据是合成 fixture 单测 + 治理门禁 + CI，不做 live 声明。

Scenario-Test: HLT-004, RTE-005
