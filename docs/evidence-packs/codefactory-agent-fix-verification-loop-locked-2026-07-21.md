# CodeFactory 长任务修复验证闭环证据

- Req ID: `CF-TB-R47`
- 日期: `2026-07-21`
- 状态: candidate verified, not live
- Proof tier: `agent-runtime-no-gui`
- 基线版本: public `v1.51.5` / `4432cbc`
- candidate headless SHA-256: `45d1ce3e86aa8f9373856b411ae393ee9710b567e3f108810c63abc3a127e7e0`

## Released-Build 失败证据

`v1.51.5` 的同参数 `circuit-fibsqrt` canary 已消除 R46 的 provider response fatal，
但 reward 仍为 `0` / failure class `verification`。Agent 在 `906.84s` 内完成 `21` 次成功
模型响应、消耗 `200,687` tokens，并执行 `21` 个外部工具调用；它反复改写并打印玩具
候选，直到最终墙钟保留区耗尽。Terminal-Bench verifier 的文件存在和大小检查通过，28 个
功能样例全部失败，输出只有 `0` 或 `4`。最后修改之后没有独立机器断言。

## 通用产品修复

- 共享 `agent-core` 在 Autonomous/Execute 剩余最后 16 轮，或 headless 墙钟只剩三分之二后，
  若最新成功 mutation 尚无后续机器断言，只允许独立 Verification、RuntimeProbe 或 bounded
  FunctionalProbe；新的 edit/read 探索会被拒绝。
- 失败验证会写入真实 failure evidence，并重新允许一次纠正性修改，不形成无法修复的策略死锁。
- source build/install/runtime/test 继续使用既有分阶段门禁，不被本规则抢占。
- completion policy 内部拒绝的动作不再作为 workspace mutation、failure 或成功 outcome 计入
  headless completion evidence。
- 一个成功模型响应的全部工具调用都被内部拒绝时，sidecar 立即发送累计
  `usage_snapshot`，因此紧随其后的 provider fatal 也不会丢掉该响应的用量。
- 成功响应在 tool-call 解析阶段因缺失 command/arguments 等 malformed payload 退出时，也会在
  fatal 前发送 usage snapshot。
- 规则同时由桌面主 Agent 与 headless 调用，不包含 Terminal-Bench task 名称、仓库指纹、
  artifact 名称、预期答案或 verifier 读取。

## Failure-First

实现前的独立测试证明：

- 收敛窗口内，最新成功 mutation 后仍允许第二次 mutation 和 ReadOnly 探索。
- 桌面 Autonomous 主 Agent 的同一路由也允许继续 edit。
- Headless 全拒绝响应不发送 usage event，且被拒绝动作错误增加 `outcome_count` 并改变
  mutation/verification sequence。

实现后对应 shared-core、desktop-route 和 headless-protocol 测试全部转绿。

## 锁屏真实 DeepSeek Runtime

真实 `codefactory-agent-headless` 使用本机 CodeFactory 的 `deepseek / deepseek-v4-pro` 配置，
在普通临时项目 `/private/tmp/codefactory-product-eval-fix-loop-r47` 执行非 Benchmark 任务：修复
`./tool 6`，使其输出严格等于 `42`，并在结束前验证行为。

- status: `passed`
- screen locked at start/end: `true / true`
- duration: `17,053 ms`
- model requests: `5`
- usage: prompt `19,819`, completion `977`, total `20,796`
- external tool calls / outcomes: `4 / 4`
- last mutation sequence: `3`
- independent machine assertion sequence: `4`
- completion blocker: none
- workspace isolation: `macos-sandbox-exec`
- raw evidence: `.codefactory/product-acceptance/fix-loop-r47-v2`

## 锁屏受控策略路径

同一 candidate sidecar 连接本地 OpenAI-compatible fixture。fixture 第一次响应创建修复，第二次
响应要求再次 mutation，第三次响应执行独立 shell equality assertion，第四次正常结束。

- status: `passed`
- screen locked at start/end: `true / true`
- model requests: `4`
- provider usage: `60` tokens
- first mutation: sequence `1`
- second mutation: 被 `fix_verification_loop` 拒绝，没有外部执行
- event after denial: `usage_snapshot`, model requests `2`, total tokens `30`
- independent assertion: sequence `2`
- external tool calls / completion outcomes: `2 / 2`
- completion blocker: none
- raw evidence: `.codefactory/product-acceptance/fix-loop-r47-fixture-v2`

轨迹只包含 mutation request/result、拒绝后的 usage event、assertion request/result。独立终端
再次执行相同 equality assertion，exit code 为 `0`。

## 回归结果

- agent core: `93 passed`
- headless: `20 passed`
- desktop Rust: `377 passed / 6 ignored`
- benchmark report runner: `38 passed`
- targeted Rust formatting: passed

## 结论边界

R47 已通过共享策略、桌面主 Agent 路由、真实 DeepSeek Runtime 和受控 sidecar 策略路径验证，
但当前尚未合并、尚未发布，因此是 `not live`。只有 PR/CI、release、发布产物验证和同题
released-build canary 完成后才能称为产品化。当前有效固定 18 题总分仍为 `6 / 18`。
