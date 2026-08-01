# 现代 Agent Workbench UX 设计

## 1. 体验基调

CodeFactory 的现代感定义为：安静、精确、可信、响应及时。它应像一个高质量工程工作台，而不是聊天网页、日志终端或卡片看板。

现代感来自：

- 清晰的 canvas / pane / raised 三层空间；
- 统一的 15px 正文、13px 操作、11px 辅助信息；
- 适度圆角、细描边和局部阴影，而非每个区域都画卡片；
- 中性底色 + 单一蓝色 progress + 克制的状态色；
- 120–180ms 的颜色、位移和展开过渡；
- 一致的 hover、focus、selected、disabled 和 loading；
- 重要状态总是有文字，图标不承担猜谜。

禁止把“现代”理解为玻璃拟态、大面积渐变、霓虹发光、无意义动效、超大圆角或低对比浅灰。

## 2. 视觉层级

### 2.1 Surface

| 层级 | 用途 | 视觉 |
| --- | --- | --- |
| Canvas | 会话主背景 | 最安静，承载阅读 |
| Pane | 顶栏、侧栏、composer 区 | 与 canvas 明显但克制地区分 |
| Raised | 输入框、菜单、抽屉、结果 footer | 1px 边界，必要时轻阴影 |
| Subtle | hover、选中、证据行 | 低对比局部色块 |
| Overlay | modal、浮层 | 明确遮罩与最高层级 |

浅色侧栏应比正文略暗，正文阅读区最亮；深色主题保持同样的相对层级。

### 2.2 排版

- 助手正文：15px / 24px，正常字重；
- 用户消息：14–15px / 22–24px；
- 操作、任务标题、按钮：13px / 20px；
- 时间、计数、静态元信息：11px / 16px；
- 禁止用 9–10px 承载正文、动作、失败原因或验证结果；
- 普通正文宽度 760–880px，中文约 40–50 字/行；
- 代码、diff、表格可进入专用检查器扩宽，不把所有正文铺满屏幕。

### 2.3 几何

- 主按钮与图标按钮最小 32×32px；
- 通用圆角：行/按钮 8px，输入框/浮层 12–16px；
- 会话行 40–44px，任务行至少 36px；
- 区域间距优先使用 4/8/12/16/24px 节奏。

## 3. 页面布局

```text
┌ Session rail ┬ Header: identity · model · permission · git · delivery · jobs ┐
│ search       ├──────────────── Conversation canvas ───────────────────────────┤
│ attention    │             readable 760–880px column                         │
│ running      │     answer → evidence → compact result footer                 │
│ recent       ├───────────────────────────────────────────────────────────────┤
│              │ raised composer: scope/queue · input · context                │
└──────────────┴───────────────────────────────────────────────────────────────┘
```

正文是唯一视觉主角。顶栏回答“在哪里、用什么、交付到哪”；右侧工作面只在查看任务、diff、Git、交付或浏览器时出现。

## 4. 作业流

### 4.1 当前回合

状态顺序：

```text
规划中 → 执行中 ↔ 等待用户 → 验证中 → 待审核 → 已完成
                   ↘ 已阻塞 / 失败
```

只有结构化 plan 存在时显示步骤完成度。当前步骤和下一步直接可读；失败或等待时，spinner 替换为 warning/danger 图标和具体原因。

### 4.2 后台作业

顶栏入口只在 pending、running、failed 或需要用户处理时出现。抽屉首屏依次呈现：

1. 需要处理的 blocker 与主要动作；
2. running / pending 摘要；
3. 任务列表与验收状态；
4. 失败归因、重试和完整输出。

完成与取消历史不形成持续告警。可恢复失败为 warning，不可恢复阻塞为 danger。

### 4.3 交付

交付视图固定显示五级：

```text
PR → CI → 合并 → 正式发布 → 线上验证
```

- PR open、CI pending：progress；
- CI success：仅 CI 步骤 success；
- merged：仅到合并 success；
- release tag 包含 merge：显示“正式版本已创建”；
- 没有 live verifier：明确显示“未验证上线”，整体不得使用完成绿色。

### 4.4 队列与纠偏

- 正常输入：Enter 发送；
- 执行中：Enter 在安全点引导当前执行；
- `⌘/Ctrl+Enter`：排到当前回合之后；
- Stop：停止后续生成，不回滚已完成修改；
- 队列折叠为 composer 上方 chip，展开后可逐条移除，箭头方向与 `aria-expanded` 一致。

