# M52：完成门禁把失败的探查命令当成「必须重跑通过的检查」

## 现象（10-10 真机，需先确认是否仍存在）

- 会话 2bb95fef：完成门禁连续 4 次以 blocker `check: "cat package.json && ls vitest.config* vite.config* && grep -rn lightModeAudit ..."` 判证据不足。那是 agent 早先的只读探查命令（ls 不存在的文件导致非零退出），不是验收检查；门禁却要求它重跑通过，任务卡在「证据不足」，实际没有任何检查失败。
- 同族：U1（门禁误判）。

## 需求

- R0 用合成 fixture 复现（或用代码+数据证明已不可能），结论写进规格；已修复则补回归测试锁住并说明由哪个改动修复。
- R1 门禁区分「验收检查」（测试/构建/lint/typecheck/声明的验收命令）与「只读探查」（cat/ls/grep/find/rg/head 等组合、无写入的查询）。探查命令非零退出不构成 blocker。
- R2 判定依据是命令结构与已声明的验收清单，不是关键词词表堆砌（参见：不许往词表加词修复反复出现的问题）。说明判定规则及其边界情况（如 `ls x && pnpm test` 这类混合命令按验收处理）。
- R3 测试：探查失败 + 验收全绿 → 通过；验收失败 → 仍阻断；混合命令 → 按验收处理。各有先失败的用例。

## 约束

- 不放宽真实验收失败的阻断。合成 fixture，不写生产 DB。

## Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| M52-R0 | 用合成 fixture 复现，或证明已不可能；已修复则补回归测试并说明由哪个改动修复 | 规格内的 R0 结论 + 锁定回归测试 |
| M52-R1 | 门禁区分验收检查与只读探查；探查非零退出不构成 blocker | `a_read_only_probe_that_mentions_a_runner_stays_a_probe`、`a_failed_read_only_probe_never_opens_a_must_pass_check` |
| M52-R2 | 判定依据是命令结构与已声明验收清单，不是关键词词表；给出规则与边界情况 | 规格内的判定规则章节 + `a_mixed_probe_and_check_command_is_an_acceptance_check` |
| M52-R3 | 三条测试（探查失败+验收全绿 / 验收失败 / 混合命令）都先失败 | 先失败记录 + 三条测试 |

## 判定规则（R2）

命令按 shell 结构切成「语句 → 管道段」，每段只看**实际执行什么**（`execution_payload`，
剥掉 `cd`/`env`/赋值等前缀），不看参数里出现的词。

- **只读探查（probe）**：每一个可执行段的动词都是查询类动词（`cat`、`ls`、`grep`、
  `rg`、`find`、`head`、`tail`、`wc`、`stat`、`file`、`jq`、`sed`、`awk`、`sort` 等，
  以及 `cd`/`echo`/`pwd` 这类无副作用段），且没有任何一段把退出状态当断言用
  （`grep -q`/`--quiet`、`jq -e`/`--exit-status`）。探查段失败表示「没找到 / 答案是否」，
  不是「工作失败」。
- **验收检查（acceptance check）**：至少一段是真正的检查——项目测试/构建/lint/typecheck、
  治理 validator、专用 verifier 脚本、显式 shell 断言（`test`、`[`、`[[`、`fail=0 … exit $fail`），
  或带 `-q`/`-e` 的机器断言查询。
- **边界情况**：
  - `ls x && pnpm test`（混合命令）按验收处理：只要有验收段，整条命令的失败就是验收失败。
  - `cat package.json && ls vitest.config* && grep -rn lightModeAudit src` 是纯探查：
  参数里出现 `vitest` 字样不把命令变成测试。
  - 写操作（`sed -i`、`rm`、`tee`、重定向、依赖安装、后台启动）先于探查判定，
  仍然是 mutation / background，不会被探查规则洗白。
  - `grep -q` 是断言而不是探查：它的退出状态就是结论。

## R0 结论（合成 fixture + 代码路径）

历史命令形状为 `cat … && ls <不存在的 glob> && grep -rn …`：`ls` 失败使整条命令退出非零。
当时把这条命令认成「检查」的来源是**子串匹配**：`is_project_test_command` 只看整串里有没有
`vitest` 之类字样，`ls vitest.config*` 于是被当成测试命令；一旦判定成 verification-like，
`CompletionGate` 就把它写进 `failed_verifications`，并给出
`检查 <scope> 没有通过；修复后请重跑 <command>` 的 blocker。

**复现结果**：按当前 `main` 的代码，该命令形状已不再被判成检查——
`classify_command` 里的测试识别已改成按 shell 段首匹配（`invokes_project_test_command`），
修这条线的是 PR #611（`fix(gate): exploratory commands never block completion…`，
规格 `completion-gate-honesty.md` 的 CF-GATE-R1）。合成 fixture 下该命令得到
`ToolKind::ReadOnly`，失败不进入 `failed_verifications`，同 gate 内一次通过的验收检查即可完成。

因此本任务的 R0 结论是「已修复」，工作分两部分：

1. **锁定**：`a_read_only_probe_that_mentions_a_runner_stays_a_probe` 与
   `a_failed_read_only_probe_never_opens_a_must_pass_check` 把「含 runner 字样的探查」
   钉在 `ReadOnly` 且不产生 blocker；`a_mixed_probe_and_check_command_is_an_acceptance_check`
   钉住混合命令仍按验收处理；`a_failed_acceptance_check_still_blocks_until_it_passes`
   钉住真实失败仍然阻断。
2. **结构兜底（本轮新增）**：`command_is_read_only_probe` 按 shell 段结构判定纯探查
   （每段都是查询动词、且没有 `-q`/`-e` 这类断言标志），在 `classify_command` 中先于
   verification 分支短路，并在 `record()` 里作为同一结构守卫——这样即使将来有人再往
   关键词识别里加词，纯探查也不会被提升成「必须重跑通过的检查」。混合命令含验收段，
   不满足「每段都是查询动词」，因此不会被兜底洗白。

## Applicable Harnesses

Spec Harness；Compatibility Harness（既有 `CompletionEvidence` / `failed_verifications` /
`gate_events` 语义不变，只收紧 blocker 的产生条件）；AI Collaboration Harness。

本任务无 UI、无发布面、无请求体或对象存储变更，故不触发 Viewport / Release / Payload Harness。

## 测试矩阵

| 场景 | 断言 |
| --- | --- |
| 纯探查（含 `vitest` 字样）失败 + 验收全绿 | 分类为 `ReadOnly`；`evidence().completed == true`，无 `failed_verification` blocker |
| 探查失败后门禁单独看 | 不开出「必须重跑通过」的检查 |
| 验收检查失败 | 仍不完成，且 blocker 仍要求重跑该命令；重跑通过后才完成 |
| 混合命令 `ls x && pnpm test` | 分类为 `Verification`；失败仍阻断 |
