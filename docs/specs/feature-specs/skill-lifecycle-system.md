# Skill 生命周期与运行时系统规格

> 状态：Draft for approval
>
> 适用范围：CodeFactory 桌面端、Headless runner、资源中心、Agent tool runtime、Skill marketplace
>
> 当前实现基线：`origin/main@d4386979cbde398912bb91cd62cba8494b49c52a`（v1.81.23）
>
> 交付边界：本文件及配套 business / architecture / UX 设计只定义目标合同，**不代表已经实现或发布**。

配套设计：

- `docs/design/skill-lifecycle-business-design.md`
- `docs/design/skill-lifecycle-architecture-design.md`
- `docs/design/skill-lifecycle-ux-design.md`
- `docs/design/skill-lifecycle-threat-model.md`

## 1. 结论与问题定义

当前 Skill 能力只能视为“可启停的提示词包原型”，不能视为可靠的 Skill 安装与执行系统：

- 安装路径分别处理 marketplace、JSON URL、本地目录、Git 仓库和 Agent 工具，校验、默认启用和错误语义不一致；
- 安装过程直接向最终目录逐文件写入，崩溃或部分失败会留下无法被资源中心解释的目录；
- 资源中心只枚举直接子目录中的合法 `manifest.json`，损坏、未完成和不兼容包被静默隐藏；
- `SKILL.md` 导入只保留正文和少量可选文件，`scripts/`、`references/`、`templates/`、assets 等内容丢失；
- 启用后的 Skill 以系统提示词正文注入所有会话，没有按任务选择、加载回执或可靠的“本次已使用”证据；
- `tool_policy.json` 与 Skill slash commands 在资源中心可见，但没有完整进入真实工具权限和 Workspace 执行链；
- UI runner 与 Headless runner 使用不同加载路径，缺少跨入口一致性合同；
- 没有“安装 → 列表可见 → 审核 → 启用 → 新回合按需加载 → 禁用/回滚”的真实端到端场景门禁。

因此，本项目不以修补某一个列表刷新缺陷为目标，而是建立统一的 Skill Package、安装事务、审核/激活状态、按需运行时与证据合同。

## 2. 产品承诺

CodeFactory Skill 是一个本地优先、可检查、可版本化、可撤销的能力包。用户可以明确回答五个问题：

1. 这个 Skill 从哪里来，当前内容摘要与版本是什么？
2. 它安装成功了吗；失败或不兼容时具体停在哪一步？
3. 它会向 Agent 提供哪些指令、资源、命令和约束？
4. 它是否已启用，在哪个范围启用，本回合为什么使用或没有使用？
5. 更新、禁用、回滚和卸载后，系统实际处于什么状态？

不得再把“目录存在”“安装命令返回成功”“资源中心出现一行”或“提示词被拼接”单独称为 Skill 可用。

## 3. 目标

- **零静默丢失**：安装成功的包 100% 出现在资源中心；安装失败、损坏、不兼容或隔离的包也必须以明确状态出现。
- **统一生命周期**：所有来源进入同一个解析、校验、暂存、审核、提交、激活和审计管线。
- **默认安全**：远程和导入 Skill 安装后默认禁用；Skill policy 只能收紧权限，不能扩大用户或项目权限。
- **完整保真**：保留完整 Skill 包及资源；无法支持的声明必须阻止启用或清楚标记，不得静默降级。
- **按需使用**：运行时先暴露紧凑目录，再按显式选择或可解释匹配加载 Skill；不再把所有正文无条件注入每个回合。
- **可证明使用**：每次加载记录 Skill ID、包摘要、版本、选择理由、加载资源、截断和结果状态，并在 UI 可见。
- **入口一致**：桌面会话、Headless runner、自主任务和恢复后的任务遵循同一个 catalog、resolver 和 receipt 合同。

## 4. 非目标

- v1 不执行来自 Skill 包的任意二进制或脚本；脚本只能作为只读资源保留，执行仍需走现有工具权限与沙箱。
- v1 不建设团队 Skill 云同步、多租户审批或企业级私有市场。
- v1 不自动启用、自动扩大作用域或自动接受更新。
- v1 不用 embedding/LLM 评分作为唯一激活判据；首版优先显式选择和可解释规则。
- v1 不把 marketplace 下载量、评分或作者声明当作信任证明。
- 本规格不重定义 Codex、Claude Code 或 OpenClaw 的原生 Skill 格式；它们是输入适配器，不是 CodeFactory 内部权威模型。

