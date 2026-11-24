# PPT 专家洞察与规划能力补齐方案

> 状态：设计草案，供审查，**尚未实施、不代表已发布能力**。
>
> 适用范围：CodeFactory 桌面端会话、既有 Office 工具、知识库、Skill 运行时、文档预览与视觉桥。
>
> 当前实现基线：`origin/main@a0375c72e6f008f26a5be694450b8c86d19a1c40`（应用版本 1.122.0，已发版标签 v1.122.0）。本方案依据该提交的只读核查撰写。
>
> 交付边界：本文件只定义目标合同、阶段、接口草案和验收标准，不包含实现代码。任何实施必须另开实现 PR，并且遵守第 3 节「不影响现有功能」硬约束。
>
> 配套既有规格：
>
> - `docs/specs/feature-specs/personal-knowledge-office-assistant.md`（知识库、规划中的 `pptx_plan.json` / PowerPoint 插件）
> - `docs/specs/feature-specs/skill-lifecycle-system.md` 与 `docs/specs/feature-specs/skill-command-recovery.md`
> - `docs/specs/feature-specs/vision-bridge.md`
> - `docs/plans/chat-document-artifact-preview.md`
> - `docs/principles/design-docs-for-major-changes.md`

## 1. 背景与目标

CodeFactory 已经能在聊天里生成一份可在 PowerPoint、Keynote、Google Slides 和 LibreOffice Impress 打开的真实 `.pptx`，也能在用户上传的稿子上改字并尽量保留原设计。这解决了「模型只会输出 Markdown，用户还要手工转文件」的问题，但还远不是一个具备洞察和规划能力的高级 PPT 专家。

高级 PPT 专家的工作顺序不是先排版，而是先定义问题与受众，提炼一句可反驳的统领结论，用金字塔或 SCQA 把论证写成页级行动标题，再为每页取证，最后才进入母版、版式、原生图表和交付质检。审美服务于论证；文件必须是可编辑的原生对象，而不是拍平的图片墙。

本方案的产品目标是：把 CodeFactory 从「能出可打开的 PPT 的编码助手」推进到「在用户授权下，能完成洞察、规划、取证、出片和质检的高级 PPT 专家」。编码助手的主路径保持不变；PPT 专家能力通过 Skill、新的可选工具和可选质检门禁进入产品，而不是改写默认系统提示词。

成功时，用户可以明确回答下面这些问题：

1. 这份稿要说服谁、要回答哪一个决策问题、统领结论是什么？
2. 只读各页标题，能否连成一条完整论证？每页主张的证据来自哪里？
3. 文件是否沿用用户自己的主题、母版和版式，而不是一份固定的裸 Office 主题？
4. 数据页是否是可在 PowerPoint 里改数据的原生图表或表格，而不是截图或纯文字要点？
5. 交出前是否经过逐页视觉质检，且用户能在应用内翻页预览？

## 2. 现状证据

以下均为基线提交上的事实，不是推断。

### 2.1 已经具备的生成与编辑能力

- README 的 Documents 特性已公开承诺：可用 `read` / `edit` / `format` / `write_pptx` 处理 PowerPoint，可用 `write_docx`、`read_xlsx` / `edit_xlsx` 处理 Word 与 Excel；把 `.pptx` / `.docx` / `.pdf` / `.xlsx` 拖进输入框后智能体可以读取。见 `README.md`。
- `write_pptx` 定义与实现见 `src-tauri/src/tools/pptx.rs`。它用 `zip` crate 手写 OOXML，输入是 `path` 加 `slides[]`（`title`、`bullets`、`layout: content | section`、`notes`），每次最多 200 页。注释写明：一份幻灯片母版、一份「标题+正文」版式、固定 `theme1.xml`，不支持主题定制，也不生成图片、形状或图表。文件头注释仍写「暂不支持演讲者备注」，但同一文件已经实现 `notes` 部件；实施时不得再把「备注未实现」当成现状。
- `read_pptx`、`edit_pptx`、`format_pptx` 见 `src-tauri/src/tools/pptx_edit.rs`。`read_pptx` 以 `sN.F.P` 定位段落，可选 `with_format` 与 `check_overflow`。`edit_pptx` 只支持 `replace` 与 `insert_after`，通过按字节拷贝主题、媒体和母版来保留设计。`format_pptx` 只改文本格式（字体、字号、颜色、粗体、斜体、对齐、段间距），不移动或缩放形状；应用规则后会做溢出估算，并写明「没有渲染引擎，只是近似」。
- 工具注册见 `src-tauri/src/tools/mod.rs`。默认权限策略把这些工具列入可读或可写集合，见 `src-tauri/src/agent/mod.rs` 的 `permission_policy_for_mode`。
- 没有在 `src-tauri/Cargo.toml` 或前端依赖中找到 `python-pptx` 或 `pptxgenjs`。规格里曾把 PptxGenJS 写成后续渲染方案，见 `docs/specs/feature-specs/personal-knowledge-office-assistant.md`，当前并未落地。仓库中也没有 Reveal.js、Marp 或 HTML 幻灯片实现。

### 2.2 技能与规划流程

