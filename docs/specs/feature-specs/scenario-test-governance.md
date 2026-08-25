# 场景测试统一治理与复杂端到端测试规格

> 状态：Active governance contract  
> 权威机器源：`docs/testing/scenario-registry.json`  
> 适用范围：CodeFactory 所有产品变更（不依赖 PR 标题或开发工具）、主路径回归、nightly 与 release artifact 验收

## 问题

CodeFactory 现有测试数量很多，但场景分散在 Rust 测试、Vitest、headless acceptance、可执行 smoke、nightly 和 release workflow 中。过去发生过以下共同失败：

- 单元测试覆盖了局部函数，但没有覆盖历史 SQLite → App 启动 → UI 操作 → 持久状态 → 再次重启的完整链路；
- 同一个逻辑场景在 PR、nightly、release 重复执行，却被误算成多个场景；
- UI acceptance 与真实 Workspace 使用不同组件或不同 hydration/stream 路径，测试证明的是替身而不是用户路径；
- 修复当前缺陷时没有识别被同一文件影响的既有 P0 场景，导致“修好继续，改坏停止”一类回归；
- 生产历史提供了真实失败形状，但没有被匿名化、版本化并转成长期测试资产。

本规格建立一个统一注册表和变更门禁，使“场景是什么、由什么自动化、在哪个证据层阻断、改动影响哪些场景”可以被机器校验。

## Requirements Traceability

| Req ID | 要求 | 自动化证据 |
| --- | --- | --- |
| CF-STG-R1 | 所有逻辑场景必须有稳定且唯一的 ID | `validate_scenario_test_governance.py` |
| CF-STG-R2 | 分类、优先级、变更路径、自动化入口、证据等级和 gate 必须统一登记 | registry validator |
| CF-STG-R3 | PR/nightly/release 对同一逻辑场景的重复执行不得重复计数 | `required_evidence` + `gates` |
| CF-STG-R4 | 生产历史只能抽取匿名聚合形状，不得保存原文、真实 ID、路径、凭据或工具参数 | registry privacy contract |
| CF-STG-R5 | 任何标题的产品变更都必须声明 `Scenario-Test:`；`not-applicable` 只适用于机器判定的非产品变更 | CI change contract |
| CF-STG-R6 | 修改命中 P0/P1/P2 场景路径时，声明必须覆盖全部受影响 ID；产品文件无映射时 fail closed | `validate_change_contract` |
| CF-STG-R7 | 复杂 E2E 必须同时包含 UI、持久状态、进程、副作用和交付 oracle | registry validator |
| CF-STG-R8 | “真实桌面”不得由 jsdom、mock AppHandle 或静态页面替代 | L3 evidence contract |
| CF-STG-R9 | “跨重启”必须使用独立进程和同一 SQLite，不能只重建内存对象 | L2 evidence contract |
| CF-STG-R10 | “正式版本可用”必须由 exact release artifact、build identity、安装/更新路径证明 | L4 evidence contract |
| CF-STG-R11 | 长任务无人参与用例必须保持一条用户消息、零人工 prompt，并自动完成或进入真实不可恢复终态 | `E2E-001` |
| CF-STG-R12 | 显式停止必须覆盖 session 全部 live objectives，并在两次重启后保持 | `E2E-003` |
| CF-STG-R13 | 历史 session 的简短继续必须从旧 schema 和分页历史真实续接 | `E2E-002` |
| CF-STG-R14 | 增量约束、失败修复、PR/CI/merge/artifact 必须保持同一交付身份 | `E2E-004` |
| CF-STG-R15 | 浏览器失败必须证明进程树和租约回收，不得只断言命令返回 | `E2E-005` |
| CF-STG-R16 | 自动更新必须覆盖历史卡住 objective、旧进程锁和首次启动 reconciliation | `E2E-006` |
| CF-STG-R17 | 浏览器扩展稳定性必须覆盖 MV3 idle、connection generation、迟到 close/reply、heartbeat ACK、半开失活关闭与多 profile 稳定 winner/standby 接管，不得用单次连接 happy path 代替 | `RTE-002` + `E2E-008` |
| CF-STG-R18 | attached session 瞬断恢复必须保持 objective、lease、selected tab，并在有界窗口内只重放一次只读调用，不能依赖模型或人工 continue | `RTE-003` + `E2E-008` |
| CF-STG-R19 | 每个登记的自动化 target 必须绑定到明确的 `pull_request`、`nightly` 或 `release_artifact` 阶段，且该阶段必须被场景自身声明；不能只证明文件/函数存在 | registry v2 `gate_policy.target_bindings` + stage validator |
| CF-STG-R20 | PR 不得使用候选分支修改后的 validator 自证；唯一 required context `scenario-gate-pr` 必须通过 `pull_request_target` 使用默认分支 runner 只读检查候选树 | trusted `scenario-gate-pr` |
| CF-STG-R21 | base SHA、required run 或 live ruleset 状态无法读取时必须失败，不能 warning 后继续 | fail-closed runner + ruleset verifier |
| CF-STG-R22 | PR 阶段只对**受影响**的场景与复杂用例强制「PR slice 已实现、无 PR gap、全部 required targets 在该阶段真实执行」；不得用无关目录债务冻结当前 PR | `pull_request_gate` + `validate_impacted_execution` |
| CF-STG-R23 | 逐字节 trust root 同时覆盖审判逻辑和提供执行证据的 required workflow；普通候选 PR 不得把测试改成空跑后自证 | `TRUST_ROOT_FILES` + external governance bootstrap |
| CF-STG-R24 | 纯版本 bump 只有在四个 manifest 的字段级 diff 同时证明为同一旧版本到同一新版本、且无其他字段变化时才不产生 PR 场景影响；依赖、脚本、Cargo 或 Tauri 配置的其他变化仍按全局产品变更 fail closed | `scenario_impact_files` + version manifest negative fixtures |
| CF-STG-R25 | Release 门禁以「上一已发布 tag 到候选 tag」的产品差异计算影响集，只要求受影响场景具有绑定到 release workflow 的 exact-artifact target；无关目录债务不得冻结发布，缺基线、未映射产品文件或受影响 target 未绑定时 fail closed | `scenario-gate-release --base-ref` + `validate_impacted_execution(stage=release_artifact)` |

