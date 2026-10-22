# M157：回合中途重启的可执行 liveness 守门

## 背景

2026-10-22 一天内四次出现同一形态的卡死：上一个进程把回合停在 `running`，
重启后的进程既不续跑也不结算，用户界面上的「正在做」永远不动，只能靠再发一句话
或重启应用绕过去。启动扫描（`running_interrupted_by_restart`）与心跳扫描
（`running_without_executor`）已经把这类会话选出来，但没有任何跨进程的可执行
证据能阻止它再次退化。

## 需求（Req）

- **Req-1**：回合中途重启后，重启扫描必须仍然选中该会话，即使它是在重启前
  66 秒才开始的回合（不得被心跳宽限挡住）。
- **Req-2**：上一进程留下的 live 控制行（`chat_run_controls` 中 `active` /
  `cancel_requested`）必须被回收；否则用户下一句话会永久排队。
- **Req-3**：跨重启仍在飞的工具调用必须被显式写成「结果未知」，不得留在
  `pending` / `approved`。
- **Req-4**：重启不得伪造结算——用户消息条数不变、回合仍是 `active`、thin 状态
  仍是 `running`；也就是"接管"而不是"结束"。
- **Req-5**：第二次重启必须幂等：回收与标记都返回 0，且在宽限期内不得被心跳
  扫描再次选中（避免热循环）。
- **Req-6**：以上全部在真实多进程、真实 SQLite 旋转上验证，并在回执里留下
  可机器断言的布尔字段。

## 实现

`src-tauri/src/agent/history_session_smoke.rs` 新增三个 phase：

| phase | 作用 |
| --- | --- |
| `seed-liveness` | 在真实 schema 上播种两种现场：①工具执行中被杀（用户消息 + 未结算回合 + 在飞工具调用 + live 控制行）；②模型流式阶段被杀（用户消息 + 未结算回合 + live 控制行，无工具调用） |
| `verify-liveness` | 每条 Req 的断言：两个扫描都选中、控制行回收恰好 1 条、live 计数归零、工具调用标记为不确定、用户消息数不变、回合仍 `active`、thin 仍 `running` |
| `verify-liveness-again` | 第二次重启的幂等性与宽限期不热循环断言 |

父进程 `process_restart_count` 与 `pids.len()` 同步改为 14，回执新增：

- `liveness_receipt_schema: codefactory.liveness_restart_smoke.v1`
- `mid_turn_restart_still_selected_after_restart`
- `mid_turn_restart_never_left_running_alone`
- `mid_turn_restart_phantom_run_control_retired`
- `mid_turn_restart_tool_call_marked_unknown`
- `mid_turn_restart_idempotent_on_second_restart`
- `mid_turn_restart_no_user_reprompt_needed`

## 验证

```
node scripts/cargo-shared.mjs run --manifest-path src-tauri/Cargo.toml -- \
  --history-session-smoke /tmp/m157-receipt.json
```

实测结果（2026-10-22）：`ok: true`，`process_restart_count: 14`，上述六个布尔字段全为
`true`，`scenario_ids` 为 `E2E-002`、`E2E-003`、`E2E-007`。

## Scenario

`Scenario-Test: E2E-002, E2E-003, E2E-007`

## 未覆盖（保持诚实）

- `ui_oracle_status: remaining_L3_real_desktop_gap`：真实桌面 UI 的「正在做」消失
  仍需 L3 人工/截图证据，本 smoke 只覆盖后端主路径。
- `sweep_timer_oracle_status: supervisor_cadence_covered_by_unit_wiring_only`：
  清扫节奏由 supervisor 单元接线覆盖，本 smoke 不验证定时器周期。
