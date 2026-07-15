# Evolution Agent 长任务记录

## Basics
- Task ID: CF-EVO-20260714
- Title: Session 轨迹信息提取与持续改进闭环
- Feature spec: `docs/specs/feature-specs/evolution-agent-closed-loop.md`
- Related Req IDs: CF-EVO-R1..R14

## Completion Standard
- Done means: Phase 0 到 Phase 5 的规格均落地；候选从真实轨迹产生，经人工裁决、物化、评估与独立激活；PR+CI 通过；发布后真实主路径验证。
- Blocked means: 同一外部权限/环境阻塞连续三轮且已无安全替代路径，或用户明确暂停。

## Current State
- Current phase: Phase 0/1 — Review Workbench + Persistent Job Ledger 交付门禁
- Current checkpoint: R9-R14 已完成一级「进化审查」入口、project scope 深链、待审主从布局、人工采纳/拒绝、持久作业与结构化节点日志、owner liveness 重启终态和分层验收；全量自动化已通过。隔离 `CodeFactoryEvolutionDev` 的早期版本已在完全权限下走通宽屏、390px 窄屏、采纳、拒绝与重启恢复；最终 UX/日志加固版本仍需同路径复验。当前仍是 Dev 证据，PR #104 尚未完成本轮 CI、合并、刻意发版和发布包主路径验证，因此保持 `not live`
- Next owner: 完成独立产品/架构复核和最终 Dev 实地复验，推进 PR #104 CI、合并和刻意发版；随后在发布产物重跑同一主路径。versioned change request、通用 Evals、activation 与更完整的工作流引擎保持后续边界
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

## Remaining Items
- 本轮交付门禁：最终版本 Dev 实地复验、独立 QA/架构复核、PR #104 CI、合并、刻意发版、安装包/build metadata 与发布版真实主路径验证。
- Phase 0 其余底座证据：真实 dispatch error、修复后 post-mortem 真实候选、Quick 多会话稳定分析 scope 的产品边界、最终 Evidence/隐私整体验收。
- Phase 1 后续：更细的结构化 extractor、分析窗口、partial/dropped、失败节点重试与校准；本轮已提供最小持久 job/event ledger、幂等决定和重启中断明确终态，但不宣称是通用工作流引擎。
- Phase 2：统一候选/Review 工作台。
- Phase 3：受控 materializer、receipt、rollback。
- Phase 4：通用 eval case、回归门禁、activation。
- Phase 5：显式授权的 draft PR 流程。

## Blockers
- 共享 `/Applications/CodeFactoryDev.app` 仍由另一条任务占用，但本任务已用独立 identifier、1421 端口和临时 HOME 绕开，不再阻塞本轮桌面验收。最后一次 post-mortem 定向会话准备时桌面控制因检测到物理输入暂停；这只留下该项真实候选证据，不阻塞代码、自动化或已完成的主路径证据。

