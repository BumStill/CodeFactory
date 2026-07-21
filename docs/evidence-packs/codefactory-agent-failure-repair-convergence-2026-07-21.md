# CodeFactory Agent 最终阶段失败修复闭环证据

- Req ID: `CF-TB-R50`
- 日期: `2026-07-21`
- 状态: candidate, not live
- Proof tier: `shared-policy + provider-protocol + controlled-headless-runtime`
- 基线: public `v1.51.9` released-source canary `ce1ecc95-3077-497d-ae9c-272d02a1f3fb`

## 失败基线

R49 已解决外层 watchdog 与 sidecar 生命周期脱节：同一 `circuit-fibsqrt` canary 在 780 秒
host cap 内主动退出，runner 无 intervention、无停止 workspace 后调用，模型用量从 `80` requests /
`686,352` tokens / `69` tools 降到 `17` / `126,633` / `12`。reward 仍为 `0` / verification。
最终轨迹显示最后一次检查失败后，Agent 又进行多次只读调查和纯文本模型响应，没有在剩余时间内执行
纠正性 mutation 与后续机器复验。这是通用开发 Agent 的收敛缺口，不是生命周期或题目特定缺口。

## 通用产品改动

1. shared `CompletionGate` 记录当前真实失败之后的第一条 ReadOnly 诊断序号。自主/执行任务进入最后
   8 个模型轮次或墙钟最后三分之一时，第二条 ReadOnly 被 `failure_repair_loop` 拒绝；Mutation、
   Verification、RuntimeProbe 和 bounded FunctionalProbe 仍可继续。
2. 失败 Mutation 不能再伪装为 workspace 改动。只有成功 Mutation 更新
   `last_mutation_sequence`；成功 no-op 也不能清除尚未复验的失败。
3. Verification、RuntimeProbe 和 bounded FunctionalProbe 失败保留所有未解决的最宽票据，而不是
   用最后一条失败覆盖原始失败。票据按 runner、大小写敏感且词法归一化的 cwd、package/test
   selector、workspace/feature/config 和排除条件建模；同范围、selector 超集或无 selector 的更宽检查
   可关闭，其他 cwd、其他 package/config、无关绿色和新增 `-k`/`--exclude`/`--ignore` 的更窄检查
   不能关闭。Python runner 自身缺失与 runner 已启动后的业务模块导入失败分开识别；仅前者和无效
   `cd` 等前置失败不创建不可重跑票据，但仍保留普通失败门禁。
4. completion evidence 未满足而模型给出纯文本结束时，只允许一次恢复。headless 与桌面
   Execute/Autonomous 的下一次 provider 请求都要求工具调用：OpenAI Chat Completions / ChatGPT
   Responses 使用 `tool_choice=required`，Anthropic 使用 `tool_choice={"type":"any"}`。provider
   若明确拒绝 forced tool choice，最多降级一次到 `auto`；若仍返回无工具响应，运行以
   `completed=false` 明确结束。只有成功 mutation、相同/更宽复验关闭失败票据或 completion blocker
   实质减少才能重置恢复次数；被拒绝工具、失败工具、只读诊断和无关绿色检查不能重新打开纯文本
   恢复循环。
5. Interactive 保留普通工具选择，不应用自主 final-stage 读取限制；已有 source-delivery、service
   lifecycle、verification-diversity 和 scope-narrowing 门禁保持优先。

实现仅依赖任务说明、工具类型、命令目标、退出状态、输出语义和当前执行预算；不包含 Terminal-Bench
任务名、仓库指纹、答案、hidden verifier 或任务特定修复脚本。

## Failure-First 与回归验证

- failure-first: 新增 final-third 测试先观察到第二条 ReadOnly 被错误允许；新增 recovery prompt
  测试先观察到下一轮未强制工具。独立审查后新增 7 组失败范围用例，旧实现稳定复现 5 组失败：
  RuntimeProbe 被无关绿色清除、Python 业务 import failure 被当作 runner 前置失败、`--workspace`/
  shell assertion/Vitest target 合并、Linux 大小写 cwd 合并、selector 超集误阻断；material-progress
  helper 在实现前编译失败。headless 另新增失败工具不能重开恢复循环的协议回归。
- shared core: `cargo test -p codefactory-agent-core` -> `128 passed / 0 failed`。
- headless protocol/runtime: `cargo test -p codefactory-agent-headless` -> `27 passed / 0 failed`。
  覆盖失败检查 -> 一次诊断 -> 拒绝第二次读取 -> mutation -> 同检查复验，以及第一次纯文本被拒后
  请求 `required`、第二次仍无工具则 incomplete 停止。
