## 修改与范围
实现 CF-BLD-R2a/R2b。失败任务最后活动满默认 24 小时后回收私有缓存；CODEFACTORY_FAILED_CACHE_IDLE_HOURS 可配置。启动扫描 startup，运行巡检 runtime_sweep，手动清理 manual_clean。审计 reason failed_idle_24h 与 task_ended 区分。

Scenario-Test: RTE-004, E2E-009
README-Update: not-needed
README-Update-Reason: 不改变公开安装、隐私或平台承诺，配置与回收语义记录在特性规格。

## Req 与自验收
| Req | 场景 | 证据 | 结果 |
| --- | --- | --- | --- |
| R2a | 23 小时保留、25 小时回收、24 小时边界、活动重计时、未来时间保护、配置窗口 | failed_idle_boundary_and_activity_reset | pass |
| R2a | SQLite 消息、工具调用、模型 usage、turn 活动、objective 进度均重置窗口 | failed_cache_ownership_resets_on_each_persisted_activity_source | pass |
| R2a | 删除真实合成缓存、保护 in-use、审计失败原因 | failed_reclaim_audits_reason_and_sweep_trigger_and_preserves_builder | pass |
| R2b | startup 与 runtime_sweep 分离并写入 JSONL | failed_reclaim_audits_reason_and_sweep_trigger_and_preserves_builder | pass |

TDD：新增两个 build_cache 回归测试先因缺少闲置判定、failed_idle_owners、sweep_trigger 编译失败（exit 101），实现后同一命令通过。额外 SQLite 活动来源测试在全 workspace 测试中通过。

## 验证
- Rust 专属 CARGO_TARGET_DIR cargo test --workspace --no-fail-fast：通过。
- pnpm test（含 lightModeAudit）：通过。
- pnpm build：通过（既有大 chunk 警告）。
- 治理基线：通过。
- UI 未修改，无浏览器场景或截图要求。not live；本任务授权 pr_only，不合并、不发布。

## Harness / AI Collaboration
Spec：原文追加 R2a/R2b；Compatibility：保留 cancelled/completed 语义及旧审计枚举；Observation：字段断言；Payload：真实合成目录删除及 builder 保留；AI Collaboration：context scope 为任务文件、模板、缓存与 workspace 接线；assumptions 为持久活动时间毫秒，模型 usage RFC3339；review point 为所有归属调用者与审计；validation result 为全工作区、前端测试及构建通过。用户明确禁止 delegate_tasks，单执行流。

## 风险与收尾
仅删除已有受管缓存，不删除 workspace 或源码；续做重新编译。活动按 session 保守保护。CF-CLOSEOUT/U26 的 workspace 生命周期不修改，只变更缓存归属判定。无新增依赖、无迁移、无 workflow 或 scenario-registry 变更。无待决 Spec Amendment。按临时规则停在 PR，§8 合并确认、分支删除和合并后清理暂不适用。
