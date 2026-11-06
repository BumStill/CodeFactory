# M248b：信任模式下，真实任务的本地 git 命令还在要审批

## 现场（v1.98.0、v1.99.0，M248 已上线）
M248 的固定组合通过了测试，但真实任务里的命令仍被 `trusted.git.unproved` 拦下，每单 1 到 3 次。样本（路径已省略）：
1. `git fetch --prune origin main && git merge origin/main && git config core.hooksPath .githooks; gh auth status; sed ...; rg ...; cat ...`
2. `git fetch --prune origin main && git merge-base --is-ancestor origin/main HEAD && git config core.hooksPath .githooks; cat ...; rg ...; ls '<应用数据目录>'; ...`
3. `git add docs/specs/... && git commit -m 'docs: ...'; sed ...; grep -n 'fn git' ...; grep -n 'pub.*register\|fn.*execut\|static.*' ...; command -v cargo; df -h .; git status --short`

4. `python3 - <<'PY'` 只读打开应用数据库（`mode=ro`）被 `trusted.heredoc.unproved` 拦：「路径在工作区外」。一单批了 5 次。

## 用户决定（必须遵守）
2026-11-04：信任模式靠 OS 沙箱，不再补文字规则，不加危险词表。沙箱不可用时退回审批，绝不裸跑。
`trusted_git.rs` 的 `outside_write_sandbox_segment` 现在就是一张词表（`gh`、`curl`、`token` 等）。

## 规格（放到 `docs/specs/feature-specs/m248b-trusted-git-real-commands.md`）
- R1：把上面 3 条样本写成测试，确定每条是哪个判断项（`rewrite`、`remote`、`outside_scope`、`is_enforced`）拦下的，结论写进规格。第 3 条必须查清。
- R2：用 `permission_intents` 再统计 M248 上线后的命令形状，只留匿名聚合，确认样本有代表性。
- R3：沙箱生效时，只读取工作区外文件的 heredoc 脚本也不产生审批，写入由沙箱挡下。本地命令组合（`git add`、`commit`、`fetch`、`merge`，以及读取类命令）不产生审批，哪怕同一行混了只读的 `gh` 查询或读工作区外文件。沙箱本身挡写入，读取不需要文字证明。
- R4：判定收敛到结构化事实，能删的词表项要删。不新增按命令文字匹配的规则。如果某项必须保留，写明为什么沙箱管不了。
- R5：不放宽：网络写操作（`git push`、`gh pr edit` 等）和破坏性改写（`reset --hard`、`push --force`、删别人分支）照旧要授权；`core.hooksPath` 门禁、`--no-verify` 门禁不动；沙箱不可用时退回审批。

## 测试
- 3 条样本原样通过，不产生审批。先看到失败。
- R5 每项一条反例，包括一行混合只读和写操作的命令。

## 验收（真机，编排方发版后做）
下一单任务全程 `trusted.git.unproved` 为 0。

## 交付
授权交付。PR body 写 `Scenario-Test`，按任务模板收尾。
