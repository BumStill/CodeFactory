# M186：PR 已合并的 `waiting_wake` 会话不得被「取消唤醒」后无人结算

## 需求（任务原文）

- **R1 已合并 + 工作区已缺失**：直接结算 `ended`，写明原因，清理残留工作区登记，不再调模型。
- **R2 已合并 + 工作区尚在**：由系统代做任务模板 §8 的收尾（删除本地工作区、本地分支、已合并远端分支），然后结算 `ended`；收尾失败 → `waiting_user` 并给出原因。
- **R3 取消唤醒必须与结算原子化**：任何「取消唤醒」都不得与 `waiting_wake` 同存；启动自检 + 周期检查兜住。
- **R4 测试先失败**：跨进程构造上述两种状态 → 启动后 ≤1 分钟 `ended`；不变量测试覆盖「`waiting_wake` 无唤醒源」。
- **R5 真机验收**：编排方在发版安装后核对 14 个存量会话是否已 `ended`。

## 证据（v1.91.47，含 M183/M184；10-27 04:32 编排方查库）

- 过期 pending 唤醒已清零（M183 生效）。
- 仍有一批会话：PR 已合并、`thin_session_state.state='waiting_wake'`，
  其 `thin-source-repair-<session>` 唤醒 `status='cancelled'`、`fire_count=0`，
  之后没有新消息、没有新唤醒源 —— 会话既不在跑，也不会再醒。

## 根因

1. `execution_workspace::settle_missing_terminal_workspace` 只在**投递唤醒**时被调用
   （`commands::chat::send_system_wake`）。唤醒一旦先被取消（3 次投递失败、
   拆分工作区恢复连续失败或重启回收），结算入口就再也不会被执行到。
2. 取消与状态写入是两次独立写：`cancel_session_wakeups` 与 `thin_state::save`/
   `ask_user` 分属不同语句，中间崩溃或路径不同步就留下
   「`cancelled` + `waiting_wake`」这种既不会醒也无人负责的组合。
3. M184 的 R1 只覆盖「工作区已消失」；PR 已合并但工作区仍在时，没有任何
   系统代做的 §8 收尾，会话只能等模型回合——而唤醒已经没有了。

## 实现

- 新逻辑放在已登记场景文件 `src-tauri/src/agent/thin_derive.rs` 的
  `merged_settlement` 子模块（新增独立模块需要改场景登记表 = 信任根，普通 PR 不得
  自我修改，故不另开文件）：
  - `settle_merged_sessions(pool)`：扫描 `waiting_wake` 且存在完成事实
    （`thin_external_observations.completed_reason` 含「已合并」/「已关闭但未合并」）
    的会话，按 R1/R2 结算；有更新用户消息、执行器仍在跑的一律跳过。
  - `safety_tick(pool)`：60 秒预算闸门下的同一轮扫描，供 supervisor 周期调用。
  - `waiting_wake_without_wake_source(pool)`：不变量探针。
- R2 的 §8 收尾是确定性的本地动作（不调模型）：脏工作区拒绝删除并
  `waiting_user` 给原因；干净则 `git worktree remove --force` → `branch -D` →
  `push origin --delete`（仅合并后；远端状态未知记为备注，不网络阻塞结算）。
- `thin_state::settle_ended_and_cancel` / `ask_user_and_cancel`：状态写入与
  「取消该会话全部 pending 唤醒」在**同一事务**内完成（R3）。
- 接线：`objective_supervisor` 启动时结算一次（≤1 分钟满足 R4），
  5 秒轮询里由 60 秒预算闸门限制频率地兜底。

## 不变量

- 任何 `waiting_wake` 行必须有真实唤醒源（pending 唤醒 / 运行中后台任务），
  否则必须被结算成 `ended`，或由 M183 的自愈登记重新拿到唤醒源。
- 「唤醒 `cancelled` 且状态仍 `waiting_wake`」在结算后不得存在。

## 测试

- 跨进程 fixture：子进程（同一测试二进制的重入）写一个真实 SQLite 库 + 真实
  git 工作区，父进程只做「重启后启动扫描」，断言 ≤ 首个 tick 内结算。
- 覆盖：R1 工作区缺失、R2 工作区存在（干净/脏）、R3 原子性、不变量为空。
- 覆盖场景：`HLT-004`（thin 会话重启/结算）、`RTE-004`（工作区生命周期）、
  `RTE-005`（唤醒与后台任务）——沿用既有登记，不改信任根。

验证记录：

- `cargo test merged_settlement::` → 5 passed（含跨进程重启用例）。
- CI 修复：`production_background_commands_use_no_window` 先复现两个后台 Git 命令缺少保护的失败；补充 `NoWindow` 后，`cargo test --test no_window_regression` → 2 passed。
- `cargo test --lib`（全量）→ 2042 passed / 0 failed / 12 ignored。
- 相关子系统（`execution_workspace::`、`thin_state::`、`thin_derive::`、
  `background_tasks::`、`objective_supervisor::`）→ 174 passed。
- `python3 tools/governance/validate_repo_governance_baseline.py` → pass。
- `python3 tools/governance/run_scenario_harness_gate.py --stage local/pull_request` → OK。


## 真机验收（发版后由编排方完成）

1. 安装新版启动，等待 ≤1 分钟。
2. 查 `thin_session_state`：上述 14 个会话应为 `ended`，`reason` 含「已合并」。
3. 查 `wakeup_schedules`：这些会话不再有 `pending` 唤醒。
4. 任选一个会话打开：不再显示「等待中」，只有最终结论与收尾说明。
