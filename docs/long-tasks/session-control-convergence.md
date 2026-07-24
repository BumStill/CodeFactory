# 会话控制收敛长任务记录

## Basics

- Task ID: CF-SCC-20260723
- Title: Full access 意图解耦、完成门禁收敛与恢复过程可见性
- Feature spec: `docs/specs/feature-specs/session-control-convergence.md`
- Related Req IDs: CF-SCC-R1..R13

## Completion Standard

- Done means: CF-SCC-R1..R13 均有失败先行测试、真实 App 主路径、PR+CI 和精确发布产物证据。
- Blocked means: 同一阻塞有连续证据，且没有安全的本地、headless 或 GitHub runner 替代路径。

## Current State

- Current phase: CF-SCC-R13 本地修复和真实 Dev App 展示验收完成，等待 PR+CI；尚未进入发布产物，`not live`。
- Current checkpoint: 2026-07-24 现场会话已定位为跨段关键词误判；原始故障文本回归、全量前端/Rust 测试、生产构建和真实 Tauri Dev hydration 路径通过。
- Next owner: 当前修复分支完成 PR+CI；随后按刻意发版节奏进入下一批 release，并在具备独立 Dev provider 凭据时补录真实模型往返。
- Updated at: 2026-07-24

## Completed Items

- [x] 复盘真实 44 分钟会话：进程活跃但 Full access 强制 Execute，4 次 recovery 被 UI 隐藏。
- [x] 编写业务、架构、UX 设计与 Requirements Traceability。
- [x] 启动独立意图/门禁与前端可见性只读审查。
- [x] 失败先行覆盖 Full access 意图、累计 recovery、跨回合 replay、可见恢复和停止语义。
- [x] 实现 CF-SCC-R1..R11，并同步 Desktop OpenAI、Anthropic 与 Headless recovery 预算。
- [x] 前端 293 项测试、生产构建、Rust 定向测试和治理基线通过。
- [x] 真实 App 验证信任模式文案、DeepSeek v4 Flash 默认选择，以及恢复摘要的尺寸、层级、次数、原因和最近活动；内部 prompt、命令和参数未泄漏。
- [x] PR #183、PR CI 与 main CI 全绿后 squash 合并到 main。
- [x] 按刻意发版流程发布 v1.64.2；Windows executable smoke、macOS 安装产物 GUI 验收和公开 DMG 重新下载验收通过。
- [x] 复盘 2026-07-24 现场会话：正确分析答案完成后，`turn_notice` 因跨段拼接“请检查模型配置”和示例“GitHub token 不可用”而错误注入 `deliver_changes`。
- [x] CF-SCC-R13 失败先行：精确现场文本、同句示例、分析意图、明确交付意图和旧 `role=user` notice 均有 Rust 回归。
- [x] 自动事实纠偏只进入执行型回合；交付纠偏再经过局部事实单元 + 当前用户交付意图双门禁。`turn_notice` 改存 system 来源，旧脏数据也不会成为下一回合用户目标。
- [x] 合并前独立复审补齐行内引号/反引号/Markdown 表格排除、批准短语继承上一轮交付方案，以及不依赖本机 `gh` 的确定性正向测试；复审确认无剩余 P1。
- [x] 隔离 `CodeFactoryTurnNoticeDev` 真实 App hydration 验证：用户问题和正确助手答案保持正常层级，system `turn_notice` 显示为居中运行时提示，不伪装成用户气泡；测试会话、进程组和 1420 端口已清理。

## Remaining Items

- [ ] 在具备 provider 凭据的发布产物补测完整诊断响应和运行中停止操作；本机 Dev 不复制或迁移用户凭据。
- [ ] CF-SCC-R13 完成 PR+CI；按刻意发版节奏进入 release 后，用精确安装产物复验现场「待执行」分析路径。

## Blockers

- 本机 `com.codefactory.dev` keychain 没有 `codefactory.endpoint.deepseek`；真实 App 已确认 DeepSeek v4 Flash 路由，但 provider 请求在发送前以明确缺失凭据结束。自动化已覆盖 dispatch 与停止文案，完整 provider/停止主路径待发布产物补测。

## Evidence

- Local evidence: 2026-07-23 正式 App SQLite 中 15:00 后存在 4 条 `gate_recovery`、4 条 `rejected_candidate`、56 次工具调用；CodeFactory 进程与 provider 连接持续活跃。
- Test evidence: R13 失败先行后，`pnpm test` 68 files / 294 tests、`pnpm build`、`pnpm exec tsc --noEmit`、`pnpm test:rust:fast --lib` 477 passed / 6 ignored、治理基线和 `git diff --check` 通过。
- Real App evidence: `CodeFactoryDev` 中信任模式显示“减少确认，不改变分析/执行意图”；独立持久化 recovery 会话显示紧凑的“执行已中断 / 第 1/3 次 / 未收到最终答复 / 最近活动”状态卡，未显示内部英文 gate prompt、命令或参数。R13 隔离 Dev 会话显示原始用户问题、正确助手正文和居中 system 运行时提示三层来源；没有出现第二个用户气泡或“前 N 步”折叠。
- Release evidence: PR https://github.com/BumStill/CodeFactory/pull/183 merged；main CI run 29994718058 passed；Auto Release run 29995215300 生成 v1.64.2；Release run 29995236008 全绿；GitHub Latest 指向 https://github.com/BumStill/CodeFactory/releases/tag/v1.64.2，6 个资产与 `latest.json` 的 Windows/macOS 路由、签名均可公开下载。
- Blocking evidence: R13 真实 Dev App 调用 DeepSeek 时返回 HTTP 401；隔离 Dev 没有有效 provider 凭据，未复制或迁移正式 App 凭据。真实模型往返和发布安装产物仍待后续证据，当前修复保持 `not live`。

## AI Collaboration

- context scope: `commands/chat.rs` dispatch、AgentLoop completion finalization、事实纠偏、持久化来源、stream events、chat reducer/hydration、MessageList、输入取消语义。
- assumptions: Full access 只属于权限层；内部 prompt/草稿保持隐藏；事实纠偏不能覆盖当前用户目标；本切片不实现进程组立即强杀。
- review point: 既有两个独立只读审查核对 Rust 状态机与前端事件/持久化边界；R13 追加原始数据库取证、失败先行和真实 Tauri Dev 页面复验。本切片因上层约束未授权派生 sub-agent，使用单执行流。
- validation result: R1..R12 的失败先行、本地自动化、无凭据真实 App 路径、PR+main CI 和 v1.64.2 精确发布产物通过；R13 本地自动化与真实 Dev 展示通过，待 PR+CI、下一批发布产物和凭据化 provider 往返。

## Stop Boundary

- 不在设计或单元测试后停止。
- 不在 PR 创建、合并或 release workflow 启动后停止。
- 只有公开安装产物的真实会话路径验证通过，或有明确外部 blocker，才允许标记完成/live。
