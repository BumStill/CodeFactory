## M125 过载路由回退验收

R1：规格及现状调查见 docs/specs/feature-specs/m125-overload-fallback.md。只使用合成 HTTP 服务和内存 SQLite，不接触生产 DB。
R2/R4：prefer_mode_switches_instead_of_burning_the_overload_budget 先失败后通过，主路由两次 503、第三次 POST 备用成功，包含路由通知及无 waiting episode 断言。
R2：m125_unconfigured_fallback_is_not_available 先失败后通过；备用检查凭据、使用时段、额度、健康。
R3/R4：m125_overloaded_route_remains_eligible_for_scheduled_retry 先失败后通过；durable_provider_overload_stops_after_three_receipted_posts 核对固定候选、不切换、等待说明、未来唤醒及禁止第四次 POST。M123 等待载荷不计失败。

## Harness / AI Collaboration
Spec：逐字规格与 Req 映射；Compatibility：固定计划、既有副作用保护及全量 Rust 回归；Observation：真实 loopback HTTP POST 计数、路由通知、SQLite 字段；Release：仅合并，不执行发布，not live；AI Collaboration：context scope 为后端路由与持久恢复，assumptions 为现有 supervisor 到点唤醒，review point 为固定策略/准入/副作用安全，validation result 为红绿测试和全量回归。按任务模板，未调用 delegate_tasks，单执行流。

## 自验
- pnpm test：153 文件、1024 测试通过（含 lightModeAudit）。
- pnpm build：通过。
- Rust：专用 cargo-target-m125；合并最新 origin/main 后跑 `cargo test --workspace --no-fail-fast`，20 个测试目标 2141 项全通过、0 失败（fail-closed，无管道掩盖退出码）。
- 无前端布局变更，不适用 Viewport 截图。真机验收由编排方发版后完成：配置 prefer 及 fixed 模型，触发两次过载，核对备用切换/等待通知及自动续跑。
- 后台可操作性：后端路由及 supervisor 自动工作，无需 GUI 点击。
- 无 Spec Amendment；历史证据中的实际忙闲时状态无法从合成数据核实。

Scenario-Test: HLT-001
README-Update: reviewed
README-Update-Reason: 仅修复现有模型路由恢复，不改变公开功能、安装、隐私或平台承诺。
