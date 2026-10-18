# 场景测试完整补全长任务记录

## CodeFactory E2E-002 L2 schema matrix（2026-10-18）

- 规格原文：`docs/specs/feature-specs/e2e002-schema-matrix.md`；R1–R4，匿名 SQLite 迁移前缀 1/6/10/19，生产 connect 自动迁移后通过正式 binary admission 发送继续。
- 失败优先：同一 integration 测试先因缺 `schema_matrix` 失败，再复现无身份旧历史错误创建新任务；修复为中文明确拒绝且事务回滚。6 保留历史 root，10/19 保留既有 Objective；重新打开无重复消息、Objective 或副作用。
- 矩阵暴露生产迁移宏路径错误：`../migrations` 指向根目录旧 1–5；改为 crate `./migrations`，真实迁移至 20。checksum mismatch 的防御性 schema sync 保留。
- 验证：源码目录外正式 binary 四行矩阵通过；focused integration 通过；完整 Rust workspace 重跑通过（首轮已有 socket accept WouldBlock，按模板重跑一次全绿）；前端 151 files / 1013 tests 和 build；scenario --ci、README、baseline、历史 Node 合同均通过。详见 `docs/evidence-packs/e2e002-schema-matrix.md`。
- registry 最小登记真实迁移单测，移除已完成旧 schema L2 gap；保持 partially_implemented、L3。cannot self-modify 交编排方，不调整保护门禁。
- 真机验收由编排方发版后完成（not live），步骤见规格和证据包；本任务仅 merged，不发版。无待决 Spec Amendment。遵循模板单执行流，不调用 delegate_tasks，不声称独立 QA。


## 当前进展（通俗版）

已经修好了测试结果格式不一致、可能把不完整结果误判为通过的问题，新检查程序已合入，原来的保护已恢复。随后通过普通 PR #512 在 Windows 和 Mac 上实际复测：113 个执行项全部通过，原始记录独立核对一致，六项合并检查通过后正常合入。

#514 也已正常合并：修正了“调试程序测试通过，却被写成正式安装包已验收”的报告标签，19 个执行项和六项合并检查通过。现在继续准备真实桌面测试：先让测试程序不接触真实账号和配置，再到一次性远端 Mac 上确认能看到它自己的窗口和设置按钮、结束后能回收主进程。本机锁屏不动；这一步还没有真实界面成功结果。

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

- Current phase: Bootstrap-1a/#508、完整保护 canary/#512、runtime 证据标签修正/#514 已合并验收。继续 M1 桌面安全隔离与非 required 的远端原生观察实验；不改变 registry、trusted judge 或现有合并规则。
- Current checkpoint: M0/#502、M1a/#503、M1b/#504 均已合入 `main`。#505 最小路由修复合入后，#504 作为全量 canary 通过 111/111 target（Windows 102、macOS 9）和全部六项 required checks，合并提交为 `05461f19c62cb90e592defd0c53fc78c4996835e`；两个已完成 worktree 已按 PR 证据回收，用户主 checkout 未修改。详见 `docs/evidence-packs/scenario-runner-bootstrap-2026-09-07.md`。
- Next owner: #512 已在 base `5c25e49fc1947ea7557ee0d4184129b63400ffae` / head `ac38ea66b7cee9bb56be78928a07f85e624868b4` 实际通过 113 targets（Windows 104、macOS 9）和唯一 Windows E2E-001 v2 case；正常合并为 `c9bd334091551240f38cd3072159e4e88f9101d9`，父提交和完整文件树读回与被测版本一致。六项检查、可信原始观测复算、12 类主执行者反例与 24 项独立 QA 反例均通过。#514 后继修正已合并为 `52bd3657ed15b75ea916cc5c1a271a5d85b2bacc`。下一步提交已独立审查的桌面隔离入口与原生观察实验，取得远端实测结果后再决定桌面 feasibility 和 E2E-004 PR slice 的接入；不重复请求或执行已完成的 #508 迁移。
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
- #512 恢复后 canary 已正常合并：[执行 34184641912](https://github.com/BumStill/CodeFactory/actions/runs/34184641912) 与 [独立 gate 34184640431](https://github.com/BumStill/CodeFactory/actions/runs/34184640431) 在同一 base/head 全通过。单用户消息、零人工 prompt、真实中断与不同进程接管、幂等副作用和零泄漏清理均由原始记录核验；不包含 UI、Mac E2E-001 或正式安装包证明。线上 ruleset 标准 verify 为 converged。
- #514 runtime 标签修正已正常合并：[执行 34186734056](https://github.com/BumStill/CodeFactory/actions/runs/34186734056) 和 [独立 gate 34186734286](https://github.com/BumStill/CodeFactory/actions/runs/34186734286) 通过，Windows 17/macOS 2 targets 均有实际结果；完整 plan/aggregate 由原始 runner 回执独立复算，六项 required checks 通过。被测 head `f714f489e0265630965d3de194083167c195bbe5` 与 merge `52bd3657ed15b75ea916cc5c1a271a5d85b2bacc` 文件树相同。实际 Mac Chrome attach 回执为 `native_runtime_smoke`，四项 bridge/tab/detach/lease 断言成立；正式 DMG 来源校验未放松。这是测试证据修正，不单独触发产品发版。

