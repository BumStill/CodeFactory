# M166：M159 信任模式免审批要覆盖组合命令（v1.91.27 实测仍弹审批）

## 证据（10-24 00:25，v1.91.27 已含 M159 #849）
会话 dd5ed203（信任模式，授权交付）在**自己登记的托管工作区**执行，仍进入待审批：
`cd "<本会话托管工作区>"; git stash push -m prefill; git fetch --prune origin main; git merge origin/main --no-edit; git stash pop; git status --porcelain | head`
同晚还有：`S=…; A=…; git -C "$S" worktree remove --force "$A"; git -C "$S" checkout <本会话分支>`（两个路径都是本会话登记的工作区）。
## 需求
- R1 M159 的判定对组合命令（`;`/`&&`/`|`/`cd X;`/变量赋值后引用/`git -C <path>`）逐段解析：每一段都落在本会话登记工作区内且属于 M159 允许的本地改写或只读/拉取（fetch/merge/stash/pop/status/log/diff/worktree remove 本会话工作区），则整条放行；任一段越界仍审批。
- R2 变量展开：只解析同一命令内的字面量赋值（如 S="…"），其它不可静态确定的路径一律按越界处理（保守）。
- R3 测试（先失败）：上述两条真实命令 → 放行；把其中一个路径换成用户主仓库/其他会话工作区 → 审批；含 `push --force` 段 → 审批。
## 约束
- task-template；合成 fixture；不削弱 M159 R2 的保护。授权交付（ceiling: merged）。

Release-Urgency: default

## Implementation Notes

- 工作区授权仍来自本会话登记及 Git identity 校验，不从路径前缀或变量名推断。未知 shell 语法、动态展开、重定向、远端写操作默认不自动放行。
- Requirements Traceability：R1/R3 对应合成组合命令、真实 Git 工作树与备份恢复测试；R2 对应未知变量、命令替换、越界和管道作用域反例；M159 原有远端历史与备份测试保留。
- Applicable Harnesses：Spec、Compatibility（标准模式与旧 M159）、Payload（shell 字段级权限判定）、Observation（备份 ref 与审批记录）、AI Collaboration；无 UI 布局变更。Release Harness 以合并交接为界，不执行发版。
- 后台入口继续使用既有 bash/dispatch 权限路由，不新增 GUI 依赖。真机验收由编排方发版后完成：信任模式运行两条合成命令核对无审批、备份 ref 可恢复；替换为主仓库/其他会话路径或 force push 后核对产生审批；标准模式确认行为不变。
- Windows 路径（M166 续，PR #861 首轮 CI 暴露）：Windows 托管工作区路径以反斜杠分隔（`C:\...`），命令里必须加引号。分解器此前把引号内的 `\` 也当转义符拒绝，导致同一组合命令在 Windows 上整条退回审批（`m159_gateway_session_identity_backup_and_remote_fence` 在 Windows 断言失败）。现在引号内的 `\` 按字面量处理，引号外仍拒绝（转义/续行无法静态确认），双引号内 `\"`、`\\`、`\$`、`` \` `` 仍按转义拒绝。该修正不放宽 R1/R2：越界、动态展开、远端写操作的拒绝路径不变。
- 测试可移植性：`m159_trusted_git.rs` 的 shell 执行统一走 `shell()`（Windows 用 Git for Windows 的 `bin/bash.exe`，其它平台用 `sh`），使 R3 的端到端执行在 Windows 上同样被验证。
