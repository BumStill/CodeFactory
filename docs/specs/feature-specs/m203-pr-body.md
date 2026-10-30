# fix: isolate startup reconciliation from new sessions (M203)

## 需求与测试
- R1：启动等待对账排除本进程新建会话和 active transport；孤儿清理保留 M206 时间边界并增加 created_process_instance 身份保护。
- R2：当前进程新会话尚无首回合时不补 thin-source-repair。
- R3：合成 SQLite fixture；m203_current_process_turn_survives_startup_admission_gap、m203_first_turn_not_started_gets_no_source_repair 在原基线均先失败；合入 M206 后前者通过而后者仍失败。新增 m203_startup_skips_fresh_session_then_first_executor_runs 验证重复启动对账及首回合真实执行者注册；旧进程续接复用 m200_restart_model_wait_is_not_orphaned_and_wakes_once。

## Harness 与验收
- Spec：docs/specs/feature-specs/m203-startup-reconcile-race.md，保留原始 R1–R3。
- Compatibility：保留 M200 中断记录与模型等待唤醒，M206 未回答消息保护；不改 schema，不修改生产数据。
- Observation：断言 terminal_reason、pending wake 数量和真实进程执行者对应 Running。
- Release：只交付至合并队列，不执行发版；Release-Urgency trailer 保留到 squash commit。
- AI Collaboration：context scope 为 thin_derive 启动/周期对账；assumptions 为 sessions 时间戳使用毫秒、transport 带进程身份；review point 为首回合入库前的无执行者间隙与 M200 兼容；validation result 见测试记录。按任务模板保持单执行流，未调用子代理。
- 无 UI 变更，不适用截图与 Viewport Harness。
- 真机验收由编排方发版后完成：启动后立即 dispatch create_and_send，确认首回合有助手响应且没有 thin-source-repair / orphaned_by_restart；重启前在跑任务仍按 M200 自动续接。

## 自验收
- [ ] R1：本进程首回合不被孤儿结案。
- [ ] R2：首回合尚未入库不补 source-repair。
- [ ] R3：启动对账重复执行后首回合正常 Running；M200 等待模型自动续接。
- [x] 前端完整 pnpm test 与 pnpm build 通过。
- Rust workspace 与治理校验执行后更新记录。

Scenario-Test: HLT-001, HLT-004
README-Update: reviewed
README-Update-Reason: 内部启动竞态修复，不改变对外功能、安装、隐私或平台承诺。

Release-Urgency: immediate
