# Skill 生命周期与运行时系统 UX 设计

> 状态：Draft for approval
>
> 对应规格：`docs/specs/feature-specs/skill-lifecycle-system.md`
>
> UX 目标：用户不需要理解文件系统，也能准确判断 Skill 的存储、审核、启用、匹配、加载和验证状态。

本 UX 覆盖 `CF-SKL-R4/R5/R8-R15/R17/R18`，并为 `R1-R3/R6/R7/R19` 的错误、来源与摘要证据提供用户界面；准确阶段与 Scenario 以规格 §6.2 RTM 为准。

## 1. 设计原则

1. **状态说事实**：已安装、已审核、已启用、已匹配、已加载、已验证分别呈现。
2. **异常不消失**：失败、损坏、不兼容、隔离、部分成功都有可操作页面。
3. **安装不等于生效**：安装成功后直达审核，不自动启用。
4. **下一回合边界明确**：启用、禁用、更新从下一 root turn 生效。
5. **运行证据靠近任务**：Skill 使用状态出现在 composer/context 和回合活动中，不只藏在设置。
6. **安全信息可理解**：展示实际 effective capability，而不是无效的“工具策略”标签。
7. **Agent 与 UI 同一语言**：聊天安装卡、资源中心、控制平面和错误码使用同一生命周期词汇。
8. **不以颜色代替状态**：文字、图标、辅助说明和 aria 状态同时存在。

## 2. 用户角色

| 角色 | 核心任务 | 需要看到的证据 |
| --- | --- | --- |
| Skill 使用者 | 找到、安装、启用并在任务中使用 | 安装结果、作用域、匹配/加载状态 |
| 审核者/项目负责人 | 判断来源、能力、权限和版本是否可接受 | 内容/权限 diff、digest、兼容与来源 |
| Skill 作者 | 创建、修订、测试和发布 | 草稿、validation、版本、文件树、兼容报告 |
| 支持/维护者 | 定位“找不到/未生效/失败” | operation ID、错误码、package digest、receipt |

首版不为团队管理员/发布者增加独立后台；未来复用相同对象扩展。

## 3. 信息架构

入口保持：`设置 → 功能 → 资源中心 → 技能`。

```text
资源中心 / 技能
├── 我的技能
│   ├── 全部
│   ├── 待审核
│   ├── 已启用
│   ├── 已禁用
│   └── 可更新
├── 需要处理
│   ├── 技能包问题
│   └── 安装/恢复失败
├── 发现技能
│   ├── 搜索/筛选
│   └── Skill 详情/安装
└── 安装记录
    ├── 进行中
    ├── 成功/部分成功
    └── 失败/已恢复
```

### 3.1 顶部摘要

顶部显示四个可点击计数：

- 待审核 N
- 已启用 N
- 需要处理 N
- 可更新 N

计数来自统一 projection，不由前端推断。点击“需要处理”进入统一分组页：上组是 package 的 corrupt/missing/incompatible，行可进入 Skill 详情；下组是未形成 package identity 的 quarantine/recovery/fetch/validate failure，行进入安装记录。不能为 operation 失败伪造 Skill 行，也不能让用户先猜计数属于哪类。

### 3.2 我的技能列表

每行最少显示：

- 名称、版本；
- 来源：内置/市场/Git/URL/本地/Agent 创建/旧版迁移；
- review 状态；
- activation scope：未启用/当前项目/全局；
- health：正常/需要处理/不兼容；
- update 状态；
- 最近一次 loaded 时间（没有则显示“尚未加载”）。

禁止直接显示 `user`、`builtin` 之类内部枚举作为主要文案。

列表行点击打开详情；行内 `审核`、`启用`、`禁用`、`更多` 是独立可聚焦按钮，不嵌套交互控件。删除、修复和重试不能只在 hover 出现。

### 3.3 发现技能

市场卡片显示：

