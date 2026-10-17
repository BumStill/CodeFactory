# fix: reconcile local tool side effects before provider safety gate (M106)

## 需求与验收
- R1：M103 把任意未决 receipt 视为外部未知；executable Python heredoc 产生 workspace_git receipt，观察结果未在 provider 请求边界消费。详见 docs/specs/feature-specs/m106-reconcile-classify-side-effects.md。
- R2/R5：m106_failed_local_tool_observation_does_not_remain_unknown：合成 Python gh api GET 与 Path 本地编辑，快照未变 cancelled、已变 reconciled。先看到 unknown != cancelled 失败后转绿。
- R2/R4/R5：m106_python_forge_writes_cannot_use_workspace_receipts：Python gh POST、git push、gh --field= 写操作不得使用本地观察器。先失败后转绿。保留现有 m103_reconciliation_refuses_unproven_side_effect 终止边界。
- R3：不修改恢复预算；保留失败工具结果及 receipt summary（需重试/检查 diff），provider 请求前先本地观察。
- R6：复用 E2E-007 组件回归；未修改 registry，不宣称完整复杂 E2E 实现。

## 验证
- 完整 cargo test --workspace --no-fail-fast 通过（专用 cargo-target-m106）。
- pnpm test：151 文件、1013 测试通过，含 lightModeAudit。
- pnpm build 通过（只有现有大 chunk 警告）。
- Applicable Harnesses：Spec（版本化任务规格）、Compatibility（完整旧状态/provider 回归）、Observation/Payload（SQLite 回执与文件快照断言）、Release（仅合并，not live）、AI Collaboration（以下记录）。无 UI 修改。
- AI Collaboration：context scope=provider_recovery/model_transport/tool_recovery/M52b；assumptions=串行请求已返回且本地观察契约可读；review point=外部写入不得凭本地快照结算；validation result=上述全量测试通过。任务模板禁止 delegate_tasks，未启动该工具。
- 真机验收由编排方发版后完成：GET error/本地 edit unknown 自动继续且预算不递增；未知 push 明确终止并保留成果位置与交付回执。

README-Update: reviewed
README-Update-Reason: 内部对账修复，不改变公开安装、隐私和平台承诺。
Scenario-Test: RTE-001
