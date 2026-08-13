# Objective Recovery Control Plane 规格

- 状态：已批准实施
- 顶层原则：用户授权目标和边界后，CodeFactory 必须持续持有所有可恢复的技术状态；只有不可安全推导的核心输入、没有安全默认的不可逆业务决策、显式拒绝或取消，才允许中断并回交用户。
- 适用面：chat root turn、task、tool、permission、model route/auth、browser、DeliveryRun、release/update、completion projection、App restart。
- 产品完成边界：本规格的完成必须同时有实现、跨进程故障注入、CodeFactoryDev 真实主路径、PR/CI/merge、正式 release、安装版复验和 24 小时生产指标；局部单测、transport `Done`、已合并 PR 或发布元数据都不是完成。

## Requirements Traceability

| Req ID | Requirement | Minimum evidence |
| --- | --- | --- |
| CF-ORC-R1 | 同一目标跨 root turn、tool future、task、delivery 与 process 保持唯一 `objective_id`；新的用户 steer 只能增加 revision，不能创建替代目标 | continuation + restart SQLite integration |
| CF-ORC-R2 | 身份字段按 objective kind 强制：分析型使用 typed scope，代码变更使用 repo/worktree/base/head/change-set，交付型再加入 requested ceiling。任何适用身份冲突必须在副作用前 fail closed；revision 只能在 lease 与 receipt 证明后前进 | kind matrix + collision + two-worktree zero-side-effect tests |
| CF-ORC-R3 | 所有 blocker 先形成共享 `DecisionEnvelope`；system-owned 技术状态不得产生 terminal user CTA，只有 `core_input_required` 或 `needs_business_decision` 可回交 | schema/serde/router table tests |
| CF-ORC-R4 | `CompletionArbiter` 是业务 `completed` 的唯一写入者，并按 informational/local-mutation/delivery/live objective kind 校验适用证据；transport `Done`、无依据的 `noop`、候选总结或单次 tool success 都不是完成证据 | kind-aware completion + projection integration |
| CF-ORC-R5 | startup 与常驻 `RemediationSupervisor` 在 30 秒内 CAS claim 已失效的 system-owned lease，并先 observe/reconcile；chat、permission、task、provider/auth、browser、delivery、release 都必须注册 domain adapter | paused-clock claim race + process restart |
| CF-ORC-R6 | `external_state_uncertain` 在证明副作用未发生前只读对账；同一 canonical PR/head/release/browser submit/外部 mutation 至多一次 | fake remote/side-effect counters |
| CF-ORC-R7 | 明确 transient provider failure 在 root turn 零输出、零副作用且预算未耗尽时同回合退避或切兼容 route；预算耗尽转 durable remediation，不回交用户 | scripted provider failure -> success + restart |
| CF-ORC-R8 | 已有可见输出、tool intent/result、未知副作用、auth/policy/payload incompatibility 时禁止盲重放；从持久 checkpoint、receipt 与 resume cursor 续接 | latch/classifier/reconcile boundary tests |
| CF-ORC-R9 | `turn_settled_at`、`stream_closed_at` 与 `objective_completed_at` 分离；阶段性内容和错误通知不得投影成业务完成，刷新后系统持有状态仍可见 | hydration + component + SQLite truth |
| CF-ORC-R10 | 创建 provenance 不可变；最后观察 provenance、queue/runtime、attempt、stable failure code、owner 与真实 progress 独立记录；heartbeat 不得冒充 progress | migration + persistence tests |
| CF-ORC-R11 | 旧非终态记录身份不足时只标 `legacy_orphan`，不猜 objective，不执行 provider/tool/remote mutation | legacy migration test |
| CF-ORC-R12 | 下一条真实用户消息到来时才能写 `user_reprompt_driver`，并以仍开放 objective 关联，不做关键词计数、不伪造用户消息 | continuation attribution test |
| CF-ORC-R13 | `objectives` 是跨域业务状态真相源；turn、task、tool、delivery 只做 projection。数据库拒绝 system-owned objective 被写成 `completed/blocked`，拒绝未带证据的 Complete | DB constraints + illegal transition tests |
| CF-ORC-R14 | permission 必须区分 `denied_by_user/timed_out/channel_closed/cancelled/policy_denied`。timeout/channel close 仅结束当前工具等待并进入 system remediation；显式允许后同一 objective 自动续接，不需要“已授权，重试” | fake-clock permission + channel restart + UI tests |
| CF-ORC-R15 | scheduler 的本地 attempt 预算只是 approach budget。技术失败耗尽后创建 durable remediation、退避并自动重派；只有显式取消或结构化 core input/business decision 才停止 session | scheduler exhaustion + app restart + no-manual-retry tests |
| CF-ORC-R16 | auth/credential/2FA/CAPTCHA 先穷尽 refresh、broker、受管身份和等价 route；确需核心输入时同 objective 只请求一次，输入满足后从 checkpoint 自动续接且不要求重述 | auth recovery + request-count + replay-fence tests |
| CF-ORC-R17 | browser pairing/登录态/2FA 等待保留 objective 与 lease；公开页面自动切受管浏览器，确需现有身份时进入 core input wait，满足后自动 attach/resume | browser lease + managed fallback + 2FA resume smoke |
| CF-ORC-R18 | UI 只能投影 typed objective state：system-owned 显示 owner、阶段、最近真实进展和下次观察，不显示“继续/重试/重新发送/回到对话”；只有 core input/business decision 显示一次 CTA | reducer/component/forbidden-copy + real App |
| CF-ORC-R19 | release controller 对 version PR/check/conflict/branch policy/远端不确定建立同 batch remediation；CI 绿但 merge policy 阻止直接合并时自动选择受支持的 auto/queue 路径，不把 job failure留给用户 | workflow contract + real repository canary |
| CF-ORC-R20 | 正式产品聚合并发布非业务回交、自动恢复成功率与时延、ownerless duration、duplicate side effect、false complete、requested-ceiling downgrade 指标；发布门禁拒绝违反零容忍指标的候选 | aggregate query + release gate + 24h production review |
| CF-ORC-R21 | Spec/实现/测试必须反向追踪。治理 validator 拒绝与本规格冲突的人工技术恢复文案、旧测试契约和缺失 Req 证据；安全边界文案不受误伤 | semantic validator positive/negative fixtures |
| CF-ORC-R22 | Objective 只有 `completed/cancelled` 两种终态；waiting/recovering/platform incident/awaiting input/decision 均非终态，attempt failure 不得写 Objective terminal | state property tests + SQLite constraints |
| CF-ORC-R23 | chat、tool、permission、task、provider/auth、browser/terminal、delivery/release、update 必须注册统一 `ObjectiveDomainAdapter`；未实现能力只能落 system incident owner，不能降级为人工 CTA | adapter conformance registry + missing-adapter negative test |
| CF-ORC-R24 | 通用 remediation queue 持久化 owner、lease、failure signature、strategy/approach、attempt、next observation 和 receipt；局部预算耗尽只能扩大退避或换策略 | exhaustion + restart + no-CTA integration |
| CF-ORC-R25 | 用户 steer 只修改同一 objective revision/capability；显式 cancel/deny 才能终止或禁止等价副作用；技术 reprompt 不得改变 authority | steer/cancel/deny matrix |
| CF-ORC-R26 | App update/restart 聚合全部 active objective owner；不安全时自动等待，归零后安装，启动后全域 claim；`streaming=false` 不能误判安全 | update aggregation + packaged restart |
| CF-ORC-R27 | Project、Quick、Task、subagent 共用相同状态语义；anonymous 因隐私策略不持久化时必须在开始前说明不能跨 App 重启 | surface matrix + anonymous contract tests |
| CF-ORC-R28 | decision/attempt/event/receipt 不保存 secret、raw prompt、完整工具参数、OAuth URL 或真实外部 ID；指标只读聚合 | redaction/privacy tests |
| CF-ORC-R29 | migration 必须 additive，且 `ensure_schema` 幂等补齐；旧 active/terminal 只在身份唯一时回填，回滚版本仍可读取旧 projection | fresh/old/checksum-conflict/rollback fixtures |
| CF-ORC-R30 | fault injection 覆盖 permission、auth、429/503、tool timeout/panic、task exhaustion、context、process restart、remote unknown、CI failure、browser/terminal 与 update wait | cross-domain E2E matrix |
| CF-ORC-R31 | 只有精确正式安装包重走成功与边界路径，DB/side-effect 证据与 PR/CI/merge/release/live 一致，且 24 小时 KPI 达标，才允许结论为“满足” | packaged App evidence pack + production gate |
| CF-ORC-R32 | 本原则/规格进入 Feature Specs 索引和可执行治理规则；CI 拒绝未标 superseded 的技术 retry/continue/resend 契约以及缺 Req→test 映射 | governance validator negative fixtures |
| CF-ORC-R33 | 每个 fence 资源必须有明确的释放责任人和释放时机，不得只靠"下一次准入顺手关闭"。回合结束时必须释放该回合证据已确定的 provider episode（无未结副作用且所有 attempt 已终态）；`prepared`/`in_flight`/`streaming`/`unknown` 或存在未结副作用时保持关闭，交由 supervisor 观察 | 回合结算释放测试 + 不确定证据保持 fenced 的负例 |
| CF-ORC-R34 | 只读 bash 探查（含 `&&`/`;`/管道/丢弃型重定向的复合命令）不得被判定为需要观察契约的外部变更；判定按 segment 逐段进行，任一段不在只读白名单、含真实重定向、含命令替换或后台 `&` 则整条命令继续 fenced | 复合只读命令与逐段 fence 的双向单测 |
| CF-ORC-R35 | system-owned 恢复必须有界。持久 remediation 历史按 `(objective, recovery_generation, failure_signature)` 在一次用户授权代际内累计计数，并对该代际设总量兜底；任一上限达成后不得再排下一次 observation，必须以 `technical_recovery_exhausted` 进入 typed core input 等待并结算 transport turn。计数不因 failure code 变化而重置；用户明确新输入续接 exhausted Objective 时保留同一 Objective 与全部历史、递增 `recovery_generation` 并获得一份新的有界预算；用户驱动的 remediation（`apply_recommended` 与 `resume_authorized_action`）不计入预算 | 上限/同代签名抖动/跨代续接/再次耗尽/证据门禁/权限豁免单测 + 真实 app 复现 |

