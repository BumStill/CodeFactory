# M286：OpenAI API 模型自动获取、按性价比选默认、单价准确

## 现场（v1.125.0，2026-11-25 核实）
- openai 端点只有手写的 gpt-5。OpenAI 官方弃用页：gpt-5 于 2026-12-11 停用（替代 gpt-6.1-sol）。
- `commands/models.rs list_models` 用 OpenRouterClient 解析 `/v1/models`，失败只记日志、静默返回自定义模型。
- `openai_api.rs` 单价表日期 2025-09，没有 gpt-5.6/gpt-6.x。前缀匹配使 `gpt-5.6-sol` 按 gpt-5（$1.25/$0.125/$10）计，官方现价 $4/$0.40/$20，会低估并突破日上限。gpt-6.x 落保守价（缓存 $1.25），比官方 $0.05–$0.10 高一个数量级，几十次调用就误触上限。
- 派单 `model=openai/gpt-5` 被解析成 OpenRouter 的模型（会话 2804a05f），应先匹配已配置端点名。
- `model_usage_events.estimated_cost_usd` 对 openai 事件为空、cost_source=unknown（会话 fab1c487）。
- 会话 2509e667 为 prefer openai/gpt-5，却一直用 chatgpt 且无 openai 尝试记录。

## 用户决定
- 以性价比为准：不默认最新、不默认最高思考强度。模型不写死，自动取可用列表。
- 默认档「均衡」= terra 族（当前 gpt-6.1-terra）+ 中等思考强度；难单可升 sol 族（当前 gpt-6.2-sol）。型号只作族偏好，不写死具体版本。
- 每日 $3 上限不变。

## Requirements Traceability
- R1：按 OpenAI 格式解析 `/v1/models`，只列可用于对话的模型；缓存并定期刷新；失败在界面和派单结果可见，保留上次成功列表，不静默当空。
- R2：默认模型由可配置的偏好（型号族 + 档位，默认「均衡」档）在可用列表中解析到该族最新版本；列表没有该族时按明示顺序回落并提示。已停用或不在列表里的当前模型自动换成可用项，并在对话里说明一次。
- R3：思考强度可配置，默认中等；不发送该模型不支持的取值。用户已选的值不被覆盖。
- R4：单价表更新为官方现价（输入/缓存/输出，写明来源与日期）；匹配按完整型号族边界，`gpt-5.6-*` 不得落到 `gpt-5`；长上下文、Fast 等加价档须计入；未知模型继续用保守价，且不得低于表内同族最高价。
- R5：openai 事件写入 estimated_cost_usd 与 cost_source；与日上限账本口径一致，重启重建结果一致。
- R6：派单 `model` 为「端点名/模型」时优先解析为已配置端点，端点上没有该模型则明确报错，不静默落到 OpenRouter。
- R7：查清 2509e667 为何不尝试 openai，写清根因；若是路由或冷却缺陷则修复，否则说明。
- R8：增加派单操作 `list_models`（只读，按端点返回可用模型及单价），支持锁屏使用；不返回凭据。
- R9：不改日上限数值、周额度、路由顺序和权限判定。
- R10：gpt-6.x（已核实 gpt-6.1-terra、gpt-6.2-sol）在 `/v1/chat/completions` 上返回 400「Function tools with reasoning_effort are not supported … use /v1/responses」。openai 端点对这类模型必须改走 `/v1/responses`（工具调用、思考强度、流式、用量计费都要对齐）；synthetic 服务器测试覆盖两种接口的选择与回落。
- R11：openai 无工具请求不得发送 `tool_choice`，避免 400 `FIELD_UNSUPPORTED`。

## 测试
本地 synthetic 服务器返回 OpenAI 格式列表：成功、失败、非对话模型混入、当前模型消失。单价：gpt-5.6 不落 gpt-5、gpt-6.x 正确、未知保守、缓存占比高的真实形状（133k 输入/129k 缓存/771 输出）。openai/… 解析到端点。反例：openrouter 的 owner/model 选择不受影响。不读真实 Key，探针用隔离 HOME。gpt-6.x 带工具与思考强度时使用 Responses API；非 gpt-6 兼容模型保留 Chat Completions；无工具请求不发送 `tool_choice`。

## 验收（真机，编排方发版后做）
`list_models` 返回真实列表；派单用默认「均衡」档跑一次最小请求走 openai，usage_status 金额与官方单价一致。

Release-Urgency: default
