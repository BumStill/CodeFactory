## M217：每日人民币预算完整接线

R1：统一用量入口写入 estimated_cost_cny；启动和路由读取当日 SQLite 重建，兼容历史 NULL，不改旧行。
R2：启动、路由前（60 秒缓存）、回合终态读取 DeepSeek 余额；开盘、充值和当前余额持久化；读取失败使用估算且不输出凭据。
R3：新增只读 usage_status；get_quota 增加 daily_spend，对旧订阅字段保持兼容。
R4/R5：重建后实际 admission 产生 daily_spend_cap skip 和可持久化模型不可用等待，复用 waiting_wake 及既有预算中文文案。

### Req ↔ 测试与红绿记录
- R1：m217_usage_cost_is_persisted_for_restart：先失败 R1 must persist CNY，修复后通过；重复请求不重复落库。
- R2：m217_restart_rebuild_includes_legacy_null_and_preserves_balance：开盘持久化、重启、充值、失败降级、跨日。
- R3：m217_usage_status_is_a_readonly_dispatch_operation：先失败 unknown variant usage_status，修复后通过；拒绝 key_ref 等未知字段。前端 M217 forwards readonly usage_status to the durable spend command 覆盖调用接线。
- R4/R5：m217_restart_rebuild_includes_legacy_null_and_preserves_balance：同一真实迁移数据库、新 ledger、新 Settings，实际 admission 拦截并断言 skip、文案、午夜恢复（沿用通用 250ms 防边界抖动）。

### Harness / AI Collaboration
- Spec：版本化 M217 原始任务与 Implementation Notes。
- Compatibility：历史 NULL 和 SQLite 幂等 schema、新旧 quota 响应兼容。
- Observation：金额、来源、余额、重置时间、路由 skip，不输出密钥。
- Release：本 PR 只合并，不执行发版；Release-Urgency: immediate。
- Viewport：复用既有布局；订阅控件过滤人民币预算行，防止错误百分比操作。`pnpm test:usage:headless` 通过 1366×768、800×600、375×812 的趋势、热力图、详情和预算键盘路径；截图位于系统临时目录 `codefactory-token-usage-headless/{wide-settings,wide-day-detail,minimum-window-settings,narrow-settings}.png`（深色）及 `/tmp/codefactory-m217-usage-light/`（浅色）。两种主题均通过相同布局/键盘断言，并经 view_image 确认宽屏截图无重叠、裁切。
- AI Collaboration：context scope 为 M217、成本持久化、路由、余额与 dispatch；assumptions 为既有 Asia/Shanghai 用量日界；review point 为重启不能清零、凭据不出现在输出；validation result：M217 四项定向验收通过，路由构造守卫修复后通过，前端 163 个文件 / 1087 项测试通过，build、用量真浏览器、统一 local scenario gate、治理基线、README/scenario validator 通过；Rust workspace 全量复跑已通过（CARGO_INCREMENTAL=0，专用目录与心跳保护）。按任务模板不调用 delegate_tasks，单执行流。

### 自验收
- 已花按单价、缓存命中去重、幂等写入：定向测试通过。
- 新 ledger 和 Settings 重建、历史 NULL 计费、余额持久化、次日清零：定向测试通过。
- 真机验收由编排方发版后完成（not live）：执行 codefactory --dispatch usage_status，核对当日金额误差 ≤5%；重启再查；在设置 endpoint_daily_spend_caps 临时降低 daily_cap_cny，触发 DeepSeek，核对 daily_spend_cap 与 waiting_wake；恢复 ¥10。能读取余额时核对余额差。
- 无 Spec Amendment；未修改信任根。

Scenario-Test: HLT-001, CXD-002, DAT-001, RTE-003, RTE-004, SKL-002, SKL-003
README-Update: reviewed
README-Update-Reason: 修复既有预算功能的持久化与内部 dispatch 接线，不改变安装、隐私或平台承诺。

Release-Urgency: immediate
