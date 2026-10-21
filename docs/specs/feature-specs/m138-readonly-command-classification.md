# M138：只读意图下，纯只读的组合命令被判为「变更动作」拒绝（同族第 3 次）

## 证据
- v1.91.10 会话 4a95f646（只读演练）：重启续跑后核对现场的命令 `git status --short; git log -1 --oneline; gh pr view --json number,state 2>/dev/null; ps -eo pid,etime,args | grep '[s]leep 60'` 被拒：「已跳过与当前显式只读意图冲突的 bash 变更动作…」。
- 之前：v1.91.0 只读回合 `git status`/`git log` 被拒（M131b R4 部分修复）；`git branch` 列举被拒（#787 修）。均为按字面/整条命令粗判。
## 需求
- R1 只读意图下的 bash 判定改为**按片段**：把命令按 `;`、`&&`、`||`、`|`、换行拆开，逐段判断；只有出现明确的写入/副作用片段才算变更（重定向写文件 `>`/`>>`（/dev/null 除外）、rm/mv/cp/mkdir/touch/chmod、git 的 commit/push/checkout -b/reset/merge/rebase/stash/tag 等、gh 的 create/merge/edit/close/run 等、包管理安装、curl -X POST/PUT/DELETE 等）。常见只读命令（git status/log/diff/show/branch/remote -v/rev-parse/ls-files、gh pr view/list/checks、gh run list/view、ps、grep/rg、ls、cat、head、tail、wc、find（无 -delete/-exec 写）、env、which、date 等）放行。拿不准的片段 → 放行但不执行写入类重定向；不得把整条命令因一个不认识的只读工具判为变更。
- R2 拒绝时返回给模型的提示改为一句人话并指出具体是哪一段被视为变更、为什么；不把内部策略说明整段塞给模型。
- R3 测试（先失败）：上述证据命令放行；`git status; git commit -m x` 拒且指出 `git commit`；`cat a > b` 拒；`ls > /dev/null` 放行；管道 `ps | grep x` 放行。
## 约束
- 安全：不放宽写入判定；信任模式与审批规则不变。授权交付（ceiling: merged）。不要超出规格自加范围。

## Implementation Notes
- 只读意图准入与完成证据分类分离；复用 shell 分段基础设施，保留既有完成证据分类语义。
- Req ↔ 测试：R1/R3 `m138_readonly_compositions_are_allowed`、`m138_explicit_side_effects_are_denied`；R2 `m138_denial_names_segment_and_reason`。
- Applicable Harnesses：Spec、Compatibility（既有只读与执行能力）、Payload（shell 参数及重定向）、AI Collaboration；此次不修改 UI，不发版，真实安装版验收由编排方发版后完成。
- 主路径：ReviewOnly → capability_denial → 逐段副作用检查 → 原权限网关（不变）；同一后台 agent-loop 入口无需 GUI。
