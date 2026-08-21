# Skill 生命周期与运行时系统业务设计

> 状态：Draft for approval
>
> 对应规格：`docs/specs/feature-specs/skill-lifecycle-system.md`
>
> 业务结论：在可靠闭环完成前，CodeFactory 不应把当前能力描述为“可安装并自动使用的 Skill 系统”。

本业务设计覆盖 `CF-SKL-R1..R19`：P0 可信闭环为 `R1-R12/R18/R19`，P1 可维护能力为 `R13-R15/R17`，生态为 `R16`；准确阶段与证据以规格 §6.2 RTM 为准。

## 1. 为什么现在必须重做

Skill 是 CodeFactory 从一次性对话工具走向可复用本地 Agent 的关键能力。如果用户无法确认“装了什么、是否生效、何时使用、出了什么错”，Skill 不但不能减少重复解释，反而会制造三种新成本：

- **信任成本**：资源中心没有显示、启用后无证据、不同入口行为不同；
- **任务成本**：复杂 Skill 被扁平化后悄悄失去 references/scripts/templates，结果看似安装、实际不可用；
- **安全成本**：远程来源、路径写入、更新和删除没有统一边界，Skill 可能成为 prompt 与文件系统的供应链入口。

继续在现有目录扫描器上增加市场条目、自进化提案或更多导入入口，会扩大不可诊断状态。正确顺序是先建立可靠生命周期，再扩生态和自动提议。

## 2. 用户问题

### 2.1 使用者

用户想完成的是“复用一种可靠做事方法”，不是管理几段 prompt。当前产品无法稳定回答：

- 安装是否真正成功；
- 为什么资源中心没有显示；
- 安装后为什么没有生效或为何已经自动生效；
- 本回合有没有使用、是否只加载了一部分；
- Skill 更新后是否改变当前运行中的任务；
- 禁用、删除和回滚是否真的完成。

### 2.2 审核者/项目负责人

审核者需要确认某个确定版本的来源、内容、权限和作用域，而不是笼统批准一个可变名称。当前 `enabled` 无法表达“审核了哪个版本、在哪个项目启用、运行时是否加载”。

### 2.3 Skill 作者

作者需要完整包保真、兼容错误和版本迭代。当前导入把包转成 `system_prompt.md`，作者无法判断哪些能力被保留、哪些被忽略。

### 2.4 支持与维护者

维护者需要 operation ID、稳定错误码、package digest、状态转换和 turn receipt。当前“列表为空”无法区分未安装、安装中断、manifest 损坏、来源不兼容或刷新失败。

## 3. 业务目标

### 3.1 用户价值

- 用户安装后立即知道结果与下一步；
- 用户启用前能够审核确定版本和能力范围；
- 用户在任务中看到 Skill 的选择、加载和失败证据；
- 用户可以安全更新、回滚、禁用和移除；
- 简单 Skill 保持低摩擦，复杂 Skill 不被静默降级。

### 3.2 产品价值

- 让 Skill 从展示性功能变成可验证的复用机制；
- 为 marketplace、自进化提案、团队分发和能力评测提供同一底座；
- 将“安装量/启用量”升级为“审核→加载→显式调用→结果验证”的真实漏斗；
- 降低由隐式 prompt 污染、版本漂移和路径风险造成的产品事故。

### 3.3 工程价值

- 所有入口复用统一 installer/catalog/runtime；
- 以稳定状态机和 receipt 替代文件存在性推断；
- 将 package 内容、生命周期状态和运行证据分层，便于测试、迁移和恢复；
- 为 UI、Headless 与 Objective recovery 提供同一契约。

## 4. 核心产品定义

Skill 是：

> 一个内容不可变、来源可追踪、能力可审核、作用域可控制、运行可证明、版本可回滚的本地 Agent 能力包。

它不是：

- 一条系统提示词；
- 一个安装目录；
- marketplace 的一张卡；
- 一个 `enabled=true` 布尔值；
- 一次模型“好像遵守了说明”的主观判断。

## 5. 六层价值与正交事实

| 层级 | 用户问题 | 产品事实 | 不得冒充的上层事实 |
| --- | --- | --- | --- |
| 已安装 | 包是否完整保存？ | package digest 已原子提交 | 不等于已审核 |
| 已审核 | 用户批准了什么？ | 确定版本与 capability snapshot 已批准 | 不等于已启用 |
| 已启用 | 哪些任务可以考虑它？ | package 在 global/project scope 激活 | 不等于已匹配 |
| 已匹配/已选择 | 为什么认为它相关；最终是谁选择？ | match 与 selection 是独立轴；显式选择可不经自动匹配 | 不等于已加载 |
| 已加载/已调用 | 指令或资源是否进入本回合；是否有显式调用？ | load 与 invocation 是独立轴 | 不等于结果有效 |
| 已验证 | 是否改善或完成了任务？ | 结果通过对应验收/eval | 不能由 prompt 注入推断 |

