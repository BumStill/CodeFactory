# 非业务阻断回交审计证据包（2026-08-10）

## 1. 结论

当前工作树已经出现两组正确的止血机制：

- completion gate 的证据不完整不再写成 `completed`，而是写为 `incomplete`、`failed_internal` 或 `platform_incident`，同时 transport `Done` 仍可关闭 streaming；
- `deliver_changes` 正在引入带稳定身份、lease、授权续接和不可降低 requested ceiling 的 durable run。

但这两组机制仍是**未发布的工作树改动**，且尚未解决系统性根因：生产路径没有统一、强类型、可持久化的 decision router，也没有横跨 scheduler/provider/delivery/browser 的 remediation supervisor。结果是同一种技术故障在不同模块中分别变成“请重试”“切换模型”“已修复，重试”“人工续接”“先切分支”“安装后再试”。用户仍被迫承担系统诊断、恢复触发和续接动作。

本次静态只读审计共归并出 **42 组需整改的生产命中**，另列 7 组可复用的安全机制：

- P0：28 组；会直接把技术阻断回交用户、错误完成，或在已授权 next action 上停止；
- P1：11 组；会丢失恢复身份、降低验收边界、重复请求或造成不一致状态；
- P2：3 组；主要是观测、文案和测试契约缺口；
- SAFE：7 组；方向正确，可作为统一机制的基础，但尚未构成端到端闭环。

最高优先级的三个系统修复不是逐条改文案，而是：

1. 建立统一 `DecisionRouter`，默认 `system_owned_recovery/apply_recommended`，只有结构化核心输入或不可逆业务选择能投影给人；
2. 建立持久 `RemediationSupervisor`，接管恢复预算耗尽、permission timeout、provider failover、scheduler failed、delivery reconciliation 和 browser session 等待；
3. 将授权语义从“每个工具弹窗”提升到 objective/capability/side-effect 级，`autonomous_completion=true` 下自动执行已授权且可逆的推荐动作。

在这三项完成前，不能宣称“非业务阻断用户回交率 = 0%”。

## 2. 审计边界与证据层级

- 工作树：`agent/durable-delivery-recovery`
- 基线 commit：`fa80a6be20fe757a07e1a3e56c85e0c0a405d149`
- 审计时工作树：28 个 modified/untracked 路径；本证据包不把这些未提交内容当成已发布事实。
- 扫描面：agent-loop、tool backend、permission、scheduler/sub-agent、provider/model failover、context budget、delivery/release、browser/session、UI 文案。
- 方法：生产代码静态路径复核；测试和历史文档单独列出，不把测试断言当作生产能力。
- 本证据包未执行发布、远端 mutation 或真实账号操作；“生产命中”指生产代码路径，不等于正式版 live evidence。

证据标签：

- `HEAD`：基线 commit 已存在；当前正式版是否包含仍需 release/tag/app build 对齐。
- `WT`：当前工作树新增或修改，尚未 commit/merge/release/live verify。
- `SAFE`：符合新原则的现有机制，但不代表端到端闭环。

权威决策规则来自 [human-business-decisions-only.md](../principles/human-business-decisions-only.md)：存在安全/推荐且可逆方案时直接执行；无法推导的外部核心输入只允许一次合并请求；CI、冲突、网络、provider、工具 timeout、进程、依赖、cwd、context、凭据刷新和恢复预算耗尽均由系统拥有。

## 3. 分类口径

| 分类 | 允许的外部表现 | 示例 |
| --- | --- | --- |
| `system_owned_recovery` | 后台诊断、退避、修复、reconcile、切路由；不生成用户 CTA | CI 失败、permission channel closed、context overflow、merge conflict |
| `apply_recommended` | 在既有授权内直接采用推荐且可逆方案 | 切换健康模型、修 PATH/cache、选择唯一匹配 worktree |
| `core_input_required` | 一次性合并最小缺失输入，保留 objective 和自动续接点 | 首次外部身份凭据、2FA/CAPTCHA、用户独有文件、不可替代生产账号授权 |
| `needs_business_decision` | 仅结构化、不可安全代选、不可逆且改变实质业务结果 | 删除语义、成本/发布时间/质量取舍、扩大安全权限、解除明确 release hold |
| `failed_internal/platform_incident` | 保留 objective、证据和 remediation id；不得变成“回复继续” | 所有自动恢复耗尽、系统能力缺失、平台凭据故障 |