- desktop product loop: `cargo test -p codefactory` -> `395 passed / 6 ignored / 0 failed`。
  覆盖 Autonomous/Execute final-stage 路由、Interactive 豁免、一次恢复限制和 OpenAI
  `none/auto/required` 工具选择。
- frontend: `pnpm test` -> `54 files / 239 tests passed`；`pnpm build` 通过，仅保留既有 chunk-size warning。
- governance baseline、long-task validator 和 `git diff --check` 均通过。
- formatting: 三个可独立格式化的本轮 Rust 文件通过直接 `rustfmt --check`；仓库级
  `cargo fmt --all --check` 被当前主干其他既有未格式化文件拦截，本轮未批量改写无关文件。

## 真实 DeepSeek 产品 Runtime

- Fixture: `/private/tmp/codefactory-product-eval-failure-repair-r50-v1`，普通 Python slug normalizer；
  不含 Terminal-Bench task、verifier、仓库指纹或隐藏答案。
- Provider/model: `deepseek / deepseek-v4-pro`；policy=`product`；max steps=`10`；wall=`300s`。
- failure-first v1: Agent 已完成源码修复和 unittest，但 completion 恢复请求使用
  `tool_choice=required`，DeepSeek thinking mode 返回 HTTP 400
  `Thinking mode does not support this tool_choice`，Runtime 如实失败，没有伪装为成功。
- fixed v2: `.codefactory/product-acceptance/failure-repair-r50-v2`，`status=passed`、
  `completion_evidence.completed=true`、无 blocker；`last_failure_sequence=3`、
  `last_mutation_sequence=4`、`last_successful_verification_sequence=5`。
- v2 trajectory: 错误 cwd 的 precondition failure -> 一次目录诊断 -> 真实 unittest failure ->
  最小源码修复 -> 同一 `python3 -m unittest -v` 复验通过。独立在 fixture cwd 重跑 unittest 为 OK。
- v2 duration=`36,936ms`，tool calls=`5`，model requests=`8`，tokens=`42,596`。
- exact headless SHA-256: `70cb2206980baea38b60bf235d71542ff81a23ad928522d35d4583552ddf623d`。
- execution contract SHA-256: `fbdddec740c94908e4d3e974540519ab606668e547ce7848c0a497261c50f1fd`。
- provider forced-choice fallback 的 `required -> auto` 请求字段由 headless fake-provider protocol test
  精确断言；本地脱敏 Runtime trajectory 不记录 provider payload，因此不把真实 run 单独包装成字段证据。
- final candidate v4: 显式重建当前 sidecar 后，`.codefactory/product-acceptance/failure-repair-r50-v4`
  再次以 `deepseek / deepseek-v4-pro` 和相同普通 fixture 通过：`status=passed`、`completed=true`、
  blockers 为空，`last_failure/diagnostic/mutation/verification=5/4/6/7`。轨迹包含真实 unittest failure、
  最小源码 mutation、同一 unittest 复验；独立终端再次为 OK。duration=`28,340ms`，tool calls=`7`，
  model requests=`9`，tokens=`45,951`；当前 sidecar SHA-256 为
  `dcfaaa6ef73322483671920ef514e25e6b3d9261a0ae5dd97164124c71d1ec32`，contract SHA-256 为
  `b15fa14f6243ea94e78717df51c22a13a9a791f1ce0b59ade21abbe785bb09f8`。

## 独立审查

- 第一轮发现 4 个 P1：非 Verification probe 失败可被清除、Python import failure 误判、范围 fingerprint
  合并非等价检查、失败/无关工具重置恢复；另有 cwd/selector P2 和 desktop provider 协议测试 P2。
- failure-first 修复后，同一审查者逐项核销全部 4 个 P1 与 cwd/selector P2，最终结论为“无发布阻塞
  P1”。剩余非阻塞 P2 是 desktop OpenAI/ChatGPT/Anthropic 尚无请求捕获级 `required -> auto`
  回归；实现、共享 detector/tool-choice 单测、headless HTTP 协议测试和真实 DeepSeek fallback 已存在，
  后续补齐桌面 transport 测试，不阻断本轮发布。

## 待交付门禁

- fetch/merge 最新 `origin/main` 后重跑测试。
- PR/CI、remote real-App、deliberate Auto Release、公开安装产物复验。
- 精确发布 tag 上保持 task、model、verifier、resource 和 900 秒 outer watchdog 不变，重跑同一
  focused canary。单题 canary 只验证行为差异，不能改变固定 18 题 `6 / 18` 的有效总分。
