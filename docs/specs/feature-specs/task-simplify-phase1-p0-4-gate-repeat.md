# 阶段 1 P0-4：thin 只读浏览器任务仍被完成门禁拒收，模型重复同一答案 6 次；受管浏览器会话被过早回收

## 证据（编排方 10-19 18:2x，v1.89.8，已含 #759）
- 会话 535f7bf1（thin，只读：「用受管浏览器打开 example.com，读标题，关闭会话，回复标题」）：
  - 工具序列：`open` done（返回页面内容）→ `tabs` error「only applies to the user's own browser」→ `snapshot` error「session is unknown or has already been reclaimed」→ `close` error「session is unknown or has already been reclaimed」。
  - 助手回复「Example Domain」共 6 次（18:25–18:33），objective_events 6 条 `decision_applied / waiting_system / completion_evidence_incomplete`（detail_json 为空）。
- 两个缺陷：
  1. 完成门禁（thin）：#759 后仍对这种任务给 completion_evidence_incomplete，并且**不告诉模型缺什么**，模型只能原样重复答案——与历史上的「7 遍重复回答」同族（见 PR #119/#122 记录）。
  2. 受管浏览器会话在 open 返回后的下一步就被判 reclaimed（同一回合/相邻回合内），导致 snapshot/close 全失败。

## 需求
- R1 先复现：用合成 fixture 在 thin 会话重现「open 成功 → 后续浏览器工具报错 → 正确最终回答」，确认当前被拒收。写出门禁拒收的真实原因（代码路径），写进 PR。
- R2 thin 完成判定只看阶段 1 R5：无外部进行中事项 → 最终回答即完成。中途工具报错不构成拒收理由（模型已在回答里交代即可）。若确实保留任何检查，拒收时必须把「缺什么」作为明确提示给模型，且同一拒收最多 1 次，第二次直接完成或结束并说明——**绝不允许模型重复同一答案**。
- R3 受管浏览器会话生命周期：open 创建的会话在本任务内持续可用，直到显式 close、任务结束或闲置超时（给出时长）；查明本例为何下一步即被回收（回合结束清理？owner 识别不一致？）并修复。tabs 对受管会话应可用或给出正确指引。
- R4 测试（先失败）：上面复现用例 → completed，最终回答只出现 1 次；open→snapshot→close 在同一任务的相邻回合都成功；门禁拒收（若有）时模型收到的提示非空。
- R5 编排方真机验收：同一只读浏览器任务 → objective completed、答案只出现一次、close 成功、无残留 Chrome。

## 约束
- 阶段 1 自身缺陷（P0）。不改 legacy。合成 fixture，不写生产 DB。授权交付（ceiling: merged），PR 正文带 `Release-Urgency: immediate`。

## Implementation Notes
- 任务未指定规格文件名，按任务同名落盘。原需求以上逐字保留。
- Applicable Harnesses：Spec、Compatibility、Observation、Release（仅合并）、Payload、AI Collaboration；无 UI 修改。
- 单执行流：任务模板禁止 delegate_tasks；不与其他 agent 并行修改。
- 生命周期保留既有 20 分钟闲置超时、显式 close 与任务结束清理；非生命周期命令错误不得销毁受管会话。
- 真机验收由编排方发版安装后执行；本任务不发布，not live。

## 本地验收记录
- 先红：循环出口 `completed=false`，blocker 为 `at least one successful verification is required`；thin Delivery 仲裁为 WaitingSystem；真实浏览器 tabs 旧指引且后续会话被回收。
- 修复后：循环一次最终回答回归通过；所有 thin kind 完成与 legacy Delivery/Live 对照通过；生产结算入口的完成/非失败等待通过；真实 Chromium open→新调用上下文→tabs 指引→缺 query 错误→snapshot→read 正文→close 全通过，最终零本任务租约。
- `snapshot` 是交互元素快照，不承诺包含静态正文；测试用 `read` 断言正文，保留 snapshot Done 的断言。
- 前端：153 个测试文件、1024 项测试通过，`pnpm build` 通过。
- 场景声明/README/统一场景门禁与治理基线通过；统一门禁使用 Python 3.11（系统 Python 缺 tomllib）。完整工作区 Rust 回归后台执行，结果由 PR/CI 记录。

## Requirements Traceability / 测试矩阵
| Req | 验证 |
| --- | --- |
| R1/R2/R4 | 合成 thin run：browser open 成功、后续报错、一次最终回答；循环回执与 objective 持久结算检查 |
| R2 | thin 所有目标类别无外部事项时完成；legacy 原仲裁器不变；外部等待测试保持 |
| R3/R4 | 真 Chromium 本地 fixture：open、错误命令、tabs 指引、snapshot、close；确认租约回收 |
| R5 | 发版安装后运行原只读浏览器任务，核对 completed、答案一次、close 成功、无本任务 Chrome |
