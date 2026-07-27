# ChatGPT 图片输入兼容规格

## 背景

CodeFactory 会把消息中的本地图片附件转换为内部 `image_url` content part。ChatGPT 订阅模型使用 Responses API；该传输层必须继续保留图片，而不能把整条消息压成纯文本。

## Requirements Traceability

| Req ID | 用户要求 | 归一化要求 | Surface | 验证方法 |
| --- | --- | --- | --- | --- |
| CF-VISION-R1 | 解决图片根本没有识别到 | ChatGPT Responses 用户消息必须把内部 `image_url` 转换为带 `detail=high` 的 `input_image`，与官方 Codex 请求兼容 | Rust ChatGPT transport | 失败优先单元测试断言字段和顺序 |
| CF-VISION-R2 | 图片与提问一起可被理解 | 混合消息必须保持 text-image-text 原始顺序 | Rust ChatGPT transport | 混合 content part 回归测试 |
| CF-VISION-R3 | 不破坏既有输入 | 纯文本请求体保持 `input_text`；空图片 URL 不得发送为 `input_image` | Rust ChatGPT transport | 兼容与失败断言 |
| CF-VISION-R4 | 发送后图片只显示为链接 | 用户消息气泡必须通过 Tauri asset URL 内嵌图片预览，不显示原始 `file://` Markdown | React 消息列表 | 用户角色回归测试 + 真实 Tauri 桌面验收 |

## Payload Harness

- Request type: ChatGPT Responses `POST /responses`
- Payload boundary: 单图片最大 8 MB，由既有附件提取层控制
- Expected request shape: `input[].content[]` 同时支持 `input_text` 和带 data URL 的 `input_image`
- Actual route selected: `ApiStyle::Chatgpt` → `call_chatgpt_model`
- Field-level assertions: `type=input_image`、`image_url=data:image/...;base64,...`、`detail=high`、混合 part 顺序不变
- Failure assertion: 空图片 URL 不进入 `input_image`；缺失、非图片或超限文件继续由既有附件层降级为文本

## Primary User Path

用户在 CodeFactory 中附加本地图片并输入问题，发送后用户气泡必须显示图片预览；选择 ChatGPT 订阅模型发送时，模型请求必须同时收到问题文本和图片像素数据，并能基于图片内容回答。
