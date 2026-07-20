# CodeFactory 后台服务生命周期锁屏验收证据

## 结论

- proof tier: `agent-runtime-no-gui` + desktop bash integration
- product path: CodeFactory `product` policy、共享 Rust `codefactory-agent-core`、Rust headless sidecar、桌面 bash tool runtime
- screen state: runtime 开始与结束均为 `CGSSessionScreenIsLocked=Yes`
- model backend: `deepseek / deepseek-v4-pro`
- task type: 非 Terminal-Bench 的本地 HTTP 服务启动、PID/日志记录、跨调用 readiness 与存活验证
- contamination: 未使用 benchmark task name、答案、verifier、solution 或 task-specific repair

## Failure-First 发现

第一次验收错误返回 `passed`，但 trajectory 中所有 socket bind 都因 sandbox network policy 返回 `Operation not permitted`。共享 core 当时没有识别同一行中间的单 `&`，也没有把零退出输出里的 `failed:` 作为 semantic failure，因此 service sequence 全为空却允许完成。

第二次验收允许本地 network 后服务真实运行，但又暴露：错误 `cd` 可被末尾 `echo` 掩成零退出；timeout 的 service start 不会激活 lifecycle gate。对应 failure-first 测试均先变红，再由共享 core 修复。

## 最终锁屏运行

- evidence dir: `.codefactory/product-acceptance/service-v151-r3`
- status: `passed`
- duration: `46,728 ms`
- tool calls: `9`
- model requests: `11`
- token usage: prompt `37,691`，completion `2,372`，total `40,063`
- completion blockers: `[]`
- service start sequence: `6`
- service log evidence sequence: `8`
- service PID evidence sequence: `9`
- bounded functional probe sequence: `9`
- final functional evidence: `curl -s --max-time 5 http://127.0.0.1:18765/health` 返回 `ready`，同一后续调用确认 PID `93737` 存活
- acceptance-run headless binary SHA-256: `de16642f5a374196c048844fc3826eab5cbd788440e6a2d1626271f41c528297`
- current release-candidate headless binary SHA-256: `15a0f96ee12015b21a6e6aca3c1926a9772d7f70da35f879d7167d0516daee10`
- execution contract SHA-256: `6ba5e53e23d56459fb63afaf5d5ac3cd8d2561127ffa4eb73a74b0e4f4ff7f3b`

## 桌面工具独立证据

`tools::bash::tests::background_service_survives_after_the_shell_tool_returns` 直接调用 CodeFactory 桌面 bash tool，启动重定向日志的后台 `sleep`，在工具调用返回后通过 PID 确认进程仍存活，再精确清理该 PID。`background_service_without_redirect_cannot_hold_output_pipes_forever` 覆盖后台进程同时继承两侧输出管道；`background_service_holding_only_stderr_preserves_completed_stdout` 覆盖仅 stderr 未关闭，确认工具有界返回、保留已完成 stdout 且后台 PID 仍存活。对应 timeout 测试同时确认超时命令的后代不会存活。Harbor bridge 另有 Linux-only CI 测试真实执行 Bash 专属语法，并实际启动和清理 `setsid` 进程组；macOS 本地因系统不提供 `setsid` 跳过这两项，由 PR Linux runner 提供最终证据。

两轮独立审查提出的单侧 reader、workspace redirect 伪验证、失败/元数据型源码编辑、Bash 降级、preflight fail-open、预期负向状态误判和 model deadline 等问题均已先由新增测试复现失败，再由共享产品路径或评测基础设施修复。第二轮进一步覆盖无空格/fd 前缀重定向、复合命令扩大 absence 豁免、以及失败 compound edit 无法证明编辑阶段成功三个反例。当前本地全量结果：桌面 Rust `375 passed / 6 ignored`，共享 core `73 / 73`，headless `15 / 15`，Python `85 / 85` 且 Linux-only `setsid` 两项在 macOS 跳过；前端 `226 / 226` 和 production build 通过。最终 Linux-only 结果仍以 PR CI 为准。

## 证据边界

- no-GUI acceptance 使用 macOS `sandbox-exec`，sandbox runtime 结束后不会用于证明服务继续存活；跨工具调用到结构化 `Finished` 的存活已验证。
- 桌面 bash integration 证明主产品工具返回后的保活语义，但不是 GUI 截图证据。
- Unix 进程组语义已有本地和 Linux CI 门禁；Windows 当前仍使用 `taskkill /T /F` best-effort tree cleanup，尚未以 Job Object 给出同等级严格保证，本轮不把 Windows detached descendant 声明为已完全证明。
- 远端 macOS 可见窗口、PR CI、合并、发布 artifact 和发布版本复评仍是产品化门禁。
