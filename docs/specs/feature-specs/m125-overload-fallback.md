# M125：模型服务「过载」时长时间退避、不回落到可用的备用模型

## 证据（10-19 09:34，v1.89.6）
- 会话 83f43fab（thin，模型 gpt-6.1-sol）：09:34:45 系统消息「模型服务过载，正在自动退避恢复；无需回复"继续"」，之后 23 分钟以上无任何进展；同期 DeepSeek（当时在闲时窗口外？需核实）/OpenRouter 状态未被尝试。最终会话停滞、需编排方介入。
## 需求
- R1 先查 model_route_attempts 与路由代码，写清当前「过载（503/529/overloaded）」的处理：退避上限、是否会切换到同一会话允许的其他路由。
- R2 过载属于可恢复但「换一条路更快」的错误：同一路由连续过载 2 次（或退避累计 >2 分钟）后，若存在当前可用的备用路由（考虑忙闲时窗口、额度、凭据），切到备用路由继续；没有可用备用路由时按 M123 的「等到可用时间」语义，界面说清在等什么、预计多久。
- R3 不改变用户显式「固定模型」的会话语义：固定模型时不切换，只说明在等什么。
- R4 测试（先失败）：主路由过载×2 + 备用可用 → 第三次请求走备用并成功；固定模型 → 不切换且有说明；无备用 → 等待且不计失败。
## 约束
- 合成 fixture，不写生产 DB。授权交付（ceiling: merged）。

## Implementation Notes
- Harness：Spec、Compatibility、Observation、Release、AI Collaboration。修改后端路由，不修改布局；安装版验收由编排方发版后完成。
- R1：`model_route_attempts` 是回合结束后由 `persist_model_route_attempts` 写入的路由汇总；逐次真实 POST 写入 `provider_route_attempts`。本任务只使用内存 SQLite 合成 fixture，不读取或写入生产会话数据库，无法据此核实历史真实忙闲时状态。
- 修复前：`RoutedDesktopModelTransport::complete` 在有备用时第一次过载就切换；`has_available_fallback` 只检查索引、失败集合、健康冷却，不检查时段、额度和凭据；无备用时 `overload_budget_decision` 在第 1/2 次给出 20/40 秒退避，第 3 次登记 60 秒后再观察；共享 loop 的退避为 20/40 秒，两次重试后返回持久恢复。`record_current_failure` 会把暂时过载标记为不可用，thin 下一轮准入因此可能没有可选择的候选且没有恢复原因，陷入短周期等待。
- 固定模型的候选计划只有单路由，不能越过该计划寻找备用。保留现有副作用安全边界，不重放已开始的工具调用。
- AI Collaboration：context scope 为路由准入、持久 Provider 恢复与合成 HTTP 测试；assumptions 为既有 supervisor 持久等待负责自动续跑；review point 为固定策略、时段/额度/凭据过滤及副作用安全；validation result 随验收记录更新。按任务模板要求不调用 delegate_tasks，使用单执行流。
