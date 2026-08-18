# Feature Specs

本目录承载会长期存在的特性规格。一次性计划、聊天记录或 `PROJECT_PLAN.md` 不能替代这里的规格。

每个特性规格至少包含：
- Requirements Traceability
- Primary User Path
- Applicable Harnesses
- 测试矩阵
- Evidence Pack Requirements
- 兼容性和发布边界

## 跨规格权威顺序

- `objective-recovery-control-plane.md` 是 objective ownership、阻断归属、完成语义、自动恢复和用户回交边界的最高产品契约。
- 其它领域规格继续定义各自的分类、协议、安全边界和领域证据；当其旧文案把 provider、permission timeout、task attempt、CI、进程重启等 system-owned 技术状态变成人工“继续/重试/重新发送”动作时，以 CF-ORC 为准。
- 明确用户拒绝/取消、不可替代核心输入、无安全默认的不可逆业务决定，以及 hard deny/不可逆副作用门禁不受自动恢复覆盖；但必须使用 CF-ORC 的 typed decision，而不能从错误字符串或旧 `blocked` 状态推断用户责任。
- `completed` 只能由满足 CF-ORC Completion Predicate 的 `CompletionArbiter` 写入；turn、task、tool、delivery 和 stream 的局部终态只是 projection。

## 当前规格
- `mvp-agent-client.md`: CodeFactory MVP AI 编程 Agent 主路径规格。
- `personal-knowledge-office-assistant.md`: 个人知识库、PowerPoint 插件和通用助手化规格。
- `terminal-bench-21-evaluation.md`: Terminal-Bench 2.1 能力评估、Harbor 接入、失败分类和回归闭环规格。
- `evolution-agent-closed-loop.md`: Session 真实轨迹、信号提取、人工审核、受控改进与 Evals 门禁规格。
- `task-failure-attribution-repair-loop.md`: Workspace 任务失败归因、修复建议和主产品闭环规格。
- `objective-recovery-control-plane.md`: 跨 chat、task、permission、provider、browser、delivery、release 与进程重启的 objective 真相源、自动恢复和必要用户回交规格。
- `durable-delivery-recovery.md`: 同一 objective/repo/change-set/PR 的持久交付、恢复分类和 Completion Arbiter 领域规格。
- `session-control-convergence.md`: turn capability、permission outcome、segment guard 与会话恢复投影规格。
- `model-runtime-control-plane.md`: 会话模型策略、OAuth、CredentialBroker、route replay fence 与安全续接规格。
- `repository-owned-specifications.md`: 仓库归属的长期规范、会话内计划与旧 Specs 产品模块退场合同。
- `chat-continuity-conversational-evidence.md`: 用户目标跨内部执行分段连续完成、异常可恢复终态、自然对话式工具证据与历史密度规格。
- `endpoint-capability-failover.md`: 首选模型服务不可用时，基于本机已配置端点、凭据与本轮能力做有界自动接管，保持上下文和工具连续性并提供可行动失败说明。
- `settings-hooks-remotes-tabs.md`: Settings 中 Hooks 与 Git remotes 管理能力的历史规格。
- `token-cost-dashboard.md`: token 用量与成本可见性的历史规格。
- `agent-workbench-experience.md`: Workspace 现代视觉层级、语义状态、会话阅读、composer、后台作业与真实交付链体验规格。
- `on-demand-embedded-browser-pane.md`: 当前会话按需创建的内置右侧浏览器分屏、控制权、安全隔离与自动回收规格。
- `browser-access-zero-touch-provisioning.md`: 浏览器扩展零手工配对、受管浏览器下载在 Windows 权限受限时的自愈与回退规格。
- `scenario-test-governance.md`: 统一场景注册表、PR 影响声明、证据等级，以及基于匿名历史形状的复杂真实端到端测试治理规格。

## 模板
- `requirements-traceability-template.md`
- `testing-matrix-template.md`
- `payload-harness-template.md`
- `viewport-harness-template.md`
- `evidence-pack-template.md`