“缺凭据”本身不是自动等于 `core_input_required`：必须先穷尽现有 credential broker、系统钥匙串、受管身份、CLI 登录、等价 provider/route 和安全刷新路径。

## 4. 生产代码命中清单

### 4.1 Agent loop 与完成状态

| ID | 级别/证据 | 生产命中 | 当前机制 | 风险与判定 |
| --- | --- | --- | --- | --- |
| AG-01 | P0/HEAD | `src-tauri/src/agent/mod.rs:328` | 测试失败原因不清时直接“stop and ask the user” | 不清晰是工程诊断状态，不是业务选择；应继续读取 spec/实现/历史或进入 `failed_internal`，不能回交用户。
| AG-02 | P0/HEAD | `src-tauri/src/agent/mod.rs:360-363,377-382,396-397` | 三种 approach 或 iteration budget 耗尽后形成 hard blocker，并要求用户采取动作 | 恢复耗尽明确属于系统；凭据/文件也未要求先穷尽替代路径和合并请求。
| AG-03 | P0/HEAD | `src-tauri/src/agent/mod.rs:448-452` | execute 模式把任意“用户必须提供的 credential/file”视为终止条件 | 缺少 `core_input_required` 的不可推导性、已尝试路径、一次请求、自动续接字段。
| AG-04 | P0/HEAD | `src-tauri/src/agent/mod.rs:1648-1654` | 只尝试两个方案后即可 ask；机器可检查等待仅靠 prompt 约束 | “两次失败”不是用户门禁；应交给 durable remediation，且不能靠模型自律保证。
| AG-05 | P2/HEAD | `src-tauri/src/agent/mod.rs:1852-1868,2007-2023` | fact checker 只识别少量“完成后回复”等词组，并只纠正部分环境断言 | 文案变体、英文变体、结构化 tool output 和 UI CTA 都可绕过；需要状态校验而非关键词。
| AG-06 | P0/WT/SAFE | `src-tauri/crates/agent-loop/src/policy.rs:163-215`、`run.rs:1160-1224,2109-2131,2180-2211,2255-2335` | 证据不完整映射为 `incomplete/failed_internal/platform_incident`，`ReleaseWithWarning` 不再完成业务状态，且不要求“回复继续” | 方向正确；但尚未发布，且状态目前只停止当前 turn，没有统一 remediation owner。
| AG-07 | P1/WT | `src-tauri/src/agent/persistence.rs:162-177` | `failed_internal/platform_incident` 被视为 terminal 并写 `completed_at` | transport/turn 可以终止，但 objective 不能被结算；需拆分 `turn_settled_at` 与 `objective_completed_at`，incident 必须带 remediation id。
| AG-08 | P1/HEAD | `src-tauri/src/agent/mod.rs:2026-2034` | 注释仍以“用户决定何时完成”为 interactive 完成边界 | 完成应由 objective + acceptance evidence 判定；用户可停止或改目标，但不应靠再次催促推动未完成执行。

### 4.2 Tool 与 permission

| ID | 级别/证据 | 生产命中 | 当前机制 | 风险与判定 |
| --- | --- | --- | --- | --- |
| TP-01 | P0/HEAD | `src-tauri/src/agent/mod.rs:2498-2554,2635-2690` | standard 默认允许文件写，但所有 bash 询问；safe 对所有 mutation 询问；trusted 才允许普通命令 | 用户已明确“搞定/发布”时，普通测试、构建、git/gh 探测仍成为人肉队列。应按 objective 授权 + 命令 side-effect 分类自动执行。
| TP-02 | P0/HEAD | `src-tauri/src/agent/permission_gateway.rs:36,111-147`、`agent-loop/src/services.rs:46-65`、`run.rs:1419` | 60 秒未响应或 channel closed 被当成会停止 tool chain 的 denial | timeout/channel close 不是用户拒绝；应重建通道、持久等待或转 platform incident。只有 `DeniedByUser` 必须绑定并停止。
| TP-03 | P0/HEAD | `src-tauri/src/tools/bash.rs:62-63` | sandbox 缺 Docker 时要求用户安装/启动或关闭 sandbox | Docker/PATH/runtime 是系统阻断；优先自动启动/修复。关闭隔离会改变安全边界，安全默认应保留隔离并报 platform incident，不能用普通重试文案诱导降级。
| TP-04 | SAFE/HEAD | `src-tauri/src/tools/shell_policy.rs:79-104` | `rm -rf`、`git reset --hard`、registry/disk/boot 等高风险操作独立 Ask/Deny | 这是合理安全边界；后续应由结构化 `side_effect=destructive/irreversible` 驱动，而非按工具名一刀切。
| TP-05 | SAFE/HEAD | `src-tauri/src/agent/permission_gateway.rs:133-138` | 明确 `DeniedByUser` 不允许换工具绕过 | 正确；需与 timeout/channel closed 分离持久化，避免把无回应伪装成拒绝。

