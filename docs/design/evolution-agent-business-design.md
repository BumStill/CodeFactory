# Evolution Agent 业务设计

## 1. 决策

CodeFactory 要把 Session 从“聊天历史”升级为可审计的改进原料，形成：

```text
真实执行轨迹 -> 可解释信号 -> 人工裁决 -> Harness / 知识 / Evals -> 受控生效
```

这不是新增一个分析看板，也不是让模型自行修改产品。首要目标是让现有自进化能力建立在真实、脱敏、可回溯的数据上。

## 2. 当前产品事实

CodeFactory 已有 session、message、task、learning、knowledge、skill、evidence、benchmark 和人工批准 UI。现有 P1/P3/P4 还不能视为真实闭环：Agent 只把工具调用 JSON 写进 `messages.tool_calls`，而跨会话挖掘、工具门控和自我改进查询规范化 `tool_calls` 表；该表在真实 Agent 路径没有写入。普通聊天虽然触发 post-mortem，后端却只分析 `task_runs`，因此常返回空。

业务上必须先修“观察层数据真相”，否则继续做聚类、Review 工作台或指标看板只会把空数据包装成能力。

完成真实轨迹底座后，审核能力也不能继续埋在「我的画像」的长页面里。产品需要一个一级「进化审查」入口，让用户从真实轨迹、分析作业、待审候选到人工决定都能在一个明确的工作面中完成；但首期只是对现有 `learning_events` 的可信投影，不得借新页面宣称统一候选状态机、Evals 或 activation 已经存在。

## 3. 目标用户与价值

- 本地开发者：知道 Agent 为什么失败、同类问题是否重复，以及哪些改进值得批准。
- CodeFactory 维护者：从真实使用中得到带证据的 Harness、Skill、工具策略与回归候选。
- 领域审核人：只处理排序后的候选，不阅读未脱敏原始会话。

北极星仍是任务成功率；过程指标为工具失败率、重复失败率、人工纠正率、候选采纳率和回归通过率。指标必须来自真实执行记录，不能以 UI 出现或非空数组替代。

## 4. 产品边界

### 纳入

- project、Quick Task、任务调度三类持久 session 的结构化轨迹。
- anonymous session 零持久化。
- 工具生命周期、权限结果、错误、耗时、任务验证和终态。
- 入库前脱敏、截断、审计与兼容升级。
- 候选先审核，再进入知识、Skill、工具策略或 Evals。

### 暂不纳入

- ClickHouse、Temporal、Kafka、pgvector 或独立 OTel Collector。
- 保存 token delta 或模型 reasoning 内容。
- 自动合并 PR、自动部署、自动发布或直接修改生产配置。
- 未经用户单独同意，把原始会话交给第二个远程模型分析。
- 远程会话复盘默认关闭；只有用户在设置中显式 opt-in 后，系统才可把有限、脱敏的复盘摘要发给当前配置模型。本地确定性跨会话分析、人工审核和作业日志不依赖该开关。

## 5. 进化审查产品面

### 一级入口与范围

- Home 的一级能力区新增「进化审查」，显示待审数量，不能只放在顶部小图标或 Profile 深处。
- Workspace 的待审提示可直接深链到当前项目 scope；进入后必须保留 canonical project cwd，不能默认切到最近项目。
- Profile 继续负责个人偏好和项目记忆；学习日志、自我改进和工具门控逐步迁入「进化审查」。迁移期只保留一个可写审核面，避免两处状态和动作漂移。
- scope 明确区分 `project`、`quick` 与 `global`。项目候选只能作用于对应项目；Quick Task 在有稳定聚合 scope 前不得按每个临时 scratch cwd 假装跨会话；全局信号不得无目标写入某个项目记忆。

### 审核与作业

