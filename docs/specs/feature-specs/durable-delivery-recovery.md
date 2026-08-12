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
| CF-DR-R14 | 程序内更新可下载但不得安装或重启仍有本地执行 owner 的 App；安全状态未知时 fail closed，归零后自动续接安装 | update safety command、updater store、状态 UI | frontend retry contract + Rust owner aggregation + real App |

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
