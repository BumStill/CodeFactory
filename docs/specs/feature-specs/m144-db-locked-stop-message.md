# M144（用户报告）：①「继续」后弹出 database is locked ②停止提示泄漏内部用语且重复、与交付无关也出现 ③write_file 收到 null

## 证据（用户 10-21 截图：/Users/leo/.codefactory/agent-tools/m143-win-shell.jpg 底部、/Users/leo/.codefactory/agent-tools/m144-stop-msg.jpg，Windows）
- 用户发「继续」后对话里出现 `Error: Database error: error returned from database: (code: 5) database is locked`（同时有命令在执行）。
- 一个与代码交付无关的任务（知识整理/写脚本）停止后，对话里**连续两次**出现：「已停止：远端还没有本次交付的 PR，停止之后不再执行还没发生的交付动作（推送、开 PR、合并、发版）；已经发生的本地提交不…要接着交付，需要你或编排方明确发起。停止时远端还没有本次交付的 PR，没有需要关闭的自动合并」（来源 src-tauri/src/agent/stop_fence.rs:202–205）。「编排方」是内部用语。
- 同截图顶部 `Invalid arguments for write_file: invalid type: null…`。
## 需求
- R1 SQLite 锁：所有写路径使用统一的忙等待与有限重试（WAL + busy_timeout 已有处查明为何仍报 code 5：是否有长事务/未用同一连接池/Windows 文件锁更敏感），高频写入合并或串行化；任何情况下 `database is locked` 不得作为对话消息展示给用户——用户操作（发送/继续）应在后台重试成功，真失败时给一句人话。
- R2 停止提示：仅当本会话确实有交付动作（存在交付映射/推送/PR）时才提及交付；文案改为面向用户的一句话（例：「已停止。没有推送或创建 PR；本地改动保留在工作区。」），去掉「编排方」等内部词；同一次停止只显示一次（查明为何出现两次并修复）。
- R3 write_file 参数为 null/缺字段：返回给模型清晰的可修复提示（哪个字段、期望类型），不作为用户可见错误刷屏。
- R4 测试（先失败）：并发写压力下发送消息不出现 code 5 给用户；无交付的会话 stop → 一条不含 PR/编排方的提示；有交付的会话 stop → 一条准确提示；write_file null → 工具错误提示具体字段。
## 约束
- 合成 fixture。授权交付（ceiling: merged）。不要超出规格自加范围。

## Implementation Notes

- 采用既有 `storage::write_lock::begin_write`（BEGIN IMMEDIATE、有限重试）修复发送准入的快照升级竞争；日志保留驱动诊断，用户边界输出可读说明。
- 停止栅栏始终保留，不因无交付而削弱；提示与审计 notes 分离，按当前 root turn 建立幂等消息身份。
- Harness：Spec、Compatibility、Observation、Release（仅合并，不发版）、Payload、AI Collaboration；若前端展示路径发生变更追加 Viewport。
- R1/R4：合成 SQLite 竞争与发送准入回归；R2/R4：无交付、有交付、重复停止及重新执行再停止；R3/R4：null、缺字段、错误类型、合法空文本。
- 真机验收由发版安装后的编排方完成：Windows 中并发命令期间发送“继续”，停止无交付/有 PR 会话，检查提示计数及 write_file 错误仅进入工具结果。

## P0 追加（用户 2026-10-21 补充）：一次 code 5 后永久不可继续

Windows 现场：一旦出现 `database is locked`，之后每次发送/继续都失败，只能重启应用。根因不是瞬时争用，而是写锁被长期持有。

### 需求
- **R5 无泄漏事务**：禁止用 `execute("BEGIN IMMEDIATE")` 开启事务（sqlx 无法追踪，提前返回/取消会把未结束事务随连接还池）。全部改为 RAII `Transaction`（`begin_write` / `Connection::begin_with`）。
- **R6 归还即清理**：连接池 `after_release` 检查 SQLite autocommit；发现未结束事务立即 `ROLLBACK` 并告警，单次故障不得演变为永久 outage。
- **R7 有界等待**：`acquire_timeout` 限制取连接等待；`busy_timeout` + 有限重试仍保留。连接全部被占时必须报错而不是无限挂起。
- **R8 事务不跨外部 I/O**：模型请求、子进程、网络期间不得持有写事务；`scripts/validate-write-transaction-scope.mjs` 作为可执行门禁。
- **R9 回归**：模拟泄漏事务归还后，后续写入必须成功且脏写被回滚；零超时制造一次真实 code 5，随后同一池的写入必须成功；8 写者 × 5 轮并发在 Windows CI 上全部提交。

### 实现
- `pool_builder()` 统一生产/测试连接池：`after_release` 清理 + `acquire_timeout(30s)`；`pool_connect_options` 保持 WAL/FULL/FK/10s busy timeout。
- `ensure_objective_failed_status` 的 schema 重建改用 `conn.begin_with`，失败或取消自动回滚。
- `ObjectiveStore`、`delivery_run`、`persistence`、`thin_derive` 的 31 处写事务统一改走 `begin_write`（BEGIN IMMEDIATE + 重试）。
- 多池/多进程核查：生产侧写主库均通过 `storage::db::connect`，同一套 pragmas；`--dispatch` 不另开主库池。
