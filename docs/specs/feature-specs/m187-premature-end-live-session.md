# M187（P0）：正在工作的会话被判定结束（工具执行中 / 回合进行中）

## 证据
1. v1.91.49（含 M186 #900）编排方 liveness 验证会话 606c00c4（10-27 07:06）：用户「用 bash 执行 sleep 60; echo LIVENESS-DONE…」→ 07:06:16 assistant「我会执行这条命令…」并发起 bash（tool 状态 pending）→ 15 秒内（重启前）thin 已 ended，reason「无活跃执行者或未决等待」，无答案。v1.91.48 同脚本通过。
2. M182 会话 3a37beaf（10-26 23:11）与 944c64a0（10-27 04:07，v1.91.44）均在开工后几分钟、模型说「现在去读规格/读分词器代码」之后被 ended「任务已结束」，工作区无改动、无 PR。
3. M170 会话 b29ca719（10-26）首回合后 ended「无活跃执行者或未决等待」。
## 需求
- R1 查清是哪段逻辑把有活跃回合/在途工具调用/后台任务的会话结算为 ended（M186 结算扫描？derive_state 对 executor 注册的判断？回合间隙？），写入 PR。v1.91.49 的回归必须用 git bisect 或对照测试定位到具体提交。
- R2 不变量：存在任一 (a) 活的 executor/回合、(b) pending/running 工具调用、(c) 运行中后台任务、(d) 未完成的授权交付（工作区有改动或 PR 未合并）时，会话绝不能被结算为 ended；违反时记录告警并改为 running/waiting_wake。
- R3 M186 的「已合并会话结算」只作用于：PR 已合并/关闭 **且** 无 (a)(b)(c) **且** 工作区已无未推送改动的会话。同时完成 M186 未达成的部分：编排方列出的 14 个存量会话 + 其它「waiting_wake 且无 pending 唤醒」的已合并会话（v1.91.49 时 26 个）全部结算 ended。
- R4 测试（先失败，跨进程）：bash 长命令执行中 → 会话不被结束、命令完成后给出答案；回合间隙（模型思考中）不被结束；已合并且空闲的会话被结算；verify-liveness 等价断言进 PR hard gate（M157 smoke）。
- R5 真机验收：编排方装新版后跑 verify-liveness（重启前状态必须为 running）+ 查 14/26 个存量会话已 ended。
## 约束
- task-template；合成 fixture；授权交付（ceiling: merged）；release 前必须通过 R4 的跨进程 smoke。

Release-Urgency: immediate

## Implementation Notes
- 真实安装版验收与存量会话检查由编排方发版安装后完成；本任务不读取或修改生产会话数据库。
- Spec / Compatibility / Release / Observation / AI Collaboration Harness：以合成 SQLite、真实子进程、状态字段断言、全量项目测试与合并收尾为证据。不涉及 UI 布局修改。
