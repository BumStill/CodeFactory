# M116：同一任务内交付授权上限从 pr_only 提到 merged 被判「交付身份冲突」

## 现象（真机 2026-10-18 16:05，v1.88.25）
- 会话 74b33b1e：派单消息明确「授权交付（ceiling: merged）」，但 delivery_runs.requested_ceiling=pr_only（首次交付以 pr_only 建立）；之后再调用 deliver_changes 时报 `delivery run identity collision: 交付授权上限发生变化；未执行提交、推送或 PR 操作。请回到原任务的原工作区继续…` → 2 分钟内恢复耗尽。成果靠编排方推送保住。

## 需求
- R1 用数据确认首次交付为何是 pr_only（agent 未传 ceiling 时的默认值？会话 delivery_authorized/授权上限没有传到交付工具？）。
- R2 首次交付的默认 ceiling 取会话已授权的上限（派单/用户消息解析出的 ceiling），而不是固定 pr_only。
- R3 同一任务、同一分支、同一身份下，在会话授权范围内**提高**上限（pr_only→through_ci_green→merged）视为同一交付的升级，更新记录继续；**降低**或超出会话授权才拒绝，并给出用户看得懂的说明（不得出现「请回到原任务的原工作区」这类当前就在原工作区时自相矛盾的文案）。
- R4 测试（先失败）：pr_only 建立后以 merged 再交付 → 继续；会话只授权 pr_only 时请求 merged → 拒绝且文案清楚；未传 ceiling → 取会话授权。

## 约束
- 不放宽授权边界。合成 fixture，不写生产 DB。

## 取证与证据边界（R1）

- `dispatch::ceiling_declaration` 原先只返回 bool；派单消息中的具体 ceiling 没有持久化。`commands/chat.rs` 原先只更新 `sessions.delivery_authorized`。
- `tools/delivery.rs::execute` 未传 ceiling 时得到 None；`prepare_durable_run` 以配置快照的 `delivery_ceiling` 兜底，而非固定 pr_only。显式请求也会被配置上限夹取。因此首次 pr_only 可能来自显式请求或配置快照，不能仅凭源码认定生产设置就是 pr_only。
- `delivery_ceiling_allows_resume` 原先只允许相等或降低；`0004_delivery_runs.sql` 和运行时 `ensure_schema` 还安装不可变触发器。合成 SQLite fixture 已重现升级时的完整生产报错。
- 未读取或修改生产 DB，未获得首个工具调用的原始参数/当时设置快照，故 R1 的生产逐调用归因仍需编排方核对：session 74b33b1e 首次 deliver_changes 的 ceiling 参数、当时 settings.delivery_ceiling、对应 delivery_runs/event 回执。不得把候选原因写成已证实事实。

## Requirements Traceability

| Req ID | 实现/证据 |
| --- | --- |
| R1 | 上述代码路径与合成重现；生产首调用参数归因待编排方补证 |
| R2 | 会话 ceiling 字段、消息解析、聊天事务内写入、交付工具默认读取；会话 SQLite round-trip 与 resolver 断言 |
| R3 | 同身份升级更新原记录；降低调用明确拒绝、记录不变；工具入口越权拒绝；全局配置仍为硬上限 |
| R4 | 升级测试先出现原始 identity collision；数据库旧触发器拒绝测试先红；解析、默认与工具入口测试补齐。后两类没有独立 red-before-implementation 证据，不声称全量严格 TDD |

## Applicable Harnesses

- Spec Harness：本规格、需求映射、验收矩阵。
- Compatibility Harness：启动幂等 schema 为历史库补列，移除旧不可变触发器；合成历史 schema 验收。
- Observation Harness：SQLite 字段级断言、真实工具入口拒绝及零文件副作用断言；本地/CI 原始状态明确区分。
- Release Harness：本次仅授权合并，不发版；真实安装版验收由编排方另行发版后完成（not live）。
- AI Collaboration Harness：同一执行流，context 为交付工具、授权解析、聊天事务和 SQLite schema；不写生产数据；PR 记录假设、review point、验证结果。模板禁止 delegate_tasks，本次不启动该工具。
- 无 UI/载荷改动，不启用 Viewport/Payload Harness。

## 验收矩阵

| 场景 | 最低断言 |
| --- | --- |
| 同一任务 pr_only → through_merge | 原 run ID 不变，记录更新为 through_merge |
| 原记录 through_release，本次 through_ci_green | 明确拒绝降低、记录不变、不出现回原工作区文案；M116-R3 覆盖旧 M69 的降低调用可继续行为 |
| 会话只授权 pr_only，请求 merged | 真工具 execute 在 repository preflight 前拒绝，点名会话授权上限，临时目录无文件副作用 |
| 未传 ceiling，会话授权 through_merge | 默认 through_merge，但不得突破全局硬上限 |
| 全局 off 或全局 pr_only | 不因消息授权而抬高全局边界 |
| 消息 ceiling: merged / through_ci_green | 正确保留标签并归一化 merged → through_merge；明确只读/撤权不产生授权 |
| 历史数据库 | 幂等初始化补列，旧 immutable trigger 被替换，重启后升级仍可继续 |

## Implementation Notes

- `sessions.delivery_ceiling` 为 NULL 时保留原配置默认。全局配置与会话声明取较低者；配置为 off 始终否决。
- 降低调用按 R3 拒绝，不自行将需求解释为仅禁止改写数据库。无需 Spec Amendment。
- SQL 文件记录 schema 意图；生产兼容入口以 `storage::db::ensure_schema` / `delivery_run::ensure_schema` 为准。
- 真实应用验收（编排方发版安装后）：授权 merged、先 pr_only 建立交付，再 merged 续接，核对同一 run/PR；仅授权 pr_only 时请求 merged，核对清楚拒绝且无提交/推送；省略 ceiling 核对会话默认；重启重复续接；降低请求核对拒绝。
