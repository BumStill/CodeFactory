# M143（P0，用户报告）：Windows 上命令因 PowerShell 5.1 不支持 && 而失败，输出乱码

## 证据（用户 10-21 截图 /Users/leo/.codefactory/agent-tools/m143-win-shell.jpg，Windows 中文系统）
- 后台任务命令 `cd "…execution-workspaces\…" && npm ins…` 报 `ParserError: InvalidEndOfLine`（Windows PowerShell 5.1 不支持 `&&`/`||`）。
- 输出尾部与日志为乱码 `����λ�� ��:1 ��: 124`、`��ā�&&������Ǵ汾…`：中文系统控制台输出为 GBK/CP936，被按 UTF-8 解码。退出码 124（超时）。
- 设置默认 shell：`src-tauri/src/config/settings.rs:629` Windows 为 `"powershell"`（即 5.1）。
## 需求
- R1 Windows shell 选择：优先 PowerShell 7（`pwsh`，支持 `&&`/`||`），检测不到再用 Windows PowerShell 5.1；工具描述/系统提示里如实告诉模型当前 shell 与版本以及语法要点（5.1 时：用 `;` 与 `if ($?) { … }` 代替 `&&`，路径用双引号，环境变量 `$env:NAME`）。对 5.1 可选：把顶层 `a && b`/`a || b` 安全改写为等价的 `a; if ($?) { b }`（只在顶层、不在引号内），并在结果里注明已改写。
- R2 输出编码：执行前设置 UTF-8 输出（PowerShell：`[Console]::OutputEncoding=[Text.UTF8Encoding]::new(); $OutputEncoding=…`；cmd：`chcp 65001`），并在解码侧对非 UTF-8 字节回退到系统代码页（GetACP/CP936）解码；前台命令与后台任务日志都要一致。
- R3 失败可读：命令语法错误时，工具结果给模型一句明确的修复建议（例如「当前是 Windows PowerShell 5.1，不支持 &&」），不要只有乱码。
- R4 测试（先失败）：Windows CI（execute-windows）上：`echo 中文` 输出正确；`cmd1 && cmd2` 在 pwsh 可用时执行、在 5.1 下被改写或给出明确提示；后台任务日志中文不乱码。PR 写明 Windows 实测证据。
## 约束
- 授权交付（ceiling: merged）。不要超出规格自加范围。

## 新增需求（用户补充，R5–R7）

- R5 非交互执行：所有命令（前台与后台任务）stdin 关闭或接 NUL；设置非交互环境（CI=1、npm_config_yes=true、GIT_TERMINAL_PROMPT=0、GCM_INTERACTIVE=never、PowerShell 用 -NonInteractive -NoProfile）；检测到等待输入的提示（如 "Press any key"、"(y/N)"、Read-Host）时尽早结束并告诉模型改用非交互参数。
- R6 超时与进程树清理：Windows 上用 Job Object 管理子进程，超时或停止时整棵进程树一起结束（node/npm 子进程不残留），状态及时从「正在执行」变为结束并带原因（超时/被停止/等待输入）。
- R7 卡住可见：命令运行超过阈值（例如 5 分钟无新输出）时，界面显示「卡在 <命令摘要> 已 N 分钟，无输出」并提供「停止」；模型侧收到超时结果后能继续，而不是整个回合挂起。
- Windows CI 验收：等待输入的命令被非交互化或及时结束；超时后无残留子进程；回合在命令超时后继续。

## 实施与验收补充

- 前台读取输出时维护最后输出时间；后台保留直接日志输出，通过文件修改时间检测静默和输入提示，保留重启兼容。
- 只识别输出末尾的输入提示，不因普通命令包含这些词就终止。返回明确“等待输入”原因与非交互参数建议，不绕过失败。
- 前台停止单个命令，返回工具错误供同一回合继续；后台沿用现有 stop 工具和会话归属校验。均提供无 GUI 操作通道。
- R5 测试：真实进程读取 stdin 得到 EOF、环境变量断言、输入提示命令被尽早终止。
- R6 测试：Windows Job Object 超时/停止清理父子进程；现有超时结果与后续执行集成验证。
- R7 测试：真实最后输出时间、静默阈值（300 秒）、停止动作；会话头部活动条把静默命令显示为「卡在 <命令> 已 N 分钟，无输出」，停止按钮走同一 `stop_session_background_task` 入口；前后台 UI 回归（`SessionBackgroundTasks.test.tsx`）及真实浏览器明暗/窄屏布局。
- Applicable Harnesses 追加 Viewport；不增加发布范围。

## Implementation Notes

- 不自动改写 PowerShell 语法，避免引号、脚本块及退出码语义变化；5.1 解析失败提供明确提示。
- 前台及后台复用 shell 选择、UTF-8 初始化及代码页解码。后台保留直接写文件以支持进程重启后持续输出，读取日志时统一解码。
- Requirements Traceability：R1 → shell 选择与描述测试；R2 → UTF-8/CP936 解码、前台和后台真实进程测试；R3 → 5.1 语法失败诊断测试；R4 → Windows runner 执行上述测试。
- Applicable Harnesses：Spec、Compatibility、Payload、Viewport（R7 会话活动条）、Observation、Release（交付至合并，未发版）、AI Collaboration。
- AI Collaboration：context scope 为命令执行和后台日志；assumption 为 Windows 标准 PowerShell 主版本 5/7；review point 为退出码和后台持久输出不变；validation result 在 PR 中记录。任务模板禁止 delegate_tasks，因此单执行流。
