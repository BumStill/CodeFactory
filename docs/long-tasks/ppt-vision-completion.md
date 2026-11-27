# PPT 视觉截断与后续答复修复

## Basics
- Task ID: PPT-COMPLETE-20261112
- Title: PPT 视觉截断与工具后静默结束
- Feature spec: docs/specs/feature-specs/ppt-vision-completion.md
- Related Req IDs: PPT-R1..PPT-R5

## Completion Standard
- Done means: 正确区分完整/截断/空回复，视觉恢复有界且不缓存残缺内容；真实单页密集 PPTX 验证、相关回归与独立审查完成，远端质量门禁按对应授权执行。
- Blocked means: 有准确工具/权限/环境证据且不存在已授权可行路径。

## Current State
- Current phase: 本地回归与真实渲染链通过，独立复核通过，not live。
- Current checkpoint: 已在 989c8da9 上无冲突恢复原测试；旧 dirty 状态留有快照和 stash。
- Next owner: 当前项目任务协调共享 Cargo 验证与独立审查。
- Updated at: 2026-11-12

## Completed Items
- [x] 实际查看用户截图，区分症状与未获证据的线上原因。
- [x] 原视觉 wire 测试三项失败；最新基线 OpenAI/Anthropic 两项失败、AgentLoop 一项失败，记录先红。
- [x] 本地实现终止原因拒绝、视觉单次请求、45秒时限且不做未记账自动重试、缓存策略版本化及可见失败终态。
- [x] 单页原生夹具：120个文本形状、40个图形、39条连接线，加模板标题；不使用用户文件或单张图片替代。

## Remaining Items
- [x] 编译及相关回归；单页真实渲染显式验收。
- [x] 独立代码与验收审查。
- [ ] 准确 head 远端 CI / 原生 GUI / 真实 provider 验证，当前均不能称通过。

## Blockers
- 共享 Cargo 由父任务串行协调；不是产品授权阻塞。
- 未获得故障 Windows 运行版本、原 PPTX 或 provider 事件日志，无法证明该次会话唯一根因。

## Evidence
- /workspace/ppt-evidence/vision-red.log：原视觉 2过3失败。
- /workspace/ppt-evidence/resume-20261112/transport-red.log：最新 main 1过2失败。
- /workspace/ppt-evidence/resume-20261112/loop-red.log：最新 main 0过1失败。
- 截图仅表明选择的主对话模型；view_image 可能选择独立视觉路由，不能由截图推断实际视觉 provider。
- Responses SSE 使用实际生产解析器的本地协议服务验收；认证路径未替换，也不声称已认证。

## AI Collaboration
- context scope: provider transport、视觉缓存、共享 AgentLoop 与真实合成 PPTX。
- assumptions: 任务不修改用户源文件、不跨越 #1043 远端暂停、不混入 PM 或交付归属修复。
- review point: 原三个角色因额度中断，恢复者保留测试与快照后实现；最终需独立审查。
- validation result: 最终相关回归和单页渲染链通过；见下方准确范围。

## Stop Boundary
- 不执行真实远端写入或发布；父任务依据本次 PPT 对应授权和实际质量门禁协调后续。

## 中间回归与修正
初版共享 AgentLoop150项通过，视觉/PPT工具回归通过；新增空响应测试发现摘要清理器先生成虚假完成文本，已将拒绝前移。原 internal_text 空结果成功测试需调整为显式空结果错误。独立复核要求 Responses 按未完成原因分类，已修正。
初版两次请求的真实单页渲染通过（LibreOffice，120文本形状+40图形+39连接线）；因预算记账缺口，最终改为单次请求失败后结构/OCR降级，旧渲染回执不作为最终通过证据。最终回归仍待执行。

边界说明：单次限制指每个视觉 route 的 helper 调用；保留既有配置路由间 fallback 与本地 OCR，不新增同路由自动重试或额外预算绕行。既有 helper 持久记账缺口不在本次宣称已解决。

## 最终本地验收（2026-11-12）
- 基线 HEAD：989c8da9c272de73142099ef25ed2eb7e3daa88b；源码未推送。
- 桌面 Rust：model_transport 51、vision 18、internal_text 6、pptx_edit 32、view_image 5，全部通过。额外 ppt_output_ 4 项与前述重叠，不重复计数；anthropic_client 独立过滤命中0项，不计通过测试数，真实 Anthropic 协议用例位于 model_transport。
- 共享 AgentLoop 全150项通过；原生显式 ignored 验收单独1项通过。以上共263个不重叠主测试（112桌面+150共享+1原生）。
- 官方 Debian/LibreOffice/poppler 环境真实渲染：单页120文本形状、40图形、39连接线；PNG282476字节，SHA256 b04560cab8c5910cd95eb16f16074732f1f19d259e6e18a58e3093eeb7b2ebed。
- 真实 read_pptx→view_image→read_pptx handler 经共享 AgentLoop 执行；本地 SSE 返回非空 length，下一模型轮收到明确 VISION_OUTPUT_TRUNCATED，结构重读包含 CELL-119，第四轮有 TextDelta 和 Done。共一次视觉请求、部分缓存0行、源文件字节不变。
- 证据层级：真实 OOXML/LibreOffice/工具 handler/共享循环/视觉 SQLite；模型决策与视觉 provider 为合成，本次主循环使用 NullPersistence/测试权限，不证明生产认证、真实会话恢复、UI或线上故障唯一原因。
- 日志：/workspace/ppt-evidence/resume-20261112/final-exits.tsv、final-native-render.log 及 final-*.log。命令入口 green-final.sh 使用仓库共享 Cargo wrapper，原生测试明确 --ignored --nocapture。
- 本地治理基线、统一 local scenario gate 与 git diff --check 通过；不能替代精确提交的远端 required checks。

