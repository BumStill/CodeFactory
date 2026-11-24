# M275：系统收尾要认出压缩合并，且不得重开历史会话

## 规格（原样，来自任务文件）

### 现场（2026-11-11 01:35 起，v1.116.0，M273 刚上线）

- 安装 v1.116.0 后，系统收尾逻辑扫到了很早以前就结束的任务会话（11-01 的 #767、#769、#771，11-06 到 11-09 的 #1007、#1010、#1027、#1031、#1037、#1044、#1048、#1050、#1052），把它们重新打开。
- 有的会话被收尾唤醒，起了新的模型回合，申请 `rm -rf`（73ddd1b2，#1010）。
- 有的会话被写成 `waiting_user`，问题是：「PR #N 已合并，但系统自动收尾没有完成：工作区还有 K 个未推送且未被其他分支保存的提交，系统自动重试/自愈：保留工作区。需要你决定」。
- 这是误报：这些 PR 都是压缩合并（squash），代码已在 main 上；本地分支上的原始提交本来就不会出现在远端。编排方逐个确认 PR 已合并后 stop 了这些会话。

### 规格

- R1：判断「本地提交是否已保存」时，要认出压缩合并：本地分支相对合并基点的改动内容已经包含在默认分支里（比较树或补丁内容，不只看提交号），就算已保存。写清判定方法和依据。
- R2：系统收尾不能为了收尾去问用户。确实有内容没进默认分支时，写清楚是哪些提交、在哪个分支或备份引用里，会话以结束状态写明结果；不准进入 `waiting_user`，也不准写「系统自动重试/自愈……需要你决定」这种中间态。
- R3：只对新版上线之后结束、或仍在进行中的会话做收尾。已经结束的历史会话不能被重新打开、不能起模型回合。如果要补清理历史残留，只做系统侧的分支和目录清理，不改会话状态，不写消息。
- R4：收尾本身不起模型回合，除非会话本来就在等这个收尾。
- R5：M241、M251、M255、M259、M262、M264、M273 的测试继续通过。

### 测试

- 真实 git：分支被压缩合并后，本地有原始提交，判定为「已保存」。先看到失败。
- 历史上已经结束的会话，升级后不被重开，消息数不变。
- 确有未合并内容时，会话以结束状态写清楚，不进 `waiting_user`。

### 验收（真机，编排方发版后做）

安装新版后，已结束的旧会话状态和消息数不变；下一单合并后正常收尾。

## 决策

| 决策 | 内容 | 依据 |
|---|---|---|
| 收尾范围边界 | `thin_session_state.state='ended'` 的会话只有 `updated_at >= closeout_scope_boundary.installed_at` 才进入收尾候选；`waiting_wake`/`running` 不受此限制 | R3：只处理新版上线之后结束、或仍在进行中的会话 |
| 边界只写一次 | 新表 `closeout_scope_boundary(id=1, installed_at)` 在首次收尾扫描时 `INSERT OR IGNORE`，此后不再前移；首次写入回看 10 分钟，覆盖「进程已起、首次安全 tick 前自己结束」的会话 | 多次重启不能让「上次运行中结束、尚未收尾」的会话掉出范围；天级历史残留仍被排除 |
| 压缩合并判定 | 先看树，再看补丁：`git diff <default> HEAD` 为空即已保存；否则比较 `<merge-base>..HEAD` 的聚合补丁与默认分支自 merge-base 起每个非合并提交的补丁 `patch-id`，命中即已保存 | R1：压缩合并保留内容、丢弃提交号；GitHub squash 提交的补丁内容等于分支聚合补丁 |
| 默认分支解析 | 依次尝试 `origin/HEAD`、`origin/main`、`origin/master`、`main`、`master`；都取不到才退回旧的「提交号可达」判定 | 真实安装有 origin；测试夹具可能只有本地默认分支 |
| 收尾未完成不提问 | 工作区脏、内容未进默认分支等确定性失败：写清提交/分支，直接以 `ended` 写明「收尾未核验：…」，取消唤醒，保留现场（工作区、分支、远端分支） | R2：收尾不能为了收尾去问用户；不准出现 `waiting_user` 与「需要你决定」中间态 |
| 技术性扫描失败仍重试 | closeout worker 超时属于系统自愈范围，保留状态等下一次 tick，不写用户问题 | 治理：技术状态用「系统自动重试/自愈」 |
| 历史残留 | 不主动清理升级前已结束会话的工作区；它们不进入候选，状态、原因、消息一律不改 | R3：历史会话不能被重新打开；「如果」补清理才允许系统侧清理，本任务选择不清理以免升级时自动删除旧工作区 |

