# M231 交付证据

Scenario-Test: HLT-001, HLT-004
README-Update: reviewed
README-Update-Reason: 内部 SQLite 锁竞争与等待来源恢复，不改变公开安装、隐私或平台承诺。

## 当前状态
本地功能验收已通过；PR、CI、合并及 §8 收尾以交付回执为准。无待决定 Spec Amendment。

## Req 与测试
- R1/R2：m231_concurrent_plan_revisions_are_serialized，已先复现 code 5；改为 BEGIN IMMEDIATE 后通过。
- R2/T1/T2/T6：m231_db_locked_limbo 集成测试使用真实文件 SQLite 和并发连接，锁分类先失败，修复后 4 项通过。
- R3/R4：m231_uncovered_wait_is_rejected_and_contention_is_bounded，先失败后通过。
- R3：m231_internal_contention_never_enters_model_tool_history，断言内部锁错误不进入模型工具历史，已通过。
- R2/T1：trajectory::tests::m231_real_tool_receipt_waits_for_writer_without_replaying_effect，通过真实生产轨迹 API 在文件数据库写锁下断言落库等待、释放后成功、内容匹配且副作用计数为 1，已通过。
- R5/R6：m231_patrol_replaces_expired_reason_without_reviving_terminal_sessions，已先失败，修复后通过。
- 兼容回归：等待派生层 90 项全通过；取消/停止等待原子性 4 项全通过。

## Harness 与协作记录
Spec、AI Collaboration、Compatibility、Observation；E2E-001 关联场景以校验器输出为准。
Context scope：工具轨迹、update_plan 事务、thin 状态和后台唤醒。
Assumptions：锁竞争属于内部可恢复状态，已发生的副作用不得重复执行。
Review point：已请求两项独立只读审查，但派发工具仅返回任务面板待启动状态，未获得实际审查结论；不把它计作已完成独立 QA。当前由本执行流完成代码审阅与回归证据核对。
Validation result：前端 163 个文件 / 1089 个测试通过（含 lightModeAudit），pnpm build 通过；统一 local 场景门禁、README 契约、场景声明与仓库治理基线均通过。完整 Rust 已在合入 M232 基线后全绿（主库 2254 通过、12 按仓库既有设置忽略，其余 workspace targets 与集成测试均通过）。随后合入版本 1.91.83 更新，并以相同命令复验全绿（Rust 主库 2254 通过、前端 163 文件 / 1089 测试、build 通过）。

## 真机验收分工
真机验收由编排方发版后完成。外部进程短暂持有实际数据库写锁，同时会话执行 update_plan/bash；核对自动继续、不产生原始锁错误工具结果。发版后一周审计 tool_calls 与无来源 waiting_wake 数量。
当前安装版 not live；本任务不自行发版。合并后必须完成模板 §8 收尾。
