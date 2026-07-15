# Evolution Agent 架构设计

## 1. 设计选择

CodeFactory 首期保持 `Tauri + Rust + SQLite` 本地架构。演示方案中的 OTel `gen_ai.*` 作为字段语义参考，不引入独立遥测服务。结构化数据留在本机；远程二次分析后续单独 opt-in。

## 2. Phase 0 数据路径

```text
Model round
  -> assistant message + tool declarations
  -> normalized tool_calls INSERT(status=pending, redacted args)
  -> permission / hook / dispatch
  -> normalized tool_calls UPDATE(done|error|denied, redacted result/error, duration)
  -> session/task postmortem
  -> evolution_jobs analysis job
  -> learning_events pending + evolution_job_events
  -> human accept/reject job
  -> accept: review event -> materialization event -> learning_events accepted
```

`messages.tool_calls` 继续用于模型对话重放，但持久副本先脱敏且不做轨迹限长；规范化 `tool_calls` 是分析、Evidence 与审计真相源，使用限长摘要。两者职责不同，不能删除前者或继续让分析读取空表。

terminal outcome 必须在同一 SQLite 事务中更新规范化 row，并以稳定 message id upsert 对应的 `role=tool` replay message；任一步失败都整体回滚。同一个 provider call 的终态被修正时更新原 replay，不新增或保留旧结果。消息重放按 `created_at, rowid` 排序，避免 0ms 拒绝路径同毫秒写入时顺序不稳定。

replay JSON 同时保存 provider `tool_call_id`、脱敏内容和 `done|error|denied` 终态。前端历史加载不能把 `role=tool` 当普通聊天消息丢弃：必须先解析 assistant 的 tool declaration，重建工具卡，再把 replay 按 `tool_call_id` 折叠回所属 assistant message。旧 replay 没有 status 时只为兼容默认 `done`，新数据不得丢失 error/denied。

## 3. Phase 0 合同

### `tool_calls`

沿用现有表：

- `id`：`session_id + provider_tool_call_id` 的稳定复合标识，避免跨 session 冲突。
- `message_id`：声明该调用的 assistant message。
- `tool_name`、`arguments`：入库前按敏感 key 和常见 token 模式脱敏并限长。
- `status`：`pending | done | error | denied | legacy`。
- `result`：成功输出的脱敏限长摘要。
- `error`：失败/拒绝的脱敏限长摘要。
- `duration_ms`：真实执行耗时；未执行的拒绝路径为 `0`。
- `created_at`：工具声明时间。

分析只把 `done` 与 `error` 计入工具可靠性；`denied` 不是工具故障，`pending/legacy` 不进入失败率分母。

### 兼容

- `ensure_schema` 必须创建缺失的 `tool_calls` 表和索引，兼容旧数据库。
- 历史 message JSON 不推断不存在的失败状态或耗时；如后续回填，状态只能标记 `legacy`。
- 新路径保持脱敏后的 `messages.tool_calls`，旧会话仍能重放。
- `learning_events` 继续作为候选真相源，并保留现有 `pending | accepted | rejected` 语义：`pending` 是待人工决定，`accepted` 是已审核且已完成本地物化，`rejected` 是已拒绝。不得把仅通过审核但尚未物化或物化失败的候选标为 `accepted`。

## 4. 脱敏合同

远程 post-mortem 还必须经过独立设置 `remote_postmortem_enabled=true`。旧配置和新安装默认 `false`；关闭时 chat/task 终态不得自动调用远程复盘，但本地 `mine_cross_session_patterns`、Review 工作台和审计读取继续可用。后端命令本身也要校验该开关，不能只依赖前端隐藏入口。

入库、Evidence 和后续候选证据复用同一套 redactor：

- 递归屏蔽 `api_key`、`token`、`password`、`secret`、`authorization`、`cookie`、`credential` 等 key。
- 屏蔽 Bearer、OpenAI/GitHub 常见 token、私钥块和 URL userinfo。
- 完整 JSON 派生消息先用 `serde_json` 解析，再按敏感 key 递归替换并重新序列化；只有无法解析的普通文本才使用正则 fallback，避免转义字符串残留或数字/布尔值破坏 JSON。
- normalized arguments、result、error 分别限长；provider replay message 只脱敏、不截断，避免破坏长参数和上下文。
- 用户主动输入继续遵循现有聊天历史保留语义；系统派生的 assistant、reasoning、tool result、tool declaration、normalized trace 与 Evidence 不复制命中的敏感值。
- `reasoning_content` 不进入 Evolution 分析或 Evidence；现有 provider 重放副本仅做脱敏持久化。
- anonymous AgentLoop 在任何 recorder 调用之前返回，保持零 DB 写入。

