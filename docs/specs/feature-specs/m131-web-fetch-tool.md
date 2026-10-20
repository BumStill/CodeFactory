# M131：轻量「读网页」工具 `web_fetch`

## 1. 问题

用户发来一个网址要求总结或引用时，模型只有两条路：

- `browser_session`：受管浏览器（或附着用户浏览器）打开 → 读 → 关闭，冷启动与页面渲染都算在等待里；
- `bash` + `curl`：拿回整页 HTML，再靠模型自己从标签里挑正文。

两条路都把一个"读一段文字"的请求变成昂贵或易错的流程。缺的是一个**一次 HTTP GET、返回可读正文**的工具。

## 2. Requirements Traceability

| # | 需求 | 实现 |
|---|---|---|
| R1 | 工具名 `web_fetch`，参数 `url`（必填）、`max_chars`（默认 8000，夹紧 400..40000）、`start`（默认 0 的分页偏移） | `src-tauri/src/tools/web_fetch.rs`：`definition()` / `Args` |
| R2 | 单次 GET：跟随重定向（上限 8）、20s 超时、8 MiB 响应体上限、自定义 UA；不发送任何 cookie，URL 中的用户名口令在请求前剥除 | `default_client()` / `read_capped()` / `validate_target()` |
| R3 | 安全边界：拒绝**字面**回环与链路本地/云元数据地址（`localhost`、`*.localhost`、`127.0.0.0/8`、`0.0.0.0`、`169.254.0.0/16`、`::1`、`fe80::/10`、IPv4-mapped 回环）；域名不做解析判定，代理沿用用户系统配置 | `validate_target()` |
| R4 | 正文提取：HTML → Markdown（标题层级、段落、列表、绝对化链接、代码块、表格；剥离 script/style/nav/header/footer/aside/form 等外壳）；JSON 格式化；PDF 复用既有字面串提取；纯文本原样 | `html_to_markdown()` / `classify()` / `crate::knowledge::pdf_text_from_bytes()` |
| R5 | 编码：`Content-Type` 的 `charset` → `<meta charset>` 嗅探 → UTF-8 → GBK 回退 | `charset_from_content_type()` / `sniff_charset()` / `decode_body()` |
| R6 | 结果结构：`Final URL` / `Title` / `Status` / `Content type` / 本次范围 / `Next page` 续读提示；正文始终包在 `<untrusted_page_content>` 数据边界内 | `execute_with()` + `crate::browser::as_untrusted_page_data()` |
| R7 | 明确回退：401/403、纯 JS 外壳、正文为空 → `is_error` 且**建议改用 `browser_session`**，并说明重试 `web_fetch` 无意义 | `execute_with()` 内的三条分支、`looks_js_only()` |
| R8 | 分页可续：`start` 从 `Next page` 行取；超出正文长度时返回说明而不是空串；切分点尽量落在空白/换行处 | `page_slice()` |
| R9 | 注册为只读工具：工具定义与分发、`ToolKind::ReadOnly`、默认 allow 列表、subagent 允许列表 | `tools/mod.rs`、`agent/tool_backend.rs`、`config/settings.rs`、`agent/scheduler.rs` |

## 3. Primary User Path

用户在聊天里粘贴一个 URL 并说"看下这篇写了什么" → 模型调用 `web_fetch {url}` → 一次 GET 后拿到标题、最终 URL 和 Markdown 正文（长文按 `Next page` 续读）→ 模型基于正文回答，引用链接为绝对地址。

失败子路径：需要登录 / 纯前端渲染的页面 → 工具返回明确错误与 `browser_session` 用法，模型不再重复 `web_fetch`。

## 4. Applicable Harnesses

| Harness | 覆盖 |
|---|---|
| Spec Harness | 本文档与 R1–R9 对照 |
| Compatibility Harness | 新增依赖 `encoding_rs`（reqwest 已在依赖树中），不改变任何既有工具签名；PDF 提取抽成 `pdf_text_from_bytes()` 后 `extract_pdf()` 行为不变 |
| Release Harness | 随常规按需发版；无迁移、无配置破坏 |
| Observation Harness | 工具错误信息自带 `Final URL` / HTTP 状态，便于从会话记录定位失败面 |
| Payload Harness | 响应体 8 MiB 上限 + `max_chars` 夹紧，避免把整站灌进上下文 |

## 5. 测试矩阵

`src-tauri/src/tools/web_fetch.rs` 内的单元测试用本地 fixture HTTP 服务器（`127.0.0.1:0` + 测试专用 `resolve("fixture.test", addr)`，不依赖外网、不污染生产客户端）：

| 用例 | 断言 |
|---|---|
| `html_article_becomes_readable_markdown` | 标题/二级标题/列表/绝对化相对链接/代码块/表格单元格保留；script、style、导航不外泄 |
| `body_is_labelled_as_untrusted_data` | 页面注入文案严格位于 `<untrusted_page_content>` 内 |
| `redirects_report_the_final_url` | 302 后 `Final URL` 为重定向终点，正文来自终点页 |
| `gbk_page_is_decoded` / `gbk_is_detected_from_a_meta_tag_when_the_header_omits_it` | GBK 正文与标题正确解码（含仅靠 meta 声明的页面） |
| `long_pages_paginate_and_resume` | 逐页续读能走完整篇，尾部标记可达，`start` 单调递增 |
| `start_past_the_end_is_reported_not_silently_empty` | 越界返回说明文本 |
| `pdf_is_text_extracted` | PDF 正文可读且不泄漏原始字节 |
| `js_only_page_points_at_the_browser_tool` / `login_walls_point_at_the_browser_tool` | `is_error` 且给出 `browser_session` 建议；403 不伪造页面数据边界 |
| `literal_local_hosts_are_rejected_without_any_dns_lookup` | 9 种字面本地/元数据地址在请求前被拒 |
| `ordinary_domains_are_not_ruled_out_by_address_guessing` | 普通域名与非 http(s) 协议的判定；域名字面量不做地址猜测（fake-IP DNS 下仍可读公网） |
| `a_rejected_host_never_reaches_the_network` | 拒绝发生在网络层之前 |
| `json_is_pretty_printed` | JSON 格式化输出 |

执行：`pnpm cargo:shared test --manifest-path src-tauri/Cargo.toml web_fetch`。

## 6. Evidence Pack Requirements

- 上述测试的实际运行输出（通过/失败数量）。
- 工具定义在 `all_definitions()` 中可见，`dispatch("web_fetch", …)` 可达。
- 只读分类证据：`desktop_command_and_kind("web_fetch") == ToolKind::ReadOnly`（默认 allow 列表包含 `web_fetch`）。

## 7. 兼容性与发布边界

- 不新增配置项；`encoding_rs` 为小体量纯 Rust 依赖。
- 既有 `extract_pdf()` 改为调用抽取出的 `pdf_text_from_bytes()`，输出保持一致。
- 工具仅做 HTTP 读取，不写入工作区、不执行脚本；页面正文按不可信数据处理。
- 本工具不替代 `browser_session`：交互、登录、JS 渲染仍走浏览器路径。
