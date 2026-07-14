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

## 5. 分阶段交付

1. **Phase 0 — Trace Truth**：真实写入规范化工具轨迹；普通聊天 post-mortem 有真实输入；Evidence 读取正确表；匿名和脱敏边界可证。
2. **Phase 1 — Signal Extraction**：补 verification/failure/correction/knowledge-gap/success-pattern 分类、窗口与幂等 job。
3. **Phase 2 — Review Workbench**：统一候选、证据、置信度、优先级、revision 和人工裁决。
4. **Phase 3 — Controlled Materialization**：知识、disabled Skill、工具门控和 eval case 类型化落地，保存 receipt 与回滚信息。
5. **Phase 4 — Evals Gate**：基线/变更后评估、回归门禁、activation 分离。
6. **Phase 5 — Draft PR**：仅在显式授权、分支保护和 CI 完整时生成 Harness/产品代码草案；永不自动合并或发版。

## 6. 成功与停止条件

Phase 0 完成必须同时满足：真实 CodeFactoryDev 成功、失败/拒绝两条路径；SQLite 字段级断言；重启后可回溯；匿名计数不变；除用户主动保留的聊天输入外，系统派生轨迹和 Evidence 不复制敏感值；现有 miner 能从新轨迹读到非伪造信号。

任何“工具卡可见但规范化轨迹为 0”、真实执行与状态/耗时不一致、匿名会话产生记录或敏感值泄露，均直接拒绝验收。
