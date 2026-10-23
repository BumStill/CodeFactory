# M165：产品文件→场景映射拆出信任根，改为「只增不删」文件（用户 10-23 选方案 B）

## 背景
- 现在每个新增产品文件都要改 `docs/testing/scenario-registry.json` 的 change_patterns；该文件是信任根，候选 PR 不能自证，编排方每次都要摘 `scenario-gate-pr` → 合并 → 还原（10-22 一天 4 次：#836/#839/#845/#846），是零介入的主要障碍。
## 需求
- R1 新增 `docs/testing/scenario-mappings.json`（非信任根）：只允许**新增**「路径 glob → 已存在的 Scenario ID」映射。场景定义、gate、required_evidence、删除或缩小已有映射仍只能改信任根 registry。
- R2 默认分支 trusted runner 上的校验（scenario-gate-pr）合并两处映射：对候选 PR 校验 mappings 文件相对 base 只增不减、只引用已存在且 active 的 Scenario、glob 不得匹配信任根文件本身、不得匹配全仓（如 `**`）；违反即 fail closed。
- R3 change_patterns 求影响集时两处并集；M156 的 PR 元数据自动填写同步使用。
- R4 测试（先失败）：新增映射 → 通过且不触发「cannot self-modify」；删除/缩小映射 → 失败；引用不存在场景 → 失败；`**` → 失败；改 registry 仍需 gate-dance。
- R5 迁移：不搬动现有映射（registry 保持原样），只是新映射走新文件。
## 约束
- 本 PR 本身改信任根校验逻辑，需要编排方 gate-dance 一次（PR 写明）。task-template；授权交付（ceiling: merged）。

Release-Urgency: default

## Implementation Notes
- 新文件格式：`{"schema_version": 1, "mappings": [{"pattern": "src/example.*", "scenario_id": "UI-001"}]}`；已有条目按精确二元组保留，不允许通过更换 glob 或 Scenario ID 移除旧覆盖。
- 当前 registry 没有独立 active 字段：以包含 `pull_request` gate 且未显式标记 inactive/retired 的场景作为 active；场景定义仍由 registry 唯一控制。
- trusted runner 使用自身代码和 base registry，仅把候选 mappings 当数据读取；执行规划、回执校验与 M156 共用合并后的加载入口。
- Spec Harness：R1–R5 对应 `tests/test_scenario_mappings.py`；Compatibility Harness：保留 registry 原始内容、缺失新文件视为空映射；AI Collaboration：单执行流，遵循任务模板不调用 delegate_tasks，审查重点为信任边界和 fail-closed。
- 无 UI、Rust 源码或发版变更；真机验收由编排方发版后完成：新增产品文件映射后执行交付，核对自动生成的 Scenario-Test、场景执行回执和 scenario-gate-pr；删除旧映射应被拒绝。
