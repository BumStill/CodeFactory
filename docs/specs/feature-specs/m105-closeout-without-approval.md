# M105：信任模式下，合并后收尾清理本任务自身产物不再弹审批

## 背景（真机 2026-10-17，两次以上）

合并完成后，agent 按 task-template §8 清理自己产生的文件，却被要求人工审批——会话本身已经是信任模式（`full_access`）：

- `9a6ac759`（#681 合并后）：`rm -rf .codefactory-cache/cargo-target-close-objective-multi .codefactory-cache/pr-body-… /tmp/pr681-body.md && gh api -X DELETE …/git/refs/heads/<本任务已合并分支>` → 弹审批。
- `d8f2260f`（#705 合并后）：`rm -rf "$PWD/.codefactory-cache/cargo-target-m103"` → 弹审批。

结果：每个任务都无法无人值守收尾，2.0 门禁①把这次人工批准记成人工介入。

## 需求

- **R1** 用代码与数据说明这两次为何被判需要审批（哪条规则命中）。
- **R2** 提供一条免审批的安全收尾路径（优先：交付工具 / closeout 内置动作，而不是放宽通用 bash 审批）：
  - 删除本任务工作区内 `.codefactory-cache/` 下的产物；
  - 删除本任务自己的、经 GitHub 确认 MERGED 的 PR 的远端分支；
  - 删除本任务在系统临时目录创建的、可证明归属的文件（若无法证明归属则不删）。
- **R3** 安全边界不变：工作区外路径、非本任务分支、未合并分支、用户文件的删除仍需审批或拒绝；通用 `rm -rf` 的审批规则不放宽。
- **R4** 更新 task-template §8 收尾指引，让 agent 使用新路径而不是手写 `rm -rf`（仓库内对应文档一并更新）。
- **R5** 测试（先失败）：信任模式下内置收尾删除自身缓存与已合并分支 → 无审批；试图删工作区外路径或未合并分支 → 仍需审批/拒绝。

## Requirements Traceability

| Req ID | 需求 | 最小证据 |
| --- | --- | --- |
| R1 | 说明两次事故命中的规则 | 本节「R1 结论」+ `trusted_workspace::tokenize` 的 `REJECTED` 列表与事故命令逐字符对照 |
| R2 | 免审批安全收尾路径 | `agent::closeout`（`classify` / `remove` / `bash_cache_delete`）+ `agent::closeout::tests` |
| R3 | 边界不放宽 | `agent::closeout::tests::r3_*`、`agent::trusted_workspace::tests::r2_counterexamples_still_ask`、`m105_closeout_cache_spelling_is_allowed` |
| R4 | 收尾指引更新 | `docs/development-agent.md` §收尾；`~/.codefactory/agent-tools/task-template.md` §8 由编排方同步（同文本已写入本文档「收尾指引」） |
| R5 | 先失败后通过的测试 | 见「测试矩阵」列出的 5 个用例；首轮运行中 `r5_own_cache_artifact_is_allowed`、`r3_*`、`m105_closeout_cache_spelling_is_allowed` 均失败 |

## Applicable Harnesses

- Spec Harness：本文件。
- Payload Harness（文件/命令 surface）：收尾删除只作用于三类可证明归属的 artifacts，见 R2/R3 证据。
- 不涉及 Viewport / Release / Compatibility Harness：纯权限判定与本地文件删除逻辑，无 UI、无协议、无既有配置兼容问题。

## R1 结论（哪条规则命中）

信任模式唯一放宽的路径是 `agent::mod::decide_permission_for_call` →

```rust
if let (PermissionDecision::Ask, "bash") = (&base, tool_name) {
    if policy.full_access {
        if let (Some(root), Some(command)) = (workspace_root, cmd) {
            if trusted_workspace::local_delete(root, command) {
                return PermissionDecision::Allow;
            }
        }
    }
}
```

`local_delete` 先过 `trusted_workspace::tokenize`，它的 `REJECTED` 列表是

```rust
[";", "&&", "||", "|", "&", ">", "<", "`", "$", "(", ")", "{", "}", "*", "?", "[", "]",
 "~", "\n", "\r", "\"", "'", "\\", "!", "#"]
