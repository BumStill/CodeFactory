## 结果与范围

后续修复 #789：R6 停止/取消终态、R7 活跃回合被心跳误改，以及编排方批准的事实推导设计。thin_derive::derive_state(session) 按真实存活执行者 → 运行后台任务或未来唤醒 → 未回答用户问题 → ended 的顺序推导；thin_session_state 仅作缓存。心跳、会话列表、运行状态查询和当前回合水合共享推导入口；重启恢复按持久化中断事实选择，不依赖缓存 running。保留原写入点为兼容刷新，不变更 legacy 四态以外协议。

## 为什么 #789 合并后再次询问授权

是执行者误把“同一 PR”约束理解为“原 PR 合并即持续授权失效”，不是工具消耗了授权。已检查 agent/dispatch.rs 的 resolve_turn_delivery_authorization 与 tools/delivery.rs 的 session-authorized 路由：sessions.delivery_authorized 是会话级持久 grant，普通后续消息保持授权，仅明确撤销/只读约束降权；合并不清除 grant。因此不为不存在的一次性消费问题修改授权实现。本次按会话持续授权交付至 merged，刻意发版另行执行。

## 需求与验收映射

| Req | 验收及证据 |
| --- | --- |
| R6 stop | r6_stop_ends_thin_sessions_and_late_errors_cannot_reopen_them：先观察 Running != Ended；修复后停止、迟到错误/最终回答/save 与 legacy 对照通过 |
| R6 cancel | r6_cancelled_outcome_is_terminal_not_a_technical_failure、r6_exact_objective_cancel_ends_thin_state：先失败后通过；精确取消不再被 thin legacy 退役分支截走 |
| R7 derive | facts_override_poisoned_cache_and_dead_process_controls：先失败后通过；污染缓存、真实 refresh、活执行者与死亡执行者 |
| 推导优先级 | future_wakes_and_unanswered_questions_follow_fact_precedence：未来唤醒优先、过期不算、用户回答解除问题；assistant 提问不能解除，已观察 Ended != WaitingUser 后修复 |
| 终止原因 | stop_reason_is_a_fact_even_when_cache_says_running；最终回答与回合终止事实按时间选择原因 |
| legacy | commands::session 18 项、全量 Rust 回归；旧 schema 不要求 thin 表 |

## 验证与自验

- 最后源码修改后的 Rust lib 全量：1843 passed / 0 failed / 10 ignored；同步 main 后再执行全量验证，输出 /tmp/thin-followup-rust.log。
- 前端最新基线验证通过：154 个文件、1028 个测试（含 lightModeAudit），pnpm build 通过。
- test:turn-handback:headless 真实浏览器返回 status=pass；检查 runningTurnKeepsItsBanner、systemIncidentRetiresTheBanner、composerReturnsToSend 等均为 true。产物目录 /var/folders/k9/w_s6rw316n7gvz454845pgfc0000gn/T/codefactory-turn-handback-headless。
- 持续授权实现核验：resolve_turn_delivery_authorization 保留 persisted grant，a_continuation_steer_keeps_a_standing_delivery_grant 与相关既有回归纳入全量 Rust 验证。
- 治理：validate_scenario_test_governance --ci、validate_readme_contract、run_scenario_harness_gate local 与 baseline；最终正文校验后交付。
- 独立失败测试先行；测试 setup 曾修正 cfg(not(test)) API 与必填 started_at，均未改变功能断言；legacy 表缺失通过实现兼容修复而非改旧测试。
- 所有 fixture 为合成数据；不改 trust roots，不降低 required checks。

## Applicable Harnesses / AI Collaboration

Spec：仓库版本化规格 docs/specs/feature-specs/thin-final-answer-settlement-and-restart-recovery.md。
Compatibility：legacy/minimal schema、取消注册竞态、持久停止与真实执行者弱引用。
Observation：cache 污染、真实刷新与终止 reason 字段断言。
Release：仅 PR/CI/队列合并及 trailer 验证，不触发发版，not live。
Viewport：没有布局改动，运行既有真实浏览器 turn-handback 复验。
AI Collaboration：context scope 为上述 thin/取消/水合路径；假设旧写入保留兼容而非权威；review point 为用户批准事实优先级与本次持续授权澄清；validation result 以本 PR CI 和本地真实命令为准。按任务模板不调用 delegate_tasks，采用单执行流。

## 真机验收与剩余事项

真机验收由编排方发版安装后执行：活跃回合跨心跳仍 running；stop 后 ended(cancelled)；重启中断回合先读现场再续跑；只读最终回答 ended；后台任务/未来唤醒 waiting_wake，用户实际问题 waiting_user。没有待决定 Spec Amendment；设计修订已由编排方授权。合并后执行 §8：核实 merged/main/trailer、远端分支、工作区状态与本任务临时资源；不删除受管 workspace。

README-Update: reviewed
README-Update-Reason: 本次修复运行状态推导与恢复，不改变 README 的产品、安装、平台或隐私承诺。
Scenario-Test: CXD-001, CXD-002, DAT-001, HLT-001, HLT-002, HLT-003, HLT-004, HLT-005, RTE-001, RTE-002, RTE-003, RTE-004, SKL-002, SKL-003, UI-001, UI-002, UI-003, UI-004, UI-005, UI-006, UI-007, UI-008, UI-009, UI-010, UI-011, UI-012, UI-013, UI-014, UI-015

Release-Urgency: immediate
