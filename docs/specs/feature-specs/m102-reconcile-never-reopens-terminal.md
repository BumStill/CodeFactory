# M102：PR 进度对账不得把已结束的任务改回「等待中」

## 现象（真机 2026-10-17 19:40，v1.88.14）
- 会话 3b87a1b6（#688 已合并）与 f8cc1607（#693 已合并）：经派单入口 close_objective 验收成功，objective_events 第 266 版记 `acceptance_confirmed → completed`；紧接着第 267 版被改回 `waiting_system / delivered_awaiting_ci`，没有任何事件记录。两者在装 1.88.14 之前是 failed，也被同一路径「复活」成 waiting_system。
- 会话 77b8b247（#689 已合并）：delivery_runs 仍停在 live/waiting，close_objective 报 `only a merged task awaiting acceptance can be closed`——PR 已合并但对账没认出来。
- 后果：任务永远结不了案，侧栏持续转圈（M84 家族），用户验收被静默推翻。

## 代码线索
- `src-tauri/src/agent/execution_workspace.rs` 约 1300–1360，canonical PR progress 对账：
  `UPDATE objectives SET … WHERE id=? AND status IN ('running','waiting_system','cancelled','failed','completed')…`
  `complete` 只在 kind ∈ {delivery, local_mutation} 且 requested_ceiling ∈ {pr_only, through_ci_green, through_merge} 时为真，否则写 `waiting_system / delivered_awaiting_ci`。
- 用 git log -S 找出把 'completed'/'failed'/'cancelled' 加进这个 status 列表的提交，写清它当时想解决什么（可能是 #676 / #692 / #693 之一），新修复不能把那个场景再弄坏。

## 需求
- R1 先用数据写清这三个会话 19:3x–19:41 的时间线与根因（含为何 complete=false：实际的 kind、requested_ceiling 值）。
- R2 已 completed（含显式验收）的 objective，任何对账都不得改回非终态；只允许追加证据。
- R3 failed/cancelled 的 objective 在其 PR 实际合并后：可以前进到「已合并、等验收」或直接 completed（按规格选择并说明），但不得退回「等 CI」；PR 未合并时保持原终态，不得复活。
- R4 complete 判定覆盖派单入口实际产生的 kind 与 requested_ceiling 组合（如 ceiling=merged 的 chat 任务），合并即可进入「已合并、等验收」。
- R5 77b8b247 这类 delivery 停在 live/waiting 但 PR 已合并：对账能识别并推进。
- R6 每次改 objective 状态都必须写 objective_events（不允许无事件改状态）。
- R7 测试（先失败）：completed 后对账 → 仍 completed；failed+PR 已合并 → 前进不倒退；failed+PR 未合并 → 保持 failed；ceiling=merged 的会话合并 → 可验收。

## 约束
- 合成 fixture，不写生产 DB。

## Implementation Notes
- 保留既有 delivery/local_mutation 合并后 completed（UI/派单层仍要求显式验收）的语义，以 objective 的 requested_acceptance 为完成目标，不能把交付工具默认 through_release 当成任务必须发版。
- completed 保持 revision、验收信息及终态不变；failed/cancelled 仅在正向合并证据且满足 objective 合并目标时前进。不足以完成的 live/release 目标保留原终态。
- 仅修复历史 delivery 错位到 release/live 的 run；真实 live/release objective 不因合并完成。不修改数据库 schema、前端或派单授权。
- Applicable Harnesses：Spec（R1–R7）、Compatibility（历史终态/旧最小 schema）、Observation（同事务事件）、Release（仅交付不发版）、AI Collaboration（单执行流，模板禁止 delegate_tasks）。
- 验收矩阵：completed × open/merged；failed/cancelled × open/merged；delivery merged 目标 × through_merge/through_release × pr_open/release_triggered；显式 live 目标不得提前完成；重复对账幂等；既有 M72/M74/M84 回归。
- 真机验收由编排方发版后完成：重新打开三类历史任务，核对侧栏终态、close_objective、objective_events 版本及重复轮询不回退。

## R1 只读现场证据（新加坡时间）
- #688：17:28:57 failed revision 4；19:40:39 acceptance_confirmed completed revision 266；随后无 objective_events 的 SQL 持续加 revision，取证时 286 waiting_system。kind=delivery、requested_acceptance=merged、run requested_ceiling=through_release、reached=merged、stage=complete、status=waiting。
- #693：18:42:04 failed revision 3；19:40:51 acceptance_confirmed completed **revision 56**（原任务描述的 266 是 #688 的值）；取证时 76 waiting_system。kind=delivery、requested_acceptance=merged、run requested_ceiling=through_release、reached=merged、stage=complete、status=waiting。
- #689：17:10:02 最后 objective 决策为 waiting_system revision 3；取证时仍 revision 3。kind=delivery、requested_acceptance=merged、run requested_ceiling=through_release、reached=release_triggered、stage=live、status=waiting。run UPDATE 排除了 release_triggered，fallback 又只允许 reached=merged，故没有认领合并结果。
- 三者 complete=false 的直接原因均为 through_release 不在固定 ceiling 白名单，不是 kind=chat；19:3x–19:41 的静默状态变更没有 objective_events，不能伪造其准确时刻，后续用 delivery_run_events 的聚合时间补证。
- `git log -S "'cancelled','failed','completed'"` 指向 2dc3fdaf（#641，M74）：为远端实际已合并、但本地 takeover 因身份冲突失败的历史记录恢复真实状态。修复必须继续通过 M74 的 failed/cancelled、停机 fence、失效工作区与重复对账测试，不能简单删除所有终态候选。
