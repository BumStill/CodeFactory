# M74 验收与 PR 记录

Scenario-Test: HLT-001, HLT-002, HLT-005, CXD-002, RTE-004
README-Update: reviewed
README-Update-Reason: 内部交付对账返修，不改变公开产品、安装或隐私承诺。

## 要求与自验收
- CF-TRUTH-R7 PASS：local_takeover_identity_conflict_has_a_stable_bounded_recovery_signature 先失败（platform_incident != waiting），修复为只读等待，不因重复 replay 失败耗尽 objective；mutation 仍 fenced。
- CF-TRUTH-R8 PASS：m74_remote_merge_repairs_failed_cancelled_and_incident_with_stale_workspace 先失败（failed != completed），修复后合成 failed/cancelled/platform_incident delivery 记录均 completed/merged，objective completed，failure 清空。覆盖本地 identity/head 漂移、工作区 closed 且已移除。
- CF-TRUTH-R9 PASS：同用例 Unknown 保持原状，重复 merge 不递增 revision；m71_failed_takeover_tracks_open_queued_and_external_merge 验证入队及更早观察不回退。
- live/release PASS：m71_external_merge_never_fakes_live_acceptance 保持 waiting_system，不删除工作区。
- 安全 closeout PASS：41 项工作区、39 项交付工具回归，真实临时 Git + 合成 SQLite；未读写生产数据库，不改会话内容。
- 完整 Rust workspace --no-fail-fast 通过：主库 1567 passed / 9 ignored，其他 workspace/integration/doc suites 通过。前端 pnpm test 144 files / 956 tests 通过（含 lightModeAudit）；pnpm build 通过。

## Harness 与协作
Spec：沿用 R7/R8/R9；Compatibility：历史终态和旧 schema；Observation：canonical_pr_progress 证据、幂等及 CAS lease；Release：授权只到 merged，不发版。AI Collaboration：context scope 是 task M74、template、CF-TRUTH 与两处实现；assumptions 为正向远端 merge 才修复终态；review point 为观察不授予 replay/删除权限；validation result 为上述测试。任务模板禁止 delegate_tasks，采用单执行流。无 UI 改动；后台 supervisor 入口保持可用。

## 真机核对（独立发布后，not live）
安装包含本 PR 的新版、重启并等待后台对账；由用户在实际 sessions.db 上只读执行（本任务未访问生产库）：
```sql
SELECT r.id,r.canonical_pr_number,r.stage,r.status,r.reached_ceiling,
       r.failure_code,r.failure_signature,r.last_observed_app_version,
       o.id AS objective_id,o.kind,o.status AS objective_status,
       o.decision_type,o.failure_code AS objective_failure,o.reached_acceptance
FROM delivery_runs r LEFT JOIN objectives o ON o.id=r.objective_id
WHERE r.canonical_pr_number IN (573,591,616,635,638,639)
ORDER BY r.canonical_pr_number,r.created_at;
```
远端确认已合并的 delivery-only 记录应 reached_ceiling=merged / status=completed / stage=complete；objective completed / complete / merged，不再残留身份冲突 failure_signature。live/through_release 仍等真实验收。离线保持原状，联网后再观察。会话内容不变化。

Spec Amendment：无。剩余：新版安装重启的真机核对由独立发布任务完成。