- 源码中有完整技能包 `src-tauri/resources/skills/deck-from-references/`（`manifest.json`、`system_prompt.md`、`slash_commands.json`、`tool_policy.json`）。提示词要求确认交付物、受众、角度和页数，先写大纲再生成，一页一个想法，章节分隔页，深度放进备注，矛盾要在大纲里暴露，并提供 enrich / beautify / summarize 管道。斜杠命令包括 `/deck`、`/report`、`/outline`、`/enrich`、`/beautify`、`/summarize`。
- 该技能没有进入发版打包清单。`src-tauri/tauri.conf.json` 的 `bundle.resources` 只列出 `react-expert`、`code-reviewer`、`git-workflow`。`src-tauri/src/commands/skills.rs` 的内置市场目录里也没有 `deck-from-references`。因此，就打包配置而言，安装版不会自带这份工作流。本方案未拆开 v1.122.0 安装包做二次验证。
- 默认系统提示词把自身定义为桌面端 AI 编码助手，主循环是读改代码、TDD 和 `deliver_changes`，见 `src-tauri/src/agent/mod.rs` 中的 `SYSTEM_PROMPT`。仓库里没有金字塔、SCQA、MECE 或行动标题的产品提示词。
- `update_plan`（`src-tauri/src/tools/update_plan.rs`）最多 8 步，种类是 `analysis` / `implementation` / `verification` / `delivery` 等，用于展示执行路线，不是幻灯片故事线。
- 既有规格规划过 `pptx_plan.json`、`ppt_generate_plan`、`ppt_render`，以及 `kb_get_document_outline`、`kb_get_deck_profile`、`kb_retrieve_slide_examples`，见 `docs/specs/feature-specs/personal-knowledge-office-assistant.md`。这些工具和 schema 在基线源码中均未实现。已实现的知识库工具只有 `kb_search` 与 `kb_get_chunk`（`src-tauri/src/tools/knowledge.rs`）。

### 2.3 知识库、预览与质检

- 知识库可以扫描并抽取 `.pptx` / `.docx` / `.pdf` 的纯文本，chunk 带路径和页码或幻灯片号，见 `src-tauri/src/knowledge.rs` 的 `extract_pptx`。抽取不保留图表、图片和版式。`kb_search` 返回带来源字段的片段，并写 `retrieval_events`。
- 聊天里的 `FileArtifactCard`（`src/components/FileArtifactCard.tsx`、`src/components/ToolCallCard.tsx`）可为生成的 `.pptx` 提供「系统打开」。`DocumentPreview` 经 `read_document`（`src-tauri/src/commands/document.rs`）把文件字节按 UTF-8 有损解码后显示；对 zip 格式的 `.pptx` 得不到逐页幻灯片预览。`docs/plans/chat-document-artifact-preview.md` 已规划「首版用文本提取，后续再补页面级渲染」，页面级渲染尚未落地。
- `view_image`（`src-tauri/src/tools/view_image.rs`）配合 `src-tauri/src/vision/render.rs`，可以把 PPTX 或 PDF 的某一页渲成 PNG，再交给视觉端点或本地 OCR。渲染路径是：Windows 上的 PowerPoint COM、全平台 LibreOffice (`soffice`) 转 PDF 再加 `pdftoppm`，以及 macOS QuickLook（对 PPT 基本上只保证第 1 页）。缺少这些依赖时工具会失败并列出可用渲染器。
- 子代理验证引擎（`src-tauri/src/agent/verification.rs`）检测的是 `cargo test`、`pnpm lint`、文件是否存在等代码检查，不是幻灯片质检。技能里的自检主要是再读文本、确认文件大约大于 5KB。
- `read_xlsx` / `edit_xlsx`（`src-tauri/src/tools/xlsx.rs`）只读写单元格值；`edit_xlsx` 重建工作表时不保留样式和实时公式，也没有图表 API。

### 2.4 可复用而不必重做的底座

任务分派（`delegate_tasks`）、工具调用与权限审批、Seatbelt / Docker 沙箱中的 `bash`、`web_fetch` 与受管浏览器、`update_plan`、会话记忆、evidence pack，以及面向代码交付的 `deliver_changes` / 合并队列，都已经存在。本方案把它们当作 PPT 流水线的运行时，而不是再造一套智能体。合并队列仍只服务于代码合入，不拿来当出片流程。

## 3. 非目标与「不影响现有功能」硬约束

这是 Leo 的硬约束，也是本方案一切接口选择的前提。实施 PR 若无法证明遵守本节，不得合并。

### 3.1 明确不在本方案范围

- 不重写默认编码助手系统提示词，不把所有会话变成 PPT 专家。
- 不在本方案内实施 PowerPoint Web Add-in、本地 HTTPS pairing 或 `insertSlidesFromBase64`（仍以既有规格 `CF-PPT-R1` 至 `CF-PPT-R3` 为准，另开专项）。
- 不引入云端知识库、团队空间或品牌资产云同步。
- 不把 HTML 幻灯片或 PDF 导出作为主交付格式；主交付仍是可编辑的 `.pptx`。
- 不修改 `scenario-gate`、分支规则集、发版签名环境或合并队列策略。
- 本设计 PR 只落地文档；实现按阶段另开 PR，每阶段不得搭车修改无关模块。

### 3.2 兼容性硬约束（规范性）

1. **既有工具签名保持向后兼容。** `write_pptx`、`read_pptx`、`edit_pptx`、`format_pptx`、`write_docx`、`read_xlsx`、`edit_xlsx`、`kb_search`、`kb_get_chunk`、`view_image`、`read_file`、`read_document` 的已有必填字段、默认行为和错误语义不得破坏。新能力只能以可选字段、可选参数或新工具名出现。`write_pptx` 继续接受今天的 `slides: [{ title, bullets, layout, notes }]`，并且在不传新字段时产出与今天同类的「标题+要点」稿（允许主题 XML 的等价修订，但不允许调用方观察到必填参数或默认 layout 语义变化）。
2. **既有编辑语义保持。** `edit_pptx` 的 `replace` / `insert_after` 仍只改文本并保留主题、媒体与母版。`format_pptx` 仍不得移动或缩放形状。新增的增页、表格、图表、图片必须是新的 `op` 或新工具，缺省不触发。
3. **默认权限不收紧编码路径。** 不得为了 PPT 质检而让普通 `write_file` / `edit_file` / `bash` 突然多一道审批或失败。视觉质检只挂在 PPT 出片与明确的质检工具上。
4. **默认系统提示词的编码合同不变。** TDD、交付、审批和「不要再确认已批准计划」等条款保持原意。PPT 方法论只通过启用的 Skill、斜杠命令或用户明确的做稿意图进入上下文。
5. **已打包技能不受影响。** 把 `deck-from-references` 加入 `bundle.resources` 时，不得删除、重命名或默认禁用 `react-expert`、`code-reviewer`、`git-workflow`。技能加载失败必须隔离在该技能，不能让资源中心或会话启动失败。
6. **知识库与检索行为兼容。** 可以新增表和工具，但既有 `knowledge_*` 表、`kb_search` / `kb_get_chunk` 的返回字段和权限范围必须保持可读兼容。迁移必须幂等，并覆盖「无 PPT 文档的已有库再扫描不丢 chunk」的回归。
7. **预览降级必须保守。** 改进 `.pptx` 预览时，`.md` / `.txt` / 源代码的现有预览路径保持不变。渲染器缺失时，预览可以退回「文本摘要 + 系统打开」，但不得把 zip 二进制垃圾当作成功预览，也不得让预览命令在非 Office 文件上改行为。
8. **渲染依赖缺失不得破坏主路径。** 没有 PowerPoint、LibreOffice 或 poppler 时，生成、编辑、格式化、知识库检索和编码任务必须仍然可用。受影响的只是逐页视觉质检和应用内页面预览，并且必须给出可行动的降级说明，而不是让出片工具返回失败。
9. **仓库治理边界。** 实现不得修改 ruleset、`scenario-gate` 工作流或信任根文件，除非另有 Leo 的书面批准。本设计 PR 尤其不得触碰这些文件。