- 名称、描述、作者声明、版本；
- registry/source 与 catalog revision；
- 官方 registry envelope 验证状态；
- CodeFactory 兼容性；
- 已安装版本、是否有更新；
- 能力摘要：instructions、resources、slash commands、scripts present、tool restrictions；
- 明确的 `查看详情` 与 `安装` 文字按钮。

远程失败后使用本地 catalog 时，页面顶部固定提示：

> 当前无法连接技能目录，正在显示随 CodeFactory 提供的离线目录（版本 {revision}）。离线不改变已安装技能的本地状态；是否可用取决于完整性、审核和启用状态。

不得把 fallback 冒充远端最新。

### 3.4 安装记录

每次 operation 展示：

- source 摘要、开始/结束时间、operation ID；
- queued、fetching、staged、validating、committing、succeeded/succeeded_with_errors/recoverable_failed/failed/rolled_back 等 operation 阶段；另显示 pending/recovered/recovery_failed 恢复 badge；审核是安装后的 package 状态，不是 operation 阶段；
- discovered/succeeded/failed 数量；
- 每个 package 的版本、digest、状态和错误码；
- 可安全恢复的技术中断由 Objective recovery 自动续跑；需要凭据、来源选择等用户输入时展示精确输入门禁，输入后自动继续；用户另可继续审核、打开详情或取消并移除暂存。

批量导入可以部分成功，但总结果必须写成：

> 已安装 3 个技能，2 个未安装。查看每项结果。

不能只返回成功数量。

## 4. Skill 详情

详情使用五个 section，桌面可用页内导航，窄屏使用单列：

### 4.1 概览

- 名称、描述、版本、package digest；
- 来源、resolved Git commit 或 registry snapshot；
- 当前 review、activation scope、health、update；
- trigger 摘要和最近 loaded 状态；
- 主操作按钮根据状态变化。

### 4.2 审核

审核必须回答：

1. 来源和确定版本是什么？
2. 何时会被匹配？
3. 包含哪些 instructions/resources/commands/scripts？
4. 声明了哪些工具限制；实际 effective policy 是什么？
5. 哪些内容当前不支持，会阻断启用还是只读保留？

能力摘要使用真实运行结论：

- `可用：指令、只读引用资料`
- `可用：斜杠命令（2）`
- `受限：只允许 read/grep`
- `可只读保留：optional scripts（不自动执行）`
- `阻断启用：required script execution 当前不支持`

不得只显示“工具策略”或文件存在图标。

### 4.3 内容

- 友好预览 `SKILL.md`；
- 可切换原文；
- 文件树展示 resources/scripts/templates/assets；
- 文件显示 size、digest、role 和 compatibility；
- 支持复制错误/manifest，不默认执行任何内容。

### 4.4 版本

- 当前批准版本；
- 待审核 revision；
- 来源可用更新；
- manifest、instructions、resources、policy diff；
- 回滚目标和保留策略；
- 当前运行中的 root turn 仍固定旧版本提示。

### 4.5 活动

按时间展示：

- installed/reviewed/enabled/disabled/updated/rolled back/removed；
- match/selection/load/invocation 四个正交轴；
- selection rule、effective scope/source、被 project override 覆盖的 global 项、package digest；
- context 字符/token、截断原因；
- outcome evidence 关联。

活动页必须注明：

> “已加载”表示 Skill 内容进入了本回合上下文，不代表它一定改变了模型输出或改善了任务结果。

只有关联独立 criteria/evidence/verifier receipt 时才显示“任务结果已验证”；否则显示“结果未验证”。即使有验证，也注明它与本次 Skill load 相关联，不宣称因果。

## 5. 主要交互旅程

### 5.1 市场安装

1. 用户在“发现技能”搜索；
2. 进入详情查看来源、兼容、能力和文件摘要；
3. 点击 `安装技能`；
4. 安装抽屉显示解析来源、下载、校验、提交进度；
5. 成功后显示：

   > “{name}” v{version} 已安装，尚未启用。审核后可选择使用范围。

