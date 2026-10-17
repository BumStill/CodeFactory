# M98 家族审查：应用升级/重启后接管进行中的任务，必须先对账再继续，不能直接判恢复耗尽

## 现象（真机，2026-10-17 17:28，v1.88.11 → v1.88.12 自动安装重启）

- 会话 3b87a1b6（PR #688，CI 全绿，只差同步 main 后打 armed）。
- 17:28:52 objective_events：`technical_failure_detail` / `agent_loop_error`：
  `prepare provider attempt: previous provider attempt "response_committed" cannot continue under this live owner; observe/reconcile first`
- 17:28:57 `failed / technical_recovery_exhausted`（5 秒内）。delivery_runs 同时变为 `takeover_reconciliation | platform_incident | takeover_reconciliation:delivery_identity_conflict`。
- 报错本身说了「observe/reconcile first」，但没有任何路径去做这次对账，恢复预算直接烧光。
- 同族历史：M79（升级重启后 chat_identity_unreconcilable 失败）、M75（同一工作区续改被判身份冲突）、M89（睡眠后 FENCED）。第 4 次以上 → **家族审查，不是补丁**。
- 关联 2.0 门禁：E2E-001（跨进程自动完成）、E2E-006（历史卡住任务存在时仍可完成应用更新）。

## 取证起点（只读，先数据后代码）

- 本机 DB：3b87a1b6 的 objective_events（17:26–17:29）、model_route_attempts / provider attempt 表中状态为 response_committed 的那条、delivery_runs 全字段、chat_turn_state。
- 弄清：重启前那次 provider attempt 为何停在 response_committed（响应已落库但回合未结算？）；新 owner 为何不能接管；delivery 的 identity_conflict 是由同一原因引起还是独立判定。

## 需求

- R1 用数据写清根因，写入规格。
- R2 重启接管时，遇到上一 owner 留下的 response_committed（或其他「已提交但未结算」的中间态）：先对账——已落库的响应视为已完成的一步，结算后继续下一步；不得把它当成恢复失败计入预算。
- R3 交付记录在重启接管时：同一 objective、同一分支、远端 head 可由本地工作区解释（祖先或相等），判定为同一身份并续接，不判 identity_conflict。真正外来的改动仍要阻断（保持 R6 类安全边界）。
- R4 恢复预算只计真实失败的尝试；对账步骤不消耗预算。
- R5 测试：合成 fixture 复现「response_committed 时进程被替换 → 新进程启动」，断言 objective 自动继续到完成，用户消息数不变、无需人工；以及「远端有外来提交」仍阻断。先失败。
- R6 优先把这条场景接进 E2E-001 / E2E-006 的自动化（scenario-registry 对应 automated_by），减少它们的 remaining_gaps。

## 约束

- 不放宽外部写入的安全边界；合成 fixture，不写生产 DB。

## R1 根因（本机数据 + 代码路径）

### 现场数据

只读查询本机 `codefactory.db`（objective `38ff789d-db30-42ac-b19f-437ba3adda90`，即会话 3b87a1b6 的 objective，revision 2；时间均为 2026-10-17 SGT）：

| 时间 | 证据 |
| --- | --- |
| 17:05:51 → 17:27:09 | `provider_route_attempts` 中该 objective 连续留下 12 条 `status='response_committed'`、`output_started=1`、`side_effect_started=0`、`owner_kind='remediation'`、`owner_id='objective-supervisor:44238:…'`、`owner_epoch=1` 的尝试（例：`9f0f443e` 17:05:51、`798eb5f5` 17:26:31、`7fe94c91` 17:26:51、`e6ac6166` 17:27:01、`58eef688` 17:27:05、`4e3ff2e2` 17:27:07） |
| 17:26:31 / 17:27:05 | 同批还有 `failed_replayable` + `provider_transport` / `provider_external_state_uncertain`（`b8546540`、`df16c08a`）——真实失败与「已提交但未结算」在同一 episode 里并存 |
| 17:28:52 | `objective_events` revision 2：`technical_failure_detail` / `agent_loop_error`，正文 `prepare provider attempt: previous provider attempt "response_committed" cannot continue under this live owner; observe/reconcile first` |
| 17:28:52 | revision 3：`decision_applied` / `waiting_system` / `agent_loop_error` |
| 17:28:57 | revision 4：`decision_applied` / `failed` / `technical_recovery_exhausted` |

`owner_id` 里的 `44238` 是升级前的进程号：v1.88.12 重启后新进程用新的 owner/epoch 复跑同一 objective revision，第一个动作就是 `begin_attempt`，立刻撞上旧 owner 遗留的 `response_committed`。从技术失败到 `technical_recovery_exhausted` 只隔 5 秒，整个窗口内 objective 只有 5 条事件、1 条技术失败——说明预算不是被多次真实失败烧掉的，而是被**一次无法对账的接管**判死的。

### 代码路径

