# M287：会话活性不变式（Session Liveness Invariant）

> 状态：架构审视完成，进入实现。本文承接任务文件 `/Users/leo/.codefactory/agent-tools/task-m287-session-liveness-invariant.md`，不改变 R1–R8 的需求含义。

## 1. 背景与问题定义

用户要求的不是“卡住后手工诊断/重置”，而是系统始终满足活性：任何未结束会话都必须有且只有一个可识别的推进者，并且该推进者有明确截止时间；过期后由统一监督者接手，而不是等待用户发现。

现场同族问题包括：模型流断开后整端点冷却、权限应答通道消失、模型调用永久挂起、前台命令长时间占用、唤醒早于冷却边界后被跳过、交付网络重试无及时观察、重启后延迟接续，以及 UI 把所有情形统称为“等待中”。过去的修补按故障族分别处理，没有形成跨模型、工具、权限、唤醒、交付、重启的一条全局不变式，因此新的停滞形态仍可绕过各自的局部恢复代码。

## 2. 非结束情形清单与活性契约

统一术语：
- **推进者（owner）**：当前唯一有权使会话进入下一可观察状态的执行实体。
- **截止时间（deadline）**：推进者必须在该时刻前产生心跳、完成、转交或显式续期。
- **接手者（takeover）**：推进者过期后负责取消/结算旧推进者并恢复原任务的统一监督者。
- **最早可恢复时间（not-before）**：冷却、额度窗口等外部约束允许再次尝试的最早时刻；唤醒不得早于它，也不得晚于它超过一个监督周期。

| 非结束情形 | 唯一推进者 | 截止时间 | 超时/失联后的接手者与动作 | 界面必须显示 |
|---|---|---|---|---|
| 正在连接模型/等待首字节 | 当前 provider attempt | 模型调用空闲截止 + 调用总时长截止（二者取早） | Liveness Supervisor 取消 attempt；无副作用则同回合有界重试，否则按 receipt 对账 | `正在连接模型`，必要时附路由名 |
| 正常流式输出 | 当前 provider stream | 每个 chunk 刷新空闲截止；总时长上限不续期 | Supervisor 取消挂起流；有部分输出但无副作用时按同回合流恢复策略重试 | `模型正在生成` |
| 流中途断开 | provider recovery remediation | 短有界重试的下一次截止 | Supervisor 同回合重试；不得把一次解码中断升级成整端点 15 分钟冷却 | `模型连接中断，系统自动重试/自愈（第 n 次）` |
| 所有模型路由暂不可用（402/429/额度/冷却） | 一个持久化 wake schedule | `wake_at ∈ [not_before, not_before + supervisor_period]` | Supervisor 修正过早/过晚唤醒；边界早触发只重排，不消耗退避 | `模型暂不可用，HH:mm 自动继续`，附原因 |
| 正在运行前台工具 | 当前 tool execution | 工具具名超时；运行时心跳只更新活动，不无限延长硬上限 | Supervisor/工具执行器终止进程树并把可恢复错误交回同回合 | `正在运行：<命令摘要>` |
| 预计长时间构建/测试 | background task row | `created_at + timeout_sec`，且任务进程/日志有心跳 | Background Task Supervisor 杀超时进程树；完成/失败均唤醒原会话 | `后台运行：<命令摘要>` |
| 等待工具审批 | 持久化 permission intent + 当前可用响应通道 | intent `expires_at`；通道存活是必要条件 | Permission adapter 重挂同一 intent；无法重挂则在截止内按现有“未应答”规则结算并让原回合继续 | `等待你批准：<工具>` |
| 等待用户补充核心信息 | 持久化 attention request | 业务等待不自动代决；技术上必须有稳定 request_key，截止取“请求仍可见”的租约并可续期 | Supervisor 只修复丢失的可见请求，不替用户作产品决定 | `等待你回答：<明确问题>` |
| 等待已排定唤醒 | wake schedule | `next_at`，且不晚于最早可恢复时间 + 一个监督周期 | Wake scheduler/ Supervisor 到点投递；过早则原地校正，过期则立即补发 | `<原因>，HH:mm 自动继续` |
| 交付网络/CI/队列重试 | delivery remediation 或 PR subscription | 每次远端观察截止 + bounded next observation | Delivery adapter 重新观察同一 PR/head；不得创建第二个 PR，不得无观察地静止 | `正在核对 PR #n：<状态>` / `网络恢复后系统自动重试/自愈` |
| 重启恢复 | startup reconciliation claim | 启动后一个监督周期内 | Startup pass 先对账 permission/provider/tool/delivery receipt，再立即 claim 原 objective | `正在恢复中断任务` |
| Supervisor 自身接手 | liveness remediation lease | lease 60s，20s 心跳；每轮 ≤60s | 下一 Supervisor 进程可在 lease 过期后接手 | `系统正在恢复：<真实情形>` |
| 正常后台构建仍活跃 | background task row | 任务 deadline 尚未到且进程存活 | 不接管；仅继续观察 | `后台运行：<命令摘要>` |
| 真正等待用户产品决定 | attention request | 不设业务代决截止，但可见性租约必须持续有效 | 只恢复请求展示，不替用户决定 | `等待你决定：<问题>` |

