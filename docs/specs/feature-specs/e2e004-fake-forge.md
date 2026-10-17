# E2E-004 第一段：本地 fake-forge 端到端自动化（L2，无真实桌面）

## 背景
- 2.0 门禁③要求 docs/testing/scenario-registry.json 的 11 个 complex_e2e_cases 达到自动化。E2E-004「长任务中追加约束并完成完整交付链」目前 automation_status=designed、零自动化，是缺口最大的一个。
- 原负责的 Codex 长任务（docs/long-tasks/scenario-test-completion.md）自 2026-09-09 未更新；本单由 CodeFactory 接手 E2E-004 的 L2 部分。动手前先读该长任务记录与 registry 中 E2E-004 定义，复用已有的 fake-forge / headless runner / receipt 基础设施（#568 已映射的 delivery fake-forge 测试、tools/governance/scenario_case_receipt 等），不要另起一套。

## 需求（只做不需要真实桌面的 L2）
- R1 合成 fixture：一个带预存未提交改动（dirty file）的 git 仓库、一条会失败的测试、确定性的 provider 轨迹、可控的本地 fake forge（PR/CI/merge/artifact 的本地实现，CI 第一次失败、重试后成功）。不访问 GitHub、不触网。
- R2 按 registry steps 跑通：用户一次授权 → 运行中追加一条验收约束和一次方向纠正（通过同一会话 steer/queue）→ 首次验证失败后自动诊断修复并复验 → 在 fake forge 创建 PR → CI 失败转成功 → 合并 → 生成与合并 commit SHA 绑定的 synthetic artifact receipt。
- R3 注入故障：provider 重连、改动后测试失败、CI 瞬时失败、预存 dirty file。
- R4 断言 registry oracles 的 L2 可测部分：同一 objective / delivery run / change set / PR identity；预存改动未被覆盖；无重复 PR/重复发布；等 CI 时释放本地执行 owner、恢复后续同一 run；缺测试/PR/CI/merge/artifact 任一项不得 completed。UI 顺序可见这一条属 L3，记为 remaining gap。
- R5 以 nightly 方式接入（沿用现有 nightly / scenario-execution 机制），同时提供本地一条命令可复跑；产出匿名 receipt。
- R6 更新 scenario-registry.json：E2E-004 的 automated_by、automation_status（按实际达到的程度，不得虚报为 implemented）、remaining_gaps（保留 L3/L4 与 manual_canary）。注意：registry 属信任根，scenario-gate-pr 的 cannot self-modify 由编排方走规则集流程，不要为绕过它改门禁。
- R7 先失败：在实现前先让 nightly 目标因缺失实现而失败，记录证据。

## 约束
- 合成 fixture，不写生产 DB，不访问真实 GitHub。
- 结束时在 docs/long-tasks/scenario-test-completion.md 追加一段进展（中文），注明 E2E-004 L2 已由 CodeFactory 接手完成到什么程度。

## E2E-004 第二段：把剩余 L2 串成一条端到端，并登记到场景注册表

### 前置
- 第一段已合并（#701，b988a7de）：`rust:e2e004_ci_failure_recovers_same_pr_and_artifact` 覆盖 FakeForge 上 CI 失败→成功、跨进程恢复同一 PR、合并、artifact 绑定合并 SHA、预存 dirty file 不被覆盖。
- 读：docs/specs/feature-specs/e2e004-fake-forge.md、docs/long-tasks/scenario-test-completion.md（含第一段追加的进展）、registry 中 E2E-004 定义。

### 需求
- R1 在同一条端到端里串起第一段未组合的部分：运行中追加一条验收约束 + 一次方向纠正（通过同一会话的 steer/queue 入口）、provider 重连、改动后测试失败→自动诊断最小修复→复验；断言 durable objective / delivery run / change set / PR identity 自始至终相同，等 CI 时释放本地执行 owner、恢复后继续同一 run。
- R2 本单**明确授权修改** `docs/testing/scenario-registry.json`：为 E2E-004 增加 automated_by 与对应的硬门禁声明（参照 E2E-007 等已有条目的 `pull_request_gate` / nightly 结构，满足 validate_scenario_test_governance.py 的 hard-gate 要求），automation_status 按实际只能是 `partially_implemented`；remaining_gaps 如实列出仍缺的 L3/L4（UI 顺序可见、manual_canary）。本地跑 validator 通过；scenario-gate-pr 的 cannot self-modify 由编排方处理。
- R3 删除第一段误提交的 `docs/long-tasks/e2e004-pr-body.md`（PR 正文草稿不应进仓库）。
- R4 先失败；在长任务记录追加第二段进展。

### 约束
- 合成 fixture，不访问真实 GitHub，不写生产 DB。不放宽任何 validator。

- 使用已有 `delivery_scenario_tests.rs` 的真实 Git、SQLite FakeForge、隔离跨进程 worker 和 production `deliver` 入口，不引入第二套 forge。
- 模板 §6 禁止修改 registry、validator 和 workflow，与 R6 的直接修改冲突；保持信任根不变，PR 记录 Spec Amendment，供编排方决定后续可信接入。
- 现有 nightly 已执行全 Rust workspace；新增非 ignored Rust 回归自动包含于该步骤。PR required 的 exact target 注册仍须编排方批准更新 registry。
- Harness：Spec、AI Collaboration；本任务无 UI/安装包变更，不将本地 fake-forge 视为 L3/L4 或真实发布验收。
