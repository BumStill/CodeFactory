# ChatGPT 图片输入真实路径证据包（2026-07-17）

## 结论

`ApiStyle::Chatgpt` 的 Responses 请求已保留本地图片附件。隔离 Dev App 使用 `gpt-5.6-sol` 对用户提供的架构截图完成无工具视觉问答，准确读出仅存在于图片像素中的主标题和底部三个模块标题。

## 变更基线

- 基线提交：`b2fdd3ded516a2d21a2158b6d8dd9cece4cac991`（v1.47.0）
- 规格：`CF-VISION-R1` 至 `CF-VISION-R3`
- 修复路径：内部 `image_url` content part → ChatGPT Responses `input_image`
- 未改路径：OpenAI/OpenRouter Chat Completions、Anthropic Messages、既有 8 MB 附件上限

## 失败优先证据

修复前新增回归测试得到：

```text
left:  [{"type":"input_text","text":"先看截图\n再回答问题"}]
right: [{"type":"input_text","text":"先看截图"},
        {"type":"input_image","image_url":"data:image/png;base64,..."},
        {"type":"input_text","text":"再回答问题"}]
```

这证明旧实现会合并文本并完全丢弃中间图片。

## 自动化验证

- ChatGPT 图片转换定向测试：`4 passed`
  - text-image-text 顺序
  - 纯文本兼容
  - 空图片 URL 不进入请求
  - 本地 PNG 文件字节进入 data URL
- Rust lib：`353 passed, 0 failed, 6 ignored`
- 前端：`196 passed, 0 failed`
- TypeScript + Vite build：通过
- governance baseline：通过
- governance rules：`OK (4 rules)`
- `git diff --check`：通过
- 本机 `cargo fmt --check`：基线中存在大量与本次无关的既有格式漂移，因此没有批量改写无关 Rust 文件。

## 真实 Dev App 主路径

- App：`CodeFactoryVisionDev`，独立端口 `1426`
- 数据：独立临时 HOME 和 SQLite，不读写生产数据库
- 权限：沿用项目约定的 `full_access=true`，没有人工授权弹窗
- 模型：`chatgpt / gpt-5.6-sol`
- 会话：`6e2d18fa-deb3-4382-8ccf-da91565b946f`
- 原图与 App 附件均为 `403645` bytes
- 两者 SHA-256 均为 `1fcd4568724ba37a61ea2a527333f90eb02662fd38843b5b89f096037febd68d`
- 请求日志确认路由：`https://chatgpt.com/backend-api/codex`、模型 `gpt-5.6-sol`
- SQLite `tool_calls`：`0`，排除模型通过工具读取文件的可能

验收问题：

> 不要调用任何工具，也不要根据文件名猜测。只看这张图片并回答：图片最顶部的主标题是什么？最底部三个蓝色模块的标题从左到右分别是什么？

模型回答：

```text
最顶部主标题：自进化智能体：整体架构

1. Harness 优化
2. 知识库沉淀
3. Evals 回归
```

四项文字均与原图一致，证明用户可见的 Tauri 附件路径、ChatGPT Responses 请求和模型视觉识别端到端生效。