资源中心、聊天回执、控制平面和指标必须使用规格 §6.1 的唯一词典。`explicit_user/slash_command/model_catalog_call` 是 invocation/selection 来源，不是 `runtime_loaded` 后的下一级；“任务结果已验证”必须有独立 criteria/evidence/verifier receipt，且只表明相关联、不声明因果。

## 6. 产品原则

1. **安装、审核、启用分离**：安装不改变 Agent 行为。
2. **异常不消失**：已提交 package 的损坏/隔离/不兼容属于 catalog；未形成 package 的 fetch/validate/commit failure 属于安装记录。顶部可以聚合，但不能伪造 package。
3. **完整保留或明确拒绝**：不允许 silent degradation。
4. **当前回合固定版本**：更新/禁用只影响下一 root turn。
5. **按需加载而非全量注入**：启用表示可被选择，不表示每次加载。
6. **Skill 不能越权**：policy 只收紧 permission ceiling。
7. **Agent 可建议/安装草稿，不能替用户审核启用**。
8. **证据优先于状态文案**：loaded、called、verified 分开。
9. **本地优先**：离线不改变本地包事实；只有 integrity、compatibility、review、activation 均满足 eligibility 的确定版本可继续使用，更新检查失败本身不撤销它。
10. **先可靠再生态**：marketplace 扩张、自进化和团队分发排在 P0 闭环之后。

## 7. 业务范围

### P0：可信 Skill 闭环

- 统一 package、source adapter、install operation 和 catalog；
- 原子安装、崩溃恢复、错误可见和 legacy migration；
- 默认待审核/禁用；
- 来源、版本、digest、兼容和能力预览；
- global/project activation；
- compact catalog、按需加载和 turn receipt；
- UI/Headless/恢复一致；
- path、symlink、远程 payload、permission ceiling 安全门禁；
- 真实桌面与 release artifact 主路径。

### P1：可维护与高效使用

- 更新 diff、批准、回滚和可恢复移除；
- slash command 真实执行；
- Agent 结构化候选卡、安装进度和资源中心深链；
- 运行活动、上下文成本、匹配精度和安装漏斗；
- source health 与离线 catalog 状态。

### P2：生态

- 作者签名与可信 registry；
- 团队私有分发和组织审批；
- 受控 executable resources；
- Skill 依赖和组合；
- 基于结果 eval 的推荐与自动匹配优化。

## 8. 明确暂缓

- 暂缓扩大 marketplace 条目数量；
- 暂缓把“从使用习惯提议 Skill”作为核心增长功能；
- 暂缓自动安装、自动启用和静默自动更新；
- 暂缓用安装数、启用数或 prompt 注入数宣称用户价值；
- 暂缓执行任意第三方 scripts；
- 暂缓团队云同步和付费生态。

这些能力不是永远不做，而是必须建立在可审计生命周期之上。

## 9. 主业务旅程

### 9.1 发现并安装

1. 用户或 Agent 搜索 Skill；
2. 产品展示来源、作者声明、兼容性、能力和确定版本；
3. 用户发起安装；
4. 后台显示 fetch/validate/commit 进度；
5. 成功进入“待审核”；已形成 package 的异常进入“我的技能/需要处理”，未形成 package 的失败进入“安装记录/失败”；
6. operation record 保留，资源中心实时更新。

### 9.2 审核并启用

1. 用户查看 `SKILL.md`、文件树、trigger、policy、slash commands 和 unsupported 内容；
2. 用户批准确定 package digest；未支持的 required capability 阻止批准/启用，optional script 只读保留并显示警告；
3. 默认选择当前项目 scope，全局 scope 为次级选项；
4. 激活从下一 root turn 生效；
5. UI 明确“启用不表示每个任务都会使用”。

### 9.3 任务中使用

1. composer 通过 backend preview 展示显式选择、自动匹配和未选择的可用 Skill；前端不自行匹配；
2. 提交时 backend 重算并创建 immutable activation/package snapshot；
3. runtime 只加载显式/可解释候选；
4. 回合中分别展示 match、selection、load、invocation；matched-but-not-selected 有稳定理由，selected/load-attempted 有完整 outcome；
5. 结果验证独立记录，不能由 loaded 或任务完成推断。

### 9.4 更新与退出

1. 新版本先作为独立 package 安装；
2. 用户查看内容、policy 和资源 diff；
3. 批准后切换下一 root turn 的 activation；
4. 失败回滚旧版本；
5. 禁用不删除内容，移除默认可恢复，历史 receipt 保留。

## 10. Agent 行为合同

