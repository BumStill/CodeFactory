# Objective 受管工作区长任务记录

## Basics
- Task ID: CF-MOW-20260821
- Title: 主会话 Objective 受管 worktree 与 delivery 身份闭环
- Feature spec: `docs/specs/feature-specs/managed-objective-workspaces.md`
- Related Req IDs: CF-MOW-R1..R10, CF-ORC-R2, CF-ORC-R6, CF-ORC-R29

## Completion Standard
- Done means: 规格反向追踪、失败先行测试、迁移、主会话分配/重附着、delivery fail-closed、相关测试与治理、CodeFactoryDev 成功/边界路径、PR/CI/merge、release artifact 与安装版复验成立。
- Blocked means: 缺少不可替代凭据/平台能力，或三种安全实现路径均不能满足零根目录副作用；必须附可复现实证。

## Current State
- Current phase: P0 实现与本地验证
- Current checkpoint: Objective workspace manager、chat/recovery/tool/delivery fence、跨进程 repo lease 与 UI 投影已实现
- Next owner: development -> independent QA -> release ops
- Updated at: 2026-08-21

## Completed Items
- 生产 DB、Git reflog、PR #334/#411 与 delivery run 已只读归因。
- 业务、架构、UX 与 feature spec 已定义。
- real-git/SQLite 测试覆盖 dirty root、双 Objective、重启、allocation 中断、legacy grandfather、identity incident 与跨进程 lease。
- 主 AgentLoop/checkpoint/delivery 已统一使用 Objective `effective_cwd`；subagent 由该 cwd 派生，因此 merge-back 落到 Objective worktree。
- UI 显示受管 worktree/incident，并让 Git 与 delivery 面板使用受管路径。
- delivery 原生 fetch + ancestor gate 已覆盖正常 PrOnly 与 stale-base 零副作用路径。

## Remaining Items
- 运行治理校验、完整相关回归与真实 CodeFactoryDev 成功/边界路径。
- PR、CI、merge 与适用 release。
- 后续 slice：canonical PR 合并后的自动安全 closeout；在该 slice 合并前不宣称完整 workspace lifecycle 已完成。

## Blockers
- None

## Evidence
- Local evidence: `execution_workspace` 8/8；`primary_git_mutation` 2/2；stale/normal delivery 2/2；Workspace UI 16/16；TypeScript 通过。
- Release evidence: 待补 canonical PR、required checks、merge SHA、tag/artifact/installed build。
- Blocking evidence: None

## AI Collaboration
- context scope: current origin/main, chat admission, agent loop cwd, tool backend, delivery identity, Objective schema
- assumptions: Git 代码任务必须隔离；非 Git 目录保持现有本地执行；任何 identity 冲突 fail closed
- review point: 独立架构/QA sub-agent 只读复核 Req 与测试矩阵
- validation result: architecture review completed; local focused tests green; live/release pending

## Stop Boundary
- Do not stop after local-only validation.
- Do not stop after PR/CI without merge and applicable release proof.
- Stop only when done or explicitly blocked with evidence.
