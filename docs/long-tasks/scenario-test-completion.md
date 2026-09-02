# 场景测试完整补全长任务记录

## Basics

- Task ID: CF-SCENARIO-COMPLETION-20260902
- Title: 统一场景 Harness 的 11 个复杂 E2E 补全与可信门禁闭环
- Feature spec: `docs/specs/feature-specs/scenario-test-governance.md`
- Related Req IDs: CF-STG-R1 至 CF-STG-R31

## Completion Standard

- Done means: registry 中 27 个 active Scenario 全部绑定可执行 target；11 个 Complex E2E 均为 `implemented`，`remaining_gaps=0`，PR slice 11/11 implemented；PR/nightly/release 的 stage-required oracle、cleanup、exact SHA/artifact identity 和真实 L3/L4 证据全部通过；trusted implementation digest 与线上 ruleset 已经外部 bootstrap 并复核。
- Blocked means: 已完成所有不依赖外部控制面的实现与验证，但 external governance bootstrap、Windows/macOS runner、公开签名产物或受管 Chrome 环境连续无法取得；必须保留失败 receipt、准确的 `not live` 边界和下一条可执行动作。

## Current State

- Current phase: M1a case receipt v2 与 synthetic fixture manifest 基础合同
- Current checkpoint: M0 已由 PR #502 合入 `main`；M1a 正在普通 PR 中落地不参与 required judge 的 receipt verifier、stage-aware oracle、隐私/cleanup fail-closed 合同和 E2E-001 synthetic fixture。registry 仍保持 27 个 Scenario、11 个 Complex E2E、26 个 remaining gaps，不提升状态。
- Next owner: 主实现完成 M1a 普通 PR；随后以独立普通 PR 让 E2E-001 binary 输出结构化 raw observation；取得用户明确批准后再执行 Bootstrap-1，把既有 foundation 纳入 trusted plan/executor/verifier。
- Updated at: 2026-09-02

## Completed Items

- 已核对线上主分支规则集：`scenario-gate-pr` 等 6 个 required checks 为 strict、active、无 bypass actor。
- 已核对 registry 与现有自动化：27/11 的 schema v2 validator 和 57 项治理单测通过，但 release/PR readiness 对未补全 case 仍 fail closed。
- 已确认文档漂移：README 仍写 19/7，规格同时出现 26/27，repo profile 仍称发布未启用。
- 已制定 M0-M7 顺序、可组合 Scenario World、case receipt schema v2、stage-aware oracle 和 trust-root bootstrap 边界。
- M0 已先加入失败优先文档契约；旧文档稳定产生 4 个缺 marker/过期错误，证明红灯有效。
- M0 已通过 PR #502 合入 `main`：registry 派生摘要、分类和 case 表现在由 candidate-side governance check 阻断漂移。
- M1a 已先加入失败优先测试：在实现模块不存在时稳定因 `ModuleNotFoundError` 失败；独立评审给出的伪绿反例已继续扩成 28 项 receipt/fixture 合同测试。

## Remaining Items

- M1a：以普通 PR 合入独立的 case receipt v2 verifier、fixture manifest digest、五类 stage-aware oracle、递归隐私扫描、失败 receipt 留存和 E2E-001 synthetic fixture；该实现暂不参与 required judge。
- M1b：以普通 PR 让 E2E-001 正式 binary 输出结构化 raw observation，同时保留 execution receipt v1 和 legacy smoke 字段；修复失败路径 cleanup 证据、hard-kill supervisor marker 与 target receipt 临时目录回收。
- Bootstrap-1：取得用户明确审批后，把已在 `main` 的 planner/executor/verifier、canonical driver/fixture/oracle digest 接入可信门禁，并在证据满足后补齐 E2E-004 PR slice。
- M2：补 E2E-001/002/003/007/011 的真实 WebView、旧 schema、停止/恢复/停泊 UI 和 exact release canary。
- M3：补 E2E-010 的二进制 hard-kill nightly、isolated CodeFactoryDev required canary 与安装版单消息 canary。
- M4：补 E2E-004/009 的 fake forge、完整交付链、worktree reservation CAS hard kill 和双会话并发。
- M5：补 E2E-005/008 的 failure/retry matrix、MV3 lifecycle、真实 Dev 断线续接和扩展升级/restart。
- M6：补 E2E-006 的真实 Windows N→N+1、旧进程锁/WAL/安装中断/首次 reconciliation 和桌面投影。
- M7：通过 Bootstrap-2 提升最终 registry/target/status，闭合 nightly/release delegated script 与 exact-artifact 信任链，恢复并对账所有 required checks，执行最终 release probes。

## Blockers

- M1a 当前无实现 blocker。
- 两次 external governance bootstrap 都涉及临时控制门禁，执行前必须取得用户明确审批；普通候选 PR 不能修改 trust root 后使用自己的 judge 自证。
- 当前 trust root 保护 target 名称与执行工作流，但尚未完整保护候选分支中的 delegated script、scenario driver 和 oracle verifier；M1/M7 必须闭合这个空跑风险，未闭合前不能把 exact-head outcome 称为可信完整 E2E。

## Evidence

- Local evidence: 任务开始基线 `origin/main=088847de56a05174e5189abddda94b071ebca60e`；M0 合入后 `main=778990c18ae1200c45d9acb304d86e406a164922`。M1a failure-first 运行因缺少 `tools.governance.scenario_case_receipt` 失败；独立 QA/治理评审随后实证发现 stage 降级、runner/fixture 未绑定、oracle observation/reason 未精确绑定、常见凭据形状与隐私自由文本、弱 hard-kill 与 release identity 伪绿，修复后 28 项合同测试覆盖这些反例。`evidence_sha256` 在 M1a 只是不透明证据投影的完整性字段：公开 `run_id` 自哈希不能证明候选证据真实性；只有 Bootstrap-1 由默认分支 trusted builder 从原始执行证据复算并绑定后，才可作为可信 gate 证据。
- Release evidence: 最近公开 release 与 nightly 证明现有局部 target 可运行，但不等于 11 个完整 case 已实现；每个后续版本仍须记录 tag、asset digest、installed executable 和真实主路径。
- Blocking evidence: 11 个完整 case 仍无一为 `implemented`；E2E-004 仍无 PR slice。pull request 与 release readiness 的准确错误数以最新 registry 命令输出为准，不复用任务开始时的旧统计。

## AI Collaboration

- context scope: registry、scenario validator/runner、PR/nightly/release workflow、现有 smoke、桌面验证脚本和公开交付证据；不读取生产聊天正文、真实 session/objective ID 或凭据。
- assumptions: 沿用同一 Harness；低层 slice 不等于完整 case；fixture 只用 synthetic data；候选自报 outcome 不构成可信 oracle。
- review point: QA 复核 M0/M1 contract；桌面技术评审决定 feasibility probe；治理评审划分普通 PR 与 external bootstrap，并审计 target implementation digest。
- validation result: 计划已物化，M0 已合并，M1a 本地合同红转绿；foundation 尚未接入 trusted required gate，registry 状态和 L3/L4 缺口保持不变。

## Stop Boundary

- 不在文档完成、本地绿色、PR 通过、merge、公开资产或安装成功任一单点停止。
- 每个里程碑只能在其 stage-required oracle、cleanup 和 identity 同时成立后结束；否则继续推进或记录有证据 blocker。
- 任务最终只在 11/11 case implemented、remaining gaps 清零、trust root/ruleset 对账和 exact release 主路径验证全部完成时停止。
