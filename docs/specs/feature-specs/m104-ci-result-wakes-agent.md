# M104：CI 出结果后必须叫醒会话

状态：implemented（本单）

## 背景

真机 v1.88.16 → v1.88.17。objective `59147b73` 在 23:39:21 进入
`waiting_system / delivered_awaiting_ci`，交付记录绑定 PR #704 head `cc4c166e`。该
head 的 CI 早已结束（`check-rust` 因会话自己新加的 Windows 测试 panic 而失败，
`scenario-gate-pr` 因信任根自修改而失败，属预期）。此后 45 分钟没有任何事件，装
v1.88.17 重启后又 15 分钟无动作，最终由编排方两次手工转述失败信息才继续。

同族缺陷：M77（ci_green 不打 armed）、M97（PR 全绿却停住）、M104（本条）。三者都是
「CI 状态变化没有驱动会话下一步」。

## R1 时间线（数据）

来源：真机会话记录、`delivery_runs` / `delivery_run_events`、PR #704 的 check-run 时间。

| 时间（本地） | 事件 |
| --- | --- |
| 23:39:21 | objective `59147b73` 交付完成，进入 `waiting_system / delivered_awaiting_ci`；`next_observation_at` 被写成 `NULL`。 |
| 23:39 之后 | PR #704 head `cc4c166e` 的 check-run 陆续结束：`check-rust` = failure（Windows 上新增测试 panic），`scenario-gate-pr` = failure（`cannot self-modify`，预期）。 |
| 23:39 – 00:24 | 没有观察者：`next_observation_at = NULL` 意味着目标观测调度器永远不会把它当成到期任务；交付运行只有在**会话自己再调用一次** `deliver_changes` 时才会重读 check-run。会话已经结束回合，所以两个条件都不成立。 |
| 00:24 | 编排方把 `check-rust` 的失败手工转述进会话，会话才开始修复。 |

结论：CI 结束的事实一直存在，缺的是「谁去读」和「读完叫醒谁」。观察者本身没有坏，
坏的是 `delivered_awaiting_ci` 这个 park 把唯一的唤醒入口（`next_observation_at`）
清空了。

## 决策

1. **交付等待必须自带调度**：`delivered_awaiting_ci` 的 park 不再把
   `next_observation_at` 清成 `NULL`，而是 60 秒后到期，并被
   `MAX_WAIT_BEFORE_NEXT_ATTEMPT_MS` 钳住。等待仍是「非失败、无 remediation、不消耗
   技术恢复预算」，只是它会自己醒过来。
2. **一次观察必须有结论**：新增 `classify_ci_checks`，把一次 check-run 快照归为
   `StillRunning` / `Stalled` / `AllGreen` / `TrustRootOnly` / `Failed`。
3. **三种结局三种处理**：全绿 → 按 ceiling 收尾（armed / 合并，M97 路径）；只有信任根
   自修改失败 → 明确的「等编排方放行」文案，不算失败、不进恢复预算；其他失败 → 唤醒
   会话，输入里带失败 job 名与失败测试名/首条错误摘要，修完再推。
4. **未结束不许定论**：`StillRunning` 的唤醒文案显式禁止把阻断归因定论。
5. **信任根识别靠原文**：`classify_ci_observation` 在失败 check 的 `output.title /
   summary / text` 里看到 `cannot self-modify` 时，把该标记带进失败 detail，交付结局
   与目标 park 都能据此分流，不再让会话去改它不能改的门禁。
6. **有界等待**：CI 超过 60 分钟仍无结论 → `Stalled { minutes }`，重新观察并明确说明
   「仍然没有结果」，不写成失败。

## Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| R1 | 用数据写清 59147b73 的时间线：谁在观察、为什么没有唤醒 | 本文 R1 表格（来自会话记录、`delivery_run_events`、PR #704 check-run） |
| R2 | 等 CI 的会话在检查结束后必须产生一次决策；三种结局三种处理 | `m104_ci_wake_tests` 四条：`all_green_checks_wake_the_session_to_the_merge_path`、`trust_root_only_failure_waits_for_the_orchestrator_without_failing_the_task`、`ordinary_failure_wakes_the_session_with_the_failing_job_and_test_name`、`a_pending_check_past_the_bound_is_reported_as_stalled_not_as_a_verdict` |
| R3 | CI 未全部结束时不得把阻断归因定论；交付状态区分「仍在运行 / 已失败（列出）/ 只差信任根」 | `CiOutcome::{StillRunning,Stalled,Failed,TrustRootOnly,AllGreen}` + `ci_outcome_wake_prompt`；`GhCliRemote::ci_status` 的失败 detail 携带 `cannot self-modify` 标记；`run_delivery` 对信任根失败走 `trust_root_gate` 文案 |
| R4 | 等待有界：>60 分钟仍 pending 要重新观察并给出明确状态 | `DELIVERED_CI_STALLED_AFTER_MS` + `CiOutcome::Stalled`，测试断言 61 分钟文案 |
| R5 | 合成 forge 状态覆盖三种结局，普通失败断言唤醒且输入含失败测试名 | `src-tauri/src/agent/objective.rs` 的 `m104_ci_wake_tests`（4 tests，合成 check-run 状态） |
| R6 | 能挂进 E2E-004 就挂上，状态如实 | 见下「R6 状态」 |

## Applicable Harnesses

- **Spec Harness**：本文件（Requirements Traceability + 测试矩阵）。
- **Compatibility Harness**：不改交付 ceiling、不改门禁、不改 `delivery_runs` 语义；
  `is_parked_delivery_wait` 放宽为「允许带调度」，仍禁止 remediation 与用户提问。
- 不触发 Release / Observation / Payload / Viewport（无发布路径、无 UI、无文件负载变更）。

## 测试矩阵

| 场景 | 测试 | 断言 |
| --- | --- | --- |
| 全绿 | `all_green_checks_wake_the_session_to_the_merge_path` | `AllGreen`、`failure_code = delivered_ci_green`、唤醒文案含 `merge-queue: armed` |
| 只差信任根 | `trust_root_only_failure_waits_for_the_orchestrator_without_failing_the_task` | `TrustRootOnly`、不可修复、文案含「等编排方放行」「不是任务失败」、park 不产生 remediation |
| 普通失败 | `ordinary_failure_wakes_the_session_with_the_failing_job_and_test_name` | `Failed`、唤醒文案含 `check-rust` 与失败测试名；park 的 `next_observation_at = now + 60s`（即唤醒已排期） |
| 仍在运行 / 停滞 | `a_pending_check_past_the_bound_is_reported_as_stalled_not_as_a_verdict` | 60s → `StillRunning`；60min → `Stalled{60}`；61min → 文案含「61 分钟」 |

## R6 状态

E2E-004（fake forge：CI 失败 → 修复 → 成功）的自动化在本次未接入
`docs/testing/scenario-registry.json`：本单的实现只到「一次观察 → 一个结论 + 唤醒」，
fake forge 侧的 `ci_failure → fix → success` 复合场景仍由 M89/E2E-004 既有用例覆盖，
本单不声明它已自动化，避免把未接线的场景标成 `implemented`。作为后续项提出。

## Implementation Notes

- 唤醒的真实落点是 `objectives.next_observation_at`：目标观测调度器只处理到期的
  objective，因此 park 保留一个 60 秒后的到期时间就是「CI 结果会叫醒会话」。
- `is_parked_delivery_wait` 不再要求 `next_observation_at.is_none()`：交付等待在任何
  情况下都不排队 remediation、不向用户提问，有没有调度与之无关；而「没有调度」正是
  M104 的成因。
- 信任根识别只认 `cannot self-modify` 原文，不按 job 名推断，避免把真正的门禁回归
  误判成「等编排方放行」。
