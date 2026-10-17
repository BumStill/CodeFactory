# M52b：完成门禁仍把 gh 只读查询命令当成必须重跑通过的检查（#689 未覆盖）

## 现象（真机，v1.88.12 已包含 #689「read-only probes never become must-pass checks」）

- 13350c96（#691）17:50–17:51 三次 completion_evidence_incomplete，blocker check=`gh run view --job 117365381431 --log-failed`。
- 3b87a1b6（#688）17:26–17:27 同样被 `gh run view 39018633201 --log-failed` 拦。
- 这类命令只读查询 CI 状态/日志（非零退出通常只代表「那次 CI 失败了」），不是验收检查。门禁要求它「重跑通过」→ agent 无法满足 → 反复驳回 → 回合被结算（随后触发 M99 误取消）。

## 需求

- R0 读 #689 的实现，说明它为何没覆盖 gh（列出它的判定规则）。
- R1 把 forge 只读查询纳入只读探查：`gh run view|list|watch`、`gh pr view|checks|list|diff|status`、`gh api`（无 -X/--method 或方法为 GET）、`git log|show|status|diff|fetch` 等；按命令结构判定，不是继续往词表加关键词——要给出可扩展的结构化判定（子命令 + 是否有写方法/写参数），并说明边界（`gh pr merge`/`gh api -X POST`/`gh workflow run` 是写入，不是探查）。
- R2 门禁对「探查命令失败」最多作为参考信息，不作为 blocker；若 agent 的验收检查都已通过，不得因探查失败驳回。
- R3 测试（先失败）：上面两条真实命令形状（含 `cd "<path>" && gh run view … --log-failed`）+ 写入类反例。
- R4 用真机数据回放：对 13350c96、3b87a1b6 那几次 verdict 的输入跑新判定，结果应为通过。

## 约束

- 不放宽真实验收失败的阻断。合成 fixture，不写生产 DB。

## Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| M52b-R0 | 说明 #689 的判定规则，以及它为何没覆盖 gh | 本规格「R0 结论」章节 |
| M52b-R1 | forge 只读查询按结构纳入探查，并给出结构化判定与写入边界 | `forge_read_only_queries_are_probes_not_checks`、`forge_write_commands_are_never_probes`、`forge_read_only_queries_do_not_demand_a_mutation_receipt`、`forge_write_commands_stay_mutations` |
| M52b-R2 | 探查失败不作为 blocker；验收检查全绿即可完成 | `a_failed_forge_query_never_opens_a_must_pass_check`（并保持 `a_failed_acceptance_check_still_blocks_until_it_passes` 绿） |
| M52b-R3 | 先失败：两条真实命令形状 + 写入反例 | 先失败记录（3 条新测试在实现前全部失败） |
| M52b-R4 | 用真机 verdict 输入回放，结果通过 | `a_failed_forge_query_never_opens_a_must_pass_check` 以 13350c96 的完整命令形状为输入，断言无 `failed_verification` 且验收通过后 `completed == true` |

## R0 结论：#689 的规则与缺口

`#689`（`fdf30236`，规格 `completion-gate-probe-commands.md`）在
`codefactory_agent_core::classify_command` 里加了两层：

1. **结构兜底** `command_is_read_only_probe`：把命令切成「语句 → 管道段」，
   取每段的 `execution_payload`，要求**每一段的首词**都在
   `READ_ONLY_PROBE_VERBS`（`awk/basename/cat/cd/cut/df/dirname/du/echo/file/find/
   grep/head/jq/ls/printf/pwd/readlink/realpath/rg/sed/sort/stat/tail/tr/tree/type/
   uniq/wc/which`）里，且没有 `grep -q` / `jq -e` 这类「用退出码下结论」的标志；
   命中即 `ToolKind::ReadOnly`。
2. 在 `classify_command` 中把该判定放在 verification 分支之前短路，并在
   `CompletionGate::record` 里以同一结构守卫兜底，因此纯探查失败不会写入
   `failed_verifications`。

缺口有两处，都在同一个位置：

- 该词表只覆盖 unix 查询动词，`gh`/`git` 首词永远不在表内，所以
  `gh run view --job … --log-failed` 不是探查。
- 更直接的是：`classify_command` 里有一段专门的
  `gh_read_only_verification`（`gh run view|list`、`gh pr view|checks`、
  `gh release view|list`、GET 形态的 `gh api`）把它**主动提升为
  `ToolKind::Verification`**，于是它进入 `verified`/`failed_verifications` 通道，
  非零退出被翻译成 `检查 … 没有通过；修复后请重跑 …`。
  这段分支的原始动机是另一个方向的修复（2026-09-08 的「系统未执行」：
  `ReadOnly` 但 `bash_is_explicit_read_only` 不认 `gh`，于是被要求
  mutation receipt）——修补只做了「提升为 Verification」，没有同时教会只读判定。

