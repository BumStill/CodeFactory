# M149：交付后的等待必须能用工具表达——唤醒参数容错、`--watch` 拒绝给可用替代、模型知道系统已在监视 PR

## 现场（v1.91.18，2026-10-22）
- `schedule_wakeup` 全天 `done 2`、`error 1`：模型按惯例一次给了 `after_sec` 和 `every_sec`（其中一个为
  `0`，表示「不用这个字段」），工具直接报 `after_sec 与 every_sec 只能二选一`，本回合没有任何唤醒源。
- 同一条会话里 `gh pr checks <n> --watch` 被 `shell_policy` 拒绝，拒绝文案只说「不允许持续轮询」，
  没有给出可复制执行的替代命令；模型随后没有建立任何监视，直接结束回合。
- `deliver_changes`（thin）返回 `next_action: "请用后台任务监视 CI；由合并队列合并，不要直接合并。"`
  ——与 M142 已落地的「系统对已交付 PR 低频心跳监视」相反，模型于是又去自己造监视源。

## 根因
1. 参数语义按 JSON Schema 的「字段出现即赋值」实现：`every_sec: 0` 被当成「确实传了 every_sec」，
   与 `after_sec: 180` 构成冲突，而模型的本意是「只要一次性唤醒」。
2. 拒绝文案只陈述禁令，不提供可直接调用的形态；模型在「被拒绝 + 无替代」的路径上没有下一步。
3. 交付结果文案仍描述 M142 之前的职责划分（模型自己监视），与运行时事实矛盾。

## 规则
- **R1 零值等于没传**：`after_sec` / `every_sec` 取 `0`、`null`、缺失一律视为未提供；只有当两者都是
  `≥60` 的有效值时才报「二选一」。错误文案必须给出可直接复制的调用示例，并说明「不想用的那个字段
  省略或传 0/null」。
- **R2 拒绝必须带可用替代**：`--watch` / 短间隔 `gh` 轮询的拒绝文案必须包含一条可直接执行的替代
  （`background_task_start` 里跑 `sleep 180` 起的轮询循环，或改用 `deliver_changes` 的授权 ceiling），
  并说明「已交付 PR 由系统监视，不必自己轮询」。
- **R3 交付结果说明系统在监视**：thin `deliver_changes` 的 `waiting` 结果必须包含
  「系统已在监视 PR #N（心跳间隔 ≥180 秒），CI 失败/冲突/被队列退回/合并时会自动唤醒本会话，
  你可以结束本回合等待唤醒」，`next_action` 不得再要求模型自建监视；该会话投影为 `waiting_wake`。
- **R4 先红后绿**：新增测试先失败，再实现。

## 证据（实测）
- 先红：`node scripts/cargo-shared.mjs test --manifest-path src-tauri/Cargo.toml --lib m149`
  → 4 failed（`after_sec 与 every_sec 只能二选一`；`--watch` 文案缺替代；`next_action` 仍是
  「请用后台任务监视 CI」）。
- 后绿：同一条命令 → 4 passed；`--lib -- tools::delivery_thin tools::wakeup tools::shell_policy
  agent::background_tasks` → 59 passed。
- 用例：`agent::background_tasks::tests::m149_zero_valued_wakeup_field_counts_as_not_provided`、
  `tools::wakeup::tests::m149_zero_every_sec_schedules_a_single_one_shot_wakeup`、
  `tools::shell_policy::tests::m149_watch_denial_carries_a_copy_pasteable_alternative`、
  `tools::delivery_thin::tests::m149_waiting_result_says_the_system_already_watches_the_pr`。
- 场景登记：`CXD-002`（thin 交付）、`RTE-005`（后台任务/唤醒）；
  `validate_scenario_test_governance.py --ci` 与 `validate_readme_contract.py` 均 OK。
- 全量回归：`CARGO_TARGET_DIR=.codefactory-cache/cargo-target-m149 node scripts/cargo-shared.mjs test
  --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast`。