1. `ProviderRecoveryStore::commit_response` 按设计让 attempt 停在 `response_committed`、episode 保持 active：一次 provider 往返的文本已落库，但同一 owner 还要用 tool 结果发起下一轮。这在**同一 owner** 内是正确的中间态。
2. 进程被升级替换后，新 owner 的 `begin_attempt`（`src-tauri/src/agent/provider_recovery.rs`，`previous provider attempt {status:?} cannot continue under this live owner; observe/reconcile first`）把任何「非终态且非本 owner」的旧 attempt 判为不可继续，直接返回错误。**该错误指向的 observe/reconcile 路径并不存在**：`observe()` 把 `response_committed` 映射为 `ResponseCheckpoint`（observe-only，等待旧 owner 结算——而旧 owner 已经不在了），`startup_recovery_candidates` 又只选择 `in_flight`/`streaming`/`unknown`，于是 `response_committed` 既不能续接也不会被启动对账选中，只剩「技术失败 → 预算耗尽」。
3. 交付侧同源：`observe_delivery_takeover_with_receipted_parent` 只接受远端 head 与持久化 head **严格相等**或等于 receipted parent。重启期间本地 workspace 可能已经推进（合并 main、squash 后本地 ahead、push 尚未执行），这种「远端 head 是本工作区的祖先」完全可由本地解释，却被判成 `delivery_identity_conflict`（`takeover_reconciliation:delivery_identity_conflict`）。同一个家族缺陷：**重启边界把持久化进度当成外来身份**。

结论：M98 不是「provider 偶发错误」，而是重启接管缺少**对账（reconcile）阶段**——把上一 owner 已落库的完成步骤当成需要旧 owner 才能结算的活状态，把同一 objective 的可解释 head 当成外来身份。

## Applicable Harnesses

- Spec Harness（本规格 + Requirements Traceability + 测试矩阵）。
- Compatibility Harness（v1.88.11 → v1.88.12 升级重启边界、旧 owner/旧 epoch 数据）。
- Release Harness（该缺陷发生在已发布版本的自动更新路径上）。
- Observation Harness（对账必须留下与真实失败可区分的审计码）。
- AI Collaboration Harness（接手他人分支的改动，假设与复核点见 PR）。

## Requirements Traceability

| Req ID | 需求 | 最小证据 |
| --- | --- | --- |
| R1 | 用数据写清根因并写入规格 | 本节现场数据表 + 代码路径引用 |
| R2 | 接管时先对账 response_committed，结算为已完成步骤后继续 | `m98_restart_reconciles_committed_response_before_next_attempt` |
| R3 | 同一 objective/分支、远端 head 可由本地解释则续接；外来 head 仍阻断 | `m98_remote_ancestor_head_continues_the_same_identity`、`m98_foreign_remote_head_still_blocks_takeover` |
| R4 | 对账不消耗恢复预算、不产生技术失败事件 | `m98_restart_reconciles_committed_response_before_next_attempt` 的候选集合与 `technical_failure_detail` 断言 |
| R5 | 合成 fixture 复现升级重启接管；外来提交仍阻断；先失败 | 上述 4 个测试，先回退实现看到失败再恢复实现转绿 |
| R6 | 把场景接进 E2E-001 / E2E-006 自动化 | 见 Implementation Notes：registry 由编排方维护，测试名已在 PR 声明 |

## 测试矩阵

| 场景 | fixture | 断言 |
| --- | --- | --- |
| 升级重启接管已提交响应（R2/R4） | 内存 SQLite + 旧 owner `response_committed`（`output_started=1`、`side_effect_started=0`），随后更换 lease owner / remediation epoch | 新 owner `begin_attempt` 成功；旧 attempt 变 `failed_replayable` + `provider_response_checkpoint_reconciled`；`startup_recovery_candidates` 不含该 objective；`technical_failure_detail` 计数为 0 |
| 有未结算副作用时仍 fence（R2 安全边界） | 旧 owner 先 `begin_side_effect` 再 `commit_response(side_effect_started=true)` | 新 owner `begin_attempt` 报错；旧 attempt 保持 `response_committed` |
| 远端 head 是本地祖先（R3） | 合成 bare origin + 受管分支，本地合入 main 后远端仍停在 pre-merge head | `observe_delivery_takeover_with_receipted_parent` 成功，同一身份续接 |
| 远端有外来提交（R3 安全边界） | 用 `commit-tree` 造一条本地无法解释的提交并强推受管分支 | `observe_delivery_takeover_with_receipted_parent` 返回 Err |

## Implementation Notes

- R2 的结算写在新 owner `begin_attempt` 的同一事务里，条件收紧为：`response_committed` 且 `output_started<>0` 且 `side_effect_started=0` 且无 `started`/`unknown` 的 `side_effect_receipts`。审计上使用独立的 `failure_code='provider_response_checkpoint_reconciled'`，与真实 provider 失败区分。
- R4 在 `startup_recovery_candidates` 里排除已可对账的 `response_committed`（以及已结算记录），使对账不进入恢复候选、不产生 `technical_failure_detail`。
- R3 的判定顺序：等于持久化 head → 等于 receipted parent → 是本地 head 的祖先；否则 fail-closed。只放宽「本工作区可解释」的方向（远端落后），不放开「远端超前且本地无法解释」。
- R6：场景自动化登记已落到 `docs/testing/scenario-registry.json`——E2E-001 / E2E-006 的 `automated_by` 增加 `rust:m98_remote_ancestor_head_continues_the_same_identity`（validator 只扫描 `src-tauri/src` 与 `src-tauri/crates` 下的 `#[test]`，`src-tauri/tests/` 的集成测试无法被引用）。`remaining_gaps` 中仍待 real-nightly 与真机 UI 的条目保持不变，不做虚假关闭。
