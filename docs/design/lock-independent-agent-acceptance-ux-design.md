# CodeFactory 锁屏无关 Agent 验收：UX 设计

## 使用路径

1. 验收者选择真实项目目录和任务说明。
2. Runtime driver 自动读取 CodeFactory 当前 endpoint 和 active model。
3. 终端持续显示阶段、工具数量、完成状态和证据目录，不显示密钥。
4. 结束时输出明确 proof tier；提交 PR 后由独立远端可见会话继续 GUI 验收，不要求用户回来解锁。

## 交付状态文案

以下 `remote_gui_*` 是 PR check / evidence 状态，不是当前 Runtime driver 已实现的应用内状态，也不表示 Runtime 会主动 dispatch GitHub workflow。

- `running`: 真实 provider 和 Agent Runtime 正在执行。
- `passed`: 完成门禁满足，结构化证据已写入。
- `blocked`: 配置、credential、provider 或命令执行存在明确阻塞。
- `remote_gui_queued`: Runtime 已验证，远端 macOS 可见会话正在生成 GUI 证据。
- `remote_gui_passed`: 本次精确 App bundle 的窗口和截图证据已通过。
- `remote_gui_blocked`: 远端 runner 或 App 验收失败；发布被阻止，但不要求用户解锁本机。

## 约束

- 不显示“锁屏已绕过”或暗示系统安全机制被关闭。
- 不把 `agent-runtime-no-gui` 显示为“完整桌面验收”。
- 本地 GUI 预演一旦开始，应自动防止空闲休眠；用户主动锁屏后保留 Runtime 进度。PR/发布的远端 GUI check 独立运行，不依赖本机状态。
