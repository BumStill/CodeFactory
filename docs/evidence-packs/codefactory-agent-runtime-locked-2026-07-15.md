# CodeFactory 锁屏无关 Agent Runtime 证据

## 结论

- status: `passed`
- proof tier: `agent-runtime-no-gui`
- macOS runtime state: `CGSSessionScreenIsLocked=Yes`
- provider: `deepseek`
- model: `deepseek-v4-pro`
- fixture: `/private/tmp/codefactory-product-eval-member-scan-v1`
- fixture type: 普通 Python 源码包，不是 benchmark task

这次运行证明锁屏不会再阻断 CodeFactory 的真实 provider 请求、共享 Agent core、真实工作目录命令和完成门禁。它不证明 Tauri 视觉布局、点击、滚动或状态刷新；这些仍属于 `real-app-gui` 证据。

## 闭环

第一次锁屏运行完成两处源码修复、零残留和 `4 / 4` 测试，但最终 evidence 为 `failed`。原因是共享门禁只接受 `exit "$grep_rc"` 形式，误拒绝了“保存搜索状态、`>1` 时明确 `exit 2`、最后 `test ! -s`”的等价可靠扫描，导致 30 个模型请求耗尽。

加入等价退出语义的失败回归并修复后，第二次仍在锁屏状态运行：

- 两处旧 API 修改完成。
- `last_source_mutation_sequence=11`。
- `last_source_scan_sequence=13`。
- `last_successful_project_test_sequence=13`。
- completion blockers: `[]`。
- tool calls: `11`。
- model requests: `12`。
- duration: `42437 ms`。
- project tests: `4 passed`。

完成证据满足后，Rust runtime 使用无工具 finalization round 收口，没有继续修改工作区。

## 独立复核

- 独立状态保留型 `grep` + `test ! -s` 零残留检查通过。
- 独立 `.venv/bin/python -m pytest -q` 为 `4 passed`。
- 复核结束时 `CGSSessionScreenIsLocked=Yes`。
- 证据目录扫描未发现 `api_key`、`CODEFACTORY_AGENT_API_KEY` 或 secret-shaped value。

## 完整性

- execution contract SHA-256: `67f7c4be4c2913e51189b9aea01055f6d091330638671f6826446abc31cd0ef3`
- result SHA-256: `fa6b221661d6e2d3648f3b258c2b2676709bf790c46f427e2676c6af6812ae47`
- trajectory SHA-256: `b74eb040c6fe4a716f9b43ff5f189c933f74e456381f96a0fe2b31b9689e5953`
- trajectory: `22` JSONL rows，`14042` bytes

## 剩余边界

- OS credential 第一次授权仍可能要求用户在解锁状态确认；已授权的 credential 和运行中的 Runtime 不依赖屏幕。
- 用户主动锁屏不会被绕过。
- Desktop 同会话历史修复仍需解锁后的真实 App 补一次 `real-app-gui` 验证后才能发布。

## 当前候选锁屏复跑

独立审查后，当前候选加入三项通用加固并再次在真实锁屏状态运行：

- 产品 Runtime 使用独立 `ProductPolicy`，不再继承 benchmark 对 `/tests/`、`/solution/` 的专用限制。
- macOS 工具命令使用工作区写入 sandbox；独立协议测试中的 `touch ../escaped.txt` 实测返回 `Operation not permitted`，工作区外文件未创建。
- 子进程环境改为白名单，证据在回传与落盘前统一脱敏；自定义 endpoint 的合法 `owner/model` ID 不再被错误截断。

当前候选的 DeepSeek canary 在开始与结束时均记录 `CGSSessionScreenIsLocked=Yes`：

- status: `passed`
- completion blockers: `[]`
- tool calls: `10`
- model requests: `12`
- duration: `64777 ms`
- independent residual scan: zero matches
- independent project tests: `4 passed`
- workspace write isolation: `macos-sandbox-exec`
- result SHA-256: `59d3203c94b747d9876608c7d8aab46309474955542ddf04a14992a4e9c69525`
- trajectory SHA-256: `e8a51db26699ebc6e97ee88c270934a33b800cc1df6f782c9513ea5ff8cd0d17`
- trajectory: `20` JSONL rows，`12034` bytes

本次 trajectory 中 `cd /home/user` 的失败原因是路径不存在，不能作为 sandbox 拒绝证据；sandbox 写边界只引用上面的独立真实命令测试。该结果仍是 `agent-runtime-no-gui`，不是 release artifact 或 `real-app-gui` 证据。
