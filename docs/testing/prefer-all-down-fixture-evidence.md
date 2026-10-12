# prefer-all-down 修复验收 / PR 正文

## Summary
修复 Windows CI #635 的 prefer-all-down 偶发失败，仅改变测试 fixture 与断言，生产恢复/回落逻辑不变。

## 根因
属于 fixture 协议时序错误触发错误文案分支，不是共享 endpoint 状态竞争。`unreachable_base_url` accept 后未读 POST 就写 502、关闭连接；未消费请求使 TCP 可能 reset，客户端收到含随机端口 URL 的传输错误而非显式 502。现有 `is_provider_overloaded` 以 `429/503/529` 子串匹配；相邻随机端口可能均含这些数字，三条失败被计入过载预算，第三次返回 `PROVIDER_RECOVERY_WAITING: provider overloaded`。本 PR 不扩展到生产分类器重构。

fixture 现在消费 Content-Length 对应的完整请求后才返回 502。健康注册表仍为每测试独立实例；生产跨 session 健康共享是有意保留的冷却语义，不改变。没有扩大超时、没有增加重试、没有 sleep；原文案要求与端点顺序断言保留，新增账本字段级断言。

## 复现方法与 TDD（Req ID ↔ test）
- CF-PFB-R7/R8：先新增 `unavailable_fixture_returns_http_502_after_consuming_request`，1 MiB POST 在旧 fixture 上真实失败：reqwest `Canceled / UnexpectedMessage`；修复后同一命令通过。
  `CARGO_INCREMENTAL=0 cargo test --manifest-path src-tauri/Cargo.toml --lib unavailable_fixture_returns_http_502_after_consuming_request -- --nocapture`
- CF-PFB-R2：`prefer_mode_waits_only_after_every_endpoint_is_unavailable`，逐一断言 ChatGPT/DeepSeek/OpenRouter、重新观测文案和三条 `failed_replayable/provider_rejected` 记录。
- 根因机械核验：`unread_request_transport_errors_can_hit_overload_budget_via_url_ports` 使用合成端口 54290/54291/54292，验证 URL 可触发 overload、三次预算返回 DurableWaiting，而显式 HTTP 502 不会。
- CPU 压力复跑（默认 N=100，避免以少量 smoke 代替调度稳定性；全套再跑 20 次）：
  `python3 scripts/verify-prefer-all-down-stress.py <cargo-test生成的codefactory_lib测试可执行文件>`
  脚本启动每逻辑 CPU 一个负载进程，直接从临时 cwd 执行测试二进制，每次检查退出码和测试计数，任一失败即停止而非重试，finally 回收所有进程及临时日志。

## Applicable Harnesses
- Spec：`docs/specs/feature-specs/prefer-all-down-fixture-preconditions.md`。
- Compatibility：仅 cfg(test) 变化，生产 provider API/策略不变；完整 transport / Rust workspace 回归。
- Observation：真实 loopback HTTP + SQLite failure_class/status/endpoint 字段断言。
- AI Collaboration：context scope = 当前 main 的 transport/failover/provider_recovery/http_util 与任务模板；assumptions = fixture 应交付显式 502；review point = TCP 请求消费、随机端口分类、独立健康实例；validation result = 先红后绿、压力循环与全套回归。按用户明确要求未调用 delegate_tasks。
- UI/浏览器/截图：不适用，仅测试基础设施变化；未发版，not live。

## Self-acceptance
| 验收项 | 结果 | 证据 |
| --- | --- | --- |
| 新回归先失败再修复 | PASS | 旧实现 UnexpectedMessage，同命令修复后通过 |
| 三端点及真实前置条件 | PASS | 保留文案与顺序；新增 failed_replayable/provider_rejected |
| CPU 压力循环 | PASS | 10 个负载进程，目标 100/100，全 transport 20/20，8 test threads |
| 完整 Rust workspace | PASS | cargo test --workspace --no-fail-fast，含 transport 与 SQLite 集成 |
| 前端测试与 build | PASS | 144 文件、956 tests；tsc + vite build |
| 场景本地门禁 | PASS | 使用 /opt/homebrew/bin/python3（支持 tomllib）；默认系统 Python 缺 tomllib 的环境失败已用现有运行时修复 |

正文 validator 在开 PR 前执行 scenario --ci 与 README validator。无待决 Spec Amendment。后续建议（不在本 PR 实施）：生产 overload 分类应避免对 URL 端口做裸数字子串匹配。

Scenario-Test: HLT-001, HLT-002, HLT-005
README-Update: reviewed
README-Update-Reason: 仅修复测试 fixture 的 HTTP 前置条件并新增回归与压力验证，不改变公开产品、安装、隐私或平台承诺。