## 5. 用户角色与故事

### 5.1 使用者

- 作为 CodeFactory 使用者，我希望任何安装结果都在资源中心可见，以便知道是成功、失败、损坏还是等待审核。
- 作为 CodeFactory 使用者，我希望启用前看到来源、版本、权限、文件和变更摘要，以便理解将影响什么。
- 作为 CodeFactory 使用者，我希望在会话里看到本回合用了哪个 Skill 以及原因，以便判断回答是否可信。
- 作为 CodeFactory 使用者，我希望更新后可以回滚、禁用和恢复删除，以便错误 Skill 不会长期污染工作。

### 5.2 Skill 作者

- 作为 Skill 作者，我希望 CodeFactory 保留标准 `SKILL.md` 和声明的资源，以便复杂 Skill 不会被降级成一段提示词。
- 作为 Skill 作者，我希望不兼容字段得到明确错误，以便修正包而不是猜测为什么列表没有显示。

### 5.3 CodeFactory 维护者与 QA

- 作为维护者，我希望所有安装来源复用同一后端服务和错误码，以便安全修复不会只覆盖部分入口。
- 作为 QA，我希望通过同一 install receipt 关联 UI、文件、SQLite、运行时加载和回合结果，以便拒绝结构性假通过。

## 6. Requirements Traceability

| Req ID | 优先级 | 规范化要求 | 影响 surface | 最低验证 |
| --- | --- | --- | --- | --- |
| CF-SKL-R1 | P0 | 定义唯一 `SkillPackage v2`，完整保存 manifest、`SKILL.md`、声明资源、摘要、来源和兼容信息 | tauri-backend + filesystem | L0 schema/parser + L1 synthetic package |
| CF-SKL-R2 | P0 | marketplace、registry ID、URL、Git、本地目录、Agent install/create 全部进入同一 `SkillInstaller`；旧 `skill_fetch/skill_create` 仅作为路由到 canonical API 的兼容 alias | tauri-backend + tool-runtime | L0 source/alias contract + L1 route assertion |
| CF-SKL-R3 | P0 | 安装使用独立 staging、严格校验、内容摘要、原子提交和可恢复 install receipt | filesystem + sqlite-store | L0 state machine + L1/L2 fault injection |
| CF-SKL-R4 | P0 | 资源中心展示 installed、`unreviewed`、enabled、disabled、failed、quarantined、incompatible、corrupt、update available 等真实状态，不得静默跳过 | desktop-ui + sqlite-store | L1 state projection + L3 UI |
| CF-SKL-R5 | P0 | 所有安装、导入和 Agent 创建的 Skill 默认禁用；UI 现场新建只有通过显式“保存并启用”才可连续完成审核/激活，且必须生成独立 receipt | desktop-ui + tauri-backend | L0/L1 negative assertion + L3 enable flow |
| CF-SKL-R6 | P0 | 所有 ID、archive path、resource path、更新和删除目标都限制在 Skill 根目录；拒绝 traversal、absolute path、symlink escape 和重复归一化 ID | tauri-backend | L0/L1 adversarial corpus + L2 process sentinel |
| CF-SKL-R7 | P0 | 官方 registry 使用配置 allowlist；用户显式 public Git/HTTPS source 走独立确认；两者都限制私网/redirect、状态码、超时、大小、文件数、深度和内容摘要；官方 envelope 必须验签并只提供不可变下载引用 | network adapter + installer | L0 policy + L1/L2 malicious/failure fixtures |
| CF-SKL-R8 | P0 | 运行时只向模型暴露紧凑 catalog；正文和资源通过 `skill_load` 按需加载，记录选择理由和 receipt | agent-loop + tool-runtime | L1/L2/L3 UI/headless parity |
| CF-SKL-R9 | P0 | Skill policy 只能收紧现有 permission ceiling；未支持的 required capability 阻止审核/启用，optional 内容完整保留并警告 | permission gateway + UI | L0/L1 policy intersection + L3 denial evidence |
| CF-SKL-R10 | P0 | UI、Headless、自主任务和恢复任务使用同一 catalog、activation 和 resolver；恢复不得重复加载或改变版本 | agent-loop + objective recovery | L1/L2/L3 cross-entry parity |
| CF-SKL-R11 | P0 | 合法 legacy Skill 可迁移；只有与签名内置包 digest 精确匹配且存在旧 enabled 事实的 builtin 可保留 activation，其他 legacy 均 unreviewed+disabled；损坏包必须可见且升级可回退 | migration + filesystem + sqlite-store | previous-version fixture L2/L4 |
| CF-SKL-R12 | P0 | 建立安装到真实使用的统一 Scenario 与 E2E 门禁，绑定 UI、持久状态、文件摘要、运行时 receipt 和回合证据 | governance + CI + release | L0 + L1 + L2 + L3 + L4 |
| CF-SKL-R13 | P1 | 支持版本更新预览、内容/权限 diff、显式批准、失败回滚和保留最近可用版本 | installer + desktop-ui | L1/L2/L3/L4 update/rollback |
| CF-SKL-R14 | P1 | Skill slash command 进入 Workspace 建议与执行链，处理冲突、模板参数和禁用状态 | desktop-ui + command router | L0/L1 contract + L3 real input |
| CF-SKL-R15 | P1 | 为非 anonymous 回合提供去标识聚合指标与用户预览后的诊断导出；不得采集 R18 anonymous turn | observation + settings | L0 event/privacy schema + L1 data validation |
| CF-SKL-R16 | P2 | 支持第三方作者签名、团队私有 registry、受控同步和组织审批 | future registry | future design only |
| CF-SKL-R17 | P1 | disable 与 remove 分离；remove 默认 recoverable，purge 明确确认，open objective 引用阻止 GC，builtin override 语义和历史 receipt 保留 | lifecycle + desktop-ui | L0/L1 state + L2 recovery + L3 UX |
| CF-SKL-R18 | P0 | anonymous turn 产生只存在内存的 ephemeral runtime receipt，不写 SQLite/遥测/诊断；非匿名 turn 才持久化 | agent-loop + observation | L0 privacy contract + L1/L3 anonymous path |
| CF-SKL-R19 | P0 | 定义跨来源稳定的 canonical package digest，排除 source、operation、时间和 receipt 元数据，并版本化 normalization algorithm | package validator + store | L0 digest corpus + L1 cross-source parity |

