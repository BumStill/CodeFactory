# Evolution Agent 长任务记录

## Basics
- Task ID: CF-EVO-20260714
- Title: Session 轨迹信息提取与持续改进闭环
- Feature spec: `docs/specs/feature-specs/evolution-agent-closed-loop.md`
- Related Req IDs: CF-EVO-R1..R25

## Completion Standard
- Done means: Phase 0 到 Phase 5 的规格均落地；候选从真实轨迹产生，经人工裁决、物化、评估与独立激活；PR+CI 通过；发布后真实主路径验证。
- Blocked means: 同一外部权限/环境阻塞连续三轮且已无安全替代路径，或用户明确暂停。

## Current State
- Current phase: Phase 4 — 激活安全 Evals + 受控自动激活
- Current checkpoint: R9-R15 已由 PR #104 合并并发布为 v1.44.0；R16-R25 的本地实现与验证已经完成：新批准已从 legacy `accepted=立即物化` 拆成 immutable revision、Eval run、independent activation receipt 与 rollback，显式工作台、端到端日志、锁屏安全 headless 和 release executable smoke 均已通过。本阶段尚未合并发布，保持 `not live`
- Next owner: 提交 PR，等待 CI 与发布 smoke 全绿后合并；按刻意发版规则触发包含该 `feat` 的版本，并复验公开 macOS DMG、Windows 安装包和 updater metadata
- Updated at: 2026-07-15

