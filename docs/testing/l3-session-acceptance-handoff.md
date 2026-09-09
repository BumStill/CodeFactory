# 会话类 L3 验收与双线交接

## 先给结论

E2E-001/002/003/007 的现有 L3 缺口要求**真实 CodeFactory Tauri WebView 窗口中的业务路径**。真实 Chrome（包括有布局和 SQLite 的 headless Chrome）可以验证前端或集成前置，但不能单独划掉这四项真实桌面缺口。`pnpm:test:startup-session:headless` 已登记，表示已有低层覆盖，并不表示 E2E-002 的所有证据层均已完成。

这不是新增加的门槛：沿用 [统一规格](../specs/feature-specs/scenario-test-governance.md) 的 CF-STG-R7/R8/R9/R11/R12/R13/R28/R29/R30/R31，以及 [唯一机器注册表](scenario-registry.json) 各 case 的 steps、fault injection、oracles 和 remaining_gaps。本交接不改证据分类、receipt schema、场景状态或线上规则。

## 共同验收条件

1. **测试的是产品。** 按该 case 实际涉及的路径，使用该次构建的真实 Workspace、消息输入、历史加载、Tauri IPC、Rust ToolBackend/supervisor、持久状态及前端事件路径。不能使用独立 acceptance 页面、mock IPC/AppHandle、直接塞前端终态或测试中复制的状态机来代替。provider 可使用合成且确定性的本地轨迹；不得依赖生产账号。故障注入只制造失败条件，不直接写入期望的通过终态。
2. **安全隔离先成立。** 独立 identifier、合成旧库/工作目录、凭据与配置隔离、只允许已审查的本地 fixture 请求、受管进程和浏览器租约。单改 HOME、数据库目录或 wrapper 指针不等于隔离。不得在生产库制造故障、访问真实聊天或借用真实 token。#515 当前 Synthetic 入口只开放设置能力，**不开放聊天、数据库和恢复监督器**，不是这四项业务测试可以直接使用的完整运行环境；扩展业务 fixture 必须先审查，不能偷偷放开 normal setup。
3. **证据属于同一次运行。** 绑定 run、构建提交、实际运行 executable 摘要、fixture/driver/verifier 版本、合成 session/root/objective 身份。记录真实窗口操作与状态时间线、对应持久字段、进程出生身份、副作用计数。wrapper 日志的目标 checkout/commit 只是一项来源，不能单独替代运行二进制身份。公开材料仅匿名投影，不上传生产 ID、账号、原始聊天、凭据或真实路径。
4. **跨重启是真的重启。** OS 观察旧进程退出及新进程出生；新进程重开同一隔离 SQLite。不得用重新 mount React、刷新页面或重建内存对象代替。App 的启动/重启由测试驱动负责，不因此补发用户“继续”。界面证据必须与实际状态变化绑定，不能把另一次 backend smoke 和另一次截图拼成完整 case。
5. **能识别失败。** 每个子场景在 fixture 中声明 deadline 和重启后观察窗口；超过期限、缺事件、错身份、只出现 spinner、在用例约定输入之外等待技术性“继续”或人工提示、重复副作用均失败。按下表同时覆盖成功与边界，至少有故意破坏关键结果的反例，证明观察器会拒绝；不能只检查窗口打开、HTTP 200、非空数组或顶层 `ok`。
6. **清理可证明。** 成功、失败、超时和取消均回收本次进程树、工作目录及租约。记录实际回收，不把 runner 销毁当 cleanup，不把不可观测计数写为 0。只能清理已确认归属本 run 的资源。

## 四个场景逐项判定

| 场景 | 必须在真实窗口跑通 | 必须验证的边界和重启结果 |
| --- | --- | --- |
| E2E-001 无人值守恢复 | 在真实输入框发送一次任务；制造可恢复的 provider/tool 故障及进程中断；重开后同一任务自动推进到完成，UI 与持久终态一致 | user message=1、human prompt=0；同一 root/objective；不同进程接管；幂等副作用恰好一次；无 live owner/claimable remediation。另测真实不可恢复条件能稳定进入既定终态；不能用不可恢复分支替代本应成功的可恢复路径，也不能要求用户补发“继续” |
| E2E-002 历史会话继续 | 从合成旧 schema/分页历史启动 App；在真实会话列表选择历史 session，输入简短“继续”；立即出现与后端事件对应的真实进度 | 开放 objective 不在首屏消息页、没有旧内存 run control；重启后仍续接同一 objective/root；同一输入重试只 admission 一次；多个开放 objective 时拒绝猜测身份，不能继续错任务。只关闭本次已覆盖的 L3 缺口，不冒充旧 schema 全矩阵完成 |
| E2E-003 停止后不复活 | session 中有跨消息页的多个 live objectives；在 active/waiting_system 且未 streaming 时点击真实停止按钮；UI 必须由持久取消结果确认停止 | 取消与 supervisor claim/UI listener 并发；注入投影失败时显示错误且不伪装停止成功；重试成功后全部 live objectives cancelled/explicit_cancel；两次独立进程重启后 live owner/claimable remediation=0；重复点击不创建新 objective |
| E2E-007 恢复耗尽后界面收敛 | 真实 ToolBackend 拒绝无 change_reason 的计划变更，随后严格只读审计真实成功；重复技术失败触发生产恢复上限，supervisor 在没有新 claim 时仍发布持久化后的终态，真实 UI 接收 | 拒绝计划不增 revision/外部 receipt；unknown receipt 保留；同 root 的 steer 前后片段及瞬态工具全部结束，只显示一个结束回执；无旧 spinner/Stop/ETA，原因可见且 requires_user_action=0；迟到旧 root 终态不停止新 active root；两个新 PID 重开后旧策略不复活，settlement/stream/run 字段一致 |

