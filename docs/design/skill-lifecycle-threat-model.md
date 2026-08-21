# Skill 生命周期安全与可靠性威胁模型

> 状态：Draft for approval
>
> 基线：`origin/main@d4386979cbde398912bb91cd62cba8494b49c52a`
>
> 对应架构：`docs/design/skill-lifecycle-architecture-design.md`

本威胁模型覆盖 `CF-SKL-R2/R3/R5-R12/R17-R19`；供应链扩展 `R16` 属 Phase 3，P0/P1 阶段与 Scenario 以规格 §6.2 RTM 为准。

## 1. 保护资产

| ID | 资产 | 安全目标 |
| --- | --- | --- |
| A1 | 用户项目、HOME、CodeFactory 配置及任意本机目录 | Skill 操作不得越界读写删 |
| A2 | Agent 系统指令和决策权 | 未审核内容不得持久进入上下文或覆盖高优先级规则 |
| A3 | Skill 包、版本、来源和启用状态 | 完整、可追踪、不可被并发混写 |
| A4 | 用户/项目/回合/tool permission ceiling | Skill 只能收紧，不能扩大 |
| A5 | registry、Git/URL、publisher 与 canonical package digest | preview、install、update 绑定同一可信内容 |
| A6 | 安装、更新、迁移的一致性 | crash 后只有旧完整版本或新完整版本 |
| A7 | install/activation/runtime receipt | 能解释 actor、版本、作用域、选择和结果 |

## 2. 信任边界

```text
TB1 Renderer ──> Tauri backend
TB2 Provider/model ──> skill_* tools
TB3 Registry/Git/URL ──> quarantine
TB4 Local/OpenClaw directory ──> quarantine
TB5 Validated package ──> immutable installed revision
TB6 Reviewed/activated revision ──> Agent context/tool gateway
TB7 Builtin release artifact ──> user activation/override
```

规则：renderer、model、remote source 和 local import 都不属于包内容权威。只有 backend validator 产生的 package digest 可以进入 installed store。

## 3. 当前 P0 风险

### 3.0 基线证据索引

以下行号固定到文档顶部记录的 `origin/main` SHA，实施前需重新定位：

| 证据 | 当前代码 |
| --- | --- |
| catalog 对缺失/损坏 manifest 静默 `continue` | `src-tauri/src/commands/skills.rs:75-105` |
| URL installer 只在单一路径调用 `is_safe_skill_id` | `src-tauri/src/commands/skills.rs:247-305` |
| 本地目录 import 固定 `enabled=true` | `src-tauri/src/commands/skills.rs:485-506` |
| update/delete 直接 `user_skills_dir().join(id)` | `src-tauri/src/commands/skills.rs:642-685` |
| marketplace 直接使用 renderer/remote `skill.id` 写入 | `src-tauri/src/commands/skills.rs:1014-1044` |
| enabled prompts 全量进入 agent context | `src-tauri/src/commands/skills.rs:735-770`、`src-tauri/src/agent/mod.rs:1104-1115` |
| `skill_*` permission 前缀级 Allow | `src-tauri/src/agent/mod.rs:2825-2829` |
| `tool_policy.json` 只检查存在并展示 | `src-tauri/src/commands/skills.rs:175-189`、`src/pages/Skills/SkillsPage.tsx:687-691` |
| Skill slash commands 未接入 Workspace props/handler | `src/components/MessageInput.tsx:55-70,200-228`、`src/pages/Workspace/WorkspacePage.tsx:878-915` |

### P0-1 任意路径更新与递归删除

当前 `skill_update`/`skill_delete` 接收模型提供的自由 `id`，内部直接 `user_skills_dir().join(id)`；delete 最终 `remove_dir_all`。相同安全校验没有覆盖所有入口，且 `skill_*` 被权限层前缀级放行。

可能结果：`../`、absolute path、Windows drive/UNC 或平台分隔符逃逸到 Skill 根外；误调用、恶意 Skill prompt 或 provider 污染均可能触发。

硬控制：

- 强类型 `SkillId`；
- update/remove 只接受 DB 解析的 package/revision identity；
- handle-relative/no-follow/reparse-safe 文件操作；所有攻击性输入在第一次根外读取或 mutation 前拒绝；
- mutation 重新进入正常 permission gateway；
- destructive remove 默认 trash，可恢复。

### P0-2 Marketplace 越界写与不可见成功