回归最低标准（每个实现阶段都要跑，并在该阶段 PR 中列出命令）：既有 `pptx` / `pptx_edit` / `xlsx` / `docx` / `knowledge` / `view_image` / `document` 相关测试保持通过；前端中与 `write_pptx`、`FileArtifactCard`、`DocumentPreview` 相关的测试保持通过。若某阶段不得不改测试，必须在 PR 中说明那是断言扩容而不是放宽既有合同。

## 4. 能力模型

把高级 PPT 专家拆成七项可独立验收的能力。前一项的输出是后一项的输入；实现可以按阶段交付，但产品叙事上不得跳过规划直接出精美空壳。

| 能力 | 专家标准 | 本方案的产品落点 |
| --- | --- | --- |
| 问题定义与受众洞察 | 先确认受众、场合、决策问题和成功标准 | Skill 强制澄清；规划对象记录 audience、decision_question、success_criteria |
| 结论先行的故事线 | 一句可反驳的统领结论；页标题是行动句；横向逻辑可连读 | `pptx_plan.json` 中的 `governing_thought` 与按顺序排列的 `slides[].action_title` |
| 调研与证据 | 每条主张有出处，矛盾要暴露 | 规划对象的 `source_refs`；知识库与 `web_fetch` 的引用闭环 |
| 数据可视化 | 用正确的图种表达比较、构成或趋势 | 原生图表与表格操作，数据可追溯到单元格或表 |
| 版式与母版 / 品牌 | 主题色、主题字体、母版和版式统一 | `deck_profile` 与按版式增页；新建稿可套用参考稿或品牌设定 |
| 可编辑原生对象 | 文本、表、图都是 Office 对象，不拍平 | 生成与编辑工具只写 OOXML 对象；质检拒绝「整页图片冒充内容」 |
| 交付质检与迭代 | 逐页看、按清单改、按反馈改结构 | 渲染 QA 门禁、应用内翻页预览、保留设计的结构编辑 |

`update_plan` 继续只跟踪「澄清 / 检索 / 写计划 / 出片 / 质检」这类执行步骤。故事线本身必须是独立的计划文件或工具对象，不能塞进最多 8 步的执行路线。

## 5. 与既有规格的关系

- 本方案**细化并落地** `personal-knowledge-office-assistant.md` 中已写但未实现的 CF-KB-R3（先产出带引用的 outline 和 `pptx_plan.json`，再生成 PPTX）。若两份文件对计划字段有出入，以本方案第 6.2 节的 schema 为准，并在实施 PR 中同步修订那份规格的对应表格，避免双源。
- 本方案**暂不实施**同一规格中的 PowerPoint 插件主路径（P-KB-PPT-1 的插件插入段）。规划、渲染和 QA 的产出应让未来的插件可以直接消费 `pptx_plan.json` 和 PPTX 字节，但不以插件为第一阶段的验收条件。
- 既有规格曾建议「Node sidecar + PptxGenJS，后续再评估 Rust/OOXML」。基线已经用 Rust 手写 OOXML 交付了最小可用生成器。本方案的默认路径是：**在现有 Rust/OOXML 工具上做加法**，只有当原生图表或母版套用在现有打包器上无法在合理复杂度内做对时，才评估引入成熟库。引入新库属于待决问题，必须单独立项，且新渲染路径要能消费同一份 `pptx_plan.json`，不得分裂合同。
- Skill 的打包、审核和按需加载遵循 `skill-lifecycle-system.md`：远程或新导入技能默认不自动扩大权限；`deck-from-references` 作为官方内置包，启用状态仍要在资源中心可见。

## 6. 分阶段方案

阶段编号即推荐实施顺序。阶段 0 可单独先合，风险最低。后面阶段可以准备设计，但不得在前一阶段的合同和回归未通过时把破坏性渲染改动一并塞入。

### 6.1 阶段 0：技能打包与洞察规划流程

**目标。** 安装版能够发现并启用「从参考文档做 PPT」技能；技能提示词达到「先洞察、再规划、再出片」的专家工作顺序；用户不启用该技能时，编码会话行为与今天一致。

**改哪些模块。**

- `src-tauri/tauri.conf.json` 的 `bundle.resources`：补上 `deck-from-references` 的 `manifest.json`、`system_prompt.md`、`slash_commands.json`、`tool_policy.json`，与现有三个技能并列。
- 可选：把该技能加入 `src-tauri/src/commands/skills.rs` 的内置市场目录，便于在资源中心搜索。若加入，目录条目必须与磁盘包内容一致，不得再内嵌一份会漂移的精简提示词。
- 升级 `src-tauri/resources/skills/deck-from-references/system_prompt.md` 与 `slash_commands.json`。提示词必须要求：书面确认受众、场合、决策问题；给出一句可反驳的统领结论；用 SCQA 或金字塔组织章节；每页标题是完整判断（行动标题），而不是主题词；只读标题应能连成论证；每页 3 到 5 条要点，深度放进备注；列出证据与假设；参考互相矛盾时停止并在大纲中暴露；除非用户明确说「直接做」，否则大纲必须先得到用户批准。保留现有的 enrich / beautify / summarize 管道和「不要用 `write_pptx` 覆盖用户上传稿」的硬规则。
- 不改 `SYSTEM_PROMPT`。

