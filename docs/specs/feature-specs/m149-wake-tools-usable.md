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
- **R2 模型必须看到可用替代**：`--watch` / 短间隔 `gh` 轮询被拒绝后，模型必须能立刻拿到一条可执行的形态
  ——在 `background_task_start` 里跑 `sleep 180` 起的轮询循环——并知道「已交付 PR 由系统监视，不必自己轮询」。
  该说明放在 `background_task_start` 的工具描述（模型必然读到、且属映射文件 `tools/background_task.rs`），
  而不是 `shell_policy` 的拒绝文案：`docs/testing/scenario-registry.json` 是受信 gate root，普通 PR 不得
  自改，`tools/shell_policy.rs` 目前没有场景映射，写在那里会同时触发「trust root 自改」和「覆盖缺口」。
- **R3 交付结果说明系统在监视**：thin `deliver_changes` 的 `waiting` 结果必须包含
  「系统已在监视 PR #N（心跳间隔 ≥180 秒），CI 失败/冲突/被队列退回/合并时会自动唤醒本会话，
  你可以结束本回合等待唤醒」，`next_action` 不得再要求模型自建监视；该会话投影为 `waiting_wake`。
- **R4 先红后绿**：新增测试先失败，再实现。

## 落地位置（受信 gate root 约束后的最终形态）
- `src-tauri/src/agent/background_tasks.rs`（RTE-005）：R1 的零值归一与两条错误文案。
- `src-tauri/src/tools/background_task.rs`（RTE-005）：R2 的可用替代写进 `background_task_start` 工具描述。
- `src-tauri/src/tools/delivery_thin.rs`（CXD-002）：R3 的 `waiting` 结果与 `session_state`。
- 不再改动 `src-tauri/src/tools/wakeup.rs`、`src-tauri/src/tools/shell_policy.rs`：二者无场景映射，
  且 `docs/testing/scenario-registry.json` 属于受信 gate root（普通 PR 自改会被
  `validate_trust_root_immutability` 拒绝）。为这两个文件补映射必须先走外部 governance bootstrap。

## 证据（实测）
- 先红 → 后绿（第一版实现，含未映射文件）：`--lib m149` 4 failed → 4 passed。
- 第一版 PR #829 CI 红：`scenario-gate-pr` 报
  `trusted scenario gate root cannot self-modify in a normal PR: docs/testing/scenario-registry.json`；
  回退 registry 后报 `scenario registry coverage gap for product files:
  src-tauri/src/tools/shell_policy.rs, src-tauri/src/tools/wakeup.rs` —— 这两条是重排落地位置的依据。
- 最终形态：`node scripts/cargo-shared.mjs test --manifest-path src-tauri/Cargo.toml --lib m149`
  → 3 passed；`--lib -- tools::background_task tools::delivery_thin agent::background_tasks` → 47 passed；
  `pnpm test:background-tasks:headless`（RTE-005 的 `automated_by`）→ `background tasks headless
  acceptance: OK`。
- 用例：`agent::background_tasks::tests::m149_zero_valued_wakeup_field_counts_as_not_provided`、
  `tools::background_task::tests::m149_start_definition_teaches_the_bounded_watch_shape`、
  `tools::delivery_thin::tests::m149_waiting_result_says_the_system_already_watches_the_pr`。
- 场景登记：`CXD-002`（thin 交付）、`RTE-005`（后台任务/唤醒），PR body 声明 `Scenario-Test: CXD-002, RTE-005`；
  `run_scenario_harness_gate.py --stage pull_request` 与 `validate_readme_contract.py` 均 OK。
