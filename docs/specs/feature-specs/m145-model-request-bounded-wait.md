# M145 — 模型请求必须有界等待，等待状态必须可解释

## 事故

2026-10-21，用户配置自托管 DeepSeek 端点开始对话，界面停在
「正在做 · 等待中」，无 thinking 输出、无工具调用、无错误提示，直到手动点击停止。
Windows `v1.91.13` 现场。

根因：模型请求使用 `reqwest::Client::new()`，既无连接超时也无读超时。
端点接受 TCP 连接后不再返回任何字节，`send()` 的 future 永不 resolve，
于是 turn 既不产出事件也不报错；而 UI 把「还没有流」这一状态渲染成
「等待中」，与真正的系统级等待无法区分，用户无法判断是模型慢、网络断、
还是产品卡死。

## 需求

- **R1 请求有界**：模型请求必须存在有限的「连接 + 首字节」预算（默认 60s）、
  「流中途空闲」预算（默认 120s，按每次读取计算）与连接预算（默认 10s）。
  超时必须以可读错误暴露，且可被 M125 失败分类/回落接管；
  UI 在「还没有任何流」阶段必须写明在等什么（`等待模型首个响应`），
  而不是笼统的「等待中」。
- **R2 停止即生效**：用户点停止后下一条消息必须立即进入新 turn，
  不被上一条的残留执行占用。
- **R3 诊断可见**：诊断面板提供「复制诊断」，包含错误原因、耗时、
  provider/model、最后事件与完整日志尾部（含密钥脱敏）。
- **R4 测试**：R1、R2 必须有自动化测试；R3 手工验收。

## 实现（本次）

- `src-tauri/src/http_util.rs`
  - `MODEL_CONNECT_TIMEOUT_MS = 10s`、`MODEL_FIRST_BYTE_TIMEOUT_MS = 60s`、
    `MODEL_STREAM_IDLE_TIMEOUT_MS = 120s`，均可用
    `CODEFACTORY_MODEL_{CONNECT,FIRST_BYTE,STREAM_IDLE}_TIMEOUT_MS` 覆盖。
  - `model_http_client()`：`connect_timeout` + `read_timeout`（每次读取的空闲上限），
    同时约束「响应头永不到达」与「流中途静默」，但不限制长输出的总时长。
  - `send_model_request_bounded_with_policy()`：用
    `tokio::time::timeout` 包住整个「连接 + 发送 + 等响应头」阶段；超时错误
    文本含 `MODEL_FIRST_BYTE_TIMEOUT`、模型名与 `timeout`，
    经 `classify_provider_failure` 归为 `TransportUnreachable`，交给既有回落/重试。
- 所有模型客户端改用 `model_http_client()`：
  `agent/mod.rs`（ChatGPT + OpenAI 兼容）、`agent/internal_text.rs`、`openrouter/client.rs`；
  四个模型 POST 站点改走 `send_model_request_bounded_with_policy`。
- `src/components/MessageList.tsx`：新增 `connecting` 阶段，
  文案 `等待模型首个响应 · mm:ss`；真正的系统等待（`objectiveStatus: waiting_system`）
  仍显示 `等待中`。

## 未完成

- R2、R3 未在本 PR 实现，保留在需求中（R2 需先确认停止后的执行占用语义，
  R3 需要诊断面板的数据面）。
