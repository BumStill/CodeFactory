# M207：信任模式 git 证明器比普通命令更严（M198→M204 第三次复发，改设计不加词）

Release-Urgency: immediate

## 证据（v1.91.63，10-30 04:2x，会话 05638da0，信任模式）
M204 上线后仍被 `trusted.git.unproved` 拦：
```
git diff --check && python3.11 tools/governance/run_scenario_harness_gate.py --stage local ...; grep -n -E '...' '/Users/leo/Library/Application Support/com.codefactory.app/background-tasks/bg-….log' | tail -12; git config core.hooksPath .githooks; git fetch --prune origin main; git merge-base --is-ancestor origin/main HEAD
```
每段都是只读或仓库规则要求的动作。M204 根因写明：证明器对「非 Git 段」只接受只读 filter / 工作区脚本白名单，其余整串 return None。这正是问题本身：**一串命令里只要含 git，非 git 段就要过一套比单独执行时更严的白名单**。同一条 `grep` 单独执行在信任模式下不弹，和 git 串在一起就弹。

## 设计要求（不再扩白名单）
1. 复合命令切段后：git 段走 git 证明（只读 git 子命令——status/log/diff/show/rev-parse/merge-base/stash list/branch --list/config 读写本仓库 core.hooksPath 等——直接放行；改动型 git 只要求影响本会话工作区+可备份）；**非 git 段走与它单独执行时完全相同的信任模式判定**。整串结论 = 各段结论的最严者，但任何一段都不得比单独执行更严。
2. 读取应用自身目录（background-tasks 日志等应用写给 agent 看的文件）属于只读，按单独执行规则处理。
3. 表驱动夹具：把 M198、M204 以及本单证据里的所有命令，逐段单独判定一次、整串判定一次，断言「整串结论 == 各段单独结论的最严者」。这个性质测试要对随机组合（至少 200 组）成立。
4. 不放宽真正危险的：push 非本会话分支、reset --hard 非本会话产物、删他人分支、执行型 remote helper 仍拦。

## 验收
真机信任模式会话原样执行本证据命令不产生 permission_intents；装机后 1 小时内信任模式 `trusted.git.unproved` 审批为 0。

## Implementation Notes

原始任务未指定文件名及 Req ID；在此原样持久化，按设计要求编号映射 M207-R1～R4，不改变语义。测试夹具仅用合成路径、会话和仓库；真实应用验收由编排方发版安装后完成。

| Req ID | 要求 | 最低证据 |
| --- | --- | --- |
| M207-R1 | 复用单段信任判定、组合取最严 | 网关表驱动与至少 200 组确定种子随机组合 |
| M207-R2 | 应用日志只读 | 合成应用目录绝对路径 grep/tail 与组合对照 |
| M207-R3 | M198/M204 家族 | 现有夹具逐段/组合与 gateway SQLite intent 断言 |
| M207-R4 | 危险 Git 不放宽 | 非会话 push、远端 reset、删他人分支、remote helper 反例 |

Applicable Harnesses：Spec、Compatibility、Payload、Observation、Release、AI Collaboration；不改 UI，无 Viewport 变更。沿用现有 headless 工具与网关入口，后台可操作。

真机步骤：编排方发版安装后，在 trusted 会话执行原始复合命令，核对 permission_intents 无新增；随后观察一小时内 trusted.git.unproved 审批为零。此任务仅授权合并，不执行发版或声称 live。