6. 焦点移动到 `审核此版本`；
7. 审核通过后选择 scope；
8. 返回 Workspace 后，从下一 root turn 起可参与匹配。

### 5.2 Agent 查找并安装

1. 用户要求查找；Agent 调用 `skill_search`；
2. 聊天渲染与市场一致的候选卡；
3. 用户明确选择后，Agent 创建 install operation；
4. 聊天中显示实时 operation card；
5. 成功后显示：

   > 已安装 {N} 个技能，均等待审核。我没有启用它们，也没有修改当前回合。

6. 卡片提供 `审核技能` 深链；
7. 资源中心通过 catalog event 实时出现，不需重开。

Agent 主动认为 Skill 可能有帮助时，只能推荐，不得自行安装。

### 5.3 本地/Git/OpenClaw 导入

1. 用户选择 source；
2. 系统先扫描并显示发现项；
3. 每项标记可导入、已存在、损坏、不兼容、ID 冲突；
4. 用户选择要安装的 package；
5. 每项独立原子提交、统一进入待审核；
6. 包含 unsupported scripts/resources 时，完整保留并明确 compatibility 结论；
7. 不允许“只复制 prompt 但返回成功”。

### 5.4 Agent 创建或修改

- `skill_create_draft`/`skill_revise_draft` 将内容送入同一 staging/validator，生成 immutable、unreviewed、disabled package；
- 每次保存未启用 draft 都产生新 immutable digest/package；UI 以同一 draft lineage 分组并把旧 draft 标为 superseded；
- 修改已启用 Skill 必须生成新 revision：

  > 更新已保存为待审核版本。当前 v1.2 继续生效；审核新版本后才会切换。

- Agent 不能启用、扩大 scope、purge 或覆盖当前 active revision。
- UI 创建提供 `保存草稿` 与 `审核并启用`；后者只是连续旅程，仍分别显示并保存 install、review、activation receipt。

### 5.5 启用

启用 dialog：

> 启用“{name}” v{version}？
>
> 从下一回合起，它可以在所选范围内被匹配并加载。启用不表示每个任务都会使用。

scope 使用有可访问名称的 radio group：

- `仅当前项目`（默认）
- `所有项目`

`暂不启用` 是 dialog 的取消动作，不是第三个 scope。若同名 global Skill 已启用，用户在当前项目选择禁用时写入明确 project negative override，并显示“本项目将屏蔽全局版本”；删除该 override 前必须说明会恢复 global 还是继续屏蔽。

若存在 unsupported/blocking capability，启用按钮禁用并解释原因。

### 5.6 禁用与移除

禁用：

> 禁用“{name}”？正在执行的回合不受影响；下一回合起不再选择或加载。

移除：

> 移除“{name}”？这会移除 CodeFactory 保存的 Skill 包和配置，不会删除原始本地目录或远程仓库。历史运行记录会保留。

按钮使用 `禁用技能` / `继续保留`、`移除技能` / `保留技能`，不使用含糊的“确定”。默认移入可恢复 trash；永久清理另行确认。

## 6. Workspace 运行时体验

### 6.1 Composer “本回合资源”

在 composer 上下文入口中增加 Skill 分组。输入变化后前端调用 backend `preview_skill_resolution`；发送时 backend 用同一规则重算并固定 snapshot，前端不得自行匹配：

- 显式选择；
- 自动匹配；
- 可用但未选择。

每项显示名称、版本、scope 和选择理由。发送前用户可移除自动匹配项或手动添加；发送后 snapshot 固定。

紧凑 chip 分别表达事实：

- `已匹配 · Release PR Writer`
- `用户已选择 · Release PR Writer`
- `自动已选择 · Release PR Writer`
- `已匹配但未选择 · Release PR Writer`
- `已加载 · Release PR Writer`
- `部分加载`
- `未加载 · 预算不足`
- `加载失败`

chip 展开后展示 match、selection、load、invocation、package digest 短值、rule reason、loaded resources、context cost 和 receipt ID。普通回合只需 selection summary；仅 selected/load-attempted 项显示完整 load chip，不能要求所有 enabled Skill 每回合逐项出现。

