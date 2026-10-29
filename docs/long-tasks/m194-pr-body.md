# fix: improve attached browser reliability (M194)

## 需求与证据
- R1：tab id/兼容序号解析；歧义先红测试 `m194_ambiguous_reference_fails_instead_of_selecting_another_page` 已由失败转通过；select_tab 经真实扩展 list/read 核对，保存页面信息。
- R2：只读操作有且只有一次重试；真扩展 36 秒闲置和断线重连验收执行中。
- R3：结构化错误码和 attach 能力清单；配对终态保持兼容。
- R4：登录页识别，LOGIN_REQUIRED 路由至既有用户等待；既有后台 observer 检查登录恢复。
- R5/R6：当前活动 HTTP(S) 标签只读截图；不激活标签、不聚焦窗口；四条独立测试先红（三项失败）后绿；保留 untrusted 页面边界与 click/fill/press 拒绝。

## Spec Amendment（尚未裁决，不冒称全部验收完成）
1. R6“所有失败路径回收租约”与 R2 重连、R4 登录后自动继续及仓库已有会话保留回归存在冲突：建议只回收失败创建/显式关闭/任务结束/TTL，临时读取错误保留租约。当前实现延续既有行为，等待编排方审查。
2. Chrome 是否安装扩展无法从“准备好的扩展目录”得知：当前无握手证据时返回 EXTENSION_NOT_PAIRED，而非虚构 EXTENSION_NOT_INSTALLED。建议允许未知安装状态；没有修改原始规格含义。
3. 截图权限修订：原 `<all_urls>` 方案违反既有 host 范围安全测试，已撤回；改为 HTTP(S) host + 用户主动调用扩展后临时授予的 `activeTab`。此前真 Chrome 截图通过仅证明旧权限版本，不能作为当前权限版本的截图成功证据；当前真实权限/截图链路待复验。

## Harnesses / AI Collaboration
- Spec：原文位于 docs/specs/feature-specs/m194-attached-browser-reliability.md；只实现 R1–R6。
- Compatibility/Observation：旧配对终态与 metadata、target 歧义、只读重试与可见登录交接。
- Payload：PNG header/base64 校验、项目相对路径、活动标签身份校验。
- Release：仅授权合并，不发版；not live，真机验收由编排方发版后完成。
- AI Collaboration：context scope 为扩展/工具/终态恢复；assumptions 只用合成页面，不访问内网；review point 为只读与权限边界；validation result 按下表。按 task-template 不调用 delegate_tasks。
- Viewport：无前端布局变化，不适用。

## 验证与诚实性
- pnpm test && pnpm build：通过。
- node --test scripts/attached-browser-reliability.test.mjs：4/4 通过；先看到 3 条失败。
- 专属 Cargo m194_ambiguous_reference 测试：先失败后通过。
- 完整 Rust workspace：同步主线前运行通过；同步主线后最终完整测试后台运行中，成功前不得入队。pnpm verify:extension：真 Chrome 截图、36 秒闲置后读取、断线重连全部通过。
- 重启前 33 条通过不能倒写成 R1–R6 全量 TDD。
- 尚未验证 canvas→view_image/OCR 的真实工具全链路，以及 Windows 安装版；不能据 unit/mock 或普通页面截图声称这些通过。

## 编排方发版后真机验收
Windows 已配对扩展：tabs→select_tab(id/序号)→read；闲置后读取；浏览器重启/断线恢复；SSO 登录完成自动恢复；canvas 截图→view_image；click/fill/press 拒绝；关闭任务无残留租约。无需本轮索要辅助功能权限或配对。

README-Update: required
README-Update-Reason: 说明只读截图及 Chrome 所需 host 权限，不改变导航/输入边界。
Scenario-Test: CXD-001, CXD-002, HLT-001, HLT-002, HLT-005, RTE-001, RTE-002, RTE-003, RTE-004