**接口。** 本阶段不新增 Agent 工具。斜杠命令保持 `/deck`、`/outline` 等名称，只升级模板用语。

**验收与测试。**

- 单元或集成测试：官方资源目录扫描结果包含 `deck-from-references`；模拟打包资源布局时，该技能与原有三个技能同时可见。
- 契约测试：提示词包含受众、决策问题、统领结论、行动标题、先批准再生成等必需要素（可用稳定子串或清单断言，避免把整份提示词锁死）。
- 负向：故意不把该技能放入测试夹具时，资源中心仍只列出夹具中的技能，且不崩溃。
- 人工验收：在启用技能的会话里用 `/outline` 给一个含混需求，助手应先提问或给出带统领结论的大纲，而不是直接 `write_pptx`；在未启用该技能的编码会话里，既有欢迎路径与代码修改行为不变。
- 本阶段不要求故事线 schema 已落地；若模型把大纲写进普通 Markdown 文件，可以接受。

**风险与降级。** 打包路径写错会导致安装包变大或技能不可见。必须在 CI 或构建测试中断言资源文件存在。提示词变长可能挤占上下文：默认仍按需加载，不得改为注入每一个会话。若官方技能与用户同 id 覆盖规则与今天一致（用户技能覆盖内置），不得为这个技能开特例。

**对现有功能的约束。** 满足第 3.2 节第 4、5 条。既有 enrich / beautify / summarize 命令名和「保留设计、只改文本」的承诺不得弱化。

### 6.2 阶段 1：故事线与大纲数据合同（`pptx_plan.json`）

**目标。** 出片前有一份可校验的计划对象。它既是给用户审阅的故事线，也是后续渲染、引用和质检的输入。执行路线仍然走 `update_plan`。

**改哪些模块。**

- 新增计划 schema 与校验库（建议放在 `src-tauri/src/tools/` 下的独立模块，或与 pptx 工具为邻的 `pptx_plan.rs`），供工具和测试共用。
- 新增工具 `ppt_validate_plan`（只读校验并回显摘要）。是否另设 `ppt_generate_plan` 见第 14 节：推荐先让模型用 `write_file` 写出计划、再用 `ppt_validate_plan` 门禁，以减少与既有文件工具的重叠。
- 阶段 2 及以后的渲染入口消费该计划；本阶段的 `write_pptx` 可以继续按旧参数工作。允许增加**可选**参数 `plan_path`：一旦提供，工具先校验计划再生成，且 `slides` 必须与计划一致或省略。不传 `plan_path` 时行为与今天完全一致。
- Skill 提示词改为：批准后先写计划文件，再生成。

**schema 草案（`pptx_plan.json`，版本 `1`）。**

```json
{
  "schema_version": 1,
  "language": "zh-CN",
  "audience": {
    "who": "公司管理层",
    "occasion": "季度业务回顾",
    "decision_question": "是否在下季度把资源集中到两个区域市场？",
    "success_criteria": "读完执行摘要后能回答结论、理由和下一步行动"
  },
  "governing_thought": "下季度应把新增投放集中到华东和西南，因为增量的七成来自这两区且交付能力已经就绪。",
  "narrative": {
    "pattern": "scqa",
    "situation": "过去四个季度全国铺开投放。",
    "complication": "增量高度集中，其余区域获客成本上升。",
    "question": "下季度资源应如何取舍？",
    "answer": "与 governing_thought 相同"
  },
  "slides": [
    {
      "id": "s1",
      "role": "exec_summary",
      "action_title": "把新增投放集中到华东和西南，预计覆盖增量的七成",
      "layout_hint": "title_content",
      "objective": "让决策者在一页内看到结论、理由和行动",
      "bullets": ["...", "..."],
      "notes": "口头展开的证据与限定条件",
      "source_refs": [
        {
          "claim": "增量的七成来自这两区",
          "document_path": "knowledge/q3-review.pptx",
          "slide": 6,
          "chunk_id": "optional-chunk-id",
          "quote_or_data": "简短出处说明"
        }
      ],
      "data_needs": [],
      "open_questions": []
    }
  ],
  "assumptions": ["交付能力在西南已经就绪，依据内部备忘，未经财务复验"],
  "contradictions": [],
  "appendix_slides": []
}
```

字段约束草案：`schema_version` 必填且为整数；`governing_thought` 必填，必须是可反驳的完整句，不能只是主题词；`slides` 至少一页；每一页必须有 `action_title` 和 `role`（`exec_summary` / `section` / `argument` / `evidence` / `appendix` 等枚举）；`source_refs` 在 `role` 为 `argument` 或 `evidence` 的页上建议必填，缺失时校验器给警告而不是硬失败，以免无知识库的用户无法出片；`narrative.pattern` 取 `scqa` / `pyramid` / `other`。页数上限与 `write_pptx` 一致，为 200。

校验器输出：错误（缺统领结论、空标题、schema 无法解析）导致失败；警告（某页没有出处、标题不像完整句、正文要点超过 5 条、没有执行摘要）不阻止本阶段生成，但要写入工具结果，并在阶段 4 的质检中默认升级为必须处理的项（可通过计划或调用参数显式豁免）。

**验收与测试。**

- 合法计划通过；缺 `governing_thought`、空 `slides`、错误 `schema_version` 失败。
- `write_pptx` 不传 `plan_path` 的既有测试全部通过，字节级合同以现有测试为准。
- 传入合法 `plan_path` 时，生成页数和标题与计划一致。
- 传入与 `slides` 参数互相矛盾的计划时失败，不得静默以其中一方为准。
- 技能或文档示例含一份最小合法计划和一份应失败的计划。

