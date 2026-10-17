## 问题与结论

失败终态摘要会把「工作区里的未提交修改」写成「改动已经提交在本机的分支 … 上（还没推送）」。根因是观测范围错误：未推送提交数用 `git rev-list --count HEAD --not --remotes` 统计整条分支历史上所有不在远端的提交（含与本任务无关的本地提交），再与 `base_sha..HEAD` 的提交数拼接，就能在本任务一次提交都没有时得出「已提交、未推送」。

修复后摘要只陈述实测事实，并把三件事分开写：未提交修改（文件数）、本地未推送提交（数量 + 最新提交标题）、已推送分支/PR。有未提交修改时会明确写出「这些改动还没提交，清理工作区就会丢失」并给出工作区路径，同时说明系统不会自动提交、只是保留工作区。closeout 遇到未提交内容保留整个工作区，原因文字改为用户可读（含文件数与路径）。

## Req ID ↔ 测试

| Req | 变更 | 证据 |
| --- | --- | --- |
| R0 | 合成 fixture 复现真机状态（工作区改动、零提交），断言观测器与摘要一致 | `agent::failure_summary::tests::m46_synthetic_repo_reports_working_tree_only`（临时 git 仓库，无生产 DB） |
| R1 | `PreservedWork` 新增 `uncommitted_files` / `latest_unpushed_subject`，`change_status` 只在实测到提交时才声明提交；新增 `commit_status_lines` 单列未推送数量与标题 | `m46_local_commits_name_the_count_and_newest_title`、`m46_pushed_commits_are_not_reported_as_unpushed`、`m46_mixed_state_lists_all_three_facts_separately` |
| R2 | 未提交时输出「还没提交，清理工作区就会丢失」+ 工作区路径 + 系统保留说明 | `m46_uncommitted_only_never_claims_a_commit`（断言不出现「已经提交」「没有丢掉」） |
| R3 | closeout 脏工作区保留并写入用户可读原因 | `agent::execution_workspace::tests` 脏工作区用例新增断言：状态仍为 `cleanup_pending`、`workspace_cleanup_dirty`、原因含「没提交」与工作区路径 |
| R4 | 四种状态各一条测试 | 上述四个 `m46_*` 用例 |

失败优先：新增的 `m46_*` 用例与 `uncommitted_files` 字段在实现前不存在，首次编译/运行必然失败；实现后同一批命令转绿。

## 验证

- `CARGO_TARGET_DIR=.codefactory-cache/cargo-target-m46 cargo test --manifest-path src-tauri/Cargo.toml --lib --no-fail-fast` → 1623 passed / 0 failed。
- `pnpm test` → 151 files / 1013 tests passed。
- `pnpm build` → 成功。
- `python3 tools/governance/validate_scenario_test_governance.py --ci --base-ref origin/main …` → OK，未报告受影响场景。

## Applicable Harnesses

- Spec Harness：`docs/specs/feature-specs/m46-honest-failure-summary.md`（含 Requirements Traceability）。
- Compatibility Harness：只读 git 观测，无 DB 迁移；旧会话按现有行渲染，缺观测时显示「无法确认」而不是编造。
- Observation Harness：摘要文本与 closeout 原因都来自 `git rev-list` / `git status` 实测，并有合成仓库用例断言两者一致。
- Payload Harness：不涉及。
- Viewport Harness：不涉及（无界面结构变更，文案由后端摘要渲染）。

## AI Collaboration

- context scope：`src-tauri/src/agent/failure_summary.rs`、`src-tauri/src/agent/objective.rs` 的 `collect_preserved_work`、`src-tauri/src/agent/execution_workspace.rs` 的 closeout 脏工作区分支。
- assumptions：`execution_workspaces` 记录的 `base_sha` 是工作区起点；未推送提交必须限定在该范围内才代表本任务。
- review point：`change_status` 在 `commits_ahead == Some(0)` 时永不声称有提交；缺失观测只说「无法确认」。
- validation result：Rust 全量库测试、前端测试与构建全绿，合成仓库用例证明观测与摘要一致。

## 真实应用验收

本任务无界面结构变更；真机安装版核对由编排方发版后完成：制造一次失败任务（工作区有未提交改动、无提交）→ 摘要应写「改动仅在工作区，未提交」+「清理工作区就会丢失」+ 工作区路径，且不出现「已经提交」。

README-Update: reviewed
README-Update-Reason: 用户可见的失败摘要文案更诚实，但不改变产品承诺或安装方式，README 无需更新。