## Remaining Items

- Bootstrap-1a：入口、执行回执 v2、可信摘要、原始观测重算、目录清理与严格集合校验已随 #507/#508 合入，完整保护下的非空 exact-head canary 与独立复核已由 #512 完成；后续不重复计为待办。
- Bootstrap-1 后续：可信 catalog 接入、桌面 feasibility probe、E2E-004 PR slice；只能在真实证据满足后完成相应里程碑。
- 桌面安全前置：源码检查确认独立 HOME 仍可能访问共享 Keychain、普通配置迁移和后台账号/更新入口。新增 Synthetic 入口在 WebView/插件之前验证真实目录，关闭普通账号、更新、恢复、数据库和后台入口，只开放合成设置的主题读写，凭据公共边界拒绝访问。本地失败优先测试与独立代码审查已完成，仍需普通 PR/CI 和真实 Tauri 结果；不把源码隔离当成 OS 沙箱或已观测零请求/零凭据访问。
- M1 远端原生观察实验：只在一次性 macOS CI 从精确候选源码构建 App，按实际 PID/birth/path/digest/bundle 身份观察窗口与设置控件，并回收本次主进程。公开回执严格验证后才上传；原始 AX、owner/state/manifest 不上传。即使 observer slice 通过，完整 probe 仍为 blocked：点击、输入、重启持久化、窗口截图、后代清理、目录清理、请求观测、凭据观测和二进制内嵌身份仍为九项未完成内容。supervisor 取消后的清理也未实现。本机只读核对已锁屏，未请求解锁或启动 App。
- #515 实际尝试：head `9d837d750776e19532ca4aee170fdace47caf929` 的远端 Mac 成功完成测试、打包和源码未改核验；[原生实验 34190155096](https://github.com/BumStill/CodeFactory/actions/runs/34190155096) 在启动 App 前被 `gui_session=false` 拦截，辅助功能和截图权限均为 true。当前证据不能区分缺会话、未登录或锁屏未知，不把它说成产品行为失败或远端明确锁屏。继续补匿名预检定位，并前移到编译之前。原六项合并检查和完整执行回执另行验收，不能用它们替代这个失败结果。
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
- validation result: M0/M1a/M1b 已合并，旧最小路由 bootstrap 已通过 111 个 target canary。Bootstrap-1a 的入口、配置、类型、隐私、重复结果和版本漂移问题均已补失败优先测试，#508 已进入默认分支 judge，原始保护恢复。新基线非空 canary 已由 #512 以 113/113 及独立复算验收，完整 case 状态和 L3/L4 缺口保持不变。

## CodeFactory 接手 E2E-004 L2（2026-10-17）

