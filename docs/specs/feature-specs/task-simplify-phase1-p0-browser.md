# 阶段 1 P0-2：thin 会话里浏览器完全打不开（回执门禁漏改）

## 证据（编排方 10-19 09:2x，v1.89.6 真机）
- 会话 6539d4ca（delivery_mode=thin，只读任务「打开 example.com 读标题」）：两次 `browser_session open` 都返回 waiting「浏览器外部操作缺少完整持久化回执；系统已阻止执行并将自动核对。」(code=browser_observation_contract_required)，objective 停在 waiting_system/browser_observation_contract_required，Chrome 从未启动。
- 原因：`src-tauri/src/tools/browser_session.rs` 约 582–611 行：`ctx.db` 存在但 `ctx.outer_receipt_id` 为 None 时，非只读动作一律拒绝。thin 模式按阶段 1 R3 不写副作用回执，所以 outer_receipt_id 永远为 None → thin 会话的 open/click/type/navigate 全部被拦。v1.89.4（全 legacy）时浏览器可用。

## 需求
- R1 thin 会话：浏览器动作不要求持久化回执，直接执行（保持现有安全边界：受管一次性 profile、域名/凭据相关的权限审批、扩展通道与受管通道分离——见 docs/specs 中浏览器控制规格，不得削弱）。中断后的恢复走阶段 1 R3「读现场」提示（例如「上一步可能已打开/点击，先用 tabs/snapshot 确认」）。
- R2 legacy 会话行为不变。
- R3 测试（先失败）：thin 会话 open → 实际调用启动（可用现有 fake/fixture 浏览器）且不返回 browser_observation_contract_required；legacy 会话缺回执时仍按原逻辑；只读动作两种模式都可用。
- R4 顺带排查：thin 模式下是否还有其他工具在 `outer_receipt_id`/mutation_permit 为空时拒绝（grep `outer_receipt_id`、`mutation_permit`、`contract_required`），一并修并在 PR 列出清单。
- R5 编排方真机验收：新会话「打开 example.com 读标题」能拿到 "Example Domain"，且 `/usr/bin/log show` 中无 responsible=com.codefactory.app 的 SystemPolicyAppBundles 拒绝（M119 一并验证）。

## 约束
- 阶段 1 自身缺陷（P0）。合成 fixture，不写生产 DB。授权交付（ceiling: merged）。

## Implementation Notes
- 本文原文来自任务文件；任务未指定落盘文件名，使用与任务同名的 feature spec。
- 只对数据库确认 delivery_mode=thin 的会话跳过 durable authorizer；未知会话和读取失败仍保留 legacy 行为。
- 不改域名权限、profile、扩展只读规则、macOS spawn 或生产数据库。现有工具与 dispatch 路由继续支持后台操作。
- Applicable Harnesses：Spec、Compatibility、Observation、Release（仅合并，不发版）、Payload、AI Collaboration。无 UI 变更，不启用 Viewport。
- AI Collaboration：context scope 为任务、阶段 1、浏览器策略及回执入口；assumptions 为会话模式运行中不变；review point 为 legacy 保持与安全策略不变；validation result 随测试与 PR 记录。按任务模板不启动 delegate_tasks，单执行流。

## Requirements Traceability / 测试矩阵
| Req | 最小证据 |
| --- | --- |
| R1 / R3 | 合成 thin 会话通过 execute 打开本地 fixture 页面，断言标题和完成状态，关闭回收 |
| R2 / R3 | 合成 legacy、未知会话无回执返回原 waiting code；两种模式只读请求均越过回执门禁 |
| R4 | tools 及调用路由 grep 清单、thin recovery 回归 |
| R5 | 编排方发版安装后新会话读取 example.com 标题，并检查 tccd 日志与残留进程 |

## 发布后验收边界
本任务只授权合并，not live。真机验收由编排方发版后完成：读取 Example Domain，核对 SystemPolicyAppBundles 无 responsible=com.codefactory.app 拒绝，关闭会话后无本任务 Chrome 残留。不把合成 fixture 与本地测试描述为真实安装版验收。
