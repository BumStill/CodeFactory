# 当前会话按需内置浏览器分屏规格

## 1. 问题与目标

CodeFactory 已具备 `browser_session` 受管浏览器工具、浏览器 Profile、权限分类、租约与回收能力，但浏览器目前主要表现为后台工具资源，用户无法在当前会话中直接观察页面、完成登录或接管交互。

本特性将浏览器定义为**当前 Agent run 按需创建的内置右侧分屏**：没有活动浏览器时不显示任何常设入口；Agent 实际需要浏览网页时自动创建并展开；主会话同步压缩；任务结束后按生命周期自动关闭和收起。

### 当前交付边界（2026-08-11）

本批只交付 Phase 1 的 Workspace 辅助 pane 仲裁、按会话显隐、响应式布局、原生 child WebView 同 URL 预览和失败恢复。Agent 工具仍操作 `LOCAL` ChromiumDriver 中的受管页面，child WebView 依据 lease 的 `pane_url` 独立加载；两者不共享 Cookie、DOM、导航状态或页面控制权。因此当前 pane **不是 Agent 实际页面的实时镜像，也不支持接管/交还**，EBP-R3 的“同一受管页面”与 EBP-R9 保持 `not live`。PR、release 与 UI 文案不得用“实时观察”“接管 Agent 页面”描述本批能力。

以下 Primary User Path、控制权和生命周期条款是完整目标态；每个阶段只能按已取得的真实证据声明完成。

## 2. 产品原则

1. **按需而非常设**：只有当前会话实际持有活动 `browser_session` 时才显示浏览器分屏，不在输入框、侧栏或全局导航增加浏览器入口。
2. **内置而非外置**：页面显示在 CodeFactory 当前 Workspace 内的右侧独立 pane，不默认拉起外部 Chrome 或独立系统窗口。
3. **归属当前会话**：浏览器 pane 与创建它的 `owner_session_id`、必要时与 `task_id` 绑定；切换会话时不得串台。
4. **会话可见、网页隔离**：主会话和浏览器同屏，但浏览器页面运行在独立安全 WebView/受管浏览器上下文中，不能与 CodeFactory UI 共用 DOM、Cookie 或脚本权限。
5. **终态自动回收**：正常完成、失败和取消默认关闭浏览器；用户正在登录、授权或接管时延后关闭，以无活动租约回收。
6. **保留身份，不保留进程**：持久 Profile 可保留登录态；浏览器进程、页面、DOM 引用和控制租约不因后续追问而常驻。

## 3. Requirements Traceability

| Req ID | 用户要求 / 来源 | 规范化要求 | 产品表面 | 验证方法 | Owner |
| --- | --- | --- | --- | --- | --- |
| EBP-R1 | 浏览器不应有常设入口 | 无活动浏览器时 Workspace 不呈现浏览器按钮、占位 pane 或导航项 | Workspace | 组件测试 + 真实应用截图 | frontend |
| EBP-R2 | 浏览器应按需自动打开 | Agent 首次成功创建当前会话的 `browser_session` 后自动展开 pane | Workspace / Tauri events | 集成测试 + 真实主路径 | full-stack |
| EBP-R3 | 浏览器应该是内置的 | 网页在右侧隔离 WebView/受管浏览器 pane 中显示，不默认打开外部窗口 | Workspace / browser host | 桌面集成测试 | desktop |
| EBP-R4 | 宽屏使用可调右侧 pane | ≥1440px 时默认约 38vw，并限制在 480–720px；separator 可由指针或键盘调整并保存偏好 | Workspace layout | Viewport Harness | frontend |
| EBP-R5 | 主会话保持可读 | ≥1440px 停靠时主会话流式重排且不低于可读下限；更窄窗口使用 overlay，不继续把正文压成细栏；输入框与关键控制始终可用 | Workspace layout | 真实应用视口验证 | frontend |
| EBP-R6 | 审视自动关闭 | run 完成、失败、取消默认自动关闭；等待用户或用户接管时按活动租约延后 | lifecycle manager | Rust 单元/集成测试 + smoke | backend |
| EBP-R7 | 长方案写入文件 | 长期产品决策保存在仓库规格中，会话只返回摘要和文件链接 | `docs/specs` | 治理基线检查 | planning |
| EBP-R8 | 并行会话隔离 | 只显示当前 `owner_session_id` 的 pane；其他会话浏览器继续后台运行但不占当前布局 | Workspace / manager | 多会话集成测试 | full-stack |
| EBP-R9 | 用户可介入 | 需要登录、验证码或授权时 pane 自动聚焦并暂停 Agent；用户完成后显式交还 | browser pane controls | 真实成功与边界路径 | full-stack |
| EBP-R10 | 安全边界不降低 | 继续拒绝非 HTTP(S)，持久身份读站点按 host 授权，Agent 点击/输入逐次授权 | permission layer | Rust policy tests + UI dialog test | security |

