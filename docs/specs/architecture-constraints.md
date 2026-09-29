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
- 硬只读门禁(`TurnCapability::ReviewOnly`)只有一个来源：**结构化显式来源**——调度子 agent 时显式携带的只读字段(`dispatch::TurnIntent::structured_read_only`)、验收/场景声明里的只读声明。措辞里的任何只读或局部约束(**包括条件句**，例如「如果 CI 再抖动，就别改代码或验收脚本」)**只影响姿态和给模型的指令**，永远不决定写权限：它保留写能力、把姿态压成「先讨论」，并把这条约束转成一条回合级指令块注入模型上下文。意图不明、无法分类、纯提问、单纯寒暄同样**保留写权限**。
- 任何代码路径都不得从 `AgentMode` 反推 `TurnCapability`。二者是正交字段，`AgentMode::Interactive` 表示「先讨论」，不表示「禁止写入」。
- 写入的安全网是**逐动作权限审批**(`decide_permission` 对 `write_file`/`edit_file` 在非 trusted 模式返回 `Ask`)和用户可见的 safe/trusted 模式，**不是**意图分类器。
- 结构性拒绝必须留出路(见 `docs/principles/release-cadence.md` 同源原则「拒绝必须留出路」)：拒绝文案不得声称用户表达过他没表达的约束，也不得同时禁止写入和禁止向用户追问。
- **为什么(2026-09-28 更新)**：2026-08-05 一天内两次翻车——用户描述缺陷并提出改法，被兜底判成只读，写入被拒且拒绝文案禁止追问，agent 只能编造一个不存在的「implementation 模式」让用户去切。此后(2026-09-24)又发生一次同类事故：一条交付续接指令的末尾针对假设情形写了「如果 CI 再遇到环境抖动，不要改代码或验收脚本」，整轮写权限被撤销，agent 只做只读核查就停了。生产库取证(576 条 user 消息中 28 条被判只读，逐条复核后只有 5 条真是整轮只读，**82% 是执行请求里的局部或条件约束**)；历次十次修复(#37 → #204 → #261 → #265 → 291cc73 → #331 → #287 → #432 → #511)全部落在词表维度，永远补不完；方案 1a 的「措辞存在性 + 动作意图 + 条件限定」两条判据经验证在不新增词表的前提下无法实现(判据 3 的零词表代理 28/28 恒假)。业界四家(Claude Code、Codex CLI、Cursor、Aider)的整轮只读一律是用户显式持有的模式开关或显式结构化规则(`Edit(docs/**)` 的 deny/ask、`writable_roots`、`/read-only`)，**无一从措辞推断写权限**。取证与可行性验证见 `docs/plans/explicit-read-only-inference-phase1-forensics.md` 与 `docs/plans/explicit-read-only-inference-phase2-criteria-feasibility.md`。
- 回归护栏：`src-tauri/src/agent/dispatch.rs` 的 `the_hard_read_only_gate_comes_only_from_an_explicit_user_constraint`(措辞层面的只读句必须**保留写能力**、只得到先讨论姿态与一条注入指令；`ReviewOnly` 只由结构化意图触发)与 `the_field_report_message_can_now_reach_the_edit_it_was_denied`(从未要求只读的用户不得被结构性拒绝写入)。改动本约束前必须先让这两条测试失败并说明理由。

## 兼容约束
- 任何 SQLite schema、配置文件、会话导出或权限策略变更都必须提供 Compatibility Harness 证据。
- 各 provider/model 差异由模型适配层处理，UI 不直接依赖某一家模型的私有字段。
- Windows 10/11 + WebView2 版本、macOS(Apple Silicon)+ 系统版本、以及各平台安装包签名状态都必须纳入 release evidence。

## 观测约束
- 本地日志不得包含 API Key、完整敏感文件内容或未脱敏工具参数。
- release-facing 任务至少记录启动状态、错误、延迟、用户可见症状和主路径结果。
- 成本、token、模型 route 和工具调用耗时属于可观测业务字段。
