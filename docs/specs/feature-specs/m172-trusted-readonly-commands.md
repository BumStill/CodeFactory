# M172：信任模式下只读本地命令不再弹审批

状态：已实施（v1.91.x 修复）
关联：M41（CF-TPP-R1/R2 本地越界证明）、M130（两档权限：本地放行 / 远端看交付授权）、M159（可疑 Git 操作先快照再放行）

## 证据

v1.91.34，2026-10-26 10:3x，会话 `dad2db79`，`permission_mode=trusted`，授权交付。

进入审批的两条命令（均为只读）：

1. `git fetch --prune origin main && git merge-base --is-ancestor origin/main HEAD; find src-tauri/src -iname '*deliver*'; grep -R -n … | head -90; ls src-tauri; command -v pnpm; command -v cargo; gh auth status`
2. `node -e '…require("./docs/testing/scenario-registry.json")…console.log(…)' | head -20`

同会话 12:0x 又复现一次。这两次是"零介入试跑"的第 1 次人工介入，本身即缺陷。

## R1：触发点定位（结论）

审批不是来自权限策略表（信任模式 `allow: ["*"]`、`ask: []`、`full_access: true`），而是来自 M159 的"可疑 Git 操作"预检：

`src-tauri/src/agent/trusted_git.rs::sensitive()` 对**整条命令文本做子串匹配**：

```rust
["reset","restore","checkout","clean","stash","rebase","filter-branch","filter-repo"]
    .iter().any(|w| lower.contains(w))
    || lower.contains("worktree") || lower.contains("git -c") || lower.contains("git merge") || lower.contains("git fetch")
    || (lower.contains("push") && …)
```

两个后果：

- **命令 1**：`git fetch` 命中 `contains("git fetch")`；`git merge-base --is-ancestor` 命中 `contains("git merge")`。`fetch` 只更新远端跟踪引用、`merge-base` 只回答祖先关系，都不动本地工作树，却被当成"会丢弃本地工作"的动作 → 要求快照 + 审批。
- **命令 2**：匹配范围是**整条命令文本，含引号内的内联脚本正文**。`node -e '…'` 的脚本文本里只要出现 `restore`/`clean`/`checkout`/`stash`/`worktree` 等词（脚本读的 `docs/testing/scenario-registry.json` 全量打印就包含这类单词），就被判成破坏性 Git 操作 → 审批。这是"数据"被当成了"可执行语句"。

同时确认 M130 远端判定（`agent-loop/src/policy.rs::is_delivery_command`）在命令 1 上没有误判：该命令全部片段都是只读探针，`command_is_read_only_probe` 短路返回 false。但该函数只在**整条命令**全部为探针时短路，混合命令（如 `git fetch … && gh pr view 12`）里出现 `gh`/`pr` 字面量仍会被判成远端动作，这是同一类"按词扫描而不是按可执行片段判定"的缺陷，一并修正。

## R1：需求

信任模式下，下列只读命令及其任意组合不弹审批：

- `git fetch` / `merge-base` / `log` / `status` / `diff` / `show` / `rev-parse`
- `gh auth status`（不带 `--show-token`）、`gh pr view|list|checks|diff`、`gh run list|view`、`gh release list|view`
- `find` / `grep` / `rg` / `ls` / `cat` / `head` / `command -v`

实现约束：判定必须基于**可执行片段**（引号外的 shell 词），不得把引号内数据当作可执行语句；`git merge-base` 与 `git merge` 必须区分到子命令粒度。

## R1b：需求

信任模式下，cwd 为本会话托管工作区、且不含远端/越界/破坏性片段（R2）的本地命令——包括 `node -e`、`python3 -`、`cargo`、`pnpm` 等解释器与构建命令——默认放行，不因"无法静态判定"而默认审批。

边界仍然 fail-closed：无法安全切分（未加引号的 `$(...)`、反引号、控制字符、未闭合引号）、解释器内联代码里出现**执行 API + 破坏性 Git 动作**（`execSync`/`spawnSync`/`exec(`/`subprocess`/`os.system` 且含 `git reset|clean|checkout|restore|stash|rebase|merge|force push`）时，仍判为可疑 → 审批 + 快照。

## R2：仍需审批（不得削弱）

- 凭据：`gh auth token`、`gh auth status --show-token`、`gh auth login|logout`
- 远端写：`gh pr merge|close|reopen`、`gh release create|delete|upload|edit`、`gh api` 写方法、`gh workflow run`、`gh ruleset`、`gh secret`、`gh variable`、`gh repo delete|create|archive`
- 破坏性 Git：`git reset|restore|checkout|clean|stash|rebase|filter-branch|filter-repo|merge|worktree`、`git -c`、force/delete push
- 越界：绝对路径、`..`、`~`、`$VAR`、`.git`/`.env`/凭据文件（M41 既有规则不变）

## R3：测试矩阵（先失败后通过）

| # | 场景 | 期望 |
|---|---|---|
| 1 | 命令 1（真实只读组合）在信任模式 | 放行（`Allow`），`is_delivery_command` 为 false |
| 2 | `gh auth status --show-token` | 审批（shell 策略 `Ask`） |
| 3 | `gh pr merge 12` | 远端动作，需交付授权；未授权即审批 |
| 4 | `node -e '…restore/clean/worktree…' \| head -20` | 放行（引号内数据不参与判定） |
| 5 | `python3 -`、`cargo test`、`pnpm test` | 放行 |
| 6 | `git merge-base --is-ancestor a b` | 放行；`git merge main` 仍需审批 |
| 7 | `bash -c "git reset --hard"` | 仍审批（解释器内联 shell 递归判定） |
| 8 | `node -e '...execSync("git reset --hard")...'` | 仍审批（执行 API + 破坏性动作） |
| 9 | `git fetch --prune origin main && gh pr view 12` | 非远端（片段级只读判定） |
| 10 | `gh pr view 12 && gh pr merge 12` | 远端（混合命令不得洗白写动作） |

## 变更点

- `src-tauri/src/agent/trusted_git.rs`：`sensitive()` 改为按引号外的可执行片段、子命令粒度判定；内联解释器代码单独处理。
- `src-tauri/src/agent/permission_gateway.rs`：M159 分支改用新的 `sensitive()`；远端分支遇到 shell 策略 `Ask` 时仍然审批（不被交付授权短路）。
- `src-tauri/crates/agent-loop/src/policy.rs`：`is_delivery_command` 片段级判定，只读片段不再贡献远端语义。
- `src-tauri/crates/agent-core/src/lib.rs`：公开片段级探针 API；`--show-token` / `gh auth token` 不再算只读探针。
- `src-tauri/src/tools/shell_policy.rs`：R2 的凭据暴露与仓库保护类写操作归为 `Ask`。
