# M77b / M85 返修验收与 PR 正文

## 变更
M77b：明确的 lowered ceiling 交付提示通过 delivered 契约，恢复字段矛盾仍拒绝。
M85：回落链按配置顺序显示失败及使用时段跳过、下一可用时间；主模型 transport failure 不再被末端 AUTH_MISSING 覆盖。不同端点失败才汇总，单端点 overload 等待语义不变。仅合成数据；未改工作流、注册表、校验器。

## Requirements / TDD / 自验收
| Req ID | 证据 | 结果 |
| --- | --- | --- |
| CF-M77B-R1 | a_ceiling_lowered_below_the_configured_one_says_so 新增 validate_contract 断言，先失败后通过；完整 workspace 交付与队列既有回归 | 契约 PASS；专项 CI→armed 时序尚未补齐 |
| CF-M85-R1 | mixed_exhaustion_keeps_primary_transport_cause_and_window_skip 先失败后通过，三环顺序、时段时间、TransportUnreachable 断言 | PASS |
| CF-M85-R2 | transport 统一 exhausted_error；既有 objective 路径将同一 error_text 写 record_failure_detail；workspace 包含 SQLite/objective/transport 集成 | 既有回归 PASS；混合链专项持久化精确一致测试尚未补齐 |

验证：完整 Rust workspace --no-fail-fast 通过；pnpm test 147 files / 984 tests 通过（含 lightModeAudit）；pnpm build 通过。过载等待回归曾失败，按不同端点计数修复后同命令全绿。治理基线通过。

## Applicable Harnesses
Spec：docs/specs/feature-specs/m77b-m85-repair.md。
Compatibility：合法 delivered 提示、矛盾恢复状态、单端点 overload / fixed routing 回归。
Observation：合成链原因、顺序、恢复分类与既有 objective 持久化路径。
Release：PR / CI / 合并队列，不发版；not live。
AI Collaboration：context scope 为批准任务、模板及 delivery/failover/model_transport/objective；assumptions 为合成数据和首个失败为恢复主因；review point 为 lowered 提示边界及副作用保护；validation result 为上述全量测试。采用单执行流最小 patch，未启用 sub-agent：返修共享状态契约且模板禁止 delegate_tasks。
无布局变化，不启用 Viewport；后台 deliver_changes / dispatch 入口保持不变。

## 真机验收分工及限制
真机验收由编排方发版后完成：安装新版后走 merged 交付，CI 绿后核对 armed 与 objective；重演 GPT 临时不可达、DeepSeek 时段跳过、OpenRouter 缺凭据，检查会话与 objective 审计说明一致，GPT 恢复后同一目标续接。
无待定 Spec Amendment。尚缺专项跨层混合链持久化与 CI→armed 时序断言，不把既有回归或单元测试包装成完整业务验收。

Scenario-Test: CXD-002, HLT-001, HLT-002, HLT-005
README-Update: reviewed
README-Update-Reason: 修复既有交付与回落契约和归因，不改变公开功能、安装及隐私承诺。
