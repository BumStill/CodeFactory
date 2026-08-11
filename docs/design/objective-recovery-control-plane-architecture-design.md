# Objective Recovery Control Plane：架构设计

```text
Objective identity (chat segment chain / task)
  -> RootTurn + Tool + ModelRouteEpisode + DeliveryRun projections
  -> RecoverySupervisor (continuous + startup CAS claim)
       -> domain adapter observe/reconcile
       -> idempotency fence before side effect
  -> DecisionRouter
  -> CompletionArbiter
       -> Complete(evidence_ref) is the only business-completed transition
```

## 身份与 revision

`objective_id + repo_identity` 是 active delivery 合并键；`worktree_identity` 来自 canonical gitdir/worktree admin identity 的摘要，路径只作定位。base、初始 worktree 和 requested ceiling 在 run 创建后不可静默替换。head/change-set 是 revisioned identity，只能在当前 lease owner 持有 canonical receipt 且只读对账唯一匹配后前进。

旧行不回填猜测的 objective。新增 partial uniqueness 只约束 identity-complete 新行；旧行保持 NULL 并在过期时投影为 `legacy_orphan`。

## Supervisor

常驻 supervisor 每 15 秒扫描一次已过期 lease（满足 30 秒恢复 SLO），与 startup 使用同一 CAS claim。claim 只授予 observe 权；adapter 先核对本地 worktree、canonical PR/head/receipt 和 requested ceiling，再决定安全动作。两个 supervisor 可重复读取，但 mutation 由 receipt/head fence 保证至多一次。

## Provider fast path

provider 的首次零输出恢复留在 transport/loop 快路径。允许重放必须同时满足：typed failure policy 为 transient、`output_started=false`、`side_effect_started=false`、root-turn retry budget 未耗尽、cancel 未触发。auth、policy、context、vision、field incompatibility 走各自恢复，不泛化为跨 provider replay。

## Completion

Delivery adapter 只能写 observation 与 `awaiting_completion_arbitration`。Arbiter 以 typed evidence 计算 `Continue/Waiting/Incident/CoreInput/BusinessDecision/Complete/Cancelled`，并事务性更新 objective、DeliveryRun 与 root-turn projection。`noop` 永远不单独证明 objective 完成。

## Provenance

`created_by_version/build/process` 不可变；`last_observed_by_*` 可变。attempt 记录 stable failure code、failure class、attempt index、queue wait、runtime、output/side-effect latch、resume owner 与 terminal decision，不保存原文、secret 或完整参数。
