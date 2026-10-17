# M100：提交回执家族收敛

## 用户任务原文

# M100 家族审查：本地提交回执与「提交后又改代码」——报错、提交被撤回、被迫 git reset --hard

## 现象（真机 2026-10-17，第 3 次以上）
1. a1ce7c0b 17:43:39 `agent_loop_error`：`local commit receipt cannot absorb source edits made after the commit`，随后推送后被判 `remote branch moved to an unrecognized head`（自己推的）→ 后续被系统误取消（M99，已修）。
2. 0c4deae0 18:1x：工作区 HEAD 被退回父提交，原提交 31cc46fe 的内容变成未提交改动（逐字节一致）；agent 只能请求 `git reset --hard 31cc46fe` 找回，触发审批（信任模式仍需批准）。
3. 402ca0b7 21:3x（v1.88.15）：再次 `local commit receipt cannot absorb source edits made after the commit`（failure_signature sha256:4ac4893a…）。
- 正常开发流程「提交 → 跑测试发现问题 → 再改 → 再交付」反复撞上这条路径。

## 取证起点（只读，先数据后代码）
- 本机 DB：三个会话对应时间段的 delivery_runs / delivery_run_events / objective_events / tool_calls（deliver_changes 参数与结果）。
- 代码：搜 `cannot absorb source edits made after the commit` 的出处，以及任何会把 HEAD 回退而保留工作树的路径（git reset --soft / --mixed、提交回滚、receipt 对账）。

## 需求
- R1 用数据写清三次的时间线与根因，写入规格。
- R2 已有本地提交回执后又有源码修改：交付工具应生成新的提交（或追加提交）并更新回执，然后继续，不报错、不要求人工。
- R3 不得在 agent 不知情时回退 HEAD；若确需回滚，必须是显式、可见、可恢复的操作并写事件。
- R4 agent 自己工作区内恢复到自己的已知提交（如 0c4deae0 场景）应有不需要审批的安全路径（或由交付工具内部完成），不要逼 agent 用 `git reset --hard`。
- R5 测试（先失败）：提交→改→再交付；提交→改→改→再交付；交付中途进程重启后再改再交付。断言无报错、无 HEAD 回退、远端 head 被识别为自己的。
- R6 家族收敛：盘点提交回执的生成、吸收、对账路径，文档化不变量。

## 约束
- 不放宽外部写入安全边界；合成 fixture，不写生产 DB。

## Implementation Notes：只读取证（新加坡时间，2026-10-17）

生产 SQLite 通过 `mode=ro` 查询，仅记录匿名聚合形状，不复制消息或工具参数。

| 案例 | 数据时间线 | 根因/证据边界 |
| --- | --- | --- |
| A | 17:41:07 本地提交回执已 materialized；17:41:08 push 开始并成功；PR body 被拒；17:43:00、17:44:12 takeover 报 source edits；17:45:57 identity conflict | 已完成提交对象的证明错误绑定后续 worktree manifest；不是旧提交对象损坏。远端自身提交误识别路径已有 M99 回归。 |
| B | 18:14:04 工具 bash 显式执行 mixed reset；18:14:25 工具请求 hard reset 恢复；18:17:09 又执行 mixed reset。现存 delivery run 18:39:55 才创建，18:41:42 CI 失败，18:42:12 isolated-index identity 检查失败 | 现存证据表明早期 HEAD 后退来自 agent 的 bash，而非 delivery 内部 reset。早期记录受到历史清理命令影响，不能据现存 run 重构已删除事件，也不能声称系统静默撤销提交。 |
| C | 21:11:07 本地提交 materialized；21:11:09 push；21:11:15 PR body 被拒；21:11:37 identity conflict；21:12:18 source edits 对账失败 | 与 A 相同：提交事实与之后的编辑错误耦合，迫使 agent 清理/隐藏合法修改。 |

## Requirements Traceability / 验证矩阵

| Req ID | 最低证据 |
| --- | --- |
| R1 | 上述只读事件与 tool audit 时间线，明确区分观察与推断 |
| R2 | 真实临时 Git 中旧回执 + 新修改，受控追加提交，新的精确 SHA/父提交/远端 head |
| R3 | 观察/重复对账前后 HEAD、index 和源码逐字节不变；不引入 reset |
| R4 | 已知 receipt 在 pre-ref crash 时由带 fence 的 CAS 前向恢复；外来 HEAD/index/源码漂移仍拒绝 |
| R5 | 一次编辑、多次编辑、跨 owner/claim 恢复测试，含远端确认 |
| R6 | 以下回执路径和不变量 |

## Applicable Harnesses

Spec、Compatibility、Observation、Release（仅合并交接，不发版）、AI Collaboration；本次不修改 UI。生产安装版的真实应用验收由编排方发版后完成：提交后改文件、再次交付、重启后再改再交付，核对同一 PR、追加提交、审计事件和无审批/无 HEAD 后退。

## 回执家族与不变量

- `prepare_scoped_commit` 用隔离 index 规划，`LocalCommitIntentEvidence` 绑定 repo/worktree/branch、精确父提交、tree、message、目标 SHA。
- `materialize_receipted_local_commit` 只能在原始 parent 上按精确 index/source 证据前向 CAS；不得回退 HEAD，不得吸收外来 index。已在目标 SHA 时不应重放已完成的 index 事务。
- `observe_receipted_local_commit` 只证明不可变提交对象，不把之后 worktree/index 的合法编辑纳入旧 receipt，更不能在观察中提交或重写 index。
- `reconcile_receipted_local_commit_head` 用持久 identity revision 记录确切 child；已绑定 child 的回执不应再拿旧 parent 去证明新一轮修改。
- `reconcile_unresolved_delivery_mutation_intents` 正向确认旧提交副作用；后续编辑由正常 mutation-permit 提交链签发独立 receipt。
- 外来 SHA、错误 repo/worktree、错误 tree/message、pre-ref 漂移继续 fail closed；已有 R4 fenced CAS 是恢复自己的已知提交的安全通道，不新增通用 hard-reset 免审批。

## AI Collaboration

Context scope：delivery engine / tool adapter / durable store、合成 Git/SQLite fixtures。Assumption：提交后的编辑不改变已落地提交对象。Review point：旧回执只确认旧副作用；新源码只在 mutation permit 下提交。Validation result：见 PR 验收记录。任务模板要求单执行流，未调用 delegate_tasks。
