# M190：聊天框直接粘贴图片/文件作为附件 + 附件预览信息完备性审计

## 用户诉求（10-29）
「要支持在聊天框直接粘贴图片作为附件，另外检查一下各个附件的预览信息是否完备。」

## 现状（编排方只读核对 origin/main，仅供定位，以你实测为准）
- `src/components/MessageInput.tsx` 已有 `onPaste`：只处理 clipboard 里**第一个** `image/*` 文件项，然后 return；粘贴路径**不检查 25MB 上限**（拖放/选择文件有）；无名图片统一叫 `pasted.png`。
- 输入区：图片显示缩略图，文档是 chip（名字 + KB）。已发送消息：`MessageList.tsx` 用 `ImagePreview` / `MessageFileArtifactCard`。
- 用户说「要支持」，说明真实 app 里粘贴体验不成立或不完整——先在真实 app（macOS + Windows 语义）复现再改。

## 需求
### A. 粘贴附件
- R1 真实 app 中，以下粘贴来源都能变成附件（先逐个复现现状，记录哪些不行）：系统截图（macOS ⌘⇧⌃4 / Windows Win+Shift+S）、浏览器「复制图片」、聊天软件复制的图片、Finder/资源管理器里「复制」的一个或多个文件（图片和非图片文档都要）、一次粘贴多张图。WKWebView / WebView2 拿不到文件项时，走 Tauri 侧读取剪贴板（图片位图 / 文件路径列表）的兜底。
- R2 粘贴与拖放/选择文件走同一入口：同一大小上限、同一错误提示、同一类型判定；粘贴的无名图片命名为 `粘贴图片-<时间>.png` 之类可区分的名字。
- R3 粘贴纯文本行为不变；同时含文本和图片时（如从网页复制图文）：图片成附件、文本照常粘贴。
- R4 上传中、失败（超限/读取失败）有可见状态，失败可移除，不吞错。

### B. 附件预览完备性审计
- R5 先产出审计矩阵（写进 `docs/specs/feature-specs/` 相应规格）：类型 × 位置。类型至少含 png/jpg/gif/webp/heic、pdf、docx、pptx、xlsx/csv、txt/md/代码文件、音视频、zip、未知扩展名；位置含：输入框 chip、已发送消息、重新打开会话/重启后的历史、点击后的预览面板。每格记录：类型图标、文件名（长名截断+悬停全名）、大小（KB/MB 自动单位）、可打开预览与否、文件已被删除时的提示。
- R6 按矩阵补齐缺口（缺图标、缺大小、点了没反应、历史里丢预览、文件丢失时报错难懂等）。不要超出矩阵自加功能。
- R7 非视觉模型下粘贴图片仍按 M153 视觉桥接处理（不得退化）。

## 验收
- 先写失败测试（粘贴多项、超限、文件列表、图文混合；预览矩阵每类的渲染断言）。
- UX 改动按 AGENTS.md 实地验证：headless acceptance 页（参考 `src/acceptance/image-preview.tsx`、`attach-file-links.tsx`，真浏览器真布局，不占屏幕）+ 真实 app 中至少走「截图粘贴→发送→重启后历史仍有预览」一条主路径；PR 列出实地走过的场景，Windows 无法本机验证的写明替代证据。
- 场景登记按 Option B 追加映射。

## 约束
- task-template（harness + SDD + TDD）；合成 fixture；不改生产数据。授权交付（ceiling: merged）。

Release-Urgency: default

## Implementation Notes：执行边界与基线审计

按 task-template §8，安装版/OS 剪贴板实机验收由编排方发版后执行。本会话先用测试复现 WebView DOM 输入语义，使用真实浏览器布局验收；不操作生产剪贴板或会话。沿用当前 managed worktree，不另建分支。模板要求单执行流、不调用 delegate_tasks，因此跳过后台角色。

### 修复前审计矩阵
每格依次记录「图标；全名悬停/截断；大小；预览/打开；删除提示」。历史由相同 Markdown 渲染器重放，故与已发送一致。