## 5. 关键组件

### 5.1 会话侧栏

- 顶部提供搜索；第二批增加 Pin 和状态分组；
- 展开时，“收起”使用简单左向 Chevron 并与“会话”标题同组，不把侧栏操作混入当前会话工具栏；
- 收起后，当前会话工具栏仅显示带“会话”文字和消息图标的恢复入口，不能退化成含义不明的 panel 方框图标；
- 行内显示标题、项目/分支上下文、最后活动时间和状态文字/图标；
- 当前项使用 2px accent + subtle background；
- 完整标题和路径通过 accessible label/tooltip 可读；
- 不出现 9px 操作或仅 hover 才可发现的唯一关键动作。

### 5.2 会话正文

- 整体居中到 760–880px；
- 成功工具调用保持低强调行，失败、审批、阻塞自动展开；
- inline code 使用中性蓝灰，不再让橙色代码成为全页最强视觉；
- 所有用户可见文案使用中文：`新内容`、`回到最新`、`正在处理`。

### 5.3 结果 footer

完成：

```text
✓ 已完成 · 6/6       修改 4 · 验证 3 · 用时 8m
  查看证据   执行过程   结果摘要
```

未完成或阻塞：

```text
! 需要处理 · 4/6
  当前：交付预检     原因：缺少 live verifier
  下一步：补齐 verifier 或调整目标上限
```

未完成不得继续显示 `CheckCircle2`。结果 footer 不复制最终回复。

### 5.4 Composer

- placeholder 简化为“描述任务或继续对话…”；
- 支持格式只在附件按钮 tooltip/menu 展示；
- 附件 chips 和错误在输入框上方；
- 会话/今日用量与 context meter 作为 composer header 放在输入框上方，不在底部制造独立 footer 留白；
- 曲别针、单行 textarea 和发送/停止按钮使用同一 32px 垂直盒；textarea 以 24px 行高 + 上下各 4px 内边距对齐图标视觉中心，多行时控制继续贴底；
- send 在可发送时为清晰主动作，stop 使用 danger 语义；
- queue、draft scope、context 都属于同一 raised surface；
- context 正常区为 neutral/progress，70–85% warning，≥85% danger；显示 meter 的 accessible name/value。

## 6. 动效

- hover/focus 120ms，drawer/accordion 160–180ms；
- 动效只解释状态变化和层级，不循环吸引注意；
- running 可以有轻脉冲或 spinner，但必须同时有文字；
- `prefers-reduced-motion` 下关闭旋转、脉冲、位移和宽度动画；
- 新 stream 不强拉回正在上翻的用户。

## 7. 颜色与可访问性

- 正文对比度 ≥4.5:1；
- 非文本 UI、focus、状态边界 ≥3:1；
- 状态使用“颜色 + 图标/形状 + 文案”；
- focus ring 在所有 surface 上清晰可见；
- 200% 缩放时顶栏允许收缩/隐藏低优先级摘要，不能遮挡 composer；
- 关键状态使用 `role=status` / `aria-live=polite`，避免逐 token 打断读屏。

## 8. 视口验收

### 1366×768

- 左栏至少自然显示 10 条会话；
- 正文宽度不超过 880px；
- 顶栏核心状态不重叠；
- composer、context 和主动作完整可见。

### 800×600

- 顶栏无横向溢出，低优先级文案可收束；
- 结果 footer 的动作可换行；
- composer 不被任务/交付抽屉遮挡；
- 会话正文仍有至少 480px 有效宽度。

### 375×812 / 200% zoom

- 左栏与辅助 pane 采用 overlay，不与正文并排；
- composer 固定可达，页面无整体横向滚动；
- 状态文字不被压成只有颜色或图标；
- 所有核心操作目标至少 32px。

## 9. 冲突裁决

- `chat-continuity` 中“最终回复不切 dashboard”保留；P2 结果卡解释为紧凑 footer。
- `workspace-navigation` 中“latest release 即已上线”作废；正式发布与 live verification 分离。
- `token-usage` 的常驻独立底栏改为 composer 输入框上方的低强调 header，数据语义不变。
- `TasksColumn`、Git/交付 drawer 与按需 browser pane 后续统一受 pane arbiter 管理。
- 外部机器人不计入 CodeFactory 视觉审计、布局避让或产品品牌。
