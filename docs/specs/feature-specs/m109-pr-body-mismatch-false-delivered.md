# M109：PR 正文对账失败后不得把未推送改动标成「已交付、等 CI」

## 背景

### R1 现象与数据（真机两次）

**第一次：cd8d910e / PR #713（v1.88.19）02:55**

| 时刻 | 事实 | 证据 |
| --- | --- | --- |
| 02:5x | 会话在本地修好并自验通过 | 会话输出「本地已绿；推送该修复并等待新一轮 CI」 |
| 02:55 | 调用 `deliver_changes` | 工具结果 `observed PR body does not match the unresolved update`，`failure_signature sha256:d670aaf8…` |
| 同一秒 | objective 变成 `waiting_system / delivered_awaiting_ci` | objectives 行 |
| 之后 40 分钟 | 工作区仍是未提交改动，PR #713 head 不变 | `git status` 有改动；`canonical_pr_progress` 两次看到同一旧 head 却没有判定异常 |
| 后补 | 编排方手工提交并推送 `315fa02b` | 手工动作记录 |

**第二次：59147b73 / PR #704（v1.88.16）**

`delivery_runs` 行：`takeover_mutation_reconciliation | waiting |`
`mutation_intent_reconciliation:observed PR body does not match the unresolved update`。

### 「unresolved update」是什么，为什么不一致

- 「unresolved update」= `delivery_mutation_intents` 里一行未结算的
  `provider_pr_body_update` 意图。它记下的是**上一次进程认为要写进 PR 正文的那段字节**
  的 `external_operation_key("provider_pr_body_update", &[pr_number, body, head, base, head_sha])`。
- 该意图在写前落库（write-ahead），进程在 provider 调用返回前后崩溃/被接管时保持
  `started`/`committed` 未结算状态。下一次接管必须先只读对账：**远端现在的正文是不是我那次更新的结果**。
- 对账实现只做了字节相等比较（`expected_key != intent.operation_key` → 直接
  `Err`）。但 PR 正文是**可覆盖的展示内容**：交付工具自己会重建 PR body（README 决策块修复、
  `Scenario-Test` 行修复、squash 前重写），编排方也会 `gh pr edit` 改正文，
  GitHub 端还可能规范化空白。任何一次外部改写都会让远端正文不再等于旧意图的字节，
  于是对账把「正文被改过」判成「无法安全续接」，整次 `deliver_changes` 返回 tool error。
- 关键错误不是「发现不一致」本身，而是**不一致之后的处理**：这是展示内容，
  应当以观察到的远端正文为准重新生成待更新意图继续走；不需要防重放的副作用保护。

### 为什么错误之后会进 `delivered_awaiting_ci`

`bound_system_recovery`（`src-tauri/src/agent/objective.rs`）在两条分支里把 objective
park 成 `delivered_awaiting_ci`：

1. `pending_pr`：只要 `delivery_runs` 最近一行有 `canonical_pr_url`、`status IN
   ('waiting','pr_opened','merge_queued')` 且 `reached_ceiling IN ('pr_open','pr_only','merge_queued')`，
   就直接 park「PR 已开出、等 CI」。
2. `delivered_pr_closing_state`：只要历史上有过 canonical PR，就把任何后续系统等待
   解释成「改动已交付」。

两条分支都**只看远端 PR 曾存在**，完全不看本地交付身份：本地 HEAD、工作区是否还有
未提交改动、是否有未推送提交。M109 的两台机器上，PR 是上一轮开的，本地还有本次
未推送的修复，于是「PR 存在」被当成「这次改动已交付」，objective 进入
`delivered_awaiting_ci` 并调度 60s 一次的自观察——它永远看不到新 head，因为新 head
根本没被推送，会话空等到窗口耗尽。

结论：`delivered_awaiting_ci` 的准入条件必须包含「本次交付没有遗留本地工作」，
而不是「远端曾经有过一个 PR」。

## 需求