优先级与交付阶段是两套不同维度：`P0/P1/P2` 表示业务与风险优先级；`Phase 0/1/2/3` 表示交付顺序。P0 要求可以跨越多个交付阶段，只有对应阶段与证据同时完成，才算该要求完成。

### 6.1 唯一状态词典

以下轴互相正交；API、SQLite、UI 投影、Headless 和测试必须使用这些枚举或其明确映射，不能再创造 `awaiting_review`、`review_required`、`candidate_loaded` 等重叠状态：

| 轴 | 权威状态 | UI 映射 |
| --- | --- | --- |
| Storage | `absent/staging/installed/missing/corrupt/quarantined` | 未安装/安装中/已安装/需要处理 |
| Installation lifecycle | `present/tombstoned/purged` | 已保留/可恢复移除/已永久清理 |
| Review | `unreviewed/approved/rejected/revoked` | 待审核/已批准/已拒绝/批准已撤销 |
| Approval basis | `explicit_user/builtin_release/legacy_grandfathered` | 用户审核/随正式版批准/兼容启用待复核；initial rollout 不允许 grandfathered package 进入 runtime |
| Activation | `disabled/enabled/blocked_in_scope` | 未启用/已启用/当前范围已屏蔽 |
| Match | `not_evaluated/not_matched/matched` | 未评估/未匹配/已匹配 |
| Selection | `not_selected/selected_explicit/selected_auto/conflict` | 未选择/用户选择/自动选择/选择冲突 |
| Load | `not_requested/loaded_full/loaded_partial/dropped/failed` | 未请求/完整加载/部分加载/未加载/加载失败 |
| Invocation | `none/explicit_user/slash_command/model_catalog_call` | 未显式调用/用户调用/命令调用/模型目录调用 |
| Outcome | `not_evaluated/passed/failed/inconclusive/not_applicable` | 结果未验证/通过/失败/无法判断/不适用 |
| Compatibility | `compatible/incompatible/unsupported` | 兼容/不兼容/能力不支持 |
| Update | `current/update_available/superseded/rollback_available` | 当前/可更新/已被替代/可回滚 |
| Operation | `queued/fetching/staged/validating/committing/succeeded/succeeded_with_errors/recoverable_failed/failed/rolled_back` | 安装记录阶段；不承担 review 语义 |
| Recovery | `not_needed/pending/recovered/recovery_failed` | 独立恢复轴；operation 仍保存恢复后的最终状态 |

