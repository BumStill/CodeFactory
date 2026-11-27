# M299：附件上限提到 200MB，按路径复制不经 base64；大 Office 文件不崩溃、模型可按需读取

## 现场（用户 2026-11-27，Windows，v1.131.0）
- 选择一个约 80MB 的文件上传，提示「超过 50MiB 上限，已跳过」。用户经常处理 Office 类文件（Word/Excel/PPT），要求上限 200MB，并「确保不崩溃」。
- 现状（v1.131.0）：`src/components/MessageInput.tsx` 约 170–195 行 `saveAttachmentFile` 把整个 File 读成 ArrayBuffer → 拼二进制字符串 → `btoa` → 通过 `invoke("save_session_attachment", { dataBase64 })` 送到后端（`src-tauri/src/clipboard_attachments.rs`，上限常量 `MAX_ATTACHMENT_BYTES = 50 MiB`）。200MB 文件在这条路径上内存峰值约为文件大小的 4–6 倍（ArrayBuffer + 二进制字符串 + base64 + IPC 序列化 + 后端解码），WebView 与进程都可能崩溃。
- Office 工具：`src-tauri/src/tools/{docx,pptx,pptx_edit,xlsx,read}.rs`，xlsx 有 `MAX_ROWS=400`，其他未见大小/分段约束（请核实是否整文件解压/读入）。

## 规格
- R1：附件单文件上限 200MB（仍由 Rust 单一常量定义，前端从 `attachment_metadata` 取，提示文案随常量）。
- R2：大文件不再走 base64：
  - 拖放（Tauri drag-drop 给出路径）与「添加附件」（改用原生文件选择对话框拿到路径）都由后端按路径流式复制到会话附件目录（固定内存缓冲，带进度与取消）；
  - 只有剪贴板粘贴（拿不到路径）继续走字节，但单独设较小上限（如 20MB），超限明确提示「请用添加附件或拖入」。
- R3：复制过程中界面显示进度、可取消；失败（磁盘满、权限、文件被占用——Windows 上 Office 打开的文件常被锁）给出可读原因，不静默。
- R4：发送给模型：非图片附件只传路径与基本元数据（文件名、类型、大小、页数/表数等可廉价得到的信息），由模型按需调用工具读取；图片沿用 M289 的自动压缩。
- R5：Office 工具在 200MB 级文件上不崩溃、不卡死：docx/pptx/xlsx/read 均流式或分段读取（只解压需要的部件），有输出上限与分页/范围参数（按页、按工作表与行范围、按幻灯片范围），超出部分给出「还有 N 页/行，可用参数继续读」的提示；单次工具调用有内存与时间上限。
- R6：Windows 优先：路径含中文/空格、被 Office 占用（共享读）、OneDrive 占位文件（未下载）等情况都有确定行为。

## 测试（先红后绿）
- 合成 200MB 文件（含一个 ~80MB 的 xlsx/docx/pptx 合成样本）：按路径复制成功，测内存峰值（断言不超过固定上限，如 300MB 增量），进度事件单调；200MB+1 字节被拒并提示。
- 剪贴板字节路径 20MB 边界。
- Office 工具：大样本按范围读取返回有限输出且耗时有上限；损坏文件给出可读错误。
- 前端 headless 真浏览器：进度条、取消、错误提示；不得只用 jsdom。
- Windows 场景纳入 M296 的 windows-latest 真实 app 验收（若 M296 已合并，则在该作业加用例；否则在 PR 写明需 Windows 回填：80MB Office 文件选择与拖入）。

## 约束
改文件只用 edit_file / write_file。不改信任根；场景缺口只追加 `docs/testing/scenario-mappings.json`。不碰凭据、settings.json、其他会话 PR，不动任何 merge-queue: hold 标签。规格文件在工作区外，用 bash cat 只读查看。新增依赖须在 PR 正文说明用途与风险。

## 交付
PR 正文 `R | 是否完成 | 证据` 列 R1–R6；`Release-Urgency: immediate`（用户日常工作文件加不进来）。

## 验收（发版后）
用户在 Windows 上：选择并拖入约 80MB 的 Office 文件，均成功加入附件，app 不卡死；发送后模型能读到指定页/表内容。
