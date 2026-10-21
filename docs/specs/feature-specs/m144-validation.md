# M144 验收与交付记录

## 修复
- R1：发送/继续、停止及关闭的 deferred 事务改为共享 BEGIN IMMEDIATE、忙等待和有限重试；聊天错误边界隐藏 SQLite 锁诊断。
- R2：保留停止栅栏；无交付记录不提 PR，按输入轮次幂等保存一条提示；审计 notes 留日志，远端撤回部分失败如实说明。
- R3：write_file null/缺字段以 ToolOutput 返回具体字段、expected string 和重试方式，不抛出调用异常。

## Req ↔ 测试与验收
|需求|结果与证据|
|---|---|
|R1/R4|m144_admission_retries_a_real_concurrent_writer：先真实 code 5 失败、后通过；chatEvents.storage.test.ts 三个锁诊断先失败，修复后 4/4 通过|
|R2/R4|m144_stop_notice_is_plain_and_delivery_specific：先内部用语断言失败、后通过；m144_stop_without_delivery_is_plain_and_idempotent_per_turn 验证无交付、并发重复停止、新轮次再次停止；r2_a_stop_closes_the_arms_on_the_sessions_pr 验证合成 PR 双撤回，部分失败测试保留|
|R3/R4|m144_write_file_invalid_fields_are_actionable_tool_results：先失败、后通过；覆盖 null/缺字段、工具错误及无文件写入|

完整验证：
- pnpm test：157 文件、1048 测试通过，含 lightModeAudit。
- pnpm build：通过。
- CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-m144" cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast：通过，主库 1877 通过、12 个既有 ignored，workspace 其余单测、集成、doctest 均通过。
- 首次全量发现旧长文案断言不符合 R2，仅更新文案断言，最终同一完整命令重跑通过。
- node scripts/verify-m144-storage-headless.mjs：通过。真实 Chromium 生产 MessageList 与错误呈现函数，light/dark 断言提示正文、停止计数、无内部诊断及无横向溢出。截图目录为系统临时目录 codefactory-m144-evidence，文件 m144-light.png、m144-dark.png。
- 场景 --ci、README contract、统一 local harness、baseline 均通过；同一场景校验函数额外检查真实未提交文件集通过。

## P0 追加验证（永久 database is locked）
|需求|结果与证据|
|---|---|
|R5/R6/R9|m144_returned_raw_transaction_cannot_poison_later_writes：在 1 连接池上用裸 BEGIN IMMEDIATE 写入后丢弃连接，模拟 COMMIT 失败/取消；连接归还清理回滚脏写，后续 begin_write 成功，表中只剩新写入。|
|R6/R9 负向对照|m144_without_release_cleanup_the_lock_would_stay_poisoned：无 after_release 的裸池上同一泄漏使裸 BEGIN IMMEDIATE 持续失败；rollback_idle_transactions 返回 1 后 begin_write 恢复并成功提交。若清理或自愈失效，该用例失败。|
|R7/R9|m144_failed_contended_write_does_not_disable_later_writes：busy_timeout=0 制造一次真实 code 5，释放持有者后同一连接池的下一笔写入成功，无需重启。|
|R9 Windows|m144_parallel_writers_all_commit_on_every_platform：8 写者 × 5 轮并发事务全部提交（40 行），运行于 check-rust 的 windows-latest。|
|R8|node scripts/validate-write-transaction-scope.mjs：扫描生产 Rust 源码，事务作用域内出现 Command::new、reqwest、model_transport、provider 流或子进程即非零退出；当前通过。|
|R5 覆盖|ensure_objective_failed_status 的 schema 重建由裸 BEGIN/COMMIT 改为 conn.begin_with RAII；ObjectiveStore 14 处、delivery_run 13 处、persistence 3 处、thin_derive 1 处写事务统一走 begin_write。|
|R7 有界等待|pool_builder 增加 acquire_timeout(30s)；busy_timeout 10s × 3 次重试后进入自愈回收。|

多池核查：生产主库写路径均经 storage::db::connect（同一 WAL/FULL/FK/busy_timeout）；独立库路径 delivery-recovery smoke 也使用 connect；`--dispatch` 不另建主库池。Windows 上 WAL 仍走 SQLite 统一锁语义，差异由 acquire/busy 超时与归还清理覆盖。

## Harness 与协作
Spec：原任务规格位于 docs/specs/feature-specs/m144-db-locked-stop-message.md。
Compatibility/Observation：合成 WAL 多连接竞争与真实准入、持久状态断言，保留驱动日志。
Payload：字段校验与无写入失败路径。Viewport：真实浏览器明暗模式验收，无布局改动。
Release：仅合并，不发布；not live，真机验收由编排方发版后在 Windows 核对并发发送“继续”、无交付停止、有 PR 停止及工具纠错显示。
AI Collaboration：context scope 为发送/停止/参数链；assumptions 为仅合成数据；review point 已请求两个只读独立审查，但工具仅创建任务、未返回执行结果，不将其计为通过证据；validation result 为失败先行与完整回归通过。未调用 delegate_tasks。
后台操作沿用 cancel_chat/dispatch，不新增 GUI 依赖。
Spec Amendment：无。

## 信任根映射
按模板 2026-10-21 例外授权，给 HLT-005 最小增加 stop_fence.rs、write.rs 两个精确 change_patterns 并绑定本变更真实测试；不删减映射、不改 validator/workflow。若 scenario-gate-pr 因 cannot self-modify 拒绝，等待编排方处理，不绕过。

README-Update: reviewed
README-Update-Reason: 修复既有持久化及停止提示，不改变公开功能和安装承诺。
Scenario-Test: CXD-001, CXD-002, DAT-001, HLT-001, HLT-002, HLT-003, HLT-004, HLT-005, RTE-003, RTE-004