显式选择和 slash command 不要求先自动匹配；`Invocation` 是选择来源/调用事实，不是 `Load` 之后的价值阶梯。`loaded_partial` 只允许可选内容被截断，任何 required entrypoint/resource/capability 缺失都必须是 `failed`。

Catalog 投影也必须区分对象归属：已提交 package 的 `missing/corrupt/incompatible` 出现在“我的技能/需要处理”；尚未形成 package identity 的 fetch/validate/commit failure 只出现在“安装记录/失败”。顶部“需要处理”可以聚合两类，但不能为失败 operation 伪造 Skill 行。

### 6.2 Requirements Traceability Matrix

| Req | Delivery phase | 架构合同 | UX/业务旅程 | 威胁/失败验收 | Scenario / evidence | Owner |
| --- | --- | --- | --- | --- | --- | --- |
| R1 | Phase 1 | Package v2、canonical digest | 审核内容/文件树 | REL-003 | SKL-001 L0/L1/L2 | Backend |
| R2 | Phase 0→1 | typed source、single installer | 市场/Agent/本地导入 | REL-005 | SKL-001 L0/L1/L2/L3 | Backend |
| R3 | Phase 1 | staging、atomic commit、reconciler | 安装记录/恢复 | REL-001 | SKL-001 L0/L1/L2/L3 | Backend |
| R4 | Phase 0→1 | catalog projection | 我的技能/需要处理 | REL-004 | UI-013 L1/L3 | UI + Backend |
| R5 | Phase 0→2 | review、activation receipts | 审核并启用 | SEC-005 | SKL-003 L0/L1/L3 | Product + UI |
| R6 | Phase 0→1 | strong ID、handle-relative FS | 所有 mutation | SEC-001 | SKL-002 L0/L1/L2 | Security |
| R7 | Phase 0→1 | signed official registry、bounded fetch | 发现/安装 | SEC-002/003 | SKL-002 L0/L1/L2 | Security |
| R8 | Phase 2 | resolver、runtime receipt | Workspace 本回合资源 | SEC-005 | SKL-003 L1/L2/L3 | Runtime |
| R9 | Phase 0→2 | deterministic permission intersection | 审核/effective policy | SEC-004 | SKL-002 L0/L1/L2/L3 | Security + Runtime |
| R10 | Phase 2 | shared service、snapshot pin | UI/Headless/恢复 | REL-005 | SKL-004 L1/L2/L3 | Runtime |
| R11 | Phase 1 | migration/reconciliation | 待重新审核 | REL-001/004 | SKL-005 L1-L4 | Migration + Release |
| R12 | Phase 0→2 | receipt chain、scenario registry | P-SKL-001 | all P0 | E2E-009 L0-L4 | QA |
| R13 | Phase 3 | immutable revision/CAS/rollback | 版本 diff/回滚 | REL-001/002 | SKL-006 L1-L4 | Backend + UI |
| R14 | Phase 3 | command registry/router | slash command | command collision | SKL-008 L0/L1/L3 | UI + Runtime |
| R15 | Phase 3 | privacy-safe aggregate events | 安装/使用漏斗 | privacy contract | OBS-SKL-001 L0/L1 | Data |
| R16 | Phase 3+ | publisher/team trust | 组织审批 | supply-chain trust | future | Security |
| R17 | Phase 3 | installation tombstone/removal receipt | 禁用/移除/恢复 | safe delete | SKL-007 L0-L3 | Backend + UI |
| R18 | Phase 2 | ephemeral anonymous receipt | 匿名回合披露 | privacy contract | SKL-003 anonymous L0/L1/L3 | Runtime |
| R19 | Phase 1 | digest algorithm v1 | 来源一致性 | digest mismatch | SKL-001 L0/L1 | Backend |

