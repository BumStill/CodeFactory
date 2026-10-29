## M205 到期收尾唤醒不再静默滞留

Scenario-Test: HLT-001, HLT-004, RTE-003, RTE-005
README-Update: reviewed
README-Update-Reason: 修复后台调度与历史等待恢复，不改变公开功能、安装、隐私或平台承诺。

## 根因与范围
- 真实 start_runtime 在首次到期扫描前 await reconcile 及全部 waiting_wake 对账；后者会进入 heartbeat_external 的外部命令路径。一个启动对账未完成即可使整个持久唤醒循环尚未启动。现在独立启动两个对账任务，投递循环不再等它们。
- drain_due_wakeups_at 在审计前用 workspace_recovery_budget::due(...).unwrap_or(false) 静默跳过耗尽/已结算预算。旧 M202 测试只有成功 Recorder 和空预算，遗漏此真实持久形状。
- 新拒绝路径将审计、可读 ended 与取消 pending 放在同一事务；明确说明未执行唤醒、不冒充 §8 清理完成。仍保留活执行者/后台任务保护及重试退避。
- 未读取或修改生产数据库；上述为代码路径与合成复现证据，不将推测的生产预算值当成已证实事实。

## Req ID ↔ 测试 / 自验
| Req | 验证 | 结果 |
|---|---|---|
| R1 | m205_restart_backlog_never_silently_skips_exhausted_recovery：5 条合成 waiting_wake + 已合并 PR + 3 closeout / 2 repair，文件 SQLite 关闭重开 | 已先红：pending=4，期望 0；修复后原命令通过 |
| R2 | 生产 start_runtime 调用链及恢复预算 guard 审查 | 根因见上；未把旧 Recorder 用例当真实安装证明 |
| R3 | 每个会话具备 recovered/rejected 审计，耗尽预算的 4 会话 ended 且原因含 PR 和恢复；正常行投递一次 | 通过 |
| R4 | 全表 COUNT(status=pending AND next_at<now-120)=0；第二次扫描无重复投递 | 合成积压通过；活执行者/基础设施故障仍保留安全保护，不声称任何环境无条件清零 |
| R5 | 重开 SQLite 后一次 drain 清理所有 5 条积压；生产扫描不再 await 启动对账 | 通过；真实安装后一个周期由编排方核对 |

## Harness / AI Collaboration
- Spec：原文任务 docs/specs/feature-specs/m205-overdue-closeout-wake.md；R1–R5 对应原文要求编号。
- Compatibility：保留 schema、至少一次投递、退避、活执行者保护、旧 M202 回归；新增文件数据库重启 fixture。
- Observation：审计 detail_json 含 wakeup_id、next_at、拒绝原因、attempts；字段级检查 pending、ended、原因、投递去重。
- Release：仅授权到合并，由合并队列合并；保留 Release-Urgency trailer，另行刻意发版。
- AI Collaboration：context scope 为 background_tasks + 相关 thin/工作区调用链；assumptions 为不能根据历史症状捏造生产预算值；review point 为启动屏障、静默跳过、事务终态、活执行者保护；validation result 随最终全量结果更新。按 task-template 禁用 delegate_tasks，采用单执行流。
- 无 UI 修改，不适用真实浏览器截图。后台能力沿用既有 schedule_wakeup/CLI 调度入口，不依赖 GUI。

## 验证命令
- CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-m205" CARGO_INCREMENTAL=0 cargo test --manifest-path src-tauri/Cargo.toml m205_restart_backlog_never_silently_skips_exhausted_recovery -- --nocapture：先红后绿。
- pnpm test && pnpm build：通过。
- Rust workspace 全量、场景治理及 README 验证：执行中，最终以完成记录为准。

## §8 与真实应用验收分工
- not live：本任务不发布、不安装、不操作生产会话；真机验收由编排方发版安装后完成。
- 安装 10 分钟内检查 pending 且 next_at < now-600 的行数为 0；核对任务中 3 个收尾会话 ended、可读原因；审计中每条积压有投递或拒绝记录；核对 fire_count 与状态一致，无重复执行或重复交付。
- 合并后核对 merged=true、squash SHA 位于 origin/main、release plan trailer；检查准确远端分支、工作区干净且无未推送改动；清理本任务专属 target 与临时产物，不删除托管工作区。
- Spec Amendment：无。

Release-Urgency: immediate
