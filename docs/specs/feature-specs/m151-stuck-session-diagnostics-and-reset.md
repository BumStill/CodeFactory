# M151 卡住的会话：可见的诊断与可用的重置

状态：实施中（本 PR）
适用版本：v1.91.x 升级后存量会话
主路径：用户打开 CodeFactory → 左侧会话列表 → 某个会话永远显示「正在做 · 等待中」→ 现在可以「会话诊断」看到为什么卡、并「重置会话状态」把会话拉回可用。

## 现场与问题

v1.91.21（Windows 升级安装之后）出现一类存量会话：

- 界面上永远显示「正在做 · 等待中」，不会再产生任何模型输出；
- 用户既发不出下一条消息，也得不到任何关于「在等什么」的解释；
- 唯一能看的东西是设置页的扩展诊断——它讲的是扩展，不是这个会话。

这类「正在做」让用户无从判断：是模型在跑、在等工具、在等 PR 的 CI、在等授权，
还是根本没有执行者在跑。事实层面通常有两种成因：

1. `thin_session_state` 缓存里写着 `running`，但那个回合的执行者早已随进程退出。
   这一条此前被有意保留（见下文「含糊不得降级」），于是缓存可以永久自持。
2. `chat_run_controls` 里留着上一进程的 live 状态行（`active` / `cancel_requested`）。
   这张表的 `session_id` 上有 live 唯一索引——它就是会话级的排他锁。残留一行，
   新回合永远排「等待中」，且没有任何入口把它清掉。

## 需求

| ID | 需求 |
| --- | --- |
| R1 | 会话级诊断：导出「为什么卡」的全部事实依据（状态缓存与重算结果、执行者是否存活、live/无主控制行、后台任务、唤醒、待审批、取消意图、交付 PR、最近事件、消息计数），**不含消息正文与会话凭据**。 |
| R2 | 缓存里的 `running` 只有在存在**硬证据**（上一进程留下的 live 控制行）时才可以被判成结束；**含糊不得降级**（没有控制行的会话不因此被判结束）。 |
| R3 | 单会话重置：会话没有活执行者时，结算残留取消意图、回收无主控制行、把状态重算成 `ended`/`waiting_user`；有活执行者时**拒绝**并说明原因。 |
| R4 | 启动时对 thin 会话重算状态（含 `cancel_requested` 的残留控制行），修正被写坏的缓存。 |
| R5 | 状态都要给出可解释的 `reason`；不得出现无理由的「正在做」。 |
| R6 | 准入/启动自愈：无主 live 控制行（含 `cancel_requested`）必须被回收，不得让上一进程的残留把新回合挡在「等待中」。 |
| R7 | 启动对账把 `active` 与 `cancel_requested` 同等视为 live 状态行。 |
| R8 | 入口在会话行菜单里，用户不离开主路径即可操作；重置结果当场可见。 |
| R9 | 重置**绝不**删除消息、工具结果、摘要或交付记录；诊断不含凭据。 |

## 设计

### 判据（R2 与「含糊不得降级」并存）

`thin_derive::derive_state` 在回落到缓存前增加一条硬证据判断：

```
缓存 running 且 chat_run_controls 里存在 status ∈ {active, cancel_requested}
且 created_process_instance ≠ 当前进程 → 判 ended（"上一回合的执行者已随进程退出"）
```

- 存在控制行、且由别的进程创建 → 回合确定已死，缓存不可信；
- 没有控制行 → 含糊（thin 会话并不都用控制行），保持既有行为，不降级。
  这条边界由既有回归用例 `uncertain_running_projection_is_not_ended_or_retimestamped`
  与 `legacy_convergence_reasons_never_end_a_running_thin_session` 守住。

### 重置（R3/R9）

`agent::objective_diagnostics::reset_session_state`：

1. 会话仍有活执行者 → `reset=false` + 原因，什么都不改；
2. 删除该会话的取消意图（`chat_session_cancel_intents`）；
3. 回收该会话状态为 live 但不属于本进程运行的控制行（`status='completed'`）；
4. `thin_derive::refresh` 重算；若仍被判成 `running`，落一个带 `reason` 的 `ended`
   （"已手动重置会话状态…"）；
5. 前后消息条数必须相等（R9 的机器证据）；
6. 写一条 `session_audit_events(session_state_reset)` 审计事件（可追溯，不删除任何东西）。

### 诊断（R1）

`agent::objective_diagnostics::collect` 产出 `codefactory.session_diagnostics.v1` JSON。
只读：不修改任何状态。消息部分只有条数与时间范围（`count/first_at/last_at/assistant_finals/user_last_at`），
正文一律不出现；判断依据中的进程身份只有"是否等于当前进程"的布尔结果与行内容，不含环境凭据。

### 界面（R8）

「正在做 / 等唤醒 / 需要你决定」这行状态下方新增两项：

- **会话诊断**：调用 `session_diagnostics`，把 JSON 复制到剪贴板（工作副本环境无剪贴板时退化为在行内显示推导状态）；
- **重置会话状态**：调用 `reset_session_state`，成功后就地显示「已重置为已结束/等待你，消息都还在」并刷新会话列表；
  被拒绝时显示后端给出的原因。

## 验收（机器证据）

| Req | 测试 |
| --- | --- |
| R1/R2 | `agent::objective_diagnostics::tests::m151_diagnostics_flag_poisoned_cache_and_stay_content_free`（断言 JSON 不含消息正文、`cache_matches_facts=false`、`orphan_run_controls=1`） |
| R2 边界 | `agent::thin_derive::tests::uncertain_running_projection_is_not_ended_or_retimestamped`、`legacy_convergence_reasons_never_end_a_running_thin_session`（保持通过） |
| R3/R9 | `m151_reset_makes_a_stuck_session_usable_without_losing_content`（`settled_cancel_intents=1`、`retired_run_controls=1`、前后消息条数相等、结果状态 `ended`） |
| R3 拒绝 | `m151_reset_refuses_while_the_turn_is_alive` |
| R6 | `m151_orphan_controls_are_retired_before_admission`（`cancel_requested` 与 `active` 都被回收，`orphan_run_control_count` 归零） |
| R7 | `objective::tests` 中对账相关用例 + 本文件 R6 用例（同一状态集合） |

## 未覆盖 / 已知限制

- 会话**没有**任何控制行、却缓存成 `running` 的存量会话，按 R2 仍不会自动降级：
  这是既有里程碑的明确要求（含糊不得降级）。这类会话的出路是本 PR 的显式「重置会话状态」（R3）。
- 本 PR 未在真实升级安装的 Windows 环境上复现原始现场；上述判据与重置路径由单测与本地构建覆盖，
  运行期验证在合并后的发布批次里进行。
