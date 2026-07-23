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

## C9 任务失败归因与修复闭环
- Workspace 任务系统必须把 failed/cancelled task 归因为 provider/credential、permission、shell runtime、test failure、verification failure、cancelled 或 unknown。
- 归因来自通用任务字段，不读取 benchmark 名称或 Terminal-Bench 专用路径。
- 用户在任务列中能看到失败类型、证据来源和下一步建议，再决定是否点击 `修复失败项` 进入重试。
- `修复失败项` 只允许自动重置 `repairable=true` 的任务；provider/key、权限、运行环境等需要用户先处理原因的失败必须保持 failed，避免盲目消耗模型调用。
- 自动修复闭环必须回到同一 project session 的任务执行、验证结果和 evidence pack，而不是只更新 benchmark 分数。

## C10 有界控制面观察
- ControlPlane 的 Git 子进程必须有硬超时并在超时后终止，不允许页面永久加载。
- Git timeout、Git unavailable、非 Git 目录和普通 probe failure 必须分开归因。
- 单个 Git probe 失败时继续返回 Authority、Memory、Capabilities、filesystem delivery fields 和其他可用 Git 字段。
- 用户能看到 partial risk，并在 Git 恢复后通过刷新回到完整快照。

## C11 Session 驱动的持续改进
- 持久 session 的真实工具生命周期必须进入规范化、脱敏的本地轨迹；anonymous session 不进入 DB、学习或 hooks。
- 普通聊天、Quick Task 和任务调度都要有与各自 route 匹配的 post-mortem 输入，不能用空 `task_runs` 冒充“没有信号”。
- Session 信号先形成待审候选；知识、Skill、工具策略、Evals 和产品代码使用不同人工门禁。
- 首期复用 Tauri + SQLite，不引入独立遥测平台；完整状态机见 `feature-specs/evolution-agent-closed-loop.md`。
- Evals 只能降低已覆盖场景的回归风险，不承诺“只进不退”，也不能自动放行发布。

## C12 会话控制收敛
- Full access 只改变工具权限决策，不改变用户这轮是问答、诊断、规划还是执行；没有明确修改授权的诊断请求不得自动扩展为代码交付。
- Interactive/Execute 前台回合的 completion recovery 使用不可重置的累计上限；证据进展不能重新获得完整恢复预算。
- 内部 recovery prompt、模型草稿和被拒绝候选答复不进入聊天正文，但恢复阶段、次数、安全步骤、最近活动与停止边界必须对用户可见。
- 取消后续生成不等于回滚；已经执行、提交、推送或产生外部副作用的动作必须明确保留并可审计。