## Requirements Traceability

| Req ID | 要求 | 最低证据 |
|---|---|---|
| R1 | 压缩合并后的本地提交视为已保存，判定比较树/补丁而非只比提交号 | 真实 git 测试 `m275_squash_merged_branch_content_counts_as_saved`：远端分支已删、main 已前移，closeout 仍成功；旧实现下该测试失败 |
| R2 | 收尾不提问；未保存内容以结束状态写清提交与分支 | `m275_unsaved_content_ends_session_without_asking`（ended、reason 含分支与提交、无 question、无 pending wake）与改写后的 `r2_dirty_workspace_asks_user_with_a_reason` |
| R3 | 历史 ended 会话不重开；边界之后结束的会话照常收尾 | `m275_historical_ended_session_is_not_reopened`（settled=0、状态/原因不变、工作区保留、无新唤醒）；M273 自结束测试继续通过 |
| R4 | 收尾不起模型回合 | 收尾路径只做确定性 git/DB 操作；未保存内容改为 ended 而非 `waiting_user`，不会触发新回合；M273 重复扫描 `settled=0` |
| R5 | 既有 M241/M251/M255/M259/M262/M264/M273 测试继续通过 | 全量 `cargo test --workspace --no-fail-fast` |

## Applicable Harnesses

- Spec Harness：本文件 + 上表。
- AI Collaboration Harness：上下文范围=任务文件、M273 规格、`thin_derive::merged_settlement` 代码与测试；假设=安装版有 `origin` 远端、默认分支为 `origin/HEAD` 指向的分支；审查点=收尾范围边界与压缩合并判定；验证=下述测试与全量 Rust 测试。
- Release Harness：`Release-Urgency: immediate`；发版后由编排方做真机验收。
- Compatibility Harness：只有数据库新增表（`CREATE TABLE IF NOT EXISTS`），不修改既有表结构；旧库补表即用。
- 无 UI / 上传 / 视口改动，Viewport / Payload Harness 不适用。

## 测试矩阵

| 用例 | 类型 | 覆盖 |
|---|---|---|
| `m275_squash_merged_branch_content_counts_as_saved` | 真实 git 集成 | R1 |
| `m275_historical_ended_session_is_not_reopened` | 真实 SQLite | R3 |
| `m275_unsaved_content_ends_session_without_asking` | 真实 git + SQLite | R2/R4 |
| `r2_dirty_workspace_asks_user_with_a_reason`（改写） | 真实 git + SQLite | R2 |
| `m273_self_ended_session_still_gets_system_closeout` | 既有 | R3/R5 |
| `r1_r2_merged_sessions_settle_on_restart` | 既有 | R5 |

## Implementation Notes

- 判定入口：`thin_derive::merged_settlement::closeout_workspace`。它先解析默认分支，再调用
  `branch_content_saved`：
  1. `git diff <default> HEAD` 退出码为 0（树相同）→ 已保存。
  2. 否则取 `base = git merge-base <default> HEAD`，对 `git diff base..HEAD` 求 `git patch-id --stable`。
  3. 对 `git log --no-merges <base>..<default>` 的每个提交求 `git show --format= --patch` 的 patch-id，
     与第 2 步相同即已保存（压缩提交的补丁内容等于分支聚合补丁）。
  4. 仍未命中时，退回「提交号是否被其他远端/分支保存」检查；未保存则错误信息列出提交与分支。
- 收尾候选查询仍是 `state IN ('waiting_wake','running')` 加 M273 的 ended+已合并观测；
  ended 分支追加 `updated_at >= boundary`。
- `SettleSummary` 新增 `unverified` 计数；`asked_user` 保留为字段以兼容日志，但收尾路径不再递增它。