## 4. Primary User Path

1. 用户在普通项目会话中提出需要网页研究或网页验证的任务。
2. Agent 判断确有需要并调用 `browser_session.open`；在调用成功前，界面不预留浏览器空间。
3. BrowserSessionManager 创建与当前 `owner_session_id` 绑定的受管页面，并发出 `browser-session-opened` 事件。
4. Workspace 收到当前会话事件后，通过单一辅助 pane 仲裁器显示内置浏览器：≥1440px 时右侧停靠，默认约 38vw 并限制在 480–720px；更窄窗口使用 overlay，不压缩主会话到不可读宽度。
5. Agent 在 pane 中导航、读取和截图。若仅做后台读取，不强制把键盘焦点从输入框抢走；页面仍实时可见。
6. 需要登录、验证码、Passkey 或用户接管时，pane 提升为注意状态，Agent 暂停页面动作，并将焦点引导到明确的“接管浏览器”控制。
7. 用户完成操作并选择“交还 Agent”；原 DOM 引用全部失效，Agent 必须重新 snapshot 后继续。
8. run 正常完成后，系统关闭浏览器、释放 Profile 锁并自动收起 pane；执行消息仅保留脱敏后的访问证据和“已自动关闭”状态。

## 5. 交互与布局

### 5.1 出现与消失

- `0` 个当前会话活动页面：不渲染 pane，不显示常设浏览器入口。
- 首个页面创建成功：自动展开 pane。
- 当前会话有多个页面：首版在同一 pane 顶部用紧凑标签切换，不创建多个并排 pane。
- 最后一个活动页面关闭：pane 自动收起，主会话恢复全宽。
- 用户可以在活动期间临时折叠 pane；折叠只影响显示，不结束 browser session。折叠后仅在当前执行消息中显示“恢复浏览器”临时操作，不能形成全局常设入口。
- 当页面要求用户介入时，即使此前折叠，也应自动恢复 pane；不得在用户正在输入普通消息时无提示抢键盘焦点。

### 5.2 宽度

- ≥1440px 宽屏默认比例约为浏览器 `38vw`，像素范围 `480–720px`；主会话保留剩余空间并维持阅读列下限。
- separator 支持指针与键盘调整，不能只依赖精细拖拽。
- 状态、任务和 Git 等非浏览器 surface 使用更紧凑的默认宽度，不沿用浏览器宽度。
- 用户调整后的比例只在应用本地保存，作为下次按需展开的偏好；它不代表浏览器常驻。
- 当窗口无法同时满足两个最小宽度时，不允许继续横向压缩到不可用状态，按 5.3 降级。

### 5.3 窄窗口降级

- 窗口宽度 `1024–1439px` 时，浏览器使用右侧 drawer overlay；小于 `1024px` 或 200% zoom 下使用全高 overlay，不得把聊天压缩成不可读细栏。
- 输入框、停止生成、权限确认和“结束/交还浏览器”必须保持可达。
- 浏览器 UI 不能覆盖系统标题栏、Workspace 主导航或权限弹窗。

### 5.4 页面顶部控制条

仅在 pane 活动期间显示：

- 当前域名与页面标题；
- 加载 / 等待授权 / Agent 控制 / 用户控制 / 断线状态；
- 后退、前进、刷新（首版可按底层能力裁剪）；
- “接管 / 交还 Agent”；
- “折叠”；
- “结束浏览器”。

地址栏首版默认只读，防止它演变成通用浏览器入口。用户若需导航，可在接管状态下通过受控输入启用，且只允许绝对 HTTP(S) URL。

## 6. 内置浏览器技术边界

“内置”表示浏览器视觉上位于 CodeFactory Workspace 内，并不表示目标网页可直接运行在 React 应用 DOM 中。

实现必须满足：

- 使用独立 WebView、原生 child webview 或等价隔离容器承载目标网页；禁止用普通 `iframe` 作为通用实现。
- 目标站点与 CodeFactory UI 不共享 JS context、DOM、localStorage 或应用 IPC 权限。
- 目标网页不能调用 CodeFactory Tauri commands。
- Cookie 与站点数据继续由 `ProfileScope` 管理：匿名会话强制 ephemeral，普通会话可使用 CodeFactory 专属持久 Profile。
- 关闭 pane 的视觉节点不能被当成浏览器进程已经回收；manager 必须收到并确认真实 close receipt。
- 若目标网站因 DRM、浏览器扩展、系统 SSO 或 WebView 限制无法工作，应给出明确的不兼容状态；外部浏览器降级必须由用户明确触发，不能静默切换。

## 7. 生命周期与自动关闭决策

### 7.1 默认策略

默认自动关闭，原因是浏览器属于本次 run 的受管资源；依赖模型主动 `close` 无法覆盖遗忘、失败、取消和崩溃路径。