反例约束：正常持续流式输出、正常后台任务、真实等待用户核心输入/业务决定都不得因“静止时间长”被误杀；监督依据推进者类型、deadline、心跳和持久化 identity，不以 UI 是否收到新文本作为唯一信号。

## 3. 现有实现中的无截止或截止不统一点

以下是实现前审视所见的真实代码边界（行号以本任务基线为准）：

1. **模型流等待没有空闲/总时长截止**
   - `src-tauri/src/agent/mod.rs:180-190`：`next_stream_item` 只在 `stream.next()` 与 cancel flag 之间 select，没有 idle timeout 或 total deadline。
   - `src-tauri/src/agent/model_transport.rs:1267-1278`、`:1586-1614`：Responses 与 OpenAI-compatible SSE 都依赖该无截止 helper。
   - 后果：连接成功但不再产出 chunk 时可永久占有 turn；Objective heartbeat 仍可把它误认为健康。
   - 过去修补失效原因：`objective.rs` 的 transport outage 策略只处理“请求已返回错误”后的重试；挂起调用从未返回错误，无法进入该策略。

2. **active Objective 的兜底过于宽松且检查频率不满足 R1**
   - `src-tauri/src/agent/objective.rs:111-114`：`STALLED_ACTIVE_OBJECTIVE_MS = 30min`。
   - `src-tauri/src/agent/objective_supervisor.rs:34-37`：stalled sweep 每 60 个 5s poll，即每 5 分钟才运行。
   - 后果：无人推进但状态仍 active 的会话可静止 30–35 分钟；现场 80 分钟挂起也可能持续被错误心跳掩盖。
   - 过去修补失效原因：该 sweep 是 orphan backstop，不理解模型、工具、审批等具体推进者及各自 deadline。

3. **transport outage 仍允许 15 分钟静止窗口**
   - `src-tauri/src/agent/objective.rs:279-305`：outage window 15 分钟、probe 20 秒、重试 ladder 最长 4 分钟。
   - `src-tauri/src/agent/objective.rs:601-613`：全局 next observation 上限仍为 5 分钟。
   - 后果：即使存在 scheduled observation，用户仍可能数分钟看不到推进；且流解码中断被归入 transport unreachable 时会错误冷却整个 route。
   - 过去修补失效原因：把“路径不可达”和“已建立 SSE 的单次流中断”合并成同一故障类。

4. **前台命令有硬超时，但缺少自动转后台与会话级接手**
   - `src-tauri/src/tools/bash.rs:27-28`：默认 300s，最大 1800s。
   - `src-tauri/src/tools/bash.rs:446-474`：命令在前台等待至 timeout，才把错误交回模型。
   - 后果：长构建可占用前台 5–30 分钟；工具心跳让 Objective 看似活跃，但对话不能推进。
   - 过去修补失效原因：只给命令设置 timeout，没有按命令类别自动转为 background task，也没有把运行命令摘要投影成统一推进者。

