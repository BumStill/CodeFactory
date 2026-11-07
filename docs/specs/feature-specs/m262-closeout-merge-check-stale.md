# M262：结束说明说「main 上还没有这个 PR 的合并提交」，其实已经有了

## 现场（2026-11-07 22:28，v1.105.0，会话 f9a75b60）

结束原因写着：`PR #1031 已合并；main 分支上还没有这个 PR 的合并提交；…`。
实际上 PR #1031 已经合并，合并提交就在 main 上。

## 根因

M259 落地时，判断「合并提交是否在默认分支上」用的是**提交信息文本匹配**：

- 取 `origin/main` 最近 500 条提交的标题（`git log --format=%s -n 500`）；
- 看有没有任何一行的标题里含 `(#<PR 号>)`。

这条判据依赖一个并不成立的约定：合并提交的标题一定带 PR 号。本仓库反例就在
历史里——PR #1031 的 squash 合并提交 `139f4488a`
（`feat(m261): 插话在接受时落库，长回合读不到也不会消失`）的**提交标题和正文都没有
`1031`**。于是"已经合并"被写成"还没有合并"。另外 `-n 500` 也会让更早的合并提交
超出检索范围，同样得到假的「还没有」。

## 需求

- **R1**：合并提交是否存在，必须以**该 PR 真实的合并提交编号**为准：用 `gh pr view <n>
  --json mergeCommit` 取到合并提交 SHA，再在**刚 fetch 过**的远端默认分支上核对它是否
  已经在里面（`git branch -a --contains <sha>`）。提交标题里的 `(#N)` 只能作为辅助线索。
- **R2**：只要有一个环节确认不了（取不到合并提交编号、fetch 失败、SHA 不在本地对象库），
  写「合并提交未核验：<原因>」，绝不写成「还没有这个 PR 的合并提交」。已合并被说成没合并
  是错误信息中最伤人的一种：用户会去手工再合并一次。
- **R3**：辅助检索不得再被 `-n 500` 这类条数上限截断；查默认分支就查完整历史。

## 实现要点

`src-tauri/src/agent/thin_derive.rs`：

- `closeout_gh_merge_commit(target)`：`gh pr view <n> --json mergeCommit --jq .mergeCommit.oid`，
  带独立超时与 no-window 约束，返回 40 位 SHA 才算取到。
- `verify_closeout_with(target, lookup)`：把「取合并提交编号」做成可注入的查找函数，
  `verify_closeout(target)` 用真实 `gh` 实现。测试因此能稳定复现事故形态，而不依赖网络。
- 判定顺序：
  1. `git fetch --prune <remote> <default>` 失败 → 未核验；
  2. 取到 SHA 且 `git branch -a --contains <sha>` 列出 `refs/remotes/<remote>/<default>` →
     「合并提交已在 <default> 分支（<短 SHA>）」；
  3. 取到 SHA 但不在默认分支上 → 「<default> 分支上还没有这个 PR 的合并提交」（此时是可信的否定）；
  4. 取不到 SHA，退回**完整历史**里找 `(#N)`；找到即已在；仍找不到 → 未核验并附上原因。

## 测试

`src-tauri/src/agent/thin_derive.rs` 测试模块：

- `m262_unknown_merge_commit_is_unverified_not_absent`：合并提交标题不含 PR 号的仓库里，
  查不到合并提交编号时只能说未核验，不能说没合并（复现本次事故）。
- `m262_real_merge_commit_on_default_branch_is_reported_from_its_sha`：注入真实合并提交
  SHA 后，即使标题里没有 PR 号，也必须报「合并提交已在 main 分支」。
- M259 的既有用例（`m259_*`）全部保持通过：`(#N)` 辅助检索、远端分支清理、
  本地改动只报告、异常路径写未核验。

## 场景

- `CXD-002` 交付证据链决定完成：结束说明里的每条收尾事实都要有当场可核对的来源。
- `HLT-004` / `HLT-001`：`src-tauri/src/agent/thin_derive.rs` 的既有场景映射。
