# CodeFactory agent 任务模板（仓库内权威副本）

本文件是 CodeFactory 会话内 agent 的**任务/交付使用规矩**在仓库里的权威副本。
编排方（Claude Code）的任务模板 `~/.codefactory/agent-tools/task-template.md` 与本文件
保持同一套规矩；改动其中一处时，另一处必须同步，避免两套口径。

规格来源：`docs/specs/feature-specs/session-background-tasks.md`（CF-BG 阶段 2，R5 / R5b）。

## 1. 长等待一律放后台

任何"要等"的外部事项都不要在回合里空转，也不要把等待留给用户：

| 等待类型 | 用法 |
| --- | --- |
| 有命令可跑（CI、构建、部署、长测试） | `background_task_start`：命令结束（成功/失败/超时）以"退出码 + 输出尾部"唤醒本会话 |
| 没有可结束的进程（CI 排队、合并队列、发版、线上巡检） | `schedule_wakeup`：`after_sec` 一次性或 `every_sec` 周期（60s–6h），`wakeup_list` / `wakeup_cancel` 查看与取消 |

两种方式都持久化：应用重启后按原计划继续，关闭期间错过的周期只补一次。

## 2. 交付开 PR 之后

1. 用后台任务监视该 PR 的 CI：`gh pr checks <n> --watch`，或间隔 **≥180 秒**的轮询脚本
   （低于这个间隔会烧掉 GitHub API 额度，见 M114）。
2. 被唤醒后按结果分支：
   - **失败** → 修复、推送，再次挂监视；
   - **只差信任根自修改**（`scenario-registry` / `tools/governance` / `.github` 等）→
     明确写"等编排方放行"，不要自行绕过；
   - **全绿** → 打 `merge-queue: armed`，由合并队列按序合并。
3. 合并之后按授权收尾（closeout），再做发版/安装验收分工。

## 3. 结束回合前

如果仍有外部事项在进行中（CI、合并队列、发版），**必须先挂好后台任务或定时唤醒**，
否则不放行。这条只约束 agent 的下一步：在回复里用运维口语说明"我在盯 CI / 我在等合并"，
不要把完成门禁的内部词汇写进对话，也不要展示内部标记。

## 4. 谁来开后台任务（R5b）

- **agent 自主决定**（`agent_decided`）：等 CI、等长构建、等部署、定期检查状态、
  长测试放后台跑同时继续别的事——不限于交付流程，由 agent 按当前情况判断。
- **用户主动要求**（`user_requested`）：用户说"盯着这个 PR，合并了告诉我"、
  "每小时检查一次线上状态"、"这个测试放后台跑，跑完叫我"时，创建对应的后台任务或定时唤醒，
  并在回复里说明**建了什么、怎么停**（`background_task_stop` / `wakeup_cancel`）。
- 两种来源走同一套权限与审批（`background_task_start` / `schedule_wakeup` 属变更类工具，
  和前台 `bash` 一样需要审批）；任务记录标明来源，可列出、可停止。

## 5. 证据

`Scenario-Test:` 只写 `validate_scenario_test_governance.py --ci` 列出的受影响 ID；
`README-Update:` 只写 `required` 或 `reviewed`，并跟一行非空理由。
