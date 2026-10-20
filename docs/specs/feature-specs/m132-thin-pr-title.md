# M132 thin 交付的 PR 标题与摘要

## 背景与用户可见问题

thin `deliver_changes` 在调用方没有给 `title` 时，用 `fix: <首个暂存路径>` 兜底；没有暂存路径（提交已存在、重试交付）时路径为空，兜底退化成 `fix: <分支名>`。于是 PR 标题成了 `fix: codefactory/objective-8af68c03794255ec2beef940d3d42524`，正文直接以 `Scenario-Test:` 开头、没有一句「做了什么/为什么」。本仓库 squash 合并且发版 slot 按标题前缀计算，这个标题会原样进入 `main` 的提交历史与发版说明（10-21 PR #782，编排方手动改标题；10-17 PR #605 同族，M58 已修过一次标题通胀方向）。

## 证据

- 10-21 PR #782（阶段 2 legacy 控制器守卫）：标题 `fix: codefactory/objective-8af68c03794255ec2beef940d3d42524`，正文以 `Scenario-Test:` 开头，无摘要。
- 10-17 PR #605：合并标题成 `objective b8af…`（M58）。
- 现有代码：`src-tauri/src/tools/delivery_thin.rs` 的 `run()` 中 `format!("fix: {}", paths.first().map(String::as_str).unwrap_or(&branch))`。

## 决策

- 标题来源优先级：模型显式 `title`（且是真实的 Conventional Commit 描述）> 本分支最近提交里首个有意义的 subject > 已存在 PR 的可用标题 > 报错不推送。
- 「分支名回声」判定：标题正文为空、等于分支名、包含分支名、含 `codefactory/objective`，或形如 `objective <hex>`，都算占位标题。
- 正文摘要：正文开头不是散文（以标题、`Scenario-Test:` 一类声明行开头，或为空）时，用最新提交正文首段作为摘要，其次用标题中的描述句；摘要总是排在必填声明行之前。
- 已存在 PR 时只在候选标题/正文确实更好或不同才更新，永不把可用标题改回分支名。

## Requirements Traceability

| Req ID | 需求 | 最低证据 |
|---|---|---|
| R1 | 标题缺失、为空、等于或包含分支名时，从本分支最近提交的首个有意义 Conventional Commit subject 生成；仍无法生成则返回错误且不提交、不推送 | `missing_title_uses_first_meaningful_commit_subject`、`branch_echo_title_falls_back_to_the_commit_subject`、`no_title_and_no_meaningful_subject_errors_without_push` |
| R2 | 正文开头必须是 1–3 句「做了什么/为什么」摘要（模型输入；缺失时用提交正文首段），必填声明行放在其后 | `thin_body_gets_leading_summary_from_the_commit_body`、`existing_summary_is_kept_and_title_claim_is_the_last_resort`、`contract_lines_are_not_a_summary` |
| R3 | 已有 PR 时，更好的标题/摘要会更新；不得把好标题改回分支名 | `reused_pr_keeps_a_usable_title_instead_of_the_branch_name`、`better_title_updates_the_existing_pr` |
| R4 | 测试先行，四个验收场景先失败后通过 | 本节测试名在实现前各自红，实现后全绿；`cargo test` 全量回归 |

## Applicable Harnesses

- Spec Harness：本文件即规格，逐条对应 Req ID 与测试名。
- Compatibility Harness：只改 thin 交付的标题/摘要推导，不动 durable 交付、合并队列、发版脚本；`common` 回归覆盖既有 thin 交付路径（重推、复用 PR、arm、治理预检）。
- Release Harness：仅合并，不执行发布（not live），发版由编排方的发布任务完成。
- Observation Harness：合成 forge 记录 `pr create` / `pr edit` 的实际 title 与 body，断言落在这些真实调用参数上。
- 不涉及 UI、上传/载荷、移动端布局，故不适用 Viewport Harness 与 Payload Harness。

## 测试矩阵

| 场景 | 输入 | 期望 |
|---|---|---|
| 无标题 + 有提交 | 无 `title`，提交 subject `feat: …` | PR 标题取该 subject |
| 标题等于分支名 | `title` 含 `codefactory/objective-<hex>` | 回落到提交 subject |
| 两者都无 | 无 `title`，提交 subject 也是占位 | 返回错误，`0` 次 push、`0` 次 `pr create` |
| 已有 PR 且标题可用 | 复用 PR，候选标题是分支名回声 | 不 `pr edit` 回退，沿用可用标题 |
| 已有 PR 且候选更好 | 复用 PR，候选是真实 subject | `pr edit` 更新标题 |
| 正文无摘要 | 正文以 `Scenario-Test:` 开头 | 摘要前置于声明行之前 |
| 正文已有摘要 | 正文首行是散文 | 不改写、不重复前置 |

## Implementation Notes

- 标题与摘要推导都放在 `src-tauri/src/tools/delivery_thin.rs` 与 `src-tauri/src/agent/delivery.rs` 的既有 thin 路径内，不新增并行规则副本。
- 标题解析发生在 `git commit` 之前：无法生成标题时连提交都不产生，避免把占位标题写进历史。
