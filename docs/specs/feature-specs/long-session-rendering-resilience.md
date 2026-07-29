# 超长会话渲染韧性

## 业务设计

### 问题

CodeFactory 的核心价值是让用户在一个持续会话中完成真实软件交付。当前历史加载和消息渲染成本随会话长度无界增长：现场会话达到 3743 条持久化消息、1726 次工具调用和约 9.8 MB 文本后，后台已经完成并落库，macOS WebView 却占用约 2.25 GB RSS、CPU 峰值 48%，界面停在旧的 `21.5s` 状态，用户只能看到一个实际失活的窗口。

这不是“模型慢”，而是产品失去结果展示和控制能力。用户无法确认任务是否完成、无法读取已经生成的回复，也无法可靠停止或继续工作。

### 产品目标

- 超长会话默认只加载足以继续工作的最新历史，启动成本不再与全量会话线性绑定。
- 更早历史仍可按需访问，不迁移、不删除、不静默截断 SQLite 中的原始记录。
- 流式尾部更新不得反复解析和渲染不变的历史工具卡。
- 流式时间线中的文本段不得因后续工具或文本到达而丢失 Markdown 语义。
- 即使前端漏掉最后一个 stream 事件，回合终态也能从持久化尾页重新同步。
- 修复必须兼容旧数据库、旧 completion state、历史 tool replay 和当前会话控制语义。

### 非目标

- 本切片不删除、压缩或重写历史数据库。
- 本切片不改变模型上下文压缩策略和 provider replay 语义。
- 本切片不先引入第三方可变高度虚拟列表库；若有界分页、惰性解析和引用隔离仍不能满足门槛，再单独评估虚拟列表。
- 本切片不把图片白色预览问题混入同一根因；图片预览保持独立缺陷面。

## Requirements Traceability

| Req ID | 要求 | Surface | 验证 |
| --- | --- | --- | --- |
| CF-LSR-R1 | 历史会话首次打开不得执行无界 `SELECT *` 并把全部消息交给 WebView；单页同时受 400 行和完整序列化 `MessagePage` 2 MiB 桥接预算约束 | Rust session command | SQLite failure-first unit |
| CF-LSR-R2 | 首屏按最近 8 个真实用户回合加载；内部 gate/notice 行属于所在回合，不单独计数 | SQLite + hydration | Rust page contract |
| CF-LSR-R3 | 分页以稳定数据库游标为边界；相同时间戳和新消息追加不得造成重复或遗漏 | SQLite | cursor regression |
| CF-LSR-R4 | 前端保留 `has_more`/cursor，并提供顶部“加载更早记录”；原始历史仍留在数据库 | chat store + MessageList | store/component test |
| CF-LSR-R5 | 加载更早记录后保持当前阅读锚点，不自动跳到底部；流式期间禁用历史加载 | MessageList + sticky scroll | browser/real app |
| CF-LSR-R6 | 折叠工具卡不得解析完整 diff、知识结果或完整输出；只在展开时惰性解析并缓存 | ToolCallCard | failure-first component test |
| CF-LSR-R7 | stream 事件只替换目标 assistant 消息，未变化历史消息保持对象引用稳定 | chat reducer | 3743-message reducer test |
| CF-LSR-R8 | 历史 MessageRow/ToolCallCard 使用引用隔离，尾部 delta 不重新渲染全部历史 | React components | render-count regression |
| CF-LSR-R9 | 冷启动、重开或重新选择会话时从持久化尾页恢复最终回复；显式加载历史时用 selection generation、request owner 和 revision 门禁合并最新尾页，不得覆盖下一条 stream | chat store + Tauri command | store integration + real app |
| CF-LSR-R10 | 历史分页不得破坏 tool declaration/replay 归属、completion recovery 隐藏规则和 turn notice 来源 | hydration | compatibility matrix |
| CF-LSR-R11 | 原故障等比例 fixture 从点击会话到真实 App 可交互且最新回复可见不超过 5 秒；内部页查询与 hydration 目标不超过 2 秒；初始 DOM 消息/工具卡不超过 250 | desktop UI | component gate + real app evidence |
| CF-LSR-R12 | 原故障等比例 fixture 的 WebContent 峰值不超过 700 MB、稳定值不超过 500 MB、静置 CPU 不超过 10% | packaged/dev app | process observation |
| CF-LSR-R13 | 普通短会话、匿名会话、后台流式会话和队列语义不得回归 | desktop UI + store | existing full suite + focused tests |
| CF-LSR-R14 | PR、main CI、公开安装包和精确版本真实 App 验证完成前保持 `not live` | release | release evidence pack |
| CF-LSR-R15 | 文本段从当前尾部变为中间执行步骤时，标题、列表、行内代码、链接等 Markdown 语义必须保持；只允许改变视觉层级 | MessageList | failure-first component + real app |
| CF-LSR-R16 | 已完成文本段必须按稳定文本缓存；新的 stream delta 不得重新解析所有不变的历史 Markdown 段 | MessageList | render-count regression |

