# CF-BG 落地：agent 实际不用后台任务（v1.89.0+ 真机 background_tasks 表 0 行）+ 阶段 3 界面

## 现象
- v1.89.0 起（#728 阶段 1、#735 阶段 2 已上线），真机上会话 273a55b2（M117）交付后没有创建任何后台任务或定时唤醒：deliver_changes 在 18:04 被调用后一直挂着等 CI（pending→waiting→error），之后由系统的 canonical_pr_progress 观察推进。`background_tasks` 表总行数 0。即 R5「交付后用后台任务监视 CI」「结束回合前必须挂好唤醒」在真机上没有生效。
- 总规格：/Users/leo/.codefactory/agent-tools/task-cf-bg-background-tasks.md（R5、R5b、R6）。

## 需求
- R1 先查清原因并写进规格：后台任务/定时唤醒工具是否真的出现在模型可用工具列表里（各模型端点：chatgpt/deepseek/openrouter）；系统提示里 R5 规矩是否存在、措辞是否足以让模型采用；deliver_changes 自己阻塞等 CI 是否与 R5 冲突（交付工具不应长时间阻塞，应在推送/开 PR 后返回，由 agent 挂后台监视）。
- R2 修正使规矩真实生效：交付工具在 PR 打开/推送后尽快返回明确的「CI 进行中」结果；agent 被系统提示要求以后台任务监视 CI（给出推荐命令形状，间隔 ≥180s，注意 M114 额度）；完成门禁在「外部进行中但无唤醒源」时不放行并提示挂监视。
- R3 阶段 3 界面（R6）：会话里显示「N 个后台任务进行中」小标记，可展开看命令/来源/状态并可停止；不把唤醒/控制回路写进消息流。
- R4 验收（真机形状的合成 E2E）：一个交付任务开 PR 后，agent 自主创建后台监视任务，CI 结束时被唤醒并继续；用户说「盯着 PR #N，合并了告诉我」时创建任务并在合并后回复。background_tasks 表有对应行。
## 约束
- 不放宽权限与门禁；合成 fixture，不写生产 DB。

## Implementation Notes

- 当前 DesktopToolBackend::list_schemas 返回 tools::all_definitions，后台任务与定时唤醒已注册；三个模式系统提示已有 R5，但 tools/delivery.rs 的 schema 明确要求用 through_ci_green 阻塞等待，与 R5 相冲突。
- 实际阻塞点为 agent/delivery.rs::wait_for_ci；采用单次检查后返回持久 waiting，保留交付授权与 lease，交还 agent 自主选择后台监视。
- 完成门禁已查询 delivery_runs 与本会话唤醒源；本次核对真实阶段枚举与 agent 调用接线，不新增隐式授权。
- UI 管理复用现有会话隔离 list/stop/cancel 核心；后台操作原有 agent 工具继续可在锁屏时调用。
- Harness：Spec、Compatibility、Observation、Payload、Viewport、AI Collaboration；不发版，真机验收由编排方发版后完成。

## Requirements Traceability / 测试矩阵

| Req | 可执行验收 |
| --- | --- |
| R1 | 各 provider 工具列表/系统提示与交付 schema 无阻塞冲突 |
| R2 | CI pending 单次检查即返回 waiting；无唤醒源拒绝完成；有唤醒源允许交还 |
| R3 | 组件数量、展开命令/来源/状态、停止、会话切换隔离；headless 明暗与窄屏 |
| R4 | 合成数据库 + 真实后台进程结束唤醒 + 两种 source 落库；交付/用户要求监视闭环 |