| 类型 | 输入框 chip | 已发送消息 | 重开/重启历史 | 点击后面板 |
| --- | --- | --- | --- | --- |
| png/jpg/gif/webp | 缩略图；截断但悬停仅路径；仅 KB；放大；无清晰提示 | 缩略图；caption 无完整 title；无大小；放大；预览失败 | 同已发送 | 图片；caption 截断无 title；无大小；放大；无删除提示 |
| heic | 通用回形针；名字截断/悬停路径；仅 KB；不可点击；无 | 普通文本；原名；无大小；不可点击；无 | 同已发送 | 无 |
| pdf | 通用回形针；名字截断/悬停路径；仅 KB；不可点击；无 | 普通文本本地路径；原名；无大小；不可点击；无 | 同已发送 | 无（显式文档卡片才有打开） |
| docx | 同 pdf | 同 pdf | 同 pdf | 同 pdf |
| pptx | 同 pdf | 同 pdf | 同 pdf | 同 pdf |
| xlsx/csv | 同 pdf | 同 pdf | 同 pdf | 同 pdf |
| txt/md/代码 | 同 pdf | 同 pdf | 同 pdf | 同 pdf |
| 音视频 | 同 pdf | 同 pdf | 同 pdf | 同 pdf |
| zip | 同 pdf | 同 pdf | 同 pdf | 同 pdf |
| 未知扩展名 | 同 pdf | 同 pdf | 同 pdf | 同 pdf |

### 目标矩阵（R5/R6）
- 图片 png/jpg/gif/webp：输入缩略图+类型标签、完整 title/截断、自动 KB/MB、放大/系统打开、加载失败提示。发送与历史保留原名及大小 caption；面板完整 title、同大小、失效提示。
- heic：跨 WebView 无统一解码保证，明确作为图片文件卡（类型 HEIC）、完整 title/截断、自动 KB/MB、系统打开（不承诺内嵌解码）、可见缺失/无法打开提示。
- pdf、docx、pptx、xlsx/csv、txt/md/代码、音视频、zip、未知扩展名：每类各位置都提供文件类型标签（通用文件图标+扩展名）、完整 title/截断、自动 KB/MB、受控系统打开按钮。zip/未知/可执行代码按原有安全策略在文件管理器定位，不执行。无需新增解析器或不受支持的内嵌预览面板。
- 历史文件卡通过受控原生 metadata 查询补充大小/缺失状态；新消息将原名和大小写入 Markdown，继续使用现有消息存储，避免 schema 迁移。旧图像无大小时只在已知 metadata 后显示，不伪造。
- OS 兜底仅在 DOM 无文件项且没有普通文本时读取；文件 URL/路径列表例外，避免 Finder 文本路径遮蔽原生文件剪贴板。保留原生默认文字粘贴，不 preventDefault。

### Requirements Traceability / Harness
| Req | 最低证据 |
| --- | --- |
| R1/R2/R3/R4 | MessageInput 粘贴多项/文件列表/超限/混合文本/兜底/并发/可移除失败测试；原生解析与大小测试 |
| R5/R6 | 本矩阵；各扩展名渲染断言；宽/窄、明/暗真实浏览器截图；发送后重载 Markdown 附件 |
| R7 | 保留图片 file:// Markdown 和既有 M153 视觉桥接测试 |

Applicable Harnesses：Spec（本文件）、Payload（大小/读取失败/原生列表）、Compatibility（原有 Markdown/历史/M153）、Viewport（真实 headless 浏览器）、Observation（明确状态）、Release（仅合并不发版，编排方后验）、AI Collaboration（单流上下文=附件入口/渲染/原生桥，假设=不新增格式解析器，review point=混合粘贴与兜底边界，validation result 见后续证据）。

### 发版后编排方真机核对
macOS 与 Windows 分别：系统截图、浏览器复制图片、聊天软件图片、Finder/Explorer 一/多文件（图片+文档）、多图、图文、纯文本；核对保存中与超限/读取错误可移除；截图粘贴→发送→退出并重启→原名/大小/预览；删除合成附件→可读缺失提示；非视觉模型→M153 vision bridge route。仅使用合成文件与隔离会话。
