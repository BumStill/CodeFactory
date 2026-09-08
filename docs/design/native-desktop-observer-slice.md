# 原生只读 AX 观察器与持有租约的 supervisor

本轮只实现真实 Tauri 的只读 AX 可达性和 main child 回收。必须复用经过审核的 Synthetic 产品入口；没有 AppKit demo，不更改 registry/judge/workflow。主机当前锁屏，本地只做 typecheck 和合成合同测试，真实运行留给独立 macOS CI。

## 接口

1. `node scripts/run-macos-native-observer.mjs prepare --state <私有state.json> --build-config <tauri-override.json> --expected-build-sha <完整commit>`：建立产品协议的 `/tmp` world，写独立 identifier、`devUrl:null`、`createUpdaterArtifacts:false` 的构建配置。私有 state/manifest/owner 不上传。
2. CI 构建真实嵌入式前端 `.app`，编译 `scripts/macos-native-observer.swift` 为独立 driver。
3. `node scripts/run-macos-native-observer.mjs observe --state <state.json> --candidate-app <本checkout构建.app> --driver <已编译driver> --receipt <私有raw-receipt.json>`：复制候选 bundle 至 owned world，绑定 candidate/copied/running executable digest；本轮仅允许显式 GitHub macOS CI 环境调用。raw 回执仅在私有目录落盘，由独立 verifier 生成可上传的匿名投影；不能上传 state、原始 helper 输出或整个 world。

prepare/receipt 缺失的父目录逐层创建为 0700，拒绝 symlink 父目录和现有输出文件；文件以 0600、O_EXCL/O_NOFOLLOW 创建，已有用户目录不 chmod。写入前后检查持有的目录 device/inode；这不是对恶意同 UID 并发修改者的完整沙箱。prepare 在分配 world 前预检输出，observe 在可能 launch 前预检 receipt。标准输出只包含 prepare 的 run/identifier、固定状态摘要，以及三个严格 boolean 的权限投影；不输出 owner token、绝对路径、原始 AX 或原始异常。

## 有界状态流

准备及目录身份核验 → 原生权限预检 → 直接启动指定 bundle 的 executable 并持有 ChildProcess → 取得 OS PID/birth、proc executable path/digest、bundle identifier → 只遍历该 PID 的 AX 树并返回已知控件布尔值 → 重新核验世界 inode 和实际进程身份 → SIGTERM/有界等待；必要时重新核验后 SIGKILL → 观察 ChildProcess 回收。

Swift driver 仅通过 stdin 接收私有请求。每个原生操作先核验 owner/manifest 字节摘要与目录 identity，然后读取 OS 进程 identity；AX 查询前后均复核，信号发送紧贴实际身份核验。错误 bundle、PID 复用、目录替换不获得信号授权。Node supervisor 给每个 driver 子调用与主要阶段设定超时；driver 卡住时终止的是 supervisor 刚创建的 helper，不向不明 App 发信号。异常只输出固定匿名原因码，原始 stderr/AX 文本不上传。

bundle 查询和 executable hashing 结束后，identity 必须再读取 OS birth/path；signal 边界在所有昂贵校验完成后再次独立读取并与 lease 对照，再调用 kill。使用纯闭包注入的 Swift 编译测试覆盖 hashing 期间 birth/path 改变、进程不可观测均为零信号，正常同身份才允许一次信号。这缩小并覆盖已知的长校验 PID 复用窗口，不宣称 POSIX kill 与用户态 birth 查询具备内核原子性。

Tauri/WebView 注册和渲染不与进程启动同步，因此首次空 AX 树不是最终结论。supervisor 使用单调时钟，在总计 8 秒内每 150ms 重试完整的只读快照；每次 helper 超时不超过 4.5 秒或剩余总预算。每轮 Swift 都重新核对同一 PID/birth/digest，单快照继续保留 512 节点、14 层、1.5 秒遍历及单消息 0.2 秒限制。外层阶段上限为 15 秒，覆盖重试总预算和 helper 回收余量。只有同一个快照同时看到 window 与设置控件才成功，不能把不同快照的零散结果拼成通过；身份核验错误立即向外传播，不能通过重试隐藏。

当前真实 launch adapter 同步调用 spawn 并持有返回的 ChildProcess；尚未实现异步 launch 超时后迟到子进程的回收协议。首次身份观察失败时不会向未知身份进程发信号，结果为 failed、world retained。supervisor 自身收到 cancellation/SIGTERM 或被强制终止后的 child 回收尚无证据，也不宣称已支持。这里的有界性针对正常 supervisor 生命周期内的 helper/AX 查询和最多两次信号尝试，不是全生命周期或所有后代进程清理保证。

## 分层结果与未完成项

`observer_slice.status=passed` 只表示实际取得本次 Tauri 主窗口的设置控件，并回收本次启动的 main child；退出 0 对应这一小层结果。权限或可观察性不足退出 3；安全身份或执行失败退出 2。GUI boolean 为 false 只表示可用 GUI 未证实，不能据此判定用户锁屏。

`full_probe.status` 仍为 `blocked`。主题点击、固定文本键入、重启持久化、截图、WebKit 后代进程清理和目录删除尚未实现。目录明确保留供排障，不伪称完成 cleanup。没有可观测来源的 `request_count` / `credential_access_count` 使用 null/unknown，不能填常量 0。后续须由审查过的 IPC/网络/credential 边界观测器或 OS 约束/审计证明实际范围；只依赖产品自报计数不够。

prepare 的 `expected_build_sha` 是 CI 提供的构建输入，记录为 declared build identity；运行 executable digest 是实际计算值。二进制内嵌 build SHA 的独立对账仍未实现，不能把参数当成已验证的嵌入式身份。任何后续 AX 写入、截图或删除都必须再次绑定实际进程与目录 identity。

## 本地验证与证据边界

本地执行 `node --test scripts/native-desktop-probe-contract.test.mjs scripts/macos-native-observer-supervisor.test.mjs` 与 `xcrun swiftc -typecheck scripts/macos-native-observer.swift`。macOS 上 Node suite 会用 `-D NATIVE_OBSERVER_CONTRACT_TESTS` 编译并运行纯闭包 signal fixture；该编译分支不会执行 App、OS 进程读取、AX、权限或真实 signal。普通 driver 仅 typecheck/链接编译，不运行 GUI。新增 supervisor 测试先于实现失败；私有目录创建、CI guard 失败回执、symlink 输出路径拒绝、严格 preflight 投影、AX 延迟可达和原生 signal 边界均另有先红后绿反例。适配器 fixture 测试只验证状态流和拒绝条件，不能替代真实 Tauri 观察；当前尚无本切片实际 App 运行成功证据。

完整缺项固定为九项：native_theme_click、native_text_input、restart_theme_persistence、owned_window_screenshots、descendant_process_cleanup、world_directory_cleanup、request_observation、credential_observation、embedded_build_identity。任意失败保留这个完整集合，不能把 blocked 改为 passed。prepare 后发生的安全失败会保留 world；本切片没有自动删目录命令。