当前 marketplace 安装使用 renderer/remote object 提供的 `skill.id` 直接 join/create/write，未复用安全 ID 校验。恶意 ID 可写入 Skill 根外，而 catalog 只扫描根内直接子目录，因此可能同时出现“安装返回成功”和“资源中心没有条目”。

硬控制：

- renderer 只提交 registry snapshot/package identity；
- backend 验证 signed registry envelope、digest 和 safe ID；
- 安装只写随机 staging，校验通过后原子提交；
- final catalog 以 DB package receipt 投影。

### P0-3 不可信导入立即启用

本地目录/OpenClaw 路径当前可以安装后立即 enabled，下一回合把 prompt 持久注入 Agent。

硬控制：所有外部/导入 source 只能到 `installed + unreviewed + disabled`；enable 保留独立 UI 审核。

### P0-4 虚假 tool policy

当前只判断 `tool_policy.json` 是否存在并显示标签，没有确定性 consumer；用户可能误认为 Skill 已受限制。

硬控制：

```text
effective permission = user ceiling
                     ∩ project ceiling
                     ∩ turn capability
                     ∩ skill restrictions
```

在执行链接通前，UI 必须显示 unsupported，而不是“工具策略已生效”。

## 4. 供应链威胁

| 威胁 | 当前风险 | 控制 |
| --- | --- | --- |
| mutable registry TOCTOU | search 与 install 可读取不同内容 | signed snapshot digest + immutable package digest |
| renderer 篡改 object | 前端可回传任意 id/prompt/commands | backend 按 registry ID 重取并验证 |
| registry account compromise | 攻击者替换目录和 package | official envelope signature、key rotation、release-bound public key |
| 作者身份伪造 | author/version/tags 是字符串 | 首版不作为信任依据；Phase 3 publisher signature |
| Git ref 漂移 | branch/tag 内容改变 | resolve 并固定 commit SHA，禁 submodule |
| raw URL 内容漂移 | 相同 URL 返回不同正文 | expected/observed digest，review 绑定 digest |
| archive traversal/bomb | 越界写或磁盘耗尽 | entry-before-write 校验、文件/大小/深度/ratio 配额 |
| SSRF/redirect | 访问本机/私网/metadata | 官方 registry configured HTTPS allowlist；用户显式 public Git/HTTPS source 走确认与同一 DNS/redirect 私网拒绝；private source 默认拒绝 |
| 慢流/无限响应 | 卡死或内存耗尽 | connect/total timeout、有界 streaming、content length/cap |

## 5. Package 安全

### 5.1 ID

- lower-case portable stable ID；
- Unicode NFKC 后校验和 collision 检查；
- 拒绝空值、`.`、`..`、任意分隔符、absolute/drive/UNC；
- 拒绝 Windows reserved names、尾随点/空格、casefold collision；
- ID 长度、字符集集中定义，不在各入口复制 regex。

### 5.2 Path 与文件

- 每个 archive/resource path 在写入前逐 component 校验；
- 文件系统访问只能以已打开 root handle 逐级 no-follow/reparse-safe 解析；禁止“先 canonicalize parent、再按字符串路径访问”的 TOCTOU 模式；
- 本地 source 授权只能由 backend native picker callback 签发短期一次性 opaque handle；renderer/model 提交的 `PathBuf` 或路径字符串永远不是授权。授权提交点是 callback 内 no-follow open 成功时，之后扫描与导入只复用该 directory handle。OpenClaw well-known roots 只有在用户点击专用导入动作后由 backend 打开。能主动控制同一用户文件系统并在 callback 的 path-return/open 窗口把真实目录替换成另一真实目录的本机攻击者不在 Phase 0 包威胁模型内，列为 Phase 1 native file-identity/bookmark hardening；静态 symlink/reparse/hardlink 与 open 后替换仍必须 fail-closed；
- 拒绝 symlink、hardlink、reparse point、device/FIFO/socket；
- 不允许 optional copy error 被忽略；
- manifest 声明的 resource 必须存在并匹配 size/digest；
- 未支持的 required capability 阻止批准/activation；optional script 完整保留且只读，不自动执行；

### 5.3 内容保真

- 保留原 `SKILL.md` 与声明资源；
- scripts/templates/references/assets 不静默丢弃；
- P0 不自动执行 scripts；
- 二进制资源走现有 payload policy；
- Skill prompt 作为低于平台/用户/repository authority 的独立不可信 context block。

