# 内联解释器审批边界修复

## Basics
- Task ID: CF-IAB-20261108
- Title: 内联解释器未知代码审批回退
- Feature spec: `docs/specs/feature-specs/interpreter-approval-boundary.md`
- Related Req IDs: CF-IAB-R1..R5

## Completion Standard
- Done means: 失败先行、生产修复、完整准入与八个集成目标回归、独立审查、草稿 PR 精确 head 的必需 CI 通过。
- Blocked means: 有确切证据的权限或环境限制，且不存在已授权替代路径。

## Current State
- Current phase: 本地修复、回归与独立审查完成，待草稿 PR 精确 head CI，not live。
- Current checkpoint: 基线 `8789db79`，独立分支 `fix/interpreter-approval-boundary`，原 PR #1006 不动。
- Next owner: 当前任务继续实现、验证与草稿 PR；合并及发布需后续授权。
- Updated at: 2026-11-08

## Completed Items
- [x] 核实最新 main、打开 PR 和独立 worktree；无并行安全修复 PR。
- [x] 读取实际权限路径与既有规格；独立审查确认未知代码不可进入沙箱快捷 Allow。
- [x] 生产 shell 分类先红：argv 子进程载荷错误返回 Allow Low；载荷未执行。

## Remaining Items
- [x] 有界正向证明、未知解释器审批回退及显式 deny 优先级。
- [x] 真实 Gateway/AgentLoop 与计数工具后端测试：7 个入口、33 条载荷输入，三模式及自定义规则矩阵；权限相关全量 80/80 通过。
- [x] 八个集成目标、相关权限全量与治理/场景结构检查；提交后的差异门禁继续核验。
- [ ] 独立安全及兼容审查、草稿 PR 精确 head CI。

## Blockers
- 当前无外部授权阻塞。原 Mac 不可用，不依赖其本地证据。

## Evidence
- 初始本地统一场景门禁：`scenario-harness-gate: OK`。
- 独立 QA 复查输入环境、动态入口、显式 deny、沙箱快捷路径与解析资源上限，最终无已知代码阻断；未执行危险载荷。
- 分类先红：`unknown_inline_code_never_receives_low_risk_allow` 对生产源码失败；仅传入字符串。
- 编译测试沿用共享 Cargo 缓存与已有 Linux 测试容器；不声称该容器提供不可信代码硬隔离。

本地集成入口结果（各入口包含共享源码回归，不将重复包含的测试加总为独立用例）：

| 目标 | 通过 / 失败 |
| --- | --- |
| m159_trusted_git | 60 / 0 |
| m177_trusted_local_git | 59 / 0 |
| m180_readonly_still_prompts | 57 / 0 |
| m182_trusted_git_denylist | 60 / 0 |
| m198_trusted_heredoc | 63 / 0 |
| m207_trusted_git_composition | 57 / 0 |
| m248_trusted_git_sync | 58 / 0 |
| m248b_trusted_real_commands | 59 / 0 |

两处旧 Node 自动放行断言曾按新边界失败；原输入保留为审批断言，新增字面量输出对照后上述完整回归通过。

## AI Collaboration
- context scope: shell 风险分类、trusted Git 证明、权限 Gateway 与 AgentLoop 派发。
- assumptions: 内联静态证明不认证解释器二进制、不替代 OS 沙箱；脚本文件模式保留原策略。
- review point: 独立安全审查；实现与完整准入测试由不同角色完成。
- validation result: 多轮分类/完整准入失败先行后，独立分类 22/22、权限相关全量 80/80 通过；八个集成目标全部通过，PR 精确 head CI 待完成。

## Stop Boundary
- 已授权范围内持续到草稿 PR 和精确 head CI 结论；不合并、不自动排队、不发布，不撤销已有发布。