文档中的 `R1` 等短写均指 `CF-SKL-R1`。Supporting design 不得脱离此矩阵另定义优先级或完成门槛。

## 7. Primary User Path

`P-SKL-001`：用户在资源中心选择一个 registry Skill，或让 Agent 查找并安装一个 Skill。系统创建安装操作，下载到 staging，验证路径、大小、manifest、资源摘要和兼容性，显示来源与能力预览。安装原子提交后，资源中心显示“待审核”，不会立即影响会话。用户查看指令、资源、权限收紧项和 slash commands，选择全局或当前项目范围并启用。随后用户发起匹配任务，Workspace 显示候选；模型通过 `skill_load` 加载固定摘要版本并记录 receipt。用户查看本回合的选择原因、版本和加载结果，并可在后续禁用该 Skill。

`P-SKL-002`（P1 生命周期维护路径）：已安装 Skill 出现新版本后，用户查看 manifest/content/policy/resource diff，批准更新并从下一 root turn 切换；更新失败保持旧版本，用户可回滚、移除到 trash 或在保留期恢复。该路径不属于恢复 P0“可安装并按需使用”承诺的最低范围。

### 7.1 成功加载与可证明调用边界

只有同时满足以下条件，才能称为一次成功加载或可证明调用；它不证明 Skill 改善了任务结果：

1. install operation 为 `succeeded` 且存在不可变 package digest；
2. catalog 可查询到该 package，review/activation 状态明确；
3. 当前作用域允许该 Skill，resolver 给出明确选择理由；
4. `skill_load` 成功加载固定 package digest，资源与截断信息进入 receipt；
5. 回合证据绑定同一 session/root turn/run receipt；
6. 禁用后，后续回合不再加载旧的 activation。P1 更新/回滚路径另按 `P-SKL-002` 验收。

任务结果验证使用独立 `SkillOutcomeVerificationReceipt`：`criteria_id`、`evidence_id`、`verifier`、`status`、关联 turn/package receipt。UI 只能显示“任务结果已验证”，并注明它与 Skill load 相关但不证明因果；没有验收器或证据时必须显示“结果未验证”，不得把 `loaded`、回合完成或模型自述自动升级为 `passed`。

## 8. Applicable Harnesses

- **Spec Harness**：本规格、Req ID、主路径、状态机、API、测试矩阵和证据合同必须作为实现入口。
- **Compatibility Harness**：legacy 文件布局、旧 `manifest.json`、既有 enabled 状态、Windows/macOS 路径语义、旧会话恢复和旧 package 回退。
- **Payload Harness**：Git/archive/URL、本地目录、`SKILL.md`、资源文件、压缩大小、symlink、路径 traversal、digest 和 slash command 参数。
- **Observation Harness**：安装阶段、稳定错误码、catalog reconciliation、activation、resolver 选择、加载/截断和更新/回滚 receipt。
- **Viewport Harness**：资源中心列表/详情/安装抽屉/错误状态、Workspace Skill chip、窄窗口和长名称/长路径。
- **AI Collaboration Harness**：Agent 搜索、安装、创建、更新 Skill 时记录 context scope、assumptions、review point 和 validation result。
- **Release Harness**：legacy migration、安装版数据目录、CodeFactory App 升级/迁移回退和 exact release artifact 的 `P-SKL-001`；Skill package 更新/回滚属于 P1 `P-SKL-002` 的独立 L4 gate。

## 9. 功能范围与阶段

### Phase 0 — 安全止血（独立紧急修复）

