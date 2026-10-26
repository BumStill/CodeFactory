# M174（高）：PR 合并后会话必须被唤醒收尾并结束，不能永远 waiting_wake

## 证据（v1.91.35，含 M168 #867「PR 合并作为唤醒条件」）
- 会话 c8e12ca7（T1）：PR #872 于 10-26 ~14:0x 合并，14:30 仍 thin_session_state=waiting_wake，reason「PR #872 还没合并，系统会自动跟进，不需要你回复」。
- 编排方派过的会话中 13 个处于 waiting_wake，绝大多数 PR 已合并多日：dd5ed203(#854)、a9e95bb0(#853)、019a75d8(#857)、9433dd84(#858)、b88017e6(#859)、d7260fdf(#862)、b1f64fd7(#864)、39c8a4f2(#861)、7ff79f98(#867)、dad2db79(#869)、a0758ba1(#870)、c8e12ca7(#872)。
## 需求
- R1 PR 合并（含 squash、合并队列合并、编排方 gate-dance 合并、接手他人分支的 PR）后，会话被唤醒执行 §8 收尾（清理工作区/分支）并进入 ended；reason 文案与事实一致（不再说「还没合并」）。PR 归属按会话实际推送/交付过的分支与 PR 号判定，而不只看本会话登记工作区的分支。
- R2 启动时对所有 waiting_wake 会话核对其 PR 实况：已合并/已关闭的直接触发收尾或结束（一次性修复存量）。
- R3 测试（先失败，跨进程）：PR 在会话等待期间被合并 → 会话被唤醒并 ended；接手他人分支的 PR 合并 → 同样；应用重启后存量已合并会话被结算。
## 约束
- task-template；合成 fixture；授权交付（ceiling: merged）。

Release-Urgency: immediate

## Implementation Notes
- 保留原始需求，不改变验收含义。采用现有持久 PR 观测与唤醒投递器；修复已完成观测被旧 waiting_wake 投影重新覆盖的路径。
- Harness：Spec、Compatibility（跨进程及历史会话）、Observation（PR 事实与文案）、Release（合并及 §8 收尾）、AI Collaboration。
- 真机验收由编排方发版安装后完成：等待 PR 时外部 squash 合并、接手分支合并、重启存量已合并/关闭会话，核对收尾、ended 与事实文案。本任务不发版。
