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

- Current phase: M0 文档单源化与实施方案物化
- Current checkpoint: 最新 `origin/main` 的 registry 有 27 个 Scenario、11 个 Complex E2E、26 个 remaining gaps；10 个 case 为 `partially_implemented`、1 个为 `designed`，完整 `implemented` 为 0；PR slice 为 10 implemented、0 partial、1 missing。
- Next owner: 主实现完成 M0 红转绿和第一批 PR；独立 QA 复核 receipt/fixture contract；后续 M1 以 E2E-001 做纵向切片。
- Updated at: 2026-09-02

## Completed Items

- 已核对线上主分支规则集：`scenario-gate-pr` 等 6 个 required checks 为 strict、active、无 bypass actor。
- 已核对 registry 与现有自动化：27/11 的 schema v2 validator 和 57 项治理单测通过，但 release/PR readiness 对未补全 case 仍 fail closed。
- 已确认文档漂移：README 仍写 19/7，规格同时出现 26/27，repo profile 仍称发布未启用。
- 已制定 M0-M7 顺序、可组合 Scenario World、case receipt schema v2、stage-aware oracle 和 trust-root bootstrap 边界。
- M0 已先加入失败优先文档契约；旧文档稳定产生 4 个缺 marker/过期错误，证明红灯有效。

## Remaining Items

- M0：补齐 registry 派生块、修正文档历史状态，完成 catalog/governance/baseline 全量验证和 PR。
- M1：扩展现有 execution receipt，以 E2E-001 建立 case receipt、fixture digest、五类 stage-aware oracle、隐私扫描、失败 receipt 留存和 hard-kill cleanup；完成桌面驱动 feasibility probe；随后通过 Bootstrap-1 把 planner/executor/verifier、canonical driver/fixture/oracle digest 接入可信门禁，并在证据满足后补齐 E2E-004 PR slice。
- M2：补 E2E-001/002/003/007/011 的真实 WebView、旧 schema、停止/恢复/停泊 UI 和 exact release canary。
- M3：补 E2E-010 的二进制 hard-kill nightly、isolated CodeFactoryDev required canary 与安装版单消息 canary。
- M4：补 E2E-004/009 的 fake forge、完整交付链、worktree reservation CAS hard kill 和双会话并发。
- M5：补 E2E-005/008 的 failure/retry matrix、MV3 lifecycle、真实 Dev 断线续接和扩展升级/restart。
- M6：补 E2E-006 的真实 Windows N→N+1、旧进程锁/WAL/安装中断/首次 reconciliation 和桌面投影。
- M7：通过 Bootstrap-2 提升最终 registry/target/status，闭合 nightly/release delegated script 与 exact-artifact 信任链，恢复并对账所有 required checks，执行最终 release probes。

## Blockers

- M0 当前无实现 blocker。
- 两次 external governance bootstrap 都涉及临时控制门禁，执行前必须取得用户明确审批；普通候选 PR 不能修改 trust root 后使用自己的 judge 自证。
- 当前 trust root 保护 target 名称与执行工作流，但尚未完整保护候选分支中的 delegated script、scenario driver 和 oracle verifier；M1/M7 必须闭合这个空跑风险，未闭合前不能把 exact-head outcome 称为可信完整 E2E。

## Evidence

- Local evidence: 任务开始基线 `origin/main=088847de56a05174e5189abddda94b071ebca60e`；合并 #500/#501 后刷新到 `ab24bc1b`，registry 为 27 Scenario、11 Complex E2E、26 gaps，catalog checker 立即拒绝三处旧派生块；最初 failure-first 运行因 4 个受管块缺失而失败。
- Release evidence: 最近公开 release 与 nightly 证明现有局部 target 可运行，但不等于 11 个完整 case 已实现；每个后续版本仍须记录 tag、asset digest、installed executable 和真实主路径。
- Blocking evidence: 11 个完整 case 仍无一为 `implemented`；E2E-004 仍无 PR slice。pull request 与 release readiness 的准确错误数以最新 registry 命令输出为准，不复用任务开始时的旧统计。

## AI Collaboration

- context scope: registry、scenario validator/runner、PR/nightly/release workflow、现有 smoke、桌面验证脚本和公开交付证据；不读取生产聊天正文、真实 session/objective ID 或凭据。
- assumptions: 沿用同一 Harness；低层 slice 不等于完整 case；fixture 只用 synthetic data；候选自报 outcome 不构成可信 oracle。
- review point: QA 复核 M0/M1 contract；桌面技术评审决定 feasibility probe；治理评审划分普通 PR 与 external bootstrap，并审计 target implementation digest。
- validation result: 计划已物化；M0 正在红转绿。后续里程碑只有在相应命令、CI、artifact 与真实主路径证据落档后才能标记完成。

## Stop Boundary

- 不在文档完成、本地绿色、PR 通过、merge、公开资产或安装成功任一单点停止。
- 每个里程碑只能在其 stage-required oracle、cleanup 和 identity 同时成立后结束；否则继续推进或记录有证据 blocker。
- 任务最终只在 11/11 case implemented、remaining gaps 清零、trust root/ruleset 对账和 exact release 主路径验证全部完成时停止。
