# 更新安全点误阻塞修复长任务记录

## Basics

- Task ID: CF-UPDATER-SAFETY-20260818
- Title: 无活跃会话时更新仍被 durable wait 永久阻塞
- Feature spec: `docs/specs/feature-specs/durable-delivery-recovery.md`
- Related Req IDs: CF-DR-R14、CF-DR-R15、CF-DR-R16、CF-DR-R17

## Completion Standard

- Done means: failure-first 测试、Rust/前端/build/治理门禁、真实 Dev UI 成功与边界路径、PR/CI/merge、正式 release、固定版安装验证，以及由固定版控制的一次真实 N→N+1 自动更新证据全部成立。
- Blocked means: 缺少可签名的下一版本资产或外部发布门禁，且已完成所有不依赖该门禁的本地、CI、release artifact 与安装版验证。

## Current State

- Current phase: PR/CI/Release 交付准备
- Current checkpoint: 完整前端/Rust/build/治理门禁与 CodeFactoryDev 成功、边界 UI 路径均已通过
- Next owner: 开发完成分支同步与 PR，QA/CI 独立验收后进入 merge/release
- Updated at: 2026-08-18

## Completed Items

- 正式版已先更新到 v1.81.12，并验证签名、SHA、进程与窗口。
- 生产 DB 只读确认：两个 `waiting_core_input + technical_recovery_exhausted` 目标均无 live lease，且无 active chat/delivery owner。
- failure-first 复现 durable wait 被误算 blocker，以及第五次安全点 claim 后错误耗尽。
- 修复仅以 live lease 作为 Objective restart blocker，并保留其他进程内 owner 与不确定 receipt 的 fail-closed 门禁。
- 增加同 target rearm 与新 target auditable supersede 兼容路径，不修改生产 DB。
- 前端完整测试 628 项、Rust fast suite 1115 项（另 7 项按既有配置 ignored）、provider/no-window suites、生产构建与治理门禁全部通过。
- CodeFactoryDev 真实界面验证普通安全等待与 `observe_only` 边界文案；wrapper 日志确认运行本 worktree，验证后已退出并恢复主 checkout 指针。

## Remaining Items

- PR、CI、merge、刻意发版、公开资产与固定版安装验证。
- 在后续真实版本存在时完成由固定版控制的 N→N+1 自动更新闭环。

## Blockers

- 当前无本地实现 blocker；N→N+1 最终证据依赖固定版之后存在一个真实签名更新版本。

## Evidence

- Local evidence: 新增 Rust failure-first/兼容测试由红转绿；前端 114 个文件 628 项、Rust fast suite 1115 项、no-window/provider recovery suites、`pnpm build`、long-task validator、governance baseline 与 `git diff --check` 全部通过；正式 DB 仅使用 read-only/query-only 聚合。
- UI evidence: CodeFactoryDev 普通等待显示“更新已排队，正在等待安全安装”，并明确结束后才下载/安装/重启；`observe_only` 显示仅核对上次安装结果且不会重放未知结果。
- Release evidence: 待 PR/CI/merge/release 后补齐。
- Blocking evidence: Dev 模式明确跳过 updater，不能替代签名发布版的自动更新证据。

## AI Collaboration

- context scope: update safety、Objective/remediation、Update adapter、updater store 与状态 UI；不读取聊天正文。
- assumptions: 无 live runtime owner 且无未过期租约的 durable wait 可安全跨重启；unknown receipt 继续 fail closed。
- review point: 独立规划与 QA 均确认仅过滤 blocker 不足，必须同时恢复历史 exhausted update Objective。
- validation result: 完整本地测试、构建、治理与真实 Dev UI 验收已通过；CI、release artifact、固定版安装与后续 N→N+1 live 验收待完成。

## Stop Boundary

- 不在本地测试、绿色 CI、已合并或 release asset 任一单点停止。
- 无下一真实签名版本时，必须明确标记 N→N+1 `not live verified`，不得伪造自动更新完成。