### 4.3 Scheduler 与 sub-agent

| ID | 级别/证据 | 生产命中 | 当前机制 | 风险与判定 |
| --- | --- | --- | --- | --- |
| SC-01 | P0/HEAD | `src-tauri/src/agent/scheduler.rs:62,511-780` | 每个任务最多 3 次；验收/verification 失败会换 attempt，但耗尽后写 `failed` | 有界 retry 是合理保护，但耗尽后应创建 remediation job 或 `failed_internal`，不是用户可重试任务。
| SC-02 | P0/HEAD | `src-tauri/src/storage/tasks.rs:344-404` | retry API 明确只重置“user explicitly confirmed”的 failed/cancelled task | 技术失败的恢复触发被设计成用户责任，与原则直接冲突。
| SC-03 | P0/HEAD | `src-tauri/src/storage/tasks.rs:470-630` | provider、permission、shell runtime、unknown 被标为 `repairable:false`，next action 指向设置/PATH/授权；只有 test/verification 为 repairable | 这些大多是系统可恢复或 platform incident；`repairable:false` 被 UI 投影为“需要你”。
| SC-04 | P1/HEAD | `src-tauri/src/agent/subagent.rs:244-261` | `agent.run` 返回后 `SubagentResult.completed=true`，之后才附 acceptance check | agent-loop 内部失败/不完整可能先被总结为完成；应由 typed `RunOutcome.stop_reason + evidence` 唯一决定 completed。
| SC-05 | SAFE/HEAD | `src-tauri/src/agent/scheduler.rs:511-780` 与 attempt journal/DispatchGuard 路径 | 同一 attempt 保留失败证据，verification 输出进入下一轮，panic/orphan 可回 pending | 是 remediation supervisor 的可复用基础，但需从三次本地循环升级为持久、跨重启、可换 route/工具的恢复。

### 4.4 Provider/model failover

| ID | 级别/证据 | 生产命中 | 当前机制 | 风险与判定 |
| --- | --- | --- | --- | --- |
| PF-01 | P0/HEAD | `src-tauri/src/agent/failover.rs:88-113` | 仅 endpoint unavailable、rate limit、credential unavailable、quota 允许跨端点 failover；`AuthExpired/ContextOverflow/VisionUnsupported` 不允许 | 同目标存在等价健康 route 时仍提前失败；应按能力与 side-effect 安全性切换，而非按错误名固定禁用。
| PF-02 | P0/HEAD | `src-tauri/src/agent/model_transport.rs:330-357`、`commands/chat.rs:745-746`、`components/MessageList.tsx:671-674,849-854`、`ChatGptAuthRecovery.tsx:69-74` | 强制 refresh 后仍 401 就要求重新验证，并明确“失败回合不会自动重放/请重新发送” | OAuth 可能是真 core input，但验证成功后必须从安全 checkpoint 自动续接；有副作用时依 receipt 去重，不能让用户重述。
| PF-03 | P0/HEAD | `src-tauri/src/agent/model_transport.rs:132-157` | key 缺失/钥匙串不可读直接输出“保存后重试/允许一次访问” | 未结构化记录已尝试 routes、managed identity、request key 和 resume cursor；同 objective 可能碎片化多次询问。
| PF-04 | P0/HEAD | `src-tauri/src/agent/failover.rs:440-455` | 全 route 失败后要求用户检查服务/余额或手动选端点 | 服务/限流/单模型失败应由平台 incident/remediation；只有所有替代路径耗尽且确缺外部凭据/额度时才一次请求核心输入。
| PF-05 | P1/HEAD | `src-tauri/crates/agent-loop/src/run.rs:591-600,875-879` | vision 不支持/被拒绝时要求用户切模型重试 | 固定 route 有风险，但自动策略应选择等价 vision route；无可用 route 才判 core input 或 platform incident，不能要求用户 replay。
| PF-06 | P1/HEAD | `src-tauri/src/agent/failover.rs` 的 endpoint health registry | 有确定候选顺序、健康 cooldown、在未产生可见输出/副作用前跨 route | 正确但健康状态和 failover cursor 非 durable；App 重启后可能重复打坏端点，partial stream 后也缺少安全新 segment 续接。

