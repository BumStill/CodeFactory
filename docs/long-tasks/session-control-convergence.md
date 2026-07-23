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

- Current phase: 设计完成，准备失败验收。
- Current checkpoint: 已从 `origin/main@3071cb8` 建立隔离 worktree；原工作区及正在运行的 CodeFactory 会话未被修改或终止。
- Next owner: Codex 编写红灯测试并按三个切片实施。
- Updated at: 2026-07-23

## Completed Items

- [x] 复盘真实 44 分钟会话：进程活跃但 Full access 强制 Execute，4 次 recovery 被 UI 隐藏。
- [x] 编写业务、架构、UX 设计与 Requirements Traceability。
- [x] 启动独立意图/门禁与前端可见性只读审查。

## Remaining Items

- [ ] 为 Full access/dispatch 解耦写失败测试。
- [ ] 为累计 recovery 不可重置写失败测试。
- [ ] 为 live reducer、hydration 和状态卡写失败测试。
- [ ] 实现 CF-SCC-R1..R11。
- [ ] 完成前端、Rust、构建、治理、headless viewport 和真实 App 验证。
- [ ] 提交 PR、CI 全绿、合并并按刻意发版流程验证公开产物。

## Blockers

- None

## Evidence

- Local evidence: 2026-07-23 正式 App SQLite 中 15:00 后存在 4 条 `gate_recovery`、4 条 `rejected_candidate`、56 次工具调用；CodeFactory 进程与 provider 连接持续活跃。
- Release evidence: pending。
- Blocking evidence: none。

## AI Collaboration

- context scope: `commands/chat.rs` dispatch、AgentLoop completion finalization、stream events、chat reducer/hydration、MessageList、输入取消语义。
- assumptions: Full access 只属于权限层；内部 prompt/草稿保持隐藏；本切片不实现进程组立即强杀。
- review point: 两个独立只读审查分别核对 Rust 状态机与前端事件/持久化边界。
- validation result: pending。

## Stop Boundary

- 不在设计或单元测试后停止。
- 不在 PR 创建、合并或 release workflow 启动后停止。
- 只有公开安装产物的真实会话路径验证通过，或有明确外部 blocker，才允许标记完成/live。
