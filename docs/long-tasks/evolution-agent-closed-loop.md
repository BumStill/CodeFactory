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
- Current checkpoint: Phase 0 首个 Trace Truth 切片已本地实现并实地验证，等待 PR+CI；Phase 0 尚未整体验收
- Next owner: 当前 PR delivery，随后继续 hook/dispatch/miner 与统一候选模型
- Updated at: 2026-07-14

## Completed Items
- 完整拆解 23 页能力方案。
- 核对 origin/main@v1.43.0 与现有 self-evolution/benchmark 实现。
- 规划、架构、QA 独立审查确认 normalized `tool_calls` 空写入阻断。
- OpenAI/ChatGPT 与 Anthropic AgentLoop 写入 `pending -> done|error|denied` 标准轨迹，权限拒绝与 hook cancel 不计为工具故障。
- 普通/Quick chat 无 task 时使用有限脱敏会话摘要；Evidence 改读真实 `tool_calls`。
- 系统派生 assistant/reasoning/tool/history/normalized trace 统一脱敏；用户主动输入维持既有聊天保留语义。
- CodeFactoryDev 改用 `com.codefactory.dev` 数据目录；已验证与生产历史隔离。

## Remaining Items
- Phase 0 剩余：hook cancel、dispatch error、修复后 post-mortem 真实请求/候选、跨会话 miner、Profile 审核链路实地验证。
- Phase 1：结构化 extractor、窗口、幂等 job、校准。
- Phase 2：统一候选/Review 工作台。
- Phase 3：受控 materializer、receipt、rollback。
- Phase 4：通用 eval case、回归门禁、activation。
- Phase 5：显式授权的 draft PR 流程。

## Blockers
- None。

## Evidence
- Local evidence: Rust `210 passed / 6 ignored`；frontend `135/135`；相关 Python `47/47`；governance baseline 与 long-task validator 通过；production build 通过。Python 全量 discover 仍因当前环境缺少可选 `harbor` 包无法收齐，该边界不伪装成通过。
- Real app: 成功 `bash` 为 `done/375ms`；权限拒绝为 `denied/0ms` 且目录不存在；失败 `bash` 为 `error/175ms`。测试 token 在 assistant/tool/normalized 字段均为 0 命中，用户主动输入字段为 1 命中。
- Anonymous: 真实 shell 前后开发库计数保持 `sessions=1, messages=0, tool_calls=0, learning_events=0, cost_entries=0`。
- Harness: 首次实测发现旧 wrapper 仍写生产 DB；新增 dev identifier 配置后重启验证 `com.codefactory.dev` 空数据集与独立 schema。首次测试会话保留在现有数据库，未擅自删除用户数据。
- Post-mortem: 实地日志发现旧 `default_model=gpt-5.5` 被错误发往 DeepSeek；已改为 endpoint active-model 解析并加回归测试。最终真实请求复验因共享 CodeFactoryDev 已被另一条运行中任务覆盖而未抢占，仍列为剩余证据。
- Release evidence: not live；待 PR+CI、合并、刻意发版与安装包验证。
- Blocking evidence: 当前 main 的 self-evolution 查询读空已由代码审计确认。

## AI Collaboration
- context scope: PPT、origin/main v1.43.0、session/agent/learning/evidence/self-evolution/benchmark/UI。
- assumptions: 首期保持本地 Tauri+SQLite，不照搬重型服务。
- review point: Phase 0 真实数据后再批准统一 Review 与 Evals 实现。
- validation result: 规划、架构、QA 一致拒绝“先做空数据看板”；真实 App 已证明标准轨迹不再为 0，但完整 Phase 0/全能力仍 not live。

## Stop Boundary
- 不在本地单测后停止。
- 不在 UI 出现或 deploy 输出后停止。
- 完整能力未结束前持续按阶段记录 `not live` 与下一交付门禁。