每行是一个现有 case 的验收映射，不是另造四个新场景。一个 case 可拆为多个受控子运行；每个子运行的 UI/数据库/进程/副作用证据必须自洽绑定，不能跨子运行拼接缺失的 oracle。macOS 的一次成功不自动证明 Windows，开发构建的成功不自动证明 L4 安装版。

## 什么时候可以划掉 L3 gap

- **前置通过：** headless/集成脚本通过。保留对应低层证据，L3 gap 不变。
- **真实窗口取证通过：** 上述业务断言在一次受控 native 执行成立，可记录真实桌面证据；仅有人工或 agent 临时逐步操作、截图和文字总结时，仍不能称“L3 自动化已完成”。
- **L3 自动化验收完成、待登记：** 版本化可执行入口可重复驱动上述路径；独立检查原始证据、反例和全生命周期清理通过，身份绑定齐全。提交明确的入口、平台、运行阶段、预期产物和失败退出码给 Codex 复核。
- **L3 自动化 gap 关闭：** 上述 target/driver/verifier/fixture 经明确批准的信任根升级接入同一 Harness 和明确阻断阶段；保护恢复后取得非空、精确构建身份的真实运行及独立复算证据，才修改对应 L3 gap。若还有旧 schema、nightly 或 L4 缺口，case 保持 partial。

不要求一定使用 GitHub 托管 macOS。可以使用获准的本机或受管桌面 runner，前提是入口、调度、归属回收和可信验证满足同一合同；不能因托管 runner 前置条件未知而降低证据层。

## 归属与不撞车边界

- **Claude 线：** E2E-001/002/003/007 的业务 fixture、真实 WebView 驱动、匿名原始取证与可重复验收入口。headless 只作前置。业务隔离所需扩展先列出具体接口与文件，与 Codex 的 #515 基础核对后再改共享入口。
- **Codex 线：** #515 的原生 preflight 诊断、现有 settings-only 隔离基础和进程观察；不开发上述四个业务脚本、不占用 Claude 的 wrapper/数据目录。随后优先闭合 E2E-010 的发布 target 缺口，再推进 E2E-004 和更完整的信任链升级。
- **登记由 Codex 汇总：** 普通业务 PR 不改逐字节冻结的 registry/judge/ruleset。L3 原始产出和下述待登记补丁进入 Bootstrap-2 审查清单；实际迁移范围、diff、回滚及保护恢复方案明确后再取得对应批准。受影响发布 target 的最小升级可先独立交付，不必等待全部业务 L3 完成。
- **发版只有一位操作者：** 本轮不触发发布。Codex 的“发布预检错误已消除”通知不是“正式产物已通过”；移交时分别说明检查、产物、公开版本状态。Claude 接手 dispatch 前再次确认没有另一条发布运行，复核完整未发布批次和 exact main，按受控 hold 流程执行。

## 待登记补丁接收

输入为仓库主 checkout 的 `.claude/deferred/scenario-registration-hlt006-e2e012.patch`，原件只读保留。后续审查包括 HLT-006/E2E-012、#511/#513 的既有测试绑定和派生文档，不只新增两个名称。

- 三个新增 history-session 阶段已在源码中，不等于 registry 已登记；正式登记前重新核对当前 main、真实调用入口及所有目标的阶段绑定。
- 新 case 仍保持部分实现，未取得真实 WebView 和生产定时清扫的同 run 证据前，不删除其 L3/定时行为缺口。
- 补丁中的行号、派生数量、旧 receipt 字段与当前代码可能已漂移，不能原样 apply 后直接算通过；派生文档从最终 registry 重新生成。
- 不从“生产库曾清理五条”推导这四个 L3 case 已通过，也不复用生产内容充当 synthetic fixture。

2026-09-09 只读接收复核（main `5472df7d`）：四个 old blob 与 main 一致，`git apply --check` 通过，未应用原件。但登记前仍需修正：

1. HLT-006 声称可见故障，`required_evidence` 却漏 L3；补上对应层级，不能靠遗漏证据要求消掉缺口。
2. README 的受管数量将变成 12 个 case，但受管块外仍写“11 个复杂旅程”；派生块和解释文案都要复核。
3. `history_session_smoke.rs` 的 `scenario_ids` 仍只有 E2E-002/003/007，`abandoned_phase_registry_status` 仍说 HLT-006/E2E-012 未登记。状态只能在实际登记交付时同步修正，不提前声称已注册。
4. 当前 trusted case adapter 只执行 E2E-001 的完整 PR case 回执；给 E2E-012 声明 history binary 并不会自动获得该 case 的五类同 run oracle。要明确“已有低层 PR target”与“完整 case 回执”的差别。
5. abandoned 三阶段使用真实进程和 SQLite，但清扫由测试入口直接调用，尚非生产 timer 触发；只有 user message=1 查询也不足以独立证明 human prompt=0。保留这些证据边界，不能凭新增 ID 升绿。

## 本次协作记录

- context scope：当前 registry/spec、#515 原生诊断与 settings-only 隔离、待登记补丁；不访问生产库或凭据。
- assumptions：沿用现有 L3/L4 区分，provider 可确定性合成，真实业务 UI/IPC/持久化不能替身化。
- review point：独立复核逐项 oracle、补丁绑定和“人工取证/自动化/可信登记”三个边界。
- validation result：本文件是验收交接，尚未产生四项 L3 通过证据，也未修改 registry 或线上规则。