## 5. 普通聊天 post-mortem

`run_postmortem` 优先使用 task outcome；没有 `task_runs` 时，使用同 session 的有限用户/助手轮次摘要和规范化工具状态统计。摘要必须先脱敏和限长，不复制完整对话或工具结果。

模型生成的 post-mortem candidate 在去重和持久化前再次脱敏与限长；`kind=preference` 的 key 仅允许单行、64 字符以内的 snake_case（`^[a-z][a-z0-9_]{0,63}$`），否则降级为 memory，避免非结构化 key 进入后续系统提示词。

空 session 返回空；有足够持久消息的 project/Quick session 不得仅因没有 task_run 而返回空输入。LLM 调用仍为 best-effort，不能阻塞正常聊天。

推理模型若首轮只有 `reasoning_content`、没有最终 `content`，不得把 reasoning 当候选；系统只记录 finish reason/是否存在 reasoning 等元数据，并将当前请求实际使用的 `max_tokens` 或 `max_completion_tokens` 扩到 2000 后有界重试一次。重试仍无最终内容时返回空候选，不循环、不把隐式推理写入学习日志。

## 6. Evidence

Evidence Pack 优先读取真实 `tool_calls` 表，输出 tool name、脱敏 args/result/error、status、duration、message/session 关联。旧消息 JSON 只作为兼容 fallback，并同样脱敏。不存在的 `tool_call_records` 查询必须删除。

## 7. Review 与分析作业最小合同

本轮不拆出新的候选、Review、Receipt 或通用 Eval 平台。`learning_events` 继续承载候选内容与最终决定；只增加 `evolution_jobs` 和 `evolution_job_events`，为真实分析、审核与物化提供可观察且可追责的执行记录。

### `learning_events` 关联

现有字段与状态保持不变，只增量增加可空的 `job_id`，指向首次产生该候选的分析 job。旧候选保持 `job_id=NULL`，不得伪造历史 job 或事件。

- 一个分析 job 可以产生零个或多个 `learning_events`。
- 新分析候选只有一个来源 `job_id`；审核/物化作业通过各自的 `candidate_id` 关联，不改写来源 job。
- `cwd` 同时保留在 job、candidate 和 event 上。命令处理时必须校验三者一致，禁止通过猜测、空字符串 fallback 或跨项目 id 复用建立关联。
- 跨 session 聚合候选允许 `session_id=''`；来源范围与聚合统计写入 job/event，不再把空 `session_id` 解释为缺失的单一会话。

### `evolution_jobs`

轻量 job 只描述本机一次同步执行，不承担队列平台、分布式调度或长期工作流引擎职责。实际落库字段为：

- `id TEXT PRIMARY KEY`
- `cwd TEXT NOT NULL`
- `trigger TEXT NOT NULL`：`cross_session | review_accept | review_reject`
- `candidate_id TEXT NULL`：Review 作业必填，分析作业为空
- `status TEXT NOT NULL`：本轮真实写入 `running | succeeded | no_candidates | failed`；启动恢复同时兼容旧的 `queued`
- `idempotency_key TEXT NULL`：Review 作业按 `trigger + cwd + candidate_id` 唯一；跨会话分析每次显式运行创建新 job，靠候选内容去重避免重复候选
- `input_session_count`、`input_trace_count`、`candidate_count`：结构化聚合计数
- `started_at`、`completed_at`、`error`：错误先脱敏并限长
- `owner_pid INTEGER NULL` + `owner_start_token TEXT NULL`：当前本机执行进程及其启动标识；二者共同区分“已死亡/已被 PID 重用的旧 owner”和“另一个仍存活的 CodeFactory 进程”，旧 job 为空

索引为 `(cwd, started_at DESC)`、`(candidate_id, started_at DESC)` 和非空 `idempotency_key` 唯一索引。另有两个运行中唯一索引：同一 candidate 只能有一个 accept/reject owner；同一 `cwd` 只能有一个 `cross_session` 分析处于 `running`。后者只防并发重复运行，不代表分析窗口幂等。job row 保存可查询终态；每个阶段同时追加 event，不能只覆盖 job 后丢失执行过程。

### `evolution_job_events`

事件表是 append-only 的阶段、审核和物化日志。实际落库字段为：

- `id TEXT PRIMARY KEY`
- `job_id TEXT NOT NULL`
- `candidate_id TEXT NULL`
- `cwd TEXT NOT NULL`
- `stage TEXT NOT NULL`：`job | scope | trace_read | privacy | extract | deduplicate | review | materialize`
- `status TEXT NOT NULL`：`started | completed | waiting | failed`
- `title TEXT NOT NULL`
- `detail_json TEXT NOT NULL`：带 `schema_version` 的脱敏聚合元数据或有限 receipt
- `created_at TEXT NOT NULL`

