## 修复 M188

数据库遗留回合/工具行不是活 executor。启动时先原子结算孤儿回合，再结算已合并会话，防止旧会话被恢复为 running。周期检查修复无 executor 的 running 投影并记录告警。

Scenario-Test: HLT-004
README-Update: reviewed
README-Update-Reason: 内部状态恢复修复，不改变公开安装、隐私或平台承诺。

## Req 与验收
- R1/R2/R4：`m188_cross_process_orphans_settle_and_live_bash_survives`，先失败（Running != Ended）后通过；18 条孤儿 active 回合原子写为 orphaned_by_restart；幂等重入；真实 bash 执行期间保持 running，实际输出 M188-LIVE 后结算。
- R3：`m188_running_requires_executor_not_cached_or_durable_activity`；周期扫描无 executor 的 running 并告警。
- R5：真机验收由编排方发版后完成：核对指定 14 个会话 ended；对照 running 行与实际 executor；执行 verify-liveness。
- M187 保护测试修正：跨进程遗留行本身不能构成活性（M188 R1），改为在当前进程注册真实 executor，保留执行期间禁止终结的断言。

## Harness 与协作
Spec / Compatibility / Observation / Release / AI Collaboration。合成 SQLite + 真实子进程；不涉及 UI 改动，无需布局截图。context scope：thin 状态派生与启动恢复。assumptions：持久行不证明活性。review point：启动先对账再恢复。validation result：核心跨进程回归先红后绿；前端 159 文件 / 1056 测试通过，pnpm build 通过；正式 cargo build 通过；源目录外 `--history-session-smoke` 通过（14 次重启、4 种旧 schema、真实 HTTP SSE + bash、孤儿控制行清理，字段断言 ok/cleanup_ok 均 true）；场景统一门禁、治理基线、场景正文与 README 校验通过。最终 Rust 全工作区回归结果以交付前运行记录为准。

无待决 Spec Amendment。仅合并，本任务不发版，not live。独立审查任务为只读，不接触生产数据库。

Release-Urgency: immediate
