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

- Current phase: 已完成并发布。
- Current checkpoint: PR #197 已合并，main 精确 SHA CI 绿色，v1.64.11 已公开发布；原故障 SQLite 隔离副本 Dev App、双平台 release 构建、发布前后 macOS DMG smoke 和本机匿名下载复验均通过。
- Next owner: None.
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
- [x] PR #197 全绿合并，合并提交 `84b91b2a5fd4650e17578cb6aa07bd8f54c57803` 的 main CI 通过。
- [x] v1.64.11 刻意发版，Windows/macOS 构建、平台 smoke、公开 DMG 回下载和真实窗口验证通过。

## Blockers

- None.

## Evidence

- Local evidence: 正式 CodeFactory v1.64.8 进程 86951 与 WebContent 86962；SQLite session `43ec91cc-9b2d-4213-b8a7-a1592acf8bc5`；最终回复已落库但窗口未刷新。
- Test evidence: 最终本地候选前端 72 files / 312 tests；Rust 483 passed / 0 failed / 6 ignored；`pnpm build`、TypeScript、治理基线和 `git diff --check` 通过。
- Real App evidence: 原故障 SQLite 隔离副本在候选 Dev App 中最新 final 可见、可上翻；WebContent 稳态约 222.4 MB、峰值约 582.7 MB、静置 CPU 0–1.2%。精确 release 提交的 DMG 在 GitHub runner 发布前后两次通过安装级 smoke；本机从公开 URL 无认证下载后再次通过 bundle/version/arm64、闭环 receipt 和真实 1200×800 窗口稳定性验证。
- Release evidence: `live`；[PR #197](https://github.com/BumStill/CodeFactory/pull/197)；[main CI](https://github.com/BumStill/CodeFactory/actions/runs/30087779079)；[v1.64.11 release](https://github.com/BumStill/CodeFactory/releases/tag/v1.64.11)；[release workflow](https://github.com/BumStill/CodeFactory/actions/runs/30088206979)。公开 DMG SHA-256 `6845480013932f0b7d89fc852cdb71b25e43b06461e5a05758006ae43f015f03` 与 GitHub asset digest 一致，内置 build SHA 为 `51c81c61af44758c8f26230a44cf9e4e8ccbed66`，`latest.json` 指向 1.64.11 的 darwin/windows 更新资产。
- Blocking evidence: none.

## AI Collaboration

- context scope: `get_messages`、SQLite row ordering、`dbMessagesToUI`、Zustand runtime、stream reducer、MessageList、ToolCallCard、sticky scroll、发布验证。
- assumptions: 原始历史不得删除；旧 hydration 语义保持；首版优先有界分页和惰性解析，不引入第三方虚拟列表。
- review point: 三个独立子 agent 分别审查分页架构、稳定红灯测试和真实 App UX/性能门槛。
- validation result: 现场根因、实现、自动化、隔离 Dev App 性能、PR/main CI、双平台 release、公开更新清单和发布 App 证据全部验证通过。

## Stop Boundary

- 不在设计或单元测试后停止。
- 不在 PR 创建、CI 绿色、合并或 release workflow 启动后停止。
- 只有公开安装产物的真实超长会话路径通过，或有明确外部 blocker，才允许停止。
- Satisfied at: 2026-07-24，v1.64.11 公开安装产物验证通过。