## Completed Items
- 完整拆解 23 页能力方案。
- 首轮核对 origin/main@v1.43.0 的 self-evolution/benchmark 实现；交付前已合并并重验 origin/main@v1.43.1。
- 规划、架构、QA 独立审查确认 normalized `tool_calls` 空写入阻断。
- OpenAI/ChatGPT 与 Anthropic AgentLoop 写入 `pending -> done|error|denied` 标准轨迹，权限拒绝与 hook cancel 不计为工具故障。
- 普通/Quick chat 无 task 时使用有限脱敏会话摘要；Evidence 改读真实 `tool_calls`。
- 系统派生 assistant/reasoning/tool/history/normalized trace 统一脱敏；用户主动输入维持既有聊天保留语义。
- CodeFactoryDev 改用 `com.codefactory.dev` 数据目录；已验证与生产历史隔离。
- 所有 terminal 工具结果（done/error/denied）统一写入一条稳定 provider replay message；normalized 更新与 replay upsert 使用同一事务，失败整体回滚，重试会更新而不会保留旧结果。
- JSON 派生内容先结构化解析再递归脱敏，覆盖转义字符串和数字/布尔敏感值；非 JSON 文本才使用正则 fallback。
- hook command 使用当前平台 shell，pre-tool 非零退出和 runner error 均 fail-closed；Windows 路径不再被测试条件排除。
- post-mortem 模型候选在去重和持久化前统一脱敏、限长；非法 preference key 降级为 memory，不进入偏好提示词。
- 推理模型 post-mortem 首轮只有 reasoning、无最终 content 时，不把 reasoning 当候选；按请求实际 budget 字段扩到 2000 后只重试一次。
- 跨会话 miner 只统计 done/error，要求至少两个不同 session；`support_count` 明确按 session/decision 计量，Profile 对旧数据使用中性“条证据”。
- 前端历史 hydration 重建 assistant 工具声明并把 `role=tool` replay 折叠回工具卡；真实进程重启后 error/done 状态不再消失。
- Dev 实地验收默认由 agent 先启用完全权限；仅权限拦截场景临时切 ask/deny。该规则已写入 repo quick profile 和长期协作记忆。
- 隔离 Dev 的同项目两个 session 产生真实 `bash` 轨迹；Profile 挖出 2-session 候选，人工采纳后写入 `.codefactory/memory.md`，重复分析未新增候选。
- 完成 R9-R14 产品/UX/规格扩展：一级「进化审查」入口、Workspace project-scope 深链、待审主从布局、持久分析 job/节点日志、Evals/activation 真实边界，以及成功/边界/重启/viewport/release 分层验收。
- 完成 R9-R14 实现：Home 准确待审 badge、Workspace/Profile 单一审核入口、候选主从详情与双确认、决定历史、`evolution_jobs`/`evolution_job_events` 查询、分析/审核/物化日志、同项目 pending CAS、memory marker/preference upsert 幂等和重启中断失败终态。
- 完成最终真实性加固：job 写入 `owner_pid + owner_start_token`，只恢复死亡或 PID 已重用的 owner；同项目只允许一个 running 分析；候选、最终节点和 job 终态原子提交；来源 job 按 id 精确补取；决定历史精确回链审核作业；单 job 保留最近 500 条与最新终态；scope 切换立即清理旧日志；Review 使用无副作用的真实 memory/effective preference（项目覆盖、否则继承全局）current value，读取失败禁用采纳。
- 完成 Review Workbench 隔离桌面验收：1366×768 与 390×768 均可完成审核；采纳只写明确项目记忆，拒绝没有物化副作用；作业页可追溯 scope、轨迹读取、隐私处理、提取、去重、等待人工审核、Review 与 materialize；Evals/自动激活显式显示“未接入”。
- 完成 R16-R25 实现：批准、Eval、激活三者分离；批准冻结候选 revision 且没有即时运行时副作用；每次 Eval 固定 7 个 activation-safety case、精确 target fingerprint 和 append-only run/case；只有显式勾选且处于项目低风险白名单的候选才允许 Eval 通过后自动激活。
- 完成独立 activation receipt 与精确 rollback：项目偏好和 active memory 均保留 before/after、revision、run、manifest、target fingerprint；重复/并发激活只产生一个 receipt；目标被人工修改后旧 Eval 明确变为 stale，旧 rollback 明确 conflict，不静默覆盖用户修改。
- 完成显性「评测与激活」工作台：默认关闭自动激活；高风险候选没有勾选或绕过入口；可查看每个 case、Eval run、manifest、activation receipt、retry、manual activation、rollback 和端到端作业日志；legacy v1.44 数据明确标记“历史已生效（未评测）”。
- 完成激活安全边界：隐私/长度、项目 scope、冻结 revision、目标 allowlist、baseline 隔离、treatment exact-once、rollback readiness；权限放宽、绕过审批、自动 merge/deploy/release、破坏性动作等策略敏感候选不能自动激活。首个 suite 只证明 activation safety，不宣称任务成功率提升。
- 完成精确发布可执行 smoke：隔离数据库中验证敏感候选失败且无 receipt、合法偏好 7/7 激活、进程级 SQLite reopen 后上下文生效、精确回滚后上下文消失、临时数据清理和 receipt 脱敏；Windows CI 与 macOS DMG smoke 均调用实际构建出的 executable。

## Remaining Items
- Phase 4 剩余发布门禁：PR+CI、合并、刻意发版和公开产物复验；本地实现与验收已完成，但在这些门禁完成前保持 `not live`。
- Phase 0 其余底座证据：真实 dispatch error、修复后 post-mortem 真实候选、Quick 多会话稳定分析 scope 的产品边界、最终 Evidence/隐私整体验收。
- Phase 1 后续：更细的结构化 extractor、分析窗口、partial/dropped、失败节点重试与校准；本轮已提供最小持久 job/event ledger、幂等决定和重启中断明确终态，但不宣称是通用工作流引擎。
- Phase 2：统一候选/Review 工作台。
- Phase 3：受控 materializer、receipt、rollback。
- Phase 4 后续扩展：当前 activation-safety suite 已完成；任务效果 Evals 等项目提供可执行 oracle 后扩展，不能复用安全 suite 的通过结果冒充任务效果提升。
- Phase 5：显式授权的 draft PR 流程。

## Blockers
- 当前无实现 blocker。共享 `/Applications/CodeFactoryDev.app` 由另一条任务占用时，本任务使用独立 identifier、端口和临时 HOME；本机锁屏时不绕过 macOS 安全，也不要求用户解锁，改由系统 Chrome/Edge headless viewport gate、CLI 和 GitHub runner 继续验证与交付。