anonymous turn 的 summary/receipt 仅在当前回合内存活，UI 明确提示“匿名回合结束后不保存此 Skill 运行记录”，活动页与诊断导出不得出现该记录。

### 6.2 运行边界

- 用户显式选择优先；
- 自动匹配必须有可解释 reason；
- 没有可靠候选时不加载；
- 禁用/更新不会在当前 root turn 中途改变 snapshot；
- 恢复任务显示“固定使用 vX/digest”，缺包时停止并说明，不换新版本继续。

### 6.3 Slash commands

输入 `/` 时分组：

```text
系统命令
  /model
  /cost

技能命令
  /code-reviewer.review
  /release-pr.write
```

- 名称冲突时强制命名空间；唯一且安全时可提供短 alias；
- 选择命令后在输入框展开模板，并显示“显式调用”；
- 缺参数时保留输入并聚焦参数，不吞掉发送；
- 命令被禁用/版本变化时显示稳定错误和打开 Skill 详情动作；
- 只有端到端接通后，资源中心才显示“斜杠命令可用”。

## 7. 状态与文案

| 状态 | 主文案 | 次要说明/动作 |
| --- | --- | --- |
| 空 catalog | 还没有技能 | 从技能目录安装、导入本地包或创建草稿 |
| review=`unreviewed` | 已安装，等待审核 | 查看来源、能力和内容后启用 |
| legacy grandfathered（未来保留） | 兼容启用，待复核 | initial rollout 不使用；未来若放开，仅保留旧 prompt-only 能力，新增能力尚未批准 |
| enabled | 已为当前项目启用 | 从下一回合起可被匹配，不代表每次使用 |
| corrupt package | 技能文件存在，但内容无法验证 | 未启用；根据 `SKILL_MANIFEST_INVALID`、`SKILL_DIGEST_MISMATCH` 或 `SKILL_RESOURCE_MISSING` 查看详情 |
| incompatible | 当前版本无法启用此技能 | 查看不兼容字段和最低版本 |
| optional scripts | 已保存 optional scripts | 可只读查看；不会自动执行，prompt-only 能力仍可启用 |
| required scripts | 当前版本要求执行 scripts | CodeFactory 尚不支持，无法审核/启用 |
| quarantined artifact | 安装内容已隔离 | 尚未形成 Skill 包；在安装记录查看安全或校验错误 |
| partial batch | 已安装 3 个技能，2 个未安装 | 查看每项结果 |
| recoverable failed | 安装中断，等待恢复 | 暂存仍保留；继续或取消并回滚 |
| commit rolled back | 安装中断，现有版本未受影响 | 仅在 reconciler 确认 `rolled_back` 后显示“已回滚”；否则显示“等待恢复/可继续” |
| list refresh failed | 暂时无法刷新技能列表 | 只有存在 succeeded operation receipt 时补充“安装已完成”；否则显示“暂时无法确认安装结果” |
| budget dropped | 已启用，但本回合未加载 | 上下文预算不足；查看或减少自动 Skill |
| partial load | 本回合只加载了可选内容的一部分 | 显示截断位置；required 内容缺失必须显示“加载失败” |
| selection conflict | 两个 Skill 的规则冲突 | 本回合未自动选择；请选择一个 |
| source offline | 无法检查来源更新 | 离线不改变本地状态；是否可用取决于完整性、审核与启用状态 |

错误详情必须换行、可复制，展示稳定 error code 和 operation ID；不能单行 truncate。

## 8. 控制平面状态

AI Coding OS / Capability summary 不能再仅按数量返回 `Ok`：

- `Ok`：installed package 可解析，active revision 可加载，无 recovery failure；
- `Warning`：待审核、旧版兼容、source offline、部分加载、update pending；
- `Error`：corrupt、missing active revision、incompatible active、transaction recovery failed；
- 点击 Skills 进入资源中心对应筛选。

