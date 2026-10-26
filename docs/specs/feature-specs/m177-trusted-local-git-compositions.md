# M177：信任模式下本地 Git 组合仍弹审批

状态：已实施
关联：M159（可疑 Git 操作先快照再放行）、M166（复合片段与转义）、M172（按可执行片段判断敏感动作）、M174（交付前同步工作区）

## 证据（v1.91.35，已含 M172 #870「按可执行片段判断」）

- 10-26 17:1x 会话 `aeba516d`（M174，trusted，本会话托管工作区）在
  `git fetch --prune origin main && git stash push -u -m m174-sync-before-delivery && git merge origin/main && git stash pop`
  上等待审批，零介入试跑因此中断。
- 10-23 ~ 10-26 另有零星误拦：`git add -A && git commit`、`git checkout origin/main -- <path>`、
  真实只读脚本（`node -e '...'`、`cat <工作区内文件>`）。

## 根因（R1）

`sensitive()`（M172）判断正确：整条命令确实包含本地改写动作，因此进入 M159 的快照分支。
真正失败的是快照前的**证明层** `prove()`：它只接受极窄的 argv 白名单——

| 片段 | 白名单要求 | 实际命令 | 结果 |
|---|---|---|---|
| `git stash push -u -m m174-sync-before-delivery` | 只接受 `stash push` 或 `stash push -m <msg>` | 多了 `-u` | 证明失败 |
| `git merge origin/main` | 只接受 `merge origin/main --no-edit` | 缺 `--no-edit` | 证明失败 |

证明失败 ⇒ 无法快照 ⇒ M159 回落到“请审批”。也就是说：**不是判定错了，是可证明的本地改写语法太窄。**

## 需求

- **R1** 用真实判定路径复现，指出触发审批的片段（上表）。
- **R2** 信任模式下，本会话工作区内的本地 Git 改写
  （`stash push/pop/apply`、`merge`、`rebase`（非交互）、`checkout`、`switch`、`reset`、`restore`、`clean`）
  与 `fetch` 组合**直接放行**；只保留远端/越界/破坏性动作需要审批。
- **R3** 放行集合之外仍必须审批，且必须有回归测试。
- **R4** 在真实 App 的信任会话里验证零介入（headless/主路径）。

## 实现

`prove()` 在 M172 的只读判定之后增加一层**本地改写语法** `local_rewrite_args()`，
按子命令校验每一个参数，而不是整串匹配：

- 放行：`fetch`（仅更新远端跟踪引用）、`stash push|save|pop|apply|list|show`、
  `merge`（含 `--abort/--continue/--quit`）、`rebase`（拒绝 `-i/--interactive/--exec`）、
  `switch`/`checkout`（分支移动、`-- <工作区内路径>`）、`restore`、`clean`（不含 `-x`）、
  `add`、`commit`；只读侧补齐 `status --short/--branch` 与 `log --oneline -<N>`。
- 仍审批：`push`（含 `--force*`/删除分支/refspec）、`git -c`、`gh` 远端动作、
  任何 `..`/绝对路径/工作区外路径、`stash drop|clear`、`clean -x`（快照无法恢复被忽略文件）、
  交互式 rebase、`$(...)` 等无法预判的展开。

**M159 保证不变**：放行仍然要经过私有索引快照；本缺陷只去掉“等你点同意”，
不去掉“先备份再动”。`prove()` 一旦失败仍然 fail-closed。

## Spec 取代关系

- **M159 R2** 原有断言 `git checkout main` 必须审批，被 **M177 R2** 取代：工作区内的分支移动属于放行集合。
  `src-tauri/tests/m159_trusted_git.rs` 中该用例改为越界变体 `git checkout main -- ../other/tracked.txt`，
  保留 M159“越界 checkout 必须审批”的原意。

## 验收

| 项 | 命令/证据 |
|---|---|
| 事故命令可证明 | `src-tauri/tests/m177_trusted_local_git.rs::m177_incident_composition_is_provable_without_approval` |
| 同步族组合放行 | `m177_sync_compositions_from_the_incident_family_are_provable`（16 条命令 + 快照断言） |
| 误拦的只读命令 | `m177_previously_blocked_read_only_commands_do_not_ask` |
| 远端/越界/破坏性仍审批 | `m177_remote_out_of_bounds_and_destructive_actions_still_ask`（15 条反例） |
| M159/M166/M172 无回归 | `cargo test --test m159_trusted_git`、`--test m177_trusted_local_git` |

## AI Collaboration

- context scope：`src-tauri/src/agent/trusted_git.rs`、`src-tauri/src/agent/permission_gateway.rs`、M159/M172 两套既有测试。
- assumptions：审批提示来自“证明失败→无法快照”，而非 `sensitive()` 误判；已由事故命令的 RED 用例直接证实。
- review point：`clean -x` 未被放行，因为它会删除快照无法恢复的忽略文件；如需放行应由产品决定。
- validation result：见上表，全部机器断言通过。
