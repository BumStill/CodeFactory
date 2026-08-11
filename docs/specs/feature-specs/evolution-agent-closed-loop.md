# Evolution Agent：Session 轨迹与持续改进闭环

## 1. Requirements Traceability

| Req ID | 来源 | 规范化需求 | Surface | 验证 | Owner |
| --- | --- | --- | --- | --- | --- |
| CF-EVO-R1 | 用户方案 | 持久 session 的真实工具生命周期进入规范化轨迹 | agent-loop + sqlite | Rust integration + real app DB assertion | development + QA |
| CF-EVO-R2 | 用户方案/安全 | 系统派生的工具参数、结果、错误、assistant/reasoning 入库前脱敏；normalized/Evidence 另行限长；远程复盘默认关闭且必须显式 opt-in | agent-loop + evidence + settings | secret fixtures + DB/export grep + default-off assertion | development + QA |
| CF-EVO-R3 | 用户方案 | project、Quick、task 三类持久 session 可分析；anonymous 零持久化 | chat + task + sqlite | route tests + real app | QA |
| CF-EVO-R4 | 当前缺陷 | 普通聊天无 task_run 时仍有 post-mortem 输入 | learning | unit + integration + real chat | development + QA |
| CF-EVO-R5 | 当前缺陷 | Evidence 读取真实 `tool_calls`，不查询不存在的表 | evidence | field-level pack assertion | development + QA |
| CF-EVO-R6 | 用户方案 | 失败率只统计真实执行 done/error，不把拒绝/legacy 当工具故障 | learning | detector/query tests | development + QA |
| CF-EVO-R7 | 用户方案 | 所有改进维持人工门禁，不自动合并/部署/发布 | learning + skills + settings | code review + UI path | planning + QA |
| CF-EVO-R8 | 仓库规则 | 真实 CodeFactoryDev 成功与边界路径验证 | desktop-ui | screenshot/video + DB evidence | QA |
| CF-EVO-R9 | 用户补充 | Home 提供一级「进化审查」入口和准确待审数量 | home + app navigation | component test + real home path | product + frontend + QA |
| CF-EVO-R10 | 用户补充 | Workspace 待审提示深链到当前 project scope，不能回落到最近项目或串 scope | workspace + review routing | routing test + real project deep link | frontend + QA |
| CF-EVO-R11 | 用户补充 | 待审页使用队列/详情主从布局，展示准确去向、脱敏证据、来源和精确人工动作 | evolution-review + learning | component/action tests + real review path | product + frontend + QA |
| CF-EVO-R12 | 用户补充 | 分析、审核与有限物化作业及节点日志持久化；人工决定幂等；进程重启后死亡 owner 的旧运行中作业必须保留记录与节点证据，兼容投影可标记 `failed`，但恢复责任归系统 remediation；存活 owner 不得被误杀 | evolution jobs + objective remediation + sqlite + review UI | storage integration + recovery ownership + liveness/restart path | architecture + development + QA |
| CF-EVO-R13 | 用户补充 | 页面如实区分 proposed/approved/materialized/eval/active；Phase 0/1 不得假装已接 Evals 或 activation | review UI + governance | state contract review + negative UI assertion | planning + QA |
| CF-EVO-R14 | 仓库规则 | 工作台完成声明同时满足成功、边界、重启、viewport 与 release 分层验收 | desktop-ui + sqlite + release | real app matrix + PR/CI + installer evidence | QA + release |
| CF-EVO-R15 | 用户补充 | 本机锁屏不能阻断验证、合并或上线；不得绕过锁屏，改由不依赖交互桌面的 headless 浏览器 viewport/keyboard gate 与远端 macOS DMG smoke 继续执行；receipt 不得伪造 OS 锁屏观测 | browser harness + CI + release | 1366/390 headless receipt + PR checks + macOS artifact smoke | QA + release |
| CF-EVO-R16 | Phase 4 | 新候选的人工批准与生效分离；批准冻结不可变 revision，批准前后 live memory/preference 均不变化 | candidate/review | SQLite + prompt context before/after | development + QA |
| CF-EVO-R17 | Phase 4 | 每个 Eval run 绑定 exact candidate/revision、runner 与 manifest hash；旧 `accepted` 不追溯补写 Eval 或 activation | eval schema + compatibility | old DB migration + field assertions | architecture + QA |
| CF-EVO-R18 | Phase 4 | 首版激活安全 Evals 对 baseline/treatment 使用同一确定性 manifest，覆盖 project scope、隐私、类型白名单、隔离注入、幂等和回滚准备度 | eval runner | Rust integration + release binary smoke | development + QA |
| CF-EVO-R19 | Phase 4 | verdict schema/UI 严格区分 `passed/failed/inconclusive/error`；首版确定性安全 suite 产生 passed/failed，runner 异常产生 error，inconclusive 保留给后续证据不足型 case；只有全部 required case 通过才允许自动激活 | eval runner + UI | failure/error/retry tests | development + QA |
| CF-EVO-R20 | Phase 4 | 自动激活需要本次人工批准显式选择且默认关闭，只允许 project-scope memory/pattern 与非安全白名单偏好 | activation policy | allow/deny matrix + UI assertion | product + QA |
| CF-EVO-R21 | Phase 4 | activation 使用 exact revision + Eval + expected target fingerprint 门禁，写入 active 状态与 receipt；重复/并发调用只能生效一次 | activation | concurrency + stale target tests | architecture + QA |
| CF-EVO-R22 | Phase 4 | rollback 只撤销 exact activation；偏好被用户后改时进入 conflict，不覆盖新值；memory 使用独立 active row 原子停用 | rollback | CAS/idempotency/restart tests | development + QA |
| CF-EVO-R23 | 用户补充 | 工作台显性展示 Evals、自动激活、case 结果、exact run/receipt、失败原因和回滚动作，并与端到端作业日志同页关联 | evolution UI | component + 1366/390 headless | frontend + QA |
| CF-EVO-R24 | Compatibility | 旧 `accepted` 继续显示“历史已生效（未评测）”；新链路不再复用它表示 approved/eval/active | migration + UI | legacy fixture + negative assertion | architecture + QA |
| CF-EVO-R25 | Release | 锁屏安全交付除 headless/CI/DMG 启动外，精确 release executable 必须在隔离临时目录执行真实 stage→Eval fail/pass→activate→rollback smoke | release binary + workflow | JSON receipt on macOS/Windows artifact | QA + release |

