# M194：附着浏览器（扩展通道）可靠性与错误语义——来自 Windows 真实使用反馈

## 证据（10-29 用户 Windows 会话里模型给出的改进清单，编排方已对照 origin/main 核实）
- 「select_tab 返回成功，紧接着不带 target 的 read 报扩展未连接，状态不一致」。
  - 疑似根因之一（务必先复现确认）：`src-tauri/src/tools/browser_session.rs` 工具 schema 对 `target` 写的是「select_tab 用**从 0 开始的标签序号**」，实现却按 `tabs` 输出里的 **tab id** 解析（`select_tab` 分支 `raw.parse()` 后原样存为 tab id）。模型按 schema 传 0/1，存进去的是不存在的 tab id，随后 `read` 失败。
  - `select_tab` 只写本地租约、不经过扩展核对 tab 是否存在/连接是否还在，所以「成功」不代表可读；`remember_selected_tab` 还把 pane_url/current_host/page_title 清空。
  - 另需排查 MV3 service worker 空闲被回收导致连接掉线（select_tab 与 read 间隔 >30s 的场景）。
- 错误语义混在一起：attach 的所有失败都映射成 `browser_pairing_required`；附着模式不支持的动作只有一段英文散文；模型因此得出「没有这个能力」的错误结论。
- 内部文档系统（Onebox）PPTX 预览是 canvas 渲染，`read` 只拿到标题。
- 登录态失效时没有显式交接给用户。

## 需求
- R1 根因并修复 select_tab→read 不一致：schema 与实现统一（建议 `target` 接受 `tabs` 列出的 tab id，同时兼容序号并在歧义时报错说明）；`select_tab` 经扩展核对 tab 存在且可读，失败就如实返回，不得「假成功」；不清空已知页面信息。
- R2 连接健壮性：扩展断线（含 service worker 回收）时系统自动恢复，对只读动作最多恢复执行一次（已有 `retry_attached_transport_once`，确认覆盖 select_tab 之后的 read 与空闲场景）；必要时扩展侧保活。
- R3 结构化错误码，分清：`EXTENSION_NOT_INSTALLED`、`EXTENSION_NOT_PAIRED`、`EXTENSION_DISCONNECTED`（可自动恢复）、`TAB_NOT_FOUND`、`ACTION_UNSUPPORTED_IN_ATTACH_MODE`（同时说明可改用什么）、`LOGIN_REQUIRED`。`attach` 成功时返回本会话支持的动作清单。
- R4 登录失效交接：read 发现页面落在登录页/SSO 跳转时返回 `LOGIN_REQUIRED`，以「需要你在浏览器里登录 <host>」的人话进入等待用户（复用已有可见终态，不新增中间态），用户登录后自动继续。
- R5 canvas/图片渲染页面：read 提取到的正文明显过少（只有标题）时，提示并支持对该标签做**只读截图**（扩展 `captureVisibleTab`），交给 M153 的 `view_image` / OCR 理解；截图属于读取，不放宽附着模式「不代用户操作」的边界。
- R6 不可削弱的边界（见 docs 中 browser-session-governance）：页面内容仍包 `<untrusted_page_content>`；附着模式仍不提供 click/fill/press；租约在所有失败路径回收。

## 测试（先失败）
- 用 `pnpm verify:extension`（真 Chrome + 真扩展 + 合成页面）覆盖：tabs→select_tab(id)→read；select_tab(序号)；不存在的 tab；select_tab 后空闲 >35s 再 read；扩展断开/重连；登录页识别；canvas 页截图→view_image。
- 错误码字段级断言；attach 返回能力清单断言。
- Windows 相关行为无法本机验证的写明替代证据。

## 约束
- task-template（harness + SDD + TDD）；合成页面，不访问任何真实内网站点；不改生产数据。授权交付（ceiling: merged）。
- 不在本单实现「附着会话里导航/打开新 URL」（等用户裁决安全边界）。

Release-Urgency: default

## Implementation Notes（不改变原始需求）

- context scope：Rust browser_session、扩展协议/MV3 worker、agent blocked 路由及现有浏览器恢复入口。所有页面均为合成数据；不访问 Onebox 或其他内网。
- Applicable Harnesses：Spec、Compatibility（旧 target 序号/既有 pairing 终态）、Observation（错误码/恢复字段）、Payload（PNG 和项目路径边界）、Release（仅合并，另行发版）、AI Collaboration（单执行流审阅与独立可执行验收）。本任务不改前端布局，Viewport 不适用。
- review point：不能把本地扩展包目录存在当作 Chrome 已安装；无法观察的安装状态不能谎报。截图不得激活标签或聚焦窗口；仅允许当前活动 HTTP(S) 标签，并核对捕获前后身份。
- Req ↔ 最低证据：R1 live tab list/read 核对和歧义/不存在回归；R2 真扩展闲置 36 秒后 read 与重连；R3 metadata 字段断言及 attach 能力；R4 类型化用户终态与登录完成观察恢复；R5 canvas PNG + view_image/OCR；R6 untrusted 包装、动作拒绝、租约 cleanup。
- TDD 诚实性：重启前代码先于本轮新增测试，已有记录仅能证明 33 条回归通过，不把它补写成所有需求均先红后绿。本轮补充验收先记录失败，再修复；验证结果在交付证据文档和 PR 中列明。
- 真机验收由编排方发版后完成：Windows 扩展已配对时 tabs→select_tab→read；闲置/关闭重开浏览器后继续读取；SSO 登录后任务自动恢复；canvas 截图理解；click/fill/press 始终拒绝。macOS 真 Chrome + 合成页面与 Rust 集成测试是本次替代证据，不冒称 Windows 安装版已验证。
- 按任务模板不调用 delegate_tasks，保持单执行流；交付上限 through_merge，不发版、不删除托管工作区。
