# M181：只读任务对 git 子命令的读/写判定颠倒

## 证据（v1.91.41，10-26 22:2x 编排方真机验证会话 2d8c5086，trusted，非交付的只读任务）
- `sed -n '1,20p' README.md; grep …; git worktree list | head -3; git log -1 --oneline; git log -1 --oneline origin/main` → denied：「当前是只读任务，已跳过片段 `git worktree list`：Git 或 GitHub 操作会写入状态」。`git worktree list` 是纯读。
- 同会话 `git fetch --prune origin main && git merge-base …` 却被放行（fetch 会写远端跟踪引用）。
## 需求
- R1 只读任务里按 git 子命令真实语义判定：纯读（status/log/show/diff/rev-parse/merge-base/branch --list/worktree list/ls-files/blame/describe/remote -v/config --get）放行；写本地状态（fetch/commit/checkout/reset/stash/merge/rebase/worktree add/remove/config 写）在只读任务拒绝并给人话原因；gh 只读子命令（pr view/list/checks、run list/view、auth status）放行。
- R2 拒绝文案不出现「不要要求用户重复确认」这类内部提示语。
- R3 测试（先失败）：上述读命令在只读任务放行；fetch/commit/worktree add 在只读任务拒绝；信任+实施姿态下不受影响。
## 约束
- task-template；合成 fixture；授权交付（ceiling: merged）。

Release-Urgency: default

## Implementation Notes

- 路由：bash → capability_denial(ReviewOnly) → review_command_mutation；Implement/Deliver 仍交给权限网关，不在结构准入层增加限制。
- Requirements Traceability：R1/R3 对应 `m181_readonly_git_queries`、`m181_readonly_git_writes`、`m181_implementation_keeps_permission_gateway_authority`；R2 对应 `m181_denial_is_user_facing` 及既有拒绝回归测试。
- Applicable Harnesses：Spec、Compatibility、Payload、Observation、Release（仅合并交接，不发版）、AI Collaboration。
- 测试矩阵：纯读 git/gh；git 本地写；复合命令/管道；全局参数；config 混合读写标志；实施/交付姿态。
- 无 UI 或持久化 schema 变更；bash 即锁屏可用入口。真机验收由编排方发版安装后完成：trusted 只读会话运行 git worktree list、git log 与 gh pr view，确认放行；运行 fetch/commit/worktree add 确认拒绝且无内部提示；实施会话确认按原权限规则执行。
