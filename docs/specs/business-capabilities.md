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
- Full access mode 表示可信项目内的低打扰执行模式，可绕过配置型 ask 提示，但不得绕过 hard deny、危险命令 deny 或用户选择的 cwd 边界。

## C4 会话与审计
- 持久化 session、message、tool_call、成本和耗时。
- 每个工具调用可以追溯输入、权限决策、输出、错误和持续时间。
- 支持会话导出，敏感字段必须脱敏。

## C5 命令执行与终端
- 提供受控 PowerShell/cmd/Git Bash 执行。
- 提供内嵌终端 UI。
- 命令白名单、黑名单、hard deny 和 cwd 限制必须可审计；Full access mode 不得关闭这些硬边界。
- 高风险命令（例如递归删除、`git reset --hard`、注册表删除、管道执行脚本）在 Full access mode 下仍必须进入 ask 或 deny 路径。
- `bash` 工具结果必须包含最小审计元数据：实际 cwd、退出码和风险等级。

## C6 发布与更新
- 构建 Windows 安装包。
- 验证安装、启动、版本信息、签名状态和回滚路径。
- 未来支持自动更新时必须纳入 Release Harness。

## C7 治理与证据
- 每个非平凡特性必须有规格、Req ID、主路径、测试矩阵和证据包。
- release-facing 任务必须有 live verification 或明确 blocker。

## C8 Terminal-Bench 能力评估
- 使用 Terminal-Bench 2.1 作为 terminal agent 能力外部标尺。
- 遵循 `docs/principles/systematic-agent-evaluation.md`：评估主体默认是 CodeFactory agent，模型只是 backend/component attribution。
- 通过 Harbor 运行或导入 benchmark job，保存 run、trial、reward、trajectory、verifier output 和 artifact evidence。
- 生成按任务类别、难度、工具路径、失败类型、耗时和成本的能力画像。
- 支持同一 subset 的 baseline/head 回归对比。
- Benchmark sandbox policy 只在隔离任务容器内生效，不得污染普通项目权限、长期 memory、默认 prompt 或技能示例。