## 恢复有界性（CF-ORC-R35）

本规格的顶层原则要求"持续持有所有**可恢复**的技术状态"，CF-ORC-R24 要求"局部预算耗尽只能扩大退避或换策略"。2026-08-13 的现场证据表明这条补救路径在实现中从未发生：`objective_remediations.approach_index` 恒为 0（从不换策略）、退避恒为 ~13s（从不扩大），而每一轮都真实调用模型并产生费用。

因此本条明确边界：**不能恢复的状态不属于"可恢复"，持续持有它不是持有目标，而是空转**。

- 判定"无进展"的依据是同一用户授权代际内 failure signature 重复，而不是 failure code。代际内计数为累计而非连续 streak——否则中间插入一个不同的 failure code 就能把计数清零，同一条坏路径可以无限续命。跨代历史不删除；只有 exhausted 后收到真实用户新 turn 才递增代际，普通系统重试、进程重启或 permission 恢复都不能重置预算。
- `completion_evidence_incomplete` 明确计入：完成证据门禁驳回后重跑同一 prompt 得到同一答案，是无进展的典型形态。
- 达到上限后的出口是 `core_input_required`（CF-ORC-R3 允许的两种回交之一），不是 `completed`／`cancelled`，因此不违反 CF-ORC-R22 的双终态约束；objective 仍然存活，用户的下一条消息以 `core_input_response` 续接同一 objective。
- `core_input_response` 续接 exhausted Objective 时必须先持久递增 `recovery_generation`，再把同一 Objective 恢复为 `active`；新代际仍受相同 5/20 上限约束，不能因用户说“继续”永久解除熔断。
- 续接 turn 的持久身份是同一 Objective 的当前 `resume_cursor`；setup/settlement guard 必须接受原始 `root_turn_id` 或这个精确 cursor，不能把合法续接误判为 identity mismatch，也不能接受其他 session、Objective 或 turn。
- 上限只约束**系统自发**的重试。用户恢复能力（`CapabilityRestored`）与用户授权权限（`resume_authorized_action`）不消耗预算。
- 上限达成时 transport turn 必须结算并写入 `terminal_reason='technical_recovery_exhausted'`，否则界面会继续呈现"仍在恢复"而实际已无任何 observation 排队。

