# Evolution Agent UX 设计

## 1. 核心用户路径

Phase 0 不先新增大而全的看板，而是在真实轨迹闭合后增加一个薄的一级「进化审查」工作面，复用现有 Profile 学习日志、工具门控和自我改进能力。主路径：

1. 用户在真实项目中完成一次包含工具调用的 session。
2. 系统在后台记录成功/失败/拒绝、耗时和脱敏参数摘要。
3. session 结束后产生待审学习或跨会话模式，Home 显示待审数量，Workspace 可深链到当前项目。
4. 用户在「进化审查」的待审主从布局中查看证据并接受或拒绝。
5. 接受的知识/偏好影响后续 session；工具门控仍需单独点击启用；Evals 不自动运行。

## 2. 入口与信息架构

- Home 一级能力区提供「进化审查」卡片和待审 badge；入口文字必须描述“审核真实轨迹产生的改进”，不能用含混的“AI 学习”。
- Workspace 的「X 待审」是 project scope 深链，目标页必须显示当前项目路径并保留筛选，不得跳到最近使用项目。
- 页面顶栏包含：返回、标题、scope selector、最后分析时间、待审数量和「运行分析」。scope 显式区分项目、快速任务、全局。
- 一级 tab 只有「待我审核」「作业与日志」「决定历史」。Profile 继续承载偏好/记忆编辑；迁移期从 Profile 跳转到进化审查，不提供第二套可写审核按钮。
- 「待我审核」采用主从布局：左侧候选队列和筛选，右侧固定详情。窄窗口改为列表 -> 详情的单列导航，不能把右侧压到不可读。

候选详情顺序固定为：结论；建议去向和 scope；当前值 -> 建议值；脱敏证据摘要；来源 session/时间；风险与可逆性；人工动作。工具参数、错误和长路径默认摘要并可折叠，reasoning 永不展示。

## 3. 信息层级

- 第一层：状态和结论——成功、失败、拒绝、证据数、候选去向。
- 第二层：脱敏证据——工具名、参数摘要、错误摘要、耗时、session 来源。
- 第三层：原始详情——仅后续受权限控制打开，Phase 0 不增加原文浏览器。

不向用户展示 ClickHouse、HDBSCAN、OTel Collector 等部署名词；这些不是本地产品首期操作对象。

## 4. Review 行为

- `Accept` 只执行当前明确类型的动作：memory 写项目记忆、preference 写偏好。
- pattern 的 Harness/Evals/知识去向在统一候选模型完成前只能作为建议，不得假装已经落地。
- Skill 始终先生成 disabled proposal，用户预览后再启用。
- 工具门控只允许 `allow -> ask`，且必须由用户点击。
- 产品代码、PR、部署和发布不在 Phase 0 UI 中提供自动动作。
- 采纳按钮写出精确结果，例如「采纳并写入项目记忆」「采纳并更新偏好」「启用工具门控」；确认区必须展示 scope、目标、before/after 与“不会自动合并、部署或发布”。
- 拒绝不产生 memory/settings 副作用。拒绝原因可先作为后续模型字段设计，但在后端没有 candidate review 审计前不得只存于临时前端状态并宣称可用于校准。
- 变更请求的目标流程是 `pending_review -> changes_requested -> revision N+1 -> pending_review`；旧 revision 不可改写，stale revision 不可采纳。该流程属于后续统一 candidate/review 数据模型，不进入 Phase 0/1 的完成声明。

## 5. 作业与日志

「作业与日志」在同页展示持久分析流程，不弹出终端窗口。节点依次为：

1. 范围已锁定：scope、时间窗、纳入 session 数。
2. 轨迹读取：done/error/denied/pending/dropped 数量。
3. 隐私处理：脱敏命中和截断数量，不提供绕过。
4. 信号提取：detector/category 与产出数量。
5. 聚合去重：新候选、合并、阈值抑制数量。
6. 候选生成：draft/pending 数量。
7. 等待人工审核：待审与已决定数量。

当前本机 job 状态使用 `running | succeeded | no_candidates | failed`，启动恢复兼容旧的 `queued`；`partial/cancelled` 等状态等后续真的有对应执行语义再加入。节点同时显示文字、图标和时间，不只靠颜色。失败保留已完成节点和脱敏诊断；本轮已持久化 job/node、幂等人工决定，并以 PID 与进程启动标识共同识别 owner，只把 owner 已死亡、PID 已被重用或旧版无 owner 的 `queued/running` 明确关闭为 `failed`，不能误杀其他仍存活进程的作业，也不虚构断点恢复。

来源作业必须按 candidate 的 `job_id` 精确打开；超出最近列表时单独查询，缺失时显式说明，不允许回退到最新作业。单作业日志展示最近 500 条并优先保留最新终态，达到上限时提示截断边界。详情与确认区的“当前值”必须读取真实 project memory/preference；读取失败时禁用采纳。切换项目后旧项目日志和当前值立即清空，不能在新 scope 标题下短暂展示旧 scope 数据。

分析或人工决定失败后，工作台必须刷新持久 job/event ledger 并进入“作业与日志”，不能只弹临时错误；采纳失败还要重读 current value。ledger 读取失败使用独立错误态，不能同时渲染“还没有分析作业”。精确来源刷新失败继续保留原 job id 并显示“来源作业不可用”；打开旧来源时仍补拉最新分析事件，保证顶层阶段卡不被旧作业操作清空。

