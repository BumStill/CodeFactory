# 现代 Agent Workbench 体验规格

## Requirements Traceability

| Req ID | 要求 | Surface | 验证 |
| --- | --- | --- | --- |
| CF-WB-R1 | Workspace 使用可辨认的 canvas、pane、raised、subtle 层级，浅色与深色都具有现代、克制的工作台观感 | theme + Workspace | token audit + real app |
| CF-WB-R2 | 普通会话正文居中且不超过 880px；用户消息、助手正文和结果 footer 对齐同一阅读列 | MessageList | component geometry + real app |
| CF-WB-R3 | 正文/操作/辅助信息采用 15/13/11px 层级；9–10px 不得承载正文、关键动作、失败原因或验收结果 | Workspace components | static audit + screenshots |
| CF-WB-R4 | normal/progress/success/warning/danger/info 使用统一语义 token；状态不能只靠颜色 | theme + status surfaces | unit + contrast + real app |
| CF-WB-R5 | 结果 footer 明确区分 objective completed、system-owned 未完成、必要输入/业务决定与 cancelled；未完成不得使用成功图标/绿色完成语义，技术状态不得显示用户恢复 CTA | TurnResultSnapshot | component |
| CF-WB-R6 | 正常 context 使用中性/progress 色，70–85% warning，≥85% danger，并暴露 accessible meter | ContextUsageBar | pure function + component |
| CF-WB-R7 | composer、queue、draft scope 与 context 收敛成一个 raised 操作表面；用量/context 位于输入框上方；曲别针、单行文本与发送按钮共享 32px 垂直基准；placeholder 不列文件格式 | Workspace + MessageInput | component geometry + real app |
| CF-WB-R8 | queue chip 的展开方向、`aria-expanded`、逐条移除名称和键盘 focus 正确 | QueueBadge | component |
| CF-WB-R9 | 会话侧栏支持按标题/项目路径搜索，完整标题可读，关键辅助文字不小于 11px；收起入口归属于“会话”栏头且使用简单方向图标，隐藏后由顶栏带“会话”文字的入口恢复 | SessionSidebar | interaction component + real app |
| CF-WB-R10 | 当前回合、后台作业、交付、队列保持独立真相源，但共享用户可理解的状态语义 | Workspace | integration + adversarial fixture |
| CF-WB-R11 | 任务抽屉中的任务标题、recovery owner、失败归因、最近进展、下一次观察和验收结果可读；system-owned remediation 与 core input/business decision 使用不同 typed 语义 | task activity | component + real app |
| CF-WB-R12 | 交付链固定区分 PR、CI、合并、正式发布和线上验证；release 不能显示成 live | delivery + Settings + onboarding | component + copy audit |
| CF-WB-R13 | Workspace 用户可见残留英文收敛为中文，代码、命令、API 名称除外 | Workspace | copy audit |
| CF-WB-R14 | 1366×768、800×600、375×812 和 200% zoom 无整页横向溢出，composer 与关键状态可达 | desktop UI | viewport + real app |
| CF-WB-R15 | `prefers-reduced-motion` 下非必要旋转、脉冲、宽度和位移动画停用 | desktop UI | CSS/component |
| CF-WB-R16 | 外部机器人不进入 CodeFactory 资产、DOM、交互、品牌或 viewport 验收 | repository | negative audit |
| CF-WB-R17 | 任务、Git、交付、证据和按需浏览器共用单一右侧 pane 仲裁，不得同时叠开两个辅助面 | Workspace auxiliary pane | 第二批 integration + real app |
| CF-WB-R18 | Workspace 与 acceptance 必须挂载同一任务活动组件，旧 TaskDashboard/ExecutionStream 不得继续作为虚假验收替身 | task acceptance | 第二批 repository intent + real app |

## Primary User Path

用户打开 CodeFactory，在左侧搜索或选择会话；顶栏确认项目、模型、权限、本地 Git、交付和后台作业状态；在居中阅读列查看当前回复与自然工具证据；通过结果 footer 查看修改、验证、失败和等待边界；需要时打开任务或交付详情；始终可在统一 composer 中继续指令、引导当前执行、排队、停止或附加文件。只有明确 live verifier 通过时，界面才显示“已验证上线”。

## Applicable Harnesses

- Spec Harness：CF-WB-R1..R18。
- Viewport Harness：四种视口/缩放、overflow、composer 可达、抽屉遮挡。
- Compatibility Harness：旧会话无 plan、现有 theme key、delivery ceiling 配置值、usage 数据。
- Observation Harness：真实 Tauri 浅色/深色、成功与阻塞路径。
- AI Collaboration Harness：设计审计、关键假设、失败测试和独立 QA。

## 测试矩阵

| 场景 | 正常路径 | 边界路径 |
| --- | --- | --- |
| Result | CompletionArbiter 通过且 evidence 完整才显示已完成 | 4/6、system wait 显示系统继续处理；只有 typed input/decision 显示用户动作 |
| Context | 58% 为中性/progress | 75% warning、90% danger |
| Composer | 正常发送、附件、context footer | streaming steer、queue、stop、错误 |
| Sidebar | 标题/路径搜索并打开会话 | 空结果、长标题、等待批准、运行中 |
| Tasks | running → verified → objective completed | approach exhausted → remediation；core input/decision；explicit cancel |
| Delivery | PR → CI → merge → release | remote unavailable、release 未 live |
| Viewport | 1366×768 light/dark | 800×600、375×812、200% zoom |
| Motion | 正常展开/状态过渡 | reduced motion |

## Evidence Pack Requirements

- 失败优先的 component/static tests；
- `pnpm test`、`pnpm build`、治理基线；
- 1366×768 与 800×600 浅色/深色截图；
- 真实 CodeFactoryDev 成功路径：发送 → 执行 → 验证 → 已完成结果；
- 真实 CodeFactoryDev 边界路径：delivery 到 release 但缺 live verifier，必须显示未验证上线；
- keyboard/focus、200% zoom、无整页 overflow 记录；
- PR、CI、合并状态；正式发版与 live 证据未完成前标记 `not live`。

## 发布边界

本特性是用户可见 `feat`。合并后按刻意发版规则进入最近一班 release；只有对应公开安装产物通过真实主路径，才能称为该版本可用。正式 release artifact 不等于业务目标 live verification。