## Decision Envelope

允许的裁决为：

`continue | waiting | apply_recommended | platform_incident | failed_internal | core_input_required | authorization_required | needs_business_decision | complete | cancelled`

共享字段至少包括：

```text
objective_id, revision, root_turn_id, task_id, delivery_run_id,
domain, decision_type, failure_code, failure_signature,
recovery_owner, remediation_id, next_action, next_attempt_at,
next_action_authorized, requires_user_action,
output_started, side_effect_started, resume_cursor,
requested_acceptance, reached_acceptance, evidence_ref
```

- `platform_incident/failed_internal` 仍由系统 remediation 持有，不能要求用户回复“继续”。
- `core_input_required/authorization_required` 必须证明安全替代已耗尽，并带 `request_key`、`attempted_routes[]`、`missing_inputs[]`、最小输入、一次请求计数和输入后的自动续接点；授权必须绑定 action signature 与 revision。
- `needs_business_decision` 只用于不可逆且无安全默认的互斥业务结果，并带 `decision_key`、互斥选项、推荐项、各选项业务影响、系统不能代选原因和安全默认。
- identity 不唯一、provider 预算耗尽、CI/网络/数据库锁/进程重启、工具超时、permission timeout 都不是用户决策。
- 显式用户拒绝必须绑定 objective + action signature，系统不得换工具绕过等价副作用。