事件只能插入，不能更新或删除。`detail_json` 只允许阶段计数、稳定对象 id、错误摘要、目标类型和无敏感值 receipt；禁止写入原始 user message、`reasoning_content`、完整工具 arguments/result、token、cookie、凭据或完整 memory/preference 内容。前端再做一次字段白名单和长度限制，不能把后端新增字段直接透传给用户。

分析 job 的最低事件序列为：

```text
job.started
scope.completed
trace_read.completed|failed
privacy.completed
extract.completed
deduplicate.completed
review.waiting|completed
job.completed|failed
```

失败和候选为零都是合法且不同的结果：零候选用 `no_candidates`，收集/提取失败用 `failed`，都保留已完成节点与真实计数。候选 INSERT、去重/待审事件、job 终态和 `job.completed` 必须在同一 SQLite 事务提交；最终 ledger 任一步失败时全部候选回滚，不能留下属于 failed job 的可采纳 pending candidate。失败路径的脱敏 failure event 与 job=`failed` 同样必须原子提交；任一写入被拒绝时两者一起回滚，不能形成“失败节点 + running job”或“failed job + 无失败节点”的半终态。

### 采纳是 Review 与 Materialization 的单一受控动作

现有用户动作仍是“采纳”，不在本轮引入可长期停留的 `approved` 中间状态。一次采纳命令创建 `trigger=review_accept` 的 job，并按阶段追加：

```text
review.started
review.completed
materialize.started
materialize.completed|failed
job.completed|failed
```

`review.completed` 只是本次命令进入物化阶段的审计事实，不代表产品已生效。只有 memory、preference 等目标写入成功并完成必要的持久化校验后，才能把 `learning_events.status` 从 `pending` 更新为 `accepted`。

物化失败时必须追加 `materialize.failed` 并把 job 置为 `failed`，保留候选为 `pending`，返回显式错误；不得追加成功 receipt，不得仅因 Review 阶段通过而展示“已生效”。拒绝命令使用 `trigger=review_reject`，只写 `review.started/completed`，并在同一受控事务中把候选更新为 `rejected`，不进入 `materialize` 阶段。

采纳必须具备候选级幂等性：memory 使用稳定 candidate marker 防止重试重复追加；preference 使用同一 `(cwd,key)` upsert，event receipt 只保存 `target` 与持久化结果，不保存敏感原值。若外部文件已经写入但 SQLite 终态提交失败，下次重试必须先通过 marker/upsert 识别既有物化结果，再补齐状态与事件，不能再次追加内容。

审核命令必须以 `WHERE id=? AND cwd=? AND status='pending'` 做条件更新并检查影响行数；并发窗口中只有一个动作可以进入最终决定。无匹配行时返回当前候选状态，不能静默成功。

### 查询与 UI 状态

- 当前分析命令保持兼容，返回新候选数组；前端随后刷新 `list_evolution_jobs(cwd)` 和 `list_evolution_job_events(cwd, job_id)`，并在 job 为 `running` 时轮询。刷新或重开窗口后从 SQLite 恢复真实进度。
- 来源 job 通过 `(cwd, job_id)` 精确查询；不在最近列表时必须单独补取，找不到则显示“来源作业不可用”，不能回退到最新 job。单 job 事件查询最多保留最近 500 条并按正序显示，以保证终态不因截断丢失；UI 达到上限时必须显性提示。
- Review 详情按 candidate 类型读取真实当前值：preference 查询 `(cwd,key)`，memory 只检查候选建议是否已存在，不向 Review 面复制整份 memory。读取失败时禁止采纳，拒绝仍可执行。切换 `cwd` 时先清空旧项目 job/event/current-value，再启动新请求；所有请求用 generation id 丢弃迟到结果。
- 候选列表继续读取 `learning_events`；详情通过 `job_id` 展示来源分析摘要，并按 `candidate_id` 关联后续 Review/Materialization 日志。
- UI 只能依据 `learning_events.status='accepted'` 显示“已采纳并生效”。`accept` job 运行中显示“正在审核并应用”，失败显示脱敏错误与可重试状态，不能提前乐观展示成功。
- 旧 `accepted/rejected` 记录没有 job/event 时显示为“历史记录，无阶段日志”，不得合成虚假时间线。
- 分析或人工决定失败后，前端必须刷新持久 job/event ledger；采纳失败还要重读 current value，以显性暴露“目标已写入但审核终态待对账”等可恢复状态。ledger 查询失败与真实空记录是两个状态，不能同时展示。
- 本轮 job 在桌面进程中同步执行。创建 job 时写入 `owner_pid + owner_start_token`；macOS 使用进程启动时间、Linux 使用 `/proc/<pid>/stat` start time、Windows 使用 process creation time。`ensure_schema` 只把 owner 缺失、系统检查已死亡，或 PID 存活但启动标识不一致的 `queued/running` 关闭为 `failed`，追加 `reason=process_restart`。另一个共享数据库且进程身份仍匹配的 CodeFactory owner 必须保持运行。用户重新运行会创建新的分析 job，旧中断证据保留，不伪装成断点续跑。