## 2. Primary User Path

用户打开 CodeFactoryDev，选择真实项目和模型，完成包含工具调用的任务；系统将工具声明、权限结果、执行结果、耗时和脱敏摘要持久化。session 结束后，Home 的一级「进化审查」入口显示待审数量，Workspace 可深链到当前项目 scope。用户在候选队列/详情主从布局中核对证据，明确采纳、拒绝或启用更谨慎的工具门控；系统在同页展示从范围锁定到等待审核的真实作业节点，不把 proposal 或 approved 冒充为已评估、已激活。

## 3. Applicable Harnesses

- Spec Harness：Req ID、数据合同、状态与验收。
- Compatibility Harness：旧 SQLite、messages JSON 重放、增量 schema。
- Observation Harness：真实工具 route、状态、耗时、错误与 dropped 边界。
- Payload Harness：arguments/result/error 截断、脱敏、Evidence 导出。
- Viewport Harness：进化审查的队列/详情在 1366×768 和窄窗口可操作。
- Lock-safe Harness：本机锁屏时继续执行系统 Chrome/Edge headless viewport/keyboard 验收；发布壳由 GitHub macOS runner 的 DMG smoke 独立证明。
- Observation / Compatibility Harness 的 job lifecycle 验收维度：scope、结构化节点状态、人工决定幂等、失败记录、兼容终态投影与进程重启后的系统恢复责任。领域内断点恢复仍是后续扩展；当前由统一 objective remediation 保留目标并编排恢复，不能把技术恢复交给用户。
- AI Collaboration Harness：规划、架构、QA 独立审查；明确当前实现与建议。

