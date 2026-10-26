# M180 交付验收记录

## 需求与证据
- R1：事故原文在生产 `trusted_git::sensitive` 入口 RED：8 个分号片段中仅 `git worktree list` 为 sensitive=true；sed、grep、管道及 log 均为 false。测试 `m180_incident_segments_do_not_enter_snapshot_approval` 输出逐段判定。真实 `DesktopPermissionGateway::authorize` 亦先失败（TimedOut）。
- R2：`m159_gateway_session_identity_backup_and_remote_fence` 增加全部六类误拦命令，逐条断言 Allow 且 pending_permissions 为空。第一处修复后又 RED 于 checkout + sed 尾段；补充严格 print-only sed 证明。数据集会产生多个快照，因此原单引用断言改为最新快照，仍断言未提交内容可恢复。
- R3：生产网关测试保留强推、越界 checkout、越界删除反例，新增 gh auth token / gh pr merge。shell Ask 不再被交付授权跳过；永久 deny 保持不变。`m180_worktree_queries_and_mutations_remain_distinct` 验证 worktree remove/prune/add/move/lock 仍敏感。
- R4：实际 dispatch list_approvals 返回空，但未执行全部 R2，因此不是 R4 完成证据。依模板 §8，真实安装版验收由编排方发版后完成。此项明确保留未验收，不用本地集成测试冒充。
- R5：原有 full_access 对无高危标记、非敏感 Git 的未知本地命令默认 Allow 保持不变；生产网关加入 node 算术命令验证。敏感改写仍保留 M159 快照边界。

## Harness 与协作
Spec：原文规格 m180-readonly-still-prompts.md。
Compatibility / Payload / Observation：生产 authorize、SQLite 会话与工作区身份、快照恢复及权限结果断言。
Release：本次仅授权合并，not live，不发版。
AI Collaboration：context scope 为 trusted_git、permission_gateway 与 M159/M177 回归；assumptions 为敏感识别和证明语法缺口，已由 RED 确认；review point 为 worktree list、print-only sed 及直接 merge 安全例外；validation result 见本地检查与 CI。模板禁止 delegate_tasks，采用单执行流。

## 安装版交接
真机验收由编排方发版后完成：在合成仓库中 dispatch create_and_send trusted 会话，逐条执行 R2 全部命令，核对返回结果、工作区状态和快照；断言该会话 list_approvals 为空。另建 R3 反例会话，核对审批或永久 deny，切勿批准危险操作。

Scenario-Test: UI-009
README-Update: reviewed
README-Update-Reason: 修复既有 trusted 权限承诺，无新增安装、隐私或平台承诺。

Release-Urgency: immediate