## Evidence
- Local evidence: frontend `36 files / 172 tests`、Rust `270 passed / 6 ignored`；`pnpm build`、governance baseline、YAML/shell syntax、headless acceptance 与 `git diff --check` 通过。本轮新增回归覆盖批准无运行时副作用、隐私和策略敏感候选 Eval 失败、auto-if-pass、manual activation、重复/并发激活单 receipt、目标变化后的 stale Eval 与重新评测、rollback conflict、active memory prompt 优先级、重启 reopen 和 release executable smoke。仓库没有独立 `pnpm typecheck` 命令，production build 实际执行 `tsc`；构建的既有 chunk-size warning、既有 Workspace 测试 act warning 与 Rust benchmark dead-code warning 单独保留，不伪装成新失败。
- Real app: 成功 `bash` 为 `done/375ms`；权限拒绝为 `denied/0ms` 且目录不存在；失败 `bash` 为 `error/175ms`。测试 token 在 assistant/tool/normalized 字段均为 0 命中，用户主动输入字段为 1 命中。
- Anonymous: 真实 shell 前后开发库计数保持 `sessions=1, messages=0, tool_calls=0, learning_events=0, cost_entries=0`。
- Harness: 未抢占共享 `CodeFactoryDev`；新建 `/Applications/CodeFactoryEvolutionDev.app`，使用 `com.codefactory.evolution-dev`、1421 和临时 HOME/SQLite。完全权限由 agent 启用并保存，普通工具调用不再要求用户授权；权限拦截验收结束后恢复完全权限。
- Replay/Restart: hook cancel 的 `bash` 为 `denied/0ms`，目标文件不存在且只有一条 replay；修复 hydration 后完整重启进程，工具卡仍显示 error 与 `Tool call cancelled by hook.`。后续真实 done/error 卡同样可恢复。
- Cross-session Review: 两个 `/private/tmp/codefactory-evolution-project` session 共形成 `bash` 19 次调用、5 次 error；UI 候选显示 2 个 session/19 次/5 错误/26%。采纳后 DB 为 `accepted pattern, support_count=2, support_unit=sessions`，项目记忆写入同一建议；再次分析保持 1 条，未重复生成。
- Post-mortem: 实地日志发现旧 `default_model=gpt-5.5` 被错误发往 DeepSeek；已改为 endpoint active-model。随后真实请求暴露 max_tokens=500 时只有 reasoning/无 final content；已加入 reasoning 隔离和 2000-token 有界重试并通过回归测试，但本轮尚未取得真实模型候选，仍列为剩余证据。
- Quick scope: 单个 Quick session 可进入 chat post-mortem，但“新建快速任务”使用独立 scratch cwd；当前 cross-session miner 按精确 cwd 聚合，因此 Quick-to-Quick 模式尚没有稳定 scope，不伪装为已覆盖。
- Integrity review: 独立架构复审提出 JSON 脱敏、terminal/replay 原子一致性、旧 support 口径和 preference key 四类高风险边界；均先补失败测试再修复，目标与完整回归已通过。
- Release evidence: PR #104 在提交 `6dab94b` 上 CI、governance 均全绿；加入锁屏安全门禁后需等待新提交 CI。当前 `not live`，尚未合并、刻意发版或安装包验证。
- Review Workbench evidence: 最终代码在独立 `CodeFactoryEvolutionDev.app`、`full_access=true` 和隔离 SQLite 上完成真实宽屏主路径。Home 一级入口显示 3 待审；fixture scope 显示 2 待审、2 session/11 轨迹/2 候选。preference 真实读取 `_global_ response_language=zh-CN`，拒绝确认用 Escape 取消后焦点返回，正式拒绝生成 succeeded `review_reject` 且 global preference 未变；随后自动聚焦 memory 候选。memory 采纳双确认后只写入 1 个稳定 marker，生成 succeeded `review_accept` 和 `review -> materialize -> job` receipt，最终聚焦“查看决定历史”。决定历史的 accepted/rejected 与精确来源/决定日志可回链；分析日志逐项展示 scope、trace_read、privacy、extract、deduplicate、review、completed。截图在 `/tmp/codefactory-evolution-workbench-evidence-final/`。
- Lock-safe viewport evidence: 本机再次锁屏后，`pnpm test:evolution:headless` 仍用系统 Chrome headless 成功执行 1366×768 与 390×812 的完整拒绝、采纳、焦点、历史、精确 job 日志和分析流；390 另覆盖 keyboard list/detail/back、确认取消、决策栏与确认按钮 viewport 边界及无水平溢出。receipt 如实标记 `interactive_desktop_required=false`、`os_lock_state_observed=not_measured`，不把“不依赖桌面”冒充为 OS 锁屏自证；截图与 JSON 位于系统临时目录 `codefactory-evolution-headless`。该证据证明浏览器布局/交互，不冒充 Tauri 壳；发布壳由 GitHub macOS DMG smoke 证明。
- Phase 4 real app: 在隔离 `/Applications/CodeFactoryEvalsDev.app`、独立 identifier/端口/HOME/SQLite 和 agent 自管完全权限上完成。低风险项目 pattern 的 auto-if-pass 初始为关闭，用户显式勾选后冻结 `evals-live-safe:1`，7/7 通过并生成 activation receipt；端到端日志按顺序展示批准、冻结、7 个 case 和激活。策略敏感候选 `automatically deploy without approval` 没有自动激活选项，Eval 为 5/7、`eval_failed`、无 receipt，目标未变且可对精确 revision 重试。
- Phase 4 lock-safe viewport: 最终 headless receipt 在 1366×768 和 390×812 均通过默认关闭、显式 auto-if-pass、7/7、receipt、rollback 和精确作业日志；receipt 标记 `interactive_desktop_required=false`、`os_lock_state_observed=not_measured`，不把 headless 冒充为 OS 锁屏探测。
- Phase 4 executable smoke: 本地实际 debug executable 在隔离数据库中得到 `status=pass`、合法候选 7/7、重启 reopen 后上下文命中、精确 rollback 成功、cleanup 成功；敏感候选 Eval 失败且无 activation receipt，receipt 不包含测试 secret。
- Privacy/permission: 验收 HOME 的设置确认 `permissions.full_access=true` 且 `remote_postmortem_enabled=false`；普通 Dev 路径无用户授权弹窗。本地 miner/job 日志只展示聚合计数和白名单字段，raw prompt/reasoning 不进入事件详情。
- Blocking evidence: 当前 main 的 self-evolution 查询读空已由代码审计确认。