5. **审批通道在首次超时后延长等待，但通道丢失仍先返回失败**
   - `src-tauri/src/agent/permission_gateway.rs:691-715`：持久化 expires_at 后创建进程内 oneshot。
   - `src-tauri/src/agent/permission_gateway.rs:782-829`：首次等待超时后延长并重发 UI 请求。
   - `src-tauri/src/agent/permission_gateway.rs:873-884`：channel closed 直接结算为 denial outcome，随后才靠 Objective recovery。
   - `src-tauri/src/agent/permission_intent.rs:1722-1749`：重启时把旧 pending intent 标为 channel_closed，而不是直接重挂可见 prompt。
   - 后果：窗口 reload/进程重启时存在“持久化 intent 仍待处理，但当前响应通道不存在”的间隙。
   - 过去修补失效原因：M281 只允许同进程后续调用替换 `is_closed()` sender；没有统一监督者主动重挂 prompt。

6. **唤醒边界由多个局部函数修正，没有全局 deadline 校验**
   - `src-tauri/src/agent/objective.rs:1242-1257`：只修正 `no_model_available` 且已在过去的时间。
   - `src-tauri/src/agent/failover.rs:564-601`：ModelUnavailable 单独编码 resume_at。
   - `src-tauri/src/agent/background_tasks.rs:1589-1601`：wake scheduler 又单独读取/校正 resume_at。
   - 后果：过早 2 秒、晚于最早恢复数小时等边界可在不同层得到不同处理；过早触发还可能被当成失败增加退避。
   - 过去修补失效原因：M123/M220/M234 分别修了过去时间、最早时间、冷却 floor，但没有由同一 invariant 校验所有 wake owner。

7. **重启恢复入口分散，不能证明一个监督周期内全部接续**
   - `src-tauri/src/agent/objective_supervisor.rs:1364-1378`：启动时只显式处理 waiting PR 与 merged settlement。
   - `src-tauri/src/agent/objective_supervisor.rs:1485-1498`：running-without-executor 在主循环后部恢复。
   - `src-tauri/src/agent/permission_intent.rs:1722-1775`：permission 又有独立 startup reconciliation。
   - 后果：恢复顺序、owner 互斥与 deadline 没有单一快照；某类恢复失败只能等下一 heartbeat。
   - 过去修补失效原因：每个事故新增一个 startup hook，缺少“枚举所有未结束会话并验证恰好一个 owner”的启动扫描。

8. **UI 以 streaming 布尔值代替推进者类型**
   - `src/components/MessageList.tsx:738-771`：`!streaming` 直接落到 waiting phase。
   - `src/components/MessageList.tsx:775-780`：过去统一显示“等待中”。
   - `src/components/MessageList.tsx:969-1037`：StuckSessionRecovery 把诊断/重置按钮放在健康未结束回合旁。
   - 后果：模型连接、工具运行、审批、冷却、真正卡住不可区分；把系统恢复责任推给用户。
   - 过去修补失效原因：UI 只有派生文案，没有消费持久化的 owner/deadline 投影。

9. **已有 Supervisor 监督 Objective/remediation，不监督所有实际执行者**
   - `src-tauri/src/agent/objective_supervisor.rs:23-39`：5s poll、60s lease、20s lease heartbeat 已存在。
   - `src-tauri/src/agent/objective.rs:1087-1103`：WaitingSystem 已要求 owner/remediation/next observation；但 active、waiting authorization、background task 与 provider attempt 不共享同一 validator。
   - 后果：局部状态都“合法”，组合后却可能没有推进者或有两个推进者。
   - 过去修补失效原因：校验对象是 Objective decision envelope，不是会话全局 liveness snapshot。

## 4. 统一机制设计

### 4.1 持久化 Liveness Lease

新增一张会话级结构化表 `session_liveness_leases`，每个未结束 session 至多一行 active lease：
- identity：`session_id`, `objective_id`, `root_turn_id`, `revision`；
- owner：`owner_kind`（model/tool/background/permission/user/wake/delivery/startup/supervisor）、`owner_id`；
- 时间：`started_at`, `heartbeat_at`, `deadline_at`, 可选 `not_before_at`；
- UI：`display_kind`, `display_summary`（只存安全摘要，不存命令完整敏感参数）；
- fencing：`process_instance`, `lease_generation`；
- recovery：`recovery_action`, `recovery_attempt`。

