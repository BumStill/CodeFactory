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

## 兼容约束
- 任何 SQLite schema、配置文件、会话导出或权限策略变更都必须提供 Compatibility Harness 证据。
- 各 provider/model 差异由模型适配层处理，UI 不直接依赖某一家模型的私有字段。
- Windows 10/11 + WebView2 版本、macOS(Apple Silicon)+ 系统版本、以及各平台安装包签名状态都必须纳入 release evidence。

## 观测约束
- 本地日志不得包含 API Key、完整敏感文件内容或未脱敏工具参数。
- release-facing 任务至少记录启动状态、错误、延迟、用户可见症状和主路径结果。
- 成本、token、模型 route 和工具调用耗时属于可观测业务字段。
