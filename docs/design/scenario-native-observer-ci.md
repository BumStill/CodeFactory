# 远端 Mac 原生控件观察：最小可执行切片

## 目的与边界

对应 CF-STG-R29 与 M1 桌面 feasibility probe。本机当前锁屏，不要求用户解锁、不绕过系统保护；使用一次性 macOS runner 验证独立的真实 Tauri App 能否被原生 Accessibility 观察，并能回收本次启动的进程。

本切片不是完整桌面验收。点击、输入、主题重开、截图、完整目录清理和请求/凭据访问计数尚未取得时，`full_probe.status` 必须保留 `blocked`。`observer_slice.status=passed` 仅说明本片列出的原生观察与进程回收成立，不提升 registry 或 L3/L4 状态。

## 流程与接口

1. 新增独立、非 required 的 `pull_request` 工作流，权限只有 `contents: read`。checkout 使用 PR 的精确 head SHA，关闭凭据持久化；不使用 `pull_request_target`，不读取或传入 secrets，不修改现有工作流与合并规则。
2. 先编译 Swift helper，执行只读 `preflight --driver <绝对路径>`；条件未证实时立即退出，不安装产品依赖、不构建 App、不创建 World。条件满足后，`run-macos-native-observer.mjs prepare` 在 canonical `/tmp` 的专属目录创建 Scenario World；私有 state、manifest 和 owner 不上传。生成命令级 Tauri override：唯一 identifier、内嵌前端、禁用 updater artifact。仓库配置与依赖不改。
3. 从同一候选源码构建真实 debug App 和 Swift 原生观察器。构建输入的 expected SHA 绑定 PR head，但本片尚未验证二进制内嵌身份，必须标为 `ci_input_unverified_in_binary`；候选二进制与复制到 World 的二进制摘要必须一致，不能用输入标签替代正式产物来源证明。
4. `observe` 只使用自己创建的进程及真实 OS 身份。它不能终止 LaunchServices 意外返回的另一个 App；每次 AX 观察与回收前重新验证 PID、birth token、executable path/digest、bundle identifier。没有验证的行为保留 unknown，不填假零值。
5. 上传只有匿名 `receipt.json`。必须校验精确 expected build SHA、observer slice 与 full probe 的不同状态；world 保留不能写成完整清理通过，未知请求/凭据次数必须为 null。runner 销毁也不能冒充用例清理能力。

job-level `env` 只使用该层合法的 `github.*` 上下文。构建之外的私有 state/raw/public 输出位于 checkout 的 ignored `.codefactory-cache` 中，与 `cargo-target` 并列，不进入 Rust target 缓存；输出父目录仍由 supervisor 逐层核验。#515 首轮 `34189620864` 实际产生 workflow 文件级启动失败，没有执行 App；普通 YAML 解析不能验证 GitHub 上下文可用性。已补 `runner.temp` 在 job env 中被拒绝的失败优先回归，改为显式 `github.workspace` 路径后再到远端验证。

#515 的 `9d837d75` 已实际完成预测试、真实 App 打包及 source-preserved 核验；run `34190155096` 在 observe 之前返回 `accessibility=true`、`screen_capture=true`、`gui_session=false`，退出 3。它证明窗口测试未执行，不证明远端锁屏，也不证明产品窗口不可达。本次将只读 preflight 前移至依赖安装和产品编译之前，匿名区分 session、控制台、登录、同用户及锁屏未知条件；仍在不具备条件时失败，不使用 unknown 代替 unlocked，也不改权限。`ready` 仅代表预检条件满足，不能替代后续实际 AX 和清理断言。

前移检查已先观察旧工作流缺少早期检查的失败，再合并独立诊断实现；本地 57 项 Python、39 项 Node 测试、Swift typecheck、治理基线及场景治理验证通过。纯 Swift 字典与 CLI fixture 不读取真实桌面，不作为 App 验收证据。

## 失败处理

- observer 缺权限或不能观察时退出 3，明确 `blocked`；身份/路径不安全或其他执行失败退出 2。公开回执校验器拒绝结果或发布失败时退出 1。
- 不使用 `continue-on-error`、不把 blocked 转绿、不上传 HOME、state、manifest、owner 或原始 AX 文本。observe 原始结果只保留在私有目录；独立校验器严格接受完整 schema 后原子发布匿名结果，任何拒绝只发布固定错误码。上传还必须取得本次安全发布后的 `public_receipt_ready=true`；目的地被替换为链接、发布失败或代码崩溃时没有该标记，即使旧文件存在也不能上传。安全错误码发布成功仍保留失败退出码，不能把失败测试改绿。
- Job 最长 45 分钟，同一 PR 的过期运行可取消；supervisor 在正常生命周期内具备阶段超时和 owned 主子进程回收，但自身被取消、SIGTERM 或强制终止后的主子进程、后代进程及目录清理尚未实现，不能计作已完成。临时 runner 销毁不能替代这些证据。
- 先以工作流合同测试观察缺文件失败，再加入实现。实际远端结果在 PR 中逐项核对，不能靠静态合同声称已经跑过 App。

## 协作与审查

产品隔离、原生观察器与 CI 分工开发、独立复查；合并前同步最新默认分支，保留原 six required checks。此处只新增诊断执行渠道，不把候选工作流当 trusted judge；正式接入统一场景门禁仍需后续已定义的信任链升级。
