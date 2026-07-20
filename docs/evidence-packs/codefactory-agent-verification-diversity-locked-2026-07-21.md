# CodeFactory 多输入验证充分性证据

- Req ID: `CF-TB-R48`
- 日期: `2026-07-21`
- 状态: candidate verified, not live
- Proof tier: `agent-runtime-no-gui`
- 基线版本: public `v1.51.6` / `e34c2ec`
- candidate headless SHA-256: `e9f4493bc86579f3d763a9ad6484a3f6768ad84242f1a920f8971557049b37a5`

## Released-Build 失败证据

`v1.51.6` 的同参数 `circuit-fibsqrt` canary reward 仍为 `0`，failure class 为
`verification`。该单题 run 启用了 watchdog，因此明确标记 `official_comparable=no`，不能替代
固定 18 题总分。

R47 已真实生效：运行在 `507.54s` 内完成，使用 `18` 次模型请求、`137,811` tokens 和 `11`
个外部工具调用；最后成功 mutation 为 sequence `10`，随后独立机器断言为 sequence `11`，
completion evidence 正常闭合。缺陷转移到验证充分性：该断言只复用了任务正文明确给出的
`208 -> 377` 和 `20000 -> 1407432322` 两组示例，Agent 随即宣告完成。Terminal-Bench verifier
的文件存在/大小检查通过，但 28 个非示例功能样例全部失败，实际输出退化为近似 `N / 2`。

原始证据：

- `docs/evidence-packs/terminal-bench-21-regression-subset-2026-07-20T22-37-28Z.md`
- `.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260720-222901`

## 通用产品修复

- 共享 `agent-core` 从任务正文的明确 example 句中提取数字字面量和被测程序身份；示例边界止于
  当前句，后续 timeout、年份或版本号不会被误并入，`e.g.,` 也不会重复计数。
- 仅当任务要求机器可判定行为且明确示例值至少形成多组输入/输出时，启用 verification
  diversity。单一固定输出任务保持既有行为。
- 成功断言若只使用正文示例，只记录 `last_example_only_verification_sequence`，不能单独完成。
- 项目测试、专用 verifier、generated/property check，或至少一组含非示例值且数据流来自同一
  被测程序的机器断言，才记录 `last_independent_verification_sequence`。变量覆盖会清除旧关联，
  无关管道、常量自证、quoted runner 文本均不能解锁。
- 真正执行的项目测试和 verifier 可作为独立证据；`--no-run`、collect/list/help/version 等仅发现
  或展示模式不能充当执行证据。`bash -lc 'cargo test'` 和被测程序经过 `tee` 的多级管道仍受支持。
- 收敛窗口中，example-only smoke 之后拒绝新的无关 read 探索，但允许纠正性 mutation 或创建
  测试/verifier；mutation 后沿用 R47 门禁，必须再次完成独立机器检查。
- Desktop OpenAI-compatible 与 Anthropic 路径在有限次恢复提示耗尽后发送明确 `Error`，不会把
  `completed=false` 的证据错误转换成成功 `Done`。
- shell 控制符分段感知引号、转义、注释和 command substitution；只有活动 substitution 才能
  建立目标关联。目标 pipeline 只允许跨透明 `tee` 传递，变量被 assignment、`printf -v`、
  `read`/`unset`/`mapfile` 或动态 `eval`/`source` 改写时旧关联立即失效。
- 若自然语言示例无法可靠提取被测 executable，ad-hoc 数值断言 fail-closed，必须使用真实项目
  测试或专用 verifier；前置 `VAR=value` 的真实测试 runner 仍能正确识别。
- 执行契约、桌面主 Agent 与 headless 使用同一证据模型；规则不包含 Terminal-Bench task 名、
  artifact 名、预期答案、仓库指纹或 verifier 读取。

## Failure-First

实现前的独立测试复现：多输入任务修改后，仅执行正文两组示例断言时，现有 completion gate
返回 `completed=true`。

实现后覆盖：

- example-only 断言保持未完成，增加非示例输入后完成；
- 完整项目测试可直接提供独立证据；
- 单一固定输出示例不会触发多样性门禁；
- example 句之后的 timeout/年份不会污染示例集；
- headless 最终回答被拦回后继续请求独立验证，并持久化两个 sequence；
- 收敛窗口阻止 example-only smoke 后继续无关 read，仍允许纠正性 edit 或创建测试/verifier。
- Desktop `require_action=false` 主路径在发生 mutation 后同样启用 R48，恢复次数耗尽时失败终止；
- 常量断言、无关 pipeline、quoted test 名、静态变量覆盖、非目标 executable 和 nonexecuting
  test/verifier 模式不能绕过门禁；
- 变量赋值断言、quoted runner 与多级目标 pipeline 不会被误拒；
- `0/1/2`、重复输出、中文无空格句号和单个 `e.g.,` 的边界均有回归覆盖。

## 锁屏真实 DeepSeek Runtime

最终 candidate sidecar 使用本机 CodeFactory 的 `deepseek / deepseek-v4-pro` 配置，在普通临时
项目 `/private/tmp/codefactory-product-eval-verification-diversity-r48` 修复任意非负整数平方工具。
正文只给 `3 -> 9`、`5 -> 25` 两组示例。

- status: `passed`
- screen locked at start/end: `true / true`
- duration: `17,780 ms`
- model requests: `4`
- usage: prompt `12,936`, completion `1,050`, total `13,986`
- external tool calls / outcomes: `3 / 3`
- repaired mutation: sequence `2`
- independent machine assertion: sequence `3`
- non-example cases: `0 -> 0`, `1 -> 1`, `7 -> 49`, `10 -> 100`
- independent post-exit assertion: `11 -> 121`, exit code `0`
- completion blocker: none
- workspace isolation: `macos-sandbox-exec`
- raw evidence: `.codefactory/product-acceptance/verification-diversity-r48-v5`

## 锁屏受控策略路径

同一 candidate sidecar 连接本地 OpenAI-compatible fixture。fixture 先修复工具，再只执行正文
示例断言，然后尝试直接结束；completion gate 拒绝该结束请求并发送 usage snapshot，下一轮
执行非示例 `7 -> 49` 后才允许完成。

- status: `passed`
- screen locked at start/end: `true / true`
- model requests: `5`
- provider usage: `75` tokens
- mutation sequence: `1`
- example-only verification sequence: `2`
- rejected final response usage snapshot: model request `3`, total tokens `45`
- independent verification sequence: `3`
- external tool calls / completion outcomes: `3 / 3`
- completion blocker: none
- raw evidence: `.codefactory/product-acceptance/verification-diversity-r48-fixture-v6`

## 回归结果

- agent core: `115 passed`
- headless: `21 passed`
- desktop Rust: `379 passed / 6 ignored`
- regression subset runner: `38 passed`
- targeted Clippy: passed
- targeted Rust formatting: passed
- governance baseline / long-task validation / `git diff --check`: passed
- final independent targeted re-review: `no blocker`; `r48_` `15 passed`

## 结论边界

R48 已通过共享策略、headless 协议、锁屏真实 DeepSeek Runtime 和受控 sidecar 策略路径验证，
但当前尚未合并、尚未发布，因此是 `not live`。只有 PR/CI、release、发布产物验证和同参数
released-build canary 完成后才能称为产品化。当前有效固定 18 题总分仍为 `6 / 18`。
