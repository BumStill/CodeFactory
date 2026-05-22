# CodeFactory MVP AI 编程 Agent 主路径规格

## 范围
- 本规格定义 MVP 需要验证的 AI 编程 Agent 主路径。
- 代码脚手架尚未创建前，本规格作为后续实现和验收的源头。

## Requirements Traceability
| Req ID | User request | Normalized requirement | Surfaces | Validation method | Owner |
| --- | --- | --- | --- | --- | --- |
| CF-MVP-R1 | 能像 Claude Code 一样在本地项目里工作 | 会话绑定 cwd，并允许模型读取、搜索、理解项目上下文 | desktop-ui + tauri-backend + tool-runtime | 读取真实测试项目，断言 cwd、文件列表和上下文摘要字段 | planning |
| CF-MVP-R2 | 模型层接 OpenRouter，可切换模型 | 支持模型列表、模型选择、chat completions、SSE 流式输出和 usage 统计 | desktop-ui + tauri-backend + openrouter-api | 代表性响应样本断言 model route、delta、tool_calls、usage/cost | development |
| CF-MVP-R3 | 本地 Agent 能读写文件和跑命令 | 工具系统支持读、写、编辑、搜索、列目录和受控命令执行 | tool-runtime + desktop-ui | 工具 route selection、权限决策、输出字段和失败路径测试 | development |
| CF-MVP-R4 | 所有破坏性操作可控可审计 | 写文件、编辑、命令执行默认 ask，危险命令 deny，工具调用入库 | desktop-ui + tauri-backend + sqlite-store | 权限弹窗截图、deny/ask/allow 字段断言、审计记录断言 | qa |
| CF-MVP-R6 | 用户需要在可信项目里减少权限确认打断 | 提供 Full access mode，开启后绕过配置型 ask 提示，但仍强制执行 hard deny、危险命令 deny 和 cwd 边界，并在 UI 中清楚标识风险 | desktop-ui + tauri-backend + tool-runtime | 权限策略单测、cwd 越界失败测试、设置 UI 截图、权限事件归并测试 | qa |
| CF-MVP-R5 | Windows 原生发布可验证 | 可构建 Windows 安装包并安装后跑通主路径 smoke | release-artifact + desktop-ui | 安装包版本、签名状态、启动、build metadata、主路径 live verification | release |

## Primary User Path
P1: 用户打开 CodeFactory，选择项目工作目录和 OpenRouter 模型，输入一个编程任务。系统通过 OpenRouter 获取模型响应，模型请求读取项目文件并提出编辑或命令。工具调用以卡片形式显示参数和执行结果；ask 级操作弹出审批，用户允许后系统执行工具、展示输出和状态，并把消息和工具调用写入 SQLite。用户可在可信项目中开启 Full access mode 以减少配置型权限提示，但工具仍只能在当前 cwd 边界内运行，且 hard deny 和危险命令 deny 始终生效。

## Applicable Harnesses
- Spec Harness: 本规格、追踪表和测试矩阵必须存在。
- Compatibility Harness: 会话数据库、权限配置、模型响应格式和项目记忆变更必须验证旧数据。
- Release Harness: 安装包、签名、更新通道和启动 smoke 属于发布门禁。
- Observation Harness: 记录 latency、errors、token/cost、工具耗时和用户可见失败。
- Payload Harness: 文件内容、命令输出、SSE chunk、tool_calls arguments 和导出文件都属于 payload。
- Viewport Harness: 主聊天、权限弹窗、diff、终端和模型选择器必须覆盖关键视口。
- AI Collaboration Harness: AI 生成代码或治理变更必须记录最小 AI Collaboration。

## Testing Matrix
| Path type | Scenario | Expected result | Evidence |
| --- | --- | --- | --- |
| Primary path | 真实测试项目中发起“读取并解释项目结构” | 选择正确 cwd，读取 route 为 `read_file/list_dir/grep`，展示摘要并入库 | 工具调用记录 + SQLite 字段断言 |
| Primary path | 模型提出工具调用，用户允许 | 工具卡片可见，参数、权限状态、结果或错误可展开查看 | UI 截图 + 事件归并测试 + 命令输出 |
| Failure path | 用户拒绝写文件 | 工具结果返回拒绝，模型收到拒绝观察，不改文件 | 权限字段断言 + 文件未变 |
| Failure path | 用户开启 Full access mode 后再次发起 ask 工具 | 配置型 ask 提示被绕过，工具仍有卡片和审计状态 | 权限策略单测 + 设置 UI 截图 |
| Failure path | Full access mode 下工具请求 cwd 外路径或命中危险命令 deny | 工具返回错误，不读取、不搜索、不写入 cwd 外文件，危险命令不执行 | Rust 工具测试 + 权限策略单测 |
| Compatibility path | 旧会话数据库启动 | 自动迁移或明确阻塞，不丢历史消息 | migration 测试 |
| Release path | 安装包启动后执行 P1 smoke | 版本可见，主窗口可用，主路径通过或记录 blocker | 安装证据 + live verification |

## Evidence Pack Requirements
- 主路径截图或录屏。
- OpenRouter 代表性样本或测试替身，包含 model、delta、tool_calls、usage/cost 字段断言。
- 工具 route selection 和权限决策记录。
- Full access mode 的开启状态、风险提示、ask bypass 行为和 hard-deny/cwd 边界证据。
- SQLite session/message/tool_call 字段断言。
- Windows 安装和启动证据。
- AI Collaboration：context scope、assumptions、review point、validation result。

## 当前实现状态
- 基础聊天、模型列表、工具定义、工具卡片、权限确认事件、Full access mode 设置已进入本地代码。
- 尚未完成真实桌面主路径、安装包发布和 live verification，不能宣称 MVP 已发布可用。