- 规格：`docs/specs/feature-specs/e2e004-fake-forge.md`，原始 R1–R7 保持原意。
- 已完成切片：复用 `delivery_scenario_tests.rs` 的生产 `deliver`、隔离 worker、真实 Git 和 SQLite FakeForge。新增 `e2e004_ci_failure_recovers_same_pr_and_artifact`：第一次 CI 失败，第二个进程切换成功，同分支/提交/PR identity；第三次重放无重复 PR/merge/release，预存 dirty file 和 root index 不变。产物从实际 Git 对象提取，匿名三字段 receipt 绑定 merge SHA 与字节摘要，并回读核验。
- 失败优先：相同目标先以 worker 不支持恢复阶段失败（exit 101），修复后通过；全组反例发现旧 tag 提前读取不存在文件的问题，修复 fixture 提取路径后 10 passed、2 supervisor-only workers ignored。worker 由 supervisor 实际启动，并非跳过被测路径。
- 本地复跑：`CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-e2e004" CARGO_INCREMENTAL=0 cargo test --manifest-path src-tauri/Cargo.toml --lib agent::delivery::scenario_tests -- --nocapture`。现有 nightly 的全 workspace 测试自然包含此非 ignored target；未修改 workflow、validator 或 registry。
- **未完成且不计为完整 E2E-004**：同会话运行时 steer/queue、provider 重连、测试失败自动诊断修复、durable objective/delivery run/change set 和 CI 等待 owner 释放尚未组合验证；缺证据任一项不得 completed 的整矩阵尚未覆盖。当前只是 PR/CI/merge/artifact 的 L2 前置切片，不将 10 项测试等同完整 case。
- Spec Amendment 待编排方决定：R6 要求修改 registry，但任务模板 §6 禁止修改信任根。建议由编排方通过受控规则集流程更新 `automated_by` 与精确 remaining gaps，状态最多 `partially_implemented`；不得改为 `implemented`。本 PR 保持 registry 不变。
- L3/L4、manual_canary 均保留。真机验收由编排方发版后完成：同一任务追加约束和纠正可见顺序、重连恢复、CI 等待释放 owner、重启续接原 run、实际交付回执及无重复副作用。
- AI Collaboration：context scope 为任务文件、registry 和既有 FakeForge；assumption 为合成 fixture 不等于生产验收；review point 为本组反例与身份/字节断言；validation result 以 PR 中最终命令和 CI 为准。按任务模板禁止 `delegate_tasks`，本轮单执行流，不伪称独立 QA 已执行。

## CodeFactory 接续 E2E-008 L2（2026-10-18）

- 规格：`docs/specs/feature-specs/browser-extension-lifecycle-smoke.md`；原文 R1–R5 保持不变。备份 diff 在当前 main 可应用，但重写隔离 smoke，修复 listener abort 后未 await 的清理竞态。
- 正式 binary 新增 `--browser-extension-lifecycle-smoke <receipt.json>`，在 Tauri 初始化前执行；真实 loopback WebSocket 覆盖 heartbeat idle、旧代际 pending 取消、旧连接关闭不清新连接、瞬断自动重连、半开命令 deadline 驱逐与恢复。全程同一内存 bridge/合成 tab，6 条唯一命令，pending 为空，listener 退出后原端口可重新绑定；不读写生产 pairing/profile/DB，不启动真实浏览器。
- 失败优先：Node 契约首次 0/4（入口、失败 receipt、CI、registry 均缺失），实现后原命令 4/4。完整 Rust workspace 首轮仅入口录制 facade 缺新函数失败，补齐 fixture 后同命令全绿，未放宽原顺序断言。生命周期 Rust 测试内部重复三轮真实 loopback。
- 正式 binary 从源码目录外实测成功 receipt 全字段；注入失败时写固定匿名失败 receipt 后 exit 1。前端 151 files / 1013 tests（含 lightModeAudit）及 build 通过。治理校验发现 execution_policy 缺 binary receipt oracle，按既有格式补齐；scenario --ci 与 README validator 均 OK。
- R3/R4：CI check-rust 阶段运行 binary 并逐字段断言；registry 增加 automated_by、PR required target 和 receipt oracle，移除仅 L2 gap，仍保持 partially_implemented、L3/L4。信任根变更由本单明确授权，cannot self-modify 交由编排方处理，不改 validator。
- 真机验收由编排方发版后完成：同一会话选择 tab、空闲/瞬断/交错重连/升级，核对续接、prompt、去重和审计日志；L2 不等于真实桌面或 exact release 验收（not live）。无待决 Spec Amendment。
- AI Collaboration：context scope 为任务、备份、transport、registry；assumption 为合成 bridge session 不是生产 DB；review point 为清理竞态及失败路径；validation result 见 `docs/evidence-packs/e2e008-lifecycle-smoke.md` 和 PR CI。遵循任务模板单执行流、不调用 delegate_tasks。
## E2E-004 第二段进展（2026-10-17，未完成交付）

