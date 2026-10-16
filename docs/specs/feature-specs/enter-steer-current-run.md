# M2：Enter 引导与死回合消息恢复

## 已批准任务（原文）

## 问题
- 运行中的会话输入框提示「Enter 引导当前执行」，用户按 Enter 后，消息实际进了队列，当前回合并没有收到；输入框提示的模式与真实行为不一致。用户以为已经纠正了 agent，其实它没收到。

## 要做的
1. 先在真实应用（headless 验收页 + 真实 store）复现：一轮正在运行（包括模型流式输出中、工具调用中两种情况）时按 Enter，断言消息按提示的「引导」语义在当前回合的下一个安全点送达，并在会话里可见「已插入当前执行」；如果此刻确实只能排队（例如在不可中断的工具里），输入框提示与发送后的回执都必须如实写「当前步骤结束后读取」，不能写「引导当前执行」。
2. 统一界面与本地入口 send(steer) 的语义与回报，同一套状态来源。
3. 当前回合已经停摆（M80：next_observation 过期、无活跃 run）时，排队消息必须直接开启新一轮，不能挂在死回合后面。
4. 先写失败测试，再修；前端改动附 headless 截图（明暗两种主题）。

## 约束
不改 `.github/` 工作流、场景注册表与校验器；只用合成数据；其余同模板。

## Requirements Traceability
| Req ID | 要求 | 最低证据 |
| --- | --- | --- |
| M2-R1 | 安全点前诚实显示等待，确认后显示已插入 | composer/reducer 集成测试、真实浏览器明暗截图 |
| M2-R2 | GUI 与本地入口复用发送语义及回报 | dispatchSend 集成测试、Workspace 接线 |
| M2-R3 | 后端无运行所有权时启动新回合 | stale-run 回归测试 |
| M2-R4 | 测试先失败再修复 | 命令结果与 PR 自验收记录 |

## Applicable Harnesses
Spec、Compatibility、Observation、Viewport、AI Collaboration；Release 仅交付至合并，不发版。

## 测试矩阵
模型流式输出、工具执行等待、steer_applied 确认、无活跃 run 恢复、Enter 与修饰键排队、明暗主题与窄屏。

## Implementation Notes
使用现有 dispatchSend 作为 GUI 与本地派单的统一业务入口；不改变后端安全点和不可中断工具行为。真实安装版验收由编排方发版安装后完成。任务模板禁止 delegate_tasks，因此单执行流自验收。
