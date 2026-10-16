## M90

修复运行命令的活动标签计时，从审批和钩子通过后的执行入口保存 execution_started_at_ms；连续心跳与恢复复用同一起点。10 分钟后显示安全的命令摘要（例如 cargo test），不展示参数、URL 或环境变量。

## Req ID 与验收
- M90-R1：command_activity::tests::m90_heartbeats_and_restart_keep_execution_origin；agent::persistence::tests::m90_execution_origin_survives_persistence_recreation。连续 26 次分钟心跳，序列化恢复以及 SQLite 重建后均为 26 分钟，pass。
- M90-R2：m90_threshold_and_clock_rollback、m90_command_summary_does_not_disclose_arguments；599/600 秒、回拨、17 分钟非示例、合成秘密参数，pass。
- M90-R3：专项测试先因 CommandActivityTimer 缺失报 E0425/E0433，补实现后原命令 3/3 pass；未放宽断言。

## 验证
- pnpm test：147 文件、991 测试通过（含 lightModeAudit）。
- pnpm build：通过。
- CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-m90" cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast：通过。
- pnpm test:turn-progress:headless：真实 Chrome 的现有进度布局、亮暗与窄屏通过。截图：临时证据目录 codefactory-turn-progress-headless/turn-progress-{dark,light,narrow}.png。该布局验收不冒充 M90 真机计时验收。

## Applicable Harnesses
Spec：仓库规格及 Req 映射；Compatibility：无 schema 变更，旧 NULL metadata 初始化、匿名模式保持无数据库痕迹；Observation：注入时钟与持久状态字段断言；Payload：摘要不泄露原始参数，结果元数据保留起点；Viewport：现有聊天进度浏览器布局回归；Release：仅合并，本次不发版；AI Collaboration：模板要求单执行流，不调用 delegate_tasks。

AI Collaboration record：context scope 为共享 agent-loop、SqlitePersistence 与 trajectory；assumptions 为相同 tool ID 复用持久起点，不把重启后新命令冒充旧命令；review point 为审批后起点、结果元数据覆盖和摘要安全；validation result 为上述本地测试通过。

真机验收由编排方发版后完成：安装新版本；启动持续超过 10 分钟的合成命令，核对 cargo test 摘要和墙钟时长；重启后核对保留起点；结束后核对完成状态。本任务 not live，不发版。

Spec Amendment：无待审批的需求变更。实施说明补充在规格中。

Scenario-Test: HLT-001
README-Update: reviewed
README-Update-Reason: 仅修复已有活动计时，不改变 README 的公开功能、安装或隐私承诺。
