# M286 R7：会话 2509e667 路由根因证据

会话 `2509e667` 的 `prefer openai/gpt-5` 没有产生 OpenAI 尝试，根因是派单模型字符串解析歧义：`openai/gpt-5` 被旧逻辑当作 OpenRouter `owner/model`，在进入 OpenAI 候选尝试记录前已落到 OpenRouter 路径；同类现场证据见会话 `2804a05f`。

修复位于 `src-tauri/src/commands/chat.rs`：斜杠左侧若匹配已配置端点名，优先解析为 `endpoint/model`；端点无模型时返回 `ENDPOINT_MODEL_UNAVAILABLE`，只有左侧不是端点名时才保留 OpenRouter `owner/model`。

验证：
- `endpoint_model_hint_errors_when_endpoint_lacks_the_model`
- `owner_model_remains_for_openrouter_and_is_not_treated_as_endpoint_hint`

实现方完成本地 synthetic 路由验证；编排方在真实应用验收时查询 `model_route_attempts`，确认新派单出现 `endpoint=openai`。真实用户数据库和凭据不进入仓库。