唯一索引保证一个 session 只有一个 active owner。owner 转交必须在同一事务中完成：旧 lease settled + 新 lease insert/update，禁止先清空再异步补写。

### 4.2 统一 Liveness Snapshot 与 Validator

纯函数 `evaluate_session_liveness(snapshot, now)` 返回：
- `Healthy(owner, deadline)`；
- `Violation::MissingOwner`；
- `Violation::MultipleOwners`；
- `Violation::DeadlineExceeded`；
- `Violation::DeadChannel`；
- `Violation::WakeBeforeNotBefore`；
- `Violation::WakeTooLate`。

Snapshot 由 Objective、run control、provider attempt、tool call、background task、permission intent、attention request、wake schedule、delivery receipt 联合读取。任何局部表不能再独立宣称会话“在运行”。

### 4.3 Liveness Supervisor

在现有 Objective Supervisor 内增加独立的 `LIVENESS_SUPERVISOR_INTERVAL`（默认 5s，硬上限 60s），每轮：
1. 枚举所有未结束 session；
2. 构建 snapshot 并运行 validator；
3. 健康则不改动；
4. 违例则先写 `session_liveness_events`（detected）；
5. 以 session fencing claim 接手，取消/结算旧 owner；
6. 按 owner_kind 调用现有 domain adapter（provider/tool/permission/wake/delivery/startup）；
7. 写处置结果（recovered/rescheduled/settled/failed），继续原 Objective。

结构化事件字段：`id, session_id, objective_id, owner_kind, violation_kind, idle_ms, deadline_at, detected_at, action, outcome, resolved_at, detail_code`。不写凭据、完整命令或消息正文。

### 4.4 各域适配

- Model：`next_stream_item` 增加 idle timeout；整个 provider attempt 外包 total timeout。流中断独立分类，同回合有界重试，不触发 endpoint cooldown。
- Tool：前台命令 lease 使用具名 hard timeout；识别构建/测试长命令并自动建立 background task lease，原回合转为 wake owner。
- Permission：Supervisor 检测 dead oneshot/旧 process instance 后，以同一 intent identity 重发 PermissionRequest 并绑定新 channel；到期则按“未应答”结算，不伪造拒绝。
- Wake：统一 `normalize_wake_at(not_before, proposed, now)`；早触发只重排且不增加 attempt，过期立即补发。
- Restart：Supervisor 启动前执行一次 full liveness scan，顺序为 receipt 对账 → permission 重挂 → owner claim → resume，保证一个周期内接续。
- Delivery：复用同一 PR/head receipt；网络失败只排 bounded observation，不新建 PR。

### 4.5 UI 投影

前端只消费后端投影的 `display_kind/display_summary/deadline/wake_at`，不得用 `!streaming` 猜状态。标准文案：
- model_connecting → `正在连接模型`；
- model_streaming → `模型正在生成`；
- tool_running → `正在运行：<摘要>`；
- background_running → `后台运行：<摘要>`；
- permission_waiting → `等待你批准：<工具>`；
- user_input_waiting → `等待你回答：<问题>`；
- wake_waiting → `<原因>，HH:mm 自动继续`；
- recovering → `系统自动重试/自愈：<情形>`。

`StuckSessionRecovery` 不再在回合旁渲染。诊断数据保留为 R8 只读后台入口；不提供“重置会话状态”作为正常用户补救动作。

## 5. Requirements Traceability

| Req | 实现边界 | 最低证据 |
|---|---|---|
| R1 | lease 表、snapshot、validator、唯一 active owner | 正反例单元测试 + 数据库约束测试 |
| R2 | supervisor 周期扫描、自动接手、events 表 | synthetic 故障注入 + event 字段断言 |
| R3 | owner UI 投影、删除行内恢复按钮 | 浏览器 headless + light/dark 截图 |
| R4 | permission channel 重挂/截止结算 | channel drop synthetic E2E |
| R5 | 路由、402/429、流中断、idle/total timeout、source chain | 假模型服务器 matrix |
| R6 | 前台超时、长命令自动后台化、完成唤醒 | 真 shell fixture + UI 文案 |
| R7 | startup full scan，一个周期内恢复 | 隔离 HOME 跨进程重启 E2E |
| R8 | 只读 metrics command/dispatch | DB fixtures + dispatch 输出断言 |