## Objective State Machine

```text
active
  -> waiting_system -> active
  -> waiting_core_input -> active
  -> waiting_business_decision -> active
  -> completed
  -> cancelled

waiting_system
  -> diagnosing -> repairing -> verifying -> active/completed
  -> platform_incident/failed_internal -> queued remediation

只有 CompletionArbiter 可以写 completed；只有显式 cancel/deny 可以写 cancelled。
```

`turn` 可以 settled、stream 可以 closed，但只要 objective 未 `completed/cancelled`，就必须仍有 durable owner、lease 或明确的 core-input/business-decision wait。

## Completion Predicate

`Complete(evidence_ref)` 必须同时满足：

1. objective identity 唯一且 revision/lease 合法；
2. 没有未解决 failure、pending tool 或未对账副作用；
3. 最后 mutation 后有新鲜验证；
4. objective kind 对应证据满足：informational 有来源/推理与已答问题，already-satisfied/no-op 有当前状态验收，local mutation 有 agent-owned change-set 与 post-change validation；
5. `reached_acceptance >= requested_acceptance`；
6. delivery/live 场景的 canonical PR/head/CI/merge/release/live 证据一致；
7. evidence ref 可从正式产品重新读取，不依赖聊天宣称。

## Primary User Paths

### P1：普通技术失败自动恢复

用户提交一次实施目标。模型 503、工具 timeout、permission channel close、测试失败或 CI 等待发生时，stream 可以结束，objective 卡仍显示“系统正在继续处理”、恢复 owner、最近真实进展和下次观察。系统在同一 objective 内退避、切 route、修复、验证并继续，无需用户回复。

### P2：必要核心输入后自动续接

所有受管凭据和等价 route 已耗尽，系统一次性请求 OAuth/2FA/不可替代文件。用户只完成该输入；CodeFactory 识别输入已满足后从 checkpoint 自动续接，不新建 user message、不要求“继续”或重述目标。

### P3：进程与更新恢复

App 在 provider、permission、task、browser 或 delivery 等待中退出或安装更新。新进程在 30 秒内 claim 同一 objective，先对账 receipt，再继续安全动作；原 objective 卡、requested acceptance 和 canonical PR/head 不变。

### P4：不可逆业务决定

系统遇到无安全默认、不可撤销且改变业务结果的选择，展示一次结构化决策卡和推荐项；选择后同一 objective 自动继续。缺少结构化字段时 router 拒绝回交。

### P5：正式交付与发布

用户要求交付到正式 release/live。PR、CI、merge queue、version PR、release workflow、artifact 与安装验证都属于同一 objective；分支策略或远端状态不确定由 release adapter 修复/对账。只有正式安装版主路径和证据满足 requested acceptance 后才 completed。

## Applicable Harnesses

