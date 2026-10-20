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
- M130（2026-10-20 用户批准）采用两级权限：本地写入（含 commit、分支、stash）随会话审批模式；远端 push、PR/MR、合并、标签、发布由会话级 `delivery_authorized` 决定。文字只能降低权限（明确只读与撤销），不能开启交付。结构化只读声明同样优先；条件句和局部文件限制只约束对应动作，不撤销整轮写入。
- 未授权远端动作进入四态 `waiting_user`，显示明确问题和允许按钮；GUI 与 `--dispatch resolve_approval` 使用同一持久审批入口。允许后开启会话开关并继续原动作；拒绝后仅在本地完成，不要求用户输入口令。会话权限菜单可显式开关交付，dispatch 的结构化布尔值可以设置授权。
- 历史的“文字只读永远不影响权限”和“文字授权升级 Deliver”断言已被 M130 R1/R6 取代；普通咨询与模糊姿态仍不降低写权限。
- 任何代码路径都不得从 `AgentMode` 反推 `TurnCapability`。二者是正交字段，`AgentMode::Interactive` 表示「先讨论」，不表示「禁止写入」。
- 写入的安全网是**逐动作权限审批**(`decide_permission` 对 `write_file`/`edit_file` 在非 trusted 模式返回 `Ask`)和用户可见的 safe/trusted 模式，**不是**意图分类器。
- 结构性拒绝必须留出路(见 `docs/principles/release-cadence.md` 同源原则「拒绝必须留出路」)：拒绝文案不得声称用户表达过他没表达的约束，也不得同时禁止写入和禁止向用户追问。
- **为什么(2026-09-28 更新)**：2026-08-05 一天内两次翻车——用户描述缺陷并提出改法，被兜底判成只读，写入被拒且拒绝文案禁止追问，agent 只能编造一个不存在的「implementation 模式」让用户去切。此后(2026-09-24)又发生一次同类事故：一条交付续接指令的末尾针对假设情形写了「如果 CI 再遇到环境抖动，不要改代码或验收脚本」，整轮写权限被撤销，agent 只做只读核查就停了。生产库取证(576 条 user 消息中 28 条被判只读，逐条复核后只有 5 条真是整轮只读，**82% 是执行请求里的局部或条件约束**)；历次十次修复(#37 → #204 → #261 → #265 → 291cc73 → #331 → #287 → #432 → #511)全部落在词表维度，永远补不完；方案 1a 的「措辞存在性 + 动作意图 + 条件限定」两条判据经验证在不新增词表的前提下无法实现(判据 3 的零词表代理 28/28 恒假)。业界四家(Claude Code、Codex CLI、Cursor、Aider)的整轮只读一律是用户显式持有的模式开关或显式结构化规则(`Edit(docs/**)` 的 deny/ask、`writable_roots`、`/read-only`)，**无一从措辞推断写权限**。取证与可行性验证见 `docs/plans/explicit-read-only-inference-phase1-forensics.md` 与 `docs/plans/explicit-read-only-inference-phase2-criteria-feasibility.md`。
- 回归护栏（M130）：明确只读阻止产品写入；无只读约束的普通消息保持写能力；任何文字授权与继续都不能开启远端授权；已授权会话继续交付；撤销关闭开关。历史条件句误降级案例继续保留回归。

## 兼容约束
- 任何 SQLite schema、配置文件、会话导出或权限策略变更都必须提供 Compatibility Harness 证据。
- 各 provider/model 差异由模型适配层处理，UI 不直接依赖某一家模型的私有字段。
- Windows 10/11 + WebView2 版本、macOS(Apple Silicon)+ 系统版本、以及各平台安装包签名状态都必须纳入 release evidence。

## 观测约束
- 本地日志不得包含 API Key、完整敏感文件内容或未脱敏工具参数。
- release-facing 任务至少记录启动状态、错误、延迟、用户可见症状和主路径结果。
- 成本、token、模型 route 和工具调用耗时属于可观测业务字段。
