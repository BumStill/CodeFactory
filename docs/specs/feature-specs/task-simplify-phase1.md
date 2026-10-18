# 减法迁移 · 阶段 1：「先查后做」交付薄层 + 读现场恢复（开关控制，新任务默认）

## 背景
- 用户 2026-10-18 决定按减法评审重构（报告 https://claude.ai/artifact/9jmjU93EsuG3K7cfVCBdaZ）。阶段 0 清单：/Users/leo/.codefactory/agent-tools/simplify-phase0-inventory.md（先读，含 30 天失败归因与文件/表处置）。
- 核心判断：git/GitHub 是唯一事实源；外部写入只有推送、PR、打标签、合并、发版需要防重复，全部改为「动手前先查现状」；其他命令与模型请求按「至少一次」处理，中断后由模型读现场自行判断。

## 需求
- R1 开关：新增按任务的 `delivery_mode`（`thin` / `legacy`），新建任务默认 `thin`；存量任务保持 `legacy`。一个任务只走一条路径，不混用。在规格文档写清开关位置与回退方法。
- R2 薄交付命令（thin 模式下 deliver_changes 的实现）：每一步先查现状再动手，全部幂等——
  1) 有未提交改动则提交；2) 比对远端分支提交号，不同才推送（拒绝 force，除非远端是本分支祖先）；3) 按 head 分支查 PR，没有才创建；正文/标题以本次输入为准直接覆盖；4) ceiling=merged 时确保 `merge-queue: armed` 标签（已存在则不动；打标失败直接报错返回，不得改为直接合并，参见 #744）；5) **立即返回**：PR 链接、当前 CI 概况、「请用后台任务监视 CI」的提示。不在工具调用里等待 CI。
  只持久化一条映射：任务 ↔ 分支 ↔ PR 号 ↔ ceiling。不写身份修订、变更意图、回执、交付事件流。
- R3 读现场恢复（thin 模式）：进程重启、会话中断、模型请求失败时，不做 provider episode/attempt 对账、不写副作用回执；会话恢复时追加一条系统提示：「你在 <时间> 被中断，最后一步是 <工具/命令摘要>，可能已执行也可能未执行。先用 git status、git log、gh pr view 等确认现状，再决定下一步。」然后继续。模型请求失败按原有回落/重试，但重试就是直接重发（至少一次）。
- R4 会话状态（thin 模式）：只用 运行中 / 等唤醒 / 等用户 / 已结束（附一句原因与成果位置）。不使用恢复预算；连续同一失败 3 次即「已结束：<原因>」，说清试了什么、成果在哪。
- R5 完成门禁（thin 模式）只保留一条：存在外部进行中的事项（PR 未合并、CI 未结束、发版未完成）而本会话没有后台任务/定时唤醒在等它，就不放行，并提示挂监视。
- R6 计数：给 thin 路径加轻量计数（gh 调用次数、交付命令每步结果），写进会话日志，供编排方验收。
- R7 测试（先失败）：
  - 合成 forge：推送后进程中断→恢复→不重复推送、不重复开 PR；PR 已存在→复用；打标失败→返回错误不合并；交付命令在 CI 运行中立即返回。
  - 中断恢复：最后一步是 bash（成功/失败/未知）→ 恢复后系统提示出现且会话继续，不进入任何「无法确认」终态。
  - legacy 任务行为不变（现有测试保持通过）。
- R8 不删除旧代码（阶段 3 才删）；只在 thin 路径不调用它们。

## 退出标准（编排方真机验收）
连续 10 个新任务交付环节人工介入 ≤1，且无重复推送/重复 PR；GitHub REST ≤600/h。

## 约束
- 推送、PR、合并、发版之外不新增任何「拿不准就停」的检查。合成 fixture，不写生产 DB。授权交付（ceiling: merged）。

## Implementation Notes
- 开关位于 `sessions.delivery_mode`：迁移的默认值是 `legacy`，两个新建会话入口显式写入 `thin`；匿名会话不创建任务行。回退：任务停止后将该任务的 `sessions.delivery_mode` 改为 `legacy`，重启后走原路径，不删除薄映射或旧表；不要在运行中的任务切换模式。
- 接手自会话 2ceb489c 的分支；本分支取代原 WIP 分支，不向原分支推送。
- 本次不调用 delegate_tasks：按任务模板要求单执行流；context scope 为任务规格、WIP 与 thin 分流；assumptions 为保留旧 schema/状态承载兼容；review point 为外部写入幂等和旧恢复隔离；validation result 见下表。

## Requirements Traceability / 自验记录
| Req | 实现及证据 | 结果 |
| --- | --- | --- |
| R1 | `migration_keeps_old_rows_legacy_and_accepts_explicit_thin`；新会话入口显式 thin | 通过 |
| R2 | `pushed_then_interrupted_resumes_without_repush_or_duplicate_pr`、`existing_pr_and_label_are_reused_while_ci_returns_immediately`、`label_failure_returns_error_never_merges`、`merged_same_head_is_delivered_without_new_pr_or_label` | 通过；最后一项在修复前见失败 |
| R3 | `resumed_bash_always_has_system_prompt_for_success_failure_unknown`；resume_chat_objective 装配系统提示；provider owner 在 thin 下返回 None | 单元通过；完整重启真机路径待编排验收 |
| R4 | `thin_three_identical_failures_end_with_summary_without_legacy_budget` | 先失败后通过；三次结束、保留说明与成果指引 |
| R5 | `external_wait_tracks_forge_not_mapping_existence` 与现有 background_tasks 的 session-owned wake tests | 通过；完成前只查询一次 forge，不以映射存在代替 pending |
| R6 | tool metadata 与 tracing 中记录 gh_calls、steps、失败信息；completion_observe 单次计数 | 已实现，真机会话日志待编排验收 |
| R7 | Rust workspace 全测；前端 152 文件、1,016 测试；pnpm build | 通过 |
| R8 | 旧代码保留；`thin_mutation_dispatches_without_any_receipt_even_with_unknown_legacy_effect` | 通过 |

## 编排方发版后真实应用验收
1. 新建任务，核对 delivery_mode=thin；历史任务仍为 legacy。
2. bash 成功/失败/未知时重启，确认系统提示出现、会话自动继续，不出现「无法确认」终态。
3. 推送后中断，恢复时核对远端 head 与 PR 号不重复；CI 未结束时工具立即返回且后台监视存在。
4. 打标失败必须返回错误，不能直接合并；合并后再次执行不得开第二个同 head PR。
5. 同类错误连续三次后核对「已结束」说明及成果位置。
6. 连续十个新任务记录人工介入次数（≤1）、无重复推送/PR，以及 GitHub REST ≤600/h；这些指标不以本地单测替代。

- Applicable Harnesses：Spec、Compatibility（旧任务/数据库）、Observation（调用计数）、Release（只交付至合并）、Payload（工具命令）、AI Collaboration。
- 真机验收由编排方发版安装后执行；本任务不发版，不操作生产数据库。
