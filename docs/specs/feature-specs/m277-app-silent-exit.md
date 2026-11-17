# M277 应用静默退出 / 无人在场时的 dispatch（R1、R2 补交付）

## 背景
- 现场 2026-11-11：正常退出 CodeFactory 之后，`startup.log` 里只有 `step=setup`、`step=database`、`step=recovery` 这类启动阶段，**没有任何 `step=exit` 行**。也就是说应用「消失」了，但磁盘上没有任何一条记录说明它是怎么退出的。
- 同一现场：应用不在时执行 `codefactory --dispatch '<json>'`，直接得到 `Connection refused (os error 61)`。需要用户自己先把应用打开，无人值守流程因此断在这里。
- 本次补的是 R1 / R2；R3（stderr 断管不崩）已随 v1.120.0 交付，见 `stderr-broken-pipe-survival.md`。

## 需求

### R1 每次退出都留一条退出记录（本次补交付）
- 应用无论是被用户关窗、由代码请求退出，还是进程走到 `Exit`，都必须在 `startup.log` 追加一行：
  `<rfc3339 时间> step=exit reason=<退出原因>`
- `reason` 必须区分真实路径，不能只有一个笼统字符串：
  - `window-closed`：最后一个窗口关掉引发的退出请求；
  - `requested-exit-code-<n>`：代码/更新器等显式请求退出并给了退出码；
  - `exit`：进程真正走到退出事件（兜底，保证任何退出路径都有记录）。
- 记录写在数据目录的 `startup.log`，与既有 `step=setup|database|recovery` 同一文件、同一格式，便于编排方用一条命令检查。
- 退出路径不得因此阻塞或崩溃：写日志失败不得影响退出本身。

### R2 `--dispatch` 遇到连接被拒时自动拉起应用并重试一次（本次补交付）
- CLI 客户端首次连接失败且失败原因是「应用没在监听」（socket 不存在 / `ECONNREFUSED`）时：
  1. 在后台拉起应用（同一个可执行文件、不带参数、脱离当前终端会话）；
  2. 有界等待入口就绪（socket 出现且可连接）；
  3. 就绪后**重试一次**原请求，成功即按原有 JSON 契约输出并返回原退出码。
- 只对「连接不上」重试。请求已经送达、只是返回错误（业务失败、格式错误回复、连接被中途重置）一律不拉起、不重试——否则一次坏请求会把应用反复打开。
- 拉起+重试后仍失败时，错误信息必须写明「已尝试在后台拉起应用并重试」，并带上两次失败原因，用户不必猜是不是自己少开了应用。
- 边界：只在 Unix（socket 入口）路径；Windows 命名管道不在本单。

## 测试矩阵
- T1（R1）：`record_exit(dir, "window-closed")` 之后 `startup.log` 含 `step=exit reason=window-closed`，且既有阶段行不受影响。
- T2（R1）：不同 reason 写出的行可区分（`requested-exit-code-3`、`exit`）。
- T3（R2）：首次连接被拒 → 拉起恰好一次 → 就绪探测通过 → 第二次请求成功返回，退出路径不报错。
- T4（R2）：两次都失败 → 错误信息包含「拉起并重试一次」的措辞与两次原因，拉起次数为 1、请求次数为 2。
- T5（R2 反向）：非连接类错误（如回复格式错误）→ 不拉起、不重试。
- T6（R2 反向）：首次直接成功 → 不拉起。

## 验证证据
- `cargo test --lib m277` → T1–T6 全绿；红灯先于实现（见 PR 讨论里的失败输出）。
- `cargo test --lib startup_recovery`、`cargo test --lib headless_dispatch` 回归不退化。

## Implementation Notes
- 退出记录复用 `startup_recovery::record_exit`，写入与 `record` 同一 `startup.log`；`lib.rs` 用 `Builder::build(context)` + `run(|app, event|)` 接管 `RunEvent::ExitRequested` / `RunEvent::Exit`。
- 拉起/重试逻辑抽成纯函数 `dispatch_with_launch(socket, payload, request, launch, ready, timeout)`，`request` / `launch` / `ready` 由调用方注入，测试用闭包注入，不依赖真实应用进程。
- 拉起实现：`std::process::Command::new(current_exe())`，无参数，stdin/stdout/stderr 接空设备，Unix 下 `setsid` 语义（`pre_exec`）脱离终端。
- 就绪探测：有界轮询（默认 60s，100ms 间隔）socket 路径可连接；超时视为拉起失败并进入「已尝试拉起」错误分支。
