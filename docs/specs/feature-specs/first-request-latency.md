# M4：首请求关键路径延迟

## 原始任务（逐字保留）

# 任务：准入后到首次模型请求的 3.5 秒延迟（M4）

按 `/Users/leo/.codefactory/agent-tools/task-template.md` 执行（harness + SDD + TDD + 自验收 + §8 收尾；交付 ceiling: merged）。证据见 `/Users/leo/.codefactory/agent-tools/ux-observations.md` 的 M4（09-29 16:08 时间线：准入后约 3.5 秒才发出第一次模型请求，每一轮都会这样）。

## 要做的
1. 先测量：在合成会话里给「消息准入 → 回合开始 → 上下文组装 → 路由/额度/时段判定 → 首个 HTTP 请求发出」每一段打时间戳（进 trajectory 或调试日志），用一次合成回合把这 3.5 秒拆开，找到大头。注意：CF-QUOTA 的实时额度查询（#661）、CF-WINDOW 的时段判定、技能/记忆加载都可能是新来源。
2. 只修大头：把可以并行或提前做的步骤挪出关键路径（例如额度查询改为后台刷新、读缓存不阻塞首请求），不改变任何门禁语义（额度上限、时段规则、权限判定必须仍在首请求前生效）。
3. 写一个回归测试固定首请求前的关键路径（例如断言首请求前没有同步的网络调用），并在 PR 里给出修复前后的分段耗时对比。

## 约束
不改 `.github/` 工作流、场景注册表与校验器；只用合成数据；其余同模板。

## 标题
`perf(chat): the first model request of a turn no longer waits ~3.5s behind work that can run off the critical path`

## M4B 原始任务（逐字保留）

# 任务：把首请求前的 3.5 秒完整测清楚，并修掉剩下最大的一段（M4 第二段）

按 `/Users/leo/.codefactory/agent-tools/task-template.md` 执行（harness + SDD + TDD + 自验收 + §8 收尾；交付 ceiling: merged）。
上一段 #672 只修了额度刷新锁这一段等待（102ms → 0），完整的分段测量还没有做，PR 里已经如实说明。规格见 `docs/specs/feature-specs/first-request-latency.md`。

## 要做的
1. 写一个可重复的测量入口，例如 `scripts/measure-first-request-latency.mjs`，或者 Rust 侧的集成测试。用合成会话加本地假模型端点（不要访问外网），跑完整的「消息准入 → 首个 HTTP 请求发出」路径，输出每一段的耗时：admission、workspace_checkpoint、route_quota、quota_refresh_lock、context_assembly、skills/memory 加载、provider_post 之前。连续跑 5 轮，给出中位数。
2. 根据测量结果，修掉剩下最大的一段。前提是不改变门禁语义：额度、时段、权限仍然必须在首请求之前生效。
3. 写回归测试，锁住首请求前的关键路径，例如关键路径上不得有同步网络调用，或者某个阈值以内的测量断言（阈值要宽松，避免 CI 抖动）。
4. PR 里附上修复前后的分段耗时表。

## 约束
不改 `.github/` 工作流、场景注册表与校验器；只用合成数据；其余同模板。

## 标题
`perf(chat): measure every step before the first model request and remove the largest remaining wait`

### M4B Implementation Notes

追踪项 M4B-1/2/3/4 对应以上四项；沿用本文 Harness 与真实应用验收分工。完整路径测量必须使用生产准入与 AgentLoop，不能以手写 sleep 或局部耗时推断历史 3.5 秒。测量原始样本、阶段覆盖、首个本地 HTTP 请求及门禁断言均需机器检查。模板明确禁止 delegate_tasks，按单执行流执行。

## Implementation Notes

原任务未提供 Req ID、指定规格路径或完整测试矩阵；本文件保留原文，不自行扩大验收语义。追踪标识 M4-1/2/3 分别对应上述三项。

- Spec Harness：保留原任务与证据映射。
- Compatibility Harness：CF-QUOTA-R7/R9 的过期刷新、上限、重置保护保持不变；不修改时段与权限语义。
- Observation Harness：仅记录阶段名和耗时，不记录消息、凭据或生产数据。
- AI Collaboration Harness：范围限首请求前路径；假设必须由合成测试验证；单执行流（任务模板禁止 delegate_tasks）；复核点为缓存新鲜度、锁竞争、强制刷新和过期门禁。
- Release Harness：本任务仅合并，不发布；安装版真实验收由编排方发版后执行。

### 测试矩阵

| 追踪项 | 检查 |
| --- | --- |
| M4-1 | 合成输入的阶段计时；区分等待刷新锁与刷新请求 |
| M4-2 | 有效实时读数在刷新锁被占用时仍立即返回；过期/重置/缺失读数不绕过刷新 |
| M4-3 | 先红后绿的锁竞争回归与已有额度/时段测试 |

### 真实应用验收分工

真机验收由编排方发版后完成：使用合成会话发送两轮消息，核对阶段计时与首次 provider 请求；缓存有效且后台刷新进行时不应等待该查询；缓存过期、重置、额度已到上限时仍刷新/回落；时段与权限限制仍在请求前生效。本会话不操作生产消息或用户凭据。