### 旧库增量迁移

- `ensure_schema` 使用 `CREATE TABLE IF NOT EXISTS` 创建两张新表和索引，并用现有 additive migration 方式为 `learning_events` 增加可空 `job_id`、为 `evolution_jobs` 增加可空 `owner_pid` 与 `owner_start_token`；不得重建表或改写旧状态。
- 旧库的 `pending/accepted/rejected`、`support_count` 和 `evidence_json` 原样保留。旧 `accepted` 继续代表已按历史路径生效；不回放历史采纳，也不补写推测事件。
- 新代码读取不到 job/event 时必须降级到旧候选展示；旧版本代码可忽略新增表和可空列。
- migration 必须幂等；应用重复启动、部分升级后重启以及空数据库初始化得到相同 schema。

### Evals 与 Activation 禁用边界

本轮不新增 `eval_cases/eval_runs`、activation 状态或部署动作，也不把现有 Terminal-Bench benchmark 当作通用 Eval 门禁。工作台可以展示禁用的 `Evals`、`Activation` 阶段说明，但不得暴露可调用命令、自动触发评测、修改 tool gate、生成或启用 Skill、改写 harness/code，或把 `accepted` 宣称为 `eval_passed/active`。

本轮 `accepted` 只表示当前目标已按现有本地语义物化；是否经过评测、是否适合发布或推广到其他项目均未知。后续引入 Eval/Activation 时必须单独设计兼容迁移和显式状态，不能追溯性地给旧记录补写“评测通过”。

### 锁屏不阻断验证

- `scripts/verify-evolution-workbench-headless.mjs` 启动本地 Vite、使用系统 Chrome/Edge 的 headless 模式加载专用验收入口，并在 1366×768 与 390×812 两个真实浏览器布局引擎视口执行候选选择、确认取消、拒绝、采纳、焦点、历史和精确日志断言；390 视口还要求固定决策栏与确认按钮完整落在 viewport 内。
- 验收入口只由独立 HTML 加载，使用 Tauri 官方 mock IPC 和有界 fixture，不进入 production `index.html` bundle，不读取用户数据库或凭据。
- headless receipt 证明 DOM、CSS layout、键盘与状态流不依赖交互桌面，在锁屏时替代不可用的桌面控制接口；它不检测或证明 OS 当时确实锁屏，也不证明 Tauri 壳或安装包。receipt 必须写 `interactive_desktop_required=false` 与 `os_lock_state_observed=not_measured`，不得硬编码伪造锁屏观测。
- `.github/workflows/release.yml` 的 macOS job 继续从 DMG 复制精确 app、启动隔离 HOME、检查稳定窗口和数据库；该远端 runner 证明真实发布壳与产物，不依赖本机是否锁屏。
- CI 同时运行 unit/integration、headless viewport 和 Rust tests。任何一层失败都保持 PR/Release blocked；不得因为本机锁屏静默跳过。

## 8. 后续目标架构

在上述最小合同稳定并有真实使用证据后，再评估 `agent_runs`、`agent_events`、独立 `improvement_candidates`、`candidate_evidence`、`candidate_reviews`、`candidate_change_receipts` 和通用 `eval_cases/eval_runs`。只有届时才引入 `draft -> pending_review -> approved/rejected -> materialized -> eval_passed/eval_failed -> pending_activation -> active` 等完整状态机及 `expected_revision`；不得为了未来模型提前复制现有数据。

## 9. 风险

- 双写不一致：规范化写入失败应使当前持久 Agent 路径显式失败或产生 dropped 计数，不能静默声称轨迹完整。
- 采纳假成功：Review 通过但 memory/preference 写入失败时，候选必须保持 `pending`，UI 不得显示 `accepted`。
- 外部文件与 SQLite 非原子：memory 物化必须使用稳定 marker/receipt 和幂等恢复，避免重试重复追加。
- 关联漂移：job、candidate 和 event 的 `cwd` 必须一致，禁止空 cwd fallback 把记录挂到错误项目。
- 数据膨胀：只存脱敏限长摘要，不复制 token delta 与 reasoning。
- 错误归因：权限拒绝与 hook cancel 不计入工具故障。
- 文档漂移：每个阶段必须更新 `docs/self-evolution/README.md` 的真实状态。