## 6. 测试矩阵（先红后绿）

必须逐项先看到失败：
1. 流中途断开：同回合恢复，用户消息数不增加，记录 `stream_interrupted`。
2. 流挂起：idle/total deadline 内取消并恢复，记录 `model_idle_timeout` 或 `model_total_timeout`。
3. 402：切换可用 route；全不可用时中文原因 + 最早恢复时间。
4. 429/无额度：同上，且不早于真实 cooldown/window。
5. 审批 channel drop：同 intent 重挂且保持可见，或截止结算；零人工新增消息。
6. 前台命令超时：进程树终止、原回合继续；长构建自动后台化并完成唤醒。
7. app 重启：一个 supervisor period 内恢复原 objective，不能误判任务结束。
8. wake 早于 cooldown：只重排到 not-before，不增加 backoff attempt。
9. 反例：持续流输出、正常后台构建、真正等待用户回答均不被接管。
10. UI：真实 Chromium headless 验证所有标准文案，且页面不存在“会话诊断/重置会话状态”；light/dark 截图落 `.codefactory-cache/`。

## 7. 约束与交付

- 仅使用 `edit_file` / `write_file` 修改文件；JSON 查询使用 `jq` 或 `grep`。
- 不修改 `docs/testing/scenario-registry.json`、validator、workflow；缺映射仅追加 `docs/testing/scenario-mappings.json`。
- 构建与临时文件仅放 `.codefactory-cache/`。
- R1–R8 未全部完成前不准备 PR 正文、不调用交付；不完整 PR 保持 draft 且不 armed。
- 全部完成后 PR 使用 `Release-Urgency: immediate`；真机安装版 24 小时验收由编排方发版后执行。

## 8. M287b：正式程序生产 schema 与监督可观测性修复

# M287b：M287 的活性表在正式程序里从未创建，统一监督在生产中整体失效

## 现场（2026-11-26 22:1x，v1.128.0 真机，只读核实）
- 生产库 `~/Library/Application Support/com.codefactory.app/codefactory.db` 中不存在 `session_liveness_leases`、`session_liveness_events`（`.tables` 无任何 liveness 表）。
- `migrations/0027_session_liveness.sql` 只在 `src-tauri/src/agent/liveness_store.rs:371` 的 `#[cfg(test)] mod tests` 里通过 `include_str!` 执行；生产路径没有任何地方执行它（对比 `thin.rs:120` 生产代码会执行 0022）。
- `objective_supervisor.rs` 约 1425–1430 行每 5 秒调用 `project_unfinished_objectives` / `scan_liveness_violations`，在生产中必然因 no such table 失败，只记 `tracing::warn`，R1/R2/R7/R8 在真实应用里全部未生效；测试全绿是因为测试自建内存库。
- 合并后审计结论：为何合并前没发现——所有 M287 测试都自建 schema，没有任何测试走「真实应用启动 → 生产 schema 初始化 → 监督周期」路径。

## 规格（追加到 `docs/specs/feature-specs/m287-session-liveness.md` 的 M287b 小节）
- R1：应用启动时（与其他模块 ensure_schema 同一初始化路径）幂等创建 0027 的全部表与索引；已有库升级与全新库都覆盖。
- R2：`project_unfinished_objectives` / `scan_liveness_violations` 失败时不得只 warn 后静默：同一错误持续出现时要有可观测信号（结构化事件或健康状态），且不得影响其他监督逻辑。
- R3：查清 M287 其余新增的生产依赖（命令注册、只读度量查询 `query_session_liveness_metrics`、前端调用）在真实应用里是否都已接线；未接线的一并接上，并列出清单。
- R4：补一条「真实启动路径」测试：用生产初始化函数打开一个全新文件库（临时目录），断言 liveness 表存在、一次监督周期能写入 lease、制造过期能写入 violation 事件；再用一个旧版本 schema 的库升级同样断言。不得在测试里手动执行 0027 SQL 来凑绿。