### 4.5 Context/token budget

| ID | 级别/证据 | 生产命中 | 当前机制 | 风险与判定 |
| --- | --- | --- | --- | --- |
| CX-01 | SAFE/HEAD | `src-tauri/crates/agent-loop/src/context.rs:13-25,135-261`、`run.rs:744-833` | 75% 预压缩、单消息硬上限、overflow 后 80% emergency budget 再试一次，原始 DB 历史不改 | 已覆盖主要单次爆量故障，是正确系统恢复基础。
| CX-02 | P1/HEAD | `src-tauri/src/agent/mod.rs:1126-1260`、`agent-loop/src/run.rs:750-768,805-817` | Anthropic 路径明确 `context_compression=false`，overflow 不进入 emergency compression | context 是系统阻断；应统一 compactor 或自动切到可承载 route，不能因 provider 风格而终止。
| CX-03 | P1/HEAD | `src-tauri/crates/agent-loop/src/context.rs:224-254` | 至少保留两个 user turn，且 user content 永不压缩；最小骨架仍可能大于窗口时停止压缩 | 需持久 objective digest、附件/大输入外置引用和可验证 resume snapshot；不能让用户删历史或说“继续”。
| CX-04 | P2/HEAD | context state 仅为单次 run 内存状态 | 没有 durable `context_snapshot_id/digest/version/resume_cursor` | crash/restart 后无法证明续接使用同一 objective 与关键证据，恢复成功率也不可对账。

### 4.6 Delivery 与 release

