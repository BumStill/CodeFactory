# M202（紧急）：「等 PR 检查结果」的 PR 监视心跳不是真实唤醒源——CI 失败 15 分钟无人知晓

## 本机证据（v1.91.57，含 M199，10-29 19:23–19:38）
- 会话 edeb347f（M195）thin=waiting_wake，waiting_for「等 PR #921 合并/CI」→ 后改「系统自动重试/自愈：接续 PR 检查结果」，wake_source「系统 PR 监视（心跳，间隔不少于 180 秒）」。
- PR #921 check-rust 19:23 失败；到 19:38 会话 0 条新消息。`wakeup_schedules` 无 pending（唯一的 thin-repair-095b42ee 在 18:49 已 cancelled、fire_count=0），无 running background_tasks。
- 与 M197/M199 同族第 3 次：M199 只把「收尾」分支改成事实型唤醒，「等 PR/CI」分支仍写文案型来源。

## 需求
- R1 家族收口：把 thin_derive 里**所有**返回 WaitingWake 的分支列成表，每个分支必须在同一事务内绑定一个可查询的事实（wakeup 行 id / 后台任务 id / PR 监视订阅行）。加一条全局不变量测试：对任意 fixture 派生出 WaitingWake 而查不到对应事实 → 测试失败（而不是逐分支补）。
- R2 PR 监视：若真有心跳轮询器，它必须持久登记订阅（会话、PR、上次检查时间、下次检查时间），CI 结论变化（失败/通过/冲突/合并）后 ≤ 5 分钟唤醒会话；共享 GitHub 限流预算，间隔 ≥180s。
- R3 到期必触发：pending 唤醒过点 >2 个周期仍未触发要记 session_audit_events 并补触发（10-29 a9966461 有 3 条过点唤醒从未触发）。
- R4 回归测试（先失败，合成 fixture + 假 GitHub 响应）：PR 检查由 pending→failure → 5 分钟内会话收到续接回合；重启后订阅仍在。登记 `docs/testing/scenario-mappings.json`。

## 约束
- 同族第 3 次复发：先做家族审查（见 R1），PR 描述附分支表。task-template（harness + SDD + TDD）；合成 fixture；不改生产数据。授权交付（ceiling: merged）。

Release-Urgency: immediate

## Implementation Notes
- Harness：Spec、Compatibility、Observation、Release、AI Collaboration；无布局变更，不触发 Viewport。遵照模板不使用 delegate_tasks，单执行流。
- R1 分支表：未知执行者；活后台任务；已合并/关闭待收尾；失败检查待修复；未完成交付引用；后备交付引用；未来定时唤醒；后台结果/到期唤醒；历史等待重新认领 PR；历史系统等待；中断回合恢复。全部经过统一返回边界，以事务绑定具体持久行并保存投影，禁止仅返回字符串来源。
- R2 复用现有 PR 观察器和共享 GitHub 预算，订阅行记录 observed_at 与 next_check_at；PR 观测变化仍经持久唤醒投递器进入续接回合。
- R3 复用每秒扫描及至少一次投递，补充超过两个周期的审计证据。
- R4 验证矩阵：无源各类等待、已有来源幂等、pending→failure、重复扫描去重、持久重启、超期审计及补投递。
- 真机验收由编排方单独发版安装后完成：打开等待 PR 的会话，使合成 PR 检查由 pending 变为失败，5 分钟内核对续接消息；重启后核对订阅仍在及 next_check_at；核对超期审计与 fire_count。本任务不发版、不改生产数据。
