# 长任务无人参与测试系统：UX 与证据设计

## 用户主路径

测试把用户的实际交互方式作为正式主路径，而不是只覆盖“一条完整 prompt → 一次回复”：

1. 用户先给目标，例如“修好并走到正式交付”。
2. 系统开始长任务并持久记录 objective、checkpoint 和证据。
3. 用户在执行中补充验收边界、纠正方向或缩小/扩大授权范围。
4. provider、工具、CI、应用重启等可恢复故障由系统自行处理。
5. 用户偶尔说“继续/可以”时，它应精确绑定唯一开放任务；但系统正确恢复不能依赖这句话。
6. 只有真正缺少业务输入、授权被拒绝、不可逆高风险决定或显式取消时，系统才把行动交回用户。
7. 最终界面展示结果和证据链；未达到约定边界时明确显示仍在推进或真实 blocker。

## 用户可见状态契约

| Durable state | 用户看到什么 | 用户需要做什么 |
| --- | --- | --- |
| `active` | 正在执行的具体活动 | 无 |
| `waiting_system` | 系统正在恢复、观察或等待外部结果 | 无；始终可停止 |
| `waiting_core_input` | 一条精确问题，说明为何无法安全推断 | 回答缺失输入 |
| `waiting_authorization` | 具体新增权限及影响范围 | 允许或拒绝 |
| `waiting_business_decision` | 不可逆选项和权衡 | 做决定 |
| `completed` | 结果 + 已满足的证据边界 | 无 |
| `cancelled` | 已停止且重启不会恢复 | 无 |

禁止出现把技术恢复责任转给用户的 CTA，例如“重试”“重新发送”“请回复继续”。“继续/可以”只是一种自然语言 steer/确认，不是系统恢复按钮。

## 增量指令的验收

`CXD-001` 使用合成对话模拟真实工作方式：初始目标后依次到达“把正式产物也作为完成边界”“不要重复创建 PR”“可以，继续”。断言：

- 三条消息按到达顺序进入同一 session 的 interjection/continuation 轨迹；
- 已提交 receipt 和 canonical PR/head identity 不变；
- capability 只按明确指令改变，不因简短消息意外降为 review-only 或新任务；
- 已经越过 AgentLoop safe boundary、作为真实用户消息持久化的 steer 在重启后仍保留；
- 若同时存在多个开放 objective，则拒绝猜测并进入结构化 reconciliation。

当前边界：尚未被 AgentLoop drain 的 steer 仍位于进程内队列。它恰好在 safe
boundary 前遇到整机崩溃时不属于本批已证明能力；不能用本场景的单进程顺序断言
冒充跨进程持久性。把该窗口升级为 durable inbox 需要独立的 schema、claim/ack、
UI hydration 和 scheduler consumer contract，必须作为后续产品变更先写失败测试再实现。

## 交付证据阶梯

界面和 receipt 使用同一阶段模型：

```text
local change -> focused validation -> PR -> required CI -> merge
             -> release dispatch -> public artifact -> installed/live path
```

测试根据用户本次定义的 terminal boundary 决定完成位置。要求“修复”时可以停在合并；要求“发布”时必须到公开 artifact 和主路径验证。任一阶段失败都保留同一 DeliveryRun，不用新的 user message掩盖失败。

## 证据展示

每个自动场景保留：

- 场景 ID、固定 seed、binary version/build SHA；
- fault 注入点和“确实触发”证据；
- 重启前后 opaque identity 的相等断言；
- SQLite 状态摘要、receipt 计数、provider request digest；
- 文件/命令/远端结果的字段级 oracle；
- 是否出现 human prompt、是否残留 live owner；
- cleanup 结果。

日志默认不包含 prompt 原文、文件内容、绝对用户路径或 secret。失败 artifact 只保存合成 workspace 和脱敏状态摘要。

## 手工验证边界

Windows 安装、自动更新、WebView2、窗口恢复、停止按钮和重启电脑属于 release UX 边界，需要正式包上的实地验收；但手工验收不承担状态机正确性的主要证明。跨进程 identity、SQLite、receipt 和恢复上限必须先由自动 system test 拦截。
