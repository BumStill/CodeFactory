# 流式时间线 Markdown 格式保持

## Basics

- Task ID: `streaming-timeline-markdown-preservation`
- Title: 流式时间线 Markdown 格式保持
- Feature spec: `docs/specs/feature-specs/long-session-rendering-resilience.md`
- Related Req IDs: CF-LSR-R15、CF-LSR-R16

## Completion Standard

- Done means: failure-first 测试、缓存实现、前端/构建/治理验证、隔离真实 App 成功与边界路径、PR/CI/合并、公开版本和安装包路径验收全部完成。
- Blocked means: 同一外部阻塞连续三轮仍无法推进，且已记录可复现证据和下一责任方。

## Current State

- Current phase: PR 与发布交付
- Current checkpoint: 本地、双视口 headless 和隔离真实 Tauri WebView 验收全部通过，等待提交与 CI。
- Next owner: Codex
- Updated at: 2026-07-29

## Completed Items

- 正式 App `v1.71.3`、SQLite 原文、实时事件顺序和前端分支已完成字段级诊断。
- 已确认 `v1.72.0` 基线仍包含相同缺陷。
- 已扩展 CF-LSR-R15..R16 规格。
- 已先写动态降级回归并在旧实现上确认失败。
- 已实现中间步骤 Markdown renderer 与稳定段 memoization。
- MessageList 聚焦套件 38/38、完整前端 393/393、TypeScript、生产构建、治理和长任务门禁通过。
- 1366×768、800×700 headless 验收通过：格式转换、15px、活动长时间线和无横向溢出均成立。
- 独立 `com.codefactory.streaming-markdown.dev` Tauri WebView 验收通过：成功路径保持标题、列表、行内代码、链接；12 轮边界路径全部保持结构化 Markdown。
- 隔离 wrapper、Vite、Cargo 进程已关闭并确认无残留。

## Remaining Items

- 完成同步门禁、提交、PR、CI、合并和刻意发布。
- 验证公开安装包及精确版本真实 App。

## Blockers

- None

## Evidence

- Local evidence: 会话 `43ec91cc-9b2d-4213-b8a7-a1592acf8bc5` 的消息 `c0353baa-b05a-4c39-b6e8-244a19b7cc26` 在 SQLite 中保留 1217 字完整 Markdown；旧实现新增测试失败于中间步骤缺少 `<strong>`；修复后 393 个前端测试、生产构建、双视口 headless 与独立 Tauri WebView 成功/边界路径通过。
- Release evidence: 待 PR、CI、公开版本和安装包验收。
- Blocking evidence: None。

## AI Collaboration

- context scope: MessageList 流式 timeline、chat event segment reducer、SQLite 持久化与正式 App 当前会话。
- assumptions: 中间步骤允许视觉弱化，但 Markdown 语义和 15px 正文字号必须保持。
- review point: 动态格式转换、历史段重渲染次数、长时间线、真实桌面成功和边界路径。
- validation result: failure-first 已确认；实现后完整前端 393/393、构建、治理、headless 与真实 Tauri WebView 验收通过。

## Stop Boundary

- Do not stop after local-only validation.
- Do not stop after deploy output without live verification.
- Stop only when done or explicitly blocked with evidence.
