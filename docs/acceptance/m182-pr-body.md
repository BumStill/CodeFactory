## M182 修复候选

实施规格：docs/specs/feature-specs/m182-trusted-gate-denylist.md
Acceptance-Ledger: docs/acceptance/m182.md

本地 Git 改写引入危险项、路径与重定向检查，保留快照前置条件。尚未完成全部验收，不声明可合并；系统自动继续验证与修复，全部验收及 CI 全绿后按授权加入合并队列。

Req 映射：R1/R3/R4/R5 对应 src-tauri/tests/m182_trusted_git_denylist.rs::m182_incidents_and_generated_variants_snapshot_before_allow；R2 对应 m182_danger_classes_stay_unproved、m182_symlink_escape_stays_unproved、m182_snapshot_failure_never_allows；R5 注册 worktree 对应 src-tauri/tests/m159_trusted_git.rs::m166_r3_registered_worktree_remove_snapshots_and_foreign_fences。R6 候选实现 dispatch/list_approvals 与 Windows CI 尚未闭环。

修复前观察到事故命令回归失败。当前 M182 聚焦 11 项、M159/M166 12 项及 diff 检查通过。完整 Rust 工作区测试已启动，结果未确认；前端完整测试及构建未完成。

Applicable Harnesses: Spec、Compatibility、Payload、Observation、Release、AI Collaboration。无 UI 修改。
AI Collaboration: context scope 为 trusted_git 与权限网关；assumptions 为仅登记工作区可获得本地信任；review point 为 shell 解析与路径边界；validation result 为聚焦回归通过，完整验收未完成。

Spec Amendment: 无已批准修改。账本未完成项须在本 PR 内补齐，不代表用户批准推迟规格。

真机验收由编排方发版后完成：安装新版本，dispatch 创建 trusted 且允许交付的合成会话，执行 R5，核对 list_approvals 为空、快照 refs 和审计；执行 R2 反例核对审批或拒绝。当前 not live，本任务不发版。

Scenario-Test: UI-009
README-Update: reviewed
README-Update-Reason: 修复既有信任模式审批行为，不改变公开功能或安装承诺。

Release-Urgency: immediate
