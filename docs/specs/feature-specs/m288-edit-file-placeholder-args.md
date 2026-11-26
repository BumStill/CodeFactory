# M288：edit_file 空占位参数兼容

## 背景与现场

- 14 天内 7 个会话、559 次 `edit_file` 调用失败，错误均为：`Invalid arguments for edit_file: send either edits (batch) or old_string/new_string (single) — not both.`
- 两种真实失败参数形态：
  - 单次模式带空批量：`{"edits":[],"old_string":"pub mod journal;\n…","new_string":"…","path":"src-tauri/src/agent/mod.rs","replace_all":false}`
  - 批量模式带空单次：`{"edits":[{"old_string":"…","new_string":"…","replace_all":false}],"old_string":"","new_string":"","path":"…","replace_all":false}`
- 根因假设：工具 schema 的可选字段可能在端点适配时被当成必填，或模型主动补齐全部字段；参数解析把“存在但为空”的占位参数当成有效参数，错误判定为两种模式同时提供。
- 用户后果：模型无法使用单次或批量编辑，转而尝试 shell/heredoc 改源码并被审批阻断，形成会话停滞。

## Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| R1 | `edits` 为空数组时视为未提供；`edits` 非空且 `old_string`、`new_string` 都是空字符串时，视为未提供单次参数。单次模式中 `new_string` 为空（删除文本）仍合法。 | 两种现场参数形态通过 `edit_file` 真实执行入口正确修改文件；删除文本反例通过。 |
| R2 | 只有真正同时给出两组有效参数（非空 `edits` 且 `old_string` 非空）时才报互斥错误；错误信息写明收到的字段，并给出可直接照抄的正确形状。 | 混合有效参数用例断言错误字段和两种正确 JSON 形状；`edits:[]` 且无单次参数报缺参数。 |
| R3 | 查清 `edit_file` 工具 schema 是否以严格模式下发、是否把可选字段标成必填；若是，修正 schema（可选字段真正可选或用 `oneOf` 区分两种模式），并说明 chatgpt、openai、deepseek、openrouter 各端点的下发形态。 | 各端点实际 wire 构造测试断言 `edit_file` 可选字段未被强制必填，记录端点形态。 |
| R4 | 检查 `write_file`、apply 类其他编辑工具是否有同样的空占位误判；如有则按同规则处理，没有则写明已检查。 | 代码审查记录与相关测试；确认无双模式参数解析或完成对应修复。 |

## Applicable Harnesses

- **Spec Harness**：本规格、Req ID 映射、PR 自验表。
- **Compatibility Harness**：覆盖 ChatGPT Responses 与 OpenAI-compatible（OpenAI、DeepSeek、OpenRouter）下发形态。
- **Payload Harness**：通过真实 `edit_file` 执行入口验证参数解析、文件写入与错误路径。
- **AI Collaboration Harness**：记录上下文范围、假设、审查点与验证结果。

## 测试矩阵

| 场景 | 入口 | 期望 |
| --- | --- | --- |
| 单次模式携带 `edits:[]` | `edit_file::execute` | 忽略空批量占位并正确替换 |
| 批量模式携带空 `old_string`/`new_string` | `edit_file::execute` | 忽略空单次占位并原子完成批量替换 |
| 单次删除 | `edit_file::execute`，`new_string:""` | 删除成功，不误判为占位 |
| 两组有效参数 | `edit_file::execute` | 拒绝；列明收到字段和可复制的单次/批量 JSON 形状 |
| 只有 `edits:[]` | `edit_file::execute` | 报缺少有效编辑参数 |
| 各端点 schema | 实际 wire 构造函数 | `required` 仅含 `path`，不含可选模式字段；端点 schema 等价 |
| 其他编辑工具 | 定义与解析审查 | 无双模式空占位误判，或按相同规则修复 |

## 约束

- 改文件只用 `edit_file` / `write_file`。
- 不修改 `docs/testing/scenario-registry.json`；缺映射只追加 `docs/testing/scenario-mappings.json`。
- 不修改测试凑绿，不绕过门禁，不做超出 R1–R4 的改动。
- 构建目录与临时文件只放 `.codefactory-cache/`。

## 交付与验收

- PR 正文用 `R | 是否完成 | 证据` 列出 R1–R4。
- `Release-Urgency: immediate`，因为缺陷阻塞所有会话编辑。
- 需求未全部完成时 PR 保持草稿且不打 `merge-queue: armed`。
- 真机验收由编排方在发版安装后完成：新会话中让模型自然调用 `edit_file`，单次与批量各一次，均首次成功；检查当天 `tool_calls` 不再出现该互斥误报。

Release-Urgency: immediate
