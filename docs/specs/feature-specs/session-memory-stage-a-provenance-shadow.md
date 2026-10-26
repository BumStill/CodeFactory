# MEM-A：会话记忆阶段 A——稳定来源与影子回执

> 状态：已实现（阶段 A）。上游设计：`docs/specs/feature-specs/session-memory-design.md`（§13.1 阶段 A、§13.2 决策 D1–D5）。
> 范围：只做「排序身份」与「装配回执」两件事。不改变现有对话、续跑、压缩与权限行为。

## 背景与已定决策

设计文档 `session-memory-design.md` 记录了会话记忆的分层恢复设计。用户 2026-10-26 拍板 D1–D5 全部按推荐默认执行（已回写设计文档 §13.2，标注「已决：用户 10-26 按推荐」）：

- **D1** 不自动删唯一原文；可归档、显式删除。
- **D2** 项目内显式晋升；跨项目默认关闭。
- **D3** 先确定性关键事实提取，模型摘要可选且有预算上限；不自动把历史外发到新 scope。
- **D4** 原文可回读 + 关键事实硬断言 + 随机化真实模型续接评测。
- **D5** GUI 与 dispatch 同源；可查来源、撤销、重建；技术修复不要求用户点继续。

阶段 A 是这条路线的最小前置：在改动投影/摘要之前，先把「一条消息在会话里是第几条、第几版」变成**持久且可引用的身份**，并让每次请求装配留下**只读回执**。没有这两件事，后续任何摘要都无从证明「模型实际看到的和回执说的一致」。

### 现状问题（真实缺陷）

1. `messages` 表只有 `created_at`（秒级）。同一秒内产生的多行靠 SQLite 隐式 `rowid` 决定先后，而 `messages` 的主键是 `id TEXT`，不是 `INTEGER PRIMARY KEY`——`rowid` 在 `VACUUM` 后不保证保持。也就是说「第 7 条消息」今天不是一个稳定事实。
2. 请求装配（`build_openai_messages`）会**丢弃**被 completion gate 撤回的候选回答、并**合成**缺失的工具结果（`repair_incomplete_tool_history` 生成 `recovered-tool-*`）。装配后的线上消息与数据库行不再一一对应，事后无法回答「这次请求到底发了哪几条、基于哪些版本」。

## 需求与追溯（Requirements Traceability）

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| R1 | 为现有消息排序建立稳定 `seq`（会话内单调、不与并发写冲突）与 `revision`（内容版本）；旧库回填、匿名/导入会话同样适用 | 迁移后旧库 `seq` 稳定单调且可重复读取；并发插入无重复/无乱序；`revision` 随内容更新递增 |
| R2 | 每次组装模型请求时记录 assembly manifest：用了哪些消息（id/seq/revision）、哪些被排除及原因、哪些是装配期合成、线上指纹；**只记录不改变请求** | 回执条目与线上 `ChatMessage` 逐项一致；被排除项带原因；开启前后请求字节相同 |
| R3 | 旧路径仍是唯一执行者：不双跑模型、不双跑工具、manifest 不做任何投影 | 装配代码单一路径（回执与请求由同一次装配产出）；回执写入失败不影响本轮请求 |
| R4 | 先失败的测试覆盖四项：旧库迁移后 `seq` 稳定单调；并发写不乱序；manifest 与实际发送逐项一致；flag 关/开时请求字节相同 | 以下测试在实现前为红，实现后为绿 |
| R5 | 迁移非破坏（无 down migration 删数据）；按 M162 补验收账本 | 迁移只做 `ADD COLUMN` / `CREATE TABLE` / 索引与回填；`docs/acceptance/m162.md` 增加本轮账本条目 |

### 非目标（本阶段明确不做）

- 不做摘要、不做归档、不做投影替换（阶段 B/C）。
- 不改变 `replayable_history` / `repair_incomplete_tool_history` / completion gate 的任何取舍。
- 不新增用户可见 UI；影子回执不是产品能力，开关是灰度开关而非用户设置。

