# 崩溃记录：stderr 管道被关闭时进程直接 abort

## 现场
- 2026-11-10 22:24:56（本地），v1.114.0 连续写下两条崩溃记录，同一秒：
  - `panic-20261110-142456.853.log`，线程 `tokio-rt-worker`
  - `panic-20261110-142456.854.log`，线程 `main`
- 两条记录的内容相同，都不是业务失败：

  ```text
  message: failed printing to stderr: Broken pipe (os error 32)
  location: library/std/src/io/stdio.rs:1168:9
  ```

- 也就是说：应用不是「报了个错」，是**进程消失**。用户看到的是 CodeFactory 自己退出，没有任何可读原因，崩溃记录里也只有一行「写 stderr 失败」，真正的起因被覆盖掉了。
- 触发条件很普通：启动它的那一方把管道读端关了（应用自己 spawn 的子进程入口、驱动脚本超时退出、终端标签页被关掉），下一次 `eprintln!` 就落在死管道上。

## 原因
- Rust 的 `print!` / `println!` / `eprintln!` 在底层写入失败时**panic**，std 的 `stdio.rs` 把 `EPIPE` 变成上面那句 message。
- release 构建是 `panic = "abort"`，所以一次 panic 直接 `abort()`，没有恢复机会。
- 我们自己的崩溃钩子把记录写盘后还会把信息再打印一遍到 stderr；那个打印也会失败，于是「写 stderr 失败」顶替了真正的 panic 原因（本条现场就是这样丢的）。

## 需求
- R1 启动即修：进程一开始就把「读端已经没了」的 stdout / stderr 重定向到空设备，使后续所有打印都不可能 EPIPE。健康流（终端、日志重定向到文件、有读者的管道）行为完全不变。
- R2 只在坏的时候动手：只处理 `S_IFIFO`（管道）且 `poll` 报 `POLLERR` / `POLLHUP` 的描述符。不得把有读者的管道重定向掉——CLI 的 stdout 契约（smoke / dispatch 客户端输出的 JSON）由驱动脚本解析，重定向等于静默丢结果。
- R3 入口覆盖：`src-tauri/src/main.rs`、`src-tauri/src/unattended_smoke_cli.rs` 都属于 scenario gate 冻结的信任根，普通 PR 不得改动。因此守卫改为**加载期安装**（`.init_array` / `__DATA,__mod_init_func`），在任何 `main` 之前执行，天然覆盖全部入口与未来新增的二进制；`run()` 里另有一次幂等调用作为兜底。安装点零信任根改动。
- R4 崩溃记录不再被覆盖：守卫在加载期就把无读者的 stderr 改指向 null 设备，钩子里写盘后的 stderr 副本因此能正常完成，磁盘记录保持权威（不再需要修改 `panic_log.rs`）。
- R5 边界（明确不声称）：只修「启动时读端已经不在」这一种；启动之后才被关掉的管道仍会到 panicking 宏（除非换掉全部打印宏，超出本单）。仅 Unix；Windows 控制台/管道句柄要另一种修法，本单不覆盖。

## 测试矩阵
- T1（R2）：活的管道 `reader_is_gone` 为假、`detach_if_broken` 不动它。
- T2（R1/R2）：读端关闭后 `reader_is_gone` 为真。
- T3（R2）：普通描述符（`/dev/null`）永不被改写。
- T4（R1，缺陷本体）：读端关闭的管道写入返回 `-1` 且 errno 为 `EPIPE`（这就是崩溃来源）；修复后同一描述符写入成功、字节数一致。
- T5（端到端）：真实二进制在「stderr 读端已关闭的管道」下启动，退出码不是 abort，且不产生新的崩溃记录。

## 验证证据
- T1–T4：`cargo test --lib std_stream_guard` → 4 passed。
- T5：把已构建二进制的 stderr 接到一个**读端在进程启动前就已关闭**的管道，参数给错以强制它立刻打印 usage 并退出：
  - 死管道：退出码 `2`（正常用法错误），不是 panic/abort；说明写出错已被丢弃。
  - 健康管道：同样是 `2`，且 stderr 仍完整收到 `usage:` 文本 —— 守卫没有静默吞掉 CLI 输出。
- 为什么这一条能区分：debug 构建下 `eprintln!` 遇 EPIPE 会 unwind 成 panic（退出码 `101`），修复缺失时不可能得到 `2`。

## Implementation Notes
- 代码：`src-tauri/src/lib.rs` 内联 `mod std_stream_guard`（加载期安装 + `run()` 一次幂等调用）。未改动 `src-tauri/src/main.rs`、`src-tauri/src/unattended_smoke_cli.rs`、`src-tauri/src/panic_log.rs`、`docs/testing/scenario-registry.json` 等信任根或已映射文件之外的任何产品文件。
- 关掉 SIGPIPE 不是解法：Rust 已经把写入变成 `EPIPE` 错误，问题在于宏把这个错误升级成 panic。
- 没有选择的方案：把 99 处 `eprintln!` 逐一换成非 panicking 写法。改动面大且覆盖不到第三方打印；启动期修复能一次性解决「读端已死」的整类情况。
