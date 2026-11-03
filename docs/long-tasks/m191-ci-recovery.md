# M191 / PR #923：CI 恢复记录

## 本轮诊断
- context scope：PR #923 head `da67938db3c543a76d3e30e0f9b488f28dbd7ab5` 四个失败 job 的实际日志；仅合成 fixture，不读生产知识库。
- `check-rust` job 123584557353：`historical_schema_matrix_preserves_identity` 在 M187 live executor smoke 中收到本地 `127.0.0.1` HTTP 请求错误。
- `execute-windows` job 123584679373：同一历史会话 smoke 的子进程返回 `channel closed`。
- `receipt` job 123593885970：缺少 `binary:--history-session-smoke` 成功回执，是前项失败的下游结果，不伪造或放宽 receipt。
- `scenario-gate-pr` job 123584543713：唯一错误是 `docs/testing/scenario-registry.json` 的 `cannot self-modify`；不得改 judge、规则集或把知识库路径硬贴到无关场景以伪装覆盖。

## 修复范围
- 同步队列已经更新过的同一远端分支，再并入最新 `origin/main`。
- main 已含本地 synthetic HTTP provider 的修复：读超时在总 deadline 内继续读取，并给首请求通道关闭返回明确诊断。保留请求完整性检查与真实 AgentLoop/tool/持久化断言。
- 不重写原测试以规避失败，不改 workflows、validator 或 ruleset。
- assumptions：旧 Windows 失败与 synthetic HTTP fixture 的读取超时一致；必须用本轮 CI 结果确认，不能用 macOS 通过代替 Windows 通过。
- review point：四项日志分别诊断；合并后原失败集成测试与正式 binary smoke；推送后 exact-head Windows 及回执。

## 验证与后续
- 合并后本地结果：`pnpm test` 163 文件 / 1088 项通过；`pnpm build` 通过；`history_schema_matrix` 原失败集成测试通过；源码目录外真实 binary `--history-session-smoke <receipt>` 返回 `ok=true`、`process_restart_count=14`、`cleanup_ok=true`。首轮独立 binary 调用遗漏回执参数退出 2，补齐后按同一入口重跑通过。
- Windows 与 exact-head 回执仍以新 CI 为准；macOS 本地通过不能冒充 Windows 已通过。
- scenario registry 的新 KB-001 定义仍需编排方审查后按既有信任根流程处理；本执行者不临时摘 required check，也不手工合并。
- 用户授权仅到 merged，不执行发版。
- CI 全绿后保持 `merge-queue: armed`，移除 `merge-queue: blocked`；由队列合并。
- 合并后按 task-template §8 核实 merged、main 上 squash commit、远端分支和本地清理；真实安装版功能验收由编排方发版后完成。
