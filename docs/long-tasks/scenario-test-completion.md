# 场景测试完整补全长任务记录

## 当前进展（通俗版）

已经修好了测试结果格式不一致、可能把不完整结果误判为通过的问题，新检查程序已合入，原来的保护已恢复。现在通过普通 PR 在 Windows 和 Mac 上实际复测，确认新检查程序能用。

所有场景还没有补完：复杂场景仍有 26 项缺口，主要在真实界面操作、浏览器异常恢复、完整交付过程和安装升级。下面的技术记录用于核对证据，不把“检查程序升级完成”说成“所有测试补齐”。

## Basics

- Task ID: CF-SCENARIO-COMPLETION-20260902
- Title: 统一场景 Harness 的 11 个复杂 E2E 补全与可信门禁闭环
- Feature spec: `docs/specs/feature-specs/scenario-test-governance.md`
- Related Req IDs: CF-STG-R1 至 CF-STG-R31

## Completion Standard

- Done means: registry 中全部 active Scenario 绑定可执行 target（数量从 registry 派生）；11 个 Complex E2E 均为 `implemented`，`remaining_gaps=0`，PR slice 11/11 implemented；PR/nightly/release 的 stage-required oracle、cleanup、exact SHA/artifact identity 和真实 L3/L4 证据全部通过；trusted implementation digest 与线上 ruleset 已经外部 bootstrap 并复核。
- Blocked means: 已完成所有不依赖外部控制面的实现与验证，但 external governance bootstrap、Windows/macOS runner、公开签名产物或受管 Chrome 环境连续无法取得；必须保留失败 receipt、准确的 `not live` 边界和下一条可执行动作。

## Current State

- Current phase: Bootstrap-1a / #508 已合入，原始规则集已恢复；非空 Windows/macOS canary 随本记录所在普通 PR 执行
- Current checkpoint: M0/#502、M1a/#503、M1b/#504 均已合入 `main`。#505 最小路由修复合入后，#504 作为全量 canary 通过 111/111 target（Windows 102、macOS 9）和全部六项 required checks，合并提交为 `05461f19c62cb90e592defd0c53fc78c4996835e`；两个已完成 worktree 已按 PR 证据回收，用户主 checkout 未修改。详见 `docs/evidence-packs/scenario-runner-bootstrap-2026-09-07.md`。
- Next owner: #508 已获本轮最小迁移授权；同步 #510/#509 后，以 head `7bbc4f2dc96551ea01d286ac3ef80922e851db2b` 通过五项普通检查，合并为 `fc2432658b87993b8af10df0cc416f594218e6ac`。约 12.1 秒窗口后，原始六项保护精确恢复，标准 verify 为 converged。下一步核验当前普通 canary 的 exact-head 非空回执与六项检查；新基线预期 113 targets（Windows 104、macOS 9）、一个 Windows E2E-001 v2 case，须由实际计划复算。通过后继续桌面可执行性验证与 E2E-004 PR slice，不重复请求或执行已完成的 #508 迁移。细节见 `docs/evidence-packs/scenario-case-trusted-bootstrap-2026-09-08.md`。
- Updated at: 2026-09-08

## Completed Items

- 已核对线上主分支规则集：`scenario-gate-pr` 等 6 个 required checks 为 strict、active、无 bypass actor。
- 已核对 registry 与现有自动化：27/11 的 schema v2 validator 和 57 项治理单测通过，但 release/PR readiness 对未补全 case 仍 fail closed。
- 已确认文档漂移：README 仍写 19/7，规格同时出现 26/27，repo profile 仍称发布未启用。
- 已制定 M0-M7 顺序、可组合 Scenario World、case receipt schema v2、stage-aware oracle 和 trust-root bootstrap 边界。
- M0 已先加入失败优先文档契约；旧文档稳定产生 4 个缺 marker/过期错误，证明红灯有效。
- M0 已通过 PR #502 合入 `main`：registry 派生摘要、分类和 case 表现在由 candidate-side governance check 阻断漂移。
- M1a 已通过 PR #503 合入 `main`：先以缺模块的 `ModuleNotFoundError` 取得失败优先证据，再以 28 项测试覆盖 receipt/fixture 合同及独立评审给出的伪绿反例；该 foundation 仍不参与 trusted required judge。
- M1b 已先加入失败优先 Rust 测试：因缺少 `scenario_case_observation` 模块稳定编译失败；实现后 3 项集成测试覆盖 legacy 字段兼容、失败 receipt 隐私和派生结果 fail closed。
- M1b/#504 已合并；正式 Windows canary 实际观察到 hard kill、worker reap、不同进程恢复、零后代/泄漏、单用户消息和零人工 prompt。执行 run `34074235813`、gate run `34074234233` 均成功；该证据只完成非 UI 切片。
- #507 已合并 canonical CLI 和真实跨平台 Skill symlink 测试：首轮 Windows 55 targets 的唯一失败为 Unix-only 目标零执行，修复后新计划 Windows 60/macOS 9 全通过；独立 QA 确认该命名测试实际 1 passed、0 ignored，并验证完整 schema v1 aggregate 无缺失、重复或多余目标。
- #508 迁移前独立 QA 发现 receipt 版本声明漂移；失败优先修复后，registry 与 direct planner 均拒绝缺失/旧版/非法类型，即使零 target 也不放行。升级现已合入，未提升任何完整 case 状态；网络中断时自动恢复保护的实际路径和成功后的完整恢复均已验证。

## Remaining Items