摘要分别显示 installed/reviewed/enabled/needs-attention/recently-loaded，禁止只显示 total/enabled。

## 9. 可访问性

- 所有状态同时使用文本、图标与必要颜色；
- 顶部四个计数是 filter button，使用 `aria-pressed` 表示当前筛选；
- scope 使用具名 radio group；`暂不启用` 是取消 button；
- 可展开 runtime chip 是 button，并暴露 `aria-expanded`/`aria-controls`；
- drawer/dialog 具备 focus trap、Escape 关闭和关闭后的焦点恢复；
- 安装 stage 变化通过 polite live region 播报，错误使用 assertive；字节进度不逐条刷屏；
- row selection 与行内操作是独立 button；
- 键盘可访问安装、审核、scope、diff、文件树、错误复制、回滚；
- dialog 标题描述具体 Skill 与版本，危险动作聚焦在安全默认按钮；
- 完成安装/关闭 dialog 后，焦点返回触发控件或新安装项；
- 长名称、长描述、长 source 和错误可换行，不遮挡主要操作。
- 375px 的 diff/文件树横向滚动区可键盘聚焦，并有可读名称。

## 10. Viewport

### 1366×768 及以上

- 左侧列表/右侧详情双栏；
- 安装记录可作为右侧 drawer；
- 详情 section 内滚动，顶栏/主操作固定但不遮挡内容。

### 800×600

- 列表单栏；详情使用全高 drawer；
- 顶部计数横向滚动或 2×2；
- 主操作固定在 drawer footer。

### 375px

- 单列返回式导航；
- 状态 chips 换行；
- diff、文件树和原文横向滚动；
- 安装进度使用垂直步骤；
- 44px touch target，主要动作不被系统安全区域遮挡。

## 11. 验收路径

### P0 成功路径

- 市场安装 → 待审核 → 当前项目启用 → 下一 root turn 自动匹配 → 完整加载 → receipt 可见；
- Agent 搜索/安装 → 聊天 operation card → 资源中心实时出现 → 用户审核；

### P1 扩展路径

- 更新 diff → 批准 → 失败保持旧版 → 回滚；
- disable → recoverable remove → restore/purge；
- 显式 slash command → 模板展开 → 发送 → 显式调用 receipt。

### 边界路径

- 批量导入部分失败；
- 同 ID/command alias 冲突；
- 500 个 package 列表与搜索；
- 多 Skill context budget 截断/淘汰；
- 当前回合中更新/禁用仍固定旧 snapshot；
- source offline 不改变本地状态；只有 integrity、compatibility、review、activation 均满足 eligibility 时才继续使用本地确定版本。

### 失败路径

- manifest/resource/digest 损坏；
- unsupported capability；
- install commit 前后 hard kill；
- catalog refresh 失败；
- active package 在恢复时缺失；
- migration 部分失败；
- path/symlink 安全拒绝。

所有 UX 行为必须在真实 CodeFactoryDev 或 exact release artifact 中验证；jsdom 只能证明渲染合同，不能替代安装、WebView、重启和运行时加载。

## 12. UX 发布门槛

- 所有 package 状态在 UI 有对应呈现和动作；
- 已存在目录不会被空状态吞掉；
- 安装/审核/启用文案不再混用；
- 每个回合显示 selection summary；每个 selected/load-attempted Skill 显示 `loaded_full/loaded_partial/dropped/failed` 之一，matched-but-not-selected 显示理由；
- 安装、Agent tool 和 migration 触发的 catalog 变化无需重开页面；
- 删除/更新/启用作用边界明确到下一 root turn；
- 1366×768、800×600、375px 关键路径无溢出和遮挡；
- filter/radio/chip/dialog/live-region/可聚焦滚动区通过 L3 键盘与读屏验收；
- L3 真实桌面与 L4 正式产物走 P0 `P-SKL-001`；P1 更新/回滚/移除/命令分别走 `P-SKL-002` 与对应 Scenario，不阻塞 P0 承诺。