## Primary User Path

统一场景治理围绕同一条主路径建模：

1. 用户打开真实 CodeFactory，选择项目、session、模型和权限模式；
2. 用户给出编程目标，可能随后离开，也可能追加少量业务约束；
3. 系统读取、修改、执行测试，并在 provider、工具、进程、CI 或更新失败后自动恢复；
4. 用户可查看真实进度，也可显式停止；
5. 系统只有在持久状态、幂等副作用和约定交付证据同时满足后才能完成；
6. App 重启、电脑重启或版本升级后，既不能丢失应继续的工作，也不能复活已停止的工作。

## 统一对象模型

### Scenario

Scenario 是稳定的产品风险/用户行为单元，不等同于一个测试文件。字段包括：

- `id`：稳定 ID；现有系列为 `HLT`、`CXD`、`UI`、`RTE`；
- `category`：业务分类；
- `priority`：P0/P1/P2；
- `source_kind`：历史匿名形状、产品契约或 incident 形状；
- `change_patterns`：可能破坏该场景的代码路径；
- `automated_by`：当前真实自动化入口；
- `required_evidence`：最低证据层；
- `gates`：PR、nightly、release 或 manual canary。

Schema v2 的 `gate_policy` 把每一种 target 绑定到具体 workflow、job/command 和 required context。`gates` 只记录真实阻断层；所有 active Scenario 至少包含 `pull_request`，`manual_canary` 只能作为补充。函数、脚本或文件“存在”但未绑定到 protected check 时，registry 编译失败。

