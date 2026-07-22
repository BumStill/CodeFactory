# Token 用量、消耗地图与预算

## 1. Requirements Traceability

| Req ID | 来源 | 需求 | 影响 surface | 验证方式 | 责任角色 |
| --- | --- | --- | --- | --- | --- |
| CF-USAGE-R1 | 用户需求/数据真相 | 每次真实 Provider 请求 attempt 都记录 Usage，包含工具中间轮和最终轮 | agent loop + provider adapter + SQLite | failure-first multi-round tests + DB rows | backend + QA |
| CF-USAGE-R2 | 架构约束 | `attempt_id` 幂等；事件重送/恢复不重复，真实网络重试分别计量 | recorder + SQLite | duplicate/retry concurrency tests | backend + QA |
| CF-USAGE-R3 | 用户需求 | 总 Token、请求数、input/output，并在 Provider 可用时展示 reasoning/cache | normalized usage + queries + UI | provider fixture field assertions | backend + frontend |
| CF-USAGE-R4 | 成本真实性 | actual、estimate、subscription、local、unknown 成本语义分离 | provider adapter + aggregation + UI | cost-source matrix + negative UI assertions | product + backend + QA |
| CF-USAGE-R5 | 当地日语义 | “今日”和地图按本机当地自然日查询，存储仍为 UTC | query boundary + UI | UTC offset/date rollover tests | backend + QA |
| CF-USAGE-R6 | 用户需求 | 新会话显性展示今日用量、请求、成本语义、预算/7 日均值 | WelcomeScreen | component + real app | frontend + QA |
| CF-USAGE-R7 | 用户需求 | 新增设置一级「用量与预算」，提供 today/7d/30d 摘要和 90d/半年/一年地图 | Settings | component + real app | frontend + QA |
| CF-USAGE-R8 | 用户补充 | 设置展示 GitHub 风格近 90 天/半年/一年 Token 消耗地图 | usage map | unit + headless + real app | frontend + QA |
| CF-USAGE-R9 | 可解释性 | 地图支持 Tokens/预算占比/请求次数，区分零、缺失、今天和超预算 | usage map + queries | scale/state/accessibility tests | frontend + QA |
| CF-USAGE-R10 | 用户路径 | 选择日期后入口拆分与 Top 会话同步过滤，并可分别深链会话或真实端到端作业日志 | Settings + routing | integration + real deep-link path | frontend + backend + QA |
| CF-USAGE-R11 | 用户需求 | 新会话显示 28 天地图缩略图，进入完整地图不丢选中范围 | Welcome + Settings routing | route-state test + real app | frontend + QA |
| CF-USAGE-R12 | 预算治理 | 支持日/月 Token 预算和 50/80/100% 本机提醒，不自动停止或换模型 | settings + alert receipt | threshold/idempotency tests | backend + frontend + QA |
| CF-USAGE-R13 | 现有界面 | Workspace 底栏继续区分累计 Token 与上下文窗口，并可深链详情 | ContextUsageBar | component + stream real app | frontend + QA |
| CF-USAGE-R14 | 单一入口 | Profile 移除旧成本透视；新会话、Workspace 底栏与设置首个一级 tab 均进入同一用量真相面，无第二套统计/预算写入口 | Profile + Settings | navigation/negative action assertion | frontend + QA |
| CF-USAGE-R15 | 兼容迁移 | additive 新表；历史 message 可幂等回填；旧 cost_entries 不重复求和并保留回退期 | SQLite migration | old DB fixture + rerun/rollback | backend + QA |
| CF-USAGE-R16 | Provider 兼容 | OpenAI-compatible、Anthropic、ChatGPT 订阅、本地和 Usage 缺失均有明确行为 | provider adapters | route matrix + payload assertions | backend + QA |
| CF-USAGE-R17 | 实时性/可观测 | 落库后 2 秒内刷新；失败/partial/unavailable 不静默显示成 0 | event + queries + UI | latency + failure injection | backend + frontend + QA |
| CF-USAGE-R18 | Anonymous 边界 | anonymous 明示为临时用量，不写 DB、今日、预算、地图或 Top 会话 | agent + UI | before/after DB counts + restart | backend + QA |
| CF-USAGE-R19 | 可访问/视口 | 地图可键盘操作，390×812 不造成整页水平溢出且有列表替代 | usage map | axe/keyboard + dual viewport screenshots | frontend + QA |
| CF-USAGE-R20 | 隐私 | 计量/深链不保存或展示 prompt、reasoning、工具参数、凭据和原始 payload | recorder + API + UI | secret fixture zero-match | backend + QA |
| CF-USAGE-R21 | 质量护栏 | API 返回来源计数与缺失计数，拆分总和可与总量对账 | observation | source counts + reconciliation test | backend + QA |
| CF-USAGE-R22 | 发布门禁 | PR+CI、迁移、真实 App、锁屏 headless、安装包和公开产物均通过前保持 not live | delivery | evidence pack + exact artifact smoke | QA + release |

