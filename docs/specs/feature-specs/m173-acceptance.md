# M173 验收与交付

R1 根因：首轮 admission 已提交，但尚无 thin_session_state；启动 guard 将错误交给 legacy DecisionRouter，retire_legacy_objective_on_thin_session 将无 running 投影的目标退休为 legacy_orphan，随后 project_chat_objective 写入终态。setup_or_recover 返回 Ok，前端仅凭乐观气泡报告 delivered。此为代码与合成 fixture 复现，未读取生产历史会话。

R2：thin setup 独立结算，模型可用性错误沿 M164/M168 持久唤醒；配置失败为 waiting_user，未知 Drop 中断有自动重试。保留 Objective 与根回合，不写 legacy 决策。
R3：后端仅在执行者启动或 durable wake 等待时成功；持久化失败与用户配置错误返回 Err。前端要求 IPC 成功标记。
R4：SQLite 合成 fixture 验证 waiting、原 root 未完成、精确唤醒时间、到点送达和幂等。

## Req ↔ 测试与验收
- R1/R2/R4：m173_thin_setup_failure_keeps_durable_wake_and_never_orphans：先失败（actual legacy_orphan），修复后通过。
- R2：m173_missing_credentials_is_visible_user_wait_not_orphan：缺凭据、心跳、可见问题及未终结回合。
- R3：M173 optimistic bubbles without backend acknowledgement never claim delivered：先失败（resolved delivered），修复后通过。
- R4 组合与跨进程既有覆盖：m164_hard_cap_matrix_and_deepseek_boundary、m164_model_wait_persists_exact_wake_across_restart、m178_wait_has_reason_and_survives_real_process_restart。
- 完整 Rust 工作区 `cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast` 退出码 0；完整前端 159 文件/1056 项通过（含 lightModeAudit）；`pnpm build`、本地统一场景门禁、场景声明、README contract 与验收账本校验均通过。

## Harness 与协作
Spec：原需求存档 m173-first-turn-silent-abort.md；Compatibility：保留 legacy 分支、复用原 schema；Observation：可见原因和持久 wake；Release：仅合并，not live；AI Collaboration：context scope 为启动 guard/thin 状态/派单回执，assumptions 为合成数据不访问真实凭据，review point 为两项只读审查，validation result 为先红后绿及完整门禁。
无布局变化。真机验收由编排方发版后完成：设置 GPT 超 cap、DeepSeek 时段外、其他无凭据，经 dispatch 派单，核对 waiting_wake 原因/时间，重启后到点自动继续；全部无凭据时明确错误且 waiting_user，不得 legacy_orphan。

Spec Amendment: 无。
Scenario-Test: HLT-001, CXD-001, CXD-002, HLT-003, HLT-004, HLT-005, RTE-004, UI-003, UI-004, UI-005, UI-008
README-Update: reviewed
README-Update-Reason: 内部首轮启动可靠性修复，不改变公开产品、安装、隐私或平台承诺。

Release-Urgency: immediate
