# M118：M115b 后仍有两处检查不认合并队列/自推头（同族第 5 次）

## 现象（真机 v1.89.2，已含 M115b #738 的统一判定）
1. 会话 b48cc1e5（PR #743）20:31：被 CI 失败正确唤醒并改好测试后，交付报 `observed PR does not prove the persisted delivery head`（failure_signature sha256:9f7c8525…），之后会话保持 active 但 20 分钟无任何事件、无推送（PR 头 2b64d3c0 是合并队列 update-branch 生成的合并头）。编排方提交推送 f6e1a21c。
2. 会话 70c73a35（PR #742，20:00 已合并）：合并后约 40 分钟被判 failed（与 e24314f8/#735 同形：合并后头漂移导致 objective 失败），说明「PR 已合并后以合并结果结算、不再判失败」在某条路径上仍未生效。

## 需求
- R1 全仓搜所有比较 PR 头/远端分支头与持久化头的检查点（含 `observed PR does not prove the persisted delivery head` 的出处），确认是否都调用了 M115b 的统一判定函数；没有的全部改为调用它，并在规格里列出清单。
- R2 PR 已合并的交付：objective 一律按合并结算，任何后续头漂移、远端分支删除/更新都不得把它改为失败（找出 70c73a35 失败的那条路径，用 objective_events 时间线证实）。
- R3 交付工具在「头无法证明」时不得让会话无限 active 静默：要么按统一判定接受并继续，要么给出明确终态与原因。
- R4 测试（先失败）：上述两个真实形状各一条；并加一条「全仓检查点都走统一判定」的结构性测试（例如枚举调用点或以单一入口约束）。

## 约束
- 陌生头仍失败关闭。合成 fixture，不写生产 DB。

## Implementation Notes

### R1 检查点清单（本次改为调用 `delivery_head_is_explained` / `DeliveryRemote::head_is_explained`）

此前已经统一（M115b）：`observe_delivery_takeover(_with_receipted_parent)` 的远端分支检查与 canonical PR 头检查、`observe_open_pr` 的 PR 头核对、recovery supervisor 的 `MergeObservation::HeadChanged` 分支。

本次发现仍未统一、现已全部改为一处判定：

| # | 位置 | 旧行为 | 真实症状 |
|---|---|---|---|
| 1 | `tools/delivery.rs` `reconcile_unresolved_delivery_mutation_intents` → `provider_pr_create` / `provider_pr_open_or_get` | `state.head_sha != expected_head_sha` 精确比较 | 本次 #743 的 `observed PR does not prove the persisted delivery head` |
| 2 | 同一函数的 `provider_pr_body_update` | 精确比较，报 "attached to a foreign head" | 同族潜在形状 |
| 3 | `agent/delivery.rs` `observed_committed_pr_projection` | 精确比较（`committed PR receipt … canonical PR identity/body/head`） | 已 committed 的 PR create 回执被队列头判失败 |
| 4 | `agent/delivery.rs` `exact_open_pr_projection` | 精确比较（`open PR is attached to a foreign head`） | PR 观察在队列头上拒绝 |
| 5 | `agent/delivery.rs` `exact_created_pr_projection` | 精确比较（`observed canonical PR has a foreign head`） | PR 创建结果核对在队列头上拒绝 |
| 6 | `agent/delivery.rs` `exact_updated_pr_projection` | 精确比较（`has a foreign head, body, branch`） | PR 正文/标题更新核对在队列头上拒绝 |

单一入口：所有检查点都调用 `agent::delivery::delivery_head_is_explained`（本地 Git 证明自推头/祖先/本地队列合并头）或 `DeliveryRemote::head_is_explained`（provider 证明队列同步父提交），不再各自比较 SHA。为了让投影函数也能走同一判定，四个投影函数改为接收 `head_proven: &dyn Fn(&str) -> bool`，由调用方注入统一判定；`tools/delivery.rs` 的两个检查点直接调用 `remote.head_is_explained`。

### R2 已合并交付的终态

70c73a35 的失败路径：合并队列合并 PR 后，远端分支被删除或继续漂移，`delivery_runs.reached_ceiling` 在被观察到的合并回执升级之前仍是旧值；后续 recovery supervisor 重新 claim 该 run，走到 `observe_delivery_takeover` 的本地身份/分支检查失败，`record_or_park_local_takeover_identity_conflict` 连续 `MAX_IDENTICAL_TAKEOVER_FAILURES` 次后调用 `park_delivery_identity_incident_after_takeover`，把已经成功的 objective 显示成「失败」。

修复：在 `record_or_park_local_takeover_identity_conflict`（唯一的 takeover 失败归因点）记录任何失败之前，先读持久化的合并结论；只要该 run/objective 已经由合并结算（PR 观察已 concluded、`reached_ceiling` 已是合并终态、或存在 reconcile 的合并回执），就按合并结算并释放租约，不再写入 failure attempt、不再 park。陌生头在未合并的 run 上仍走原有 fail-closed 路径。

### R3 头无法证明时的终态

`resume_claimed_delivery_with_remote` 不再因为 `canonical_head_sha` 为空而把合并观察降级为 `Unsupported`（那样会落到 `unknown` 分支空等）；改用持久化 `expected_head_sha` 作为合并观察的已知头。头无法证明且不是合并时，仍按既有 bounded fail-closed 路径给出明确终态（`platform_incident` → park），不允许无限 active 静默。

### Applicable Harnesses

- Spec Harness：本规格 + PR body 的 Req↔test 映射。
- Compatibility Harness：不改 schema、不改 provider 协议、不放宽陌生头拒绝。
- Observation Harness：合并结论先于任何失败归因被读取；objective 不再从成功翻成失败。
- Release Harness：仅交付到合并队列，不发版。
- AI Collaboration Harness：context scope / assumptions / review point / validation result 记录在 PR。

### 真机验收分工

真机验收由编排方发版后完成：安装新版本，触发一次交付让合并队列 update-branch，核对 ①CI 失败唤醒后会话能重新推送（不再出现 `does not prove the persisted delivery head`）②合并后 objective 成功且 40 分钟后不翻成失败 ③陌生头仍被拒绝的审计记录。
