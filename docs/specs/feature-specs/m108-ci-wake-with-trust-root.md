# M108：M104 的漏洞——PR 同时有信任根失败和普通失败时，CI 失败不会唤醒会话

状态：implemented（本单）

## 现象（真机 v1.88.19，已含 M104 #709）

会话 `cd8d910e`，PR #713（改了 `.github/workflows/ci.yml` 与 `scenario-registry.json`，
属信任根）。CI 结束：`check-rust` 失败（`lifecycle_smoke.rs:141:46` Windows panic，
1617 过 1 失败）+ `scenario-gate-pr` 失败（`cannot self-modify`，预期）。

02:40:14 `objective_events` 记录 `canonical_pr_progress`（head `5db85c30`，
`reached_ceiling=pr_open`），objective 保持 `waiting_system / delivered_awaiting_ci`；
之后 13 分钟无任何唤醒、无新消息。编排方 02:53 手工转述失败后会话才继续。

## R1 为什么没有唤醒（代码证据）

复现这条事件序列时发现**两个叠加的缺陷**，任一个都足以让会话沉默：

1. **决策入口把「观望」写回了死寂**。`reconcile_parked_delivery_progress`
   （`src-tauri/src/agent/execution_workspace.rs`）对「PR 仍 open」的分支把
   objective 重写成 `delivery_awaiting_ci`，并同时清空
   `next_observation_at`、`attention_request_json`。目标观测调度器只处理到期的
   objective，因此这条 canonical 观察**只记录进度**（事件 `canonical_pr_progress`）
   **不产生决策**，下一个唤醒入口被自己删掉了。M104 修好的是交付工具内的 park
   （`park_delivered_ci_wait`），这条旁路没有走它。
2. **一条 check 的失败会覆盖另一条**。`classify_ci_observation`
   （`src-tauri/src/git_remote/github.rs`）按名称排序后遇到第一个失败就
   `return`，所以 #713 的 head 只会被报成 `check-rust` 或
   `scenario-gate-pr` 中的一个。若被报出的是信任根那条，调用方
   （`run_delivery` 的 `d.contains("cannot self-modify")` 判断）就把整轮 CI
   归入「等编排方放行」，普通失败随之消失。M104 的
   `classify_ci_checks` / `ci_outcome_wake_prompt` 虽然写好了，但只被测试调用，
   没有任何生产路径使用。

结论：数据（`canonical_pr_progress` 后 13 分钟静默）不是「观察者坏了」，而是
「观察者把唤醒排期清空，且拿到的失败集合不完整」。

## 决策

1. **一次观察必须落成一个决策**：canonical 观察在 PR 仍 open 时调用
   `classify_ci_checks`，把结果写进 objective 的 `failure_code` / 注意力文案 /
   `next_observation_at`，不再写死 `delivered_awaiting_ci` 并清空调度。
2. **按「排除信任根自修改失败后的其余失败」判断**：
   - 还有任何普通失败 → `delivered_ci_failed`，文案带失败 job 与失败测试名；
   - 只有信任根失败 → `delivered_ci_trust_root_only`，明确「等编排方放行」，
     不算任务失败、不消耗恢复预算；
   - 全绿 → `delivered_ci_green`，文案指向 `merge-queue: armed`；
   - 仍在运行 → `delivered_ci_still_running`，禁止把阻断归因定论。
3. **一份失败集合**：`classify_ci_observation` 汇总**全部**失败 check，携带
   `cannot self-modify` 标记与失败测试名；新增 `ci_check_states` 提供同一份
   逐 check 证据（`classify_ci_checks` 的输入），并让 `GhCliRemote` 通过
   `observed_ci_checks` 只读暴露它。
4. **判定按 check 而非按字符串包含**：`run_delivery` 不再用
   `d.contains(marker)` 定性，而是把状态行还原成逐 check 证据后交给
   `classify_ci_checks`；信任根分支只在 `TrustRootOnly` 时进入。
5. **保持只读**：canonical 观察补齐的是决策与唤醒排期，不新增 mutation 权限；
   armed / 合并仍由交付工具在 ceiling 允许时执行。

## Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| R1 | 读 #709 实现，用真实事件序列复现，写清为何没有唤醒 | 本文 R1 两条代码证据（`reconcile_parked_delivery_progress` 清空调度 + `classify_ci_observation` 首个失败即返回；M104 分类函数无生产调用者） |
| R2 | 决策按「排除信任根自修改失败后的其余失败」判断：普通失败 → 唤醒并附 job/测试名；仅信任根 → 等放行；全绿 → armed | `m108_ci_shape_tests`（#713 形状）、`m108_reconcile_decision_tests`、`m108_a_finished_ci_decision_wakes_the_parked_delivery` |
| R3 | 先失败测试：信任根失败 + 普通失败 → 唤醒；仅信任根 → 等放行；全绿 → armed，用 #713 形状做 fixture | 三个 `m108_ci_shape_tests` 用例；`cargo test m108` 首次编译失败（函数不存在），实现后 7 passed |
| R4 | 若 `canonical_pr_progress` 只是观察记录没有决策入口，补上决策触发；不依赖下一次用户消息 | `reconcile_parked_delivery_progress` 写入 `failure_code` + `attention_request_json` + `next_observation_at`；`m108_a_finished_ci_decision_wakes_the_parked_delivery` 断言三者非空 |

## Applicable Harnesses

- **Spec Harness**：本文件。
- **Compatibility Harness**：不放宽门禁；`CiObservation`、`CiStatus` 语义不变
  （单条失败格式保持兼容，多条失败用 `；` 连接）；canonical 观察仍只读。
- **AI Collaboration Harness**：单执行流完成（见 PR 记录）；未启动子代理，
  因为改动集中在同一决策链，且需要连续编译反馈。
- 不触发 Release / Observation / Payload / Viewport（无发布路径、无 UI、
  无文件负载变更）。

## 测试矩阵

| 场景 | 测试 | 断言 |
| --- | --- | --- |
| #713 形状：check-rust 失败 + scenario-gate-pr 信任根失败 | `a_trust_root_refusal_must_not_hide_the_red_check_beside_it` | 状态行同时含两个 job 与失败测试名；`CiOutcome::Failed` 含两条证据；文案不含「等编排方放行」 |
| 同一形状经状态行还原（交付工具路径） | `a_red_head_with_a_trust_root_refusal_still_reports_the_ordinary_failure` | `ci_checks_from_status` 解析出 2 条失败；文案含 `check-rust` 与测试名 |
| 仅信任根失败 | `a_trust_root_refusal_alone_still_waits_for_the_orchestrator`、`a_status_line_with_only_the_trust_root_refusal_is_not_a_task_failure` | `TrustRootOnly`、不可修复、「等编排方放行」「不是任务失败」 |
| 全绿 | `a_green_head_wakes_the_session_to_the_arm_step` | `AllGreen`；文案含 `merge-queue: armed` |
| 未结束不给定论 | `an_unfinished_head_never_becomes_a_verdict` | `pending`/`success` 不产生失败证据；`in_progress` 归为 `StillRunning` |
| parked 交付的决策入口（三种形状各一遍） | `m108_a_finished_ci_decision_wakes_the_parked_delivery` | `failure_code` 分别为 failed / trust_root_only / green；`next_observation_at` 非空；文案含对应关键字与失败测试名 |

## Implementation Notes

- `CanonicalPrObserver::observe_ci_checks` 默认返回 `None`（离线/脚本观察者行为不变），
  `RemoteCanonicalPrObserver` 通过 `GhCliRemote::observed_ci_checks` 只读实现。
- `next_observation_at` 使用 `bounded_next_observation_at(now, now + 60s)`，
  与 M104 的 `DELIVERED_CI_REOBSERVE_MS` 同一上界，避免出现无界排期。
- `attention_request_json` 按 `UserAttentionRequest` 的字段形状写入，
  `resume_cursor` 为空：这是状态说明，不是需要用户输入的问题。
- 只对 `CanonicalPrState::Open` 做 CI 决策；`Queued` 已在合并队列，`Merged`
  走既有结算路径。

## 遗留

- 真机验收由编排方发版安装后完成：复现一次「信任根失败 + 普通失败」的 PR，
  确认 objective 在 CI 结束后 60 秒内出现 `delivered_ci_failed` 与带失败测试名的
  唤醒消息，且不再需要人工转述。
