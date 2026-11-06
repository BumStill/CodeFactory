## 修复内容
M241：已结算 objective 的迟到系统唤醒不再起新回合；同一事务结束 waiting_wake 并取消整个会话的 pending schedules 与后台完成通知。

Scenario-Test: HLT-004, RTE-005
README-Update: reviewed
README-Update-Reason: 内部会话恢复缺陷修复，不改变公开安装、平台、隐私与功能承诺。

## Req ID 与 TDD / 自验
| 要求 | 测试与结果 |
| --- | --- |
| R1/R2/R3/R6 | `m241_settled_objective_cancels_wake_family_without_admission`：真实 SQLite，completed/cancelled × repair/closeout/source-repair/continue/model-availability/scheduled；扫描后 ended、未来与到点唤醒取消、不写 user 消息、不新增目标/工作区；修复前 completed/thin-repair 得到 1 次投递而预期 0，修复后通过 |
| R4 | 同一测试通过正常 User 目标准入，在 ended 后创建新目标 |
| R5 | `m241_unfinished_objective_delivers_normally` 未结算目标正常投递；`m241_objective_query_error_does_not_end_session` 查询列错误不投递、不结算、保留 pending；后者修复前错误投递 1 次，修复后通过 |
| R6 | `m241_background_completion_is_cancelled_without_admission` 后台任务通知共用结算；`m241_model_wait_still_resumes_original_objective` 保留 M228 原任务模型恢复 |

首次测试夹具错误（缺少根回合、真实 NOT NULL 字段、终态 CHECK 证据）已按迁移结构补齐，没有放宽行为断言。

## Applicable Harnesses
- Spec：首个提交保存任务原始 R1–R6 规格与实现说明。
- Compatibility：不改 schema、不改 objective.rs 迟到唤醒准入限制；保护未结算目标、未回答用户任务、模型恢复与未合并交付。
- Observation：ended reason 持久化真实原因，清除等待字段；查询失败指数退避，不使用耗尽预算误结算。
- Release：仅授权交付到合并队列，不发版；not live。
- AI Collaboration：context scope 为任务文件、模板、唤醒调度与目标准入代码；assumptions 为现有 M209/M228 接续不被破坏；review point 为事务原子性与查询错误不等于无任务；validation result 见测试。模板明确禁止 delegate_tasks，使用单执行流。

## 验证
- 前端 `pnpm test`：166 files / 1117 tests 通过；`pnpm build` 通过。
- M241 Rust 真实 DB 回归：5/5 通过（包括 completed/cancelled × 6 种来源、后台完成通知与模型恢复保护）；全工作区回归结果持续更新。
- Python 3.11 统一 `run_scenario_harness_gate.py --stage local`、治理基线、PR `--ci` 场景声明与 README 合约均通过；系统默认 Python 缺 tomllib，已用已安装的 3.11 重跑成功。
- `pnpm test:background-tasks:headless` 已尝试，Chrome 被宿主 macOS 沙箱 MachPortRendezvous 权限拒绝；托管浏览器尝试连接已退出的服务返回 ERR_CONNECTION_REFUSED。无 UI 修改，本地浏览器验收未通过，不伪报成功。
- 真机验收由编排方发版后完成：安装新版本，下一单任务经队列合并后核对自动 ended、wakeup_schedules 无 pending、全程无人工 stop；再发送新用户消息确认正常新回合。

## Spec Amendment
无。M209/M228 与未合并交付保护属于保留既有任务事实，不扩大系统唤醒授权。

## §8 收尾
合并后核对 merged=true、squash commit 在 origin/main、远端分支状态、工作区无未提交/未推送改动；清理任务构建缓存与临时验收文件，不删除托管工作区。结果在最终报告记录。
