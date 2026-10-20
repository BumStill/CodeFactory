# M131c：web_fetch 对普通服务端渲染页面（维基百科）误判「无可读正文 / 需 JS」

## 证据（v1.91.4，10-21 06:5x）
- 会话 79e46bda：`web_fetch {url: https://en.wikipedia.org/wiki/Unix_philosophy}` → error「reached … but found no readable text: the page is rendered by JavaScript, is empty, or only serves a shell. Use browser_session …」。模型随后用 browser_session 读到正文（全程 15s）。
- 同一 URL 本机 `curl` 1.1s 200，HTML 190 KB，正文完整（服务端渲染）；响应头 `content-encoding: gzip`、`content-type: text/html; charset=UTF-8`、`vary: …User-Agent`。
- 页面副本：/Users/leo/.codefactory/agent-tools/fixtures/wikipedia-unix-philosophy.html（可直接复制进仓库 fixture）。
## 需求
- R1 定位根因并写进 PR：是否未解压 gzip/br/deflate（reqwest 需开启对应 feature 或手动解码）、User-Agent 导致返回不同内容、正文抽取规则漏掉 `<main>`/`#mw-content-text`/`.mw-parser-output` 等常见结构、或长度/字符集判断错误。
- R2 修复：正确解压；带合理的浏览器式 User-Agent 与 Accept 头；正文抽取对常见站点结构（维基、新闻、博客、GitHub README、文档站、中文站点）稳健；只有在确实只剩 JS 外壳时才返回「需浏览器」。
- R3 测试（先失败）：用上面的维基页面作为 fixture（经本地 fixture 服务器以 gzip 返回）→ 拿到标题与正文段落；再加 2–3 个代表性页面 fixture（中文 GBK 页面、文档站）；真 JS 外壳 fixture 仍返回「需浏览器」。
- R4 真机（PR 写实测）：在真实应用里读该维基 URL，工具调用只有 web_fetch（无 browser_session），回答正确。
## 约束
- CI 测试不访问外网（用 fixture）。授权交付（ceiling: merged），PR 标题写明内容，正文**必须**带 `Release-Urgency: immediate`。不要超出规格自加范围。

## Implementation Notes

- 按 task-template 合成数据约束，版本化 fixture 只保留维基副本中复现问题所需的标签结构，正文为合成内容；原始副本仅用于本机额外验证，不提交仓库。
- 真实安装版验收按模板末尾的职责补充由编排方发版安装后完成；本次不擅自发版。
- 使用当前应用管理的独立 worktree 和分支，不另建分支；不修改 trust roots。

## Requirements Traceability / 测试矩阵

| Req ID | 最低证据 |
| --- | --- |
| R1 | `m131c_wiki_headers_do_not_erase_main_content` 先失败；记录 head/header 前缀匹配导致正文截断的根因 |
| R2 | gzip/deflate/br 解压、请求头、短 SSR、标签精确匹配断言；既有新闻/博客 Markdown 用例 |
| R3 | 合成维基 gzip、中文 GBK、文档站、README 和真 JS shell 测试；外网不进入 CI |
| R4 | 本机原始副本提取断言；发版后编排方在真实应用只调用 web_fetch 读取指定 URL，核对标题与 Unix 哲学原则且没有 browser_session |

## Applicable Harnesses

Spec（本文 R1–R4）、Compatibility（无工具签名/持久化变更，压缩 feature）、Payload（解压后仍受 8 MiB 限制）、Observation（结构化工具输出及错误）、Release（PR/CI/合并及 immediate trailer，not live）、AI Collaboration（单流实现、自检，按模板禁用 delegate_tasks，无独立角色 sub-agent）。
