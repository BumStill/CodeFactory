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
| warning | 可恢复、需要观察或用户动作 | context 70–85%、等待权限、可修复失败 |
| danger | 阻塞、失败或资源临界 | 不可修复失败、CI failure、context ≥85% |
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
│  ├─ model / permission
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
│  └─ ContextUsageBar
└─ AuxiliaryPaneArbiter (第二批)
   ├─ TaskActivityDrawer
   ├─ Git / delivery / evidence
   └─ On-demand browser pane
```

首批把 composer 周边收敛为一个视觉 surface；数据获取仍留在现有组件。`ContextUsageBar` 通过回调进入用量详情，不自行持有路由。

## 6. 结果状态映射

结果快照根据结构化 plan 和工具证据映射：

```text
complete && failureCount == 0 && !waitingReason -> success / 已完成
failureCount > 0 || waitingReason               -> warning / 需要处理
otherwise                                      -> neutral / 未完成
```

该映射只描述“本回合证据状态”，不推断交付 live。`waitingHistory` 仅作为历史证据，当前状态优先使用 `waitingReason`。

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

当前任务抽屉、Git/交付抽屉与并行开发中的按需浏览器 pane 都可能占据右侧。第二批引入：

```ts
type AuxiliaryPane =
  | { kind: "task"; taskId?: string }
  | { kind: "git"; view: "changes" | "history" | "remote" }
  | { kind: "delivery" }
  | { kind: "evidence"; turnId: string }
  | { kind: "browser"; sessionId: string }
  | null;
```

规则：同一时刻最多一个右侧辅助面；切换前保存各自 selection/scroll state；需要阻塞用户动作的权限 dialog 仍是 modal，不进入 pane。

## 9. 兼容与迁移

- 不改变会话、任务、usage 或 delivery 数据格式；
- 旧历史没有 plan 时不显示伪进度；
- `through_release` 配置值保持不变，只纠正用户文案；
- 现有 theme key 和 `surface-0..4` 保持可用；
- 浏览器 pane 的未提交并行实现不纳入本分支，后续通过 pane arbiter 合并；
- 外部机器人不进入 DOM、asset、测试 fixture 或 viewport 判定。