- 所有写入、更新、删除和 marketplace ID 复用同一安全 path validator；
- 本地/Git/OpenClaw 导入改为默认禁用；
- 移除 `skill_*` 前缀级无条件 Allow；search/list/get 是普通 read-only，load/resource-read 是受 activation 与预算约束的 `RuntimeContextRead`，fetch/install/update/delete 重新进入正常 mutation permission gate；
- 官方 marketplace/registry 只访问后端配置的 HTTPS allowlist；用户显式 public Git/HTTPS source 走单独确认；两者都阻断私网/redirect/超限/慢流，官方 registry 额外验证 envelope 与 package digest；
- 暂停或明确标记未接入运行时的 `tool_policy` 与 slash command；
- 失败包不再静默隐藏，最少显示稳定错误码和目标来源摘要；
- 增加攻击性 ID、symlink、absolute path 和删除越界测试。

Phase 0 只封堵当前已知高风险安装、删除、权限与远程入口，不代表全部 P0 Requirement 已完成，也不构成完整 Skill v2。只有 R1-R12/R18/R19 各自对应的 Phase 与 RTM 证据全部通过后，才能关闭完整 P0。

### Phase 1 — 统一包与原子安装

- 实现 `SkillPackage v2`、typed source adapters、staging、validator、immutable package store、SQLite lifecycle index 和 reconciler；
- 迁移 legacy package；
- 资源中心改为读取统一 catalog 和操作状态；
- 所有安装入口返回同一种 operation/receipt。

### Phase 2 — 按需运行时与证据

- 实现 compact catalog、scope activation、resolver、`skill_load`、resource reader 和 turn receipt；
- UI/Headless/恢复任务统一；
- Workspace 分别展示“已匹配 / 已选择 / 加载结果 / 调用来源”；
- 建立真实桌面与跨进程 E2E。

Phase 1 + Phase 2 全部完成并通过 L3/L4 主路径后，只可恢复“可靠安装、审核、启用、按需加载并披露证据”的承诺；不得宣称已经支持更新/回滚、可恢复移除或 slash command。

### Phase 3 — 更新、命令与生态

- 更新 diff、批准、回滚、trash/restore；
- slash command 路由；
- registry/package 更新、第三方作者签名与信任；
- 非 anonymous 回合的去标识聚合指标、质量反馈和团队 registry 设计。

## 10. 成功指标

### 10.1 发布门槛

- 成功安装后 catalog 可见率：`100%`；
- 失败/损坏/不兼容操作的稳定错误码覆盖率：`100%`；
- adversarial path corpus 越界写入/删除：`0`；
- 非现场新建 Skill 自动启用率：`0`；
- 每个 `selected` 或发生 `load_attempted` 的 Skill 都有 load outcome，覆盖率：`100%`；每个回合有 selection summary，允许 `selected_count=0`，不要求为所有 enabled Skill 生成逐项 receipt；
- UI 与 Headless 对同一 fixture 的 package digest/activation/selection 结果一致率：`100%`；
- legacy 合法包迁移成功率：`100%`，不合法包可见率：`100%`；
- 500 个已安装包的本地 catalog P95 加载：`< 200 ms`（不含网络更新检查）。

### 10.2 发布后观察目标

- 安装后 7 天内完成一次有 receipt 的真实加载比例：成功阈值 `>= 70%`，目标 `>= 85%`；
- “安装成功但找不到/未生效”类反馈相较基线下降 `>= 80%`；
- 每个普通回合的 Skill catalog 注入开销 P95 `<= 2,000` 字符；
- 非匹配 Skill 被加载率 `< 5%`，显式选择命中率 `100%`。

这些目标需在实现前补 measurement plan；没有基线时不得宣称已改善。

## 11. Scenario 设计与登记门禁

实现开始前必须在 `docs/testing/scenario-registry.json` 登记以下稳定 Scenario；当前文档只保留建议 ID，不把尚无自动化的计划伪装成已登记 gate：