- 已追加第二段获批规格；新增 `e2e004_steer_reconnect_repair_resumes_same_durable_run`，先在旧 worker 上失败，再补组合 fixture 后通过。
- 已组合生产 `SessionSteerInbox` / queue、loopback HTTP 首次断连与生产 retry、真实 rustc 五组输入验收先失败再最小表达式修复、FakeForge CI 失败后新 PID 恢复、持久 run 身份与 lease 释放、artifact 摘要及无重复交付。
- 证据边界：诊断/修复目前由 fixture 驱动，尚非完整 agent-loop；objective_id 目前是 delivery row 的关联字段，尚未组合真实 durable objective 生命周期；canonical PR 由 fixture 回填，尚未覆盖生产工具准入 wrapper。因此 R1 仍为 partial，不能声称第二段完成。
- R2 已最小登记两项 Rust 硬门禁，状态仅 `partially_implemented`；validator 本地 OK（28 scenarios / 11 complex E2E）。nightly 复用已有全 workspace 步骤，无 workflow/validator 修改；信任根 cannot self-modify 仍由编排方处理。
- 全 Rust workspace：无窗口保护发现三个新增命令问题，已补 `.no_window()`；其后默认并发触发既有 SQLite 锁等待计时测试失败，低并发 `RUST_TEST_THREADS=2 CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-e2e004-slice2" CARGO_INCREMENTAL=0 cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast` 全部通过。日志 `/tmp/e2e004-slice2-workspace.log`。未修改/放宽测试或 validator。
- 最终实现补充：已用真实 `run_agent_loop` 驱动确定性 provider/tool adapter，在运行中入队两条 steer 并断言顺序；真实测试退出码 101→0，读取实现后只修复表达式。已通过 ObjectiveStore 创建和跨进程回读同一 objective；canonical PR 改用生产 record_delivery_observation 与 identity revision receipt，不再 SQL 回填。仍不代表桌面工具准入或 L3/L4 验收。
- R3 已删除旧正文草稿。最新全 Rust workspace 低并发通过，前端 151 文件/1013 测试（含 lightModeAudit）和 build 通过；最终 PR 正文场景 validator（--ci）及 README contract 均通过。正文仅存 ignored cache，PR/CI/queue merge 与 §8 收尾以实际交付结果为准。
- 真机验收由编排方发版后完成：安装 exact release，核对同 session 两条 steer 顺序、重连后原 objective/run、测试修复、CI 等待 owner 释放、恢复与单次 artifact 身份。当前无 UI 改动，不声称 L3/L4。
- AI Collaboration：context scope 为任务/模板、FakeForge、queue、HTTP retry、delivery_run 与 registry；assumptions 为合成切片不等于完整 agent-loop；review point 为身份与副作用断言；validation result 为上述 Rust 全量和 registry 结果。模板禁止 delegate_tasks，故单执行流，无独立 QA 声明。

## E2E-004 第二段 Windows 修复（2026-10-18）

