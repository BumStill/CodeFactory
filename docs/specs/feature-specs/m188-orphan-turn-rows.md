# M188（高）：孤儿 chat_turn_state 行被当作「活跃工作」→ 已合并会话被强置 running（幻影「正在做」）

## 证据（v1.91.50，含 M187 #903；编排方读代码 + 查库）
- M187 在 `thin_derive.rs` 新增「活跃工作」判定：`chat_turn_state WHERE status IN ('active','working','running') AND terminal_reason IS NULL AND completed_at IS NULL` 计数 >0 即拒绝结算并 `thin_state::save(..., ThinState::running())`（diff 第 147–148 行附近）。
- 14 个 PR 已合并的存量会话装 v1.91.50 后全部变为 thin=running、reason 空、零模型调用（10:30 复查仍如此）。逐个查：c8e12ca7 有 1 条 `phase=working,status=active`（2026-10-26 13:52 遗留）；dd5ed203 有 18 条（10-24 00:30 起）；98b26375 有 8 条——全部是早已退出的进程遗留的孤儿回合行，没有任何活 executor / run lease。
- 结果：这些会话在界面上永久「正在做」（与用户 Windows 上的卡死症状同族），M184/M186/M187 三轮都未能清掉存量。
## 需求
- R1 「活跃工作」判定必须基于**活的执行事实**：当前进程实例持有的 run lease / executor 注册、正在执行的工具调用（且属于当前进程实例）、运行中的后台任务（进程存活）。数据库里的 turn 行本身不构成活跃证据。
- R2 启动对账：属于已退出进程实例（或无 run lease）的 `chat_turn_state` 非终态行一律原子结算为终态（terminal_reason=`orphaned_by_restart`），不得留存；之后再做会话状态派生。
- R3 不变量：thin=running ⇒ 存在活 executor；否则派生为 waiting_wake（有唤醒源）/waiting_user（给原因）/ended（PR 已合并且空闲）。加周期检查与告警。
- R4 测试（先失败，跨进程）：构造「PR 已合并 + 多条孤儿 active 回合行 + 无 executor」→ 启动后 ≤1 分钟 ended；「真实运行中 bash」→ 不被结算（保持 M187 的保护）；孤儿行被结算为 orphaned_by_restart。
- R5 真机验收：装新版后，编排方列出的 14 个会话全部 ended；`select count(*) from thin_session_state where state='running'` 只剩真实在跑的会话；verify-liveness 仍通过。
## 约束
- task-template；合成 fixture；授权交付（ceiling: merged）；PR 必须为非 draft 再 armed。

Release-Urgency: immediate

## Implementation Notes
- Requirements Traceability：R1/R2/R3/R4 由 `m188_cross_process_orphans_settle_and_live_bash_survives` 及状态派生回归验证；R5 由编排方发版安装后验收。
- Applicable Harnesses：Spec、Compatibility（旧 SQLite 行）、Observation（周期不变量告警）、Release（仅合并，发布另行授权）、AI Collaboration（限定状态恢复上下文）。无 UI 布局改动。
- 测试矩阵：退出进程写入多条 active 回合、陈旧 pending 工具、running 后台行；新进程启动对账必须幂等结算；真实 bash + 当前进程 executor 在执行时不可结束，完成后正常结算；无执行者 running 缓存必须降级。
- context scope：仅 thin 状态派生、启动恢复、后台任务身份判定。assumptions：持久行不是活性证明，进程注册与 PID 启动标记才是。review point：启动对账必须早于恢复投递；validation result 记入 PR。
- 真实应用验收由编排方发版安装后完成：核对 14 会话 ended，逐项对照 running 会话与活 executor，执行 verify-liveness（重启前 running，bash 完成后输出答案）。本任务不读取或修改生产数据库。
