# M153：视觉桥接（不能看图的模型也要能「看图」）

## 背景与用户决策

用户在 Windows 会话里用纯文本模型（`dfa_self_deepseek_v4_flash`）优化一页含 105 个
文本框的 PPT。模型想「先截图看整体布局」，却去调 `browser_session` 打开本地 PNG，
得到 `browser_session has no open session here`；随后只能靠 python-pptx 导出坐标硬猜
布局，还撞上 PowerShell 不支持 heredoc。

代码现状：`src-tauri/src/agent/context.rs` 的 `model_supports_vision` 为假时，
`codefactory_agent_loop::protocol::strip_image_parts` 把图片换成占位符
（`agent/mod.rs` 的 `IMAGE_STRIPPED_PLACEHOLDER`），`failover.rs` 的
`is_vision_rejection` 再把服务端拒绝归一到同一条路。结果就是：图被丢掉，模型失明。

用户决策（任务书）：让任何模型（含便宜的纯文本模型）都能处理截图、图表、PPT/PDF
页面、UI 设计稿；不因模型能力差异让任务失败，也不许靠坐标猜。

## 阶段 0 调研结论

### 0.1 业界做法（2026-10 检索）

| 产品/方案 | 对非视觉模型的图片处理 | 对本案的可借鉴点 |
|---|---|---|
| Claude Code | 只对视觉模型投递图片；非视觉模型下图片被丢弃，由模型自行用命令查看 | 说明「静默丢弃」是普遍缺陷，不是本仓独有 |
| Cursor | 强制视觉模型（自带多模态路由），非视觉模型不可选图片 | 「路由到视觉模型」是主流替代方案，但对 BYO-key 客户端不可行（用户可能没配视觉模型） |
| Cline | 视觉模型原生传图；非视觉模型提示「该模型不支持图片」并建议换模型 | 提示语有价值，但仍让任务停在半路 |
| Aider | 完全不做图片输入（文本 only），把「理解图片」留给外部工具 | 说明缺少本地兜底时会直接失能 |
| Open WebUI | 把「图片描述/OCR」作为独立任务模型（RAG/任务模型可单独配置） | 与本案 R2/R3 同构：视觉助手是被路由选出的独立任务端点 |
| vision sidecar / captioning bridge 模式 | 小视觉模型（或本地 OCR）先产出文字描述，再把描述喂给纯文本主模型；主模型可按需追问 | 采用：主模型保留对话与工具循环，视觉能力作为可调用旁路 |

结论：**「按需调用的视觉旁路（sidecar）+ 离线 OCR 兜底 + 明确的可用性路由」**是唯一
同时满足「任意模型可用」「不增加主回合失败率」「隐私只走用户已配置端点」的组合。
纯「换模型」「提示后停下」都不满足任务书目标。

### 0.2 本机/用户可用的视觉能力盘点

- **已配置视觉模型**：`Settings.endpoints[*].custom_models[*].supports_vision` 已是
  显式元数据，`agent::context::model_supports_vision` 已有名称推断兜底；`ChatGPT`
  订阅端点（openai/codex 风格）与 `deepseek-v4-flash-vision-exp` 一类模型可作视觉助手。
  可用性判定必须复用 M152：凭据存在 + usage window 允许 + 不处于配额的「留余量」外。
- **系统 OCR（无需凭据、离线、免费）**：
  - macOS：Vision framework `VNRecognizeTextRequest`（本次以 `swiftc` 编译一次性 shim
    调用；`swiftc` 缺失时该后端不可用）。
  - Windows：`Windows.Media.Ocr.OcrEngine`（WinRT；已在 Microsoft Learn 核实 API
    存在与语义，通过 PowerShell 调起，避免为单一工具引入重量级 WinRT 绑定）。
  - 通用：`tesseract` CLI（若在 PATH 上），跨平台且便于 CI 断言。
- **本机实测（本次会话，macOS）**：`swiftc` `/usr/bin/swiftc` 存在、`qlmanage` 存在、
  `tesseract` `/opt/homebrew/bin/tesseract` 存在；`soffice`/`libreoffice` 缺失。
  即：macOS 上 OCR 有两条可用后端；文档页渲染在无 LibreOffice 时只能降级。