## 架构设计

### 1. 持久化分页契约

新增 `get_message_page(session_id, before_rowid?, user_turn_limit?) -> MessagePage`：

- `before_rowid = null` 表示从会话尾部读取。
- 默认 `user_turn_limit = 8`，服务端限制在安全范围内。
- “真实用户回合”定义为 `role='user' AND completion_state IS NULL`。
- 先定位本页最早真实用户行的 `rowid`，再读取该边界至 `before_rowid` 之间的所有行，保证工具 replay、gate 状态和最终回复在同一页内。
- 单个旧回合异常膨胀时，服务端再施加 400 个原始行和完整序列化 `MessagePage` 2 MiB 桥接预算；优先保留最新行与最终回复，并返回 `truncated=true`。该硬边界可能暂时切开 declaration/replay，加载相邻旧页后由同一 hydration 层按 ID 重新连接。
- `reasoning_content` 是 provider replay 状态而非聊天 UI 内容，不进入历史页；UI 内容单字段最多 128 KiB。原始 SQLite 记录保持不变。
- 返回 `next_before_rowid` 与 `has_more`；旧 `get_messages` 保留，避免破坏已有调用方。

### 2. 前端历史状态

每个 `SessionRuntime` 增加：

- `persistedMessages`：当前已加载的原始 DB 页，用于跨页重新 hydration。
- `historyBeforeRowid`：下一页游标。
- `hasOlderHistory` / `loadingOlderHistory`：UX 状态。
- `historyRequestId` / `revision`：保证迟到页只能清理自己的 loading 状态，且不能覆盖新 stream。
- `localMessages`：前端本地提示与 notice，在持久化历史重新 hydration 后仍保留。

首次选择会话只 hydrate 尾页。加载更早时按 ID 去重后把原始页前置，再一次性运行既有 `dbMessagesToUI`，因此 tool owner 和 completion recovery 规则仍由同一兼容层负责。

显式连续加载会按 400 行/完整序列化 DTO 2 MiB 的独立页推进，不会再次出现一次桥接全量历史；当前版本仍允许用户主动把多个页保留在阅读窗口。若真实使用证明长时间连续追溯仍造成 DOM 压力，再以双向窗口化替代，不能以删除原始历史换性能。

### 3. 增量渲染隔离

- reducer 使用定点更新 helper：查找目标消息、复制数组、只替换一个对象。
- `MessageRow` 与 `ToolCallCard` 使用 `React.memo`；未变消息引用保持稳定。
- 时间线文本统一经过 Markdown renderer；中间步骤只使用较轻的容器样式，不降级为纯文本。
- 单个时间线文本段由 memoized component 持有。reducer 保持既有段对象引用，只有当前接收 delta 的尾段需要重新解析 Markdown。
- `ToolCallCard` 的 diff/知识结果解析放入依赖 `open` 的 `useMemo`，折叠态不读取完整结果。
- 折叠失败卡的摘要用有界首行扫描，最多构造 200 个字符，不对多 MiB 结果执行 `split/map`。
- `MessageList` 使用稳定 session id 作为 conversation key，向上分页不会被误判为切换会话。

### 4. 持久化尾部重同步

冷启动、重开或重新选择会话时，直接从最新持久化尾页恢复最终回复。显式加载更早历史时同时刷新最新尾页，但只在当前 session 未再次开始 stream、请求 revision 仍匹配时应用，并按 message id 合并。

不能在每个 `done`/`error` 后无条件异步替换 runtime：迟到响应会覆盖排队后已开始的下一轮，还会丢失 live-only timeline、duration 和 review UI。WebView 已经卡死时，JS 终态 handler 本身也无法充当 watchdog；本切片靠有界加载和渲染消除卡死源，重启恢复作为持久化兜底。

### 5. 兼容与回滚

- 不新增 SQLite 列或迁移，使用现有 `rowid` 作为本地分页游标。
- 旧数据库、旧 `role=user` 内部状态和 tool replay 仍经过 `dbMessagesToUI`。
- 回滚到旧版本不会损坏数据；旧版本仍可读取全量历史。
- 发布失败可回滚到上一公开版本；本变更不涉及凭据、外部数据或 schema。

