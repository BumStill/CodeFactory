## M184 修复

唤醒前检查已结论 PR 与已删除工作区，直接记录系统结算；分裂工作区缺失时复用 M151 重建；repo 锁复用进程实例身份检查；失败投递使用持久指数退避。

### Req ↔ 验证
- R1：m184_missing_split_worktrees_are_rebuilt；m184_terminal_missing_workspace_settles_without_git。
- R2：m184_failed_wakes_do_not_hot_loop（先失败：60 次，预期 1 次）。
- R3：m184_dead_process_lock_is_reclaimed_immediately（真实子进程退出后，先失败）。
- R4：合成 Git/SQLite fixture 与真实子进程；完整 app 重启结算仍需补充，不将单测冒充完整跨进程验收。
- R5：真机验收由编排方发版后完成：核对任务列出的 14 个存量会话 ended，wakeup_schedules 无过期 pending；观察一分钟内不再连续出现 split recovery 告警。

### Harness / AI Collaboration
Spec：docs/specs/feature-specs/m184-wake-split-recovery-loop.md。
Compatibility：SQLite 追加表、现有进程身份 API、M151 Git worktree 重建。
Observation：60 秒起指数退避，最多 3 次。
Release：仅队列合并，不触发发版；not live。
AI Collaboration：context scope 为任务文件、模板及恢复/投递路径；assumptions 为持久 PR 终态观察可信；review point 为不重新宣称 §8 文件清理；单执行流遵循模板不调用 delegate_tasks；validation result 记录于测试结果。

前端 pnpm test：159 文件、1056 项通过；pnpm build 通过。Rust 测试进行中。

Spec Amendment: 无。
README-Update: reviewed
README-Update-Reason: 修复既有恢复与唤醒行为，不改变公开功能或安装承诺。
Scenario-Test: CHAT-001

Release-Urgency: immediate
