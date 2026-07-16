# CodeFactory MVP AI 编程 Agent 主路径规格

## 范围
- 本规格定义 MVP 需要验证的 AI 编程 Agent 主路径。
- 代码脚手架尚未创建前，本规格作为后续实现和验收的源头。

## Requirements Traceability
| Req ID | User request | Normalized requirement | Surfaces | Validation method | Owner |
| --- | --- | --- | --- | --- | --- |
| CF-MVP-R1 | 能像 Claude Code 一样在本地项目里工作 | 会话绑定 cwd，并允许模型读取、搜索、理解项目上下文 | desktop-ui + tauri-backend + tool-runtime | 读取真实测试项目，断言 cwd、文件列表和上下文摘要字段 | planning |
| CF-MVP-R1A | 首轮工具调用不应猜错项目目录 | system prompt 在首轮执行前注入会话绑定的真实项目根目录，并要求优先使用该目录或相对路径，不得假设 `/workspace` 等容器路径 | tauri-backend + tool-runtime | system prompt 单测 + 真实 App 首个工具调用的 cwd/命令断言 | development |
| CF-MVP-R2 | 模型层接 OpenRouter，可切换模型 | 支持模型列表、模型选择、chat completions、SSE 流式输出和 usage 统计 | desktop-ui + tauri-backend + openrouter-api | 代表性响应样本断言 model route、delta、tool_calls、usage/cost | development |
| CF-MVP-R2A | 新会话首屏与实际请求必须使用同一模型 | 创建项目或快速任务会话时由后端按当前 endpoint 校正请求模型；后端返回的权威模型立即回写 UI，不等待首次发送时再自愈 | desktop-ui + tauri-backend + sqlite-store | 后端模型解析单测 + 前端 store 单测 + 新会话发送前 UI/SQLite 字段断言 | development |
| CF-MVP-R2B | ChatGPT 订阅模型能力应跟随官方更新 | 登录 ChatGPT 后从官方 Codex 模型目录读取可见模型、上下文、默认思考强度和支持档位；离线时使用版本化内置快照。前端只展示当前模型支持的档位，请求前再次校验并对旧会话中的失效档位安全回退 | desktop-ui + tauri-backend + chatgpt-codex-api | 目录解析测试 + 旧配置兼容测试 + 模型级档位 UI 测试 + 请求体断言 + 真实订阅目录抽样 | development + qa |
| CF-MVP-R3 | 本地 Agent 能读写文件和跑命令 | 工具系统支持读、写、编辑、搜索、列目录和受控命令执行 | tool-runtime + desktop-ui | 工具 route selection、权限决策、输出字段和失败路径测试 | development |
| CF-MVP-R4 | 所有破坏性操作可控可审计 | 写文件、编辑、命令执行默认 ask，危险命令 deny；高风险 shell 命令在 Full access mode 下仍必须 ask；工具调用入库 | desktop-ui + tauri-backend + sqlite-store | 权限弹窗截图、deny/ask/allow 字段断言、shell 审计字段断言、审计记录断言 | qa |
| CF-MVP-R6 | 用户需要在可信项目里减少权限确认打断 | 提供 Full access mode，开启后绕过配置型 ask 提示，但仍强制执行 hard deny、危险命令 deny 和 cwd 边界，并在 UI 中清楚标识风险 | desktop-ui + tauri-backend + tool-runtime | 权限策略单测、cwd 越界失败测试、设置 UI 截图、权限事件归并测试 | qa |
| CF-MVP-R7 | 长任务执行中仍需调整下一步 | 自主任务运行期间，主消息输入框切换为唯一的“引导下一步”入口；引导只进入调度器的下一任务，不创建并发聊天回合，并明确显示成功或失败，失败时保留草稿 | desktop-ui + tauri-backend + task-scheduler | 主输入框交互测试 + `queue_interjection` 参数断言 + 真实 App 成功/失败路径 | development |
| CF-MVP-R8 | 停止当前回答后，排队消息不能丢失或抢跑 | 取消聊天是协作式中断；前端在后端终止事件到达前保持 stream listener 和运行态，只在终止事件后按顺序发送下一条排队消息 | desktop-ui + tauri-backend + sqlite-store | 取消/终止事件时序测试 + 排队消息调用顺序断言 + 真实 App 边界路径 | development |
| CF-MVP-R5 | Windows 原生发布可验证 | 可构建 Windows 安装包并安装后跑通主路径 smoke | release-artifact + desktop-ui | 安装包版本、签名状态、启动、build metadata、主路径 live verification | release |

## Primary User Path
P1: 用户打开 CodeFactory，选择项目工作目录和 OpenRouter 模型，输入一个编程任务。系统通过 OpenRouter 获取模型响应，模型请求读取项目文件并提出编辑或命令。工具调用以卡片形式显示参数和执行结果；ask 级操作弹出审批，用户允许后系统执行工具、展示输出和状态，并把消息和工具调用写入 SQLite。用户可在可信项目中开启 Full access mode 以减少配置型权限提示，但工具仍只能在当前 cwd 边界内运行，且 hard deny 和危险命令 deny 始终生效。

