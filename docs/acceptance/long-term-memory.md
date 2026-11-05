# 长期记忆（M235）验收账本

规格：`docs/specs/feature-specs/long-term-memory.md`

| 需求 | 状态 | 证据 |
| --- | --- | --- |
| R1 存储：`memories` 表含内容、类型、范围、`repo_identity`、来源、状态、取代关系和时间 | done | 迁移 `src-tauri/migrations/0026_long_term_memory.sql`；`src-tauri/src/agent/long_term_memory.rs` 的 `ensure` 兜底真实库迁移账本落后的情况；`cargo test --lib long_term_memory` 12 项通过 |
| R2 写入：`memory_save` 由服务端取本会话最新 `role=user` 消息作来源，排除系统唤醒 | done | `save_from_user_message` 要求内容逐字出现在来源里；`tools/memory.rs`；测试 T1–T3 与 `memory_save_needs_no_second_approval_in_any_mode` |
| R3 脱敏：命中脱敏规则拒绝入库，报错不回显内容 | done | `save_from_user_message` 调 `trajectory::redact_text`；`long_term_memory` 单元测试断言拒绝且错误里没有原文 |
| R4 查重：相同内容合并，改口时新条目取代旧条目，旧条目保留 | done | `SaveOutcome::Merged`、`replaces` 路径；`long_term_memory` 单元测试覆盖合并、取代和链式删除 |
| R5 注入：常驻索引有硬上限，用户级优先，标注为参考资料，匿名会话不注入 | deferred | 原因：真实请求的断言要改 E2E-001 受保护驱动 `unattended_smoke.rs`，普通 PR 改它会让 Windows 场景整批不开跑 | 后续：M235b 信任根 PR 应用该断言。现有证据：`index_block` 单元测试覆盖上限和排序；提交 `7fd5c4385` 的本机真实二进制运行，接线时通过，断开时报 `long-term memory did not reach provider request 1` |
| R6 记忆页：可列出、查来源、修改、删除、全局关闭，删除是真删，对话里不加系统提示 | done | `src/components/LongTermMemorySection.tsx` 及其测试；`scripts/verify-long-term-memory-headless.mjs` 真浏览器走过浅色、深色 × 1366、375 宽，数据为合成数据 |
| R7 dispatch：`list_memories`、`delete_memory`、`set_memory_enabled` 与界面走同一批命令 | done | `src-tauri/src/headless_dispatch.rs`、`src/lib/desktopDispatch.ts`；`commands/usage_acceptance_tests.rs` 新增用例 |
| R8 兼容：现有 `memory.md`、preferences、Evolution 路径不改、不迁移 | done | 本 PR 未改这些路径；`pnpm cargo:shared -- test --manifest-path src-tauri/Cargo.toml --lib` 2279 项通过，`pnpm test` 1113 项通过 |
