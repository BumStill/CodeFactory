# CodeFactory 锁屏无关 Agent 验收：业务设计

## 问题

CodeFactory 的长任务验证可能跨越几十分钟。macOS 锁屏后会禁止辅助功能读取、截图和键鼠注入，导致真实 App 的可视化验收中断。模型调用、命令执行、会话数据库和完成门禁本身并不依赖屏幕，不应被这一平台限制整体阻断。

## 目标

- 提供不依赖桌面可见性的 CodeFactory Agent Runtime 验收入口。
- 使用产品当前 endpoint、active model 和 OS credential，不维护第二套模型配置。
- 记录真实模型请求、真实工作目录命令、完成证据和最终结果，且不泄露密钥。
- GUI 验证启动后主动防止空闲休眠，减少验证过程中自动锁屏。
- 明确证据等级，不把无界面运行声明为布局、滚动或点击验证。

## 非目标

- 不绕过 macOS 登录、锁屏或系统安全策略。
- 不用 benchmark fixture、任务 ID 或评分规则定制产品 Agent。
- 不用 mock provider 代替真实 provider 验收。
- 不取消真实 App 的视觉和交互验收。
- v1 不承诺 Windows/Linux 的无界面命令执行；没有等价 OS 工作区隔离时必须明确阻塞。

## 产品价值

1. 长任务的核心能力验证可在用户离开电脑后继续，减少等待解锁造成的迭代停顿。
2. Agent 的模型路由、工具闭环和完成证据可用统一结构化结果复查。
3. benchmark、日常代码修复和发布 canary 都可以消费同一通用 Runtime 入口，但各自保留独立评分和验收规则。

## 成功标准

- 在 `CGSSessionScreenIsLocked=Yes` 时，已授权的 Runtime 验收仍能完成真实 provider 和真实命令闭环。
- 输出中包含 `proof_tier=agent-runtime-no-gui`、provider、model、工具轨迹、完成证据和锁屏状态；不包含 API key。
- GUI 行为变更仍必须在解锁后的真实 App 验证。