| ID | 级别/证据 | 生产命中 | 当前机制 | 风险与判定 |
| --- | --- | --- | --- | --- |
| DR-01 | P0/HEAD | `src-tauri/src/agent/delivery.rs:226-235` | 外部 side effect 不确定被设为 non-recoverable，并要求“核对远端事实，再人工续接” | 远端 read-only reconcile 是系统工作；应查询 canonical PR/head/release receipt，只有不可观测时 platform incident。
| DR-02 | P0/HEAD | `delivery.rs:896-980,2205-2237,2518-2549` | BEHIND/CI/deploy/live wait 内含“重新调用 deliver_changes”文案 | WT 的 tool loop 已能同调用退避，文案和 outcome 仍会诱导模型/用户重触发；应统一 `waiting + retry_after + resume_cursor`，不暴露 recall CTA。
| DR-03 | P0/HEAD | `delivery.rs:2214-2221,2755-2775,2791-2809` | CI 确定失败、capability gap 等映射 `AgentActionRequired`；缺 live verifier 仅写 next action | 需要持久 repair job 实际读取日志、修代码/配置、产新 head；不能只有“系统必须修复”的文本承诺。
| DR-04 | P0/HEAD | `delivery.rs:3482-3492` | DIRTY 明确“需要人工解决”，draft 也变 NeedsAction | merge conflict、标记 ready 是技术执行；前者应创建冲突修复 worktree，后者在已授权 ceiling 内自动执行。
| DR-05 | P0/HEAD | `delivery.rs:1730-1780` | 多 worktree、默认分支、expect branch 不一致时要求用户切分支 | durable root-turn/task + change-set digest 应确定目标；无法确定是内部身份缺失/incident，不应让用户代替系统定位 cwd。
| DR-06 | P1/HEAD | `delivery.rs:1564-1666,1784-1794` | 缺 actuator 时主动降低 effective ceiling，并执行较低层级 | 可保留已安全完成的低层级证据，但 requested acceptance 必须不可变、状态不得 completed；能力缺失进入 remediation/core input，不得以降级作为完成。
| DR-07 | P0/HEAD | `delivery.rs:1851-1856,2598-2635` | push/review channel 缺失时要求用户登录、配 token/hook 后再试 | 需先穷尽 git credential、`gh`、broker、managed provider；真缺首次外部授权时合并为一次 core-input request 并自动续接。
| DR-08 | P1/HEAD | `delivery.rs:2390-2399` | `Release-Urgency: hold` 批次要求人工设置 `allow_guarded_batch=true` | 这是少数可能合法的不可逆业务/质量决策，但当前没有 `decision_key/options/recommendation/business impact/safe default`，不能作为裸手工步骤。
| DR-09 | P0/HEAD | `.github/workflows/auto-release.yml:98-108,485-524` | 缺 PAT、version PR check 失败/冲突/behind/超时直接 job failure | workflow 是执行器而非恢复 owner；应产结构化 incident/repair artifact，并由 supervisor 同一 batch 自动修复/重跑/reconcile。
| DR-10 | SAFE/WT | `.github/workflows/release.yml:20-70`、`auto-release.yml:51-96` | 缺 macOS signing credentials 在 mutation 前 fail closed，输出 `platform_incident`、owner、missing names、`requires_user_continue:false` | 正确止血；仍需由 release controller 判断是平台维护还是一次 `core_input_required`，workflow 不应泄露 secret 值。
| DR-11 | P1/WT | `src-tauri/src/agent/delivery_run.rs:1-213,523-650`、`tools/delivery.rs:140-383`、`lib.rs:505-557` | durable identity、lease、immutable requested ceiling、startup claim 和 waiting 自动续接已出现 | 目前只自动 resume `status=waiting && next_action_authorized`；技术等待不要求 autonomous flag，但 `agent_action_required`、`platform_incident`、core input 回填后的续接仍没有统一 worker，startup resume 失败仅日志 warn。

### 4.7 Browser/session

| ID | 级别/证据 | 生产命中 | 当前机制 | 风险与判定 |
| --- | --- | --- | --- | --- |
| BR-01 | SAFE/HEAD | `src-tauri/src/browser/install.rs:1-160`、`chromium.rs:214-244` | managed Chromium 可检测损坏、系统 Chrome fallback、按需下载/修复 | 正确方向；但 executable error 仍要求用户到 Settings 下载/安装，agent execution path 没有自动调用 installer。
| BR-02 | P0/HEAD | `src-tauri/src/tools/browser_session.rs:263-288` | attach 失败会关闭/删除 lease，并要求安装扩展、填配对码后重试 | 已登录浏览器的首次 pairing 可能是真 core input，但必须保留 objective、一次请求、等待连接并自动 attach；公开页面应自动 fallback managed browser。
| BR-03 | P0/HEAD | `src-tauri/src/agent/mod.rs:2556-2590`、`browser/policy.rs:1-76` | 所有 click/fill/press 每次 Ask，即使 ephemeral profile 或用户已授权完整目标 | DOM act 不等于业务决策。应识别 submit/pay/delete/publish/security expansion 等 side effect；普通导航/填表/可逆 click 在 objective 授权内自动执行。
| BR-04 | P1/HEAD | `src-tauri/src/tools/browser_session.rs:44,150-288,641-662,735-742` | lease 20 分钟无活动即过期；任何 surfaced error 关闭 owned session | 2FA/CAPTCHA/外部审批等待可能超过 TTL，且错误会丢失登录上下文；需要 `waiting_core_input` heartbeat、owner identity 和可恢复 session snapshot。
| BR-05 | SAFE/HEAD | `src-tauri/src/browser/chromium.rs:350-434` | headed 默认为用户完成 sign-in/2FA，launch/new-page 失败均清理进程和 profile lock | 真实身份/2FA 是合法 core input；清理机制正确，但应区分“等待外部输入”与“launch 技术失败”。

### 4.8 UI 文案与动作

