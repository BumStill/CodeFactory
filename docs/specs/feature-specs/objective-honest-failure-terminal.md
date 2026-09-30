# 诚实失败终态（取代「恢复耗尽」中间态）

- 状态：已批准实施
- 取代对象：`objective-recovery-control-plane.md` 的 CF-ORC-R35（system incident pause 出口）、CF-ORC-R37（`technical_recovery_exhausted` 保持 `waiting_system`）、CF-ORC-R41（Objective 与 DeliveryRun 的等待生命期）
- 上级原则：恢复必须持续持有**可恢复**的技术状态。不能恢复的状态不属于「可恢复」，持续持有它不是持有目标，而是空转。

## 背景与用户决策

系统自发恢复达到上限时，旧实现把 objective 停在 `waiting_system + failed_internal + failure_code=technical_recovery_exhausted`，并开一条 `waiting_capability` 的 incident；唤醒条件是 `recovery_capabilities.revision` 增长，而 `RECOVERY_CAPABILITY_REVISION` 恒为 1、注释明确写明安装新版本不构成能力变化，因此该唤醒**永远不会发生**。用户实际看到的是：

- 一句话看不懂的提示（「自动恢复已达到安全上限，已登记为系统故障……会在恢复策略或能力更新后续接同一目标」）；
- 一个不会自己动、也没有交付入口的状态；
- 未提交的改动留在执行工作区里，界面既看不到，也无法交付；
- 关联 DeliveryRun 停在 `await_system_capability_change`，顶栏长期显示「未验证上线」。

用户已明确推翻该设计（原话：「什么叫恢复耗尽……只有所有方式搞不定就说失败不就完了，为什么要造一个用户没法理解的事」）。

## 决策

系统自己搞不定时，用户看到的结局只能是两种：

1. 换一种办法继续做（第二个 PR 的范围）；或
2. 所有办法都试过后明确说「没做成」，并交代试了什么、为什么停、做了一半的东西在哪、下一句话怎么接着干。

不允许再出现用户看不懂、也永远不会自己动的第三种状态。

## Requirements Traceability

| Req ID | Requirement | Minimum evidence |
| --- | --- | --- |
| CF-OHT-R1 | 系统自发恢复达到上限时，objective 进入终态 `failed`（与 `completed`/`cancelled` 并列，`is_terminal()` 为真），不再进入 `waiting_system`。同一 SQLite 事务内：incident 关账（`resolved`）；未结束的 tool_calls 以诚实文案结束；`chat_turn_state` 进入终态（`status='completed'`、`terminal_reason='objective_failed'`、`next_action=NULL`，不显示运行中、没有停止按钮）；run-control 终止。会话立即可输入，下一条消息正常执行，不进入「当前执行结束后发送」队列 | objective 终态/事务原子性/turn 终态/run-control 单测 + 真实 app 复现 |
| CF-OHT-R2 | 终态写一条对用户可见的 assistant 消息，由结构化数据生成：①这件事没做成（一句话说清目标）；②试过的办法及各自为什么失败（按「办法」逐条列举、同类合并计数，为「换办法」留结构）；③保留下来的成果（执行工作区相对基线的未提交改动，文件列表 + 增删行数，最多 10 个文件、其余写「等 N 个文件」，以及已开 PR 链接与状态）；④下一步（「继续」/「换个办法」/「把这些改动交付」）。文案不得出现内部词汇：恢复耗尽、安全上限、系统故障、登记、能力更新、incident、objective、remediation、generation、recovery 等；不得以 `role=user` 写入 | 渲染纯函数单测（含禁用词守卫）+ SQLite 断言消息 role/内容 |
| CF-OHT-R3 | objective 进入 `failed` 时，关联 DeliveryRun 在同一事务进入终态失败：撤销 mutation authority、清空 lease、写明原因（`failure_code/failure_class='objective_failed'`），不再停在 `await_system_capability_change`。之后同一会话里用户要求「把改动交付」时，新的 `deliver_changes` 在同一工作区、同一分支上必须能正常进行：新 run 取代旧 run 并写审计事件，不被旧 run 的 identity conflict 挡住；任何时刻仍最多一个可写（mutation-capable）run。只作用于 DeliveryRun 自身的 `platform_incident` 也按同一原则：要么能继续，要么终态失败并给出原因 | linked Objective/DeliveryRun 事务 + 新 run 取代旧 run 审计事件 + 单写者不变量测试 |
| CF-OHT-R4 | app 启动时幂等收口存量中间态：`waiting_system + technical_recovery_exhausted`（以及历史的 `waiting_core_input + core_input_required + technical_recovery_exhausted`）转为 `failed` 并补一条 R2 总结（每个 objective 只补一次，重启不重复）；incident 关账；残留的 `active`/`waiting_system` turn 改终态；objective 已是终态而 incident 仍 open 的直接关账；停在旧等待态的 DeliveryRun 转终态失败 | 合成夹具（objective + incident + turn + delivery run）启动收口后全部终态 + 第二次启动零写入 |
| CF-OHT-R5 | 前端删除 `technical_recovery_exhausted` 的等待文案；失败终态回合按正常结束的回合渲染：没有运行时钟、没有停止按钮，输入框直接可用 | 组件单测（含「不显示停止按钮」与等待文案不再出现）+ 真实 app |
| CF-OHT-R6 | `objectives.status` 新增 `failed` 必须是 additive 且幂等：SQLite 不能原地改 CHECK，因此以「读活 DDL、只重写一个子句、重建表」的方式拓宽，并在迁移文件中同步新装库的词表；旧库、旧夹具和回滚版本都必须可读 | fresh/old/夹具三种库的 widening 幂等测试 |

## 与既有规格的关系

- CF-ORC-R35 的「有界、按代际累计、用户驱动不计预算」继续有效；只有「上限达成后进入 system incident pause 并等待能力版本变化」被取代。
- CF-ORC-R37 的事务原子性与 `terminal_revision`/`visible_final_message_id` 语义继续有效；只有「`technical_recovery_exhausted` 保持 `waiting_system`」被取代。
- CF-ORC-R41 的「Objective 与 DeliveryRun 不得拥有彼此矛盾的恢复生命期」「`core_input_required`/业务决策/拒绝取消不得被后台重新认领」「最多一个可写 run」继续有效；只有「以 `technical_recovery_exhausted` 等待态作为交付生命期终态」被取代。
- `durable-delivery-recovery.md`、`session-control-convergence.md`、`business-capabilities.md` 中引用该等待态的段落按同一原则理解：不再存在无人会改变的等待。
- 旧测试契约（断言 `waiting_system` / `platform_incident` / 通知只写一次 / 能力版本反激活）随本决策一并取代。

## 效果范围与后续

- 本规格只做第一步：把中间态换成诚实的失败终态，并保证做完的成果看得见、接得上、交得出去。
- 「一个办法不行自动换另一个办法」是第二步，不在本次范围；R2 的逐条「试过的办法」结构即为它预留的输入。
- 交付边界：PR 只开不合（`pr_only`），合并与发版由人工决定。
