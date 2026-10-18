# M114：CodeFactory 自身把 GitHub API 额度（5000/h，用户级）耗尽——后台轮询无终止、无退避、逐个查询

## 背景

## 现象（真机 2026-10-18 12:47，v1.88.24）
- `gh api rate_limit` core remaining=0（user ID 1858168），合并队列工作流（用 RELEASE_PAT，同一用户额度）两次 403 失败，所有交付卡住。
- 编排方停掉自身全部 gh 轮询后，60 秒进程采样：CodeFactory 主进程（pid 57215）在一分钟内启动 ≥66 个 gh 子进程：
  - 43× `gh pr view <N> --json state,headRefOid,mergeCommit,autoMergeRequest`，覆盖 17 个 PR：438,440,553,579,581,590,596,599,600,609,620,680,704,710,724,728,729——其中 15 个对应的交付/objective 早已 failed/cancelled/platform_incident（最早 2026-08-24），PR 早已合并或关闭。
  - 18× `gh auth status --hostname github.com`（每次都会调 GitHub API 验证 token）。
  - 3× `gh pr view <N> --json headRefOid,labels`，若干 `gh pr list`。
  （采样间隔 0.5s，会漏掉短进程，实际更多。）

## 根因（编排方读码定位）
1. `execution_workspace.rs` `reconcile_parked_delivery_progress`（每 60s 由 cleanup pass 调用，CLEANUP_POLL_INTERVAL）：候选 = objective 状态 running/waiting_system/**cancelled/failed**（无时间上限）且有 canonical PR 的工作区，`LIMIT 64`；对每个候选调用 `RemoteCanonicalPrObserver::observe`（gh pr view；若 open 再 `observed_queue_arm` 第二次 gh pr view；再 `observe_ci_checks` 走 REST）。对 failed/cancelled 且未合并、或 ClosedUnmerged/Unknown 的结果直接 `continue`，**从不记录已观测结论、无退避**，于是永远每分钟重查。M102（#699）扩大了这个候选集。
2. `delivery.rs` `delivery_readiness_from_origin` → `gh_cli_available()` → `gh auth status`（最多 3 个路径各试一次）：在构建系统提示时调用，**每次模型请求**都会执行，每次一次 API 调用。
3. 合并队列工作流（`.github/workflows/merge-queue.yml`，RELEASE_PAT，计入同一用户额度）：耗尽前一小时（11:45–12:50）运行 21 次，其中 20 次由 workflow_run（任意工作流完成）触发；`scripts/merge-queue.mjs` 每次运行对每个 open PR 调 check-runs（--paginate）、必要时 comments/events（--paginate）、commit，外加 2 次 rulesets，并且 `plan` 与 `run` 两个步骤各跑一遍——按当时 ~6 个 open PR 估计每次 50–60 次 REST，即 ~1000–1300/h。
4. 编排方自己的脚本另外每 5 分钟手工触发合并队列（多个脚本并行），并有多条每分钟一次的轮询，额外 ~1500–3000/h（编排方已修：去掉定时触发、轮询 ≥180s）。

## 实测基线（13:10–13:15，编排方脚本全部停止时）
- core 每分钟 ~25 次（≈1500/h），graphql 每分钟 ~7 点。即 CodeFactory 自身 + 自然触发的工作流已占额度 30%；任何额外负载叠加即可能耗尽。

## 用户决策
- 后台观测必须有终点与退避，不允许"每 60 秒把所有历史交付重查一遍"。
- 额度是用户级共享资源（CodeFactory 主进程 + 合并队列工作流 + 编排方脚本），产品侧必须主动让路。
- 不改变交付语义与安全边界：未知仍然不得当成功，也不得据此删除工作区。

## 需求

### Requirements Traceability

| Req ID | requirement | minimum evidence |
| --- | --- | --- |
| R1 | 用数据核实上述三项各自每小时的调用量（可在测试/开发构建里给 gh 调用计数并记录调用方），写进规格。 | 计数器接入的集成测试 + 规格内调用量表 |
| R2 | 观测有终点：PR 已合并/已关闭、或 objective 已终态且超过可配置窗口（如 7 天）、或观测结果连续 N 次无变化的记录，持久化「已结论/下次观测时间」，不再每轮重查。终态 objective 只在确有可能改变结论时（如 PR 仍 open）才观测。 | 60 tick 集成测试断言历史记录只被查询一次 |
| R3 | 退避：每条交付记录维护 next_observation_at，无变化时指数退避（1m→2m→4m…≤60m），有变化或会话主动请求时重置。 | 单元测试断言退避序列与重置 |
| R4 | 批量：每轮对同一仓库用一次 `gh pr list --state all --search <numbers 或 head 分支> --json number,state,headRefOid,mergeCommit,labels,autoMergeRequest` 取回所有候选，代替逐个 `gh pr view`；CI 状态只对「PR open 且会话正在等 CI」的记录查询。 | 批量观测测试：N 个候选只产生 1 次批量调用 |
| R5 | `gh auth status` 结果按进程缓存（成功缓存 10 分钟，失败缓存 1 分钟），或改为读取本地 hosts.yml 判断；系统提示构建不得每次触发网络调用。 | 100 次系统提示构建 ≤1 次 auth 检查 |
| R6 | 额度感知：读取 gh/REST 响应中的 X-RateLimit-Remaining（或周期性 `gh api rate_limit`，该端点不计额度），低于 20% 时把所有后台观测间隔放大、低于 5% 时暂停后台观测，并在状态里可见；不得把 403 rate limit 当成 PR 状态未知后立即重试。 | 额度分级单元测试 + 403 不立即重试的断言 |
| R7 | 测试必须先失败：给 gh 调用加可注入计数器；构造 20 条历史失败/取消的交付记录 + 2 条活跃 PR，跑 60 个 cleanup tick，断言总 gh 调用 ≤ 合理上界（活跃 PR 数 × tick 数的一小部分 + 常数），历史记录在首次结论后不再被查询；系统提示连续构建 100 次只触发 ≤1 次 auth 检查。 | 先失败的测试记录 + 通过后的同一命令输出 |
| R8 | 合并队列降耗（本单授权修改 `.github/workflows/merge-queue.yml` 与 `scripts/merge-queue.mjs`，信任根由编排方放行）：a) workflow_run 触发时先用一次轻量查询判断是否存在 armed PR，没有就立即退出；b) `plan` 与 `run` 共享一次数据抓取，不重复查询；c) 只对 armed PR 拉 check-runs/comments/events，非 armed 的 open PR 不查；d) rulesets 每次运行读一次。附前后调用量对比。 | `scripts/merge-queue.test.mjs` 断言抓取次数 + PR 内前后对比表 |

R8 原编号（保留原文，避免需求编号漂移）：如需改合并队列工作流以减少调用（例如没有 armed PR 时立即退出、避免逐 PR 多次查询），本单授权修改 `.github/workflows/merge-queue.yml` 与 `scripts/merge-queue.mjs`（信任根，scenario-gate-pr 由编排方处理），附调用量前后对比。

### 调用量基线（R1，按代码实测口径）

| 来源 | 触发频率 | 每次调用数 | 折算 / 小时 | 依据 |
| --- | --- | --- | --- | --- |
| `reconcile_parked_delivery_progress`（修复前） | 60s | 每候选 2–3 次（`gh pr view` + `labels` + check-runs/rules） | 60 × 17 候选 × ~2.5 ≈ 2550/h | 现场采样 43 次 `gh pr view`/分钟 + 3 次 labels |
| 系统提示构建中的 `gh_cli_available` | 每次模型请求 | 1–3 次 `gh auth status` | 现场 18 次/分钟 | 现场采样 |
| 合并队列工作流 | 每次 `workflow_run` | ~50–60 次 REST（plan+run 各一遍） | ~1000–1300/h | 11:45–12:50 运行 21 次 |

合计约 4000+/h，逼近 5000/h 用户级上限；叠加编排方脚本后耗尽。

## Applicable Harnesses
- Spec Harness（必须）：本规格 + Requirements Traceability + 测试矩阵。
- Compatibility Harness：`delivery_runs` 历史 schema 只有 `last_observed_at`，新增列必须走既有 `ALTER TABLE` 迁移路径，旧库可读。
- Observation Harness：额度分级与暂停状态必须可见（不静默）。
- Release Harness：合并队列脚本/工作流变更影响全部交付，需 runbook 级前后对比。
- AI Collaboration Harness：多角色协作、假设需记录。

## 测试矩阵

| 用例 | 覆盖 Req | 类型 |
| --- | --- | --- |
| 60 tick × (20 历史 + 2 活跃) 的总 gh 调用上界 | R2 R3 R4 R7 | Rust 集成 |
| 已结论记录不再被查询 | R2 R7 | Rust 集成 |
| 退避序列 1→2→4→…→60 分钟封顶 | R3 | Rust 单元 |
| 有变化 / 会话主动请求时重置退避 | R3 | Rust 单元 |
| 同仓库 N 个候选 → 1 次批量 `gh pr list` | R4 | Rust 单元 |
| CI 只对 open 且等待 CI 的记录查询 | R4 | Rust 单元 |
| 100 次系统提示构建 ≤1 次 auth 检查 | R5 R7 | Rust 单元 |
| 额度 20% / 5% 分级与暂停 | R6 | Rust 单元 |
| 403 rate limit 不立即重试 | R6 | Rust 单元 |
| 合并队列：无 armed PR 立即退出 | R8 | Node test |
| 合并队列：plan/run 共享抓取、rulesets 一次 | R8 | Node test |

## 约束
- 不改变交付语义与安全边界（仍不得把未知当成功）。合成 fixture，不写生产 DB。