| 建议 ID | 场景 | 优先级 | 最低证据 |
| --- | --- | --- | --- |
| SKL-001 | 任意来源安装均原子提交并在 catalog 显示真实状态 | P0 | L0 + L1 + L2 + L3 |
| SKL-002 | 恶意包不能逃逸 Skill 根目录或扩大权限 | P0 | L0 + L1 + L2 |
| SKL-003 | 审核启用后只在匹配/显式回合加载并产生 receipt | P0 | L0 + L1 + L2 + L3 |
| SKL-004 | UI、Headless 和恢复任务固定同一 package digest | P0 | L1 + L2 + L3 |
| SKL-005 | legacy 升级、损坏可见和 App migration 回退不丢包 | P0 | L1 + L2 + L3 + L4 |
| SKL-006 | Skill 更新 diff、批准、失败回滚和旧版本恢复 | P1 | L1 + L2 + L3 + L4 |
| UI-013 | 资源中心 Skill 状态、错误、审核和运行证据 | P0 | L0 + L1 + L3 |
| E2E-009 | 安装→审核→启用→UI/Headless 加载→禁用；Phase 3 再扩展更新/回滚/移除 | P0 组合 | L0 + L1 + L2 + L3 + L4 |
| SKL-007 | disable、recoverable remove、restore、purge 和 builtin override | P1 | L0 + L1 + L2 + L3 |
| SKL-008 | slash command 建议、展开、发送和 receipt | P1 | L0 + L1 + L3 |
| OBS-SKL-001 | non-anonymous 聚合指标、anonymous ephemeral receipt 与去敏诊断 | P1 | L0 + L1 + L3 |

实现 PR 不得使用 `Scenario-Test: not-applicable` 绕过上述路径。

## 12. Given / When / Then 核心验收

### 安装与可见性

- Given 一个合法 registry、Git、本地目录或 raw `SKILL.md` source，When 安装返回成功，Then catalog 必须在同一 operation receipt 下显示 Skill ID、版本、digest、来源、review=`unreviewed` 和完整文件清单。
- Given 下载完成后在 manifest commit 前 hard kill，When App 重启，Then reconciler 必须将操作恢复为可继续或稳定失败；不得出现无状态目录或重复安装。
- Given package manifest 损坏，When 用户打开资源中心，Then 显示 `corrupt` 与修复/删除入口；不得把该目录当作不存在。
- Given 批量导入中部分包失败，When 操作结束，Then 显示每个包的成功/失败状态和错误码；总成功数不得掩盖失败项。

### 安全与审核

- Given ID 包含 `..`、absolute/drive/UNC path、不同平台分隔符、Windows reserved name、尾随点/空格、大小写或 Unicode 归一化碰撞、archive symlink/reparse，When 从任一 source 校验，Then 操作以稳定错误码失败；除用户显式授权的 source root 读取外，Skill/staging root 外零读取、零写入、零删除。
- Given remote、Git 或本地导入成功，When 用户未审核启用，Then 新会话、Headless 和恢复任务都不得加载该 Skill。
- Given Skill 声明 tool policy，When 用户启用，Then effective policy 等于 `user ceiling ∩ project ceiling ∩ turn capability ceiling ∩ every loaded Skill restriction`；Skill 不得新增工具或扩大路径/网络权限。
- Given `tool_policy` 缺失，When 计算 effective policy，Then只是不增加 Skill 限制；Given policy 存在但 malformed、含未知字段或越过 ceiling，When UI、Headless 或恢复任务审核/启用/加载，Then fail-closed 并产生稳定错误，已激活项被原子撤销。
- Given package 为 missing/corrupt/incompatible、review 为 rejected/revoked、required capability 不受支持或 policy conflict，When 尝试启用或状态从健康变为异常，Then activation 在同一事务中被拒绝或全部撤销，下一 root turn 不得加载。
- Given 同一 Skill global 已启用，When 当前项目写入 `blocked_in_scope` 或 project package override，Then project 优先且本回合只解析一个 effective package；移除 override 前 UI 明确会恢复 global 还是继续屏蔽。

### 运行时与证据

- Given 多个已启用 Skill，When 用户任务只匹配其中一个，Then compact catalog 可以包含全部候选，但只加载被显式选择或可解释命中的 package，其他正文不进入 prompt。
- Given resolver 评估 Skill，When 回合结束，Then回合有 selection summary；每个 matched-but-not-selected 项有稳定理由；每个 selected/load-attempted 项在 UI 与 receipt 显示 package digest、选择/调用来源、读取资源、字符预算和 load outcome。匿名回合只保留内存态 ephemeral receipt，不写 SQLite/遥测/诊断。
- Given Skill 在回合开始后发布新版本，When 当前回合继续或恢复，Then 仍固定原 digest；只有新 root turn 在批准更新后才能使用新版本。
- Given 用户禁用或回滚 Skill，When 发起下一 root turn，Then resolver 不再加载被禁用版本；历史 receipt 保持可审计。
- Given 任务存在可执行验收器，When outcome verification 结束，Then独立 receipt 记录 criteria/evidence/verifier/status；没有验收器时 UI 显示“结果未验证”，不得由 loaded/completed 推断通过。
- Given 同一等价文件集经 registry、Git、URL 和 local adapter 安装，When 使用 digest v1 计算，Then摘要完全一致；source/time/operation/receipt 变化不改变摘要，任一文件 byte/role/required 标记变化都改变摘要。