独立审查：最终复核未发现新的阻断问题；Responses 未完成原因分类和新增自动重试费用风险已闭合，生产路由仍只来自 host settings，测试注入仅 cfg(test)。审查明确保留真实 provider/GUI 未验收限制。

## 2026-11-17 续接验收

- 原f32e9ef1及夹具保留；独立受管fix/ppt-acceptance-resume-local以最新main120a0eed合并旧候选，无冲突。安全、交付、PM工作树未改。
- 真实provider/GUI前置核查：本机常见模型API变量和正常CodeFactory设置不存在；只检查存在性，未读取/移用凭据。云环境声明desktop enabled，但当前没有cua/sky/browser/viewer工具、DISPLAY/Wayland，不能启动真实受支持GUI验收。未调用真实provider，真实用量不可验证。
- 复核旧201元素原生夹具：162形状+39连接线，160标签全部存在于保存结构输出；坐标来自OOXML，结构输出不含全量几何坐标。图片中密集文字溢出是压力夹具本身，不能拿它证明视觉语义/排版完整。原件SHA256=2576a6cb2d044e65791360ea4e73152ddfb328b7d83ff7371189385fd197fbf6，测试只复制到临时目录。
- 独立审查确认两文件可精确追加到HLT-001/VIS-001；6项普通mapping回归通过。该映射不增加针对性targets，不能将其当完整PPT门禁。Anthropic截断普通Rust测试及native渲染显式ignored入口仍需正式治理登记，未改registry/validator/hooks。
- 新增完整长描述成功路径验收与旧截断负例配对；测试provider仍为localhost合成，成功usage元数据不等于持久账单。helper失败仍无可传回Usage、billing/provider_attempt为空，未新增自动重试放大成本。
- PowerPoint选择/组合/移动编辑/取消组合/保存重开属于另一验收要求，未运行，不能由LibreOffice渲染代替。
- 本轮具体运行命令及原始结果保存在/workspace/ppt-evidence/resume-20261117；完成结果按最终日志记录。not live，未远端写入或触发CI。

### 本轮最终回执

- Rust不重复264项通过：vision18、internal_text6、model_transport51、pptx_edit32、view_image5、共享AgentLoop150、显式native2。普通pptx组忽略的2项随后显式执行均通过，不重复累计。普通mapping6/6，治理baseline和长任务记录通过。
- Debian13.7 / LibreOffice25.2.3.2 / Poppler25.03.0，保存的201元素原件只读复制。原生两例6.24秒均通过；实际renderer=libreoffice，PNG282476字节，image_sha=b04560cab8c5910cd95eb16f16074732f1f19d259e6e18a58e3093eeb7b2ebed。
- 完整分支：>5000字符合成描述及尾标记进入工具/history/SQLite全文；listener关闭后cache_hit仍返回全文，新增tokens=0。截断分支：length明确回退、1次请求、0部分缓存；两分支均三工具顺序、四轮、Finished+可见TextDelta+Done，结构重读及源字节不变。
- synthetic_wire_usage=321输入/1800输出，每分支1次合成请求；真实provider请求0。durable_usage_verified=false，real_gui=false，synthetic_provider=true。不声称真实模型语义完整、原Windows故障复验、生产持久会话或收费核验通过。
- 独立审查test blob c8912b82d8353b24b5636d65fed5ac774453071f无阻断，所有新增重构在cfg(test)，原负例断言保留。与main组合未增加旧修复之外的生产逻辑变化。

## 2026-11-27 发布版复验

- 对照发布版 `v1.133.1`，当时的 main 和 tag 都是 `d3bb94f5528d6cd67e7f05e4dcbb139a1c431528`。Office 附件上限和范围改动没有触及视觉缓存/流结束语义。旧 `d2c85e45` 本地版本与原 201 元素夹具保留。
- 先在该版本加入回归测试。工具后空模型结果缺少用户通知，非空 `finish_reason=length` 被视觉桥接受，两例均失败。视觉和桌面 Rust 测试需要临时补齐发布版剪贴板模块的 Linux 条件编译。相同临时补丁用于基线和候选，不计入产品提交。
- 候选保留最新 GPT-6 Responses 分流、Windows 的堆分配边界、两套 SSE 空闲超时和 Office 文件架构更新。直连 GPT-6 Responses 的独立解析器仍缺 `response.incomplete` 检查，未纳入已验证路由的结论。
- 原夹具 SHA256 `2576a6cb2d044e65791360ea4e73152ddfb328b7d83ff7371189385fd197fbf6`。三条原生 Linux 测试都执行实际 `read_pptx → view_image → read_pptx` 和 LibreOffice 渲染，核对全部 160 个标签及原文件字节。完整分支检查长描述尾标记、缓存与最终答复，截断分支检查不入缓存和结构续读答复，空后续分支检查可见错误、无完成事件、工具只执行一次。视觉来自本地合成 SSE，主对话模型为脚本，业务会话使用测试持久层。未调用真实 provider 或验证 Windows GUI、PowerPoint 分组及失败用量入账。
- #1100 是单独的长任务恢复 PR。历史 Windows 候选 `d45f2403` 在注入前栈溢出，而同环境基线 `03678c15` 通过，尚无精确当前组合的 Windows 栈。未编辑该分支；它不能解释 Leo 原始 PPT 会话。原始回执与复跑命令保存在 `/workspace/ppt-evidence/recheck-20261127`。
- 复验共 268 项不重复 Rust 测试通过，其中包括单独执行的三项原生渲染验收。普通 mapping 验证 6/6。具体解释和平台条件见同目录回执。