- Bootstrap-1a：入口、执行回执 v2、可信摘要、原始观测重算、目录清理与严格集合校验已随 #507/#508 合入；剩余收尾为完整保护下的非空 exact-head canary 及独立回执复核，不能把只读预检或零目标计划记为完成。
- Bootstrap-1 后续：可信 catalog 接入、桌面 feasibility probe、E2E-004 PR slice；只能在真实证据满足后完成相应里程碑。
- M2：补 E2E-001/002/003/007/011 的真实 WebView、旧 schema、停止/恢复/停泊 UI 和 exact release canary。
- M3：补 E2E-010 的二进制 hard-kill nightly、isolated CodeFactoryDev required canary 与安装版单消息 canary。
- M4：补 E2E-004/009 的 fake forge、完整交付链、worktree reservation CAS hard kill 和双会话并发。
- M5：补 E2E-005/008 的 failure/retry matrix、MV3 lifecycle、真实 Dev 断线续接和扩展升级/restart。
- M6：补 E2E-006 的真实 Windows N→N+1、旧进程锁/WAL/安装中断/首次 reconciliation 和桌面投影。
- M7：通过 Bootstrap-2 提升最终 registry/target/status，闭合 nightly/release delegated script 与 exact-artifact 信任链，恢复并对账所有 required checks，执行最终 release probes。

## Blockers

- #504 的 required checks 和全量 canary 已完成，不再列作 blocker。
- 两次 external governance bootstrap 都涉及临时控制门禁，执行前必须取得用户明确审批；普通候选 PR 不能修改 trust root 后使用自己的 judge 自证。
- 当前 trust root 保护 target 名称与执行工作流，但尚未完整保护候选分支中的 delegated script、scenario driver 和 oracle verifier；M1/M7 必须闭合这个空跑风险，未闭合前不能把 exact-head outcome 称为可信完整 E2E。

## Evidence

- Local evidence: 任务开始基线 `origin/main=088847de56a05174e5189abddda94b071ebca60e`；M0 合入后 `main=778990c18ae1200c45d9acb304d86e406a164922`，M1a 合入后 `main=10d834516bfd8a2a9d92a53be94bc09ee02893bf`。M1a failure-first 运行因缺少 `tools.governance.scenario_case_receipt` 失败；独立 QA/治理评审随后实证发现 stage 降级、runner/fixture 未绑定、oracle observation/reason 未精确绑定、常见凭据形状与隐私自由文本、弱 hard-kill 与 release identity 伪绿，修复后 28 项合同测试覆盖这些反例。M1b failure-first 因缺少 Rust observation 模块编译失败；实现后 3 项 Rust receipt 测试、1 项真实 detached descendant tree-sweep 测试、1 项启动正式 binary 的有界 failure-receipt 集成测试、6 项 formal smoke 结构合同、93 项 Python receipt/registry 回归与非测试 `cargo check` 通过。正式 binary 成功路径实测 hard-kill、不同 PID 恢复、单用户消息、零人工 prompt、单副作用 receipt、OS-backed tree sweep 后零 descendant/泄漏，M1a PR-stage builder 接受全部必需 oracle。自动集成测试与 `node scripts/verify-unattended-failure-receipt.mjs <CodeFactory-binary>` 均证明失败路径必须先写匿名 receipt 再以状态 1 退出；无效临时根在 Unix 上因子路径不可观察而保守记 `leaked_resource_count=1`，Windows 可确认子路径未创建而精确记 `0`，两端都必须是 `cleanup_attempted=false`、`orphan_sweep_performed=false`、`cleanup_ok=false`。由于本阶段新增 Windows process-tree 直接依赖并修改 Cargo manifests，PR 场景声明必须使用 `Scenario-Test: ALL`；这只表示全局影响覆盖，不提升 registry 状态。`evidence_sha256` 在 M1a/M1b 仍只是不透明证据投影的完整性字段：公开 `run_id` 自哈希不能证明候选证据真实性；只有 Bootstrap-1 由默认分支 trusted builder 从原始执行证据复算并绑定后，才可作为可信 gate 证据。
- Release evidence: 最近公开 release 与 nightly 证明现有局部 target 可运行，但不等于 11 个完整 case 已实现；每个后续版本仍须记录 tag、asset digest、installed executable 和真实主路径。
- Blocking evidence: 11 个完整 case 仍无一为 `implemented`；E2E-004 仍无 PR slice。pull request 与 release readiness 的准确错误数以最新 registry 命令输出为准，不复用任务开始时的旧统计。

## AI Collaboration

- context scope: registry、scenario validator/runner、PR/nightly/release workflow、现有 smoke、桌面验证脚本和公开交付证据；不读取生产聊天正文、真实 session/objective ID 或凭据。
- assumptions: 沿用同一 Harness；低层 slice 不等于完整 case；fixture 只用 synthetic data；候选自报 outcome 不构成可信 oracle。
- review point: QA 复核 M0/M1 contract；桌面技术评审决定 feasibility probe；治理评审划分普通 PR 与 external bootstrap，并审计 target implementation digest。
- validation result: M0/M1a/M1b 已合并，旧最小路由 bootstrap 已通过 111 个 target canary。Bootstrap-1a 的入口、配置、类型、隐私、重复结果和版本漂移问题均已补失败优先测试，#508 已进入默认分支 judge，原始保护恢复。新基线非空 canary 随本 PR 验证，完整 case 状态和 L3/L4 缺口保持不变。

## Stop Boundary

- 不在文档完成、本地绿色、PR 通过、merge、公开资产或安装成功任一单点停止。
- 每个里程碑只能在其 stage-required oracle、cleanup 和 identity 同时成立后结束；否则继续推进或记录有证据 blocker。
- 任务最终只在 11/11 case implemented、remaining gaps 清零、trust root/ruleset 对账和 exact release 主路径验证全部完成时停止。
