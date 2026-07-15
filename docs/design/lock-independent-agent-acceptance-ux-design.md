# CodeFactory 锁屏无关 Agent 验收：UX 设计

## 使用路径

1. 验收者选择真实项目目录和任务说明。
2. Runtime driver 自动读取 CodeFactory 当前 endpoint 和 active model。
3. 终端持续显示阶段、工具数量、完成状态和证据目录，不显示密钥。
4. 结束时输出明确 proof tier；锁屏状态只影响 GUI 证据，不把 Runtime 成功降级为“未运行”。

## 状态文案

- `running`: 真实 provider 和 Agent Runtime 正在执行。
- `passed`: 完成门禁满足，结构化证据已写入。
- `blocked`: 配置、credential、provider 或命令执行存在明确阻塞。
- `gui_pending_unlock`: Runtime 已验证，但本次变更还需要真实 App 视觉/交互证据。

## 约束

- 不显示“锁屏已绕过”或暗示系统安全机制被关闭。
- 不把 `agent-runtime-no-gui` 显示为“完整桌面验收”。
- GUI 验收一旦开始，应自动防止空闲休眠；用户主动锁屏后保留当前进度并切换为 `gui_pending_unlock`。