| ID | 级别/证据 | 生产命中 | 当前机制 | 风险与判定 |
| --- | --- | --- | --- | --- |
| UI-01 | P0/HEAD | `src/pages/Workspace/WorkspacePage.tsx:205-222,857-945` | 失败任务显示“可重试/需要你处理/打开模型设置/调整权限/已修复，重试/回到对话处理” | UI 把 scheduler 技术分类直接变成人工工作台；应展示系统恢复阶段、owner、下次尝试和证据，只有 core input/业务决策才有 CTA。
| UI-02 | P0/HEAD | `WorkspacePage.tsx:666` | 点击修复会向聊天注入“请继续处理失败任务…”用户消息 | 这是产品伪造用户再提示，直接抬高“可避免用户再提示率”；必须由内部 resume command 续接，不进入 user transcript。
| UI-03 | P0/HEAD | `components/PermissionDialog.tsx:111-137` | 每个 Ask 显示 60 秒倒计时，超时后 execution 停止 | 对真正不可逆授权可保留决策卡，但普通命令不应出现；超时应转系统等待/incident，而不是制造紧迫的人肉 SLA。
| UI-04 | P0/HEAD | `MessageList.tsx:671-674,849-854`、`ChatGptAuthRecovery.tsx:69-74` | auth 恢复成功后仍要求用户明确重新发送 | 应显示“已恢复，正在从 checkpoint 续接”，并依 tool/delivery receipts 防重复副作用。
| UI-05 | P2/HEAD | `UpdaterBanner.tsx:21-68`、`UpdateStatusPill.tsx:52-70` | 正常终端用户可选择稍后/安装/手动检查，错误可点击重试 | 这是产品更新偏好，不直接算 agent blocker；但当 objective 明确要求升级时，agent 路径应自动 install/retry，不得让该 UI 成为完成门禁。

## 5. 合法的人类输入与当前误分类

### 5.1 真正 `core_input_required`

以下输入在穷尽系统路径后可以请求，但不是业务决策：

- 首次外部身份授权、OAuth/2FA/CAPTCHA 或不可替代生产账号授权；
- 只有用户持有的文件、license、法律主体信息；
- 用户浏览器的首次 extension pairing，且目标确实依赖现有登录态、managed browser 无法替代；
- 发布签名材料确由外部 repo/admin 控制且没有受管 release identity。

当前缺口：这些路径大多只有自然语言错误，没有 `request_key`、完整 missing inputs、attempted routes、最小输入、resume stage、request_count=1 和 objective lease。

### 5.2 真正 `needs_business_decision`

仅在没有安全默认且不可安全撤销时成立：

- 改变用户可见产品范围/验收口径；
- 删除或迁移不可逆数据语义；
- 成本、发布时间、质量风险之间不可代选的取舍；
- 扩大安全权限/凭据授权边界；
- 解除明确的 `Release-Urgency: hold`，且依赖/批次风险无法由规则判定。

普通测试失败、merge conflict、选择模型、选择唯一 worktree、刷新 token、重新 attach、安装依赖均不属于该类。用户已明确授权“发布/搞定”时，mark-ready、等待 CI、同 PR 修复和发布验证也不应再确认。

## 6. 系统提案与优先级

优先级采用 `(Impact + Risk) × (6 - Effort)`，各维度 1-5；分数用于同批排序，不是工期承诺。

| 优先级 | 提案 | I/R/E | 分数 | 机制改动 |
| --- | --- | --- | --- | --- |
| P0 | P-01 统一 DecisionRouter 与状态 envelope | 5/5/2 | 40 | 所有 blocker 必须先过 typed router；禁止模块直接构造“请重试/需要你”作为控制语义。
| P0 | P-02 Durable RemediationSupervisor | 5/5/3 | 30 | 持久接管 scheduler/provider/permission/delivery/browser/context；换 approach/route、退避、reconcile、跨重启续接。
| P0 | P-03 Objective 授权与 side-effect gate | 5/4/3 | 27 | 将 permission 从 tool-name/per-call 提升为 objective capability + side-effect；`autonomous_completion` 自动 apply recommended。
| P1 | P-04 Context snapshot 与跨 route resume | 4/4/3 | 24 | objective digest、大输入外置、provider-neutral compaction、partial stream 新 segment。
| P1 | P-05 Release batch remediation | 4/5/4 | 18 | workflow 结构化 incident/repair artifact；冲突/behind/check failure 自动进入同 batch 修复。
| P2 | P-06 UI projection 与 forbidden-copy lint | 3/3/2 | 24 | UI 只投影 typed state；CI 扫描生产代码中的裸 `needs_user/回复继续/请重试/人工续接`。

