## Summary
接手自 2ceb489c，取代分支 codefactory/objective-098e085ba11d2074eb7cc16f417cb5e5；不向原分支推送。
任务级 thin/legacy 分流、先查后做交付、读现场恢复、三次同类失败结束、完成前检查外部监视和 gh 计数；旧代码保留。

## Req ID ↔ 自验
详见 docs/specs/feature-specs/task-simplify-phase1.md 的逐项自验与回退说明。
R1：迁移旧行 legacy、新入口 thin 测试通过。
R2：合成 forge 中断后不重复推送/PR、已有 PR 复用、标签失败不合并、CI 运行立即返回通过；merged_same_head_is_delivered_without_new_pr_or_label 先失败后通过。
R3：bash 成功/失败/未知恢复提示断言通过；provider owner 和 mutation receipt 薄路径旁路。完整安装版重启验收由编排方完成。
R4：thin_three_identical_failures_end_with_summary_without_legacy_budget 先失败后通过。
R5：external_wait_tracks_forge_not_mapping_existence 和 background_tasks 的 session-owned wake 测试通过。
R6：工具 metadata/tracing 记录 gh_calls、steps、错误和完成前单次 gh 查询。
R7：Rust workspace 全测通过；pnpm test 152 文件、1016 项通过；pnpm build 通过。
R8：旧代码未删；thin_mutation_dispatches_without_any_receipt_even_with_unknown_legacy_effect 通过。

## Applicable Harnesses / AI Collaboration
Spec：原文规格与 Requirements Traceability；Compatibility：旧行 legacy 与全量 Rust 回归；Observation：轻量 gh 计数；Release：仅合并不发版，not live；Payload：mutation preflight 路由测试；AI Collaboration：context scope=任务/WIP/thin 路由，assumptions=旧 schema 保留，review point=幂等及恢复隔离，validation result=上述测试通过。按模板不调用 delegate_tasks，单执行流。

## 真实应用验收交接
真机验收由编排方发版后完成：新旧模式核对，bash 三种结果后重启自动继续，推送后中断复用 head/PR，CI pending 立即返回并挂后台监视，标签失败不能直接合并，三次失败显示原因与成果位置。连续十单介入≤1、无重复推送/PR、GitHub REST≤600/h 待现场计量。本任务未更改前端布局。

## Spec Amendment
无需求语义修改；完整重启与十单指标不宣称已由单测验收。

Scenario-Test: CXD-001, CXD-002, HLT-001, HLT-002, HLT-003, HLT-004, HLT-005, RTE-003, RTE-004, SKL-002, SKL-003
README-Update: reviewed
README-Update-Reason: 内部开关迁移不改变公开安装、隐私或平台承诺。
