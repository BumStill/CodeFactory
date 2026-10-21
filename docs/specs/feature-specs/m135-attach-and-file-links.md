# M135：①不在项目里时聊天框无法上传文件 ②对话流里的文件链接点名字/「打开」按钮都无反应（用户 10-21 报告）

## 现象与线索
- ① 未选择项目（无 cwd）的对话里，输入框的附件/上传不可用。`src/components/MessageInput.tsx:528` `disabled={!cwd || uploading}`：附件落盘到 `<cwd>/.codefactory/attachments/`，无 cwd 时直接禁用。
- ② 对话流中的文件链接（消息 Markdown 链接、工具卡片里的路径、「打开」按钮）点击后无任何反应、无报错。相关：`src/lib/tauri.ts`（约 352 行 opener 说明）、`src/components/ToolCallCard.tsx`（约 284 行：相对路径需相对会话 cwd 解析）。可能原因：相对路径未解析为绝对路径；Tauri opener 插件 capability/scope 不允许该路径或 file:// 链接；Markdown 链接被 WebView 拦截而没转交系统 opener；错误被吞。
## 需求
- R1 无项目对话也能上传：附件落盘到应用数据目录（如 `<AppData>/attachments/<session_id>/`），与有项目时同样的预览、发送、模型可读路径；有项目时行为不变。
- R2 文件链接可打开：消息里的本地路径/`file://` 链接、工具卡片路径、「打开」按钮都能用系统默认程序打开（目录则在访达/资源管理器中显示）；相对路径按会话 cwd（无 cwd 时按附件目录）解析。路径不存在或无权限时给出可见提示（toast），不得静默失败。
- R3 安全：只打开本地文件路径与 http(s) 链接；不执行可执行文件（.app/.exe/.sh 等改为「在访达/资源管理器中显示」）；opener scope 不放宽到任意 scheme。
- R4 Mac 与 Windows 都要覆盖（路径分隔符、盘符、`\\?\` 长路径前缀——用户 Windows 截图里路径就是 `\\?\C:\Users\…`）。
- R5 测试（先失败）：单测覆盖路径解析与无 cwd 上传；**按仓库规则 UX 必须实地验证**：用 headless acceptance 页（verify-*-headless.mjs 风格）或 dev app 实测「无项目对话上传图片并发送」「点击消息中的文件链接与打开按钮」，PR 写明实测场景；Windows 路径写明替代证据。
## 约束
- 授权交付（ceiling: merged），PR 标题写明内容，正文带 `Release-Urgency: immediate`（用户报告）。不要超出规格自加范围。

## Implementation Notes
- 使用受控 Rust command 解析路径、验证存在性并选择系统打开/文件管理器显示，不扩大 shell capability。
- Spec / Compatibility / Payload / Viewport / Observation / Release / AI Collaboration Harness；真实安装版验收由编排方发版后完成，本任务不发版。
- R1：无 cwd 上传、预览、发送；R2：Markdown、文件卡片、工具卡片与失败提示；R3：scheme 拒绝与可执行文件显示；R4：Windows 盘符、UNC、长路径与 macOS 路径；R5：先红后绿与浏览器明暗/窄屏验收。