## UX 设计

### 首次打开

- 会话立即显示最新 8 个用户回合和最终回复。
- 如果存在更早历史，在顶部显示紧凑按钮：“加载更早记录”。
- 仅在服务端安全预算实际生效时显示低干扰说明：“部分超大历史内容仅显示预览或分段加载；完整原始记录仍保存在本机”，覆盖正文预览、tool declaration/replay 折叠与页分段，不得暗示数据库记录被删除。

### 加载更早

- 用户点击后按钮显示加载状态并暂时禁用。
- 新内容插入顶部，当前阅读锚点漂移不超过 4px。
- 若已到最早记录，入口消失。
- 流式生成期间入口禁用，避免历史快照覆盖活动尾部。

### 故障恢复

- 回合终态以数据库重同步为兜底，不让界面永久停留在旧计时和 typing dots。
- 重同步失败不弹阻断式错误；保留当前消息，并提供可诊断日志。
- 若未来 watchdog 检测到 WebView 长任务，应提供“重新加载当前会话尾部”，但本切片先消除已知无界成本。

### 流式时间线格式

- 当前文本段和已经完成的中间文本段使用相同 Markdown 语义。
- 中间步骤可使用较轻颜色和紧凑间距，但 `**标题**`、反引号、列表、链接不得显示为原始标记。
- 后续工具调用或新文本段到达时，已显示内容不得发生“从格式化文本闪回原始 Markdown”的视觉跳变。

## Primary User Paths

### 成功路径

用户启动 CodeFactory，打开一个含数千条历史消息的会话。真实 App 在 5 秒内恢复交互并显示最近工作和最终回复（内部页查询与 hydration 目标 2 秒）；发送新请求后工具调用、流式正文和终态正常更新，历史区域不重复重渲染。格式化状态报告在后续工具和文本到达后仍保持标题、列表和行内代码语义。

### 历史路径

用户向上滚动并点击“加载更早记录”。旧回合在顶部出现，阅读位置保持；重复加载可以继续追溯，SQLite 原始历史不变。

### 恢复路径

后台已写入最终回复但前端漏掉终态事件。持久化尾部重同步将最终回复恢复到 UI，停止计时和 typing 状态。

### 边界路径

- 旧会话没有 completion state 新字段值。
- 一页包含 tool declaration、tool replay、gate recovery 和最终回复。
- 一个用户回合包含大量工具调用。
- 会话在后台流式时切换前台。
- 匿名会话不调用持久化分页。

## Applicable Harnesses

- Spec Harness：CF-LSR-R1..R14。
- Compatibility Harness：旧 SQLite、tool replay、completion state、匿名/后台会话。
- Payload Harness：大工具输出、diff、Markdown、图片路径；折叠态不得全量解析。
- Viewport Harness：1366×768、800×700 的顶部历史入口、尾部流式区和输入区。
- Observation Harness：RSS、CPU、DOM 数量、首屏时间、最终回复同步时间。
- AI Collaboration Harness：独立架构、性能测试和 UX/QA 审查。
- Release Harness：PR/main CI、Windows/macOS 构建、公开产物和发布 App。

## 测试矩阵

| 层级 | 场景 | 断言 |
| --- | --- | --- |
| Rust unit | 90 用户回合、3743 行 | 首屏只返回最近 8 回合；游标稳定；无重复遗漏 |
| Store unit | 3743-message fixture | 首次调用分页接口；加载更早合并并去重 |
| Reducer unit | 3743 UI messages + text/tool/done | 仅目标对象引用变化；终态关闭 streaming |
| Component unit | 折叠 1726 工具卡 | diff parser 调用为 0；展开目标卡后只调用 1 次 |
| Component unit | Markdown 文本 → tool → 新文本 | 原文本从尾段变为中间步骤后，标题、列表、行内代码仍为结构化 DOM；不变段不重复渲染 |
| Browser/headless | 生产故障会话等比例 fixture 首屏与上翻 | 初始 8 回合 fixture DOM <=250；内部加载 <=2s；锚点漂移 <=4px |
| Dev App | 原 SQLite 的隔离副本 | 最新回复可见；流式成功和边界路径；RSS/CPU 达标 |
| Release App | 公开安装包 | 精确版本、真实 GUI、原故障等比例 fixture 达标 |

## 完成边界

不得在文档、单测、构建、PR、合并或 release workflow 启动后停止。只有公开版本的真实 App 在超长 fixture 上满足 CF-LSR-R11/R12，且最终回复恢复路径通过，才可标记 `live`。
