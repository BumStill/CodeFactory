修复派单发版紧急度丢失：会话持久保存 release_urgency，thin 交付在模型漏写时自动补齐 PR 尾段，设置面板允许查看和修改。

## 需求与验收
- R1/R4：desktopDispatch.test.ts 三值透传与非法值拒绝，先见 2 项失败，修复后 21 项通过。
- R2/R4：m137_session_policy_reaches_created_and_updated_pr_bodies 覆盖 default/immediate/hold 的 create/edit、null 不降级、显式优先及模型非授权 hold 拒绝。
- R3：ReleaseUrgencyPicker.test.tsx 先见组件缺失失败，后 2 项通过；保存错误保留原值。
- pnpm test：156 文件、1044 项通过（含 lightModeAudit）；pnpm build 通过。dispatchActions.test.ts 的新增 R1 用例确认显式值持久化、后续省略不覆盖。
- 真实 Chromium：light/dark × 1366/390，选择 hold、字段值、44px 高度与无横向溢出断言通过。截图 artifacts/m137/headless/{light,dark}-{1366,390}.png，receipt.json。
- Rust 全 workspace 测试：最终命令已通过；m137_session_policy_reaches_created_and_updated_pr_bodies 和 m137_dispatch_policy_survives_database_reopen_and_omitted_followup 均通过，真实 SQLite 重连后保留 hold、旧会话默认 default。修复过测试私有模块导入错误，未扩大产品 API。
- R1/R2/R3/R4 自验结果：pass。Spec Harness 的原文与实现备注在 docs/specs/feature-specs/m137-release-urgency-from-dispatch.md。
- 场景映射按模板常设授权作最小增补：CXD-002 和 UI-015 新增精确路径和真实测试；若 trusted runner 报 cannot self-modify，由编排方处理，不削弱门禁。

## Harness 与协作记录
Spec：版本化 M137 原文；Compatibility：旧会话 default、派单省略不覆盖；Release/Observation：thin create/edit 正文断言；Viewport：真实 Chromium 明暗/宽窄布局；AI Collaboration：context scope 为 dispatch/会话/thin 交付，assumptions 为不新增发布触发，review point 为持久化、优先级和 hold 来源，validation result 如上。用户模板禁止 delegate_tasks，采用单执行流。

真机验收由编排方发版后完成：派单 immediate，重启并续聊后交付，核对 PR 尾段；会话模型设置切换 default/hold 并重开核对。当前 not live，本任务只交付合并，不发版。
Spec Amendment: 无。

Scenario-Test: CXD-002, DAT-001, HLT-001, HLT-002, HLT-005, RTE-003, RTE-004, UI-015
README-Update: reviewed
README-Update-Reason: 会话级紧急度不改变 README 的公开安装、平台或隐私承诺。

Release-Urgency: immediate