| 触发 | 行为 |
| --- | --- |
| run `completed` | 当前动作结束后关闭所有归属该 run 的页面，收起 pane |
| run `failed` | 记录失败原因，关闭并清理；明确重试时创建新会话 |
| run `cancelled` | 取消待执行动作，短暂有界等待后强制清理 |
| 用户点击“结束浏览器” | 只结束当前会话所属浏览器，并通知 Agent 资源不可用 |
| 切换到其他聊天 | 不关闭；隐藏 pane，浏览器可在原 run 中继续活动 |
| 应用正常退出 | 关闭当前实例创建的全部受管浏览器 |
| 应用崩溃 | 下次启动依据租约回收失去所有者的 CodeFactory 会话 |

### 7.2 延迟关闭

以下状态不因 run 暂时无模型输出而立即关闭：

- 等待用户登录、验证码、Passkey、OAuth 或权限决定；
- 用户已接管且正在活动；
- 当前导航、下载或截图处于不可安全中断的短操作窗口。

延迟关闭使用**无活动租约**而不是固定总时长：用户输入、点击、页面交互或 Agent 动作均刷新租约。首版建议默认 10 分钟无活动，超时前显示提醒并允许继续保留一次；策略值属于内部配置，暂不增加全局设置项。

### 7.3 关闭后的保留范围

- 持久 Profile：保留 Cookie 和站点存储，不保留浏览器进程、窗口、页面、DOM 引用或控制租约。
- 临时 Profile：连同 Cookie、缓存和站点存储一起删除。
- 对话历史：仅保留域名、访问时间、工具结果摘要和经用户允许的截图证据，不保存密码、Cookie、Token 或敏感输入。

## 8. 权限与控制权

- 继续执行 `browser/policy.rs` 的安全边界：非 HTTP(S) URL 直接拒绝。
- 持久 Profile 首次读取某个 host 时请求授权；授权仅对当前聊天的该 host 生效。
- Agent 的 `click`、`fill`、`press` 每次请求授权；全局 trust/full access 不绕过浏览器身份动作分类。
- 用户点击“接管”后，Agent 的所有页面动作进入暂停队列或被拒绝。
- 用户点击“交还 Agent”后，旧 element references 作废，必须重新 snapshot。
- 权限弹窗必须显示目标 host、动作类型、当前身份范围，不把普通浏览动作描述成泛化的工具权限。

## 9. 多会话与恢复

- Workspace 只挂载当前 `owner_session_id` 的 browser pane。
- 其他并行 run 可持有自己的受管浏览器；切回所属会话时恢复该 pane 和页面状态。
- 持久 Profile 仍保持“一份 Profile 同时一个 live browser”的锁规则；冲突时展示哪个会话正在使用，不自动抢占。
- 前端重载后通过 manager 的活动会话快照恢复 pane；不能只依赖可能丢失的打开事件。
- pane 视觉恢复后必须验证底层 session 仍可 snapshot，不能仅凭租约记录显示“已连接”。

## 10. Applicable Harnesses

- **Spec Harness**：Req ID、主路径、测试矩阵与证据要求。
- **Compatibility Harness**：Windows/macOS、持久 Profile、旧设置和 WebView 能力差异。
- **Observation Harness**：浏览器状态事件、close receipt、崩溃回收原因。
- **Viewport Harness**：≥1440px 的 480–720px 可调停靠 pane、1024–1439px drawer、小于 1024px/200% zoom 全高 overlay 和权限弹窗可达性。
- **Payload Harness**：截图、下载、文件选择和页面数据进入工具输出时的尺寸与敏感信息边界。
- **AI Collaboration Harness**：模型决定何时创建浏览器、用户接管后的 stale reference 处理与自动关闭假设。

## 11. Viewport Harness

- **Target viewport**：`1440×900`、`1366×768`、`1024×768`、`800×600` 与 200% zoom。
- **First-screen expectations**：浏览器展开后当前执行消息、输入框、浏览器状态条和页面首屏同时可识别。
- **Fixed action expectations**：停止生成、权限决定、接管/交还、折叠和结束浏览器始终可达。
- **Overflow rule**：主会话与浏览器各自滚动；不得形成页面级双横向滚动条；拖动不得越过最小宽度。
- **Animation visibility rule**：展开/收起使用短时布局过渡；`prefers-reduced-motion` 下取消过渡；流式回答时不得因频繁页面事件触发布局抖动。
- **Screenshot or recording**：每个目标视口记录无浏览器、默认/边界宽度或 overlay、等待授权、用户接管和自动收起状态。

## 12. 测试矩阵

