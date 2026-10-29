# M202 交付验收记录

Scenario-Test: HLT-001, HLT-004, RTE-003, RTE-005
README-Update: reviewed
README-Update-Reason: 后端等待恢复修复，不改变公开安装、隐私、平台承诺。

## Req ID 与测试
- R1：m202_all_waiting_fixtures_require_queryable_source，已先失败（PR 等待无持久事实），修复后通过。
- R2/R4：m202_pending_failure_subscription_survives_restart：合成 pending→failure，持久重启与 5 分钟内投递断言。
- R3：m202_overdue_wake_is_audited_and_delivered，已先失败（审计计数 0），修复后通过；补投递并写 session_audit_events。
- 兼容回归：m151_reset_makes_a_stuck_session_usable_without_losing_content 在全量测试先失败，修复 reset 对具体事实型来源的识别后原命令通过，重置时取消自己的恢复唤醒。
- 已完成验证：前端 163 文件/1086 测试通过，pnpm build 通过；thin_derive 52 项、background_tasks 34 项通过；场景/README/治理基线通过。Rust workspace 全量在最终兼容修复后复跑，以该结果及 PR CI 为准。
- 构建资源：遵从仓库共享 Cargo 缓存及预算入口，未创建任务专属 target，不删除共享产物。

## 全部分支表
| WaitingWake 分支 | 可查询事实 | 上限/周期 |
| --- | --- | --- |
| 未知执行者 | pending wakeup id | 自愈最多 2 次 |
| 活后台任务 | background_tasks id | 原任务超时 |
| 合并/关闭待收尾 | pending thin-closeout/thin-repair id | 原收尾门禁与 2 次上限 |
| 失败检查待修复 | PR 订阅或 pending id | ≥180s |
| 未完成交付 | PR 订阅 session/PR | ≥180s |
| 后备交付引用 | PR 订阅 session/PR | ≥180s |
| 未来定时唤醒 | pending wakeup id | 原计划 |
| 后台结果/到期唤醒 | background_tasks/pending id | 每秒投递 |
| 历史等待认领 PR | PR 订阅 session/PR | ≥180s |
| 历史系统等待 | pending wakeup id | 自愈最多 2 次 |
| 中断回合恢复 | pending wakeup id | 自愈最多 2 次 |

统一 derive_state 返回边界在事务中绑定具体事实与投影。PR 订阅记录 observed_at、next_check_at，复用现有共享 GitHub 预算及 CAS，不增加旁路轮询器。

## Harness / AI Collaboration
- Spec：原文规格及 R1–R4 追踪。
- Compatibility：旧 schema、取消、重启、完整 thin 派生测试族。
- Observation：字段断言、具体行引用、fire_count、审计。
- Release：授权仅到合并，单独发版，不修改生产数据。
- AI Collaboration：context scope 为 thin_derive/background_tasks/场景映射；assumptions 为现有 runtime 持续对账投递；review point 为统一返回边界、取消保护和有界恢复；validation result 随最终测试更新。按模板不调用 delegate_tasks，单执行流。

## 真实应用分工与收尾
- 无 UI 布局变更，不适用截图验收。
- 真机验收由编排方发版后完成：安装后打开等待 PR 会话，pending→failure 后 5 分钟内核对续接消息；重启检查订阅和 next_check_at，核对超期审计及 fire_count。
- not live：本任务不发版；合并后另行发布安装验收。
- Spec Amendment：无。

Release-Urgency: immediate