## 2. Primary User Path

唯一主路径为 `CF-USAGE-P1`：

用户打开 CodeFactory 创建持久新会话，在输入任务前看到今日用量与 28 天缩略图；点击“查看详情”进入设置「用量与预算」，在一年消耗地图选择一个高消耗日期；同页查看执行入口拆分与 Top 会话，打开对应会话或作业日志，判断消耗来源并按需设置 Token 预算。随后返回 Workspace 发起包含多个工具轮次的真实任务，今日、会话和地图统计在每次 Provider Usage 落库后刷新。

先验证该路径，再验证 anonymous、缺失 Usage、成本未知、迁移、时区和窄窗口边界。

## 3. Applicable Harnesses

- **Spec Harness**：R1-R22、主路径、指标合同、状态与完成边界。
- **Compatibility Harness**：旧数据库、`cost_entries`、历史 messages、Provider Usage 差异、旧事件名和回退旧版本。
- **Observation Harness**：逐请求 route、采集覆盖、幂等、刷新延迟、partial/unavailable 和汇总对账。
- **Payload Harness**：Provider usage/reasoning/cache/cost 字段解析、SSE 最终 usage、缺失/非法字段和脱敏。
- **Viewport Harness**：Welcome 卡、设置页、年度地图、tooltip/内联详情、底栏及 1366×768/390×812。
- **Release Harness**：迁移后的精确安装包、版本/build metadata、启动、重启、回滚和公开产物。
- **AI Collaboration Harness**：规划/开发/QA/发布角色按 Req ID 交接，记录 context scope、assumptions、review point、validation result。

## 4. 数据与显示合同

### 4.1 Token

- `total_tokens = input_tokens + output_tokens`。
- `reasoning_tokens`、`cached_tokens` 是子维度，不重复加入 total。
- 请求数按 `attempt_id` 计；真实 Provider retry 各算一次，UI/recovery 重放不重复。
- Provider 无 Usage 时不插入 0 值记录，聚合返回 partial/unavailable 状态。

### 4.2 成本

- actual 只来自 Provider；estimate 必须标注；subscription/local/unknown 不折入 actual。
- 旧 `cost_entries.cost_usd` 迁移后只能是 legacy estimate。
- 同一范围包含多种 cost source 时分项展示，禁止用单一“花费”数字掩盖语义。

### 4.3 日期

- SQLite 存 RFC3339 UTC。
- 当地日查询使用 UTC 半开区间 `[local 00:00, next local 00:00)`。
- 聚合响应回传 `start_utc/end_utc`；测试覆盖跨 UTC 日界和支持平台的 DST。

### 4.4 地图

- 每格一个当地日；支持 90d/半年/一年，默认一年。
- Tokens 为相对对数分档；预算占比为固定档；请求次数为分位档。
- zero、missing、today、over-budget 四种状态必须兼具颜色之外的标识。
- Hover/focus 展示精确数据；click/Enter/Space 选中并同步下方过滤。

## 5. 兼容与迁移合同

- 新建 `model_usage_events`，首个发布不删除/重命名 `cost_entries`。
- 历史 assistant Usage 用确定性 message attempt id 幂等回填；已覆盖的 legacy cost entry 不再插入。
- 回填不能产生 reasoning/cache/actual cost；UI 标记历史回填边界。
- 新 UI 只读新表，绝不新旧双表相加。
- 迁移可中断重跑并保存 scanned/inserted/skipped/conflicted 计数。
- 回退旧版本后旧表仍可用，新表被忽略；发布稳定后另开删除旧写路径的任务。

## 6. 测试矩阵

所有行为修改先写独立失败测试或可执行验收，并先看到失败。