真机复现的形状（13350c96）：

```
cd "/Users/…/execution-workspaces/…" && gh run view --job 117365381431 --log-failed
```

旧判定给出 blocker：`检查 gh run view --job 117365381431 --log-failed 没有通过；
修复后请重跑 cd "…" && gh run view --job 117365381431 --log-failed …`。

## 判定规则（R1）

forge/git 判定与 `READ_ONLY_PROBE_VERBS` 并列，但按**结构**而不是首词子串：

- **二元组白名单**（`GH_QUERY_SUBCOMMANDS`）：`gh <group> <verb>`，只有
  `run view|list|watch`、`pr view|checks|list|diff|status`、
  `release view|list`、`workflow list|view`、`issue view|list|status`、
  `repo view|list`、`search code|issues|prs|repos|commits`、`auth status`、
  裸 `gh status` 是查询。`gh` 前的全局标志（`-R/--repo`、`--hostname` 等）跳过。
- **`gh api` 看方法与请求体**：`-X/--method`（`-XPOST`、`--method=POST` 也算）
  非 `GET` 即写；`-f/--field`、`-F/--raw-field`、`--input` 任一出现即视为发送请求体，
  不是探查。未写方法视为 GET。
- **git 白名单**（`GIT_QUERY_SUBCOMMANDS`）：`log/show/status/diff/fetch/
  rev-parse/ls-files/describe/blame/shortlog/show-ref/merge-base`。
- **写入边界**（`is_forge_mutation_segment`，在探查短路之前判定）：`gh` 中任何
  非查询的 `<group> <verb>`（`pr merge`、`run rerun`、`workflow run`、
  `release create/upload`…）与任何非 GET 的 `gh api` 都是 `Mutation`；
  `git` 的 `push/commit/checkout/switch/reset/clean/stash/apply/add/rm/mv/tag/
  merge/rebase/pull/…` 同样是 `Mutation`。混合命令（`gh pr view && gh pr merge`）
  因含写入段而整体按写入处理。
- 纯探查（每段都是查询）→ `ToolKind::ReadOnly`；其失败只作为参考信息，
  不进入 `failed_verifications`（R2）。
- 同一 `is_forge_query_segment` 谓词被 `src/agent/tool_backend.rs` 的
  `bash_segment_is_read_only` 复用，使 `ReadOnly` 判定不会反过来被
  「未通过只读白名单」而要求 mutation receipt（2026-09-08 的另一个方向）。
- 例外说明：`git fetch` 原先在 `has_workspace_mutation` 的写入列表里。它只推进
  remote-tracking refs、不改工作树，本规格（R1）明确把它列为只读探查，故从该
  写入列表移除；`git pull`/`git checkout` 等仍为写入。

## R4 回放证据

真机 verdict 的输入是命令形状与工具结果，生产 DB 未被写入。回放以任务书给出的
两条完整命令为输入：

- `cd "/Users/…" && gh run view --job 117365381431 --log-failed`（13350c96，退出码 1）
- `gh run view 39018633201 --log-failed`（3b87a1b6）

`a_failed_forge_query_never_opens_a_must_pass_check` 用第一条命令喂给
`CompletionGate::new(true)`（mutation + 失败的 forge 查询 + 一次失败的
`gh pr checks` + 通过的 `pnpm test`），断言：

1. 证据里不存在 `kind == "failed_verification"` 的 blocker；
2. 验收检查通过后 `completed == true`。

## Applicable Harnesses

Spec Harness；Compatibility Harness（`CompletionEvidence` / `failed_verifications` /
`gate_events` 语义不变，只改变 forge 查询的归类）；AI Collaboration Harness。

本任务无 UI、无发布面、无请求体或对象存储变更，故不触发 Viewport / Release /
Payload Harness。

## 测试矩阵

| 场景 | 断言 |
| --- | --- |
| `gh run view --job … --log-failed`（含 `cd "<path>" &&` 前缀） | `ReadOnly`，且 `command_is_read_only_probe` 为真 |
| `gh pr checks/view/list/diff/status`、`gh api` GET 形态 | `ReadOnly` |
| `git log/show/status/diff/fetch` | `ReadOnly` |
| `gh pr merge`、`gh api -X POST`、`gh api … -f`、`gh workflow run`、`git push` | 非 `ReadOnly`；混合命令整体非探查 |
| 失败的 forge 查询 + 验收全绿 | 无 `failed_verification` blocker，`completed == true` |
| 真实验收检查失败 | 仍阻断，重跑通过后才完成（既有测试保持绿） |
| 准入层 | 只读 forge 查询不要求 mutation receipt；写入仍要求 |
