# M114b：仍有一条观察路径每分钟重查已关闭/已合并的 PR（#680/#704/#710/#644）

## 现象（v1.88.25 起，M114 #730 已上线）
- 60s 进程采样：CodeFactory 主进程仍对 #644、#680、#704、#710 执行 `gh pr view <N> --json state,headRefOid,mergeCommit,autoMergeRequest`，其中 #680、#710 一分钟各 4 次；#704、#710 已关闭，#680 已合并。M114 修了 reconcile_parked_delivery_progress，但另有路径（疑似 delivery supervisor / takeover 观察对 waiting_core_input、agent_action_required 等状态的交付记录轮询）未纳入「已结论 / 退避」。
- 对应 delivery_runs（编排方核对）：680 mutation_permit|waiting|delivery_external_state_uncertain（objective waiting_core_input）；710 base_sync|agent_action_required；704 takeover_reconciliation|platform_incident。

## 需求

### Requirements Traceability

| Req ID | requirement | minimum evidence |
| --- | --- | --- |
| R1 | 用计数器/日志找出这条路径并写清。 | 调用链说明 + 计数器接入测试 |
| R2 | 纳入 M114 的同一套「已结论 + 指数退避 + 批量」机制：PR 已关闭/已合并 → 写结论并停止；objective 已终态或等待用户输入 → 不做高频轮询（≥30 分钟或仅在会话活动时）。 | 60 tick 集成测试断言历史记录只被查询一次 |
| R3 | 测试（先失败）：上述四种状态的历史交付记录在 60 个 tick 内的 gh 调用数有上界；关闭/合并的 PR 首次结论后不再查询。 | 先失败的测试记录 + 通过后的同一命令输出 |
| R4 | 真机验收由编排方做：装好后 60s 采样不再出现这 4 个 PR。 | 发布后由编排方现场采样 |

## R1 结论：残留路径

两条路径同时存在，缺一条都还会重查：

1. **恢复 supervisor 的认领前观察**
   `spawn_delivery_recovery_supervisor`（15s）→ `plan_startup_recovery` 每轮重新认领同一批过期租约的
   `delivery_runs`（`status IN ('running','waiting','platform_incident','agent_action_required',…)`）
   → `resume_claimed_delivery_with_remote` 开头就对 `canonical_pr_number` 调 `observe_merge`
   （`gh pr view`）。观察到 closed/unknown 后只释放租约并 `return`，**不写任何结论或下次观测时间**，
   于是下一个 15s tick 原样再来一遍 —— 现场即每分钟 4 次。
2. **reconcile 候选集漏掉「等用户」状态**
   `reconcile_parked_delivery_progress` 的候选只含 `running/active/waiting_system/cancelled/failed`
   （外加 completed+broken），`waiting_core_input` 完全不在候选里，因此 #680（merged）与
   #710（closed）永远得不到 `pr_observation_concluded_at`。而且即使进入候选，两处
   `continue` 也会把观测结果丢掉：run 的 stage/status 幂等栅栏与 objective 更新失败分支
   都是直接 `continue`，不写 `pr_observation_next_at`。

`#644` 属于同一批历史记录，随第一条路径修复一起消除。

## 修复

- `resume_claimed_delivery_with_remote`：观测前先读 `pr_observation_concluded_at`，已结论则直接释放租约返回，不再查询；
  merged / closed 写入结论（`conclude=true`），open / head 已解释 / unknown 写入退避时间。
- `plan_startup_recovery`：已结论（`pr_observation_concluded_at IS NOT NULL`）的运行不再进入认领候选，
  避免 15s 空转认领。
- `reconcile_parked_delivery_progress`：候选集加入 `waiting_core_input`；两处原本丢弃观测的 `continue`
  改为先持久化「结论或下次观测时间」。
- 新口径 `USER_WAIT_OBSERVATION_MIN_DELAY_MS = 30 分钟`：objective 处于 `waiting_core_input` 或
  `requires_user_action=1` 时，`next_observation_at` 至少 30 分钟后；对这类记录，closed PR 也直接结论。

## Implementation Notes

- 观察下限由 `ObservationState.min_delay_ms` 携带（从 `objectives.status` / `requires_user_action`
  持久读），cleanup pass 与 recovery supervisor 共用同一判断，不存在第二套策略。
- 语义不变：unknown / closed 仍然不算成功、不授权删除工作区；本次只决定「什么时候还可以再问」。
- 迁移仍是 M114 的 `ALTER TABLE` 可空列路径，旧库可读；列不存在时相关判断退化为原行为。

## Applicable Harnesses
- Spec Harness（必须）：本规格 + Requirements Traceability + 测试矩阵。
- Compatibility Harness：旧库列缺失时行为退化，不做破坏性迁移。
- Observation Harness：额度/暂停与用户等待状态都必须可见，不静默。
- AI Collaboration Harness：多角色协作假设记录在 PR。

## 测试矩阵

| 用例 | 覆盖 Req | 类型 |
| --- | --- | --- |
| 4 种现场状态 ×60 tick 的观测调用上界 | R1 R2 R3 | Rust 集成 |
| merged/closed PR 首次结论后不再被查询 | R2 R3 | Rust 集成 |
| 等用户 objective 的 ≥30 分钟观测下限 | R2 | Rust 单测 |
| 已结论运行不再进入 recovery 认领候选 | R2 | Rust 集成（既有 startup recovery 套件） |

## 约束
- 合成 fixture，不写生产 DB。
- 真机验收（装好后 60s 采样无这 4 个 PR）由编排方在发版后完成。
