# 更新安全点误阻塞修复长任务记录

## Basics

- Task ID: CF-UPDATER-SAFETY-20260818
- Title: 无活跃会话时更新仍被 durable wait 永久阻塞
- Feature spec: `docs/specs/feature-specs/durable-delivery-recovery.md`
- Related Req IDs: CF-DR-R14、CF-DR-R15、CF-DR-R16、CF-DR-R17、CF-DR-R18、CF-DR-R19、CF-DR-R20

## Completion Standard

- Done means: failure-first 测试、Rust/前端/build/治理门禁、真实 Dev UI 成功与边界路径、PR/CI/merge、正式 release、固定版安装验证，以及由固定版控制的一次真实 N→N+1 自动更新证据全部成立。
- Blocked means: 缺少可签名的下一版本资产或外部发布门禁，且已完成所有不依赖该门禁的本地、CI、release artifact 与安装版验证。

## Current State

- Current phase: v1.81.21 live 回归的 PR/CI/Release 交付准备
- Current checkpoint: 最小 SQL 修复与 failure-first 回归已转绿；Rust 全量、updater 前端契约、生产 build、治理与长任务门禁全部通过
- Next owner: 提交 PR，等待 CI 独立验收后进入 merge/release
- Updated at: 2026-08-20

## Completed Items

- 正式版已先更新到 v1.81.12，并验证签名、SHA、进程与窗口。
- 生产 DB 只读确认：两个 `waiting_core_input + technical_recovery_exhausted` 目标均无 live lease，且无 active chat/delivery owner。
- failure-first 复现 durable wait 被误算 blocker，以及第五次安全点 claim 后错误耗尽。
- 修复仅以 live lease 作为 Objective restart blocker，并保留其他进程内 owner 与不确定 receipt 的 fail-closed 门禁。
- 增加同 target rearm 与新 target auditable supersede 兼容路径，不修改生产 DB。
- 前端完整测试 628 项、Rust fast suite 1115 项（另 7 项按既有配置 ignored）、provider/no-window suites、生产构建与治理门禁全部通过。
- CodeFactoryDev 真实界面验证普通安全等待与 `observe_only` 边界文案；wrapper 日志确认运行本 worktree，验证后已退出并恢复主 checkout 指针。
- 2026-08-19 Windows 反馈证实两个新缺口：backend 实际下载/安装时 renderer 仍显示“已排队/0 项运行”；更新重启夹在 Objective 终态与 task/turn 投影之间时，启动恢复会把完成项短暂重置为等待。
- 已实现 backend 字节进度事件、零 blocker 真实文案、Update/Objective claim 共享入场门、journal/task completion 同事务、终态 Objective 启动自愈、`plan_resume` 终态保护与迟到 Chat 投影 CAS。
- 新增回归均先证明旧逻辑失败，再转绿；前端 115 个文件/662 项、Rust 主库 1128 项（另 7 项 ignored）、no-window 2 项、provider recovery 13 项、生产 build、治理基线与 `git diff --check` 全部通过。
- 2026-08-20 在 v1.81.19 正式运行时点击 v1.81.21“立即安装”后，界面持续显示 1 项本地执行；正式 DB `mode=ro + query_only + quick_check=ok` 排除 chat/task/browser/permission/Objective owner，唯一 lease 为 `takeover_reconciliation + external_state_uncertain + observe_only_reconcile`，且 `claim_epoch=708 > reconciled_claim_epoch=2`，证明它没有 mutation permit。
- 使用 v1.81.21 官方 `CodeFactory_aarch64.app.tar.gz`，核对 GitHub 发布 SHA-256 与包内版本后完成手动安装；旧 v1.81.19 App 已保留为废纸篓可恢复副本，新进程和主窗口验证通过。
- 新增失败优先 Rust fixture 同时放入 mutation-capable 与 observe-only live lease，旧实现稳定得到 `left: 2 / right: 1`。

## Remaining Items

- PR、CI、merge、紧急切版、公开资产与固定版安装验证。
- 在后续真实版本存在时完成由固定版控制的 N→N+1 自动更新闭环，并复测完成任务不会在重启后降级为 Waiting。

## Blockers

- 当前无本地实现 blocker；最终 N→N+1 证据依赖固定版之后存在一个真实签名更新版本。

## Evidence

- Local evidence: 新增 failure-first/兼容测试由红转绿；2026-08-20 新回归的旧实现失败证据为 delivery blocker `left: 2 / right: 1`；修复后 Rust 主库 1139 项通过（另 7 项 ignored）、no-window 2 项、provider recovery 13 项、updater 前端 13 项、`pnpm build`、本次 Rust 文件 `rustfmt --check`、governance/scenario/long-task validators 与 `git diff --check` 全部通过。正式 DB 仅使用 read-only/query-only 聚合，并回放得到 live lease 1、mutation-capable 0、observe-only 1。
- UI evidence: CodeFactoryDev 普通等待显示“更新已排队，正在等待安全安装”，并明确结束后才下载/安装/重启；`observe_only` 显示仅核对上次安装结果且不会重放未知结果。
- Release evidence: v1.81.21 官方 macOS tar.gz SHA-256 `90e7af8c897c119e4e6b0cce428ad61ef7c8a6f30672b67f526ba9cffb481c3c` 与 GitHub 发布资产一致；本机运行版本已升至 v1.81.21。新修复待 PR/CI/merge/release 后补齐。
- Blocking evidence: Dev 模式明确跳过 updater，不能替代签名发布版的自动更新证据。

## AI Collaboration

- context scope: update safety、Objective/remediation、Update adapter、updater store 与状态 UI；不读取聊天正文。
- assumptions: 没有 mutation permit 的 takeover observer 可安全跨重启；只有 `claim_epoch > 0` 且 `claim_epoch = reconciled_claim_epoch` 的未终态 live Delivery lease 才能授权 mutation 并阻塞更新；unknown receipt 继续 fail closed。
- review point: 独立规划与 QA 均确认仅过滤 blocker 不足，必须同时恢复历史 exhausted update Objective。
- validation result: 完整本地测试、构建、治理验收已通过；仓库级 `cargo fmt --check` 仍暴露 main 已存在且不属于本分支的大面积格式漂移，本次修改文件单独检查通过。Dev 模式明确跳过 updater，无法伪造签名更新的实机证据；CI、release artifact、固定版安装与后续 N→N+1 live 验收待完成。

## Stop Boundary

- 不在本地测试、绿色 CI、已合并或 release asset 任一单点停止。
- 无下一真实签名版本时，必须明确标记 N→N+1 `not live verified`，不得伪造自动更新完成。