## 6. 生命周期可靠性

### 6.1 非原子与混版

风险：prompt、manifest、commands、policy 顺序写入，任一步失败留下半包；更新缺少清理，可能混合新 prompt 与旧 policy；并发安装同 ID 交错。

控制：

- random UUID staging；
- package 完整校验后 immutable content-addressed commit；
- stable skill ID 级锁；
- update 使用 expected old digest/CAS；
- DB transaction 切 activation pointer；
- startup reconciler 处理 committing/orphan/missing/corrupt；
- failpoint/hard-kill matrix。

### 6.2 更新和回滚

- 更新创建新 revision，不覆盖 active package；
- 当前 root turn 固定旧 digest；
- 用户批准后从下一 root turn 切新 pointer；
- 失败继续使用旧 revision；
- rollback 是新的审计动作；
- 被 open objective receipt 引用的 revision 不得 GC。

### 6.3 删除

- disable 与 remove 分离；
- remove 默认 tombstone/trash，可恢复；
- purge 需要明确确认；
- builtin override 删除必须解释“移除覆盖后内置版本会重新出现”或提供隐藏 builtin 状态；
- 历史 receipt 保留，不引用已删除的原始正文。

## 7. Prompt 与 Runtime 威胁

| 威胁 | 控制 |
| --- | --- |
| 所有 enabled prompt 全局污染 | compact catalog + explicit/explainable resolver + `skill_load` |
| Skill 尝试覆盖用户/AGENTS/policy | context priority wrapper + deterministic tool gateway |
| 未审核正文进入模型 | catalog 只含满足 eligibility 的 effective activation；initial rollout 仅 exact signed builtin digest 可用 `builtin_release` basis 保留 activation，其他 legacy 全部待审核禁用；正文仅 `skill_load` |
| 上下文超限静默截断 | receipt 记录 included/truncated/dropped；required 内容不足时 fail closed |
| 回合中版本漂移 | root-turn activation snapshot 与 package digest pin |
| UI/headless 行为差异 | AppHandle-independent shared resolver/runtime |
| 恶意 Skill 自改/删其他 Skill | mutation 不 auto-Allow；Agent 无 enable/purge 权限；DB identity target |
| slash command 冒充系统命令 | namespace/collision gate，builtin priority，明确 package digest |

## 8. 隐私与审计

允许记录（non-anonymous turn）：

- operation/receipt ID；
- skill ID、version、package digest、source kind；
- 状态转换、错误码、文件/字节计数；
- match/selection/load/invocation 正交状态与 selection rule ID；
- loaded resource path digest、字符数、截断状态；
- effective grants 摘要。

禁止外部遥测：

- 完整 `SKILL.md`、prompt、用户消息；
- 原始本机路径；
- Git/URL 中的 token/query secret；
- 敏感工具参数和资源正文；
- 可反查私人项目的 scope key。

本地诊断导出必须由用户显式触发并预览去敏内容。

anonymous turn 只保留内存态 ephemeral receipt，结束后释放；不得写 SQLite、遥测或诊断导出。任务结果必须由独立 criteria/evidence/verifier receipt 验证，`loaded` 或回合完成不等价于 outcome passed。（`CF-SKL-R18`）

## 9. 安全验收

### `SKL-SEC-001` Path corpus

Given `../../victim`、`..\\victim`、absolute、drive、UNC、`CON`、`foo.`、Unicode/casefold collision 和 symlink/reparse，When 从 UI、Agent、registry、Git、本地 import、update、remove 任一路执行，Then 在第一次未授权根外读取或 mutation 前拒绝；除用户显式 source handle 授权的只读 root 外，Skill/staging root 外 sentinel tree digest 完全不变。

### `SKL-SEC-002` Registry snapshot

Given preview snapshot A 后 registry 变为 B，或 envelope signature/package digest/expiry 任一不符，When install，Then 只允许安装精确 A digest 或失败，并记录稳定 reason。

### `SKL-SEC-003` SSRF 与配额

Given localhost、RFC1918、link-local、metadata、IPv6 loopback、DNS rebind、redirect-to-private、慢流、超限响应和 archive bomb，When fetch，Then 在配置上限内拒绝并清理 staging。

### `SKL-SEC-004` Permission intersection