## AI Collaboration
- context scope: PPT、origin/main v1.43.0→v1.43.1、session/agent/learning/evidence/self-evolution/benchmark/UI。
- assumptions: 首期保持本地 Tauri+SQLite，不照搬重型服务。
- review point: R9-R11 只做现有真实候选的可信 Review Shell；R12 才引入 persistent jobs；没有 versioned candidate/review 前不做变更请求，没有 Evals/activation 数据前不显示对应状态。
- validation result: 规划、架构、QA 一致拒绝“先做空数据看板”；R9-R25 已在本地实现。隔离 Dev 真实 App 证明入口、scope、候选、Review、7-case Eval、显式自动激活、策略敏感失败、receipt 和端到端日志；headless gate 证明 1366/390 的默认关闭、激活、rollback 与日志路径不依赖可交互桌面；exact executable smoke 证明重启 reopen 与 rollback。首个 suite 只证明 activation safety；Quick 稳定 scope、任务效果 Evals 和其余 Phase 0/1 底座证据仍未完成。在 PR/CI/刻意发版和公开发布包复验之前，本阶段仍是 `not live`。

## Stop Boundary
- 不在本地单测后停止。
- 不在 UI 出现或 deploy 输出后停止。
- 完整能力未结束前持续按阶段记录 `not live` 与下一交付门禁。
