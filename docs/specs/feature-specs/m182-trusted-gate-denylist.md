# M182（架构修复，紧急）：信任模式本地 git 审批从「白名单语法证明」改为「危险清单 + 工作区边界 + 自动快照」

## 为什么（同族第 6 次）
M159→M166→M172→M177→M180 每次往 `trusted_git::prove` 的语法白名单加写法，模型换一种写法就再次弹审批。v1.91.41 实测（会话 954eb4a1，trusted，本会话托管工作区）：
`git fetch --prune origin main && git merge origin/main --no-edit 2>&1 | tail -25; git status --short | head -20`
→ 进入审批。原因（编排方读 origin/main 代码确认）：`permission_gateway::authorize` 中 `trusted_git::sensitive` 为真（merge 在 DESTRUCTIVE 列表）→ 必须 `backup_registered → prove` 成功；`prove` 只接受逐字匹配的参数形状（如 `["merge","origin/main","--no-edit"]`），`2>&1` 进入 argv 不匹配，且管道后只认 `head` 不认 `tail` → None → request_permission。
## 用户决定（M130，10-20）
信任模式：本地操作随写权限放行；远端动作看会话级「允许交付」+ 一键批准。
## 需求
- R1 信任模式（full_access）+ cwd/所有路径参数都在本会话登记的托管工作区内 → 对 git 本地改写（merge/rebase 非交互/reset/restore/checkout/stash/clean/cherry-pick/revert/am/apply/worktree add/remove 本会话工作区）**先执行现有 snapshot 备份，再 Allow**；不再要求参数语法逐字匹配。重定向（2>&1、> 工作区内文件）、管道到只读过滤器（head/tail/grep/sed -n/wc/sort/uniq/cat）均不影响判定。
- R2 仍需审批/拒绝的危险清单（显式枚举、有测试）：任何 push（含 --force/--delete/refspec 到非本会话分支）、`gh` 写操作（pr merge/close、release、api 写、ruleset、auth token）、路径越出本会话工作区（含 `..`、绝对路径、symlink 逃逸、`-C` 指向他处）、读取凭据/密钥文件、`git config --global`、交互式命令、`git clean -x` / 删除工作区根、snapshot 失败时。
- R3 无法解析（不可静态确定的变量/命令替换）→ 仍走审批（保守）；但普通 `;`/`&&`/`||`/`|`/重定向必须能解析。
- R4 删掉/收敛 `prove` 中逐字参数白名单（或仅保留为「额外可证明」快速路径），判定以 R1/R2 为准；保留快照作为安全网（refs/codefactory/backup/*）。
- R5 数据驱动回归集：10-23~10-26 所有真实误拦命令原文（本条；`git fetch … && git stash push -u -m x && git merge origin/main && git stash pop`；`git checkout origin/main -- <path> && git diff --stat && git status --short && sed -n …`；`git add -A && git commit -m …`；`git reset -q --hard HEAD && git status --short && git merge --no-edit origin/main 2>&1 | tail -5`；`S="<ws>"; A="<ws2>"; git -C "$S" worktree remove --force "$A"; git -C "$S" checkout <branch>`）→ Allow；R2 每类至少一个反例 → Ask/Deny。
- R6 端到端：用 dispatch 起 trusted 交付会话，让其执行 R5 的 Allow 集合，断言 list_approvals 为空，附证据；Windows CI 同跑解析测试（反斜杠路径、引号）。
## 约束
- task-template；合成 fixture；授权交付（ceiling: merged）。

Release-Urgency: immediate

## Implementation Notes

- 规格来源：`/Users/leo/.codefactory/agent-tools/task-m182-trusted-gate-denylist.md`，以上正文逐字保存。
- Applicable Harnesses：Spec、Compatibility、Payload、Observation、Release（仅合并交接，不发版）、AI Collaboration。无 UI 修改，不启用 Viewport。
- R1/R3/R4/R5：`m182_incidents_and_generated_variants_snapshot_before_allow`，复用 M159/M166 注册工作区与内容恢复回归。
- R2：`m182_danger_classes_stay_unproved`、`m182_symlink_escape_stays_unproved`、`m182_snapshot_failure_never_allows`。
- R6：dispatch 验收须明确区分当前安装版与候选实现；真实安装版验收由编排方发版后完成，不把旧版本 dispatch 成功视为候选实现验证。
