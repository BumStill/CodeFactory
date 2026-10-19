# fix: restore receipt-free browser actions in thin sessions

## 需求与范围
修复 thin 会话 browser_session 原生入口仍强制 durable receipt 导致浏览器无法启动的问题。规格：docs/specs/feature-specs/task-simplify-phase1-p0-browser.md。

## Req ID ↔ 验收
- R1/R3：thin_browser_open_reaches_driver_without_receipt，合成内存 SQLite + 本地合成 HTML + 真实 Chromium；断言 Done、标题、正文、关闭租约。先失败：修复前返回 Waiting/browser_observation_contract_required；最小修复后同命令通过（1 passed），真实页面标题、正文与租约回收断言全部通过。
- R2/R3：legacy_browser_missing_receipt_still_waits_and_reads_are_admitted，legacy 生命周期/写动作仍返回原 waiting code，两模式只读动作通过回执门禁后返回未知租约错误。
- R4：排查 outer_receipt_id / mutation_permit / contract_required：tools/browser_session.rs 是本次遗漏入口；tools/mod.rs 是上下文字段；tools/knowledge.rs 为上下文构造；tools/delegate_tasks.rs 为调度器 permits 传递；tools/delivery.rs 已在 prepare_durable_run 前按 thin 分流；agent/tool_backend.rs mutation_preflight 已为 thin 提前返回无回执 Dispatch，observe_recovery 也提前返回。其余匹配为 legacy 验证与测试，无其他需要修复的工具门禁。
- R5：真机验收由编排方发版后完成，本 PR not live，不操作生产 DB。新会话打开 example.com 读取 Example Domain；检查 /usr/bin/log show 中 SystemPolicyAppBundles 无 responsible=com.codefactory.app 拒绝；关闭后无 Chrome 残留。

## Applicable Harnesses
Spec（原文落盘与 Req 映射）、Compatibility（legacy 与只读矩阵）、Observation（字段级结果、真实驱动与回收）、Release（仅合并，M119 真机交接）、Payload（本地合成页面）、AI Collaboration。无 UI 修改，不适用 Viewport，无截图要求。

## AI Collaboration
context scope：P0 任务、模板、阶段 1、浏览器控制/权限/profile、工具入口。
assumptions：delivery_mode 运行中不变；仅明确 thin 跳过 durable receipt。
review point：未知模式 fail closed，legacy 路径不变，profile/域名/凭据/扩展只读/受管启动不削弱。
validation result：前端 152 文件 1017 测试及 pnpm build 通过；治理基线与本地统一场景入口通过；浏览器工具 16 测试通过；从 /tmp 调用真实浏览器 fixture 与 legacy/只读矩阵均通过。首次 workspace 库测试 1759 通过、10 忽略，既有 begin_write_waits_for_a_contended_writer_instead_of_failing 时序断言失败（仅等待 318µs）；立即单独重跑通过，未修改断言；随后原样重跑完整 workspace 退出码 0，主库 1760 passed、10 ignored，全部集成测试与其余 workspace crate 通过，原锁等待断言在全测中通过。
按任务模板单执行流，不调用 delegate_tasks。

## 安全与后台入口
沿用原 browser_session 和 local dispatch 路由，不添加 GUI 依赖。合成数据库仅在内存，匿名 fixture 使用一次性 profile；未改系统授权、生产数据库、信任根、macOS spawn 或扩展通道。

## Spec Amendment
无待决需求改动。现有普通会话默认 persistent profile，本次保留既有策略，不把薄回执分流变成 profile 策略迁移。

Scenario-Test: RTE-001, RTE-003
README-Update: reviewed
README-Update-Reason: 修复现有浏览器能力的 thin 兼容性，不改变公开能力、安装、隐私或平台承诺。