### P-01：统一 DecisionRouter

新增单一 blocker envelope，至少包含：

```text
objective_id, root_turn_id, autonomous_completion,
blocker_class, decision_type, failure_signature,
repairable, recovery_owner, approach_id, attempt,
next_action, next_action_authorized, resume_cursor,
requested_acceptance, reached_acceptance,
evidence_complete, incident_id
```

`core_input_required` 额外强制：

```text
request_key, missing_inputs[], attempted_routes[], minimal_input,
resume_stage, request_count=1, objective_lease
```

`needs_business_decision` 额外强制：

```text
decision_key, mutually_exclusive_options[], recommended_option,
business_impact_by_option, why_system_cannot_choose, safe_default_action,
irreversible=true
```

数据库/serde/UI 三层都拒绝 generic `needs_user`。`autonomous_completion=true` 时 schema 拒绝普通 `needs_business_decision`；只有不可替代 core input 可以等待。

### P-02：Durable RemediationSupervisor

以现有 task attempt journal、endpoint failover、delivery run/receipt、browser lease 为基础，建立统一 worker：

- `observed -> diagnosing -> repairing -> verifying -> waiting_retryable -> completed`；
- 同 failure signature 连续失败触发 approach switch，不是用户 CTA；
- permission timeout/channel closed 重建 channel 或转 platform incident；
- provider auth/context/vision 优先切等价 route，再决定 core input；
- CI conflict/check failure 创建绑定 repo/PR/head 的 repair attempt；
- browser core-input wait 保活 session，输入满足后自动续接；
- recovery exhaustion 写 incident/remediation row，不把 objective 标 completed。

### P-03：Objective 授权与 side-effect gate

引入 action metadata：

```text
capability, side_effect_class,
reversible, destructive, external_commitment,
within_objective_authority, recommended_action
```

规则：

- read、测试、构建、普通文件 mutation、可逆 browser action、同 PR 修复、CI/release 状态核对在已授权 objective 内自动执行；
- 明确用户 denial 永久绑定相同 objective/action signature；
- timeout/channel closed 绝不等于 denial；
- 付款、发送、发布承诺、删除、权限扩大等按业务后果判断；若用户原 objective 已明确授权且参数确定，也不重复确认；
- 无法判断 DOM 按钮语义时先读 DOM/可访问性/页面状态，不因工具名 `click` 自动询问。

## 7. Given/When/Then 验收

### P0 正向验收

1. **恢复预算耗尽**
   - Given：`autonomous_completion=true`，同 failure signature 已用完当前 approach budget；
   - When：Agent 无法在本段完成；
   - Then：创建 `failed_internal/platform_incident + remediation_id`，objective 仍未完成，UI 无“继续/重试”CTA，transport Done 正常清 streaming。

2. **Permission timeout**
   - Given：普通可逆 bash/build 已在 objective 授权内；
   - When：permission channel 关闭或 60 秒无响应；
   - Then：不得记录 `DeniedByUser`，不得停止 objective；系统重建执行或进入 incident。

3. **Provider failure**
   - Given：首选 route auth expired/context overflow/vision unsupported，存在等价健康 route，且尚无不可重放 side effect；
   - When：首选 route 失败；
   - Then：自动切 route，持久 route attempt，用户不选模型、不重发原消息。

4. **Scheduler verification failure**
   - Given：后台任务三次 attempt 后仍同类验证失败；
   - When：本地 retry budget 耗尽；
   - Then：任务进入 system remediation 或 failed_internal，不出现“重试失败步骤/回到对话处理”，不注入伪 user message。

5. **Delivery reconcile**
   - Given：PR create/merge/release 请求超时且远端结果不确定；
   - When：tool 恢复；
   - Then：按 repo/PR/head/receipt read-only 对账，复用 canonical 对象，不重复 PR/release，不要求人工核对。