- 现象：`check-rust`（windows-latest）上 `e2e004_steer_reconnect_repair_resumes_same_durable_run` 失败，worker panic 在 `delivery_scenario_repair.rs` 的编译断言处；`gh run view --log-failed` 的完整日志是 `link.exe` 报 `link: extra operand '...rcgu.o' ... Try 'link --help'`。
- 根因：那句 `link --help` 是 Git for Windows 自带的 coreutils `link.exe` 的输出，不是 MSVC 链接器。夹具原本在隔离 worker 内用 `rustc --test` 现场链接一个验收二进制，而 worker 为隔离 git/凭证会 `env_clear()`；Windows 上 rustc 因此无法从 MSVC 工具链环境解析链接器，回退到 PATH 中排在更前面的 Git `usr/bin/link.exe`。macOS/Linux 没有同名冲突，所以只在 Windows 红。
- 修复（不放宽断言、不跳过或忽略被测路径）：验收命令改为在子进程运行**同一个已构建测试二进制**里的 `synthetic_acceptance_probe`（`--ignored` + 名称过滤），保持真实退出码 101→0 与真实的 ``assertion `left == right` failed`` 输出，五组输入 [-3,0,2,7,19] 不变；既不依赖原生链接器，也不引入 node/python 等额外解释器。工具命令名由 `cargo test` 改为 `check-acceptance`，并新增“探针确实执行”的断言（成功须含 `1 passed`、失败须含 `FAILED`），避免过滤路径写错时静默通过。
- 同时删除 worker 对 `rustup which rustc` 的依赖，worker 不再需要现场编译任何东西。
- 本地复跑：`CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-e2e004s2" CARGO_INCREMENTAL=0 cargo test --manifest-path src-tauri/Cargo.toml --lib agent::delivery::scenario_tests:: -- --nocapture` → 11 passed、3 ignored（两个 supervisor-only worker 与验收探针本身，均由 supervisor 或夹具以子进程实际执行）。
- 证据边界不变：仍是合成 fixture 的 L2 切片，诊断/修复由 fixture 驱动，L3/L4 与 manual_canary 保持 remaining gap。

## E2E-004 第二段 Windows 真实根因与修复（2026-10-18，第四次）

- 现象仍同前：`check-rust`（windows-latest）上 `e2e004_steer_reconnect_repair_resumes_same_durable_run` 红，worker panic；`link.exe` 目录冲突只是现场链接失败的次生现象，删除现场链接后仍会在 Windows 红。
- 真实根因（3495880a 定位，本单复现并定论）：夹具 provider listener 为 nonblocking（`delivery_scenario_tests.rs` 的 `slice2_prepare`），Windows 上 accepted socket 继承非阻塞模式；请求数据未到齐时 `read` 返回 `WSAEWOULDBLOCK`（10035），`unwrap` 让服务线程 panic。`set_read_timeout` 不恢复 blocking 模式。生产 `send_with_retry` 随后的重试得到 `ConnectionRefused`（10061），supervisor 回收 worker 后 `status.success()` 失败（934:17）都是同一 panic 的连带失败，不是回收超时或路径问题。
- 修复：新增 `src-tauri/src/agent/delivery_scenario_socket.rs`，`configure_provider_socket` 先 `set_nonblocking(false)` 再设读写超时；`delivery_scenario_tests.rs` 以 `#[path]` 挂载该模块并在 accept 后调用，替换原来的 `set_read_timeout` 裸调用。
- 失败优先：新增 `socket::tests::accepted_socket_reads_fragmented_request_after_nonblocking_listener`，用 `set_nonblocking(true)` 的 accepted socket 在任意平台确定性模拟 Windows 继承语义，并让请求分段到达。修复前该测试红：`Os { code: 35, kind: WouldBlock, message: "Resource temporarily unavailable" }`（macOS errno 35 ≡ Windows 10035）；加上 `set_nonblocking(false)` 后同一命令 1 passed。没有跳过或忽略 Windows，也没有放宽任何断言。
- 阶段验证：`CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-e2e004s2" CARGO_INCREMENTAL=0 cargo test --manifest-path src-tauri/Cargo.toml --lib agent::delivery::scenario_tests:: -- --nocapture` → 12 passed、3 ignored（两个 supervisor-only worker 与夹具子进程执行的验收探针）；目标场景 `e2e004_steer_reconnect_repair_resumes_same_durable_run` 通过。
- 证据边界不变：仍是合成 fixture 的 L2 切片，L3/L4 与 manual_canary 保持 remaining gap；Windows 真实结果以本 PR 的 `check-rust` 为准。

## Stop Boundary

- 不在文档完成、本地绿色、PR 通过、merge、公开资产或安装成功任一单点停止。
- 每个里程碑只能在其 stage-required oracle、cleanup 和 identity 同时成立后结束；否则继续推进或记录有证据 blocker。
- 任务最终只在 11/11 case implemented、remaining gaps 清零、trust root/ruleset 对账和 exact release 主路径验证全部完成时停止。
