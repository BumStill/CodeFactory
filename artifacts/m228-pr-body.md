## M228 修复
到点模型唤醒使用持久 root 接续原 Objective；准入错误脱敏落库与审计，5 次失败可解释、30 次失败终止；忙会话不消耗拒绝预算，ended 取消并禁止重挂模型唤醒。

Scenario-Test: HLT-001, HLT-004
README-Update: reviewed
README-Update-Reason: 内部唤醒恢复修复，不改变公开产品、安装、隐私或平台承诺。

## Req ↔ 验收
- R1：m228_model_wait_wake_continues_answered_settled_objective。合法结算夹具重跑后先见 Other("system wake has no unfinished objective to continue")，再修复；复用原任务，不创建新工作区。
- R2：m228_busy_is_not_rejection_and_errors_are_redacted，验证 last_error 与 session_audit_events 保留错误原因并去除 token。
- R3：m228_rejected_wakes_explain_and_terminate_after_thirty_attempts，先在第 5 次 waiting_for 断言失败；修复后验证 5 次提示、30 次 ended 和 30 条审计。忙会话 35 轮不计失败。
- R4：m228_ended_session_does_not_rearm_startup_interruption，先见 ended 后仍有 pending，修复后验证取消及启动阶段中断不新增行。
- R5：m228_due_wake_starts_chatgpt_after_reset_with_daily_fallback_capped，生产调度、准入事务、额度路由和持久审计的合成集成验收；不等价于真实 ChatGPT 网络验收。

## Applicable Harnesses / AI Collaboration
- Spec：原文保存于 docs/specs/feature-specs/m228-wake-admission-silent-failure.md，无 Spec Amendment。
- Compatibility / Observation：真实 SQLite 迁移夹具、原 Objective 身份、错误脱敏、终态取消与阈值字段断言。
- Release：本单授权仅至 merged；Release-Urgency: immediate；发版由另一任务完成，目前 not live。
- AI Collaboration：context scope 为 background_tasks / objective / thin_state / chat；assumptions 为模型等待通知不等价于任务完成；review point 为持久唤醒 root、ended 与忙会话边界；validation result 见验证记录。模板要求单执行流，未调用 delegate_tasks。
- 无布局改动，无新增 GUI-only 操作，调度和 dispatch 后台入口保留。不适用 Viewport 截图。

## 真实应用验收交接（§8）
真机验收由编排方发版安装后完成：构造短期 model-availability 唤醒，ChatGPT 重置且 DeepSeek 达每日上限，到点后 2 分钟内自动新回合，核对 chatgpt 路由尝试、wake-delivery 清除；核对 ended 会话不再新增 model-availability-*-re 行。无需人工发消息，不修改用户数据库。

## 验证记录
- pnpm test && pnpm build：初次通过；最终基线重跑见后续记录。
- 修复前失败中夹具 CHECK 和唯一性约束已修正，未通过降低产品断言绕过；之后复现实际 system wake 拒绝原因。
- 生产库只读核对多条 wake-delivery 累计百余次泛化错误；不将工作区 ID 误作会话 ID，不持久化真实会话数据。

Release-Urgency: immediate