## 约束
改文件只用 edit_file / write_file。不改信任根；场景缺口只追加 `docs/testing/scenario-mappings.json`。不碰凭据、settings.json、其他会话的 PR，不动任何 merge-queue: hold 标签。规格文件在工作区外，用 bash cat 只读查看。

## 交付
PR 正文 `R | 是否完成 | 证据` 列 R1–R4；`Release-Urgency: immediate`（M287 核心能力在生产中未生效）。全部完成、CI 全绿后正常交付。

## 验收（真机，发版后编排方做）
安装后生产库出现 liveness 表；运行中的会话在 `session_liveness_leases` 有 owner 行；度量查询返回数据。

Release-Urgency: immediate

## 9. M287c：活性监督在生产中空转——误判、重复记违例、"recovered" 名不副实

## 现场（v1.129.0 真机，2026-11-27 00:19–00:46，只读核实）
- `session_liveness_events` 26 分钟写入 1931 行（约每 5 秒 8 行），全部 `violation_kind=deadline_exceeded, action=resume_original_objective, outcome=recovered`，但对应 lease 的 `deadline_at` 不变，下一周期再次判违例 → 无限循环，约 10 万行/天。
- 被判违例的 8 个会话中：
  - 5 个是 10–11 月的遗留非终态 objective（0808ca03 active 11-07、48864195/7ac1e453/d91f9d3a waiting_system delivered_awaiting_ci 10-18、af7763ba/afc40ac6 active 11-07/11-11），早已无人推进。
  - 2 个是正在运行的会话 6e140abf、96c4e911：它们此刻有 `background_tasks.status='running'` 的后台任务，却被投影成 `owner_kind=model`、deadline 已过 → 误判。
  - `2da3e2a0`、`e2de2968` 为 `waiting_core_input`（等用户），owner=user_input，正常续租。
- objectives 表在 00:31 后没有任何变化（revision 未增），说明 "resume_original_objective" 实际未生效，但事件仍写 `outcome=recovered`。

## 规格（追加到 `docs/specs/feature-specs/m287-session-liveness.md` 的 M287c 小节）
- R1：owner 投影必须识别全部合法推进者并取正确截止时间：运行中的后台任务（deadline=任务超时）、待触发唤醒（deadline=next_at+余量）、PR/CI 监视订阅（deadline=订阅约定的复查时间）、待审批、等用户。存在合法推进者时不得投影成过期的 model owner。
- R2：违例去重：同一 (session, owner, deadline) 只记一次违例；处置后必须续租或改写 lease（新的 owner/deadline）或把 objective 结算到终态，否则下一周期不得重复写同一违例。事件表加保留上限或聚合（写明策略）。
- R3：`outcome` 必须反映真实结果：apply_decision 成功才记 recovered；revision 冲突、决策被拒、无可恢复路径分别记明确的失败结果与原因，并进入 R8 度量。
- R4：遗留非终态 objective（超过可配置天数无任何推进，如 >7 天）不纳入每周期恢复：一次性结算为明确终态（或标记 stale 并停止投影），写一条结构化事件；不得把它们重新唤醒去消耗模型额度。
- R5：用本次现场形状写测试（先红后绿）：运行中后台任务不误判；遗留 objective 不循环；同一违例不重复写；apply 失败不记 recovered；反例：真正过期且无推进者的会话仍被接手一次。

## 约束
改文件只用 edit_file / write_file。不改信任根；场景缺口只追加 `docs/testing/scenario-mappings.json`。不碰凭据、settings.json、其他会话 PR，不动任何 merge-queue: hold 标签。规格文件在工作区外，用 bash cat 只读查看。

## 交付
PR 正文 `R | 是否完成 | 证据` 列 R1–R5；`Release-Urgency: immediate`（生产中每天约 10 万行无效事件、真实卡顿无法接手）。

## 验收（真机，发版后）
安装后 10 分钟内 `session_liveness_events` 新增行数 ≤ 违例会话数；运行后台任务的会话不出现违例；遗留 objective 被一次性结算。

Release-Urgency: immediate
