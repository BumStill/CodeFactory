## M190 粘贴附件与预览信息补齐

实现规格：docs/specs/feature-specs/m190-paste-attachments-preview.md（含任务原文、修复前类型×位置矩阵、目标行为及真机分工）。

### Req ↔ 验证与先红证据
- R1/R2/R3/R4：MessageInput.paste.test.tsx，8 项；修复前 7 项失败/纯文本 1 项通过，修复后 8 项通过。多图/文档、clipboardData.files、25MB、无名命名、原生兜底、纯文本、并发上传/禁止提前发送、可移除错误。
- R5/R6：MessageAttachmentMatrix.test.tsx，18 项先红后绿；png/jpg/gif/webp/heic/pdf/docx/pptx/xlsx/csv/txt/md/ts/mp3/mp4/zip/unknown，原名 title、类型、自动 MB 单位、系统打开；图片面板 title 和删除/格式失败提示。
- R7：保留 image file:// Markdown，不修改 M153 route；Rust 全工作区测试覆盖既有图像视觉桥接回归。
- 原有 MessageInput.attachments.test.tsx 7 项不变并通过。
- 原生：m190_clipboard_limits_and_file_list 覆盖列表、二进制内容、25MB 边界、metadata、删除后读取失败。

### 自验收
- 多图片+文档粘贴、图文默认事件不取消、发送→重载历史→图片放大、文件受控打开、自动大小、宽窄屏无横向溢出：真实 headless Chrome 通过。
- 明/暗主题 × 1366/480×768 截图：系统临时目录 codefactory-m190-headless/{light,dark}-{1366,480}.png。
- 既有 image-preview 与 attach-file-links headless 通过，包括无项目附件、缺失文件错误和本地/相对链接。
- 前端全量与 build、Rust workspace、统一治理门禁：结果以交付前执行日志为准，未完成不宣称通过。
- 真机验收由编排方发版后完成；not live，本任务只合并不发版。macOS/Windows 按规格文档核对截图/浏览器/聊天软件/文件列表/多图/图文/纯文本，发送并重启后预览、删除错误及非视觉模型 bridge。Windows 当前会话证据为跨平台 Rust/DOM 测试及 PowerShell STA FileDropList/bitmap 实现，不宣称真机通过。

### Applicable Harnesses
Spec：原文规格+矩阵；Payload：大小/多项/读取错误；Compatibility：原 Markdown/无项目/历史/视觉桥；Viewport：真实浏览器明暗宽窄；Observation：保存中/错误可移除/缺失文件；Release：授权仅 merged，真机后验；AI Collaboration：上下文=附件输入/原生桥/消息渲染，假设=不新增解析器，review point=混合文本和安全打开，validation=先红后绿+真实浏览器+项目测试。模板禁止 delegate_tasks，采用单执行流，未启动角色子代理。

### 边界与交接
无待裁决 Spec Amendment。HEIC、Office、音视频、zip、未知扩展名使用既有受控系统打开/文件管理器定位，不承诺新增内嵌解析。UI-007 仅追加本任务精确文件映射和真实测试绑定（Option B），未放松任何已有映射、validator 或 workflow；若 trusted scenario runner 报 cannot self-modify，等待编排方按模板处理，不绕过。
新增原生 paste_session_attachments 为 Tauri IPC；现有后台 dispatch 仍可发送附件 Markdown。本次未新增专用“从系统剪贴板附加”菜单/dispatch 动作，作为后台操作入口的待补建议，不宣称 IPC 本身已接入 dispatch。

macOS 原生兜底已用独立命名 NSPasteboard 实测多文件 URL、空剪贴板、合成有效 PNG 位图→PNG base64 输出断言；不读取或修改生产 general pasteboard，不需要 Swift 开发工具。

Scenario-Test: CAP-001, CXD-001, CXD-002, DAT-001, HLT-001, HLT-002, HLT-003, HLT-004, HLT-005, RTE-001, RTE-002, RTE-003, RTE-004, RTE-005, SKL-002, SKL-003, UI-001, UI-002, UI-003, UI-004, UI-005, UI-006, UI-007, UI-008, UI-009, UI-010, UI-011, UI-012, UI-013, UI-014, UI-015, VIS-001
README-Update: reviewed
README-Update-Reason: 补齐已承诺的粘贴附件与预览行为，不改变安装、隐私或平台公开承诺。
