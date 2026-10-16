# M4 阶段验收与交付记录

修复首请求准备路径的一处可复现等待：额度缓存仍有效时，`refresh_if_needed` 不再先排队等待后台刷新锁。保留锁内复查；过期、重置、缺失及 force 查询仍同步刷新。额度上限、时段与权限规则未修改。

规格：`docs/specs/feature-specs/first-request-latency.md`，原始任务逐字保留。M4-1/2/3 对应原任务三项。

| 追踪项 | 验收 | 结果与证据 |
| --- | --- | --- |
| M4-1 | 分段观测 | 接入 admission、workspace_checkpoint、route_quota、quota_refresh_lock、context_assembly、provider_post_admitted 调试日志；完整合成会话分段数据尚缺，不能证明历史 3.5 秒已全部归因 |
| M4-2 | 有效缓存不等后台网络 | PASS：fresh_quota_does_not_wait_for_background_refresh_lock 先红后绿；前 102ms 超时，后 0ms（毫秒精度）；90% 读数仍触发上限 |
| M4-2 | 不绕过门禁 | PASS：stale_reset_missing_and_forced_quota_still_wait_for_refresh 四种路径均等待刷新；原有额度、时段测试包含在 workspace 全量测试 |
| M4-3 | 回归 | PASS：quota_live 4 项；Rust workspace 全量通过（主库 1612 passed / 9 ignored）；前端 151 文件 / 1012 passed（含 lightModeAudit）；pnpm build 成功 |

## 分段耗时对比（合成锁竞争）

| 分段 | 修复前 | 修复后 |
| --- | --- | --- |
| 新鲜额度读数→返回路由调用方（刷新锁占用） | 102ms 时仍未返回，被 100ms deadline 截断 | 0ms |
| 过期/重置/缺失/force | 同步刷新 | 保持同步刷新，各 20ms 锁占用测试不放行 |
| 完整回合 admission→首次 HTTP | 未测得 | 未测得，不以局部测试冒充整轮测量 |

## Harness 与协作

- Spec：原始任务与追踪表；不扩大需求语义。
- Compatibility：来源、120 秒期限、两个重置点判断原样复用；force 不走快路径。
- Observation：阶段日志只记阶段和耗时，不记录凭据/消息。
- Release：只合并，不发版，not live。
- AI Collaboration：context scope 为首请求准备与额度刷新；assumption 为刷新锁竞争可阻塞新鲜缓存；review point 为过期/重置/force/越限保护；validation result 为先红后绿与全量回归。任务模板禁止 delegate_tasks，使用单执行流。
- 无 UI 布局变更，不新增截图要求；真机验收由编排方发版后完成。

## 真机验收步骤

1. 安装后以合成会话发送两轮，核对各阶段日志与实际 provider HTTP 时间。
2. 有效实时读数且后台刷新占锁时，首请求不等待该查询。
3. 过期/重置、到上限时仍刷新或回落；时段与权限仍先于请求生效。

## Spec Amendment

无需求语义修改。证据缺口：完整合成会话分段测量未完成，局部修复不能宣称消除历史全部 3.5 秒。

Scenario-Test: HLT-001, HLT-002, HLT-003, HLT-004, HLT-005, CXD-001, RTE-004, SKL-002, SKL-003
README-Update: reviewed
README-Update-Reason: 内部请求准备性能与无敏感信息日志，不改变公开安装、平台、隐私或功能承诺。
