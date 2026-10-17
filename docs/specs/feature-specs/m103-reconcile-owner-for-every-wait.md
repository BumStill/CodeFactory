# M103 家族审查：每一个「先观察/对账再继续」的状态都必须有真正的对账者

## 现象（真机 2026-10-17 22:46–23:01，v1.88.16，期间无重启）
- 会话 3f0f0236（E2E-008）：22:47:28 `external_state_uncertain` → 22:48:32 `agent_loop_error`：`prepare provider attempt: previous provider attempt "in_flight" cannot continue under this live owner; observe/reconcile first` → 22:48:42 起反复 `provider_episode_unreconciled`：`PROVIDER_EPISODE_UNRECONCILED: prior provider episode is not proven replay-safe; observe/reconcile before a new Objective revision`（22:48、22:49、22:50、22:55、23:01）→ 23:01:10 `failed / technical_recovery_exhausted`。
- 同族：M98（`response_committed` 同样报「observe/reconcile first」，#692 只修了这一种状态）、M88（等待态无人重新观测）、M96（围栏）、M99。第 4 次以上。**本单是家族审查：不再一个状态一个状态补。**

## 取证（只读，先数据后代码）
- 本机 DB：3f0f0236 的 objective_events、provider_route_episodes、provider_route_attempts（含 in_flight 那条的字段）、model_route_attempts、tool_calls（22:46–22:48 那次导致 external_state_uncertain 的调用）。
- 代码：全仓搜所有产生「observe/reconcile first」「not proven replay-safe」「UNRECONCILED」之类要求先对账的错误点。

## 需求
- R1 列出所有「要求先对账」的状态/错误码（attempt: in_flight / response_committed / …；episode: unreconciled；delivery: external_state_uncertain 等），每一个写明：谁负责对账、对账依据什么证据、对账后的结果（安全重放 / 视为已完成并结算 / 真正不可恢复的明确终态）。写入规格。
- R2 实现缺失的对账者：in_flight 的 provider attempt 与 unreconciled episode 在当前 owner 下能被观察与结算（例如：无输出且无副作用开始 → 可安全重放；有输出 → 按已落库内容结算；有副作用 → 按回执对账），然后继续，不消耗恢复预算。
- R3 对账本身不得计入恢复预算；同一 failure_signature 连续出现时，必须先跑对账而不是重复同一失败。
- R4 真正无法证明安全的情况：进入明确、用户看得懂的终态并说明原因与成果位置，不得反复空转后报「恢复耗尽」。
- R5 测试（先失败）：in_flight attempt 无输出 → 自动重放继续；in_flight 有输出 → 结算后继续；episode unreconciled → 对账后继续；有不可证明的副作用 → 明确终态。用合成 fixture，复现 22:47–23:01 的事件序列。
- R6 能挂进 E2E-001/E2E-007 的自动化就挂（registry 修改本单明确授权，状态如实）。

## 约束
- 不放宽副作用安全边界（有未对账的外部副作用时不得盲目重放）。合成 fixture，不写生产 DB。

## Implementation Notes
- 取证仅提取状态、时间和布尔锁存，不保留生产消息、参数或原始标识。最后请求为 remediation 的 in_flight，output_started=0、side_effect_started=0、无关联回执；后续转 Chat 恢复反复被 episode 围栏拒绝。
- 适用 Harness：Spec、Compatibility（旧持久状态）、Observation（证据与收敛）、Release（本单只合并，真机验收编排方发版后完成）、AI Collaboration。
- 验收映射：R2/R3/R5 通过 provider_auth_recovery 合成 SQLite 集成测试，R4 通过 Objective 终态测试；R1 通过下列状态责任矩阵；R6 复用现有 E2E runner 的组件测试，不宣称完整复杂 E2E 已实现。

## 状态责任矩阵（R1）
| 状态 / 错误 | 对账者 | 证据 | 结果 |
| --- | --- | --- | --- |
| prepared | ProviderRecoveryStore/adopt_prepared_attempt | 当前 permit + 零锁存，POST 未准入 | 原请求接管 |
| in_flight / streaming / unknown | provider 请求串行边界与 supervisor | 当前 permit、请求结束边界、输出 checkpoint、无未决 receipt | 保留输出、结算为 replayable 后继续 |
| response_committed | ProviderRecoveryStore | 持久 response/checkpoint 与 permit | 已完成步骤结算后继续，不重复工具 |
| provider_episode_unreconciled | open_episode 与请求边界对账 | 最新请求、binding/objective 副作用、receipt | 可证明安全则关闭旧 episode；否则明确终态 |
| ObserveOnlySideEffect / 未决 receipt | Tool/Delivery adapter 的 read-only observe/reconcile | 精确 receipt、action signature、resource generation | 已完成则结算，明确无副作用则重放；不可证明则禁止重放 |
| external_state_uncertain（工具/交付） | objective supervisor + 对应 adapter | 工具回执、外部 PR/CI/merge 等只读观测 | 对账后继续或明确失败，不将未知当成功 |
| update 安装占用 | update_safety reconcile | 安装 attempt / 当前安装状态 | 已结束则释放，否则不重复安装 |
| stale permit / binding / generation | permit 校验与 supervisor | revision/epoch/lease | fenced，不接管另一活跃 writer |

## 发版后真机验收（编排方）
1. 安装后触发合成 in_flight 无输出及有 checkpoint 的恢复，确认无需用户“继续”。
2. 核对审计：先对账，再发起下一请求；恢复预算不因对账递增。
3. 未知工具副作用必须停止并展示原因与已有成果位置，不以恢复耗尽收尾。