| 类别 | 场景 | 必须断言 | 证据 |
| --- | --- | --- | --- |
| Normal | 一次无工具模型回复 | 一条 usage event；input/output/request 精确 | provider fixture + DB |
| Multi-round | 两轮 tool_calls + 最终轮 | 三条 attempt；汇总为三轮之和，不只最终轮 | AgentLoop integration + DB |
| Retry | 第一次 Provider 失败后第二次成功且两次有 usage | 两个真实 attempt；事件重送不产生第三条 | network fixture + unique index |
| Completion recovery | final candidate 被 gate 拒绝后重试 | 两个真实 Provider round 均计量，恢复重放不重复 | gate test + DB |
| Provider | OpenAI/OpenRouter reasoning+cache+cost | normalized 字段精确；子维度不重复加总 | payload fixture |
| Provider | Anthropic input/output/cache | 同一 recorder/聚合合同 | payload fixture |
| Subscription | ChatGPT OAuth | Token 可见；费用为“订阅”，无 fake USD | route + UI negative assertion |
| Missing | Provider 不返回 usage | 会话完成；统计 partial；不插 0 假记录 | failure fixture + UI |
| Invalid | negative/NaN/不一致字段 | 事件拒绝或标诊断；不污染汇总 | validator test |
| Anonymous | anonymous 多轮 + restart | 内存态可见；五类持久查询计数不变 | DB before/after |
| Timezone | UTC 前一日但本地已次日 | 进入本地今天；半开区间精确 | Rust query tests |
| Migration | 旧 DB 含 messages + cost_entries 重叠 | 一轮只计一次；cost 为 estimate；重跑不变 | migration fixture + counts |
| Rollback | 新 schema DB 用旧版本打开 | 旧表未破坏；旧版本可启动 | compatibility smoke |
| Map | 365 天含 zero/missing/outlier/over-budget | 分档、图例和选中日期正确 | pure scale tests + component |
| Drill-down | 选择日期 | 地图、入口 breakdown、Top 会话范围一致 | component/integration |
| Log route | Top autonomous/eval/interactive 行 | 打开对应真实 session/task/eval log；无日志不显示入口 | route integration + real app |
| Budget | 49→51→81→101%，重启、次日 | 各阈值 exact-once；跨阈值合并；次日新周期 | receipt DB + notification fake |
| Live update | 新 usage 成功提交 | Welcome/Settings/footer ≤2s 更新 | event integration + timestamp |
| Error | SQLite/query failure | 显示 unavailable/last updated，不显示 0 | failure injection + screenshot |
| Privacy | usage fixture 含 secret-like request metadata | DB/API/UI 0 命中 prompt/secret/reasoning 内容 | scan evidence |
| Viewport | 1366×768 | 地图、筛选、Top 会话和预算动作可达 | headless + real screenshot |
| Viewport | 390×812 | 无整页横向溢出；地图区滚动；列表替代；键盘路径通过 | headless screenshot + receipt |
| Restart | 记录后完整重启 | 今日/地图/预算 receipt 恢复，数值不重复 | real Dev App |
| Release | exact Windows/macOS artifact | migration、multi-round、map、restart smoke，版本/commit 精确 | CI artifact receipts |

## 7. Evidence Pack

完成声明至少包含：

- Requirements Traceability：R1-R22 对应 commit/test/evidence。
- Provider route：OpenAI-compatible、Anthropic、ChatGPT 订阅、Usage 缺失的输入字段摘要与 normalized 断言。
- SQLite：迁移前后 schema、行数、`attempt_id` 唯一、三轮工具任务逐行和聚合对账。
- Observation：带 Usage response、inserted、duplicate、missing、invalid 计数及覆盖率；拆分总和对账。
- UI：新会话、Settings 地图、选中日期、Top 会话、日志深链、partial/unavailable、预算阈值。
- Viewport：1366×768 和 390×812 截图/JSON receipt，含键盘地图与无整页溢出。
- Privacy：DB/API/UI 对测试 secret、prompt、reasoning、原始 payload 的零命中。
- Lock-safe：锁屏时 headless 双视口可执行，不要求用户解锁、不声称 headless 证明 Tauri 壳。
- Release：PR checks、build metadata、Windows installer、macOS DMG、公开下载 SHA/签名/版本和 exact executable smoke。

## 8. AI Collaboration 记录

- context scope：现有 `cost_entries`、AgentLoop 两条 Provider 路径、Workspace 底栏、Welcome、Profile/Settings、session/task/eval 日志与 SQLite 迁移。
- assumptions：首版本机单用户；预算单位为 Token；Provider Usage 是可用时的原始计量；不引入云端账单服务。
- review point：数据真相必须先于 UI；actual/estimate/subscription 不混合；地图必须可下钻且不以颜色作为唯一语义。
- validation result：开发、QA 与发布角色分别填入失败测试、实现、真实路径和发布产物证据；没有证据时保持 `not live`。

## 9. 完成边界

单元测试、页面可见、地图有颜色、SQLite 非空、PR 合并、CI 绿色或 Dev App 通过均不能单独证明完成。只有本规格 R1-R22、兼容迁移、双视口、日志深链、预算、PR+CI、刻意发版及精确发布产物全部有证据后才能标记 `live`。模型、Endpoint、项目筛选与自定义日期已明确列为后续分析增强，不得反向冒充本次已交付能力。
