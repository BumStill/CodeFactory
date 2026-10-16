# M90：运行命令的真实计时

## 原始任务（逐字收录）

# 任务：「命令仍在运行（约 N 分钟）」的计时要真实（M90）

按 `/Users/leo/.codefactory/agent-tools/task-template.md` 执行（harness + SDD + TDD + 自验收 + §8 收尾；交付 ceiling: merged）。真机返修，证据见 `/Users/leo/.codefactory/agent-tools/ux-observations.md` 的 M90。

## 真机证据（v1.88.3，2026-10-15 11:38）
- 会话 88d24168、7d3e765f 都在跑 `cargo test`，rustc 已运行 26 分钟，但 chat_turn_state.recent_activity_label 显示「命令仍在运行（约 1–2 分钟）」。
- 推测：时长计时从最近一次心跳或输出算起，而不是从命令开始时算。
- 后果：用户和编排方都会把正在编译误判为卡住，或者反过来，把真卡住误判为正常。

## 要做的
1. 运行时长从命令真正开始的时间算起，持久化下来，重启后也不归零。用注入时钟的测试复现「同一条命令运行 26 分钟期间持续有心跳」的场景，断言显示约 26 分钟。
2. 超过某个阈值（例如 10 分钟）时，标签里附带正在运行的命令摘要，例如「cargo test 已运行 26 分钟」，让人一眼看出是在编译还是卡住。
3. 先写失败测试，再修。

## 约束
不改 `.github/` 工作流、场景注册表与校验器；只用合成数据；其余同模板。

## 标题
`fix(chat): the running-command label counts from when the command started`

## Requirements Traceability / 实施说明

| Req ID | 要求 | 最低证据 |
| --- | --- | --- |
| M90-R1 | 执行入口持久化起点，连续心跳及恢复读取不得覆盖 | 注入毫秒时钟推进 26 分钟；元数据往返；SQLite 重建持久化对象 |
| M90-R2 | 10 分钟起包含有界、安全的命令摘要 | 599/600 秒边界、26 分钟、Unicode 和秘密参数用例 |
| M90-R3 | 先失败再修复 | 首次测试失败输出及同命令通过 |

Applicable Harnesses：Spec、Compatibility（旧元数据缺省）、Observation（持久状态与标签断言）、Payload（摘要有界且不展示参数秘密）、Viewport（真实浏览器亮暗截图）、AI Collaboration（单执行流，模板禁止 delegate_tasks）。Release Harness 仅交付到合并，不在本任务发版。

主路径：运行 bash → 审批/钩子通过 → 保存实际执行起点 → 心跳读取同一起点 → 持久化 turn activity → 聊天标签展示。非 bash 工具保留通用标签。无新增操作入口，已有本地 dispatch 可读取 turn activity，锁屏下无需 GUI。

真机验收由编排方发版后完成：安装新版本，运行持续超过 10 分钟的合成命令，核对命令摘要与墙钟时长；运行期间重启应用并核对保留的起点；结束命令后确认标签进入完成状态。任务内不读取真实会话或生产参数。
