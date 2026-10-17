# M96：前台交付新 claim 在首次写入前对账

## 用户需求（原文）

- R1 用数据写清根因（哪个代码路径在 git_stage 前使 permit 失效），写入规格文档。
- R2 修复使该场景交付可完成：同一 run、同一 owner 连续 stage/commit/push 不再被自己的另一条路径抢占。
- R3 防 limbo：permit 丢失若可安全重取（远端未变、外部写入为零），交付工具内部有界重取并继续；仍失败时给出明确终态与原因，不让 agent 结束回合等外部释放。
- R4 回归测试：复现「签发后被另一路径抬 epoch」的先失败测试；以及持有者进程已死时自动回收。
- R5 家族收敛：盘点所有会抬 epoch / 夺取 permit 的路径，文档化唯一所有权规则。

约束：不放宽外部写入的安全边界；远端状态不确定时仍须对账后再写；合成 fixture，不写生产 DB。

## 只读现场证据与根因（R1）

2026-10-17 本机 SQLite `mode=ro` 查询 run `delivery-8d6cefb9…`：
13:45:17 后台释放 epoch 115；13:45:51 释放 116；13:46:16 前台 epoch 117 失败并释放；13:46:28 后台释放 118；13:47:00 前台 epoch 119 失败并释放；13:47:01 后台才签发 120，13:47:04 释放。
最终观测 claim_epoch=167、reconciled_claim_epoch=14；自 epoch 14 的 base_sync 之后无 claim_reconciled。

直接根因：prepare_durable_run 调用 create_delivery_run 为每次前台 invocation（带独立 UUID）签发新 epoch，新 claim 保持 observe-only。前台随后直接挂 mutation_permit 调用 deliver，未执行后台 resume_claimed_delivery_with_remote 已有的只读对账与 mark_delivery_claim_reconciled。于是 owner/epoch 都匹配也会在 git_stage 因 reconciled_claim_epoch 不匹配被拒绝。

后台的周期性认领发生在前台 invocation_return 释放之后。上述证据**不支持**“后台在 epoch 119 失败之前抬 epoch”的结论；周期扫描放大 epoch 数字，但不能凭此删除 invocation UUID、跳过过期 fencing 或直接晋升权限。初版此类候选已撤回。

## 唯一所有权规则（R5）

| 路径 | 权限与 epoch |
| --- | --- |
| create_delivery_run 新建 | epoch=1 且 identity 已捕获；初始 claim reconciled |
| create_delivery_run 重入 | 同一 live invocation 原地续期；释放、过期或不同 invocation 必须新 epoch 并重新对账 |
| plan_startup_recovery | 仅无人持有或过期 claim；新 epoch 默认 observe-only |
| foreground reconcile | 只读核对 local identity、remote branch、canonical PR、未决 mutation intents、release receipt；随后 CAS 标记同 owner/epoch reconciled |
| resume_claimed_delivery_with_remote | 同样先对账后晋升，旧 epoch 不得写 |
| renew_delivery_lease | 只续 TTL，不改变 epoch，不得复活过期租约 |
| release_delivery_lease | owner + epoch CAS 清空 lease；不改变 epoch |
| mark_delivery_claim_reconciled | 只允许仍然 live 的 owner/epoch 且无未决 intent；调用方必须提供真实对账前提 |
| permit 每 rung 校验 | fresh DB 校验；元数据读取成功不是远端对账 |

## 实施与验收矩阵

| Req | 实施 | 验证 |
| --- | --- | --- |
| R1 | 本文时间线及实际调用链 | 生产 DB 全程只读 |
| R2 | 前台首次写入前补齐真实对账；心跳覆盖对账；不改变 epoch 规则 | foreground_readmission_reconciles_before_first_mutation，真实 bare origin + Git worktree + 内存 SQLite |
| R3 | 同调用对账成功即继续；失败不得写，lease guard 必须释放 | 同用例断言旧 permit false、新 claim 对账后 true，HEAD 不变、intent 数=0；外来远端仍被现有 observer 拒绝 |
| R4 | 重启后过期 claim 由真实 plan_startup_recovery 回收；再在前台恢复 | 上述集成测试；既有 takeover_requires_and_consumes_positive_push_observation_before_new_permit / lost_heartbeat_drops_old_future_and_takeover_stays_observe_only |
| R5 | 不改 create/renew/release 的既有围栏语义；完整盘点见上表 | 现有 delivery 测试回归 |

## Harness 与协作记录

Spec、Compatibility、Observation、Release、AI Collaboration Harness：本变更属于已发布工具运行路径修复；不执行发版。无 UI，不需 Viewport Harness。无 schema 或授权上限变化。真实安装版验收由编排方发版后完成。

Context scope：任务文件、模板、真实 DB、前台/后台交付与 mutation ledger。Assumption：不能把同一 OS 进程等同唯一 invocation。Review point：禁止仅凭 owner 相同直接 reconcile；禁止让旧 permit 跟随新 epoch。Validation result：记录于 PR；未完成项不得宣称通过。

## 发版后真实应用验收

编排方安装新版后在已有 PR 的返修会话调用 deliver_changes，检查新 epoch 出现 claim_reconciled 且继续 stage/commit/push，同一 PR 不重复创建；断开远端或制造 head 不匹配时不得写，工具返回后 lease 应释放。会话内仅使用合成仓库，不改生产 DB。
