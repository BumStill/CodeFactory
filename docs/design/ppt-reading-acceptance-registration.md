# PPT 读取验收登记与托管复验

原实现来源 f32e9ef1，d2c85e45 首次复验基于 main 120a0eed。2026-11-27 的复验基于发布版 v1.133.1（d3bb94f5）。普通追加映射保留发布版已有条目，补齐本次两个源文件的 HLT-001/VIS-001 归类。源文件、固定夹具和补充工作流沿用 HLT-001/VIS-001 的普通追加映射。正式可信 registry 尚未登记下列新目标。

| 覆盖点 | 已存在真实入口 | 当前证据/正式缺项 |
|---|---|---|
| OpenAI非空截断与空响应 | rust:ppt_output_openai_partial_and_empty_are_not_success | 本地合成协议；需可信registry登记 |
| Anthropic max_tokens拒绝 | rust:ppt_output_anthropic_partial_is_not_success | 生产解析器、本地HTTP合成事件；非真实账户 |
| ChatGPT Responses 终止状态 | rust:ppt_output_responses_incomplete_and_completed_use_real_sse_parser | 生产SSE解析；需独立目标登记 |
| 直连 GPT-6 Responses 结束状态 | ppt_output_direct_gpt6_rejects_incomplete_text_and_tool_calls / ppt_output_direct_gpt6_completed_preserves_text_tools_and_usage | 真实 /v1/responses 路由接本地 SSE；截断、未完结、EOF 与 [DONE] 不释放部分工具 |
| EOF、超时、取消区别 | rust:ppt_output_eof_timeout_and_cancel_keep_distinct_outcomes | 合成协议；不证明GUI投影 |
| partial不入缓存 | rust:nonempty_truncated_vision_is_bounded_and_never_cached | SQLite缓存及请求上限；不是失败费用账本 |
| 密集结构/副本编辑 | rust:dense_pptx_structure_reread_and_copy_edit_preserve_source | 201元素原生夹具，源文件保持不变 |
| 原生渲染及截断回退 | dense_pptx_real_render_truncation_then_structure_reread | #[ignore]，须显式--ignored及LibreOffice/poppler |
| 完整描述及后续答复 | dense_pptx_real_render_complete_description_then_structure_reread | #[ignore]，合成长文本/尾标记，真实handler与SQLite；非模型语义能力 |
| 第二次结构读取后空答复 | dense_pptx_real_render_empty_final_round_surfaces_error | #[ignore]，要求显示错误、保留工具结果且不重跑 |

## 普通 PR 的补充验收

`ppt-dense-acceptance.yml` 在 GitHub 托管 Windows、macOS 对 PR head checkout，核对 201 元素合成夹具的 SHA256，安装并记录原生渲染依赖版本。逐名运行三条 `--ignored` 渲染用例和两条直连 GPT-6 协议用例。每条必须取得一项实际通过的结果、日志哈希和精确 SHA 回执。失败也保留阶段证据。现有 CI 和可信门禁继续独立运行。

该工作流为补充验收，不构成可信 registry 的正式目标回执。要将这些目标纳入必过执行契约，需维护者按独立治理流程审查。现有 Rust 执行器不会执行 `#[ignore]` 用例，所以不能直接登记三个 Rust 名称充当覆盖。普通 PR 未修改可信文件、必过检查或保护。

合成提供方不会证明真实模型输出质量、Windows 桌面交互、PowerPoint 分组或应用实际费用。直连 GPT-6 的已有整段读取机制未新增空闲超时与中途取消。失败 helper 的持久用量仍待补齐。

helper失败Usage持久化仍缺失；本轮仅记录成功合成321/1800 token和失败不重试的边界，不宣称失败收费已核对。上线前需明确这项残余风险及相应修复/验收责任，不将合成值记为真实费用。