Given Skill 声明只读却要求 bash/write/delete/skill mutation，When UI 与 Headless 执行，Then deterministic gateway 拒绝，receipt 关联 Skill digest 与 denied tool；Skill 声明不能扩大 ceiling。

Given policy 文件存在但 malformed、含未知字段或 over-ceiling，When UI、Headless 或恢复任务审核/启用/加载，Then fail-closed；若包已激活则原子撤销 activation。只有 policy 文件完全缺失时才解释为“不附加 Skill 限制”。

### `SKL-SEC-005` Malicious prompt

Given已审核 Skill 内容尝试忽略平台/用户/仓库规则、修改其他 Skill 或自我提权，When load，Then高优先级 authority 与 tool gateway 保持有效，未授权 side effect 为零。

## 10. 可靠性验收

### `SKL-REL-001` Atomic failpoint matrix

Given active v1，When 在 fetch、validate、rename、DB commit、event emit、migration 各 checkpoint 注错/hard kill，Then重启后 catalog/runtime 只能看到完整 v1 或完整 v2，绝无 partial/mixed。

### `SKL-REL-002` Concurrent update

Given 两个不同 digest 同时更新同一 skill ID，When race，Then 恰好一个 CAS 成功，另一个得到 revision conflict，文件不交错。

### `SKL-REL-003` Full package fidelity

Given `SKILL.md + scripts + references + templates + assets`，When import，Then受支持文件 byte/digest 一致；未支持的 required capability 阻止 activation，optional 内容完整只读保留并警告；无 silent drop。

### `SKL-REL-004` Broken visible

Given缺 manifest、坏 JSON、缺 resource、坏 slash command 或 package missing，When打开资源中心，Then显示 corrupt/quarantined/incompatible 和逐项错误，不得静默消失。

### `SKL-REL-005` Cross-entry parity

Given同一 package，When分别经 marketplace、URL、Git、本地/OpenClaw、UI、Agent 安装，Then进入同一 state machine、默认 disabled，并得到同一 package digest/错误语义。

## 11. Threat → Requirement → Scenario 追踪

| Threat/acceptance | Requirement | Scenario | 最低证据 |
| --- | --- | --- | --- |
| P0-1 / SEC-001 | R2/R6/R17 | SKL-002、SKL-007(P1 remove) | L0/L1/L2；remove UX L3 |
| P0-2 / SEC-002/003 | R2/R3/R7/R19 | SKL-001、SKL-002 | L0/L1/L2 |
| P0-3 | R5/R11 | SKL-003、UI-013、SKL-005(P1) | SKL-003 L0-L2；UI-013 L0/L1/L3；SKL-005 L1-L4 |
| P0-4 / SEC-004/005 | R8/R9/R10 | SKL-002；SKL-004/009(Phase 2) | Phase 0 权限 L0/L1/L2；Phase 2 runtime L0/L1/L2/L3 |
| REL-001/002 | R3/R13 | SKL-001；SKL-006(P1) | P0 install L1/L2；P1 update L1-L4 |
| REL-003/004/005 | R1/R2/R4/R10/R11 | SKL-001/004/005、UI-013 | 按规格 §6.2 RTM 与 §11 各场景层级 |
| privacy/outcome | R12/R15/R18 | SKL-009(Phase 2)、OBS-SKL-001(P1) | L0/L1/L3 |

## 12. 发布门槛

- P0 path/permission/remote trust 风险全部修复并有攻击性测试；
- 未实现编译期 builtin digest eligibility 时，builtin manifest 的历史 enabled bit 一律不得进入 runtime；
- 当前交付阶段适用的 `SKL-002`、`SKL-003`、`UI-013` 已进入机器 scenario registry；`SKL-001`、`SKL-004..009` 与组合 `E2E-009` 必须在对应 Phase 实现开始前登记；
- P0 containment install failure matrix 通过；migration/reconciler 属 Phase 1 `SKL-005`，update/rollback crash matrix 属 Phase 3 `SKL-006`，均不冒充当前 Phase 0 证据；
- UI 与 Headless 主路径使用同一 package digest；
- exact release artifact 验证上一正式版本迁移、启用、加载和禁用；package update/rollback/remove 分别在 P1 Scenario 验收；
- `tool_policy`/slash commands 未执行时，从产品承诺中移除或明确 unsupported；
- 不允许用 unit test、HTTP 200、安装命令成功或资源中心一行替代上述证据。
