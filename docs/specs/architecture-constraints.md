# CodeFactory 架构约束

## 系统边界
- CodeFactory 是本地优先、跨平台(Windows + macOS)的本地 AI 编程 Agent 桌面客户端。
- 桌面壳使用 Tauri 2，前端使用 React + TypeScript + Vite，后端使用 Rust。
- 模型接入面向任意 OpenAI 兼容 Chat Completions/SSE 端点(OpenRouter、DeepSeek、Anthropic、OpenAI、本地 LMStudio/Ollama 等),不绑定单一 provider。
- 会话、消息、工具调用和成本统计使用 SQLite 本地存储。
- API Key 使用 OS 原生凭据存储(Windows Credential Manager / macOS Keychain),不落明文文件。

## 安全约束
- 文件写入、编辑、命令执行和高风险网络请求必须经过权限策略。
- 权限策略按 deny -> allow -> ask 判定，危险命令永久 deny。
- 工具执行默认限制在用户选择的 cwd 内，越界必须有明确配置和用户确认。
- 工具输出必须截断并审计，避免 token 爆炸和敏感信息泄漏。

## 意图推断约束：姿态可以猜，权限不许猜
- 每个会话回合有两个独立决定，**不得由同一个判断承担**：
  - **姿态**(`ChatContract.mode`)：这轮先讨论还是直接干。判错很便宜，模型下一句就能自纠，所以**允许**由框架从措辞推断。
  - **权限**(`ChatContract.capability`)：这轮能不能写文件。判错不可恢复，所以**只能来自用户的显式表达**。
- 硬只读门禁(`TurnCapability::ReviewOnly`)只有一个来源：用户当前消息明确约束了改动(「不要改代码」「只分析」)。意图不明、无法分类、纯提问、单纯寒暄都**保留写权限**，只把姿态压成「先讨论」。
- 任何代码路径都不得从 `AgentMode` 反推 `TurnCapability`。二者是正交字段，`AgentMode::Interactive` 表示「先讨论」，不表示「禁止写入」。
- 写入的安全网是**逐动作权限审批**(`decide_permission` 对 `write_file`/`edit_file` 在非 trusted 模式返回 `Ask`)和用户可见的 safe/trusted 模式，**不是**意图分类器。
- 结构性拒绝必须留出路(见 `docs/principles/release-cadence.md` 同源原则「拒绝必须留出路」)：拒绝文案不得声称用户表达过他没表达的约束，也不得同时禁止写入和禁止向用户追问。
- **为什么**：2026-08-05 一天内两次翻车——用户描述缺陷并提出改法，被兜底判成只读，写入被拒且拒绝文案禁止追问，agent 只能编造一个不存在的「implementation 模式」让用户去切。此前五次修复(#37 → #204 → #261 → #265 → 291cc73)都是往关键词表里加执行动词，永远补不完，因为缺的信号不是动词，而是「用一个约 53% 准确率的猜测承担了不可恢复的决策」。业界(Claude Code plan mode、Cursor 模式选择)一律由用户显式持有该开关，无人从措辞推断写权限。
- 回归护栏：`src-tauri/src/agent/dispatch.rs` 的 `the_hard_read_only_gate_comes_only_from_an_explicit_user_constraint` 与 `the_field_report_message_can_now_reach_the_edit_it_was_denied`。改动本约束前必须先让这两条测试失败并说明理由。

## 兼容约束
- 任何 SQLite schema、配置文件、会话导出或权限策略变更都必须提供 Compatibility Harness 证据。
- 各 provider/model 差异由模型适配层处理，UI 不直接依赖某一家模型的私有字段。
- Windows 10/11 + WebView2 版本、macOS(Apple Silicon)+ 系统版本、以及各平台安装包签名状态都必须纳入 release evidence。

## 观测约束
- 本地日志不得包含 API Key、完整敏感文件内容或未脱敏工具参数。
- release-facing 任务至少记录启动状态、错误、延迟、用户可见症状和主路径结果。
- 成本、token、模型 route 和工具调用耗时属于可观测业务字段。
