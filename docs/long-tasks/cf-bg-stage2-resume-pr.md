## 接手说明
接手自失败会话 74b33b1e；在本受管分支合入 origin/codefactory/objective-62659a60d42b7600251fa849570bdafb（含 c1e8fffd），不向原分支推送。

## 范围
R2 / R5 / R5b；补齐原实现只提供 wake_source_cover 查询、未接入生产完成路径的缺口。

Scenario-Test: HLT-001, HLT-005, SKL-002, SKL-003
README-Update: reviewed
README-Update-Reason: 本次扩展会话后台工具，不改变安装、平台、隐私或 README 公开承诺。

## Req ↔ 测试与自验
- R2 触发/周期/取消/重启/上下限/上限/回收：background_tasks 的 due、repeating、cancelled、restart、bounds、cap、reclaiming 测试通过。
- R5 规矩：三种系统提示与 docs/agent-task-template.md 锚点测试通过。
- R5 门禁：新增 external_wait_requires_a_session_owned_wake_source，先因缺函数编译失败，再实现后通过；desktop_completion_guard_reads_real_ci_wait_and_requires_own_timer 覆盖真实生产适配器查询、跨会话隔离、取消后重新拦截、交付终态放行。
- R5b 来源：both_task_sources_are_recorded_and_visible 通过。
- 上下限负向对照：暂时禁用上限拒绝，bounds 测试按预期失败，恢复后 26 个相关测试通过。该对照不是原会话逐 Req 的先失败证据；原会话完整首红记录未能接手。

## 验证
- Rust workspace --no-fail-fast：最终 1721 passed、10 ignored，其他工作区测试通过。一次 SQLite 写锁计时抖动，原命令重跑通过，未改该测试。
- pnpm test：151 文件、1013 测试通过（含 lightModeAudit）。
- pnpm build：通过。
- 治理基线：通过。
- Windows 专属进程身份与进程树路径由 Windows CI 执行；本地 macOS 实际运行跨平台 shell、路径、回收测试，不声称本地执行 Windows 二进制。

## Applicable Harnesses
Spec：沿用权威规格；Compatibility：追加 SQLite 表、保留旧工具行为；Payload：提示边界与输出；Observation：触发/取消日志；AI Collaboration：如下。无 UI 改动，Viewport 不适用。

## AI Collaboration
context scope：阶段 2 任务、阶段 1 代码、已有提交与完成循环。assumptions：复用会话入队和已有权限通道。review point：发现 R5 生产接线缺失并补齐。validation result：上述测试；用户模板禁止 delegate_tasks，故单执行流。

## 真实应用验收分工
真机验收由编排方发版后完成（本任务不发版，not live）：安装后创建一次性/周期唤醒，重启验证补发，取消及停止会话确认不再触发；要求“盯 PR 合并告诉我”核对来源与回复；CI 失败检查唤醒后修复及审计记录。阶段 3 UI 留后续。

## 限制与 Spec Amendment
无需求含义变更。尚无真实模型驱动的合成 forge“失败后自动修复推送”和“合并后自然语言回复”端到端证据；现有 SQLite/sink 与生产适配器测试不冒充该证据。
