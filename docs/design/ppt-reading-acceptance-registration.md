# PPT读取验收登记提案（未激活）

原实现来源 f32e9ef1，d2c85e45 首次复验基于 main 120a0eed。2026-11-27 的复验基于发布版 v1.133.1（d3bb94f5）。普通追加映射保留发布版已有条目，补齐本次两个源文件的 HLT-001/VIS-001 归类。映射本身不会登记下列新验收目标。

| 覆盖点 | 已存在真实入口 | 当前证据/正式缺项 |
|---|---|---|
| OpenAI非空截断与空响应 | rust:ppt_output_openai_partial_and_empty_are_not_success | 本地合成协议；需可信registry登记 |
| Anthropic max_tokens拒绝 | rust:ppt_output_anthropic_partial_is_not_success | 生产解析器、本地HTTP合成事件；非真实账户 |
| ChatGPT Responses 终止状态 | rust:ppt_output_responses_incomplete_and_completed_use_real_sse_parser | 生产SSE解析；需独立目标登记 |
| EOF、超时、取消区别 | rust:ppt_output_eof_timeout_and_cancel_keep_distinct_outcomes | 合成协议；不证明GUI投影 |
| partial不入缓存 | rust:nonempty_truncated_vision_is_bounded_and_never_cached | SQLite缓存及请求上限；不是失败费用账本 |
| 密集结构/副本编辑 | rust:dense_pptx_structure_reread_and_copy_edit_preserve_source | 201元素原生夹具，源文件保持不变 |
| 原生渲染及截断回退 | dense_pptx_real_render_truncation_then_structure_reread | #[ignore]，须显式--ignored及LibreOffice/poppler |
| 完整描述及后续答复 | dense_pptx_real_render_complete_description_then_structure_reread | #[ignore]，合成长文本/尾标记，真实handler与SQLite；非模型语义能力 |
| 第二次结构读取后空答复 | dense_pptx_real_render_empty_final_round_surfaces_error | #[ignore]，要求显示错误、保留工具结果且不重跑 |

## 维护者需审核的窄变更

普通Rust目标可按PPT读取/视觉/会话完成需求登记窄场景，保留现有全部targets与门禁。三个显式执行的原生目标不能只写rust:名称：默认cargo筛选会忽略，不能计通过。须选择现有治理支持的显式workflow/受支持入口，固定依赖版本、平台及--ignored --test-threads=1命令，保留逐项非零执行及真实回执；不得放宽发现器来接受零测试。该方案未应用registry、validator或workflow。

Windows真实验收依赖精确候选的已授权Windows环境/现有CI；云端LibreOffice验证不替代用户原Windows模型route。直连 GPT-6 Responses 解析器尚未接入此结束状态校验。真实视觉提供方还需合法配置、明确主对话与独立视觉路由、完整结束事件和用量账本。当前均未运行。PowerPoint组合/取消组合及保存重开另列，不能用结构对象计数或PNG渲染替代。

helper失败Usage持久化仍缺失；本轮仅记录成功合成321/1800 token和失败不重试的边界，不宣称失败收费已核对。上线前需明确这项残余风险及相应修复/验收责任，不将合成值记为真实费用。
