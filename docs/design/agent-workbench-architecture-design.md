# 现代 Agent Workbench 架构设计

## 1. 架构原则

1. **真实数据源不合并**：统一展示契约，不制造统一后端状态。
2. **语义优先于颜色**：状态由 `tone + icon + label + detail + action` 共同表达。
3. **正文优先**：会话是主表面，工具、任务和交付按需展开。
4. **渐进迁移**：首批只调整展示与文案，不修改会话、任务、Git 或用量持久化 schema。
5. **真实 surface 验证**：测试组件必须与 Workspace 实际挂载的组件一致。

## 2. 真相源与展示契约

| 展示域 | 真相源 | 用户问题 | 禁止推断 |
| --- | --- | --- | --- |
| Current turn | `plan_updated`、`turn_activity_updated` | 当前做到哪一步 | 无 plan 时伪造百分比/ETA |
| Background jobs | `useTasksStore`、scheduler events | 后台还有什么、谁失败 | 从聊天文案猜任务状态 |
| Delivery | PR head checks、merged 字段、release compare、live verifier | 代码交付到哪 | release 推断 live |
| Queue | chat queue | 用户下一条指令何时发送 | 把 queue 当后台作业 |
| Context | runtime context usage、usage DB | 本轮资源是否健康 | 把 context 百分比当任务进度 |

统一展示层使用：

```ts
type WorkbenchTone =
  | "neutral"
  | "progress"
  | "success"
  | "warning"
  | "danger"
  | "info";

interface WorkbenchStatusPresentation {
  tone: WorkbenchTone;
  label: string;
  detail?: string;
  nextAction?: string;
}
```

首批实现可以在各组件内通过纯函数映射；第二批再抽取共享 `StatusPill`、`StatusLine`、`StatusIcon` 和 `ProgressTrack`，不让共享组件反向成为业务状态源。

## 3. 状态语义

| tone | 含义 | 典型对象 |
| --- | --- | --- |
| neutral | 正常信息、待开始、未关联 | context 正常区、无 PR、pending |
| progress | 正在推进、尚未达到终态 | running、CI pending、PR open、release 已创建但未 live |
| success | 对该对象定义的终态已验证 | plan 全完成且无失败、测试通过、live verifier 通过 |
| warning | 可恢复、需要观察或用户动作 | context 75–89%、等待权限、可修复失败 |
| danger | 阻塞、失败或资源临界 | 不可修复失败、CI failure、context ≥90% |
| info | 中性结构化通知 | 模型切换、路由接管、已压缩上下文 |

状态不得只靠颜色；必须同时有图标或形状和文本。绿色只用于“当前对象的成功终态”，不能表示普通资源余量、PR open 或 release artifact。

## 4. 主题与 surface token

CSS 变量继续作为主题权威，新增：

- surface：`canvas`、`pane`、`raised`、`subtle`、`overlay`；
- semantic：`status-progress`、`status-success`、`status-warning`、`status-danger`、`status-info` 及 soft background；
- focus、selection、shadow 保持主题可读。

现有 `surface-0..4` 暂时保留兼容，重新校准为可辨认层级：

```text
canvas -> 主工作区背景
pane   -> 顶栏、侧栏、composer 外层
raised -> 输入框、卡片、菜单、抽屉
subtle -> hover、选中、内嵌证据
overlay-> modal / floating surface
```

Tailwind 继续从 CSS var 读取颜色，避免 light/dark 分支散落在组件中。

## 5. 组件拓扑

```text
WorkspacePage
├─ WorkspaceHeader
│  ├─ session identity
│  ├─ local git
│  ├─ delivery status
│  └─ task activity trigger
├─ SessionSidebar
├─ MessageList
│  ├─ CurrentTurnProgress
│  ├─ conversation reading column
│  ├─ tool activity
│  └─ TurnResultSnapshot (answer footer)
├─ ComposerSurface
│  ├─ queue / draft scope
│  ├─ MessageInput
│  └─ ComposerControlBar
│     ├─ ModelPicker / ReasoningEffortPicker
│     ├─ PermissionModePicker
│     └─ ContextUsageRing
└─ AuxiliaryPaneArbiter
   ├─ Task activity
   ├─ Git / delivery / evidence
   └─ Document / on-demand browser
```

运行策略控件继续读取各自现有 store，不引入新的持久化 schema。`ContextUsageBar` 保持当前 context 与累计 usage 两个独立数据源：圆环只表达 runtime context 百分比；会话/今日累计 Token 只在详情中展示。详情通过回调进入完整用量页，不自行持有全局路由。

## 6. 结果状态映射

结果快照根据结构化 plan 和工具证据映射：