## 4. 测试矩阵

| 路径 | 场景 | 预期 | 最低证据 |
| --- | --- | --- | --- |
| Primary | allow 工具成功 | pending -> done，result/duration 可追溯 | DB row + 工具卡 + screenshot |
| Failure | 工具返回错误 | pending -> error，error 脱敏 | DB row + 实际输出 |
| Permission | ask 后拒绝 | status=denied、duration=0，不计工具失败 | UI decision + DB row |
| Hook | pre-tool cancel | status=denied，不执行工具 | hook log + DB row |
| Runtime error | dispatch 返回 Err | status=error 后再传播错误 | regression test + DB row |
| Replay | done/error/denied terminal state | normalized row 与唯一 tool replay message 同事务一致；重试更新旧 replay | rollback + error→done integration |
| Replay | assistant/replay 同毫秒写入 | provider history 用 `created_at,rowid` 稳定排序 | query assertion + restart path |
| Replay | app 重启后加载历史会话 | assistant declaration 与 `role=tool` replay 重新折叠成同一工具卡，保留 done/error/denied | store hydration test + real restart UI |
| Chat | 无 task_run 的普通/Quick 会话 | 生成有限脱敏 session summary | prompt-builder test + real chat |
| Privacy | anonymous 同类调用 | DB/session/learning/evidence/cost 计数不变 | 前后计数 |
| Privacy | user 输入含测试 secret，模型/工具复述 | 用户原始消息按既有历史保留；assistant/tool/trace/Evidence 均不复制原值 | DB 字段级 grep |
| Privacy | JSON 敏感值含转义字符串、数字或布尔值 | 先 parse 后递归脱敏，输出仍是合法 JSON | structured redaction fixtures |
| Safety | 模型返回非法 preference key | 降级为 memory，不写入 user_preferences/system prompt | sanitizer + storage assertion |
| Compatibility | v1.43.0 旧 DB | 启动后表/索引存在，旧消息可重放 | migration fixture |
| Evidence | 生成 evidence pack | 读取 normalized rows，含 status/error/duration，无 secret | pack field assertion |
| Analysis | 运行跨会话挖掘 | 只基于 done/error，至少来自两个不同 session，真实信号非 fixture | query + UI + DB |
| Analysis | 展示新旧 pattern evidence | 新数据按声明 unit 展示；legacy count 不误标为 session | Profile component test + real UI |
| Review | 采纳 pattern 候选 | event 变为 accepted，建议只写入当前项目记忆；重复分析不新增同候选 | Evolution Review action + DB + memory.md |
| Entry | Home 打开进化审查 | 一级入口和待审数可见，进入默认待审队列 | component test + real screenshot |
| Deep link | Workspace 当前项目待审提示 | 进入后 project cwd/scope 不变，不显示其他项目候选 | routing assertion + real path |
| Review layout | 选择待审候选 | 左侧队列选中态和右侧详情一致，显示去向、证据、来源、准确 support unit | component test + DB/UI comparison |
| Review action | 采纳/拒绝 | 动作前无副作用；采纳只改变明确目标；拒绝不改 memory/settings | DB + filesystem/settings before/after |
| Job | 运行分析 | 同一项目仅一个 running 分析；候选、最终节点和 job 终态原子提交，节点按真实顺序记录输入/输出计数 | storage integration + rollback fault injection + real job UI |
| Job boundary | 样本不足/无候选 | 显示 current/threshold 或抑制数量，不显示“系统健康” | state test + real empty state |
| Job failure | query/detector 或终态写入失败 | failure event 与 job failed 原子提交，任一被拒绝时一起回滚；显示准确失败节点；统一 remediation 编排有 lineage 的后续 job，旧日志保留 | event-insert + terminal-update 双向 fault injection + recovery assertion |
| Job restart | 分析中或终态重启 app | 死亡、PID 已重用或旧版无 owner 的 queued/running 被关闭为 `failed` 兼容投影、追加 `process_restart` 并交由统一 remediation；PID 与启动标识均匹配的存活 owner 保持 running；系统恢复不得重复 candidate | real dead child + PID reuse + live identity + restart DB + recovery-owner assertion |
| Job audit | 来源作业很旧或日志超过 500 条 | 按 id 精确打开，不回退最新 job；事件保留最近 500 条与最新终态并提示上限 | query boundary + component test |
| Workbench failure | 分析、采纳、拒绝或 ledger 读取失败 | 刷新并显式展示持久失败作业；采纳失败重读 current value；ledger 读取失败不显示空记录 | component failure-path tests + real app boundary path |
| Decision focus | 窄屏/桌面处理最后或中间候选 | 成功后聚焦下一候选，最后一条聚焦“查看决定历史”；取消确认恢复原动作 | focus assertions + 390px real app keyboard path |
| Current value | memory/preference 当前值读取成功或失败 | 显示真实 current → proposed；读取失败禁用采纳且不阻止拒绝；切 scope 立即清旧数据 | component race/error tests + real app |
| Scope | project/quick/global | scope 不串；anonymous 排除；Quick 在稳定 scope 前不宣称跨会话 | query tests + real scoped paths |
| State boundary | Phase 0/1 candidate | UI 只显示真实 pending/accepted/rejected/job 状态，不出现未接入的 eval_passed/active | negative UI assertion |
| Viewport | 审核队列和详情 | 1366×768 主从同屏；窄窗口单列 list/detail，主动作可达且无水平溢出 | screenshots/video + keyboard path |
| Lock screen | 本机在验收或交付期间锁屏 | 不请求解锁、不绕过系统安全；headless 1366/390 完整决定、历史、日志与键盘交互继续，390 决策栏位于 viewport 内；PR/CI/合并/发版通过 CLI 与远端 runner 完成 | JSON receipt（`interactive_desktop_required=false`、`os_lock_state_observed=not_measured`）+ screenshots + CI log + macOS DMG smoke |
| Release | Dev 通过后的发布声明 | PR+CI、安装包/build metadata、发布版重跑主路径；此前保持 not live | PR checks + installer + live evidence |
| Phase 4 approval | 批准候选且未选自动激活 | 生成 immutable revision；live context 不变；Eval 通过后停在 pending_activation | DB revision/run + prompt context |
| Phase 4 auto | 批准时显式选择自动激活 | exact revision 的 required cases 全过后只激活一次，下一 session context 使用 active revision | DB receipt + context assertion |
| Phase 4 hard fail | secret/scope/unsupported preference/stale target | verdict failed 或 inconclusive；旧 active 不变且无成功 receipt | case rows + target fingerprint |
| Phase 4 rollback | active memory/preference 回滚 | exact receipt 变 rolled_back；下一 session 不再使用该 memory 或恢复此前 preference；用户后改产生 conflict | CAS + restart + context assertion |
| Release functional smoke | 精确 DMG/Windows executable | 隔离目录完成 stage→Eval fail/no switch→Eval pass→activate→rollback，输出无 secret 的 JSON receipt | artifact executable receipt |

## 5. 完成边界

单元测试、构建、UI 空态或一条非空数组都不是完成。Phase 0 仅在真实 app 主路径、边界路径、持久化、匿名、脱敏、Evidence、一级入口、project 深链和人工审核全部有证据后完成。本轮持久 job slice 还必须证明结构化日志、人工决定幂等、失败可追溯，以及重启中断后的兼容投影和系统恢复责任；partial/dropped 与领域内断点恢复属于后续 Phase 1 扩展。

R9-R15 的 v1.44.0 工作台仍是对 `learning_events` 的可信审核投影。R16-R25 开始把新批准 lazy-adopt 为独立 immutable candidate revision；`learning_events` 只保留来源与 legacy 决定，不能再承载 Eval/activation 真相。首版 Evals 是“激活安全回归”，证明变更可被安全、隔离、幂等地生效和回滚，不证明任务成功率提升。Dev app 验证、PR 或 CI 通过仍是 `not live`，必须经过刻意发版、精确发布二进制功能 smoke 和公开产物验证才可声明 live。