### Complex E2E Case

Complex E2E Case 是多个 Scenario 的组合旅程。它必须定义：

- synthetic fixture；
- 至少四个跨层步骤；
- 明确 fault injection；
- `ui`、`durable_state`、`process`、`side_effects`、`delivery` 五类 oracle；
- 各 gate 的执行方式和当前自动化状态。

它不会制造新的产品能力计数。例如 `E2E-001` 同时执行 HLT-001、HLT-002、CXD-002，但正式场景总数仍按这三个 Scenario 计算。

## 分类

| 分类 | 当前数量 | 代表风险 |
| --- | ---: | --- |
| 长任务连续性与恢复 | 5 | 中断、恢复预算、历史续接、持久停止、恢复交还 |
| 对话协作与交付 | 2 | 增量约束、完成证据 |
| 工作区与会话体验 | 7 | 启动、导航、恢复日志、断线、浏览器连接投影 |
| 内容输入与呈现 | 2 | 图片、流式 Markdown |
| 权限与安全 | 1 | 权限模式与可见状态 |
| 能力演进与用量 | 2 | Evolution、token/cost |
| 运行时资源生命周期 | 3 | 浏览器进程/租约回收、扩展健康、attached session 续接 |

当前统一注册表共有 26 个逻辑 Scenario、11 个 Complex E2E。任何新增主路径能力必须在合并实现前新增或扩展一个 Scenario。

## 证据等级

| 等级 | 用途 | 不足以替代的上层证据 |
| --- | --- | --- |
| L0 契约与纯逻辑 | reducer、状态机、parser、静态契约 | 真实 SQLite/进程/UI |
| L1 集成 | SQLite、provider adapter、工具边界 | hard kill、真实 WebView |
| L2 真实进程 | child process、hard kill、重启、进程树 | 用户真实操作、正式安装包 |
| L3 真实桌面主路径 | Tauri/WebView、点击、输入、hydration、stream | exact release binary |
| L4 正式产物 | 安装、升级、签名、tag SHA、公开 artifact | 生产用户长期行为观察 |

P0 场景不能只依赖 L0。涉及重启至少要求 L2，涉及用户按钮或历史页面至少要求 L3，涉及更新/安装至少要求 L4。

## PR 变更门禁

所有产品变更，无论标题是 `feat`、`fix`、`chore`、`refactor` 或由什么工具生成，都必须在 PR body 放置：

```text
Scenario-Test: HLT-003, HLT-004
```

规则：

1. ID 必须存在于统一注册表；
2. 代码路径命中任意优先级场景时，必须声明全部受影响 ID；
3. 产品文件没有命中场景时属于 registry coverage gap，直接失败；产品变更不能使用 `not-applicable`；
4. 声明不是完成证据。PR 仍必须执行 ID 对应的自动化和证据层；
5. 新功能若无法选择任何 Scenario，说明注册表缺场景，必须先补登记；
6. 缺 base SHA、事件正文或 diff 失败时 fail closed；PR body 的 `edited` 事件必须重跑；
7. `Scenario-Test: ALL` 只用于机器生成且确实影响全部场景的全局运行时/版本批次，不是 waiver。

## 复杂真实 E2E 组合

| Case | 场景 | 主要故障注入 | 最低阻断层 |
| --- | --- | --- | --- |
| E2E-001 | 用户离开后长任务自动完成 | hard kill、provider transient、重复 claim | PR L2 + release L4 |
| E2E-002 | 历史 session 简短继续 | 旧 schema、分页、无内存控制、无 listener | nightly L3 + release L4 |
| E2E-003 | 停止后永不复活 | cancel/claim race、部分投影失败、两次重启 | nightly L3 + release L4 |
| E2E-004 | 增量约束贯穿交付 | 测试失败、CI transient、dirty worktree | nightly L2/L3 |
| E2E-005 | 浏览器失败回收并继续 | 零退出逻辑失败、子进程残留 | PR L2 + release L4 |
| E2E-006 | 卡住历史任务下完成升级 | 旧进程锁、WAL、安装中断、首次 reconciliation | release L4 |
| E2E-007 | 恢复耗尽后一致交还 | 计划拒绝、旧 streaming、unknown receipt、两次重启 | PR L1/L2 + release L4 |
| E2E-008 | 扩展空闲/瞬断/多连接后同一会话续接 | MV3 idle、旧连接迟到关闭、socket drop、App restart | PR L1 + nightly L2 + release L4 |