分析作业不显示假的 materialization 进度；只有真实人工采纳 job 才显示 `review -> materialize` 事件。Evals 与 activation 仍显示“未接入”；以后接入时必须把 `approved`、`materialized`、`eval_passed`、`active` 分开。

## 6. 空态与错误态

- 真正没有达到样本门槛：显示需要多少真实调用，不显示“系统健康”。
- 采集失败或 dropped：明确显示数据不完整，禁止用 0 失败率表示正常。
- 普通聊天无 task run：仍可使用会话摘要；不得显示永久空态。
- anonymous：明确“无痕会话不会进入自进化分析”。
- 脱敏命中：详情以 `<redacted>` 展示，不提供绕过按钮。
- pattern 证据徽标只在 `evidence_json.support_unit` 明确声明时显示“session”或“决策”；旧记录缺少单位时统一显示中性的“条证据”，避免把历史调用数/任务数误说成跨会话数。
- 从未运行：说明将读取的本地数据并显示「运行分析」，不预先展示 0% 失败率。
- 样本不足：显示当前值和真实门槛，例如“1/2 个 session、5/8 次有效调用”。
- 成功但无候选：显示扫描范围、被阈值或去重抑制的数量，不写“系统健康”。
- 待审为 0：显示“已处理完”并链接决定历史。
- 决定成功后聚焦下一候选；若最后一条已处理，聚焦可操作的“查看决定历史”。取消确认才恢复到原采纳/拒绝按钮。
- 查询/采集失败必须显示 `failed`，不能降级成“暂无记录”；partial/dropped 显示证据不完整，高风险采纳动作禁用直到重试成功。
- scope/session 已删除时显示“来源不可用”；revision 或数据版本冲突时阻止动作并要求刷新。

## 7. Viewport

- 目标：1366×768 与窄窗口。
- Review 主动作在首屏可见；详情可折叠；错误与隐私状态不能只靠颜色。
- 长路径、错误和参数摘要必须换行或截断，不能撑破卡片。
- 真实桌面验证使用已注册的 CodeFactory Dev app wrapper，不能只依赖 jsdom；并行任务存在时使用独立 identifier、端口和数据目录，不抢占共享 wrapper。
- 普通 Dev 验收开始前由 agent 自行启用并保存“信任模式（完全放手）”，不把产品工具授权交给用户；只有权限询问、拒绝、hook cancel 或绕过防护本身是验收目标时才临时切回 ask/deny，结束后恢复完全权限。
- 完全权限只覆盖当前任务内的产品工具调用，不扩大部署、外发、账号、支付、交易、数据删除等权限。
- 1366×768 下左侧队列、右侧详情和主动作同时可见；窄窗口使用单列 list/detail，不允许水平滚动才能完成审核。
- 键盘可以完成 scope 切换、候选选择、证据折叠和确认/取消；焦点、错误、selected/running 状态均有文字或 aria 语义。
- 本机锁屏时不尝试绕过系统安全；同一 viewport/keyboard matrix 必须由 headless Chromium 继续执行并输出 receipt。390×812 的详情必须让固定采纳/拒绝栏和二次确认落在当前 viewport 内，并以键盘走通拒绝、采纳、历史和日志。receipt 只声明无需交互桌面，不自行伪造 OS 锁屏观测；真实 Dev App 截图作为桌面集成证据，远端 DMG smoke 作为发布证据，三者边界在 PR 中明确。

## 8. 真实桌面验收

在同一真实项目分别执行：allow 成功、ask 后拒绝、hook cancel、工具返回 error、dispatch error。核对工具卡和 SQLite 的 tool、status、duration、error、cwd、session；重启后仍可追溯。再执行 anonymous 同类动作，确认计数不变。最后从 Profile 运行跨会话分析，并验证它读取真实新轨迹而不是 fixture。

2026-07-14 隔离的 `CodeFactoryEvolutionDev` 已实地走过 allow/error、ask/hook cancel、完全权限无弹窗、进程重启后 done/error 工具卡恢复、两个同项目 session 的跨会话 miner、Profile 人工采纳、写入项目记忆和重复分析去重。真实 miner 展示 `bash` 跨 2 个 session、19 次调用、5 次错误、26%，与数据库一致。anonymous 零持久化沿用本日早先实测；真实 dispatch error 和修复后的 post-mortem 模型候选仍是 Phase 0 剩余证据，不得从单元测试推定通过。

新增进化审查面必须补充以下真实验收：

- Home 一级入口与待审数可见；Workspace 从当前项目深链后 scope/path 正确。
- 用真实新轨迹生成候选，详情的 session、调用、error、rate 与 SQLite/`evidence_json` 完全一致，系统派生字段和 Evidence 对测试 secret 为 0 命中。
- 采纳前 memory/settings 不变；采纳后只改变明确目标；拒绝无副作用；重复分析不新增同候选。
- 完整重启后待审、决定历史、job 终态和来源证据仍可追溯；anonymous 不进入队列或作业统计。
- 1366×768 与窄窗口截图/录屏证明 scope、队列、详情、证据和主动作可达且不重叠。
- release-facing 验收还需 PR+CI、安装包启动、build metadata 和发布版本上的同一主路径；Dev app 证据必须标记 `not live`。
