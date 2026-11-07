# M266：审批文案要说出真实原因，「是不是 Git 操作」只看可执行位置

## 现场（2026-11-08）

- v1.105–v1.109 上，只要命令里有 `git` 字样或者解析器没看懂，网关就落进
  `trusted.git.unproved`「无法证明 Git 操作仅影响本会话已登记工作区」，于是一条只读的
  `grep -n "git push" …`、一个 `for` 循环、一个 `BASE=$(git rev-parse …) && …` 都停下来要审批。
- 用户看到的是「Git 操作无法证明」，而实际上**没有任何 Git 写操作**；真实原因要么是「命令没被
  展开」，要么是「引号里的字只是数据」。文案错位让用户无法判断该批准什么。
- 同一批命令里还叠加了 shell 破坏性短语扫描按原始文本匹配的问题：搜索模式里的
  `git reset --hard`、提交信息里的同类文本也会触发高危审批。

## 决策

1. **文案按真实原因分流**（`git_entry()` 返回 `NotGit | ReadOnlyGit | GitWrite | Unprovable`）：
   - 可执行位置里确实有 Git 写/出网 → `trusted.git.unproved`（保留原文案）；
   - 命令无法展开成简单命令 → `trusted.shell.unproved`（说明是循环/子命令/嵌套结构超出静态展开）；
   - 其余不安全片段 → `shell.safety`（说明有片段无法静态证明为只读）。
2. **Git 性只看可执行位置**：解析后的命令位是 `git` 才算 Git 操作；参数文本、搜索模式、变量名、
   路径里出现 `git` 一律当数据。内联解释器载荷（`bash -c "…"`、`python -c "…"`）是代码，
   递归按同一规则判定。
3. **能展开就展开**：引号数据、heredoc 数据体、`for`/`while` 循环（值可枚举时按循环变量绑定展开）、
   `$(…)` 命令替换（分别证明内层与外层）、`NAME=value` 赋值前缀、管道与重定向都必须被拆成
   简单命令；只有真正无法展开的结构才落 `Unprovable`。
4. **破坏性短语只判会被执行的部分**（`shell_judged_text`）：引号跨度不参与扫描，除非它是解释器
   的内联载荷。
5. **不放宽门禁**：真实 `git push`/`--force`/`reset --hard`/`clean -fdx`、`bash -c "git reset --hard"`、
   真正没法解析的命令、`rm -rf` 仍然要审批（R5）。

## Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| R1 | 审批文案按真实原因分流：Git 写仍为 `trusted.git.unproved`；无法展开为 `trusted.shell.unproved`；其他为 `shell.safety` | `unproved_git_reason` 分流函数 + `m266_unproved_reasons_stay_distinct`（两条规则、两段文案互不复用）+ `m266` 表 |
| R2 | Git 性只看可执行位置；引号数据、模式、路径里的 `git` 不算 Git 操作 | `m266_not_git_operations_are_not_sensitive`（含 `grep -o "path:…"`、`grep -rn "git push origin main"` 两行现场样本） |
| R3 | 引号、heredoc 数据体、循环、命令替换、赋值前缀、管道/重定向都被展开；只有真解析不动才 `Unprovable` | `m266_shell_safety_ignores_quoted_data_but_keeps_real_commands`、`m266_message_heredoc_survives_output_pipeline`、`m266_interpreter_heredoc_is_still_unproved` |
| R4 | 破坏性短语扫描只判会被执行的部分，内联解释器载荷仍算代码 | `m266_shell_safety_ignores_quoted_data_but_keeps_real_commands` 的 safety 行 |
| R5 | 真实 Git 写、无法展开的命令、`rm -rf` 仍然要审批 | `m266_real_git_operations_stay_sensitive`（`git push`、`--force`、`reset --hard`、`clean -fdx`、`bash -c "git reset --hard"`、未终止引号、反引号、`rm -rf`） |

## Applicable Harnesses

- **Spec Harness**：本文件 + PR body 的 Req↔测试映射，`Scenario-Test: UI-009`。
- **Compatibility Harness**：`sensitive()` 的对外语义不变（仍是「是否需要审批」），新增的
  `git_entry()` 只用于文案分流；旧用例 `m172_*`、`m139_*`、`m159_*` 全部保持通过。
- **Observation Harness**：审批卡片的 `rule` 字段从单一 `trusted.git.unproved` 变为三类，
  可据此统计误判来源。

## 测试矩阵

| 场景 | 层级 | 断言 |
| --- | --- | --- |
| 引号内的 `git` 字样 / 搜索模式 | Rust 单元 | `sensitive == false`，`git_entry == NotGit` |
| `for i in 1..6; do out=$(cargo test); echo "run $i: done"; done` | Rust 单元 | 可展开，`sensitive == false` |
| `BASE=$(git rev-parse origin/main) && python3 …` | Rust 单元 | 只读 Git，`sensitive == false` |
| 消息 heredoc 走 `|` 管道 | Rust 单元 | 数据体不当命令，`sensitive == false` |
| R1 文案分流 | Rust 单元 | `GitWrite → trusted.git.unproved`、`Unprovable → trusted.shell.unproved`，且展开文案不得出现 `Git` 字样 |
| 解释器 heredoc（`bash <<'EOF'`） | Rust 单元 | 仍 `Unprovable` → 审批 |
| `git push` / `--force` / `reset --hard` / `clean -fdx` / `bash -c "git reset --hard"` / 未终止引号 / 反引号 / `rm -rf` | Rust 单元 | 全部仍要审批 |
| 全量回归 | Rust 单元 | `cargo test --lib`（`trusted_git` 42 项、`shell_policy` 13 项、全库） |

## 证据（2026-11-08）

- `cargo test --manifest-path src-tauri/Cargo.toml --lib agent::m266` → `7 passed; 0 failed`。
- 场景门禁：`python3 tools/governance/run_scenario_harness_gate.py --stage local --repo . --policy-repo .` → `scenario-harness-gate: OK`；UI-009 的 `automated_by` 指向真实存在的 6 个 `m266_*` 函数与 `ToolCallCard.workspace.test.tsx`。
- 治理基线：`python3 tools/governance/validate_repo_governance_baseline.py` 通过。
- 完成声明边界：本文件记录的是静态入口（可执行位置）与文案分流的机器证据；真实会话里的“同一命令不再弹 Git 审批”需在桌面主路径复核，未在本轮声明为已验证。
