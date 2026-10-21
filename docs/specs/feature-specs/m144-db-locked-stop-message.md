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
