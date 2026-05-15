# CodeFactory 稳定业务能力目录

## C1 模型与对话
- 选择 OpenRouter 模型。
- 发起普通聊天和流式回复。
- 展示 token、成本和模型 route。

## C2 本地项目上下文
- 绑定当前会话 cwd。
- 读取项目文件、搜索文件和引用上下文。
- 注入项目记忆文件 `CODEFACTORY.md`。

## C3 受控工具系统
- 注册 `read_file`、`write_file`、`edit_file`、`glob`、`grep`、`list_dir`、`bash` 等工具。
- 每个工具声明风险等级、JSON Schema、权限策略和输出限制。
- 文件写入和命令执行必须展示 diff 或参数摘要并等待用户确认。
- Full access mode 可在可信项目中绕过配置型权限提示，但必须在 UI 中清楚展示风险，并保留工具卡片和审计状态。

## C4 会话与审计
- 持久化 session、message、tool_call、成本和耗时。
- 每个工具调用可以追溯输入、权限决策、输出、错误和持续时间。
- 支持会话导出，敏感字段必须脱敏。

## C5 命令执行与终端
- 提供受控 PowerShell/cmd/Git Bash 执行。
- 提供内嵌终端 UI。
- 命令白名单、黑名单和 cwd 限制必须可审计。

## C6 发布与更新
- 构建 Windows 安装包。
- 验证安装、启动、版本信息、签名状态和回滚路径。
- 未来支持自动更新时必须纳入 Release Harness。

## C7 治理与证据
- 每个非平凡特性必须有规格、Req ID、主路径、测试矩阵和证据包。
- release-facing 任务必须有 live verification 或明确 blocker。