```

逐条对照事故命令：

| 事故命令 | 命中的拒绝字符 | 额外原因 |
| --- | --- | --- |
| `rm -rf "$PWD/.codefactory-cache/cargo-target-m103"` | `"`、`$` | 无法证明展开后是工作区内路径 |
| `rm -rf … /tmp/pr681-body.md && gh api -X DELETE …` | `&&`、`$`(若带 `$PWD`)、绝对路径 `/tmp/...` | `path_stays_inside` 对绝对路径直接返回 `false`；`&&` 属于组合语法 |

也就是说：**不是**"删除工作区内容"本身需要审批，而是这两次写法里有 parser 不能证明的语法（引号、`$` 展开、`&&` 组合、绝对路径），于是按设计 fail closed 回到普通确认。这正是 R3 要求保留的行为，因此修复方向是新开一条有归属证明的收尾通道，而不是把这些字符从 `REJECTED` 里删掉。

## 收尾指引（R4 文本，供 task-template §8 同步）

§8 的「Clean up what you produced」改为：

- 删除本任务构建目录：只发单条删除命令，路径必须是相对工作区的 `.codefactory-cache/<本任务目录>`；不要用 `$PWD`、引号、`&&` 或 `;` 拼接（组合命令一律需要审批）。
- 远端分支：先用 `gh api repos/<owner>/<repo>/pulls/<n>` 确认 `merged=true` 且 `head.ref` 等于本任务分支，再单独发一条 `gh api -X DELETE .../git/refs/heads/<branch>`；不得与 `merge` 或删除串成一条命令。
- 系统临时目录文件：只有文件名带本任务 token（任务号/PR 号）时才删；无法证明归属就保留并在最终报告里写明。
- 任何工作区外路径、用户文件、默认分支、未合并分支：不要尝试免审批，直接按普通审批流程处理。

## Implementation Notes

- 新模块 `src-tauri/src/agent/closeout.rs` 是唯一的收尾判定实现：`CloseoutArtifact::{WorkspaceCache, MergedRemoteBranch, OwnedTempFile}` + `classify` / `remove` / `bash_cache_delete`。
- `trusted_workspace::local_delete` 在最前面调用 `closeout::bash_cache_delete`：仅接受 `.codefactory-cache/<something>` 操作数（可带 `$PWD/`、`${PWD}/` 与单层双引号）和已复核的 `rm` 标志；其它一切仍走原逻辑。
- 边界：`.codefactory-cache` 根目录本身、`..`、绝对路径、`~`、其余 `$` 展开、`;`/`&&` 组合、`sudo`、不存在目标、符号链接逃逸一律 `RequiresApproval`。
- `remove` 在 `classify` 不为 `Allowed` 时不触碰文件系统。
- 遗留（不在本任务范围）：通用工作区证明 `local_delete` 仍允许 `rm -rf .codefactory-cache`（删除整个共享缓存目录），这是 M41 既有行为，未在本次收窄。

## 测试矩阵（R5）

| 用例 | 断言 |
| --- | --- |
| `agent::closeout::tests::r5_own_cache_artifact_is_allowed` | 自身 `.codefactory-cache/<task>` 四种写法（相对、`"$PWD/"`、`${PWD}/`、`./`）均免审批 |
| `agent::closeout::tests::r3_outside_and_user_paths_still_ask` | 工作区外、缓存根、用户文件、`sudo`、组合命令、通配符、不存在目标仍需审批 |
| `agent::closeout::tests::r3_branch_requires_merged_own_head` | 仅 `merged=true` 且是本任务 PR head 且非默认分支时免审批 |
| `agent::closeout::tests::r3_temp_file_requires_ownership_token` | 无 token、token 不匹配、嵌套目录、临时目录外均拒绝 |
| `agent::closeout::tests::r2_remove_executes_only_proven_artifacts` | 只删除已证明的 artifacts，用户文件不受影响 |
| `agent::trusted_workspace::tests::m105_closeout_cache_spelling_is_allowed` | 事故原写法免审批；`../checkout`、`/etc/hosts`、`"$PWD/notes.txt"`、组合命令仍审批 |
