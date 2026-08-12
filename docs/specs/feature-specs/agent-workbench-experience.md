# 现代 Agent Workbench 体验规格

## Requirements Traceability

| Req ID | 要求 | Surface | 验证 |
| --- | --- | --- | --- |
| CF-WB-R1 | Workspace 使用可辨认的 canvas、pane、raised、subtle 层级，浅色与深色都具有现代、克制的工作台观感 | theme + Workspace | token audit + real app |
| CF-WB-R2 | 普通会话正文居中且不超过 880px；用户消息、助手正文和结果 footer 对齐同一阅读列 | MessageList | component geometry + real app |
| CF-WB-R3 | 正文/操作/辅助信息采用 15/13/11px 层级；9–10px 不得承载正文、关键动作、失败原因或验收结果 | Workspace components | static audit + screenshots |
| CF-WB-R4 | normal/progress/success/warning/danger/info 使用统一语义 token；状态不能只靠颜色 | theme + status surfaces | unit + contrast + real app |
| CF-WB-R5 | 结果 footer 分开表达“计划步骤执行度”和“证据完整性”；只有明确需要用户动作时写“需要你处理”，已执行但有内部失败证据时写“已执行，证据待复核” | TurnResultSnapshot | component + adversarial fixture |
| CF-WB-R6 | context 以圆环 meter 表达当前窗口已用比例：<75% 只显示中性圆环，75–89% 追加百分比，≥90% 追加“接近上限”；状态不能只靠颜色，圆环必须暴露 accessible meter/value | ContextUsageBar | pure function + component |
| CF-WB-R7 | composer、queue、draft scope、运行策略与 context 收敛成一个 raised 操作表面；输入框内嵌唯一紧凑工具条，不再为 scope、快捷键或单个模型各占一行；曲别针、单行文本与发送按钮共享 32px 视觉基准；placeholder 不列文件格式 | Workspace + MessageInput | component geometry + real app |
| CF-WB-R8 | queue chip 的展开方向、`aria-expanded`、逐条移除名称和键盘 focus 正确 | QueueBadge | component |
| CF-WB-R9 | 会话侧栏支持按标题/项目路径搜索，完整标题可读，关键辅助文字不小于 11px；收起入口归属于“会话”栏头且使用简单方向图标，隐藏后由顶栏带“会话”文字的入口恢复 | SessionSidebar | interaction component + real app |
| CF-WB-R10 | 当前回合、后台作业、交付、队列保持独立真相源，但共享用户可理解的状态语义 | Workspace | integration + adversarial fixture |
| CF-WB-R11 | 任务抽屉中的任务标题、恢复动作、失败归因和验收结果可读；不可修复阻塞与可恢复失败不同语义 | task activity | component + real app |
| CF-WB-R12 | 交付链固定区分 PR、CI、合并、正式发布和线上验证；release 不能显示成 live | delivery + Settings + onboarding | component + copy audit |
| CF-WB-R13 | Workspace 用户可见残留英文收敛为中文，代码、命令、API 名称除外 | Workspace | copy audit |
| CF-WB-R14 | 1366×768、800×600、375×812 和 200% zoom 无整页横向溢出，composer 与关键状态可达 | desktop UI | viewport + real app |
| CF-WB-R15 | `prefers-reduced-motion` 下非必要旋转、脉冲、宽度和位移动画停用 | desktop UI | CSS/component |
| CF-WB-R16 | 外部机器人不进入 CodeFactory 资产、DOM、交互、品牌或 viewport 验收 | repository | negative audit |
| CF-WB-R17 | 任务、Git、交付、证据、文档和按需浏览器共用单一右侧 pane 仲裁，不得同时叠开两个辅助面；无有效内容时 pane 必须收起，有内容时必须显示标题、状态与关闭入口 | Workspace auxiliary pane | integration + real app |
| CF-WB-R18 | Workspace 与 acceptance 必须挂载同一任务活动组件，旧 TaskDashboard/ExecutionStream 不得继续作为虚假验收替身 | task acceptance | 第二批 repository intent + real app |
| CF-WB-R19 | 顶栏只保留会话身份、本地工程、交付、后台作业与设置；正常状态图标优先，只有当前阶段、异常与用户动作保留短文字，并提供 tooltip/accessible name | Workspace header | component + viewport + real app |
| CF-WB-R20 | 模型、思考强度与会话权限位于 composer；模型是唯一运行策略入口，思考强度合并进模型面板；运行中切换模型明确标示“下一回合生效”，权限升级仍显示完整风险说明 | composer runtime controls | component + real app |
| CF-WB-R21 | 会话/今日累计 Token 不常驻占行；点击 context 圆环后渐进披露当前窗口、会话累计、今日累计与压缩信息，context 百分比不得与累计 Token 混成同一指标 | ContextUsageBar | component + copy audit |
| CF-WB-R22 | ≥1440px 辅助 pane 可停靠并按内容类型使用合适宽度，1024–1439px 使用右侧 drawer，<1024px 或 200% zoom 使用全高 overlay；浏览器/文档宽度可调，状态类 pane 保持紧凑，任何状态不得产生无标题白色空区 | Workspace auxiliary pane | component + viewport + real app |
| CF-WB-R23 | context 百分比不可用时显示虚线未知态且不得伪造数值；详情才显示当前 used/limit/remaining、会话累计、今日累计、成本来源与压缩信息 | ContextUsageBar | partial/unavailable component + real app |
| CF-WB-R24 | 本地 Git、交付和后台任务遵循“图标表达阶段、数字表达数量、短文字只表达当前结论/异常/下一步”；错误、阻塞、待确认不能纯图标或只靠颜色 | Workspace header | fixture matrix + copy/accessibility audit |
| CF-WB-R25 | 交付摘要常态只显示 PR 标识与当前阶段；完整 `PR → CI → 合并 → 正式发布 → live verification` 放入同一详情和可访问名称，远程不可用不得退化为“未关联 PR” | WorkspaceDeliveryStatus | component + authenticated probe |
| CF-WB-R26 | 结果状态使用结构化下一动作责任人；`6/6 + failure evidence` 是“已执行，证据待复核”，system-owned 恢复是“系统继续处理”，仅 `nextActionOwner=user` 可显示“需要你处理”，禁止从 `waitingReason` 自由文本猜测责任人 | TurnResultSnapshot + plan contract | Rust/TS contract + adversarial fixture |
| CF-WB-R27 | 当前结构化 Git/交付/任务状态是权威面；历史消息中的 PR、发布或结果文字保持普通历史内容，不得与顶栏/辅助 pane 竞争为当前状态 | Workspace status presentation | stale-history integration fixture |
| CF-WB-R28 | 单一 pane tabs 具备 `tablist/tab/tabpanel` 关系、方向键、roving tabindex；overlay 支持 Escape、focus trap 与关闭回焦；separator 可键盘调整 | Workspace auxiliary pane | interaction + axe + real keyboard |
| CF-WB-R29 | 主阅读列和 composer 使用同一最大 880px 视觉网格；pane 打开后不产生页面级横向滚动，composer、停止、权限与附件保持可达 | MessageList + Workspace composer | geometry + viewport + real app |
| CF-WB-R30 | 图标按钮有稳定 accessible name、tooltip、可见 focus；桌面命中区至少 36×36px，窄屏/触控至少 44×44px；200% zoom 与 VoiceOver 路径可用 | header + composer + pane | axe + bounding boxes + real app |
| CF-WB-R31 | PR、CI、merge、正式 release、公开 artifact 与精确版本正式 App 主路径均有证据前保持 `not live`；只有显式 live verifier 通过才显示“已验证上线” | delivery + release | evidence pack + installed artifact smoke |
| CF-WB-R32 | composer 遵循渐进披露：草稿态常驻项目范围与模型，活跃会话常驻模型、权限和 context 圆环；默认 endpoint/首选策略/standard 权限/匿名关闭不重复占文字，匿名开启、safe/trusted、异常与用户动作必须升格为可见文字；快捷键仅在宽屏聚焦时出现，375px/200% zoom 不得形成第三层或横向溢出 | composer utility toolbar | component + viewport + real app |