- 默认页是「待我审核」主从布局：左侧候选队列，右侧展示结论、目标去向、脱敏证据、来源 session、影响范围和精确人工动作。
- 采纳按钮必须描述真实结果，例如「采纳并写入项目记忆」「采纳并更新偏好」「启用工具门控」；点击前不得产生副作用。
- 拒绝不落地任何改进；拒绝原因与变更请求要等 versioned candidate/review 模型后进入长期审计，不能只保存在前端状态。
- 「作业与日志」同页展示持久分析作业：范围锁定、轨迹读取、隐私处理、信号提取、聚合去重、候选生成、等待人工审核。展示结构化计数与脱敏诊断，不直接暴露 raw log 或 reasoning。
- 薄工作台继续复用 `learning_events` 的 pending/accepted/rejected 与现有 miner；本轮同时加入最小持久 `evolution_jobs`/节点日志、人工决定幂等和重启中断明确终态。它仍是本机同步 ledger，不是分布式队列或通用工作流引擎。

### 真实边界

- 本轮只到「真实信号 -> 待审候选 -> 人工采纳/拒绝 -> memory/preference 有限物化 receipt」；通用 Evals、类型化 materializer/rollback、activation 和产品代码变更尚未接入。
- 页面不得显示假的「评估通过」「已激活」或从进度条推断生效。当前 memory/preference/tool gate 的明确动作仍按现有人工门禁执行，其他候选只展示建议。
- `Request changes -> revision N+1` 依赖 `improvement_candidates`、`candidate_reviews` 与 `expected_revision`，仍属于统一 Review 的后续阶段；不得用修改原 `learning_events` 文本冒充版本化变更请求。

## 6. 分阶段交付

1. **Phase 0 — Trace Truth + Review Shell**：真实写入规范化工具轨迹；普通聊天 post-mortem 有真实输入；Evidence 读取正确表；匿名和脱敏边界可证；新增一级「进化审查」入口、project scope 深链和现有候选的主从审核面。
2. **Phase 1 — Signal Extraction + Persistent Jobs**：本轮先落地最小本机 job/event ledger、幂等人工决定和重启中断明确终态；后续再补 verification/failure/correction/knowledge-gap/success-pattern 分类、分析窗口、partial/dropped 与失败节点重试。
3. **Phase 2 — Review Workbench**：统一候选、证据、置信度、优先级、revision 和人工裁决。
4. **Phase 3 — Controlled Materialization**：知识、disabled Skill、工具门控和 eval case 类型化落地，保存 receipt 与回滚信息。
5. **Phase 4 — Evals Gate**：基线/变更后评估、回归门禁、activation 分离。
6. **Phase 5 — Draft PR**：仅在显式授权、分支保护和 CI 完整时生成 Harness/产品代码草案；永不自动合并或发版。

## 7. 成功与停止条件

Phase 0 完成必须同时满足：真实 CodeFactoryDev 成功、失败/拒绝两条路径；SQLite 字段级断言；重启后可回溯；匿名计数不变；除用户主动保留的聊天输入外，系统派生轨迹和 Evidence 不复制敏感值；现有 miner 能从新轨迹读到非伪造信号。

Review Shell 完成还必须满足：Home 一级入口可见；Workspace 能深链到正确 project scope；待审主从布局中的数字与 SQLite/`evidence_json` 一致；采纳前无副作用、采纳后只改变明确目标、拒绝无副作用；重启后待审与决定历史可追溯；1366×768 与窄窗口主动作可达。本轮 persistent job slice 必须证明持久 job/event ledger、人工决定幂等、同一项目不并发启动两个分析、进程重启中断有明确失败终态，且其他仍存活的 CodeFactory 进程不会被误杀。分析窗口幂等、partial/dropped 与失败节点续跑属于后续能力，不纳入本轮完成声明。

Release-facing 完成必须再经过 PR+CI、刻意发版、安装包启动和真实用户路径验证。PR、CI 或 Dev app 通过仍是 `not live`，不能替代发布版本证据。

任何“工具卡可见但规范化轨迹为 0”、真实执行与状态/耗时不一致、匿名会话产生记录或敏感值泄露，均直接拒绝验收。