- **不选的理由**：
  - 不内置本地视觉大模型（如 moondream/llava）：体积、显存与首次下载成本远超收益，
    且与「本地优先、无遥测」不冲突但会显著拖慢安装包。
  - 不把图片发往任何未配置的第三方（含公共 captioning API）：违反 R7。
  - 不静默把图片改写进主模型上下文（原生多模态仍优先）：会重复计费并破坏原生视觉体验。

### 0.3 判定顺序

1. 当前模型本身支持视觉 → 原生传图，**不暴露** `view_image`。
2. 当前模型不支持视觉 → 暴露 `view_image`；调用时优先视觉助手模型（显式首选 → 自动路由）。
3. 无可用视觉助手 → 系统 OCR（返回文字 + 文字框坐标，并明示「仅 OCR，无图像理解」）。
4. 两者都不可用 → 返回人话原因与可执行的下一步（配置视觉模型 / 安装 tesseract 等），
   **绝不让回合失败**。

## Requirements Traceability

| Req ID | 需求 | 最低证据 |
|---|---|---|
| R1 | 非视觉模型下暴露 `view_image(path\|attachment, question)` 工具；视觉模型下不暴露，图片原生传入。同一图片可多次追问不同问题。 | 工具注册/过滤单测（两向）+ 执行单测：非视觉模型调用后得到文字答案；参数校验错误可读。 |
| R2 | 视觉助手路由：只从「已配置 + 有凭据 + usage window 允许 + `supports_vision`」的模型里选；支持设置页指定首选（默认自动）；用量记入 `model_usage_events` 且 `purpose=vision_bridge`。 | 路由单测：无凭据/窗口外/非视觉模型不参与；首选命中；usage 断言 purpose 字段。 |
| R3 | 离线兜底：无可用视觉助手时用系统 OCR（macOS Vision / Windows.Media.Ocr / tesseract），返回文字 + 坐标并标注「仅 OCR」；都不可用时返回人话原因且回合不失败。 | OCR 后端选择单测 + 真实 PNG 的 OCR 集成测试（可用后端存在时断言抽到已知文字）+ 全不可用的降级文案单测。 |
| R4 | 用户附件在非视觉模型下自动做一次简短描述注入上下文，保留图片路径并提示可用 `view_image` 追问；注入内容按图片 hash 持久化，重放/继续不重复调用视觉模型。 | 缓存单测（第二次命中缓存、调用计数不增）+ 注入文本断言（含描述与路径）。 |
| R5 | `read_pptx`/PDF 读取类工具支持把指定页渲染为 PNG（Windows 优先 PowerPoint COM；macOS/Linux 优先 LibreOffice；macOS 无 LibreOffice 时 Quick Look 仅第 1 页并显式告警），缺渲染器时给明确说明。 | 渲染计划单测（平台/可用性 → 计划）+ 真实 PPTX 渲染集成测试（渲染器存在时断言 PNG 署名与尺寸）。 |
| R6 | `browser_session` 不得用于查看本地图片：收到本地图片路径时错误信息指向 `view_image`。 | 单测：`open`/`read` 传本地 png 路径 → 错误包含 `view_image`。 |
| R7 | 隐私：图片只发给用户已配置的端点；工具卡片显示「已发给 <模型> 识别」；不得发往未配置的第三方服务。 | 工具输出文本断言 + 路由集合断言（候选集只来自 settings）。 |

## Applicable Harnesses

- **Spec Harness**：本文件即规格；实现只覆盖 R1–R7。
- **Compatibility Harness**：`view_image` 只在非视觉模型下出现；旧 `Settings` 文件缺新字段
  仍可反序列化（`#[serde(default)]`）；`read_pptx` 原有行为不变。
- **Observation Harness**：`view_image` 输出标注来源（视觉模型名 / OCR 后端 / 降级原因），
  用量写入 `model_usage_events`（`purpose=vision_bridge`）。
- **Payload Harness**：图片以 base64 data URL 传递，限制单图大小与像素上限，超限给明确错误。
- **Viewport Harness**：设置页新增「视觉助手」选择项（浅色/深色都要可读）。
- **AI Collaboration Harness**：见 PR body 的 context scope / assumptions / review point / validation。

## 测试矩阵（先失败，合成 fixture）