**风险与降级。** 模型可能写出像主题词的标题。本阶段只用启发式警告，不把「像不像行动标题」做成不可复现的硬门禁。计划文件应落在工作区内，遵循既有可写路径与锁写规则。

### 6.3 阶段 2：主题、母版与 deck profile

**目标。** 新建稿可以套用参考 PPT 或已保存的品牌设定，而不是永远使用固定的裸 Office 主题。编辑已有稿时继续保留其设计，并允许按该稿的版式增页（增页的结构操作在阶段 3 完成，本阶段先能读取和表达品牌）。

**改哪些模块。**

- 知识库增加 `deck_profiles` 表（字段与既有规格对齐：`document_id`、`theme_json`、`fonts_json`、`colors_json`、`layout_summary_json`、`thumbnail_refs_json`，另加 `profile_version` 与抽取时间）。扫描 `.pptx` 时增量写入；失败不得阻断文本 chunk 抽取。
- 新工具 `kb_get_deck_profile`：按 `document_id` 或路径返回主题色、主题字体、母版名称、版式列表（token 与占位符摘要），以及抽样页信息。权限默认与 `kb_search` 相同（allow），并记录审计字段：`document_id`、`slides_sampled`、`profile_version`。
- 可选设置或工作区文件保存「默认品牌稿」路径，供技能在新建时使用。
- `write_pptx` 增加可选参数，例如 `theme_source`（现有 `.pptx` 路径或 profile id）。提供时，从源包复制 `ppt/theme/*`、`ppt/slideMasters/*`、`ppt/slideLayouts/*` 及其关系部件；不提供时仍写今天的默认主题。复制必须校验 OOXML 关系完整性，失败则返回明确错误并保持输出未被部分写入（沿用现有原子写）。

**验收与测试。**

- 对夹具 PPT（含自定义主题色和至少两份版式）抽取的 profile 能断言主强调色、字体和版式名称。
- 无主题的损坏 zip 不得让整库扫描失败。
- `write_pptx` 不带 `theme_source` 的输出仍通过现有打开与结构测试。
- 带 `theme_source` 时，输出包中的 theme 与源包一致或等价，且仍能被现有 zip/OOXML 读取逻辑打开。
- `kb_search` 在只含 Word/PDF 的库上行为不变。

**风险与降级。** 复制母版时若漏掉关系或主题字体，Office 可能修复或报损。必须用真实夹具在至少一种 Office 或 LibreOffice 上做打开冒烟（可在有依赖的 CI 作业中跑，作业失败不得改为跳过即成功，除非明确标记为可选手工项）。抽到的颜色若来自幻灯片硬编码而不是主题，应在 profile 中区分来源，避免把一次例外配色当成品牌。

### 6.4 阶段 3：原生表格、图表、图片与按版式增页

**目标。** 在保留设计的前提下，智能体可以按指定版式新增一页，并插入可在 PowerPoint 中编辑的表格、图表和图片，而不是把内容画成图。

**改哪些模块。**

- 扩展 `edit_pptx` 的 `op` 枚举（加法）：至少包括 `insert_slide`、`add_table`、`add_chart`、`add_image`。旧操作保持不变。也可以把结构操作放到新工具 `edit_pptx_structure`，以避免复杂参数撑破现有编辑入口；若走新工具，Skill 必须明确何时用哪个，旧工具合同不变。
- `insert_slide` 草案：`after_slide`（0 表示插到最前）、`layout`（版式名或索引，来自 deck profile 或当前包的 `slideLayouts`）、可选 `from_plan_id`。新页必须引用目标包已有的版式与母版，不得偷偷改写成默认母版。
- `add_table` 草案：目标页、位置（占位符名或 EMU 坐标）、`rows` 二维字符串或数字、可选列宽。写出 OOXML 表格，而不是一组文本框。
- `add_chart` 草案：图种枚举先从 `bar`、`column`、`line`、`pie`、`stacked_bar` 做起；`categories` 与 `series`；可选 `data_source`（xlsx 路径加区域）。图表必须是 Graphic Frame + chart part，而不是预渲染 PNG。
- `add_image` 草案：工作区内图片路径、目标页与占位符或坐标、可选锁定纵横比。图片进入 `ppt/media/` 并建立关系。不得用「整页截图」作为默认出片方式。
- `write_pptx` 若开始消费 `pptx_plan.json` 中的 `data_needs`，应在同一阶段调用上述原生对象能力，而不是先出纯文字稿再靠模型手工拼。

**验收与测试。**

- 对仅含 `replace` / `insert_after` 的既有测试零失败。
- `insert_slide` 后，`read_pptx` 看到新页；主题与原母版仍在；幻灯片关系连续。
- 夹具中的表格和图表部件名称、缓存数据和系列数可被测试断言；用 zip 列出 chart XML 存在。
- 非法图种、空系列、缺图片文件、越界 `after_slide` 返回明确错误，源文件在 `out_path` 不同于 `path` 时不被修改。
- 质检辅助断言：若某页唯一内容是铺满的图片且没有文本框或图表部件，阶段 4 应警告「可能拍平」。

**风险与降级。** 原生图表是 OOXML 中最复杂的部分之一，这是本方案技术风险最高的阶段。若第一轮无法覆盖全部图种，必须保持已承诺图种的正确性，而不是用图片冒充其余图种。做不到的图种应明确失败并建议降级为表格，而不是静默插一张图。Windows 与 macOS 上的打开冒烟至少覆盖一种图表和一种表格。

### 6.5 阶段 4：逐页渲染视觉质检门禁

**目标。** 出片或格式化之后，可以用页面渲染和确定性规则做交付质检；不过关则返回结构化问题列表，而不是只靠「文件大于 5KB」。

**改哪些模块。**