## Primary User Path

用户打开 CodeFactory，在左侧搜索或选择会话；顶栏确认项目、本地 Git、交付和后台作业状态；在居中阅读列查看当前回复与自然工具证据；通过结果 footer 区分步骤执行与证据完整性；需要时在唯一右侧辅助 pane 查看任务、Git、交付、证据、文档或浏览器；始终可在统一 composer 中选择模型、思考与权限，继续指令、引导当前执行、排队、停止或附加文件，并用 context 圆环判断窗口健康。只有明确 live verifier 通过时，界面才显示“已验证上线”。

## Applicable Harnesses

- Spec Harness：CF-WB-R1..R32。
- Viewport Harness：四种视口/缩放、overflow、composer 可达、抽屉遮挡。
- Compatibility Harness：旧会话无 plan、现有 theme key、delivery ceiling 配置值、usage 数据。
- Observation Harness：真实 Tauri 浅色/深色、成功与阻塞路径。
- AI Collaboration Harness：设计审计、关键假设、失败测试和独立 QA。

## 测试矩阵

| 场景 | 正常路径 | 边界路径 |
| --- | --- | --- |
| Result | plan 全完成且无失败显示已完成 | 6/6 + 内部失败显示证据待复核；仅结构化 user owner 显示需要处理 |
| Context | 58% 圆环为中性/progress，累计 Token 进入详情 | 75% warning、90% danger、数据未知显示可理解空态 |
| Composer | 草稿态单条工具栏显示项目范围与模型；活跃会话显示模型、权限和 context 圆环；思考强度在模型面板内一跳可达 | 匿名开启、safe/trusted、streaming 下一回合生效、queue、stop、错误；375px/200% 不出现第三层或横向溢出 |
| Sidebar | 标题/路径搜索并打开会话 | 空结果、长标题、等待批准、运行中 |
| Tasks | running → completed + verification | repairable failure、blocked failure |
| Delivery | PR → CI → merge → release | remote unavailable、release 未 live |
| Auxiliary pane | ≥1440px 单一停靠 pane，任务/Git/交付互斥 | 1024–1439px drawer；<1024px/200% zoom overlay；空态/加载/错误与浏览器关闭回收 |
| Viewport | 1366×768 light/dark | 800×600、375×812、200% zoom |
| Motion | 正常展开/状态过渡 | reduced motion |

## Evidence Pack Requirements

- 失败优先的 component/static tests；
- `pnpm test`、`pnpm build`、治理基线；
- 1440×900、1366×768 与 800×600 浅色/深色截图；
- 真实 CodeFactoryDev 成功路径：发送 → 执行 → 验证 → 已完成结果；
- 真实 CodeFactoryDev 边界路径：delivery 到 release 但缺 live verifier，必须显示未验证上线；
- keyboard/focus、200% zoom、无整页 overflow 记录；
- PR、CI、合并状态；正式发版与 live 证据未完成前标记 `not live`。

## 发布边界

本特性是用户可见 `feat`。合并后按刻意发版规则进入最近一班 release；只有对应公开安装产物通过真实主路径，才能称为该版本可用。正式 release artifact 不等于业务目标 live verification。
