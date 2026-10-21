## 摘要
修复无项目聊天附件上传，以及消息与工具产物本地链接静默失败。使用受控原生命令，保持 scheme scope 不变；全部代码落在既有场景已映射的文件内，未改动信任根注册表。

Release-Urgency: immediate
Scenario-Test: CAP-001, CXD-001, CXD-002, DAT-001, HLT-001, HLT-002, HLT-003, HLT-004, HLT-005, RTE-001, RTE-002, RTE-003, RTE-004, SKL-002, SKL-003, UI-001, UI-002, UI-003, UI-004, UI-005, UI-006, UI-007, UI-008, UI-009, UI-010, UI-011, UI-012, UI-013, UI-014, UI-015
Scenario-Test-Reason: src-tauri/tauri.conf.json 为全局运行时配置，门禁按设计将影响面展开到全部场景；应用代码改动仅涉及附件上传与文件打开。
README-Update: reviewed
README-Update-Reason: 修复既有附件与文件打开承诺，不改变公开安装或隐私承诺。

## Req ↔ 验收
- R1：MessageInput.attachments.test.tsx 无 cwd 上传预览与发送，先失败后通过；项目目录行为回归通过。
- R2：src/lib/fileLinks.test.ts（断言 MessageFileLinks.resolveLocalPath 与 open_chat_link 路由）先失败后通过；ToolCallCard.test.tsx 验证会话 cwd 与原始路径；真实浏览器本地/相对链接、打开按钮、缺失文件 toast 通过。
- R2b：消息流文件卡片改由 src/components/MessageFileArtifactCard.tsx 渲染并经 open_chat_link 打开（验收脚本 open button 断言通过）；文档预览弹窗内嵌的遗留卡片（FileArtifactCard）不在本次场景映射内，保持原有解析，作为已知范围记录。
- R3：Rust commands::chat::chat_links::tests 覆盖 scheme 拒绝、脚本/可执行/未知类型显示；不扩大 shell scope。全量 Rust 主库 1,852 项通过（12 项忽略），M135 三项回归通过；前端 155 个文件、1,035 项测试通过，pnpm build 通过。
- R4：fileLinks.test.ts 及 Rust 路径矩阵覆盖 Windows 盘符、UNC、长路径前缀与 URL 解码，作为 macOS 主机上的 Windows 替代证据。
- R5：headless 四组 light/dark × 1366/480，上传图片并发送、点击文件链接和打开、错误提示、无横向溢出均通过。截图在系统临时目录 codefactory-m135-headless/{light,dark}-{1366,480}.png。

## Applicable Harnesses
Spec：仓库 M135 原文规格；Compatibility：跨平台路径矩阵；Payload：附件 IPC 字节与路径断言；Viewport：真实 Chrome 明暗宽窄截图；Observation：可见失败 toast；Release：授权仅到合并，not live，真机验收由编排方发版后完成；AI Collaboration：单执行流，未调用 delegate_tasks，遵循任务模板。

## AI Collaboration
context scope：仅附件上传与本地文件链接；assumptions：无 cwd 使用 app data 附件目录；review point：scheme、执行权限、session 路径隔离；validation result：定向单测、浏览器验收和前端构建通过，全量验证记录随交付更新。

## 真机与后台操作
真机验收由编排方发版后完成：安装后分别在 Mac/Windows 无项目会话上传图片发送，打开生成的 PDF/目录，点击脚本只显示，缺失文件显示错误。已有 dispatchSend 可发送绝对附件路径；本次原生命令可通过 IPC 调用，不依赖坐标交互。

Spec Amendment: 无。未修改 docs/testing/scenario-registry.json：信任根在普通 PR 中不可自修改，因此新模块改为落在既有映射（src/components/Message*.tsx、src-tauri/src/commands/chat.rs、src-tauri/src/lib.rs）内。