- 新增工具 `ppt_qa`（名称可调整）：输入 `path`、可选 `plan_path`、可选页码范围。先做不依赖渲染器的确定性检查，再在渲染器可用时做视觉检查。
- 复用 `src-tauri/src/vision/render.rs` 的 `render_page`，不得再复制一套转换逻辑。渲染结果写入会话或工作区的 QA 目录，并在工具结果中给出路径。
- 可选：Skill 规定在 `write_pptx` / `format_pptx` / 结构性 `edit_pptx` 成功后必须调用 `ppt_qa`。不要把 QA 偷偷塞进这些工具的默认成功路径，以免无渲染器环境把今天已经成功的出片变成失败（第 3.2 节第 8 条）。
- 不要把代码用的 `verification.rs` 计划默认改成跑 `ppt_qa`。若要在子代理里做 PPT 验收，应显式写 custom check 或专门的 VerificationCheck 变体，且仅用于 PPT 任务。

**检查项草案。**

确定性（无渲染器也必须跑）：计划与成品页数是否一致；行动标题是否原样出现；`argument` / `evidence` 页是否仍缺 `source_refs`；`format` 溢出估算是否仍告警；是否存在无文本、无表格、无图表的空白页；是否出现整页图片而没有其它原生对象。

视觉（渲染器可用时）：逐页 PNG 可生成；可选地把页面交给 `view_image` 或视觉端点，询问「3 秒内能否读出标题主张、是否明显溢出或对比不足」，模型判断只能作为警告，不能作为唯一失败条件。对比度和溢出应以确定性或近似几何规则为主。

工具结果草案：`status: pass | pass_with_warnings | fail`，`issues[]`（`code`、`slide`、`severity`、`message`），`renderer`（名称或 `none`），`pages_rendered`。

**验收与测试。**

- 无渲染器时，确定性检查仍运行，`renderer` 为 `none`，不因缺失依赖而 `fail`，除非调用方显式要求 `require_render: true`。
- 有意构造的溢出夹具（可复用 `read_pptx` 已有溢出测试思路）在 QA 中出现对应 issue。
- 计划写 3 页、成品 2 页时 `fail`。
- `write_pptx` / `format_pptx` 在不调用 `ppt_qa` 时的成功语义与今天一致。
- 有渲染器的作业（可与后续预览共用）对一份最小稿断言第 1 页和第 2 页 PNG 存在且非空。

**风险与降级。** 渲染器在 CI 镜像中可能不存在，因此测试必须分成「无依赖确定性」和「有依赖可选作业」两层；可选作业若未安装依赖应显示 skipped 或 job-level skip，不得标记为成功的产品测试。视觉模型的主观判断不得单独阻断交付。QuickLook 仅第 1 页的限制必须在工具文档和结果中写明，避免把第 2 页起的失败误报成内容错误。

### 6.6 阶段 5：应用内逐页预览

**目标。** 用户在会话里点击生成的或附件中的 `.pptx`，可以在应用内翻页看页面图，并继续用系统应用打开以编辑母版或动画。

**改哪些模块。**

- `src-tauri/src/commands/document.rs` 的 `read_document`：**不得**改变文本文件的返回形状。建议新增命令，例如 `preview_office_document`，输入 `cwd`、`path`、`page`，返回 PNG 字节或受控临时路径、页数、渲染器名称、降级原因。旧 `read_document` 对 `.pptx` 的 zip 字节预览可以在新命令可用后，于前端改为走新命令；若仍被直接调用，应避免把二进制当作用户可读成功内容（可返回明确错误，请改用新命令），但要评估是否有其它调用方依赖旧行为。
- 前端 `DocumentPreview` / `FileArtifactCard`：对 `.pptx`（可选同样处理 `.pdf`，若与现有 PDF 打算一致）提供翻页、页码、系统打开、复制路径。渲染中和失败态要可访问（`aria-label`、键盘）。
- 复用阶段 4 的渲染缓存，避免每次翻页都完整跑一遍 LibreOffice。

**验收与测试。**

- 文本和 Markdown 预览测试保持通过。
- 前端测试：对 `.pptx` 显示翻页控件；渲染失败时显示可理解文案和「系统打开」；不把乱码文本当作正文。
- 后端测试：工作区外路径继续拒绝；非 pptx/pdf 调用新命令时返回明确错误；页码越界失败。
- 有渲染器环境下的手工或集成冒烟：打开一份两页夹具，能看到两页不同的图。

**风险与降级。** 无渲染器时显示文本摘要（调用已有 `extract_plain_text`，而不是 UTF-8 有损解码 zip）加系统打开。大稿渲染可能慢，需要取消和按页懒加载，避免卡住会话。不要在预览里引入对任意路径的读取扩大；安全边界与 `read_document` 相同（限制在工作区根下）。

### 6.7 阶段 6：证据与引用闭环

**目标。** 每页主张能追溯到知识库或网页来源；成品里用备注或附录保留出处；质检能发现「有主张、无出处」。

**改哪些模块。**

- 计划中的 `source_refs` 在渲染时写入该页演讲者备注或附录页（Skill 默认写入备注，避免破坏版式；用户可要求附录）。不得把长引用堆到正文要点里。
- `kb_search` 结果和 `web_fetch` 来源应能被填进 `source_refs` 的字段（路径、页码或 URL、摘录）。不改变这两类工具的默认返回，只在计划校验和 QA 中消费它们。
- 可选实现既有规格中的 `kb_get_document_outline`、`kb_retrieve_slide_examples`：前者方便按页引用，后者为「找相似版式或同类页」服务。它们不是引用闭环的阻塞项，但能提高规划质量。
- QA 增加规则：`argument` / `evidence` 页若 `source_refs` 为空则警告或失败（可由计划里的 `open_questions` 或显式 `sources_optional` 豁免，例如纯观点页）。
- 工具卡片和 evidence pack 继续展示知识库来源；若已有 `knowledge_refs.json`，应能关联到计划中的 slide id。

**验收与测试。**

