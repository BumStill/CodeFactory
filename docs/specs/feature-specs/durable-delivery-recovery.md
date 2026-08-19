# 持久交付恢复与结构化完成语义规格

- 状态：已批准实施
- 批准来源：用户要求基于 1.78.4 正式版复测，快速完成全部修复并发布
- Primary User Path：用户授权实现与交付后，系统持续推进同一工作树、同一 PR、CI、合并、发布与正式版验证；除真实用户门禁外，不要求再次发送“继续”

本规格是 delivery domain contract；跨域 objective 真相、typed decision 与唯一完成写入以 `objective-recovery-control-plane.md` 为准。任何 delivery `failed_internal/platform_incident` 都是 system-owned remediation 状态，不是 objective 终态。

## Requirements Traceability

| Req ID | Requirement | Implementation surfaces | Minimum evidence |
| --- | --- | --- | --- |
| CF-DR-R1 | 非终态交付跨 App 重启可恢复，30 秒内由新 owner 接管 | SQLite migration、DeliveryRun store、startup supervisor | lease/crash SQLite integration + packaged App restart |
| CF-DR-R2 | 恢复始终绑定 repo/base/change-set/expected head/canonical PR；远端未知或多匹配时 `create_count=0` | DeliveryRun identity、Git receipt、remote reconciler | fake remote contract + real PR |
| CF-DR-R3 | 可修复 CI 失败在同一 PR 内完成日志诊断、最小修复、验证、push 和再观察；本地 repair budget 耗尽后转 durable remediation，仍绑定同一 objective/PR/head | delivery failure classifier、repair attempt | failure-first + exhaustion/restart + GitHub runner same-PR canary |
| CF-DR-R4 | 心跳只表示 liveness；`last_progress_at` 只在可证明阶段/远端/证据变化时更新 | DeliveryRun events、turn projection | no-progress heartbeat test |
| CF-DR-R5 | 证据不完整、未达 requested ceiling 或有 agent-owned 未交付变化时不得业务 `completed` | Completion Arbiter、agent-loop finalization | policy/run failure-first tests |
| CF-DR-R6 | transport `Done` 与业务终态分离；UI 明确显示恢复中、等待、需用户、内部失败 | stream event、chat hydration、status components | frontend tests + real App |
| CF-DR-R7 | 旧 active/pending 无稳定身份时只标 legacy orphan，禁止猜测恢复或创建远端对象 | startup migration/recovery | legacy DB migration test |
| CF-DR-R8 | macOS 资产具备可审计签名/校验边界；有 Apple 凭据时完成 notarize/staple，缺失时不得伪造 signed | auto-release workflow | codesign/spctl/stapler + updater asset smoke |
| CF-DR-R9 | 人只处理关键业务判断；所有技术/执行/恢复阻断由系统持有 | decision router、agent-loop、delivery、scheduler | blocker taxonomy contract tests |
| CF-DR-R10 | `needs_business_decision` 必须携带 decision key、互斥选项、推荐、业务影响和安全默认动作 | structured outcome、UI | schema + UX tests |
| CF-DR-R11 | 非业务恢复耗尽进入 `failed_internal/platform_incident` 与 remediation queue，保持非终态 owner/lease，不生成任何人工技术恢复动作 | recovery supervisor、incident projection | exhaustion + restart + forbidden-CTA integration |
| CF-DR-R12 | 用户要求搞定或离开时持久化 `autonomous_completion=true`，推荐配置在授权范围内自动采用 | objective policy、decision router | restart + recommended-default tests |
| CF-DR-R13 | 外部核心输入仅在穷尽安全替代后一次性合并请求；缺失不得降低功能、签名、测试、发布或 live 要求 | input request contract、remediation | missing-credential acceptance |
| CF-DR-R14 | 程序内更新先进入持久队列；只有 backend 持有精确 target mutation permit，且本地执行 owner、未过期 Objective/Delivery 租约、权限、浏览器与终端均归零后，才可下载、安装和重启。无 live lease 的 `waiting_core_input`、`waiting_authorization`、`waiting_business_decision` 与 `legacy_orphan` 是可跨重启的 durable wait，不得伪装成执行中 blocker；安全状态未知时 fail closed | update safety command、updater store、状态 UI | frontend retry contract + Rust owner/lease aggregation + real App |
| CF-DR-R15 | 等待更新安全点是非失败 observation：重复观察不得消耗 technical recovery budget、不得进入 `technical_recovery_exhausted`；真正的 manifest、网络、签名、receipt 或安装错误仍使用有界恢复 | Objective remediation strategy、Update adapter | 超过恢复上限的安全点轮询 + 真实错误预算测试 |
| CF-DR-R16 | 历史 `domain=update + waiting_core_input + technical_recovery_exhausted` 必须兼容恢复：同一 target 保留 Objective identity 并递增 recovery generation；新 target 到来时旧目标以可审计 `legacy_orphan/update_target_superseded` 结算后新建目标。不得伪造 applied receipt、重放 unknown receipt 或直接修改生产 DB | update objective admission、decision/event audit | 历史状态 SQLite 集成 + exact/new target tests |
| CF-DR-R17 | 更新 UI 必须陈述真实阶段：取得 mutation permit 前只能称“已排队”，不得称“已下载”；只展示实际 live blocker owner，归零后说明将自动下载、安装并重启 | updater store、banner、settings、status pill | copy/component tests + real App |
| CF-DR-R18 | 下载和安装由 backend 持有的精确 target permit 执行时，必须向 renderer 投影真实阶段与单调字节进度；迟到的“已排队”快照不得覆盖 `downloading/installing`，丢失事件时仍由持久 receipt fail closed | Update adapter、Tauri event、updater store、进度 UI | backend callback + renderer race test + signed updater |
| CF-DR-R19 | 更新重启预留必须与所有新工作入场串行化，包括不在 chat/task runtime map 中的 Objective remediation claim；预留期间非 Update domain `claim_count=0`，释放后才可继续 | AppState admission gate、Objective supervisor、update safety | 真实 SQLite claim gate + active-owner safety test |
| CF-DR-R20 | Objective 终态必须单调：更新/崩溃发生在 Objective 完成与 turn/task 投影落盘之间时，启动恢复必须以终态 Objective 或 durable done journal 修复投影，不得重置为 Pending/Waiting；旧 revision 和迟到 transport heartbeat 不得覆盖终态，journal 与 task completion 必须同事务 | Completion Arbiter、journal、startup recovery、chat projection | crash-window SQLite fixtures + stale projection CAS + signed relaunch |

## Completion predicate

系统仅在以下条件全部满足时写 `completed`：

1. 最后一次本任务 mutation 后存在相关、成功且新鲜的验证；
2. 无未解决失败验证；
3. 本任务 agent-owned change-set 与 baseline 可区分；
4. `reached_state >= requested_ceiling`；
5. Delivery 场景的 commit/head/canonical PR/CI/merge/release/live 证据与远端一致。

`ReleaseWithWarning` 只能产生非终态进度提示；不得写 `completed_at` 或“任务已完成”。

## Recovery boundaries

- `wait_retryable`：系统退避后自行续接，不消耗 repair budget。
- `agent_action_required`：绑定 failure signature 的有界修复，默认最多两次。
- `apply_recommended`：存在明确推荐、可逆且在授权范围内时直接执行，尤其适用于无人值守 objective。
- `needs_business_decision`：仅选项会改变不可逆关键业务结果、没有安全推荐默认时使用。
- `core_input_required`：仅外部控制且无法替代的最小输入；必须合并一次请求并保持 objective 自动续接。
- `external_state_uncertain`：只读对账，禁止重复外部写动作。
- `failed_internal/platform_incident`：系统恢复耗尽或平台配置缺失，明确归系统并进入 remediation queue，不伪装成用户门禁。
