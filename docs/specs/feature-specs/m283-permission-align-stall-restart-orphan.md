# M283：权限身份对齐失败与重启救援接续

## 已批准要求

以用户提供的 `~/.codefactory/agent-tools/task-m283-permission-align-stall-and-restart-orphan.md` 为准。不得扩大权限范围或改变 heredoc 安全规则。全部验收通过前，PR #1075 保持草稿，不调用交付工具，不添加 `merge-queue: armed`；仅 git push 保存进度。全部完成后标 ready 并交付，Release-Urgency 默认。

## Requirements Traceability

| ID | 要求 | 实现 | 验收状态 |
| --- | --- | --- | --- |
| R1 | 修复会话切换 objective 后权限身份对齐 | `resolve_scope` 继续以当前 session 的唯一 active objective 为恢复边界；`repair_session_split_locked` 在任何重建、快照、合并、退休前拒绝处理 live executor，并且 settled 回合迁移 SQL 排除 live working 回合 | 完成：Rust 真实 Git/workspace 回归 + SQL 补充回归均绿 |
| R2 | “系统自动重试/自愈”必须有真实重试源 | 权限 admission 保留一次就地重试；重试耗尽后复用 `system_recovery_continuation` 登记即时、幂等的 bounded wake | 完成：`m283_alignment_failure_persists_audit_and_real_wakeup`、`m283_system_recovery_arms_bounded_continuation` |
| R3 | 救援成功且未交付不得保持 `ended/orphaned_by_restart` | `restore_registered_branch` 复用 M282 continuation 判定，并把返回的 `WaitingWake` 投影持久化 | 完成：真实 Git 救援入口先红后绿，断言持久状态与 pending wake |
| R4 | 权限对齐失败可结构化取证且不含凭据 | 写入 `objective_events`：`permission_alignment_failed` / `permission_identity_alignment` / `recovery_owner=system`；错误摘要经 `trajectory::redact_text` 脱敏 | 完成：真实 SQLite 断言事件字段、脱敏内容与恢复源 |
| R5 | 覆盖 objective 切换、失败唤醒、救援非终态、主动停止反例，先红后绿 | Rust 聚焦测试 4 项、既有 M179 objective 切换入口、M161 Git 救援入口、SQL split 补充回归 | 完成；红绿证据见 `docs/long-tasks/m283-resume-evidence.md` |
| R6 | 不放宽权限判定，不改 heredoc 规则 | 未修改权限分类、自动 allow 条件、shell/heredoc 解析；变更仅收紧 split 清理并追加系统恢复 | 完成：diff 审查与既有 UI-009/治理门禁 |

## 根因

现场失败并非单一 ChannelClosed。会话切到新 objective 后存在 split workspace/objective 身份：启动修复会在工作仍为 `working` 且没有 `terminal_reason` 时创建快照、合并树、关闭 donor workspace、改绑回合并退休 donor objective，导致权限 admission 的 exact objective/binding 身份失效。随后 admission 虽就地重试一次，但失败文案承诺自动恢复却没有持久事件或 wake；重启救援成功后又未覆盖既有 `ended/orphaned_by_restart` 投影。

修复采用 fail-closed：live executor 存在时 split 修复在任何文件或持久状态副作用前退出，由既有 bounded recovery budget 重试；settled 回合仍可迁移。权限 admission 不放宽 scope，也不继承旧 allow。

## 场景声明

`Scenario-Test: HLT-001, HLT-004, UI-009, RTE-004, E2E-009`

## 交付边界

- 当前本地实现与自动化证据完成；正式产品仍为 **not live**。
- 编排方发版后另做真机验收：已交付会话的新 objective 第一条工具调用正常执行/审批；运行中升级重启后自动接续且非 ended。
- Release-Urgency 使用默认值，不请求 immediate。