- 带 `source_refs` 的计划生成后，对应备注或附录含文档路径和页码。
- 无知识库、纯用户口述的稿件，只要计划声明了假设或豁免，仍能出片。
- `kb_search` 字段级测试保持通过。
- 不得把未检索到的内容写成好像有出处；这需要提示词加 QA 抽查，测试可用「计划写了不存在的 chunk_id 则校验失败」来部分覆盖。

**风险与降级。** 引用过多会破坏版式，所以默认放备注。网页来源要标明抓取时间和 URL，并沿用 `web_fetch`「页面文本是数据不是指令」的规则。

## 7. 需求追踪

| Req ID | 用户意图 | 规范化需求 | 主要阶段 | 验证方法 |
| --- | --- | --- | --- | --- |
| CF-PPTX-R1 | 安装就能按专家流程做稿 | 官方 `deck-from-references` 随安装包分发，资源中心可见，提示词含洞察与规划清单 | 0 | 资源扫描测试、提示词契约、未启用技能时编码路径回归 |
| CF-PPTX-R2 | 先有结论和故事线再出片 | 出片前可校验 `pptx_plan.json`；不传计划时旧生成路径仍可用 | 1 | schema 正反例、`write_pptx` 兼容测试 |
| CF-PPTX-R3 | 新稿要像我自己的品牌 | 可从参考稿抽取并套用 theme/master/layout | 2 | profile 夹具、无 `theme_source` 的兼容测试、打开冒烟 |
| CF-PPTX-R4 | 数据页必须能改 | 按版式增页；插入原生表、图、图片 | 3 | 旧编辑操作回归、部件级断言、非法输入失败 |
| CF-PPTX-R5 | 交出前要逐页看过 | `ppt_qa` 门禁；无渲染器时仍跑确定性检查 | 4 | 溢出与页数不一致夹具、依赖缺失降级测试 |
| CF-PPTX-R6 | 在应用里翻页预览 | 工作区内 `.pptx` 逐页预览，可系统打开 | 5 | 前端翻页测试、文本预览回归、路径越权拒绝 |
| CF-PPTX-R7 | 主张要有出处 | 计划引用写入备注或附录，QA 能发现缺出处 | 6 | 备注内容断言、`kb_search` 回归、虚假 chunk 失败 |
| CF-PPTX-R8 | 改动不能影响现有功能 | 第 3.2 节全部条款；每阶段列出既有测试命令且通过 | 全阶段 | 兼容测试全集、权限与默认提示词未变的审查清单 |

## 8. 主用户路径

P-PPTX-NEW-1：用户启用「从参考文档做 PPT」技能，提出做一份给管理层看的季度决策稿。助手确认受众和决策问题，检索知识库，写出带统领结论、行动标题和出处的 `pptx_plan.json`，请用户批准。批准后按可选品牌稿生成 `.pptx`，插入原生图表，跑 `ppt_qa`，在会话预览里翻页。用户要求改第 3 页结论后，助手改计划并仅修订相关页，而不是用裸主题整份重生成。

P-PPTX-ENRICH-1：用户上传已有品牌稿并要求补充论据。助手用 `read_pptx` 与 `kb_search`，规划文本修订和必要的按版式增页，写到新路径，保留原主题与图片，再 QA 和预览。不得对上传稿直接调用无 `theme_source` 的 `write_pptx` 覆盖设计。

P-PPTX-CODE-UNCHANGED-1：用户在未启用该技能的会话里要求修一个 Rust 测试。默认提示词、工具审批和交付路径与今天一致，不出现 PPT 规划仪式。

## 9. 适用 Harness

- Spec Harness：本文件的 Req ID、主路径、测试矩阵和兼容边界。
- Payload Harness：`.pptx`、`pptx_plan.json`、deck profile、QA PNG、知识库引用。
- Compatibility Harness：既有 Office 工具测试、知识库 schema 迁移、文本预览、默认系统提示词、已打包技能列表。
- AI Collaboration Harness：Skill 是否按需加载、计划是否经用户批准、QA 结果是否对用户可见。
- Viewport Harness：预览翻页、失败态、窄宽度下的文件卡片。
- Observation Harness：渲染耗时、渲染器缺失、QA issue 计数、profile 抽取失败。
- Release Harness：技能资源是否进入安装包；官方技能或新工具一旦对用户可见，须在**实施阶段的** README 中按 evergreen 合同更新，而不是在本设计 PR 里提前改 README。

## 10. 测试矩阵与证据包

| 场景 | 类型 | 证据 |
| --- | --- | --- |
| 官方技能出现在模拟资源目录 | 自动 | 技能扫描测试 |
| 提示词含规划清单 | 自动 | 契约子串或清单 |
| `pptx_plan` 正反例 | 自动 | schema 测试 |
| `write_pptx` / `edit_pptx` / `format_pptx` 旧合同 | 自动 | 既有 Rust 测试 |
| 无 `theme_source` 与有 `theme_source` | 自动 + 可选打开冒烟 | zip 结构断言；有 Office/LibreOffice 的作业 |
| 原生表图部件存在 | 自动 | 关系与 XML 断言 |
| QA 在无渲染器下降级 | 自动 | `renderer=none` 且确定性 issue 仍出现 |
| 文本预览不变、PPT 预览不展示乱码 | 自动 | 前端与 `read_document` 回归 |
| 工作区外预览拒绝 | 自动 | 现有路径测试的扩展 |
| 编码会话不被 PPT 流程打断 | 自动或评审清单 | 默认提示词未改；相关 agent 测试仍过 |
| 真机翻页预览与图表可编辑 | 手工 / 发版后 | 证据包截图或录屏；实施 PR 声明 not live 直至真机验收 |

证据包至少包括：基线 SHA、计划文件、输入参考、输出 PPTX 路径、QA 报告、所用渲染器、以及「未改变的既有测试命令和结果」。

## 11. 外部依赖

视觉质检和逐页预览依赖本机或 CI 镜像中的渲染栈，与今天 `view_image` 的页面渲染相同：

