# 超长会话渲染韧性长任务记录

## Basics

- Task ID: CF-LSR-20260724
- Title: 超长会话分页、惰性工具解析与持久化尾部恢复
- Feature spec: `docs/specs/feature-specs/long-session-rendering-resilience.md`
- Related Req IDs: CF-LSR-R1..R14

## Completion Standard

- Done means: 失败先行、实现、完整自动化、原故障等比例真实 App 压测、独立复审、PR/main CI、公开发布和发布 App 复验全部完成。
- Blocked means: 同一外部阻塞连续有证据，且本地、headless、GitHub runner 和公开产物均无法提供安全替代。

## Current State

- Current phase: 精确候选进入 PR 与发布链。
- Current checkpoint: 有界分页、完整 DTO 桥接预算、增量 reducer、折叠卡惰性解析、并发请求门禁与 prepend 滚动锚点已经实现；全量自动化、生产构建和治理基线绿色，原故障 SQLite 隔离副本已在 Dev App 恢复可交互。
- Next owner: 提交 PR，等待 CI、合并、刻意发版与公开安装包复验。
- Updated at: 2026-07-24

## Completed Items

- [x] 现场确认 CodeFactory 主进程无 Rust 主线程死锁。
- [x] 确认最终 assistant 回复在 17:28:31 已落库，UI 到 17:30 后仍停在 21.5 秒。
- [x] 确认故障会话包含 3743 条消息、1726 次工具调用和约 9.8 MB 文本。
- [x] 确认 WebContent RSS 约 2.25 GB、峰值 footprint 约 2.1 GB、CPU 峰值 48%。
- [x] 采样确认 JavaScript 微任务持续进行数组复制、拼接、对象分配和 GC。
- [x] 定位 `get_messages SELECT *`、无界 MessageList、折叠卡提前 diff 解析和 stream 全数组更新四个放大器。
- [x] 完成业务、架构、UX 设计与 Requirements Traceability。
- [x] 启动独立架构、性能测试和 UX/QA 只读审查。
- [x] 新增按真实用户回合分页的 `get_message_page`，并加入 400 行、完整序列化 DTO 2 MiB、UI 内容单字段 128 KiB 硬上限。
- [x] 首次选择会话只 hydrate 最新页，提供“加载更早记录”并用 request owner/revision 防止迟到结果覆盖 live stream。
- [x] stream reducer 改为目标消息定点更新，历史对象引用保持稳定。
- [x] MessageRow/ToolCallCard memo；diff、知识结果与错误摘要在折叠态不再进行大结果分配。
- [x] sticky scroll 提供受控 prepend anchor，并防止分页期间切会话后修改新会话 scrollTop。
- [x] 原故障 SQLite 隔离副本 Dev App 初次验收：最终回复可见、可上翻；WebContent 稳态约 222.4 MB、峰值约 582.7 MB、静置 CPU 0–1.2%。

## Remaining Items

- [x] 补齐 selection race、分页/stream race、local notice、prepend anchor 与兼容矩阵回归。
- [x] 全量自动化、build、治理基线和原故障等比例隔离 Dev App 验收。
- [x] 三个独立子 agent 完成架构、测试和 QA 复审，未发现 release blocker。
- [ ] PR+CI、合并。
- [ ] 刻意发版和公开安装产物真实 App 验收。

## Blockers

- None.

## Evidence

- Local evidence: 正式 CodeFactory v1.64.8 进程 86951 与 WebContent 86962；SQLite session `43ec91cc-9b2d-4213-b8a7-a1592acf8bc5`；最终回复已落库但窗口未刷新。
- Test evidence: 最终本地候选前端 72 files / 312 tests；Rust 483 passed / 0 failed / 6 ignored；`pnpm build`、TypeScript、治理基线和 `git diff --check` 通过。
- Real App evidence: 原故障 SQLite 隔离副本在候选 Dev App 中最新 final 可见、可上翻；WebContent 稳态约 222.4 MB、峰值约 582.7 MB、静置 CPU 0–1.2%。最终并发/桥接边界补丁后本机锁屏，按仓库约束不要求用户解锁，改由精确 SHA 自动化、GitHub macOS 构建产物 smoke 和公开安装包验证继续收口。
- Release evidence: `not live`.
- Blocking evidence: none.

## AI Collaboration

- context scope: `get_messages`、SQLite row ordering、`dbMessagesToUI`、Zustand runtime、stream reducer、MessageList、ToolCallCard、sticky scroll、发布验证。
- assumptions: 原始历史不得删除；旧 hydration 语义保持；首版优先有界分页和惰性解析，不引入第三方虚拟列表。
- review point: 三个独立子 agent 分别审查分页架构、稳定红灯测试和真实 App UX/性能门槛。
- validation result: 现场根因、实现、自动化和隔离 Dev App 性能证据已验证；PR、CI、公开发布与发布 App 证据待补。

## Stop Boundary

- 不在设计或单元测试后停止。
- 不在 PR 创建、CI 绿色、合并或 release workflow 启动后停止。
- 只有公开安装产物的真实超长会话路径通过，或有明确外部 blocker，才允许停止。
