# CodeFactory 锁屏无关 Agent 验收：架构设计

## 架构

```text
product settings + OS credential
              |
              v
tools/agent/run_runtime_acceptance.py
  - resolve endpoint / active model
  - start Rust runtime sidecar
  - execute requested commands in a macOS workspace-write sandbox
  - use a minimal child-process environment and redact evidence
  - write redacted evidence
              |
              v
codefactory-agent-headless
              |
              v
codefactory-agent-core
  - shared system contract
  - command policy
  - completion gate
```

## 共享边界

- Runtime driver 只负责配置解析、进程协议、本地命令执行和证据落盘。
- Prompt、策略、完成门禁和完成证据由 `codefactory-agent-core` 与 Rust runtime 决定。
- core 提供 `ProductPolicy`；`BenchmarkPolicy` 只在 benchmark profile 上叠加隐藏 verifier/solution 限制，普通产品验收不会因合法 `tests/` 或 `solution/` 路径被拒绝。
- Desktop `AgentLoop` 继续负责 Tauri 事件、结构化工具、权限、SQLite 会话和历史回放。
- Desktop 与 headless 的共享完成语义由 core tests 保证；Desktop 特有的历史持久化由主库集成测试保证。

## 锁屏策略

- Runtime driver 启动时记录 macOS 锁屏状态，但不要求屏幕解锁。
- OS credential 查询设置有限超时；锁屏验收只使用环境显式密钥或已授权 credential。首次授权失败时返回明确 blocker，不等待不可见授权弹窗。
- GUI 验收包装器使用 `caffeinate` 持有 display、idle 和 system sleep assertion，直到 dev App 退出。
- 用户主动锁屏时不尝试解锁；Runtime 验收继续。PR 和发布固定运行 GitHub macOS GUI check，二者不通过 Runtime 锁屏事件动态 dispatch，流程也不出现待解锁阻塞。

## 远端可见会话

- PR workflow 在 GitHub macOS runner 构建本次精确 debug App bundle，以隔离 HOME 启动，并绑定启动 PID 观察稳定主窗口。
- 验收脚本保存窗口字段和窗口截图，验证截图尺寸、非空像素以及与本次启动 PID 的绑定；workflow 上传证据 artifact。
- 发布 workflow 对本次 DMG 挂载、复制、卸载后启动精确临时副本并执行同一观察；已发布 tag 可再次手动复验。
- provider/runtime、desktop payload、远端 GUI、发布产物四层证据组合构成交付门禁；任一适用层失败都不得宣称上线。

## 证据分级

| proof tier | 能证明 | 不能证明 |
| --- | --- | --- |
| `agent-runtime-no-gui` | provider/model 路由、共享 Agent 决策、真实 shell 工具、完成门禁 | Tauri 事件、SQLite UI 刷新、视觉布局 |
| `desktop-integration` | Desktop 历史修复、持久化和 provider payload | 用户看到的最终界面 |
| `remote-real-app-gui` | 本次精确 App bundle 的启动、稳定窗口、可渲染截图和隔离数据目录 | 不单独证明真实 provider 结果 |
| `released-artifact-gui` | 已发布 DMG 的安装副本、版本、架构、稳定窗口和截图 | 不单独证明 Agent 任务质量 |

## 安全与隐私

- 密钥只存在于 driver 和 sidecar 子进程内存，不写命令行、轨迹或结果文件。
- sidecar 与工具进程只继承工具链所需的环境白名单，`HOME` 和 `TMPDIR` 指向每次运行的临时目录。
- macOS 工具命令通过 `sandbox-exec` 只允许写入所选工作区和临时运行目录；当前不宣称限制工作区外只读访问。
- stdout/stderr 和最终结果在回传、落盘前统一截断并脱敏已知密钥和常见敏感赋值。
- driver 默认不允许网络型 shell 命令；模型 provider 请求仍按产品 endpoint 发送。

## 平台边界

- v1 的锁屏检测、Keychain credential 和工作区写入 sandbox 是 macOS 产品路径。
- Windows/Linux 在提供等价 OS 写入隔离前 fail closed，不降级为未隔离命令执行。