完整 step、fixture 和 oracle 以机器注册表为准。

## E2E Fixture 架构

复杂场景使用隔离且可重复的测试世界：

- 临时用户数据目录和真实 SQLite/WAL；
- 合成 Git 仓库，包含预存用户修改、多个源文件、失败测试和明确 verifier；
- 可编排 provider，能产生 streaming、tool calls、断流、截断和可恢复/不可恢复错误；
- 本地 fake forge，模拟 PR、required CI、merge 和 artifact receipt，不默认触碰真实远端；
- 独立正式子进程，记录 PID/start token，可在精确 checkpoint hard kill；
- Tauri dev/安装版 UI driver，操作真实 session 列表、composer、停止按钮和恢复日志；
- Windows installer/updater fixture，用前一正式版本升级到候选 exact artifact。

所有 fixture 必须通过随机临时目录和独立 app identifier 隔离，并在成功、失败、取消和超时路径清理。

## Oracle 设计

每个复杂 E2E 的结果不能只由一条 UI 文案决定：

- UI oracle：用户看到的状态、按钮、错误和进度；
- durable-state oracle：SQLite 中 objective/turn/remediation/receipt 的身份和终态；
- process oracle：PID 替换、进程树回收、无 live owner；
- side-effect oracle：写入、PR、发布、浏览器 session 恰好一次；
- delivery oracle：测试、CI、artifact、build SHA 和真实功能验证达到约定边界。

五类 oracle 必须指向同一 `run receipt`，防止拿 A 进程的 UI 截图配 B 数据库的状态。

## 历史场景提取

允许从历史 session 提取：状态序列、计数范围、故障类型、时间顺序和用户意图形状。提取时必须：

1. 只读访问；
2. 在进入仓库前转换为 synthetic fixture；
3. 删除原始消息、session/objective ID、用户名、项目路径、凭据和原始工具参数；
4. 记录 `source_kind=anonymized_history_shape`，不记录可反查用户的数据；
5. 新场景先以 `designed` 登记，自动化完成后提升为 `partially_implemented` / `implemented`。

## Gate 策略

- PR：所有 active Scenario 都有明确 `pull_request` 绑定；唯一场景治理 required context 是 trusted `scenario-gate-pr`，并与其绑定的 Rust/frontend/真实 App checks 共同构成门禁。只计算 diff 直接命中的 Scenario 与 Complex E2E；其中任一 `pull_request_gate` 为 `designed`/`partially_implemented`、仍有 PR gap 或 required target 未真实执行时冻结。无关目录的测试债务不阻断该 PR。
- Nightly：运行旧 schema、故障矩阵、真实 App restart 和 fake-forge 组合场景。
- Release：`scenario-gate-release` 在创建/复用 draft 前解析上一 tag，按批次产品差异计算影响集，并验证受影响 exact-artifact target 已绑定到 release workflow；真正的 target 随 Windows/macOS build job 执行，任一失败都会使 `finalize` 不可达。无关历史缺口保留在 registry，但不冻结本批次。
- Manual canary：只能补充自动化 hard gate，不能是 active Scenario 的唯一门禁。

### 信任边界与工具传播

