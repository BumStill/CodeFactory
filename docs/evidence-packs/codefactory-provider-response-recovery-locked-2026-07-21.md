# CodeFactory Provider 响应恢复与用量证据

- Req ID: `CF-TB-R46`
- 日期: `2026-07-21`
- 状态: candidate verified, not live
- Proof tier: `agent-runtime-no-gui`
- 基线版本: public `v1.51.4` / `87409c1`
- candidate headless SHA-256: `913859dd57d99497b65320d2883502ea908b4f31196aab58f302fc4c26e5b997`

## 真实问题

`v1.51.4` 的 released-build 聚焦 canary 在 11 个工具调用后连续 3 次出现
`error decoding response body`，sidecar 在 verifier 运行前退出。此前成功的模型调用只在
`finished` 消息中结算，因此该失败又把已有 request/token 用量错误报告为 `0`。

## 通用产品修复

- OpenAI-compatible、OpenRouter、ChatGPT 及 Anthropic 主 Agent 请求统一通过共享 HTTP
  helper 协商 `Accept-Encoding: identity`。
- headless 在尚无工具进度时维持 3 次总尝试；已有工具进度后自适应扩大到受 wall deadline
  约束的 5 次总尝试，避免首轮不可用端点拖长等待。
- 每个 `tool_request` 携带累计 usage；成功但没有工具调用且 completion gate 仍未满足时，
  发送独立 `usage_snapshot` event。
- Harbor bridge 与 Runtime acceptance 在后续 provider fatal error 时读取最新 snapshot，写入
  失败 metadata/evidence，不再回落为伪 `0`。
- 实现不包含 Terminal-Bench task 名称、仓库指纹、预期答案、隐藏 verifier 或 task-specific
  repair script。

## Failure-first

- headless 在连续 4 个截断响应后固定第 3 次退出，无法接收第 5 个有效响应。
- shared HTTP helper 的捕获请求不包含 `Accept-Encoding: identity`。
- bridge 在 `tool_request` 后收到更新的 `usage_snapshot` event，再遇 fatal error 时仍只保存旧
  tool snapshot。
- Runtime acceptance 在 snapshot 后 sidecar 异常退出时没有生成 `result.json`。

以上失败均在实现前独立复现。

## 锁屏真实 Runtime 验收

使用当前源码构建的真实 `codefactory-agent-headless`，连接本地 OpenAI-compatible fixture。
fixture 首先要求产品执行一次 workspace mutation；任务已有进度后，对下一次模型调用连续注入
4 个声明长度大于实际内容的截断 HTTP response body，第 5 次返回有效工具调用，随后要求独立
shell equality assertion 并正常完成。

- status: `passed`
- screen locked at start/end: `true / true`
- provider HTTP attempts: `7`
- injected truncated bodies: `4`
- all requests requested identity encoding: `true`
- tool calls: `2`
- mutation sequence: `1`
- machine-checked verification sequence: `2`
- successful model responses: `3`
- usage: prompt `48`, completion `12`, total `60`
- blocker: none
- raw evidence: `.codefactory/product-acceptance/provider-response-recovery-r46-v1`

这证明锁屏不会阻塞 sidecar、工具执行、重试、usage 持久化或 completion gate。它不是 GUI
验收；安装包真实 App 仍由 PR 的 remote macOS GUI 与 release artifact smoke 提供。

## 回归结果

- agent core: `92 passed`
- headless: `18 passed`
- desktop Rust: `376 passed / 6 ignored`
- Harbor bridge: `17 passed / 2 Linux-only skipped`
- Runtime acceptance: `7 passed`
- regression/release Python: `43 passed`
- frontend Vitest: passed
- frontend production build: passed
- governance baseline: passed
- Rust targeted formatting and `git diff --check`: passed

## 独立审查

独立只读审查发现并阻止了两个不完整实现进入发布：fatal acceptance 未写证据，以及只在
tool request 保存 usage 会漏掉成功的无工具响应。两项均已补齐并有 failure-first 测试。
审查同时指出“5 次重试”与“5 次总尝试”的歧义，规格已明确为含首发 5 次总尝试；Anthropic
直连路径也已接入共享 helper。

## 结论边界

R46 candidate 已通过本地产品路径验证，但当前尚未合并、尚未发布，因此是 `not live`。
发布后必须用同一 released build 重跑相同 `circuit-fibsqrt` canary；该验收不能替代
Terminal-Bench reward，也不能提高当前有效固定 18 题 `6 / 18` 基线。
