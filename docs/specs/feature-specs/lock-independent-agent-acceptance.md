---
req_id: CF-AGENT-ACCEPT
title: 锁屏无关 Agent Runtime 验收
status: approved
created_at: 2026-07-15
updated_at: 2026-07-15
tags:
  - agent
  - acceptance
  - runtime
acceptance_criteria:
  - 锁屏状态不阻止真实 provider-backed Agent Runtime 完成工具闭环
  - Runtime 读取 CodeFactory 当前 endpoint 和 active model，且证据不包含 API key
  - GUI 验收包装器在运行期间持有 macOS 防休眠 assertion
  - 结果明确区分 agent-runtime-no-gui、desktop-integration 和 real-app-gui
---

# 需求

| Req ID | 需求 | 验收证据 |
| --- | --- | --- |
| CF-AGENT-ACCEPT-R1 | 提供 benchmark 无关的本地 Runtime 验收命令，执行真实模型请求和真实工作目录命令 | fake sidecar 协议测试 + 真实 provider canary |
| CF-AGENT-ACCEPT-R2 | 默认从 CodeFactory `settings.json` 解析 default endpoint、active model、base URL 和 key ref | 配置 fixture 测试 |
| CF-AGENT-ACCEPT-R3 | 从环境显式密钥或 OS credential 读取授权；sidecar 与工具进程使用环境白名单，密钥及常见敏感赋值不得出现在参数、日志和证据 | environment/redaction 测试 + evidence scan |
| CF-AGENT-ACCEPT-R4 | macOS 锁屏只记录状态，不阻止 Runtime；credential 查询必须有超时 | 锁屏检测测试 + 锁屏实跑 |
| CF-AGENT-ACCEPT-R5 | macOS 本地命令从选择的 cwd 启动，并通过 OS sandbox 只允许写入该工作区和临时运行目录；记录返回码和脱敏截断输出后回传 Rust runtime | 协议集成测试 + 工作区外写入拒绝测试 |
| CF-AGENT-ACCEPT-R6 | 输出 proof tier、contract SHA、provider/model、工具轨迹、completion evidence 和最终状态 | result schema 测试 |
| CF-AGENT-ACCEPT-R7 | dev App wrapper 在 macOS 下通过 `caffeinate` 防止空闲锁屏，生命周期与 App 进程一致 | wrapper 静态测试 + 进程 smoke |
| CF-AGENT-ACCEPT-R8 | GUI 行为仍要求解锁后的真实 App 成功和边界路径验证；Runtime 结果不得冒充 GUI 证据 | PR evidence review |
| CF-AGENT-ACCEPT-R9 | 产品 Runtime 使用通用产品策略；隐藏 verifier/solution 限制只由 benchmark adapter 启用，不得误拦普通项目的 `tests/` 或 `solution/` 路径 | core/headless policy tests |

# Primary User Path

验收者在真实项目目录运行 Runtime 验收，CodeFactory 使用当前产品模型配置完成任务并输出结构化证据。运行期间即使屏幕因用户离开而锁定，模型和工具闭环继续；需要验证布局、点击、滚动或流式显示时，再在解锁后的真实 App 补充 `real-app-gui` 证据。

# Applicable Harnesses

- Spec Harness
- Compatibility Harness
- Observation Harness
- AI Collaboration Harness
- Release Harness（进入发布包后）
- Viewport Harness（仅 GUI 行为）

# 测试矩阵

| 层 | 成功路径 | 边界路径 |
| --- | --- | --- |
| 配置 | active model + key ref 正确解析 | stale model、缺失 endpoint、credential 超时明确失败 |
| 协议 | tool request 执行并回传，最终 completion evidence 完整 | sidecar 非法 JSON、超时、非零命令保留错误、工作区外写入被 OS sandbox 拒绝 |
| 证据 | 结果包含 proof tier/contract/model/trajectory | API key 和敏感环境不落盘 |
| macOS | 已解锁和锁屏都可运行 Runtime | GUI 被锁时标记 pending，不宣称 GUI 通过 |
| GUI wrapper | dev App 生命周期内持有 caffeinate assertion | App 退出后 assertion 自动释放 |

# 当前证据

2026-07-15 的真实 DeepSeek canary 在 `CGSSessionScreenIsLocked=Yes` 下完成普通 Python 源码兼容修复，状态为 `passed`，完成证据无 blocker，独立零残留和 `4 / 4` 测试复核通过。随后增加的通用产品策略、macOS 工作区写入 sandbox、环境白名单和统一脱敏均有独立回归测试。证据见 `docs/evidence-packs/codefactory-agent-runtime-locked-2026-07-15.md`。