## Evidence
- Local evidence: frontend `35 files / 164 tests`、Rust `247 passed / 6 ignored`；`pnpm build`、governance baseline、long-task validator 与 `git diff --check` 通过。本轮新增回归覆盖准确 badge、project scope、A→B 请求竞态与立即清旧数据、远程复盘默认关闭/显式开启、候选详情与双确认、真实 current value/读取失败门禁、有效偏好全局回退、来源 job 精确直达与不可用不回退、旧来源打开同时保留最新阶段流、超出最近作业窗口的决定日志回链、日志读取失败非空态、来源作业读取失败非假空态、失败动作自动刷新 ledger/current value、决定完成后等待 busy 解除再聚焦下一候选、最后候选完成后的焦点去向、单 job 最新 500 条终态、作业/事件顺序、分析最终事务回滚、failure event/job terminal 双向半失败事务回滚、同项目分析并发门禁、采纳幂等、拒绝无物化、跨进程决定占用、偏好原子回滚、失败脱敏、真实死亡子进程恢复、PID 重用识别和存活 owner 保留。仓库没有独立 `pnpm typecheck` 命令，production build 实际执行 `tsc`；构建的既有 chunk-size warning、既有 Workspace 测试 act warning 与 Rust benchmark dead-code warning 单独保留，不伪装成新失败。
- Real app: 成功 `bash` 为 `done/375ms`；权限拒绝为 `denied/0ms` 且目录不存在；失败 `bash` 为 `error/175ms`。测试 token 在 assistant/tool/normalized 字段均为 0 命中，用户主动输入字段为 1 命中。
- Anonymous: 真实 shell 前后开发库计数保持 `sessions=1, messages=0, tool_calls=0, learning_events=0, cost_entries=0`。
- Harness: 未抢占共享 `CodeFactoryDev`；新建 `/Applications/CodeFactoryEvolutionDev.app`，使用 `com.codefactory.evolution-dev`、1421 和临时 HOME/SQLite。完全权限由 agent 启用并保存，普通工具调用不再要求用户授权；权限拦截验收结束后恢复完全权限。
- Replay/Restart: hook cancel 的 `bash` 为 `denied/0ms`，目标文件不存在且只有一条 replay；修复 hydration 后完整重启进程，工具卡仍显示 error 与 `Tool call cancelled by hook.`。后续真实 done/error 卡同样可恢复。
- Cross-session Review: 两个 `/private/tmp/codefactory-evolution-project` session 共形成 `bash` 19 次调用、5 次 error；UI 候选显示 2 个 session/19 次/5 错误/26%。采纳后 DB 为 `accepted pattern, support_count=2, support_unit=sessions`，项目记忆写入同一建议；再次分析保持 1 条，未重复生成。
- Post-mortem: 实地日志发现旧 `default_model=gpt-5.5` 被错误发往 DeepSeek；已改为 endpoint active-model。随后真实请求暴露 max_tokens=500 时只有 reasoning/无 final content；已加入 reasoning 隔离和 2000-token 有界重试并通过回归测试，但本轮尚未取得真实模型候选，仍列为剩余证据。
- Quick scope: 单个 Quick session 可进入 chat post-mortem，但“新建快速任务”使用独立 scratch cwd；当前 cross-session miner 按精确 cwd 聚合，因此 Quick-to-Quick 模式尚没有稳定 scope，不伪装为已覆盖。
- Integrity review: 独立架构复审提出 JSON 脱敏、terminal/replay 原子一致性、旧 support 口径和 preference key 四类高风险边界；均先补失败测试再修复，目标与完整回归已通过。
- Release evidence: PR #104 仍为 draft；旧提交 `e0acbd5` 的 CI/check 已全绿，但本轮工作台变更尚待新 CI。当前 `not live`，尚未合并、刻意发版或安装包验证。
- Review Workbench evidence: 隔离验收库用 2 个真实 project session 加入有界轨迹 fixture（8 次 `bash`、2 次 error）后，从工作台真实运行本地 miner；UI/SQLite 一致得到 1 个候选、25% error。采纳后只增加 1 个稳定 memory marker；另一个跨 2 session/3 task retry 候选拒绝后 marker 仍为 1。第三次分析保留 1 条待审用于 viewport 验收。结构化日志显示 2 session、11 条轨迹、1 个候选及完整阶段；完整进程重启后历史仍在，人工注入的 legacy `running` 验收 job 被 schema 恢复逻辑明确关闭为 `failed` 并追加 `process_restart` 事件。既有截图保存在隔离验收目录 `/tmp/codefactory-evolution-workbench-evidence/`，但生成时间早于最终双确认、current value、精确 job 直达和焦点加固，不能替代最终版本复验。
- Privacy/permission: 验收 HOME 的设置确认 `permissions.full_access=true` 且 `remote_postmortem_enabled=false`；普通 Dev 路径无用户授权弹窗。本地 miner/job 日志只展示聚合计数和白名单字段，raw prompt/reasoning 不进入事件详情。
- Blocking evidence: 当前 main 的 self-evolution 查询读空已由代码审计确认。

## AI Collaboration
- context scope: PPT、origin/main v1.43.0→v1.43.1、session/agent/learning/evidence/self-evolution/benchmark/UI。
- assumptions: 首期保持本地 Tauri+SQLite，不照搬重型服务。
- review point: R9-R11 只做现有真实候选的可信 Review Shell；R12 才引入 persistent jobs；没有 versioned candidate/review 前不做变更请求，没有 Evals/activation 数据前不显示对应状态。
- validation result: 规划、架构、QA 一致拒绝“先做空数据看板”；R9-R14 已实现并在隔离 Dev 真实 App 证明入口、scope、候选、Review、物化、日志、重启和 viewport 路径。通用 Evals/activation、versioned review、Quick 稳定 scope 和其余 Phase 0/1 底座证据仍未完成；在 PR/CI/刻意发版和发布包复验之前，本轮工作台也仍是 `not live`。

## Stop Boundary
- 不在本地单测后停止。
- 不在 UI 出现或 deploy 输出后停止。
- 完整能力未结束前持续按阶段记录 `not live` 与下一交付门禁。
