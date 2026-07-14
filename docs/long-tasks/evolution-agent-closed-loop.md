# Evolution Agent 长任务记录

## Basics
- Task ID: CF-EVO-20260714
- Title: Session 轨迹信息提取与持续改进闭环
- Feature spec: `docs/specs/feature-specs/evolution-agent-closed-loop.md`
- Related Req IDs: CF-EVO-R1..R8

## Completion Standard
- Done means: Phase 0 到 Phase 5 的规格均落地；候选从真实轨迹产生，经人工裁决、物化、评估与独立激活；PR+CI 通过；发布后真实主路径验证。
- Blocked means: 同一外部权限/环境阻塞连续三轮且已无安全替代路径，或用户明确暂停。

## Current State
- Current phase: Phase 0 — Trace Truth
- Current checkpoint: PR #104 的首个 Trace Truth 切片 CI 已绿；隔离 `CodeFactoryEvolutionDev` 已完成 hook cancel、重启恢复、跨会话 miner、人工采纳与记忆写入实测；本轮完整性加固已提交，待推送并重跑 PR CI，Phase 0 尚未整体验收
- Next owner: 当前 PR delivery 与 Phase 0 实地验收，随后才进入结构化 extractor 与统一候选模型
- Updated at: 2026-07-14

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

## Remaining Items
- Phase 0 剩余：真实 dispatch error、修复后 post-mortem 真实候选、Quick 多会话稳定分析 scope 的产品边界、最终 Evidence/隐私整体验收。
- Phase 1：结构化 extractor、窗口、幂等 job、校准。
- Phase 2：统一候选/Review 工作台。
- Phase 3：受控 materializer、receipt、rollback。
- Phase 4：通用 eval case、回归门禁、activation。
- Phase 5：显式授权的 draft PR 流程。

## Blockers
- 共享 `/Applications/CodeFactoryDev.app` 仍由另一条任务占用，但本任务已用独立 identifier、1421 端口和临时 HOME 绕开，不再阻塞本轮桌面验收。最后一次 post-mortem 定向会话准备时桌面控制因检测到物理输入暂停；这只留下该项真实候选证据，不阻塞代码、自动化或已完成的主路径证据。

## Evidence
- Local evidence: Rust `222 passed / 6 ignored`；frontend `141/141`；`cargo check`、TypeScript production build、governance baseline、long-task validator 与 diff whitespace check 通过。仓库没有独立 `pnpm typecheck` 命令，production build 已实际执行 `tsc`；构建仍有既有 chunk-size warning，Rust 仍有 5 个 benchmark dead-code warning。
- Real app: 成功 `bash` 为 `done/375ms`；权限拒绝为 `denied/0ms` 且目录不存在；失败 `bash` 为 `error/175ms`。测试 token 在 assistant/tool/normalized 字段均为 0 命中，用户主动输入字段为 1 命中。
- Anonymous: 真实 shell 前后开发库计数保持 `sessions=1, messages=0, tool_calls=0, learning_events=0, cost_entries=0`。
- Harness: 未抢占共享 `CodeFactoryDev`；新建 `/Applications/CodeFactoryEvolutionDev.app`，使用 `com.codefactory.evolution-dev`、1421 和临时 HOME/SQLite。完全权限由 agent 启用并保存，普通工具调用不再要求用户授权；权限拦截验收结束后恢复完全权限。
- Replay/Restart: hook cancel 的 `bash` 为 `denied/0ms`，目标文件不存在且只有一条 replay；修复 hydration 后完整重启进程，工具卡仍显示 error 与 `Tool call cancelled by hook.`。后续真实 done/error 卡同样可恢复。
- Cross-session Review: 两个 `/private/tmp/codefactory-evolution-project` session 共形成 `bash` 19 次调用、5 次 error；UI 候选显示 2 个 session/19 次/5 错误/26%。采纳后 DB 为 `accepted pattern, support_count=2, support_unit=sessions`，项目记忆写入同一建议；再次分析保持 1 条，未重复生成。
- Post-mortem: 实地日志发现旧 `default_model=gpt-5.5` 被错误发往 DeepSeek；已改为 endpoint active-model。随后真实请求暴露 max_tokens=500 时只有 reasoning/无 final content；已加入 reasoning 隔离和 2000-token 有界重试并通过回归测试，但本轮尚未取得真实模型候选，仍列为剩余证据。
- Quick scope: 单个 Quick session 可进入 chat post-mortem，但“新建快速任务”使用独立 scratch cwd；当前 cross-session miner 按精确 cwd 聚合，因此 Quick-to-Quick 模式尚没有稳定 scope，不伪装为已覆盖。
- Integrity review: 独立架构复审提出 JSON 脱敏、terminal/replay 原子一致性、旧 support 口径和 preference key 四类高风险边界；均先补失败测试再修复，目标与完整回归已通过。
- Release evidence: PR #104 仍为 draft；上一提交的 CI/governance checks 已绿，本轮变更尚待推送后重新跑 CI；not live，尚未合并、刻意发版或安装包验证。
- Blocking evidence: 当前 main 的 self-evolution 查询读空已由代码审计确认。

## AI Collaboration
- context scope: PPT、origin/main v1.43.0→v1.43.1、session/agent/learning/evidence/self-evolution/benchmark/UI。
- assumptions: 首期保持本地 Tauri+SQLite，不照搬重型服务。
- review point: Phase 0 真实数据后再批准统一 Review 与 Evals 实现。
- validation result: 规划、架构、QA 一致拒绝“先做空数据看板”；真实 App 已证明 hook/重启/跨会话 Review/物化链路，独立复审发现的四类完整性问题已自动化收口。post-mortem 真实候选、Quick 稳定 scope 和最终 Evidence 整体验收仍未完成，完整 Phase 0/全能力仍 not live。

## Stop Boundary
- 不在本地单测后停止。
- 不在 UI 出现或 deploy 输出后停止。
- 完整能力未结束前持续按阶段记录 `not live` 与下一交付门禁。
