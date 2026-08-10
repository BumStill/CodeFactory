# 持久交付恢复：架构设计

## Authority model

```text
AgentLoop (reasoning and bounded repair)
  -> Delivery Coordinator (durable orchestration)
  -> DeliveryRun + append-only events (authority)
  -> Remote Reconciler / Git receipt (idempotency fence)
  -> Completion Arbiter (only completed writer)
  -> chat_turn_state / session_delivery_refs (UI projections)
```

## DeliveryRun

稳定身份至少包含：`run_id`、`root_turn_id`、`task_segment_id`、`repo_identity`、`base_branch`、`head_branch`、`change_set_digest`、`expected_head_sha`、`canonical_pr_number`、`requested_ceiling`、`reached_state`。

恢复状态至少包含：`stage`、`status`、`wait_class`、`next_action`、`next_action_authorized`、`failure_signature`、`stage_attempt`、`lease_owner`、`lease_expires_at`、`last_observed_at`、`last_progress_at`、`progress_revision`、`app_version`、`build`、`process_instance_id`。

DB 是编排权威；现有 repo-local Git receipt 只防止 merge/release 等外部副作用重复。两者冲突时先读远端事实，不以任一本地记录猜成功。

## Startup recovery

1. App 创建 process instance。
2. 只 claim 租约已过期的非终态 run。
3. 重新核对 repo/base/head/canonical PR。
4. 已 merged/released 则前进；open same head 则续接；head drift 重新绑定证据；unknown/multiple match fail closed。
5. 旧 tool invocation 标为 orphaned/superseded；DeliveryRun 保持真正运行状态。

## CI repair

失败分为 `infra_retryable`、`repo_fixable`、`policy_gate`、`core_input_required`、`needs_business_decision`、`external_state_uncertain`。禁止 generic `user_action_required/needs_user`。`repo_fixable` 创建绑定 run/PR/head/check/failure signature 的持久 repair attempt；同一签名最多两次，只有新 head、blocker 减少或阶段前进才算 progress。

## Completion Arbiter

输出为 `Complete | Continue | Waiting | ApplyRecommended | CoreInputRequired | NeedsBusinessDecision | FailedInternal | PlatformIncident | Cancelled`。Decision Router 优先 `ApplyRecommended`；`autonomous_completion=true` 时不等待普通决策。只有不可逆业务选项无安全默认时才能产生 `NeedsBusinessDecision`。`CoreInputRequired` 必须证明替代路径已耗尽、合并全部缺项且不降低要求。其余恢复耗尽进入系统 remediation。transport Done 只关闭 stream；业务 completed 只能由 `Complete(evidence_ref)` 产生。

## Non-business blocker audit

完成三条 P0 后，必须逐层扫描 agent-loop、tool backends、permission、scheduler、provider route、context budget、delivery、CI/release、browser/session lifecycle 和 UI 文案。任何“回复继续/稍后再试/请自行修复”若没有结构化业务决策字段，均为发布阻断缺陷。

## Compatibility and rollback

- migration additive，旧表字段不删除。
- shadow 写入先于行为切换；旧 DB 重复 migration 幂等。
- 无稳定 identity 的旧 active/pending 仅投影为 legacy orphan，不执行远端 mutation。
- rollback 到旧版本时新表被忽略；已有 Git receipt 继续防重复副作用。