## Applicable Harnesses

| Harness | 是否启用 | 覆盖方式 |
| --- | --- | --- |
| Spec Harness | 是 | 本文件 + R1–R5 追溯表 + PR 自验收表 |
| Compatibility Harness | 是 | 旧库（仅 0024 及以前）→ 迁移 0025 的回填测试；新增列对旧查询/旧测试 fixture 非破坏 |
| Observation Harness | 是 | `assembly_manifests` 回执表 + `feature_flags` 开关状态行 |
| Payload Harness | 是 | R4「flag 关/开请求字节相同」：序列化后逐字节比较 |
| Viewport Harness | 否 | 无 UI 改动 |
| AI Collaboration Harness | 是 | context scope / assumptions / review point / validation result 记入 PR（模板禁止 `delegate_tasks`，本轮单执行流） |

## 测试矩阵

| 测试 | 覆盖 Req | 断言要点 |
| --- | --- | --- |
| `legacy_backfill_assigns_stable_monotonic_seq` | R1, R4 | 0024 状态插入同秒多行（依赖 rowid 顺序），迁移后按 `created_at,rowid` 回填 1..N；重复读取不变 |
| `backfill_is_idempotent_across_reopen` | R1, R5 | 迁移再次运行不重排、不改 `seq`，无数据丢失 |
| `concurrent_inserts_get_unique_ordered_seq` | R1, R4 | 多连接并发插入后 `seq` 唯一、严格递增，按 `seq` 复现插入顺序 |
| `revision_increments_when_content_changes` | R1 | 内容更新后 `revision+1`；无关列更新不递增 |
| `manifest_matches_sent_wire_item_by_item` | R2, R4 | 回执每项（角色 + 指纹）与线上 `ChatMessage` 逐项相等；被排除的 `rejected_candidate` 带原因；`recovered-tool-*` 标为装配期合成 |
| `shadow_flag_off_and_on_produce_identical_request_bytes` | R2, R3, R4 | 两种开关状态下序列化请求体字节完全相同 |
| `manifest_write_failure_does_not_break_assembly` | R3 | 回执写入失败时装配返回值不变（Turn 不受影响） |
| `flag_defaults_off_when_absent` | R2 | `feature_flags` 无行 / 值为 0 时视为关闭 |

## Implementation Notes

- 迁移 `0025_message_sequence.sql`：`ALTER TABLE messages ADD COLUMN seq INTEGER`、`ADD COLUMN revision INTEGER NOT NULL DEFAULT 1`；按 `ROW_NUMBER() OVER (PARTITION BY session_id ORDER BY created_at, rowid)` 回填；`CREATE INDEX messages_session_seq`。新写入由 `AFTER INSERT` 触发器补 `seq = MAX(seq)+1`（会话内，写事务内计算，SQLite 单写者串行化即保证无重复）；`AFTER UPDATE` 触发器在内容相关列变化时 `revision+1`。
- `Message` 结构新增 `seq: Option<i64>`、`revision: i64`，两者 `#[serde(default)]` + `#[sqlx(default)]`：旧测试 fixture 中手写 `messages` 表（无新列）仍可读，迁移后生产库才有值。
- `load_agent_history` 排序从 `(created_at, rowid)` 改为 `seq`（旧行已回填），`created_at`/`rowid` 只作为回填依据与兜底。
- 装配回执：`build_openai_messages` 改为 `assemble_with_provenance` 的薄包装，回执与请求由**同一次**装配产出；`record_shadow_manifest` 在开关关闭时**直接返回**（不查额外数据、不写库），开启时写 `assembly_manifests` 并吞掉错误。
- 开关：`feature_flags('memory_assembly_manifest')`，缺省为关；无 GUI 依赖，headless 可用 `CODEFACTORY_MEMORY_MANIFEST_SHADOW=1` 或直接写表启用。默认关闭意味着线上请求路径与当前版本字节一致。
