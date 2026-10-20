# M124：受管浏览器会话里 `browser_session tabs` 报「only applies to the user's own browser」

## 证据（10-19 两次真机验证）
- 会话 7f531297、d89282ac（thin，只读浏览器任务）：`open` 后调用 `tabs`（带受管 session_id）返回 error「browser_session.tabs only applies to the user's own browser — use browser_session.attach first.」模型随后改用 snapshot/find 才完成。
## 需求
- R1 受管会话支持 `tabs`（列出该受管浏览器的标签页：url/title/active），与扩展通道的 tabs 输出格式一致；或者若产品上刻意不支持，则工具描述与错误信息明确写「受管会话请用 snapshot/read」，且不计为工具失败。优先 R1 前半。
- R2 两条通道（扩展读已登录页 / 受管一次性 profile）的安全边界不变（见浏览器控制规格）。
- R3 测试（先失败）：受管会话 open 后 tabs 返回当前页；扩展通道 tabs 行为不变。
## 约束
- 合成 fixture。授权交付（ceiling: merged）。

## Implementation Notes

- 受管会话当前仅驱动一个页面；`tabs` 返回该页面，使用会话内索引 0，active=true；不引入切换页面或访问其他会话的能力。
- 两通道共享 `format_tabs`，保留原有 id/title/active 前缀并补充 URL。受管 `select_tab` 仍保持拒绝；扩展通道仍只读。
- Requirements Traceability：R1/R3 对应 `thin_browser_open_reaches_driver_without_receipt`（真实本机 HTTP fixture + Chromium）；R2 对应 `the_users_own_browser_stays_read_only`、`tab_selection_does_not_apply_to_the_managed_browser` 与扩展 bridge 测试。补充共享输出字段断言。
- Applicable Harnesses：Spec、Compatibility、Observation、Release（仅合并交接，不发版）、AI Collaboration。无前端 UI 修改，不触发 Viewport。
- AI Collaboration：context scope 为任务原文、浏览器工具/driver、既有控制规格；assumptions 为每个受管会话仅有一个受控页；review point 为会话隔离、扩展只读与 untrusted 包装；validation result 在 PR 中记录。按任务模板不调用 delegate_tasks，采用单执行流。
- 真机验收由编排方发版后完成：安装新版本，依次执行 open/tabs/read/close，核对当前 URL、title、active；attach/tabs 核对既有登录页且禁止写操作。