### 兼容与发布

- Given 上一正式版本的 legacy skills 目录，When exact candidate artifact 首次启动，Then exact signed builtin digest 可按旧 enabled 事实保留 activation；其他 legacy 包进入待审核且不加载；损坏包可见、原目录备份和回退均得到验证。
- Given migration 中途 hard kill，When 同一 candidate 再次启动，Then migration 幂等继续且不重复 package、activation 或 receipt。

### P1 更新、移除与命令

- Given active v1 与不同 digest 的 v2，When 用户查看完整 diff 并批准更新，Then使用 expected-old-digest CAS 从下一 root turn 切换；并发更新恰好一个成功，失败或 hard kill 始终保留完整 v1，rollback 产生新 activation receipt。
- Given 已禁用 installation，When 用户 recoverable remove，Then进入 tombstone/trash 并可在期限内 restore；open Objective 引用阻止 GC；purge 需要二次明确确认；移除 project override 前明确 builtin/global fallback 结果。
- Given 两个 Skill 声明冲突 slash alias，When 启用或输入 `/`，Then builtin 优先且冲突 Skill 不得静默覆盖；Given 缺参数、Skill 被禁用或 package digest 漂移，When提交命令，Then保留输入、显示稳定错误并不执行旧模板。

## 13. 决策与开放问题

### 已决策

- 内容包不可变，activation 与 review 状态独立存储；不再修改包内 manifest 表示启用。
- SQLite lifecycle index 是状态权威；文件系统 package store 是内容权威，二者由 reconciler 校验。
- 安装与启用分离；远程/导入永不自动启用。
- Skill policy 只能收紧权限。
- 运行时使用 compact catalog + 按需加载，不再全量注入。
- Phase 2 允许模型从 eligible compact catalog 调用 `skill_load`；候选最多 3、自动加载最多 2，调用携带稳定 catalog/rule reference，拒绝或不选择也进入 selection summary/receipt。
- P0 不执行 Skill 自带脚本；未支持的 required capability 阻止审核/启用，未声明为 required 的 script 完整保留为只读附件并允许 prompt-only 启用，但必须显示警告。

### 开放问题

- [Product，非阻塞] 默认作用域只提供“全局/当前项目”，还是首版同时提供 session scope？建议首版只做全局与项目。
- [Security，非阻塞] 官方 registry envelope 使用随 App 发布的 registry key 验签；第三方作者签名、key rotation UX 和团队 registry 信任链进入 Phase 3。
- [Data，非阻塞] 安装到首次加载转化率的当前基线未知，需先定义排除 anonymous turn 的去标识事件再设置正式目标。

## 14. 实现启动与完成门禁

实现不得在以下条件缺失时开始：

- 本规格与配套 business / architecture / UX 设计获批；
- §11 中适用的 `SKL-*`、`UI-013`、`E2E-009` 和 Observation Scenario 已进入机器 registry；当前 `scenario-registry.json` 尚未登记这些建议 ID，因此本文获批也不等于实现门禁已满足；
- Phase 0 threat model 和兼容迁移 fixture 已确定；
- 开发 worktree、Req ID owner、QA owner 和 release owner 已认领。

完整能力不得在以下证据缺失时称为完成：

- 相关单元/集成/跨进程/真实桌面测试；
- PR required checks 和场景治理通过；
- legacy migration 使用上一正式版本数据 fixture 验证；
- exact release artifact 安装/升级后的 `P-SKL-001` 主路径；
- package digest、build identity、运行时 receipt 和截图指向同一候选版本。
