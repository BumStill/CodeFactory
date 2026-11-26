# M289：Windows 拖文件到主界面不进附件 + 附件上限提到至少 50MB

## 现场（用户 2026-11-26 报告）
- Windows 上从文件夹（资源管理器）把文件或图片拖到 CodeFactory 主界面，不会自动加入附件。macOS 上可用。

## 已知线索（编排方只读核实，origin/main）
- 拖放只在 `src/components/MessageInput.tsx`（约 126–245、444–448 行）的输入区 div 上，用 HTML5 `onDrop` / `e.dataTransfer.files` 处理，然后走 `enqueueFiles` → `saveAttachmentFile`。
- `src-tauri/tauri.conf.json` 的窗口没有配置 `dragDropEnabled`，所以 Tauri 2 默认开启原生拖放接管。在 Windows（WebView2）上，原生接管开启时网页收不到 OS 文件拖放的 HTML5 drop 事件和文件；macOS 行为不同，所以一直没暴露。这是推断，请先在代码与 Tauri 文档里核实。
- 用户说的是拖到「主界面」，不是只拖到输入框：落点在消息区或窗口其他位置时也应该加入当前会话的附件。

## 规格（放到 `docs/specs/feature-specs/m289-file-drop-attachments.md`）
- R1：在 Windows 与 macOS 上，从系统文件管理器把一个或多个文件（含图片）拖到主窗口任意位置松手，都加入当前会话输入框的附件，结果与点「添加附件」或粘贴一致（同一条 `saveAttachmentFile` 路径、同样的大小/类型限制与错误提示）。
- R2：拖入过程中有明确的可放下提示（覆盖整个可放区域），离开或取消时提示消失；没有打开会话或输入框不可用时给出说明，不静默丢弃。
- R3：实现方式二选一，在规格里写明理由：(a) 使用 Tauri 原生 `onDragDropEvent`（拿到路径，由后端读文件进附件），三平台统一；或 (b) 关闭 `dragDropEnabled`，改用 HTML5 拖放并把放置区扩到整个主界面。须确认所选方案不破坏应用内部已有的拖拽（如列表排序、文本拖动，若存在）。
- R4：多文件、同名文件、超大文件、不支持类型、文件夹（应提示不支持或明确行为）各有确定结果。

## 测试（先看到失败）
- 单元/组件测试覆盖：拖放事件 → 附件入队（含多文件、拒绝类型、无会话）。
- 若选 (a)：用 mock 的 Tauri drag-drop 事件驱动前端，断言附件列表；后端读文件命令有 Rust 测试（路径校验、大小上限、不跟随不安全路径）。
- 真实浏览器 headless 验收页（仓库已有 verify-*-headless.mjs 模式）验证整窗放置提示。
- 实地：macOS 真机拖放验证；Windows 无法本地验证时在 PR 写明「需要 Windows 真机回填」并列出场景，编排方发版后请用户在 Windows 上验收。


## 追加：附件大小上限（用户 2026-11-26：超过 25MB 加不进去，至少要支持 50MB）
现场：上限写死在四处，彼此独立——`src/components/MessageInput.tsx:36`（MAX_ATTACHMENT_BYTES = 25MiB）、`src-tauri/src/clipboard_attachments.rs:7`（同值），以及同文件内嵌的粘贴脚本 `:50`（JS，26214400）与 `:73`（PowerShell，26214400）；`:106` 的 base64 长度校验也依赖它。
- R5：附件单文件上限提到至少 50MB（50 MiB），前后端与粘贴脚本只用同一个来源（Rust 常量经命令或构建期导出给前端，脚本里的数字由同一常量生成），提示文案随常量变化，不再出现硬编码的「25MB」。
- R6：查清 50MB 附件的整条链路是否都能承受并写明结论：前端读取与 IPC 传输（base64 会膨胀约 1.33 倍，确认 Tauri 命令载荷上限与内存峰值）、落盘位置、发给模型时的处理（图片是否超过各端点的单图大小/像素上限，超过时压缩、降采样或明确拒绝并说明原因；非图片大文件是否按路径交给工具读取而不是整段塞进上下文）。任何一环做不到 50MB，都要在界面给出该环节的真实原因，不得静默失败。
- 测试：边界值（50MiB 正好通过、+1 字节拒绝并提示新上限）、粘贴与拖放与「添加附件」三条入口一致、50MB 图片发送到模型前的处理有测试（可用合成大图）。Payload Harness 适用。

## Implementation Notes

选择方案 (b)：在 `tauri.conf.json` 关闭原生 `dragDropEnabled`，由 WebView 的 HTML5 文件拖放事件统一处理，并把监听范围扩展到 `window`。这是 Windows WebView2 能收到资源管理器文件数据且能复用现有 `File -> save_session_attachment` 路径的最小方案；监听器仅拦截 `DataTransfer.types` 含 `Files` 的事件，因此不接管应用内部文本拖动或其他非文件拖放。

附件原始文件统一由 Rust 的 `MAX_ATTACHMENT_BYTES` 限制为 50 MiB，并通过 `attachment_limits` 提供给前端，原生粘贴脚本也由该常量生成数字。模型图片链路仍以 base64 data URL 发送，50 MiB 会膨胀到约 66.7 MiB 且超过现有模型图片保护线，因此图片在统一入口明确限制为 8 MiB并提示压缩；非图片 50 MiB 只落盘并把本地路径交给模型使用工具读取，不整段塞进上下文。保存时前端当前会同时持有 `ArrayBuffer`、二进制字符串和约 1.33 倍的 base64，50 MiB 非图片峰值会显著高于文件本身，但 Tauri IPC 本身没有本项目配置的更低请求体上限，后端再次按解码字节校验后落盘。

## 约束
改文件只用 edit_file / write_file。不改信任根；场景缺口只追加 `docs/testing/scenario-mappings.json`。不碰凭据、settings.json、其他会话的 PR。规格文件在工作区外，用 bash cat 只读查看。若改 tauri.conf.json 的窗口配置，在 PR 写明影响面。

## 交付
PR 正文 `R | 是否完成 | 证据` 列 R1–R6；`Release-Urgency: default`。全部完成、CI 全绿后正常交付；未完成保持草稿、不打 armed。

## 验收（发版后）
用户在 Windows 上从资源管理器拖 1 张图片 + 1 个文本文件到主界面消息区，两者都出现在附件栏，发送后模型能读到；再拖一个 40MB 左右的文件，能加入附件且发送不报错。

Release-Urgency: default
