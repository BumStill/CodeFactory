# 会话控制收敛长任务记录

## Basics

- Task ID: CF-SCC-20260723
- Title: Full access 意图解耦、完成门禁收敛与恢复过程可见性
- Feature spec: `docs/specs/feature-specs/session-control-convergence.md`
- Related Req IDs: CF-SCC-R1..R12

## Completion Standard

- Done means: CF-SCC-R1..R12 均有失败先行测试、真实 App 主路径、PR+CI 和精确发布产物证据。
- Blocked means: 同一阻塞有连续证据，且没有安全的本地、headless 或 GitHub runner 替代路径。

## Current State

- Current phase: 本地实现和可执行的真实 App 验收完成，准备 PR+CI。
- Current checkpoint: 分支已同步 `origin/main@7709fb4`；CodeFactoryDev 已用 DeepSeek v4 Flash 验证模型选择与恢复历史卡，Dev keychain 没有对应 API key，真实 provider 回合停在凭据前。
- Next owner: Codex 提交 PR；在远端 runner/发布产物补齐不依赖本机 Dev keychain 的剩余证据。
- Updated at: 2026-07-23

## Completed Items

- [x] 复盘真实 44 分钟会话：进程活跃但 Full access 强制 Execute，4 次 recovery 被 UI 隐藏。
- [x] 编写业务、架构、UX 设计与 Requirements Traceability。
- [x] 启动独立意图/门禁与前端可见性只读审查。
- [x] 失败先行覆盖 Full access 意图、累计 recovery、跨回合 replay、可见恢复和停止语义。
- [x] 实现 CF-SCC-R1..R11，并同步 Desktop OpenAI、Anthropic 与 Headless recovery 预算。
- [x] 前端 293 项测试、生产构建、Rust 定向测试和治理基线通过。
- [x] 真实 App 验证信任模式文案、DeepSeek v4 Flash 默认选择，以及恢复摘要的尺寸、层级、次数、原因和最近活动；内部 prompt、命令和参数未泄漏。

## Remaining Items

- [ ] 在具备 provider 凭据的发布产物补测完整诊断响应和运行中停止操作；本机 Dev 不复制或迁移用户凭据。
- [ ] 推送后重跑 Rust workspace 在线 CI-status 测试；当前唯一失败是本地提交尚未存在于 GitHub。
- [ ] 提交 PR、CI 全绿、合并并按刻意发版流程验证公开产物。

## Blockers

- 本机 `com.codefactory.dev` keychain 没有 `codefactory.endpoint.deepseek`；真实 App 已确认 DeepSeek v4 Flash 路由，但 provider 请求在发送前以明确缺失凭据结束。自动化已覆盖 dispatch 与停止文案，完整 provider/停止主路径待发布产物补测。

## Evidence

- Local evidence: 2026-07-23 正式 App SQLite 中 15:00 后存在 4 条 `gate_recovery`、4 条 `rejected_candidate`、56 次工具调用；CodeFactory 进程与 provider 连接持续活跃。
- Test evidence: `pnpm test` 68 files / 293 tests passed；`pnpm build` passed；Rust workspace 464 passed / 1 online remote-SHA check failed / 6 ignored。
- Real App evidence: `CodeFactoryDev` 中信任模式显示“减少确认，不改变分析/执行意图”；独立持久化 recovery 会话显示紧凑的“执行已中断 / 第 1/3 次 / 未收到最终答复 / 最近活动”状态卡，未显示内部英文 gate prompt、命令或参数。
- Release evidence: pending。
- Blocking evidence: `com.codefactory.dev` 读取 `codefactory.endpoint.deepseek` 时返回缺失凭据；未复制或迁移正式 App 凭据。

## AI Collaboration

- context scope: `commands/chat.rs` dispatch、AgentLoop completion finalization、stream events、chat reducer/hydration、MessageList、输入取消语义。
- assumptions: Full access 只属于权限层；内部 prompt/草稿保持隐藏；本切片不实现进程组立即强杀。
- review point: 两个独立只读审查分别核对 Rust 状态机与前端事件/持久化边界。
- validation result: 两项只读审查结论已吸收；失败先行、本地自动化和无凭据真实 App 路径通过；完整 provider 回合、远端 CI 和发布产物待验收。

## Stop Boundary

- 不在设计或单元测试后停止。
- 不在 PR 创建、合并或 release workflow 启动后停止。
- 只有公开安装产物的真实会话路径验证通过，或有明确外部 blocker，才允许标记完成/live。