| Path type | Scenario | Expected result | Evidence |
| --- | --- | --- | --- |
| Primary | Agent 首次打开公开网页 | ≥1440px 自动停靠，较窄窗口使用 overlay；聊天与输入始终可用 | 真实 app 录屏 + 状态事件 |
| Primary | run 正常完成 | 浏览器真实关闭，pane 收起，主会话恢复全宽 | close receipt + 截图 |
| Interaction | 用户拖动或键盘调整分隔条 | 浏览器限制在 480–720px，偏好在下一次按需展开时恢复 | 浏览器布局断言 + 录屏 |
| Interaction | 用户接管再交还 | Agent 操作暂停；交还后旧引用失效并重新 snapshot | 工具轨迹 + UI 录屏 |
| Failure | open 失败 | pane 不留下空壳，租约与 Profile 锁均被清理 | 集成测试 |
| Failure | run 失败或取消 | 所属浏览器被自动关闭，不影响并行聊天 | 多会话集成测试 |
| Failure | 前端重载 | 活动会话经快照恢复并通过真实 snapshot 验证 | 桌面集成测试 |
| Compatibility | macOS 与 Windows | child webview、焦点、Cookie/Profile 和关闭行为一致 | 双平台 CI + 实机证据 |
| Security | 网页尝试访问 Tauri IPC / 非 HTTP(S) | 无法访问 IPC，非法 scheme 被拒绝 | 安全测试 |
| Viewport | 1366 宽 | 使用右侧 drawer，不把主会话压成细栏 | 截图 + 尺寸断言 |
| Viewport | 小于 1024 宽或 200% zoom | 切换到全高 overlay，关键操作仍可达 | 真实 app 录屏 |
| Parallel | 当前会话切换 | pane 不串台；返回原会话恢复正确页面 | 多会话实测 |
| Lifecycle | 等待登录超过无活动租约 | 先提醒，未续期后关闭；用户活动会刷新租约 | 时钟测试 + 实地验证 |

## 13. Evidence Pack Requirements

实现完成声明至少需要：

1. 当前会话从 `browser_session.open` 到 pane 自动展开的事件与 UI 对应证据；
2. 目标网页确实显示在隔离的内置容器中，而非 mock 或静态截图；
3. ≥1440px 的 480–720px 调整边界、较窄窗口 overlay 降级和键盘 separator 的机器断言；
4. 正常完成、失败、取消、用户接管和崩溃恢复的真实 close receipt；
5. 并行会话不串台、用户普通 Chrome 不受影响的证据；
6. macOS 与 Windows 至少各一条真实主路径；
7. 公开网页成功路径，以及登录等待/取消或 open 失败的边界路径；
8. AI Collaboration 记录：context scope、关键假设、review point、validation result。

仅有组件单测、HTTP 200、WebView 存在、进程存活或 pane 可见都不能作为完成证据。

## 14. 实施分期

### Phase 1：受管状态与分屏骨架

- 扩展 browser session view/state event，带上 `owner_session_id`、状态、当前 host 与页面标识。
- Workspace 依据当前会话活动状态自动挂载/收起 pane。
- 实现 ≥1440px 的 480–720px 停靠宽度、键盘/指针调节与较窄窗口 overlay 降级。
- 先使用可验证的隔离页面 host 打通生命周期，不声明第三方站点全面兼容。
- 当前实现的 child WebView 是同 URL 独立预览，不等价于 Agent 的 ChromiumDriver 页面；该限制必须在交付证据中显式保留。

### Phase 2：真实内置页面与控制权

- 接入跨平台隔离 child webview/受管浏览器页面。
- 实现页面标签、接管/交还、重新 snapshot、权限提示与焦点治理。
- 验证登录、OAuth、下载和文件选择的兼容边界。

### Phase 3：恢复、证据与发布门禁

- 前端重载恢复、多会话并行、崩溃回收和 close receipt 完整闭环。
- 建立 macOS/Windows 真实应用 Viewport 与浏览器主路径验收。
- 若第三方网站存在平台限制，形成明确兼容矩阵和受控外部浏览器降级，而非静默 fallback。

## 15. 非目标

- 不提供无任务时可打开的通用浏览器。
- 不在输入框或全局导航增加常设浏览器按钮。
- 不直接复用或扫描用户日常 Chrome Profile。
- 不允许网页继承 CodeFactory 的 Tauri IPC 权限。
- 不承诺第一版兼容浏览器扩展、DRM、任意企业 SSO 或所有下载流程。
- 不因保留登录态而让浏览器进程长期常驻。

## 16. 兼容性与发布边界

这是跨平台桌面、权限、Profile、Viewport 和运行资源生命周期变更。正式发布前必须完成 macOS 与 Windows 的真实安装包验证；仅在 Vite、jsdom 或静态 WebView mock 中通过不得发布。若任一平台无法提供满足隔离要求的内置页面容器，该平台必须保持特性关闭并标记 `not live`，不能退化为共享 CodeFactory DOM 的不安全实现。
