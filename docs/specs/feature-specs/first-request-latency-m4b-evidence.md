# M4B 阶段证据与交付说明

实现范围：M4B-1/2/3/4；规格见 first-request-latency.md 的原始任务。新增无 GUI 测量入口 `node scripts/measure-first-request-latency.mjs`，运行正式二进制，使用空 HOME、合成 SQLite 会话、生产准入/路由/AgentLoop 与本地 HTTP SSE 端点，连续五轮并校验中位数。

## 五轮中位数（ms）

| 阶段 | 修复前 | 修复后 |
| --- | ---: | ---: |
| admission | 1.506 | 1.501 |
| workspace_checkpoint | 15.647 | 15.325 |
| route_quota | 0.021 | 0.015 |
| quota_refresh_lock | 0（本场景无 ChatGPT） | 0（本场景无 ChatGPT） |
| context_assembly（包含 skills/memory） | 0.489 | 0.427 |
| skills/memory | 0.273 | 0.212 |
| delivery_readiness | 126.283 | 0.116 |
| provider_post 准入 | 0.167 | 0.171 |
| admission→HTTP | 152.427 | 25.646 |

修复前样本总耗时：165.744、146.688、167.559、152.427、151.063。
修复后样本总耗时：36.242、27.157、25.646、24.151、24.151。

最大等待来自交付提示准备：三个本地 Git 命令经过 30ms 轮询包装，且无远程时仍探测 gh。改用现有 git2 读取本地远程配置，无远程立即返回。不修改额度、时段、权限门禁。

## 验证

- 新增 Node 契约测试先红（缺模块、缺 skills/memory 阶段），后 2/2 通过。
- 真实二进制构建通过；从临时目录运行五轮，断言 POST 路由、model、stream、已准入 user 消息、全部阶段与中位数。
- Rust workspace 全量通过：主库 1612 passed / 9 ignored，其余 workspace/integration/doc tests 通过。
- pnpm test：151 文件、1012 测试通过（含 lightModeAudit）；pnpm build 通过。

## 限制与未完成验收

不能声称历史 3.5 秒已全部归因：本次合成配置未复现该延迟；ChatGPT 过期额度网络刷新、真实技能规模、桌面 setup 的全部步骤尚未覆盖。首次基线仍调用 gh 探测，未强制阻断外网；这是无外网约束的证据缺口。fixture 复用生产函数但未调用完整桌面 send_message。阈值当前只保护 fixture 30 秒 deadline，不足以锁住此次 126ms 回归。完整门禁回归和严格网络隔离仍需补齐。未修改 workflows、场景注册表或校验器。

## Harness 与 AI Collaboration

Spec：原文保留；Compatibility：门禁未改；Observation：仅阶段名/耗时；Release：仅授权合并，不发布，not live。AI context scope 为请求准备、交付提示、合成 fixture；assumption 为 Git 包装等待，review point 为完整路径与隔离缺口；validation result 如上。任务模板禁止 delegate_tasks，单执行流。

真机验收由编排方发版后完成：安装新版本，以合成项目连续发送两轮；核对首 HTTP 与各阶段；有效/过期额度、越限、时段禁用、权限拒绝均核对首请求前门禁。没有 UI 修改。

Spec Amendment：无需求语义修改；上列证据缺口不作为已通过项。

Scenario-Test: HLT-001, HLT-002, HLT-003, HLT-004, HLT-005, CXD-001, RTE-004, SKL-002, SKL-003
README-Update: reviewed
README-Update-Reason: 内部首请求性能与合成测量入口，不改变公开安装、隐私、平台承诺。

## 2026-11-10 补充：夹具失效修复并接入 CI

**现象**：`node scripts/measure-first-request-latency.mjs <正式二进制>` 在 v1.91.87 到 v1.115.0 上都报 `first HTTP request timed out`，要等满 30 秒才结束。

**根因**（已验证）：M155 起（2026-10-22，`resolve_route_candidates`），路由计划只接受已保存凭据引用（`key_ref`）的端点。夹具里的合成端点 `key_ref: None`，于是在 `route_quota` 阶段就被排除，`resolve_route_plan` 返回「synthetic：尚未配置凭据」，根本没走到 AgentLoop。另外 `measure()` 先 `join` 假服务器线程、再看 agent 结果，这个错误被服务器的 30 秒 accept 超时盖住了。没有任何 CI 或脚本在跑这个测量，所以这次漂移一直没人发现。

**修复**：
- 合成端点改为声明凭据引用 `codefactory.endpoint.synthetic-latency-fixture`。这个引用不会被真正解析：夹具路由带内联合成 key，传输层优先使用内联 key，不会访问系统钥匙串。
- agent 在首个 POST 之前就结束时，通知假服务器停止等待，并优先报告 agent 的真实错误（带轮次）。
- 新增单测 `fixture_settings_are_admitted_by_the_production_route_planner`：它直接调用生产路由规划器，规划规则再次收紧时会在 `cargo test` 中变红。修复前先看到它失败，报的正是同一条错误。
- `check-rust`（Windows）新增「First-request latency fixture」步骤，复用前面 smoke 已经构建好的二进制。

**验证**（macOS 本机）：修复后五轮全部通过，admission→HTTP 中位数约 26 ms，脚本总耗时约 1.8 s。把 `key_ref` 临时改回 `None` 时，1.6 s 内报出 `round 0: …synthetic：尚未配置凭据`，不再伪装成超时。Windows 上的结果以本 PR 的 CI 为准。