6. **Auth core input**
   - Given：所有 managed credentials、CLI、broker、refresh 和等价 route 均失败；
   - When：确需首次 OAuth/外部账号授权；
   - Then：同 objective 只生成一次合并 core-input request；授权成功后从 checkpoint 自动续接，不要求重述或“继续”。

7. **Browser pairing/2FA**
   - Given：目标必须使用现有登录态，managed browser 无法替代；
   - When：extension 未配对或需要 2FA；
   - Then：进入 core-input wait、browser/session lease 保活；连接/验证完成后自动 attach/resume。

8. **Completion evidence**
   - Given：assistant 有最终文本但 acceptance evidence 不完整；
   - When：transport stream 结束；
   - Then：stream 清理成功，但业务状态不是 completed，正文明确阶段性结果且无“回复继续”。

### 边界验收

1. **明确拒绝**
   - Given：用户明确拒绝一个 action signature；
   - When：模型换工具尝试等价副作用；
   - Then：系统继续拒绝，不以 autonomous completion 绕过。

2. **不可逆业务选择**
   - Given：两个方案改变实质业务结果、无安全默认且不可撤销；
   - When：router 判定 `needs_business_decision`；
   - Then：缺任一结构化字段即 schema 拒绝；有完整字段时只请求一次。

3. **Release hold**
   - Given：batch 含 `Release-Urgency: hold`；
   - When：用户只说“尽快发布”但未提供解除 hold 所需的业务判断；
   - Then：不绕过；显示结构化影响/推荐/安全默认。若 hold 条件已由机器事实解除，系统自动继续。

4. **不可降低验收**
   - Given：requested ceiling 为 `through_release + live verification`；
   - When：缺 release actuator/live verifier；
   - Then：可保存 PR/CI 等低层级证据，但 objective 仍未完成，requested ceiling 不变，系统进入 remediation/core input。

5. **重复副作用**
   - Given：认证或进程在 tool side effect 后中断；
   - When：系统恢复；
   - Then：依据 idempotency key/receipt 对账，绝不盲 replay。

## 8. 测试与历史文档（不计作生产能力）

### 8.1 当前测试透露的契约

- WT 新增的 agent-loop/persistence 测试覆盖 `ReleaseWithWarning -> failed_internal`、`Done` 清 streaming、`failed_internal` 不等于 completed；这是必要回归，但尚非 released/live evidence。
- `scripts/verify-repository-intent-headless.mjs:113-132` 当前仍把“先处理失败项”和“已修复，重试”作为验收行为，等于把人工技术恢复固化进真实浏览器测试；修复 UI 后必须同步反转该契约。
- `src/components/MessageList.gate.test.tsx` 只验证 gate warning 可见，不证明 objective 被 remediation supervisor 续接。
- scheduler、failover、browser lease 的单测证明局部机制，不证明跨模块、跨重启、同 objective 自动恢复。

### 8.2 历史/设计文档冲突

- `docs/design/durable-delivery-recovery-architecture-design.md:32` 已在本工作树迁移为 `core_input_required/needs_business_decision`，但仍只是未发布设计契约，不能倒推为当前正式版行为。
- `docs/plans/topbar-status-settings-task-ux.md:19` 仍把检查模型配置/重试委派作为 pending 的用户提示。
- `docs/specs/feature-specs/durable-delivery-recovery.md:21`、新三类 durable-delivery 设计和 macOS release trust 设计已与新原则对齐，但均是规格/工作树证据，不是正式版 live evidence。

## 9. 发布阻断线

在以下最小闭环完成前，本改进不应发布为“已解决”：

1. 所有生产模块统一 typed decision envelope，CI 拒绝 generic `needs_user`；
2. permission timeout、scheduler exhausted、provider exhausted、delivery uncertain、browser core-input wait 均有 durable owner；
3. UI 删除技术 retry/continue CTA 和伪 user message 注入；
4. 完成状态、transport 状态、objective 状态分离；
5. 通过上述 P0 Given/When/Then 的跨重启集成测试；
6. 在正式版真实路径验证：锁屏/重启、单模型失败、permission channel closed、CI wait/conflict、OAuth 恢复、browser 2FA、release receipt 对账；
7. 用最近 24 小时正式版会话重新计算“非业务阻断用户回交率”，目标 0%，并确认没有靠隐藏错误或降低验收达成。
