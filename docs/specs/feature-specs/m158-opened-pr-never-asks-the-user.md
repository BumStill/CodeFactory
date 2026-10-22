# M158：开完 PR 后不得停在「请选择继续监视或结束任务」

## 背景与用户决策

交付会话开出 PR 后，回合结束时用户看到的是一句内部决策：

> 外部事项尚无法确认完成，且没有活着的唤醒源。请选择继续监视或结束任务。

它同时违反两件事：系统本可以自己盯住 PR（本来就有 PR 心跳），却把「要不要监视」
这个技术动作交给用户决定；文案里还出现「唤醒源 / 外部事项」这类用户看不懂的内部词。

## 证据（2026-10-22）

- 会话 `816bc573`（M155，#842）18:01、`9161c74a`（M157，#845）22:38：assistant 自述
  「PR 已有系统级监视（心跳 ≥180 秒），不需要你回复」，但 `thin_session_state=waiting_user`，
  提示即上面那句——自述与状态机自相矛盾。
- 根因：`thin_derive::derive_state` 在 `waiting_wake` 之后没有可用的交付引用/唤醒源时，
  无条件改写成 `waiting_user` + 内部用词；`thin_state::sweep` 到期同样把外部等待抛给用户。

## 需求（Requirements Traceability）

| Req ID | 需求 | 最低证据 |
|---|---|---|
| R1 | 授权交付会话开出 PR 后，系统自动登记 PR 监视（CI 结论/合并/队列退回/关闭即唤醒），状态派生为 `waiting_wake`，带「等 PR #N 的 CI/合并」人话原因，不进入 `waiting_user` | `agent::thin_derive::tests::m158_opened_pr_registers_system_monitor_instead_of_asking_the_user`：断言状态、`thin_delivery_refs` 与 `thin_external_observations` 各有一行、用户可见字段不含内部词 |
| R2 | 若确实没有任何可登记的唤醒源，自动登记默认 PR 心跳（≥180 秒，遵守 GitHub 配额策略），而不是问用户 | 同上（写入观测行 `observed_at=0`，由既有 `heartbeat_external` 以 ≥180 秒 CAS 接管）；`agent::thin_derive::tests::m158_external_wait_stays_a_system_wait_with_or_without_a_live_monitor` 覆盖「有监视 / 监视到期」两种情形 |
| R3 | `waiting_user` 只用于真正需要用户决定/动作的事项；文案不得出现「唤醒源/外部事项」这类内部词 | `m158_user_wait_question_uses_plain_wording`、`m158_expired_external_wait_is_never_a_user_question`、`thin_state_round_trips_and_expired_wake_becomes_a_user_question`（断言问题含用户等待对象且不含「唤醒源」） |
| R4 | 测试先失败：合成 fixture 复现「开 PR 后回合结束」→ 断言 `waiting_wake` + 已登记监视；PR 合并后被唤醒做收尾 | 上述用例先在旧代码上失败（`left: WaitingUser, right: WaitingWake`、`left: Ended`），改动后全绿；合并收尾沿用 `m142_*` 观测→收尾链路 |

## 适用 Harness

- Spec Harness：本文件 + PR body 的 Req↔测试映射。
- QA / Acceptance Harness：Rust 集成测试（合成 fixture，无真实网络）；真实安装版验收由编排方发版后完成。
- Compatibility Harness：`thin_state` 四状态与既有 `session_state` 字段不变，仅派生与文案调整；旧行（历史 `waiting_user` + 内部词）被重新派生，不需要迁移脚本。
- Observation Harness：PR 监视登记为 `thin_delivery_refs` + `thin_external_observations`，仍由 `objective_supervisor` 心跳以 ≥180 秒间隔观测（遵守 GitHub API 预算）。
- AI Collaboration Harness：context scope 限定在 `agent/thin*.rs`；假设「会话事实里出现 `/pull/<n>` 或「PR #<n>」即为该 PR」；review point 是「不得把内部状态词推给用户」；validation 由下列测试给出。
- Viewport Harness：不适用（无 UI 结构变更，仅状态文案）。

## 测试矩阵

| 场景 | 用例 |
|---|---|
| 开 PR 后回合结束，无交付引用 | `m158_opened_pr_registers_system_monitor_instead_of_asking_the_user` |
| 历史库里已写死内部问题 | `m158_legacy_limbo_row_is_rederived_into_a_system_monitor` |
| 外部等待有/无活着的监视源 | `m158_external_wait_stays_a_system_wait_with_or_without_a_live_monitor` |
| 外部等待到期（sweep） | `m158_expired_external_wait_is_never_a_user_question` |
| 真正的用户侧等待 | `m158_user_wait_question_uses_plain_wording`、`thin_state_round_trips_and_expired_wake_becomes_a_user_question` |

## Implementation Notes

- `thin_derive::ensure_pr_monitor`：先认 `thin_delivery_refs`；没有则从最近会话事实里认领
  `/pull/<n>` 或「PR #<n>」，登记默认交付引用（`ceiling: ThroughMerge`）并写观测行，
  由既有心跳接管——不新增轮询循环，也不突破 180 秒/配额策略。
- `thin::is_system_followed`：外部对象之外，把「后台任务 / 定时唤醒」这类系统占位等待
  也归为系统自己盯着的等待，避免把它们写成给用户的问题。
