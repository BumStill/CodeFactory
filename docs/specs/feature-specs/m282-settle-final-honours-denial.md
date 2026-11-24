# M282：回合结束结算必须复用「审批被拒且未交付 → 系统接续」判定

## 现场（v1.121.0，2026-11-13 00:06）

会话 `ebe54c65`（M281）：

- 00:03:57 提出 `python3 - <<heredoc` 审批，编排方 00:06:20 拒绝并备注了 `edit_file` 替代做法。
- 00:06:28 模型写下「阻断总结……需要你做一件事：手工补三行」后结束回合。
- `thin_session_state` 被写成 `ended / 任务已结束`，触发器 `thin_ended_cancel_wakes` 取消全部 pending 唤醒；没有任何 `thin-continue-*` 新唤醒。
- `permission_intents` 里本会话有 00:03:57 的 `denied` 行，任务未交付（无 PR）。

## 根因

M278（a4f5ffc12）只在 `src-tauri/src/agent/thin_derive.rs` 的 `refresh` 里加了「被拒且未交付 → `continue_state_now`」分支，测试也只覆盖 `refresh`。

回合结束的真实写入路径是 `src-tauri/src/agent/thin_state.rs` 的 `settle_final`：没有未受理提问时直接 `after_final(WaitNeed::None)` → `ended("任务已结束")` 并 `save`，完全不看 `permission_intents`。同一事实（被拒、未交付）在两条路径上有两种结论，单测在 `refresh` 上绿，真机走 `settle_final`。

## Requirements Traceability

| Req | 要求 | 落地 | 证据 |
|---|---|---|---|
| R1 | 回合结束结算与 `refresh` 用同一个判定函数，不得复制逻辑 | `thin_derive::denial_continuation`；`settle_final` 在写出 `ended` 前调用，`refresh` 改为调用同一函数 | `thin_state.rs` / `thin_derive.rs` 差异；`m282_settle_final_continues_after_denied_approval` |
| R2 | 只接续一次，不形成无界循环 | 沿用 M278 的唤醒 id `thin-continue-<session>-<denied_at>`；已兑现唤醒不再重排 | `m278_denied_approval_continues_only_once`（回归绿） |
| R3 | 走真实入口的测试 | 测试直接调用 `settle_final`，断言 `waiting_wake`、存在 `thin-continue-*` 且 `next_at<=now`、状态非 `ended`、唤醒仍 `pending` | 该测试在未改实现时失败（`Ended / 任务已结束`），改后通过 |
| R4 | 反例不被覆盖 | 无 denied 行、用户停止（`chat_session_cancel_intents`）、已交付（PR 合并）三种情况都不接续 | 三个 `m282_settle_final_*` 反例测试 |
| R5 | 不越界 | 只读 `permission_intents` 结构化事实，不按助手措辞判断；不动权限与 heredoc 规则 | 判定函数只查询 DB 行 |

## Primary User Path

用户派单 → 模型触发需要审批的命令 → 用户拒绝并给出替代做法 → 回合结束结算时系统立即接续（新的 assistant 消息），会话不停在「需要你做一件事」。

## Applicable Harnesses

- Spec Harness（本文件，R1–R5）
- Compatibility Harness：`thin_session_state` 状态、`wakeup_schedules` id 形状不变，`refresh` 行为等价
- Observation Harness：接续唤醒可见（`wake_source` 含 `thin-continue-`），日志与状态一致

## 测试矩阵

| 用例 | 输入 | 期望 |
|---|---|---|
| `m282_settle_final_continues_after_denied_approval` | denied 行 + 提问收尾 + 无 PR | `waiting_wake`，`thin-continue-*` 立即 pending，非 `ended` |
| `m282_settle_final_without_denial_still_settles_by_rule` | 无 denied 行 | 不产生接续唤醒 |
| `m282_settle_final_respects_user_stop` | 用户停止 + denied 行 | `ended`，无接续唤醒 |
| `m282_settle_final_skips_continuation_after_delivery` | 已合并 PR + denied 行 | 无接续唤醒 |

## Evidence Pack Requirements

- `pnpm cargo:shared test --manifest-path src-tauri/Cargo.toml --lib m282` 全绿
- `pnpm cargo:shared test --manifest-path src-tauri/Cargo.toml --lib agent::thin` 全绿（162 项）
- `python3 tools/governance/validate_repo_governance_baseline.py` pass
- PR body 的 R|是否完成|证据 表

## 兼容性和发布边界

- 不改 schema，不改唤醒 id 形状，不改权限判定，不按措辞分支。
- 真机验收（发版后）：任意派单会话撞一次 heredoc 审批并拒绝，会话 10 秒内自动继续（新的 assistant 消息，而不是 `ended`）。
