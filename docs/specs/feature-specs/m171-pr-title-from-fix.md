# M171：交付工具生成 PR/合并标题时优先 fix/feat，不被末次 test/docs 提交带偏

## 证据
- #854（M163）含 execution_workspace.rs/objective.rs/chat.rs 的修复代码，标题却是 `test: 争用写与分叉恢复用例…`；#858（M162 一部分）标题 `test:`；10-22 #828 同样被带成 `test:`（已人工改）。标题前缀决定是否触发发版（只有 feat/fix 发版），带偏会让修复不出版本。
## 需求
- R1 生成标题时综合整个 PR diff：只要改动包含产品代码（非测试/文档），前缀取 fix 或 feat（按规格或提交里最高级别的那个），不取末次提交的前缀。
- R2 纯测试/文档 PR 才用 test/docs。
- R3 squash 合并标题与 PR 标题一致；Release-Urgency trailer 规则不变（M156）。
- R4 测试（先失败）：末次提交为 test、整体含产品代码 → fix；纯测试 → test；含 feat 规格 → feat。
## 约束
- task-template；合成 fixture；授权交付（ceiling: merged）。

Release-Urgency: default

## Implementation Notes

- 在 thin 交付入口、提交与推送之前，基于远端默认分支与工作树的完整 diff（包含未跟踪文件）决定类型；历史主题只读取本分支范围，不借用 main 上的 feat/fix。
- 显式 title、分支提交、已有 PR 提供描述；类型由完整变更与 feat 意图决定，保留 scope / breaking 标记和描述。规格中的 Conventional Commit `feat:` / `feat(scope):` 声明作为明确 feat 意图。
- 非测试/文档的配置、脚本也视为产品变更，保守使用 fix；纯测试与文档混合使用 test。
- Spec Harness：R1–R4 映射到 `m171_*` 测试。Compatibility / Observation / Release Harness：默认分支范围、PR 创建/更新、commit 与 PR 标题、既有 merge queue squash/trailer 测试。AI Collaboration：限定标题路径，使用合成 forge；按任务模板不启动 delegate_tasks；真实安装版由编排方发版后验收。
- 无 UI 修改；现有 CLI / 后台交付入口保持不变。无 Spec Amendment。

## 验证证据（2026-10-26）

- 先失败：`tools::delivery_thin::tests::m171_whole_diff_controls_title_and_commit` 首轮 `left: Some("test: cover recovery")` / `right: Some("fix: cover recovery")`，确认标题被末次提交带偏。
- 修复后 `cargo test --lib tools::delivery_thin`：22 passed / 0 failed（覆盖：末次 test 但含产品代码 → fix；纯测试 → test；纯文档 → docs；显式 feat 声明 → feat；保留 scope 与描述）。
- `cargo test --lib`：1993 passed / 0 failed / 12 ignored。
- 本地场景门禁：`run_scenario_harness_gate.py --stage local` → `scenario-harness-gate: OK (local, tool-independent policy)`，声明 `Scenario-Test: CXD-002`。
