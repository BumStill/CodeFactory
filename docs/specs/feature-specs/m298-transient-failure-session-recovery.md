# M298：ChatGPT 流断 → 整端点冷却 → 回落 OpenAI Responses 400 → 10 秒内 3 次判死，会话失败结束

## 现场（v1.131.0，2026-11-27 09:45，只读核实）
- 会话 d66ba8d7（M293）与 08467b13（M295）同一时刻：
  1. 09:45:16/18 chatgpt gpt-5.6-sol `TRANSPORT_UNREACHABLE: error decoding response body`（流式响应中途被切断）→ chatgpt 整端点冷却到 +15 分钟（之后 3 次 `skipped endpoint_cooldown`）。
  2. 回落 openai gpt-6.1-terra（M286 R10 的 Responses 传输）→ 4 次 `HTTP 400 Invalid value: 'input_text'. Supported values are: 'output_text' and 'refusal'`（FIELD_UNSUPPORTED）——assistant 历史消息被序列化成 `input_text`。
  3. d66ba8d7 以「同一个技术失败连续 3 次」结束（ended，failed），任务未完成；08467b13 停在 waiting_wake。
- 编排方临时缓解：openai 日上限设 $0.01（赠额已尽），使其被预算跳过。

## 规格
- R1（Responses 序列化）：OpenAI `/v1/responses` 请求中 assistant 角色历史内容必须用 `output_text`（或 refusal），user/tool 用对应输入类型；工具调用与工具结果按 Responses 规范的 `function_call` / `function_call_output` 结构；有图片时 user 侧 `input_image`。用真实形状的多轮历史（user→assistant→tool→assistant→user）做请求快照测试，并用本地合成 Responses 服务器按官方校验规则拒绝错误类型。
- R2（流断不冷却整端点）：流式响应中途断开/解码失败（`error decoding response body` 等）且尚未产生不可重放副作用时，按瞬时错误在同一回合内有界重试（指数退避、具名上限），不把整个端点冷却 15 分钟；只有连续多次建连失败才短冷却。新增重试状态机的 provider round future 必须用 `Box::pin` 堆分配，避免 Windows phase-one worker 的 1 MiB 主线程栈溢出。这是原 M284 R4 与 M286 Windows 栈修复中未覆盖的组合路径。
- R3（不因瞬时故障判死）：「同一技术失败连续 N 次即结束会话」只适用于确定性、不可恢复的错误；对瞬时错误或「当前无可用端点但已知恢复时间」的情况，会话应进入等待并在最早恢复时刻自动继续（与 M287 活性 owner=wake 对齐），不得结束。FIELD_UNSUPPORTED 这类请求构造错误应当回落到其他端点，而不是重试同一端点到判死。传输故障在 24 小时长窗口内保持系统等待；超过长窗口仍无任何端点可用时必须明确、可读地结束，不得无限挂起或保留中间 remediation/wake。
- R4：结束原因对用户可读、如实（不显示内部错误码堆叠）；d66ba8d7 这类被错误结束的会话可由系统续接（不需要用户点按钮）。

## 测试（先红后绿）
- 合成 Responses 服务器：错误类型 → 400；修复后 200。多轮含工具调用的历史。
- 合成 ChatGPT 流：中途断开一次 → 同回合重试成功，端点未冷却；连续建连失败才短冷却。
- 判死逻辑：瞬时错误 ×3 不结束会话、进入 wake；确定性错误按原逻辑。反例覆盖。

## 约束
改文件只用 edit_file / write_file。不改信任根；场景缺口只追加 `docs/testing/scenario-mappings.json`。不碰凭据、settings.json、其他会话 PR，不动任何 merge-queue: hold 标签。规格文件在工作区外，用 bash cat 只读查看。

## 交付
PR 正文 `R | 是否完成 | 证据` 列 R1–R4；`Release-Urgency: immediate`（会话因瞬时网络抖动被直接判死，主路径不可用）。
