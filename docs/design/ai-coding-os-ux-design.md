# AI Coding OS 控制面 UX 设计

## UX 目标

控制面不是新的聊天页，而是一个系统状态页。用户进入后应立刻知道：

- 当前项目有哪些权威规则 surface。
- 有多少待审核记忆/偏好。
- 哪些能力已启用。
- 当前交付门禁是否满足。
- 哪些风险需要先处理。

## 入口

Home 顶部增加一个图标按钮：

- 图标：`ShieldCheck`
- tooltip：`AI Coding OS`
- 点击进入控制面页面。

Workspace 后续可追加同入口，但 v1 先在 Home 顶部提供全局入口。

## 页面结构

```text
┌────────────────────────────────────────────┐
│ AI Coding OS                      返回     │
├────────────────────────────────────────────┤
│ 项目上下文 / 当前 cwd / 生成时间             │
├────────────────────────────────────────────┤
│ Authority Surfaces                         │
│  AGENTS.md  docs/specs  .codefactory/specs │
│  sync hook  release cadence                │
├────────────────────────────────────────────┤
│ Memory Lifecycle                           │
│  pending accepted rejected preference      │
├────────────────────────────────────────────┤
│ Capability Registry                        │
│  Skills MCP Knowledge Hooks Git remotes    │
├────────────────────────────────────────────┤
│ Delivery Gates                             │
│  branch dirty sync gate hook config release│
├────────────────────────────────────────────┤
│ Risks                                      │
│  missing hook / dirty tree / no cwd ...    │
└────────────────────────────────────────────┘
```

## 视觉原则

- 不用营销 hero。
- 不使用大面积装饰图形。
- 信息密度高但分区清晰。
- 状态用小 badge：`OK`、`Missing`、`Warning`。
- 风险列表放在顶部下方或底部，使用紧凑行，不用弹窗。

## 关键状态

### 无项目上下文

当用户没有 active project：

- 页面仍可打开。
- 显示“未绑定项目上下文”。
- Authority 和 Delivery 显示 warning。
- Capability 仍展示全局 Skills/MCP/Hooks/Git remote 等统计。

### 有项目上下文

当用户从已有 active session 打开：

- 显示 cwd。
- Authority 扫描该 cwd。
- Delivery 扫描该 cwd 的 git 状态。
- Memory summary 过滤该 cwd 的 learning events。

### 风险

风险只呈现事实，不替用户做不可逆动作：

- `missing-sync-gate`: 没有 `.githooks/pre-commit`。
- `sync-gate-not-configured`: `.githooks/pre-commit` 存在，但当前 checkout 没有 `core.hooksPath=.githooks`，提交门禁并未实际生效。
- `dirty-worktree`: 工作区有未提交变化。
- `no-project-context`: 当前没有 cwd。
- `no-release-workflow`: 仓库没有 release workflow。
- `memory-review-pending`: 有待审核 memory proposal。

## 交互

v1 只读：

- 返回 Home。
- 刷新快照。
- 状态项不可编辑。

后续 v2：

- 点击 Skills 跳到技能库。
- 点击 pending memory 跳到 Profile 审核。
- 点击 sync gate 跳到 Git 交付设置。
- 点击 release workflow 跳到发布中心。

## 验收

- 用户能从 Home 打开 AI Coding OS 页面。
- 页面在无项目和有项目两种状态下都不崩溃。
- 状态项不会因为长路径或长标签溢出。
- 风险列表明确显示 blocker，而不是隐藏在控制台日志里。
- Delivery Gates 明确区分 sync hook 文件存在与本地 git 配置已启用。
