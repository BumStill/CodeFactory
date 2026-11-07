# M259：会话结束原因里的「收尾未核验项」与事实不符

## 规格（任务原文）
- R1：原因里列出的每一项都必须有当场核对的事实来源。核对项至少包括：PR 是否 merged、合并提交是否在默认分支、远端分支是否还在、本地有没有未提交或未推送的改动。做过的写「已完成」，没做的写「未完成」，查不到的写「未核验」并给出原因。
- R2：核对本身失败（命令超时、网络不可用、仓库不可读）时，同样写成「未核验」加失败原因，不得静默省略。
- R3：PR 已合并而远端分支还在时，系统删除本会话自己的分支，并把删除结果写进原因；M257 的授权边界不变，只动本会话自己的分支。
- R4：原因要短、可读，去掉 `§8`、`squash commit`、`唤醒` 这类内部用语；细节留在工具收据里。
- R5：M255 的「合并后收尾有上限、不重复起回合」行为不变，只改结束语的事实依据。

## 测试（任务原文）
- 真实 git 仓库 + 真实远端：默认分支已含 PR 合并提交、远端分支已清理 → 原因写「已合并/已在默认分支/已清理/没有未提交改动」。
- 远端分支仍在 → 系统删除该分支，原因写删除结果，默认分支不受影响。
- 工作区有未提交文件 → 原因写数量与文件名，文件原样保留。
- 非 git 目录 → 每一项都写「未核验：原因」。
- 真实 SQLite + 真实仓库走完整入口：结束原因来自工作区实况，且不含内部用语。

## 验收（任务原文）
合并后的新会话结束时，原因里的每一项都能和仓库实况对上；系统已删除的远端分支不会再被说成「未核验」。

## 实施记录
Applicable Harnesses：Spec、Compatibility、Observation、AI Collaboration（本任务不发布，Release Harness 由编排方发版时覆盖）。

Requirements Traceability：R1/R2 对应 `verify_closeout` 的逐项分支与 `m259_names_unverified_items_with_a_reason`；R1 对应 `m259_reports_verified_facts_when_closeout_is_done`；R3 对应 `m259_deletes_the_sessions_own_remote_branch`；R4 对应 `m259_end_reason_is_grounded_in_the_real_workspace` 的内部用语断言；R5 对应 M255 既有收尾与唤醒上限测试保持不变。

实现边界：结束原因由 `closeout_result` 组装，`verify_closeout` 在阻塞线程里跑有界 git 命令（单条 20 秒，整体 45 秒）。只读本地状态，不清理工作区；唯一写入是删除本会话自己的远端分支。核验坐标（工作区、分支、PR 号）全部来自本会话自己的 `execution_workspaces`、`thin_delivery_refs`、`thin_external_observations`。

AI Collaboration：context scope 为任务原文、`thin_derive.rs`、`background_tasks.rs`、`delivery.rs` 的仓库解析与有界命令辅助；assumptions 为「要收尾的会话一定有本会话工作区记录」；review point 为删除分支的授权范围与未核验分支的措辞；validation result 见本任务的测试输出。