| 用例 | 覆盖 | 类型 |
|---|---|---|
| `view_image_hidden_for_vision_models_and_shown_for_text_models` | R1 | unit |
| `view_image_rejects_unknown_extension_and_missing_question` | R1 | unit |
| `vision_route_skips_uncredentialed_and_out_of_window_models` | R2 | unit |
| `vision_route_prefers_configured_assistant` | R2 | unit |
| `vision_bridge_records_usage_with_purpose` | R2 | unit |
| `ocr_fallback_extracts_known_text_from_generated_png` | R3 | integration（真实后端；无后端时显式 skip 并记录） |
| `no_backend_returns_human_reason_without_failing_turn` | R3 | unit |
| `attachment_description_is_cached_by_image_hash` | R4 | unit |
| `attachment_injection_keeps_path_and_mentions_view_image` | R4 | unit |
| `render_plan_prefers_powerpoint_then_libreoffice_then_quicklook` | R5 | unit |
| `pptx_page_renders_to_png_when_renderer_available` | R5 | integration（合成 pptx） |
| `browser_session_local_image_points_to_view_image` | R6 | unit |
| `tool_output_names_the_vision_model_for_privacy` | R7 | unit |

## Implementation Notes

- 视觉旁路与主循环解耦：`view_image` 走普通工具调用，主模型保持自己的对话与工具循环；
  因此同一张图可多次追问，且不改变主模型的上下文预算策略。
- 附件自动描述（R4）只做一次、按 `sha256(图片字节)` 缓存，缓存表随迁移创建；
  命中缓存时不产生任何网络调用。
- OCR 结果同时返回归一化坐标（0–1），因为「布局问题」问的往往不是文字本身而是文字位置。
- 渲染器缺失时不静默失败：错误里列出已探测的渲染器与安装建议，模型可以把这条信息
  如实转达给用户，而不是继续猜坐标。

## 实现状态（本轮落地，逐条对照）

代码：`src-tauri/src/vision/{mod,ocr,render}.rs`、`src-tauri/src/tools/view_image.rs`、
`src-tauri/src/tools/mod.rs` 的 `definitions_for_capability`。

| Req | 状态 | 落点与证据 |
|---|---|---|
| R1 | ✅ 已实现 | `definitions_for_capability` 按 `model_supports_vision` 裁剪；测试 `schema_exposure_follows_model_vision_capability`（两向断言）。执行层 `view_image` 支持重复追问，参数/文件错误均为可读错误。 |
| R2 | ⚠️ 部分实现 | 路由只从「显式 `supports_vision` + 有凭据（或 ChatGPT 订阅）」的端点里选，支持 `Settings.vision_assistant` 首选（`endpoint::model`）；证据 `preference_and_default_ordering_are_stable`。**未做**：usage window 过滤、`model_usage_events.purpose` 列（当前以工具输出 `metadata.surface="vision_bridge"` + endpoint/model/tokens 归属）、设置页选择控件。 |
| R3 | ✅ 已实现 | macOS Vision（`swiftc` shim，临时目录缓存）、Windows `Windows.Media.Ocr`、`tesseract` 三后端；输出首行声明证据来源，OCR 结果永远带「仅 OCR」说明。真实证据：`real_ocr_reads_text_from_a_rendered_image`（QuickLook 渲染 PNG → 系统 OCR 读回 `HELLO FACTORY 42`）。 |
| R4 | ❌ 未实现 | 本轮只做 `view_image` 按需调用；附件自动描述注入与「重放不重复调用」仍是后续工作。按图片哈希缓存描述已实现（`cache_hit_does_not_call_the_backend_twice`），可直接接到该注入点。 |
| R5 | ⚠️ 部分实现 | 分页渲染已落地（PowerPoint COM / LibreOffice+pdftoppm / QuickLook 仅第 1 页并显式告警），`view_image(page=…)` 可直接用；证据 `quicklook_renders_a_real_png`、`unsupported_extension_is_rejected_with_a_clear_reason`。**未做**：把渲染能力接进 `read_pptx` 本体。 |
| R6 | ❌ 未实现 | `browser_session` 收到本地图片路径时的错误文案仍指向原逻辑，需下一轮补 `view_image` 指路。 |
| R7 | ✅ 已实现 | 候选集只来自 `settings.endpoints`；输出首行固定「视觉助手：<endpoint> / <model>（图片已发送给该端点识别）」或「本地 OCR（…）：未使用视觉模型」；请求体实测断言见 `image_is_sent_as_an_image_url_part_and_the_answer_comes_back`（本地 SSE 桩服务抓取请求原文）。 |

场景登记：`docs/testing/scenario-registry.json` → `VIS-001`（required_evidence L0/L1，
gates `pull_request`）。
