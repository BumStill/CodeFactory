# Token 消耗审计与优化方案

## 问题

任务执行的 token 统计能记录 Provider 返回的每次请求，但发送前的上下文预算估算并不完整，可能在上下文超限时先浪费一次请求，再压缩并重试；同时多轮工具链会反复重送历史，固定系统提示和工具 schema 也会在每轮重复计入。

## 已确认的现状

- Provider usage 按 `usage_run_id:iteration` 记录，工具中间轮、最终轮和完成恢复轮会分别计量；同一 attempt 重送具备幂等键，统计口径总体合理。
- `send_with_retry_and_notify` 对网络/408/429/5xx 最多三次请求；这些是实际 Provider attempt，不能按“用户轮次”合并，否则会低估真实消耗。HTTP 层失败且没有 Provider usage 时通常不会落 usage row，但可能产生服务端已处理、客户端未收到 usage 的未知成本。
- 一次工具调用后必然再发一次完整历史，这是 Chat Completions/Responses 的协议形态；当前真正的放大器是工具输出和历史内容大小，而不是 UI 的 token 统计。
- `estimate_prompt_tokens(messages, system_prompt)` 当前只计算 system prompt、message content、tool call 名称/参数和固定 per-message overhead；没有计算本轮 `active_tool_defs` 的 name/description/JSON schema，也没有计算 `reasoning_content` replay。
- `run_agent_loop` 在计算 `active_tool_defs` 之前就估算并决定上下文窗口/压缩；因此工具 schema 很大时可能先发送一次必败请求，再走 emergency compression retry。
- 桌面工具源头已有边界：`grep` 约 64K 字符、单匹配行 4K/500 条；`bash` 约 30K；但 `read_file` 默认最多 2000 行，没有统一字符/token预算，Office/PDF 提取也可能形成较大结果。
- system prompt 的固定 contract、self-recovery、repository intent、时间、项目知识、skills、用户偏好等每轮都会重送；知识块有 24K 字符共享上限，但固定 base 与额外 route/compliance/readiness 文本在该预算外追加。
- 完成门禁可能追加 recovery、ready、summary、fact-check 和 checkpoint 轮；这些是产品正确性所需，但可以通过减少无效工具调用和更早证据收敛降低，而不是简单关闭门禁。
- Anthropic 路径当前不做历史压缩，只做 overload retry；长会话在该 provider 上仍有上下文增长风险。

## 优先级

### P0：修正发送前预算估算

新增 `estimate_prompt_tokens_with_tools(messages, system_prompt, tools)`，至少计算：

1. `reasoning_content` 的估算 token；
2. 每个本轮实际工具 definition 的 name、description、序列化 parameters 和 JSON envelope overhead；
3. 现有消息/工具调用估算。

把 `active_tool_defs` 的计算移到预算判断之前，并把它传入 `ContextCompactor`。桌面 `DefaultCompressor` 用完整 token 估算触发压缩；headless `CharBudgetCompactor` 明确忽略该参数，保持其字符预算评测语义不变。压缩触发后仍必须用最终 active tool set 重新检查，避免 finalization 的空工具集被过度压缩。

### P1：统一工具输出预算

给 `read_file`、Office/PDF text extraction 和其他高体积工具增加统一的模型输出字符上限，并优先保留头尾、路径、行号和截断标记。默认 `read_file` 不应只按 2000 行限制，因为 minified/长行文件仍可能产生大 payload。工具输出预算应低于单消息压缩上限，减少进入 replay 后才被压缩的浪费。

### P1：减少无效工具轮

保留完成门禁，但针对重复只读、已成功验证命令和明显重复搜索建立更强的证据复用；把“已达到完成证据但仍请求工具”的情况尽早转为一次无工具总结，而不是让模型多走恢复轮。不要通过降低最大迭代次数解决，这会把正确任务变成提前失败。

### P2：优化固定上下文

将稳定的 system prompt、工具 schema 和会话历史分别统计，先做可观测性再决定缓存策略：

- Provider 支持 prompt caching 时，标记稳定 prefix；
- 不支持时，优先缩短固定 contract、默认关闭与当前任务无关的 skills/知识块；
- 工具 schema 按能力/turn capability 动态裁剪，而不是每轮发送全部 Office、知识、skill-management、delivery、MCP 工具。

动态裁剪必须保留模型可能需要的工具，不应按用户自然语言做不可靠的激进猜测；安全做法是按当前 capability、已启用 MCP、surface 和 finalization 状态裁剪。

### P2：把 token 观察做成可诊断数据

在不保存 prompt、reasoning 或凭据的前提下，记录每个 round 的：估算 prompt、Provider input/output、tool schema estimate、message estimate、压缩前后 estimate、retry reason、是否 recovery/finalization。这样可以直接回答每个任务 token 花在“固定上下文 / 历史 / 工具结果 / 输出 / 重试 / 门禁恢复”的哪一类。

## 不建议

- 不把多轮工具请求合并成一次统计：会掩盖真实 Provider 计费。
- 不默认删除 reasoning replay：DeepSeek 等 provider 需要它保持协议合法；应先按 provider 能力决定是否可摘要化。
- 不仅靠扩大 context window：会提高单次输入成本，且不能解决重复工具输出。
- 不把所有历史永久摘要化或改写 SQLite：会损害审计、恢复和兼容语义；应只改 provider replay 副本。

## 验证矩阵

- 失败优先：工具 schema + reasoning replay 使 prompt 估算超过压缩阈值的单元测试。
- 估算对账：fixture 同时断言 message、schema、reasoning 三部分和总估算。
- loop 集成：active tool definitions 在压缩前可用，压缩后的发送 payload 不超过 context limit。
- 回归：headless 字符压缩输出保持不变；finalization 空工具集不被错误纳入 schema 预算。
- Provider fixture：一次 context overflow 不再先发送原 payload再 emergency retry；正常 HTTP retry 仍按真实 attempt 计量。
- 工具边界：超长单行、2000 行短文本、短行大文件、Office/PDF 长文本均满足统一输出预算。
- 隐私：诊断数据不包含 prompt、reasoning、工具参数或凭据。

## 当前阻塞

本回合的代码/测试写入工具持续返回“当前显式只读意图”，尽管用户已明确要求继续执行；因此没有源码或测试变更，也没有修改后验证证据。恢复可写执行后，按 P0 的失败测试 → focused Rust test → 实现 → focused/full suite 顺序执行。