| Req ID | 要求 | 最低证据 |
| --- | --- | --- |
| M109-R1 | 用数据写清 unresolved update 的语义、不一致的成因、以及错误后为何进入 `delivered_awaiting_ci` | 本文「背景」两节；引用 `delivery_mutation_intents` / `delivery_runs` 行与两次会话时间线 |
| M109-R2 | 交付失败（任何 tool error）不得把 objective 置为「已交付、等 CI」；只有推送成功且远端 head 等于本地新提交时才允许 | `bound_system_recovery` 的两条 park 分支都先过本地遗留工作检查；单测断言 tool error 场景的决定不是 `delivered_awaiting_ci` |
| M109-R3 | PR 正文不一致的对账：以观察到的远端正文为准重新结算待更新意图，然后继续提交、推送，不得卡住 | `reconcile_unresolved_delivery_mutation_intents` 的 `provider_pr_body_update` 分支在正文分歧时返回 `pr_body_adopted_observed_body` 并结算意图；单测覆盖匹配/分歧/异 PR 三种 |
| M109-R4 | 「等 CI」期间本地若有未推送提交或未提交改动，必须视为异常并唤醒会话（或直接继续交付），不能空等 | 本地遗留工作探测器（工作区 dirty / HEAD 偏离交付记录 / 有未推送提交）单测；park 分支在探测命中时返回「继续交付」的等待决定 |
| M109-R5 | 测试先行：复现「正文被外部修改 + 本地有新改动 → deliver_changes」，断言改动被推送、状态正确；以及 tool error 后状态不是 `delivered_awaiting_ci` | 先看到失败的用例名与失败断言，再实现；PR 记录 red→green |

## Applicable Harnesses

| Harness | 为什么适用 | 覆盖方式 |
| --- | --- | --- |
| Spec Harness | 交付状态机与控制面语义变更 | 本规格 + Req 追溯表 + 单测映射 |
| Compatibility Harness | 交付 lease/reconcile 是旧配置与旧行（`59147b73`）共用的路径 | 既有 delivery_run 迁移测试与 smoke 全量跑 |
| Observation Harness | 症状是「系统观察不到自己没推送」 | 本地遗留工作探测器 + park 分支断言 |
| AI Collaboration Harness | 交付语义涉及「什么时候可以说已交付」的判断 | PR 内 context scope / assumptions / review point / validation |

不适用：Viewport Harness（无 UI 变更）、Payload Harness（无上传/请求体变更）、
Release Harness（本任务不发版；发版由独立 release 任务完成）。

## 测试矩阵

| 用例 | 覆盖 Req | 断言 |
| --- | --- | --- |
| `m109_pending_local_work_detects_uncommitted_and_unpushed_changes` | R4 | 干净且已推送 → `None`；未提交改动 → 命中；已提交未推送 → 命中；HEAD 偏离交付记录 → 命中 |
| `m109_tool_error_does_not_claim_delivered_awaiting_ci` | R2/R4 | 本地有未推送工作 + 远端已有 PR 时，`bound_system_recovery` 的决定 `failure_code != delivered_awaiting_ci`，且不 park 成已交付 |
| `m109_pr_body_divergence_adopts_the_observed_body` | R3 | 远端正文与旧意图不同 → 返回 `pr_body_adopted_observed_body`、`adopted=true`，无 `Err` |
| `m109_pr_body_matching_still_reports_matches` | R3 | 字节一致时仍返回 `pr_body_matches`（不放宽既有正向确认） |
| `m109_pr_body_from_a_foreign_pull_request_still_fails_closed` | R3 | PR 号与持久化 canonical PR 不同 → 仍然 `Err`（身份问题不属于正文可覆盖范围） |

## 实现说明（不改变需求）

- 正文分歧的结算写入 `delivery_mutation_intents` 的 reconcile 证据，`confirmation =
  pr_body_adopted_observed_body`，并保留 `intent_operation_key` / `observed_operation_key`
  以便事后审计两种字节。
- 本地遗留工作探测纯本地（`git status --porcelain`、`rev-parse HEAD`、
  `rev-list --count HEAD --not --remotes`），不发起网络请求，避免把交付卡在 fetch 上。
- 探测命中时的决定使用新的 `failure_code = delivery_local_work_unpushed`，
  文案明确「交付尚未完成」，并授权下一步继续交付（不要求用户介入）。

## 约束

- 合成 fixture，不写生产 DB。
- 不改 `docs/testing/scenario-registry.json`、`tools/governance/`、`.github/`。