```text
complete && failureCount == 0 && !waitingReason -> success / 已完成
complete && failureCount > 0 && !waitingReason  -> warning / 已执行，证据待复核
nextActionOwner == user                         -> warning / 需要你处理
waitingReason && nextActionOwner != user        -> progress / 系统继续处理（或外部等待）
otherwise                                       -> neutral / 未完成
```

该映射只描述“本回合证据状态”，不推断交付 live。`waitingHistory` 仅作为历史证据；`waitingReason` 是原因说明，不得作为责任人真相源。只有结构化 `nextActionOwner: user`（例如核心输入、权限或业务裁决）才能产生用户待办；旧数据缺失 owner 时按非用户责任降级，不能从自由文本猜测。

## 7. 交付状态映射

`WorkspaceDeliveryStatus` 的最远阶段依次为：

```text
unavailable -> remote unavailable
no PR       -> not linked
PR open     -> review in progress
CI success  -> CI verified
merged      -> merged
release     -> release artifact created, live unverified
live        -> only when an explicit live-verifier field exists and passes
```

当前 `WorkspaceDeliverySnapshot` 没有 live 字段，因此即使存在 release，也必须显示“未验证上线”。后续如增加 `live_verified`，必须来自持久化 live verifier 证据，不得从 tag、HTTP 200 或部署命令成功推断。

## 8. 右侧 pane 仲裁

当前任务抽屉、Git/交付抽屉与 browser/document pane 都可能占据右侧。本次发布引入：

```ts
type AuxiliaryPane =
  | { kind: "task"; taskId?: string }
  | { kind: "git"; view: "changes" | "history" | "remote" }
  | { kind: "delivery" }
  | { kind: "evidence"; turnId: string }
  | { kind: "browser"; sessionId: string }
  | { kind: "document"; documentId: string }
  | null;
```

规则：同一时刻最多一个右侧辅助面；切换前保存各自 selection/scroll state；需要阻塞用户动作的权限 dialog 仍是 modal，不进入 pane。≥1440px 采用 docked pane，状态类 pane 默认 360–420px，浏览器/文档默认 38vw 并限制在 480–720px；1024–1439px 采用 drawer，<1024px 采用全高 overlay。用户可调整浏览器/文档宽度，separator 支持指针和键盘。无选择、加载失败或浏览器未得到 URL 时必须显示明确加载/错误态，已成功加载且为空时自动收起，不能渲染无标题白色区域。

本批 browser child WebView 只按 lease 初始 `pane_url` 提供同 URL 独立预览；Agent 工具仍操作另一 `LOCAL` ChromiumDriver 上下文。两者不共享 Cookie、DOM、导航或控制权，所以不得将 pane 解释为 Agent 页面实时镜像或用户接管面。完整 EBP-R3/R9 继续标记 `not live`，直至同一浏览上下文或可信状态桥及真实 App 证据闭环。

## 9. 顶栏与 composer 控制边界

- 顶栏负责会话身份、本地工程、交付、后台作业与设置；正常状态图标优先，当前阶段/异常保留短文字。
- composer 负责下一回合的模型、思考强度和权限；`MessageInput` 持有唯一 utility-toolbar slot，Workspace 按草稿/活跃会话注入控制，不再在输入框外平行渲染 scope、shortcut 或 runtime footer。
- `ModelPicker` 是运行策略的单一入口，思考强度在其面板内读取当前会话 endpoint/model capability；模型在运行中变更只影响下一回合，权限说明沿用现有会话作用域。
- 展示层按风险渐进披露，但不改变 store：单一/default endpoint、`prefer`、`standard` 与匿名关闭可视觉隐藏；匿名开启、`safe`/`trusted`、异常和用户动作不得隐藏，完整语义保留在 accessible name、tooltip 与展开面板。
- 顶栏和 composer 不重复渲染同一运行策略控件；切换会话后所有控件必须从新会话 store 重新读取，不能串台。
- 图标只压缩展示，不改变真相源；颜色之外必须保留形状、accessible label 和 tooltip。

## 10. 兼容与迁移

- 不改变会话、任务、usage 或 delivery 数据格式；
- 旧历史没有 plan 时不显示伪进度；
- `through_release` 配置值保持不变，只纠正用户文案；
- 现有 theme key 和 `surface-0..4` 保持可用；
- 浏览器 pane 的 Phase-1 同 URL 预览已纳入 pane arbiter；同一 Agent 页面与接管能力不纳入本批；
- 既有 browser/document tab 作为 pane arbiter 的初始实现继续复用，不采纳重复 PR #327；
- 外部机器人不进入 DOM、asset、测试 fixture 或 viewport 判定。
