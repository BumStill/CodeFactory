## 修复内容
修复 M226 将 legacy_orphan 历史目标误当成当前任务、重推历史 PR 导致已结束会话复活的问题。启动批量结算直接使用持久会话 ID，不依赖前端最近 200 条列表。

Scenario-Test: HLT-001, HLT-004
README-Update: reviewed
README-Update-Reason: 内部状态兼容修复，不改变公开产品、安装、隐私或平台承诺。

## Req ↔ 测试与自验
- R1：m230_legacy_orphan_is_not_current_work_even_when_newer_than_merge；已先在行为断言看到失败。
- R2：m230_completed_newer_objective_is_not_active_but_queued_input_is；已先失败；保留 M226 活目标、续接嵌套 worktree 与 M211 回归。
- R3：m230_ended_batch_restart_is_a_noop_but_new_input_resumes；已先失败；断言批量历史投影逐字段不变、无新唤醒，新输入可恢复。
- R4/R5：m230_startup_repairs_unloaded_history_without_erasing_records；已先失败；断言真实合并文案、无 pending 唤醒、无新回合，消息/objective/工作区记录保留，重复执行幂等。
- 首次夹具错误（唤醒表初始化、取消状态约束）已修正；随后原命令重跑，四项均在行为断言失败后才改实现。

## Harness
- Spec：原始任务规格入库 docs/specs/feature-specs/m230-history-session-revival.md。
- Compatibility / Observation：真实 SQLite 迁移夹具、批量重推、启动结算、状态与唤醒字段断言。
- Release：本单仅合并，Release-Urgency: immediate；发版另单，当前 not live。
- AI Collaboration：context scope 为 thin 派生、结算和 dispatch 列表；assumptions 为 legacy_orphan 不表示活任务；review point 为 ended 新事实边界及清理前守卫；validation result 见测试命令。模板要求单执行流，未启用 sub-agent。
- 无 UI/布局改动，不适用 Viewport；不以浏览器显示替代 SQLite 行为证据。

## 六个 unknown session 根因
只读生产库核对：六个均为 parent_session_id=NULL 的 project 会话，但分别已有 202/203/204/205/206/214 个更新顶层会话。commands/session.rs list_sessions 按 updated_at DESC LIMIT 200；dispatchActions.ts requireSession 只查 activeSession/store.sessions，所以落在列表外即报 unknown session。启动结算扫描 thin_session_state 持久 ID，覆盖列表外会话；本单不改 dispatch API 范围。

## 真机验收交接
真机验收由编排方发版后完成：安装新版重启，核对 thin_session_state 状态计数、六个历史会话 ended、wakeup_schedules 无历史 pending「继续当前任务」；模型恢复后对比这些会话的 chat_turn_state / model_route_attempts 无新增；用 @spec: 续接验证上一单合并不会结束后一单。78 个真实 stop 的结束文案不改写。不修改用户数据库。

## 验证状态
- pnpm test && pnpm build：163 个文件、1089 项测试通过（包括 lightModeAudit），生产构建通过。
- M230：4 项先红后绿；Rust workspace 全量测试运行中。
- validate_repo_governance_baseline.py、run_scenario_harness_gate.py --stage local、validate_scenario_test_governance.py --ci --body-file、validate_readme_contract.py：通过。

## Spec Amendment
无。

Release-Urgency: immediate
