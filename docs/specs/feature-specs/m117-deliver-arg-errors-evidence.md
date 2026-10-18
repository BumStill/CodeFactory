# M117 验收记录 / PR body

修复交付参数拒绝被误判为致命错误；同一交付较低 ceiling 在授权内沿用原记录。

Scenario-Test: CXD-002, HLT-001, HLT-002, HLT-005, RTE-004, SKL-002
README-Update: reviewed
README-Update-Reason: 修复既有交付恢复语义，不改变公开功能、安装或平台承诺。

## Req 映射与自验收
- R1：已确认 prepare_durable_run 原先的 AppError::Other 经 DesktopToolBackend -> ToolError -> LoopError::Tool 退出回合；chat_failure_code_for_error 除 SQLite contention、provider identity/reconciliation/transport/endpoint 等专类外兜底 agent_loop_error。正文与场景声明拒绝原本走 DeliveryOutcome/ToolOutput，并不走 fatal 路径。未读取生产 DB，生产耗尽现象引用任务证据，不声称完成逐调用生产取证。
- R2 pass：m117_invalid_ceiling_is_an_ordinary_correctable_tool_error 与 m117_bad_branch_is_an_ordinary_tool_error 两项先红后绿。新增 ToolArgument 类型隔离可修正预检，Db/IO/持久化/身份错误仍传播。真实 backend 参数错误后 read_file 成功，objective_remediations.attempt_index 前后相等。
- R3 pass：m117_resume_retains_recorded_ceiling_within_authorization 使用合成 SQLite，through_merge + through_ci_green 得到 ThroughMerge 与“沿用 through_merge”；记录不变；仅 PrOnly 授权拒绝；新 run 仍按本次请求。durable admission 与实际 DeliverOpts 都使用 resolver，结果附沿用说明。
- R4 pass：聚焦四项、Rust workspace、前端与 build 通过。ceiling resolver 和 backend 预算测试是补充回归，无独立先红证据，不声称全量严格 TDD。

## 验证
- Rust：`CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-m117" cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast` 通过；主 lib 1733 passed、10 ignored，其余 workspace/集成测试通过。
- `pnpm test`：151 files、1013 tests passed，含 lightModeAudit。
- `pnpm build`：通过，现有 chunk 大小警告。
- `python3.11 tools/governance/validate_repo_governance_baseline.py` pass。
- `git diff --check` pass。未改 trust roots、UI 或生产 DB。

## Harness / AI Collaboration
Spec：M117 原文与需求矩阵已版本化。Compatibility：保持既定 ceiling 不降、授权硬边界不变。Observation：真实工具入口/backend 与 SQLite 字段断言。Release：仅合并，不发版，not live。AI Collaboration：context scope 为交付工具/backend/loop 错误边界；assumptions 为合成数据及不扩大授权；review point 为错误类型隔离与持久记录不降；validation result 为上述绿测及 CI。模板禁止 delegate_tasks，采用单执行流，未启动该工具。无 UI/载荷改动，Viewport/Payload 不适用，无浏览器截图。

## 真机验收与收尾
真机验收由编排方发版后完成：同任务授权 merged，建立 through_merge 后以 through_ci_green 续接，核对同 run/PR、“沿用 through_merge”和恢复预算不变；非法 ceiling 修正后重试；仅授权 pr_only 时不允许沿用 through_merge。
无待决 Spec Amendment：M117 明确替代 M116 降低即拒绝要求。合并后按 §8 核对 merged=true、squash 在 origin/main、精确远端分支、工作区及缓存清理；不手动合并、不发版。