- 用户让 Agent 查找时，先 `skill_search`，返回结构化候选；
- 用户明确选择/要求安装后，才创建 install operation；
- 主动推荐 Skill 时只能推荐，不能自行安装；
- Agent 创建/修订 Skill 时也走 staging/validator，生成未审核、未启用的新 immutable package；
- 修改已启用 Skill 时生成新 revision，旧版继续运行；
- Agent 不得 review/enable、扩大 scope 或删除已启用版本；
- 安装回执必须包含 operation ID、版本、digest、每项结果和“未启用”；
- Agent 不得把 installed、enabled 或 loaded 描述为 verified。

## 11. 市场与信任策略

marketplace v1 是发现目录，不是信任背书：

- 卡片必须显示来源、版本、兼容、能力和本地状态；
- registry item 固定 digest，后端重新获取并验证；
- remote 不可用时明确显示离线/本地 catalog 和 revision；
- 安装不自动启用，更新不自动接受；
- 作者、下载量、评分在签名与审查机制前只作为信息，不作为权限依据。

## 12. 成功指标与漏斗

### 12.1 核心漏斗

```text
install_started
  -> package_committed / failed_visible
  -> review_completed
  -> activation_created
  -> runtime_matched (optional for explicit selection)
  -> runtime_selected {explicit|auto}
  -> runtime_loaded {full|partial|dropped|failed}
  -> invocation {none|explicit_user|slash|model_catalog}
  -> outcome_verified
```

每一层独立计数，不能跨层推断。

### 12.2 上线门槛

- 安装成功可见率 `100%`；
- ghost/half-installed package `0`；
- 失败项稳定错误码覆盖率 `100%`；
- external import 自动启用 `0`；
- 越界写入/删除 `0`；
- 每个 selected/load-attempted package 的 load outcome 披露 `100%`，每回合有 selection summary 且允许 `selected_count=0`；
- UI/Headless package digest 与 resolver 结果一致 `100%`；
- legacy 损坏包可见率 `100%`。

### 12.3 体验与价值假设

- 安装到完成审核的任务完成率 `>= 95%`；
- 安装到首次有 receipt 的显式调用 P50 `< 2 分钟`、P95 `< 5 分钟`；
- slash command 端到端成功率 `>= 99%`；
- backend resolver preview 中自动候选在发送前被用户移除的比例 `< 10%`；
- 因预算被整体淘汰的已启用 Skill 回合 `< 5%`；
- “安装后找不到/未生效”类支持问题下降 `>= 80%`，目标为零。

正式目标需先建立当前基线和去标识 measurement plan，并排除 anonymous turn。

## 13. 发布与承诺恢复

### 13.1 Phase 0 后

可承诺：已封堵已知高风险安装/删除边界；导入默认禁用；错误更可见。

不可承诺：完整 Skill 系统已可用。

### 13.2 Phase 1 后

可承诺：安装、审核、catalog、迁移和异常可见可靠。

不可承诺：任务中已按需正确使用，除非 Phase 2 完成。

### 13.3 Phase 2 + L4 后

只有完成 UI/Headless/runtime receipt、真实桌面主路径、上一正式版本迁移和 exact release artifact 验收，才可以承诺：

> CodeFactory 支持安全安装、审核、启用并按任务使用 Skill，且用户可查看运行证据。

### 13.4 Phase 3 后

才可承诺更新/回滚、slash commands 和生态质量能力。

## 14. 风险与应对

| 风险 | 影响 | 应对 |
| --- | --- | --- |
| legacy Skill 状态变化 | 老用户任务结果变化或不可信 prompt 延续 | initial rollout 仅精确匹配当前签名 builtin digest 可按 release basis 保留；其他 legacy 待审核禁用；grandfathering 仅保留为未来显式决策且必须另做 threat review；保留备份和 L4 canary |
| 审核步骤增加摩擦 | 安装转化下降 | 安装成功后直达审核；默认项目 scope；能力摘要可快速判断 |
| 按需加载增加一次 tool round | 延迟与成本 | deterministic resolver、候选上限、显式选择快速路径 |
| 状态增加导致 UI 复杂 | 用户理解成本 | 正交后组合成少量用户状态；详情保留原始事实 |
| 完整包提高存储占用 | 本地空间 | 内容寻址去重、版本保留策略、可恢复清理 |
| 生态包能力差异大 | 兼容投诉 | 完整保存、compatibility report、unsupported 阻断，不 silent degrade |

## 15. 决策请求

本设计建议批准以下产品方向：

1. 暂停把现有 Skill 描述为完整可用；
2. Phase 0 安全止血单独优先交付；
3. Phase 1 + Phase 2 作为恢复 Skill 产品承诺的最小完整范围；
4. marketplace 扩张、自进化提案和团队生态延后；
5. 默认 activation scope 为当前项目，全局启用为显式次级选择；
6. P0 完整保存 scripts/resources，但不自动执行第三方脚本；required execution 会阻止启用，optional script 只读保留并警告。