- `docs/testing/scenario-registry.json`、统一 runner 和 GitHub ruleset 是机器权威；Codex、Claude、IDE 与人工得到同一结论；
- `AGENTS.md` 与 `CLAUDE.md` 只负责告诉执行者提前运行同一命令；`.githooks/pre-commit`/`pre-push` 提供快速反馈，但允许被本地跳过；
- 最终权威是 GitHub required checks。`scenario-gate-pr` 自身使用 `pull_request_target` 从 default branch 加载 runner，以只读、无凭据方式检查 candidate checkout，candidate 代码永不执行；不再需要第二个 `scenario-gate-policy` context；
- 默认分支 policy 逐字比对 ruleset、两个 scenario workflow、runner、validator，以及提供 7 个 required contexts / release gate 的 `ci.yml`、`governance-baseline.yml`、`lock-independent-desktop-acceptance.yml`、`release.yml` 共九个 trust-root 文件；普通 PR 不允许自修改这些文件。后续治理升级必须走明确的 external governance bootstrap，再重新启用并对账 required contexts；
- 个人仓库管理员仍可在 GitHub 控制面修改规则集。组织级 required workflow 或独立 GitHub App 是更高一级的外部信任根；仓库内门禁不虚假声称能约束仓库所有者凭据。

## Applicable Harnesses

- Spec Harness：本规格、Req ID、注册表 schema；
- Compatibility Harness：旧 SQLite、旧配置、前一安装版本、provider 差异；
- Observation Harness：UI、SQLite、PID、receipt、artifact identity 的同 run 绑定；
- Release Harness：Windows exact binary、安装、更新、签名和公开元数据；
- Viewport Harness：真实 Workspace、历史 session、停止/继续状态；
- AI Collaboration Harness：历史形状提取、场景新增和 oracle review。

## 实施顺序

1. 已完成：统一 26 个场景和 11 个复杂 E2E，schema v2 将全部自动化 target 绑定到明确 gate；
2. 已完成：新增 trusted policy + PR aggregate required contexts、本地统一 hooks、全优先级 change contract 与 fail-closed base SHA；
3. 当前 gate debt：按每个 Complex E2E 的 `pull_request_gate` 分别记录；产品 PR 只需清零本 diff 直接命中的 PR slice，release 仍执行目录级 L4 readiness。不得把受影响缺口改写成 waiver，也不得要求无关 PR 先清全目录债务；
4. 逐项补齐真实 Supervisor wake、跨进程 SQLite、并发 finalization、真实 WebView、浏览器 lifecycle、fake-forge 和 exact artifact receipts，并把 case 提升为 `implemented`；
5. 全部清零后保持 PR 与 release gate 常开，nightly 只扩展故障矩阵，不再替代 required gate。

## 验收标准

- registry validator 拒绝重复 ID、未知分类、失效自动化入口和缺失 oracle；
- change contract 拒绝任意标题下未声明、漏声明 P0/P1/P2 或未映射的产品变更；
- CI 同时运行 validator 的单元测试和仓库真实注册表验证；
- 原历史场景目录不再维护第二份场景数据，只保留 canonical registry 指针；
- 26 个现有场景和 11 个复杂 E2E 可由机器读取，所有 target 都能解析到明确 hard gate；
- E2E-001 明确保证无人参与执行；
- 未完成自动化的 case 明确标注 `designed`；只完成部分证据层的 case 标注 `partially_implemented` 并列出 `remaining_gaps`。PR 额外使用 `pull_request_gate` 区分该阶段是否完整：受影响且未完成时失败，无关 case 不参与该 PR 结论；release 仍按全目录 readiness 失败。

## 风险与边界

- 路径映射过宽会制造无关声明；通过持续细化 `change_patterns` 控制噪音，不以降低优先级或 waiver 绕过。
- 声明可能变成形式主义；因此声明只负责 traceability，执行仍由自动化 target 和证据层负责。
- 复杂 E2E 成本高；PR 只跑确定性关键切片，故障矩阵进入 nightly，exact artifact 进入 release。
- 本规格建立治理与可执行设计，不等于 11 个复杂 E2E 已全部实现。`automation_status` 是当前真实边界。