- Windows：已安装的 Microsoft PowerPoint（COM 导出）。
- 全平台：LibreOffice (`soffice`) 将 PPTX 转为 PDF，再用 poppler 的 `pdftoppm` 光栅化指定页。
- macOS：QuickLook 可作为第 1 页兜底，分辨率有限，且不足以覆盖后续页。

这些依赖都是可选的。没有它们时，阶段 0 到 3 与 6 的非视觉部分、以及全部编码功能必须可用。文档、工具结果和设置页应说明如何安装 LibreOffice 与 poppler，以及 QuickLook 的页码限制。CI 若增加带渲染器的作业，应作为独立 job，失败不得用「跳过仍记成功」来冒充通过；未安装依赖时应显式 skip。

本方案不把「用户机器上必须安装 Microsoft PowerPoint」写成阶段 0 到 3 的产品前置条件。

## 12. 里程碑与优先级

| 优先级 | 阶段 | 建议里程碑 | 依赖 | 对现有功能的风险 |
| --- | --- | --- | --- | --- |
| P0，立即 | 0 技能打包与规划提示词 | 单独实现 PR，体积小，可先合 | 无外部渲染依赖 | 低：只加资源与技能文案 |
| P0 | 1 `pptx_plan.json` 与校验 | 在阶段 0 之后或紧随其后 | 无外部渲染依赖 | 低：旧 `write_pptx` 路径保持默认 |
| P1 | 2 deck profile 与主题套用 | 需要真实主题夹具 | 打开冒烟需要 Office 或 LibreOffice，但生成本身不需要 | 中：复制母版易出错，必须原子写与强测试 |
| P1 | 3 原生表图与按版式增页 | 可与阶段 2 部分并行，但增页应以能读取版式为前提 | 无强制渲染依赖 | 高：OOXML 图表复杂，须限制首发图种 |
| P2 | 4 视觉 QA 门禁 | 依赖阶段 1 的计划，最好在阶段 3 之后才能检查图表拍平 | 完整视觉检查需要渲染器 | 中：若误接入出片默认路径会破坏无依赖环境 |
| P2 | 5 应用内逐页预览 | 复用阶段 4 渲染 | 无渲染器时降级 | 中：须保证文本预览与路径沙箱不变 |
| P2 | 6 引用闭环 | 依赖阶段 1，可与 4 同时做规则 | 无强制渲染依赖 | 低 |

推荐不要把阶段 2 至 5 做进同一个实现 PR。PowerPoint 插件仍排在这些能力稳定之后，不在本里程碑内。

## 13. 兼容性与发布边界

- 本设计 PR 只新增本文件，不更新 README，不发版，不合入责任以外的任何实现。
- 阶段 0 合入安装包后，属于用户可见的官方技能，实施 PR 应按 README 合同把「从参考文档做 PPT」的规划能力写进 evergreen Features，且不出现具体版本号。那是实施 PR 的责任，不是本 PR 的责任。
- 新工具默认出现在模型工具列表中时，必须有清晰描述，以免编码任务误调用。若发现误调用率高，可将结构编辑和 QA 工具限制在技能 `tool_policy` 或任务类型里，但不得因此从全局列表误删 `write_pptx` 等已发布工具。
- 在真实安装包与至少一次手工做稿验收完成前，实施方不得宣称「高级 PPT 专家已 live」。

## 14. 待决问题

1. `ppt_generate_plan` 是否作为独立写入工具存在，还是「`write_file` + `ppt_validate_plan`」就足够？推荐后者起步，避免和文件工具双写。需要审查确认。
2. 原生图表是继续扩展手写 OOXML，还是引入成熟库？默认前者，必要时再立项评估。引入新库不能破坏旧 `write_pptx` 输出，也不能让计划 schema 分叉。
3. 行动标题的「必须是完整判断」要做到警告还是硬失败？推荐阶段 1 警告、阶段 4 对官方技能默认升级为必须处理，并允许豁免。
4. 官方技能安装后是默认启用还是默认关闭？须与 `skill-lifecycle-system.md` 的内置技能政策对齐，本方案不单独开特例；若生命周期规格要求内置技能默认关闭，则以那份规格为准，本方案只保证「可见、可启用、提示词正确」。
5. `read_document` 对 `.pptx` 的旧行为（字节当文本）是否可以在阶段 5 改为明确错误？需要先清点调用方。
6. 是否在阶段 2 同步做 `kb_get_document_outline` 与 `kb_retrieve_slide_examples`，还是推迟到阶段 6？推荐推迟，以免品牌抽取被扩大范围拖住。
7. 本设计合入后，是否在同一后续 PR 中把本文件链入 `docs/specs/feature-specs/README.md`？按仓库原则应当链接；本 PR 为遵守「只提交这一份文档」的约束而暂不改索引，审查通过后由实施或文档跟随 PR 补上。
8. 多语言稿（中英混排、CJK 字体）的默认字体策略是否写入计划 schema？技能已建议中文稿使用微软雅黑等 CJK 字体；是否升级为 profile 必填项待定。

## 15. 审查时请重点证伪的问题

请对抗性审查至少尝试证伪以下几点，而不是只做摘要：

- 第 2 节引用的文件与基线代码是否仍然一致，有无过时判断（尤其是备注是否已实现、技能是否确实未打包）。
- 「可选参数加法」是否真能保证 `write_pptx` / `edit_pptx` 的旧调用零行为变化，包括内容类型、关系 id 和默认主题。
- 阶段 3 的图表范围是否过度设计，或反过来窄到无法支撑「数据可视化」的主张。
- 阶段 4 若被错误接到出片默认路径，会如何破坏无 LibreOffice 的用户和 CI。
- `pptx_plan.json` 是否与 `personal-knowledge-office-assistant.md` 冲突，或对模型过于沉重以致被跳过。
- 验收标准是否可以被自动测试证伪，哪些仍然只是无法检查的愿望。
- 是否遗漏了对权限网关、工具恢复（`tool_recovery.rs` 已对 `write_pptx` / `edit_pptx` 有专门分支）、沙箱或无项目会话上传附件路径的影响。

