## 问题

纯文本模型（如 `deepseek-chat`）遇到图片时，图片被 `strip_image_parts` 换成占位符，
模型只能猜；用户让它"看看这页 PPT 的布局"时，它会去调 `browser_session` 打开本地
PNG 并失败，最后靠坐标硬猜版面。M153 要求：任何模型都能看图，且不因能力差异让任务失败。

## 这轮做了什么

新增**视觉桥**（`view_image`），按固定顺序尝试两条互不依赖的路径，并把"图片去哪了"
写进工具输出首行：

1. **视觉端点路由**：只从用户已配置、显式支持视觉、且有凭据的端点里选
   （`Settings.vision_assistant` 可指定首选）；图片以 OpenAI 兼容的 `image_url`
   内容块（base64 `data:` URL）随问题发出。
2. **本地 OCR 兜底**：无可用视觉端点时用系统 OCR —— macOS Vision（一次性编译
   Swift shim）、Windows `Windows.Media.Ocr`、`tesseract`；返回文字 + 归一化坐标，
   并明确标注"未使用视觉模型"。
3. **都不通**：返回可读原因与下一步建议，**回合不失败**。

配套：

- `tools::definitions_for_capability` 按模型能力裁剪工具清单：自带视觉的模型看不到
  `view_image`，纯文本模型才看得到。
- PPTX/PDF 支持按 `page` 渲染成 PNG（PowerPoint COM → LibreOffice+pdftoppm →
  QuickLook 仅第 1 页并显式告警），渲染局限写进输出。
- 描述按（会话、图片哈希、问题哈希）缓存，命中后不再产生任何模型调用。
- 输出 `metadata`：`surface=vision_bridge`、端点、模型、输入/输出 token、是否降级。

## 需求状态（不夸大）

| Req | 状态 | 说明 |
|---|---|---|
| R1 工具暴露 | ✅ | 两向单测：视觉模型不暴露、纯文本模型暴露 |
| R2 路由与用量 | ⚠️ 部分 | 凭据 + `supports_vision` + 首选已做；usage window 过滤、`model_usage_events.purpose` 列、设置页控件未做（当前归属靠 `metadata.surface`） |
| R3 OCR 兜底 | ✅ | 三后端 + 真实证据：QuickLook 渲染 PNG → 系统 Vision 读回 `HELLO FACTORY 42` |
| R4 附件自动注入 | ❌ 未做 | 缓存能力已就绪，注入点未接 |
| R5 分页渲染 | ⚠️ 部分 | 渲染已落地并可用于 `view_image(page=…)`；未接进 `read_pptx` 本体 |
| R6 browser_session 指路 | ❌ 未做 | 错误文案仍为原文案 |
| R7 隐私归属 | ✅ | 候选集只来自 settings；首行固定声明发送目标 |

Windows 的 COM 与 WinRT 路径已实现，但本轮只有 macOS 真实运行证据，Windows 需实机冒烟。

## 验证

- `cargo check --lib`：通过。
- `cargo test --lib -- vision:: tools::view_image`：21 passed。
- 真实链路证据（非结构检查）：本地 SSE 桩服务抓取真实请求体，断言
  `"type":"image_url"`、`data:image/png;base64,...`、问题文本与模型名确实上线，
  并回填 usage；`real_ocr_reads_text_from_a_rendered_image` 渲染→识别闭环。
- `python tools/governance/validate_repo_governance_baseline.py`：pass。
- `python tools/governance/validate_scenario_test_governance.py --repo .`：OK（32 scenarios）。

## AI Collaboration

- context scope：`src-tauri/src/{vision,tools,agent,config}`，`docs/specs/feature-specs/vision-bridge.md`，场景注册表。
- assumptions：视觉端点元数据 `supports_vision` 可信；系统 OCR 无需额外安装是合理的默认兜底。
- review point：`view_image` 的能力裁剪、OCR 降级文案、以及 Windows 路径尚未实机验证。
- validation result：见上（L0/L1），L2/L3 未覆盖。

Scenario-Test: VIS-001
README-Update: required
README-Update-Reason: 新增面向用户的看图能力（纯文本模型也能读截图、图表与幻灯片），README 功能表加一行并说明图片只发往用户已配置的端点。