### 首轮执行上下文不变量
- 项目会话和两种快速任务会话创建时，后端必须以当前 `default_endpoint` 为权威，保留兼容的用户选定模型；如果前端传入的是其他 provider 的过期模型，则在写入 SQLite 前回退到该 endpoint 的 active model。该规则双向适用于 direct provider、ChatGPT 和 OpenRouter 之间的切换。
- 前端必须采用 `create_session` 返回的 `session.model_id` 更新模型选择器，不能继续展示创建前的过期 store 值。
- Agent 每轮 system prompt 都必须包含会话绑定的真实 cwd。工具仍以自身 cwd 边界作为最终安全约束；prompt 注入只减少模型误猜路径，不扩大文件或命令权限。
- 首轮发送不得依赖“请求失败后再修复模型”或“命令失败后再猜目录”作为正常路径；现有发送时模型修复仅保留为旧会话兼容兜底。
- ChatGPT/Codex 模型列表和思考档位不得仅依赖前端固定数组。在线目录是权威来源；内置快照只负责离线可用。会话保存的档位若不被当前模型支持，请求必须回退到该模型目录声明的默认档位，其次才回退到 `medium` 或首个支持档位。
- 自主任务运行时不得同时展示两个“引导下一步”输入框。主消息输入框负责提交调度器 interjection；只有后端确认入队后才清空草稿，失败必须保留原文并显示错误。
- 聊天取消不得在 `cancel_chat` 调用返回时假定当前 Agent 回合已经结束。只有该会话的 stream terminal event 才能关闭监听并触发下一条排队消息，避免同一会话出现并发 Agent loop。

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
| Primary path | 当前 endpoint 为 DeepSeek，但前端 store 残留其他 provider 模型后创建项目 | 会话在首次发送前即绑定当前 endpoint 的有效模型，选择器和 SQLite 一致 | Rust/TypeScript 单测 + 真实 App 发送前截图/字段断言 |
| Primary path | ChatGPT 登录后打开模型与思考强度选择器 | 模型列表来自官方目录；GPT-5.6 Sol 显示其目录声明的完整档位，GPT-5.5 不显示 `max/ultra` | 目录响应字段断言 + UI 选项测试 + 真实 App 截图 |
| Primary path | 对带失败测试的真实项目发起修复任务 | 首个 shell/file 工具使用会话真实 cwd 或相对路径，不先尝试不存在的 `/workspace` | system prompt 单测 + 工具调用命令/cwd 证据 |
| Primary path | 模型提出工具调用，用户允许 | 工具卡片可见，参数、权限状态、结果或错误可展开查看 | UI 截图 + 事件归并测试 + 命令输出 |
| Primary path | 自主任务运行中从主输入框提交下一步引导 | 页面只有一个引导输入入口；调用 `queue_interjection` 后显示“已加入下一任务”并清空草稿 | 组件交互测试 + Tauri 调用断言 + 真实 App 截图 |
| Failure path | 用户拒绝写文件 | 工具结果返回拒绝，模型收到拒绝观察，不改文件 | 权限字段断言 + 文件未变 |
| Failure path | 调度器拒绝或无法保存下一步引导 | 不创建普通聊天消息，不清空输入草稿，错误状态可见且可重试 | 组件失败测试 + 真实 App 失败路径 |
| Failure path | 当前回答仍在工具调用时，用户取消并已有排队消息 | `cancel_chat` 后不立即发送排队消息；收到当前回合终止事件后才发送一条，两个 Agent 回合不重叠 | store 时序测试 + IPC 调用顺序断言 |
| Failure path | 用户开启 Full access mode 后再次发起 ask 工具 | 配置型 ask 提示被绕过，工具仍有卡片和审计状态 | 权限策略单测 + 设置 UI 截图 |
| Failure path | Full access mode 下工具请求 cwd 外路径或命中危险命令 deny | 工具返回错误，不读取、不搜索、不写入 cwd 外文件，危险命令不执行 | Rust 工具测试 + 权限策略单测 |
| Failure path | Full access mode 下模型提出高风险 shell 命令 | 系统仍进入 ask 路径；用户未允许前不执行命令 | 权限策略单测 + 权限弹窗证据 |
| Observation path | bash 命令执行完成 | 工具结果包含 cwd、exit_code、risk 等最小审计字段 | Rust 工具测试 + SQLite/tool result 抽样 |
| Compatibility path | 旧会话数据库启动 | 自动迁移或明确阻塞，不丢历史消息 | migration 测试 |
| Compatibility path | 旧会话保存了与当前 endpoint 不兼容的模型 | 发送时继续自动校正并更新会话；新会话不再产生该状态 | 模型 route 单测 + 旧会话回归 |
| Compatibility path | 旧设置缺少模型能力元数据，或旧会话保存了当前模型不支持的思考档位 | 设置正常加载；UI 使用兼容档位，实际请求回退到当前模型默认值，不产生 provider 400 | serde/TypeScript 兼容测试 + 请求体断言 |
| Release path | 安装包启动后执行 P1 smoke | 版本可见，主窗口可用，主路径通过或记录 blocker | 安装证据 + live verification |

## Evidence Pack Requirements
- 主路径截图或录屏。
- OpenRouter 代表性样本或测试替身，包含 model、delta、tool_calls、usage/cost 字段断言。
- ChatGPT/Codex 模型目录的 `slug/context_window/default_reasoning_level/supported_reasoning_levels` 字段断言，以及目录不可用时的内置快照证据。
- 工具 route selection 和权限决策记录。
- 自主任务引导的唯一入口、成功反馈、失败保留草稿，以及取消后排队消息时序证据。
- Full access mode 的开启状态、风险提示、ask bypass 行为和 hard-deny/cwd 边界证据。
- SQLite session/message/tool_call 字段断言。
- Windows 安装和启动证据。
- AI Collaboration：context scope、assumptions、review point、validation result。

## 当前实现状态
- 基础聊天、模型列表、工具定义、工具卡片、权限确认事件、Full access mode 设置已进入本地代码。
- 尚未完成真实桌面主路径、安装包发布和 live verification，不能宣称 MVP 已发布可用。