- Spec Harness：CF-ORC-R1..R35 与所有 superseded 规格反向追踪。
- Compatibility Harness：旧 SQLite、旧 turn/task/delivery 状态、旧 auth 错误、旧设置和历史会话 hydration。
- Observation Harness：typed decisions、lease、owner、真实 progress、attempt、receipt 和 KPI。
- Release Harness：PR/CI/merge、正式 artifact、安装版和 rollback。
- Viewport Harness：1366×768、800×600、390×844 的 objective/core-input/business-decision 状态。
- Payload Harness：provider/tool/browser payload、secret、OAuth URL 和附件不得进入错误证据。
- AI Collaboration Harness：独立规划、架构、UX、QA 审查及 Req 验证结果。

## Testing Matrix

| Path type | Scenario | Expected result | Evidence |
| --- | --- | --- | --- |
| Primary | permission timeout/channel closed | current tool wait closes; objective remains system-owned and resumes without user retry | Rust fake clock + SQLite + component |
| Primary | provider 429/503 then healthy route | same objective and root turn continue; no duplicate output/side effect | scripted transport integration |
| Primary | scheduler verification exhausted | durable remediation + backoff + same session redispatch; no failed-task CTA | scheduler integration + UI |
| Primary | auth expired then reauth | one core-input request; successful auth auto-resumes checkpoint | auth coordinator + component + real App |
| Primary | browser pairing/2FA | lease kept alive; attach/resume after input | browser runtime smoke |
| Primary | CI/version PR branch policy | same batch auto/queue merge and release continuation | workflow fixture + repository canary |
| Restart | kill process in every domain wait | owner claimed within 30s; same objective/revision/receipt | packaged App fault injection |
| Safety | explicit deny/cancel | equivalent side effect remains denied; objective cancelled or safe alternative only | permission/action-signature test |
| Safety | side effect result unknown | read-only reconcile before any replay; counter remains one | fake external counter |
| Completion | transport Done/noop without evidence | stream closes but objective not completed | arbiter + reducer + SQLite truth |
| Compatibility | pre-0006 DB with open records | identity-complete rows migrate; ambiguous rows become legacy_orphan without mutation | migration fixture |
| UX | system-owned technical state | no retry/continue CTA; owner/progress/next attempt visible | component + 3 viewport screenshots |
| Safety | same failure signature repeats with no progress | recovery stops at the ceiling, turn settles as technical_recovery_exhausted, nothing left queued | objective/chat/scheduler ceiling tests + real app |
| Primary | user reprompts an exhausted objective in the same session | same Objective reopens in a new recovery_generation, setup settles against the exact resume_cursor, responds normally, and that generation still stops at its own ceiling | objective generation + continuation setup integration + real app |
| Metrics | injected avoidable reprompt/false complete | aggregate detects non-zero and release gate fails | analytics + governance tests |
| Release | formal installed build | all above smoke receipts match version/commit/artifact; 24h KPI meets thresholds | release evidence pack |

## Compatibility and Release Boundary

- 新表和字段全部 additive；旧消息、task、tool call、delivery run 和设置必须可读。
- 旧行只有在 objective 身份可由不可变外键链唯一证明时才关联；其余 `legacy_orphan`，禁止猜测或重放。
- 原有明确用户 deny/cancel、hard deny、destructive/irreversible side-effect gate 保持不变。
- `CF-MRC-R18` 的“恢复后要求用户重发”、`CF-SCC-R28` 的 permission timeout terminal blocked、`CF-FR-R2/R6` 的人工技术重试，以及 endpoint exhausted 的人工 retry CTA 均被本规格取代。
- 正式 release 与安装版完成前状态保持 `not live`。main/PR/CI/DMG metadata 不能替代安装版行为。

## Evidence Pack Requirements

- Req R1..R35 的实现文件、测试文件、命令与结果追踪表。
- SQLite migration/constraint/legacy fixture 和 objective transition journal。
- permission/provider/task/auth/browser/delivery/release 的成功、边界与跨重启 fault receipts。
- CodeFactoryDev 真实主路径截图/录屏，包含 system-owned、core input、business decision、completed 四态。
- PR、CI、merge、release、artifact digest、安装版本/commit 和 rollback 边界。
- 正式版 24 小时聚合：avoidable reprompt=0、system-owned user handoff=0、ownerless past lease=0、duplicate side effect=0、false complete=0、requested-ceiling downgrade=0。
- AI Collaboration：context scope、assumptions、独立 review point、validation result。
