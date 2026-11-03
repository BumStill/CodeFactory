## M209：恢复重新打开会话的定时唤醒

Scenario-Test: HLT-001, HLT-004, RTE-003, RTE-005
README-Update: reviewed
README-Update-Reason: 修复既有后台唤醒与会话恢复，不改变公开功能、安装、隐私或平台承诺。

### 根因与 Req 映射
- R1/R2：调度器按 status/next_at 扫描，无 id/fire_count 去重过滤。旧工作区预算先否决投递，但 unanswered_user 又禁止 reject_exhausted_wakes 结束任务，形成永久 pending。合成夹具 fire_count=0/1/4 同时覆盖。
- R1/R2：M146 将未产生助手回合的定时唤醒投影为 ended，错误地把缺失执行当成完成；M209 改为持久自动接续，不伪造 running。
- R3：投递失败使用独立准入退避，不让旧任务工作区结论阻断未回复任务；扫描仍按所有 fire_count 处理并保留审计。
- R4：arm_durable_wake 与 derive_state 新建 UUID 唤醒，保留旧触发记录；已有 pending 时不叠加。
- 先红：m209_reused_pending_repair_ignores_stale_workspace_budget（0 次投递）；m209_unanswered_system_wake_is_recovered_not_ended（Ended）；m209_reopened_repair_uses_fresh_identity（旧行被改为 pending）。
- 真实准入补充：m209_system_wake_admits_a_fresh_turn_after_settlement 检查旧回合完成后生成不同 root、新 active turn 与持久系统来源标记。

### Harness / AI Collaboration
- Spec：docs/specs/feature-specs/m209-reopened-session-wake.md。
- Compatibility：历史 SQLite pending 与已触发 id、fire_count=0/1/4；不更改数据库既有字段。
- Observation：SQL 状态、计数、旧回执不可变、新回合 root 与 active 状态字段断言。
- Release：仅授权交付合并，Release-Urgency: immediate 保留；不擅自发布。
- AI Collaboration：context scope 为定时调度、thin 派生、恢复预算及真实 chat admission；assumptions 为纯 synthetic 数据、后台可操作、不碰生产数据库；review point 为预算与终态保护交互、新 root 身份、回归测试；validation result 待最终验证更新。按模板单执行流，未使用 delegate_tasks。
- 无布局改动，不适用 Viewport/截图；沿用持久后台调度及 dispatch 入口。

### 验证与自验
- 先红证据：三个新用例在修复前分别红在「0 次投递」「Ended 而非 WaitingWake」「旧回执被改写」，第四个真实准入用例红在 `system wake has no unfinished objective to continue`。
- 合并最新 origin/main（M203 落地提交）后复验：`cargo test --lib` 2136 passed / 0 failed / 12 ignored；`m209` 定向 4 项全绿。
- 前端全量 `pnpm test && pnpm build`：163 文件、1086 项测试通过，构建成功。
- 治理与场景门禁：`validate_repo_governance_baseline.py` failures 为空；`run_scenario_harness_gate.py --stage local --base-ref origin/main` 输出 OK。
- Spec Amendment：无。

### §8 / 真机验收分工
not live：本任务不发版安装；真机验收由编排方发版后完成：
1. 安装后十分钟内查询 pending 且 next_at < now-600，确认零条。
2. 合成误结束会话经恢复重开，两分钟内核对 assistant 回合，而非仅消息入库。
3. 同时核对 reused id、fire_count>0 及普通新 repair 唤醒，拒绝必须有可读审计与明确结算。
4. 合并后本会话核对 merged=true、main squash SHA 与 Release-Urgency trailer、远端精确分支、干净工作区及专属构建目录清理；不删除托管目录。

Release-Urgency: immediate
